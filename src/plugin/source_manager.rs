use anyhow::anyhow;

use crate::error::{Error, Result};
use mlua::prelude::*;
use sea_orm::DatabaseConnection;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use crate::model::repo;
use crate::probe::media::{AvFormatProbe, MediaProbe};
use crate::wallpaper::types::{WallpaperEntry, WallpaperType};

/// User-Agent the `ctx.http` default client sends.
const WAYWALLEN_HTTP_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) waywallen";
const LUA_CALLBACK_TIMEOUT: Duration = Duration::from_secs(25);
const LUA_HOOK_INSTRUCTION_INTERVAL: u32 = 10_000;
const RUNTIME_ACTIVE: u8 = 0;
const RUNTIME_DRAINING: u8 = 1;
const RUNTIME_INACTIVE: u8 = 2;

pub const ENTRY_VERSION_V2: u32 = 2;
pub const ENTRY_VERSION_V3: u32 = 3;
pub const ENTRY_VERSION: u32 = ENTRY_VERSION_V2;
pub const LATEST_ENTRY_VERSION: u32 = ENTRY_VERSION_V3;
pub const SUPPORTED_ENTRY_VERSIONS: &[u32] = &[ENTRY_VERSION_V2, ENTRY_VERSION_V3];

pub fn supports_entry_version(version: u32) -> bool {
    SUPPORTED_ENTRY_VERSIONS.contains(&version)
}

fn resolve_plugin_import(root: &Path, name: &str) -> LuaResult<PathBuf> {
    let mut rel = PathBuf::new();
    for part in name.split('.') {
        if part.is_empty()
            || part == ".."
            || part.contains('/')
            || part.contains('\\')
            || part == "."
        {
            return Err(LuaError::RuntimeError(format!(
                "invalid import module name: {name}"
            )));
        }
        rel.push(part);
    }

    let candidates = [
        root.join(&rel).with_extension("lua"),
        root.join(&rel).join("init.lua"),
    ];
    for candidate in candidates {
        if !candidate.is_file() {
            continue;
        }
        let path = candidate.canonicalize().map_err(LuaError::external)?;
        if path.starts_with(root) {
            return Ok(path);
        }
    }

    Err(LuaError::RuntimeError(format!("module not found: {name}")))
}

// ---------------------------------------------------------------------------
// Public types

#[derive(Debug, Clone, serde::Serialize)]
pub struct SourcePluginInfo {
    pub name: String,
    /// Domain id of the owning installable plugin.
    /// Empty when loaded without package metadata.
    pub plugin_id: String,
    pub types: Vec<WallpaperType>,
    pub version: String,
    /// Short UI label or placeholder for prompting a library path.
    /// Empty when the plugin did not declare one.
    pub library_label: String,
    /// Longer helper text for choosing a library path.
    /// May contain newlines or inline-code Markdown markers.
    pub library_hint: String,
    /// User-configurable settings the plugin declares via `info().settings`,
    /// stored under the Lua source name in the shared component settings map.
    pub settings: Vec<SourceSetting>,
}

/// One entry from a source plugin's `info().settings` sequence. Shapes the
/// same UI widgets as renderer `[settings]`, but declared in Lua so a source
/// plugin keeps all of its surface in one place.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceSetting {
    pub key: String,
    /// "string" | "bool" | "u32" | "i32" | "f32".
    pub ty: String,
    pub default: String,
    /// Human-readable label and help text (shown verbatim; no i18n yet).
    pub label: String,
    pub description: String,
    pub group: String,
    pub order: i32,
    pub choices: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceActionKind {
    Invoke,
    QrLogin,
    Form,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceActionField {
    pub key: String,
    pub label: String,
    pub description: String,
    pub placeholder: String,
    pub secret: bool,
    pub required: bool,
}

/// A button declared by `info().actions` and routed through the generic plugin
/// action contract.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceAction {
    pub id: String,
    pub label: String,
    pub description: String,
    pub browse_description: String,
    pub browse_button_label: String,
    pub group: String,
    pub order: i32,
    pub kind: SourceActionKind,
    pub visible: bool,
    pub enabled: bool,
    pub fields: Vec<SourceActionField>,
    pub required_for_browsing: bool,
}

/// A read-only status row a source plugin declares via `info().status`. The
/// `value` is computed by the daemon at query time (e.g. "Signed in as X").
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceStatus {
    pub id: String,
    pub label: String,
    pub group: String,
    pub order: i32,
    pub value: String,
}

/// One sort option a discover-capable plugin advertises via
/// `info().capabilities.discover.sorts`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiscoverSort {
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverFilterType {
    Select,
    MultiSelect,
    Toggle,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DiscoverFilter {
    pub id: String,
    pub title: String,
    pub ty: DiscoverFilterType,
    pub values: Vec<String>,
    pub value_labels: Vec<String>,
    pub description: String,
    pub confirmation: String,
}

/// Discover capability of a single source plugin, derived from
/// `info().capabilities.discover`. Plugins without that table are not listed.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiscoverSourceInfo {
    /// Discover entry name — the routing key clients echo back in
    /// `DiscoverSearchRequest.plugin_id`.
    pub plugin_id: String,
    pub name: String,
    /// Human-readable display name (falls back to `name`).
    pub display_name: String,
    pub supports_search: bool,
    pub remote_capability: Option<RemoteCapability>,
    pub remote_hint: String,
    pub sorts: Vec<DiscoverSort>,
    pub filters: Vec<DiscoverFilter>,
    /// Domain id of the owning installable plugin (e.g.
    /// `org.waywallen.open-wallpaper-engine`). Source settings remain keyed by
    /// `plugin_id`, the Lua source name.
    pub owner_plugin_id: String,
    /// User-configurable settings the plugin declares via `info().settings`.
    pub settings: Vec<SourceSetting>,
    /// Action buttons the plugin declares via `info().actions`.
    pub actions: Vec<SourceAction>,
    /// Status rows the plugin declares via `info().status` (values daemon-filled).
    pub status: Vec<SourceStatus>,
    /// Provider-owned account image returned by `lifecycle.check()`.
    pub avatar_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteCapability {
    Download,
    Subscription,
}

fn validate_source_setting(
    setting: &SourceSetting,
    raw: &str,
) -> std::result::Result<String, String> {
    let value = match setting.ty.as_str() {
        "u32" => raw
            .parse::<u32>()
            .map(|value| value.to_string())
            .map_err(|_| ()),
        "i32" => raw
            .parse::<i32>()
            .map(|value| value.to_string())
            .map_err(|_| ()),
        "f32" => raw
            .parse::<f32>()
            .map(|value| value.to_string())
            .map_err(|_| ()),
        "bool" => match raw {
            "true" | "false" => Ok(raw.to_string()),
            _ => Err(()),
        },
        "string" => Ok(raw.to_string()),
        other => {
            return Err(format!("{}.type '{other}' is unsupported", setting.key));
        }
    }
    .map_err(|_| format!("{} expects {}, got '{raw}'", setting.key, setting.ty))?;

    if !setting.choices.is_empty() && !setting.choices.iter().any(|choice| choice == &value) {
        return Err(format!(
            "{} value '{value}' is not one of [{}]",
            setting.key,
            setting.choices.join(", ")
        ));
    }
    Ok(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionState {
    Unknown,
    Unsubscribed,
    Subscribed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionItemState {
    pub id: String,
    pub state: SubscriptionState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrLoginBegin {
    pub operation_id: u64,
    pub challenge: String,
    pub poll_after_ms: u64,
    pub expires_in_ms: Option<u64>,
    pub title: String,
    pub instruction: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QrLoginPollState {
    AwaitingScan,
    AwaitingConfirmation,
    ChallengeChanged,
    Succeeded,
    Expired,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrLoginPoll {
    pub state: QrLoginPollState,
    pub challenge: String,
    pub poll_after_ms: Option<u64>,
    pub display_value: String,
    pub error: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginLifecycleState {
    SignedOut,
    SignedIn,
    Expired,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginLifecycleCheck {
    pub state: PluginLifecycleState,
    pub display_value: String,
    pub error: String,
    pub avatar_url: String,
}

/// One remote item returned by a plugin's `discover.search(ctx, params)`.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DiscoverItem {
    pub id: String,
    pub title: String,
    pub preview_url: String,
    pub author: String,
    pub wp_type: String,
    pub extra: HashMap<String, String>,
}

/// Detail blob returned by a plugin's `discover.details(ctx, id)`.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DiscoverDetails {
    pub author: String,
    pub description: String,
    pub size: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub tags: Vec<String>,
    pub web_url: String,
    pub extra: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DiscoverSearchResult {
    pub items: Vec<DiscoverItem>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DiscoverDownload {
    pub wp_type: String,
    pub url: String,
    pub filename: String,
    pub title: String,
    pub preview_url: String,
    pub description: String,
    pub tags: Vec<String>,
    pub external_id: String,
    pub size: Option<i64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub content_rating: Option<String>,
}

/// Directory item resolved by `discover.resolve` after a provider fetch.
/// Paths are relative to the fetched directory.
#[derive(Debug, Clone, Default)]
pub struct DiscoverResolve {
    pub name: String,
    pub wp_type: String,
    pub resource: String,
    pub preview: Option<String>,
    pub description: String,
    pub tags: Vec<String>,
    pub external_id: String,
    pub size: Option<i64>,
    pub content_rating: Option<String>,
}

#[derive(Debug, Clone)]
struct SourceCapability {
    types: Vec<WallpaperType>,
    library_label: String,
    library_hint: String,
    auto_detect: bool,
}

#[derive(Debug, Clone)]
struct DiscoverCapability {
    supports_search: bool,
    supports_details: bool,
    supports_download: bool,
    supports_resolve: bool,
    remote: Option<RemoteCapability>,
    remote_hint: String,
    sorts: Vec<DiscoverSort>,
    filters: Vec<DiscoverFilter>,
    /// The plugin exposes `discover.tags(ctx)`; the daemon calls it to refresh
    /// a legacy multi-select filter from a live source.
    dynamic_tags: bool,
}

#[derive(Debug, Clone, Default)]
struct WallpaperCapability {
    extras: bool,
    properties: bool,
}

#[derive(Debug, Clone, Default)]
struct PluginCapabilities {
    source: Option<SourceCapability>,
    source_item_remove: bool,
    discover: Option<DiscoverCapability>,
    wallpaper: WallpaperCapability,
    lifecycle: bool,
}

#[derive(Debug, Clone)]
struct PluginStateMigration {
    schema_id: String,
    file: String,
}

#[derive(Debug, Clone)]
struct LoadedPluginInfo {
    name: String,
    /// Human-readable display name from `info().display_name`; falls back to
    /// `name` when unset.
    display_name: String,
    plugin_id: String,
    version: String,
    capabilities: PluginCapabilities,
    settings: Vec<SourceSetting>,
    actions: Vec<SourceAction>,
    status: Vec<SourceStatus>,
    state_migrations: Vec<PluginStateMigration>,
}

#[derive(Default)]
struct PluginCallbacks {
    source_scan: Option<LuaRegistryKey>,
    source_auto_detect: Option<LuaRegistryKey>,
    source_remove: Option<LuaRegistryKey>,
    discover_search: Option<LuaRegistryKey>,
    discover_tags: Option<LuaRegistryKey>,
    discover_details: Option<LuaRegistryKey>,
    discover_download: Option<LuaRegistryKey>,
    discover_resolve: Option<LuaRegistryKey>,
    wallpaper_extras: Option<LuaRegistryKey>,
    wallpaper_properties: Option<LuaRegistryKey>,
    lifecycle_load: Option<LuaRegistryKey>,
    lifecycle_save: Option<LuaRegistryKey>,
    lifecycle_check: Option<LuaRegistryKey>,
    lifecycle_migrate: Option<LuaRegistryKey>,
    actions_status: Option<LuaRegistryKey>,
    actions_invoke: Option<LuaRegistryKey>,
    qrlogin_begin: Option<LuaRegistryKey>,
    qrlogin_poll: Option<LuaRegistryKey>,
    qrlogin_cancel: Option<LuaRegistryKey>,
    subscription_status: Option<LuaRegistryKey>,
    subscription_subscribe: Option<LuaRegistryKey>,
    subscription_unsubscribe: Option<LuaRegistryKey>,
}

struct CallbackDeadlineGuard {
    deadline: Arc<StdMutex<Option<Instant>>>,
}

impl Drop for CallbackDeadlineGuard {
    fn drop(&mut self) {
        if let Ok(mut deadline) = self.deadline.lock() {
            *deadline = None;
        }
    }
}

// ---------------------------------------------------------------------------
// LuaPluginRuntime

pub struct LuaPluginRuntime {
    lua: Lua,
    callback_deadline: Arc<StdMutex<Option<Instant>>>,
    callback_timeout: Duration,
    /// plugin name → registry key for the loaded module table.
    plugins: HashMap<String, LuaRegistryKey>,
    callbacks: HashMap<String, PluginCallbacks>,
    /// source `info().name` → parsed ABI v2 metadata.
    plugin_infos: HashMap<String, LoadedPluginInfo>,
    /// Flattened scan results from all plugins.
    entries: Vec<WallpaperEntry>,
    /// Index: wp_type → indices into `entries`.
    by_type: HashMap<WallpaperType, Vec<usize>>,
    /// Shared media probe exposed to Lua via ctx.probe(path).
    probe: Arc<dyn MediaProbe>,
    /// DB used by the `ctx.library_meta_*` async-Lua-function bridge.
    /// `None` makes the bridge no-op, which is useful for DB-less tests.
    db: Option<DatabaseConnection>,
    /// Settings store backing `ctx.plugin_config(key)`. `None` in DB-less tests.
    settings: Option<Arc<crate::settings::SettingsStore>>,
    state_store: crate::plugin::state_store::PluginStateStore,
    saved_states: HashMap<String, Option<String>>,
    http_client: reqwest::Client,
    http_cookie_store: Arc<mlua_extra::http::SessionCookieStore>,
    generation_state: Arc<AtomicU8>,
    qr_operations: HashMap<u64, LuaRegistryKey>,
    next_qr_operation_id: u64,
}

// mlua with the `send` feature makes Lua: Send.
// We wrap SourceManager in Arc<TokioMutex<>> so this is required.
fn assert_lua_plugin_runtime_send() {
    fn assert_send<T: Send>() {}
    assert_send::<LuaPluginRuntime>();
}
const _: fn() = assert_lua_plugin_runtime_send;

impl LuaPluginRuntime {
    pub fn new() -> Result<Self> {
        Self::with_probe(Arc::new(AvFormatProbe::new()))
    }

    pub fn with_probe(probe: Arc<dyn MediaProbe>) -> Result<Self> {
        let lua = Lua::new();
        let http_cookie_store = Arc::new(mlua_extra::http::SessionCookieStore::default());
        let http_client = reqwest::Client::builder()
            .user_agent(WAYWALLEN_HTTP_USER_AGENT)
            .cookie_provider(http_cookie_store.clone())
            .build()
            .map_err(|error| Error::Internal(anyhow!("build plugin HTTP client: {error}")))?;
        let callback_deadline = Arc::new(StdMutex::new(None));
        let hook_deadline = callback_deadline.clone();
        lua.set_hook(
            LuaHookTriggers::new().every_nth_instruction(LUA_HOOK_INSTRUCTION_INTERVAL),
            move |_, _| {
                let expired = hook_deadline
                    .lock()
                    .map_err(|_| LuaError::RuntimeError("Lua callback deadline poisoned".into()))?
                    .is_some_and(|deadline| Instant::now() >= deadline);
                if expired {
                    Err(LuaError::RuntimeError("Lua callback timed out".into()))
                } else {
                    Ok(LuaVmState::Continue)
                }
            },
        );
        Ok(Self {
            lua,
            callback_deadline,
            callback_timeout: LUA_CALLBACK_TIMEOUT,
            plugins: HashMap::new(),
            callbacks: HashMap::new(),
            plugin_infos: HashMap::new(),
            entries: Vec::new(),
            by_type: HashMap::new(),
            probe,
            db: None,
            settings: None,
            state_store: crate::plugin::state_store::PluginStateStore::standard(),
            saved_states: HashMap::new(),
            http_client,
            http_cookie_store,
            generation_state: Arc::new(AtomicU8::new(RUNTIME_ACTIVE)),
            qr_operations: HashMap::new(),
            next_qr_operation_id: 1,
        })
    }

    fn arm_callback_deadline(&self) -> Result<CallbackDeadlineGuard> {
        let mut deadline = self
            .callback_deadline
            .lock()
            .map_err(|_| Error::Internal(anyhow!("Lua callback deadline poisoned")))?;
        *deadline = Some(Instant::now() + self.callback_timeout);
        drop(deadline);
        Ok(CallbackDeadlineGuard {
            deadline: self.callback_deadline.clone(),
        })
    }

    fn call_callback<R>(&self, function: &LuaFunction, args: impl IntoLuaMulti) -> LuaResult<R>
    where
        R: FromLuaMulti,
    {
        let _deadline = self.arm_callback_deadline().map_err(LuaError::external)?;
        function.call(args)
    }

    async fn call_callback_async<R>(
        &self,
        function: &LuaFunction,
        args: impl IntoLuaMulti,
    ) -> LuaResult<R>
    where
        R: FromLuaMulti,
    {
        let _deadline = self.arm_callback_deadline().map_err(LuaError::external)?;
        let hook_deadline = self.callback_deadline.clone();
        let thread = self.lua.create_thread(function.clone())?;
        thread.set_hook(
            LuaHookTriggers::new().every_nth_instruction(LUA_HOOK_INSTRUCTION_INTERVAL),
            move |_, _| {
                let expired = hook_deadline
                    .lock()
                    .map_err(|_| LuaError::RuntimeError("Lua callback deadline poisoned".into()))?
                    .is_some_and(|deadline| Instant::now() >= deadline);
                if expired {
                    Err(LuaError::RuntimeError("Lua callback timed out".into()))
                } else {
                    Ok(LuaVmState::Continue)
                }
            },
        );
        tokio::time::timeout(self.callback_timeout, thread.into_async::<R>(args))
            .await
            .map_err(|_| LuaError::RuntimeError("Lua callback timed out".into()))?
    }

    async fn call_plugin_callback_async<R>(
        &self,
        plugin_name: &str,
        function: &LuaFunction,
        args: impl IntoLuaMulti,
    ) -> LuaResult<R>
    where
        R: FromLuaMulti,
    {
        if self.generation_state.load(Ordering::Acquire) != RUNTIME_ACTIVE {
            return Err(LuaError::runtime("source plugin runtime is reloading"));
        }
        let result = self.call_callback_async(function, args).await;
        let persisted = self.persist_http_session(plugin_name);
        match (result, persisted) {
            (Ok(value), Ok(())) => Ok(value),
            (Ok(_), Err(error)) => Err(LuaError::external(error)),
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(persist_error)) => {
                log::warn!(
                    "persist HTTP session after failed callback for {plugin_name}: {persist_error:#}"
                );
                Err(error)
            }
        }
    }

    fn http_view(&self) -> mlua_extra::http::LuaHttpClient {
        mlua_extra::http::LuaHttpClient::with_session(
            self.http_client.clone(),
            self.http_cookie_store.clone(),
        )
    }

    fn restore_http_session(&self, plugin_id: &str) -> Result<()> {
        let Some(snapshot) = self.state_store.load_http_session(plugin_id)? else {
            return Ok(());
        };
        self.http_cookie_store.restore(&snapshot).map_err(|error| {
            Error::Internal(anyhow!("restore HTTP session for {plugin_id}: {error}"))
        })
    }

    fn persist_http_session(&self, plugin_name: &str) -> Result<()> {
        let plugin_id = &self
            .plugin_infos
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?
            .plugin_id;
        self.persist_http_session_by_id(plugin_id)
    }

    fn persist_http_session_by_id(&self, plugin_id: &str) -> Result<()> {
        if !self.http_cookie_store.is_dirty() {
            return Ok(());
        }
        let snapshot = self.http_cookie_store.snapshot().map_err(|error| {
            Error::Internal(anyhow!("snapshot HTTP session for {plugin_id}: {error}"))
        })?;
        self.state_store
            .save_http_session(plugin_id, &snapshot.value)?;
        self.http_cookie_store.mark_clean(snapshot.revision);
        Ok(())
    }

    /// Hand the DB to the source manager so `ctx.library_meta_get/set`
    /// can read and write per-library metadata.
    pub fn attach_db(&mut self, db: DatabaseConnection) {
        self.db = Some(db);
    }

    /// Hand the settings store to the source manager so `ctx.plugin_config(key)`
    /// can read the Lua source component's config table.
    pub fn attach_settings(&mut self, settings: Arc<crate::settings::SettingsStore>) {
        self.settings = Some(settings);
    }

    pub fn clear_plugins(&mut self) {
        self.persist_all_http_sessions();
        self.generation_state
            .store(RUNTIME_INACTIVE, Ordering::Release);
        self.http_cookie_store.clear();
        self.plugins.clear();
        self.callbacks.clear();
        self.plugin_infos.clear();
        self.saved_states.clear();
        self.qr_operations.clear();
        self.entries.clear();
        self.by_type.clear();
    }

    fn plugin_lua_env(&self, root: &Path) -> Result<LuaTable> {
        let root = root
            .canonicalize()
            .map_err(|e| Error::Internal(anyhow!("canonicalize {}: {e}", root.display())))?;
        let root = Arc::new(root);
        let cache: Arc<StdMutex<HashMap<PathBuf, LuaRegistryKey>>> =
            Arc::new(StdMutex::new(HashMap::new()));

        let env = self.lua.create_table()?;
        let mt = self.lua.create_table()?;
        mt.set("__index", self.lua.globals())?;
        env.set_metatable(Some(mt));

        let import_env = env.clone();
        let import_root = root.clone();
        let import_cache = cache.clone();
        let import_fn = self.lua.create_function(move |lua, name: String| {
            let path = resolve_plugin_import(&import_root, &name)?;
            {
                let cache = import_cache
                    .lock()
                    .map_err(|_| LuaError::RuntimeError("import cache poisoned".to_string()))?;
                if let Some(key) = cache.get(&path) {
                    return lua.registry_value::<LuaValue>(key);
                }
            }

            let source = std::fs::read_to_string(&path).map_err(LuaError::external)?;
            let value: LuaValue = lua
                .load(&source)
                .set_name(path.to_string_lossy())
                .set_environment(import_env.clone())
                .eval()?;
            let key = lua.create_registry_value(value)?;
            let mut cache = import_cache
                .lock()
                .map_err(|_| LuaError::RuntimeError("import cache poisoned".to_string()))?;
            cache.insert(path.clone(), key);
            lua.registry_value::<LuaValue>(cache.get(&path).expect("cached import"))
        })?;
        env.set("import", import_fn)?;
        Ok(env)
    }

    fn require_string(tbl: &LuaTable, key: &str, context: &str) -> Result<String> {
        tbl.get::<String>(key)
            .map_err(|e| Error::Internal(anyhow!("{context}.{key} required: {e}")))
    }

    fn optional_string(tbl: &LuaTable, key: &str, context: &str) -> Result<String> {
        match tbl
            .get::<LuaValue>(key)
            .map_err(|e| Error::Internal(anyhow!("{context}.{key}: {e}")))?
        {
            LuaValue::Nil => Ok(String::new()),
            LuaValue::String(s) => s
                .to_str()
                .map(|cow| cow.to_string())
                .map_err(|e| Error::Internal(anyhow!("{context}.{key} invalid string: {e}"))),
            other => Err(Error::Internal(anyhow!(
                "{context}.{key} must be a string, got {}",
                other.type_name()
            ))),
        }
    }

    fn require_string_sequence(tbl: &LuaTable, key: &str, context: &str) -> Result<Vec<String>> {
        let values: LuaTable = tbl
            .get(key)
            .map_err(|e| Error::Internal(anyhow!("{context}.{key} required: {e}")))?;
        let mut out = Vec::new();
        for (idx, value) in values.sequence_values::<String>().enumerate() {
            out.push(value.map_err(|e| {
                Error::Internal(anyhow!(
                    "{context}.{key}[{}] must be a string: {e}",
                    idx + 1
                ))
            })?);
        }
        Ok(out)
    }

    fn optional_string_sequence(tbl: &LuaTable, key: &str, context: &str) -> Result<Vec<String>> {
        let Some(values) = Self::optional_table(tbl, key, context)? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for (idx, value) in values.sequence_values::<String>().enumerate() {
            out.push(value.map_err(|e| {
                Error::Internal(anyhow!(
                    "{context}.{key}[{}] must be a string: {e}",
                    idx + 1
                ))
            })?);
        }
        Ok(out)
    }

    fn optional_discover_sorts(discover_tbl: &LuaTable) -> Result<Vec<DiscoverSort>> {
        let Some(sorts_tbl) =
            Self::optional_table(discover_tbl, "sorts", "info().capabilities.discover")?
        else {
            return Ok(Vec::new());
        };
        let mut sorts = Vec::new();
        for (idx, sort) in sorts_tbl.sequence_values::<LuaTable>().enumerate() {
            let sort = sort.map_err(|e| {
                Error::Internal(anyhow!(
                    "info().capabilities.discover.sorts[{}] must be a table: {e}",
                    idx + 1
                ))
            })?;
            let context = format!("info().capabilities.discover.sorts[{}]", idx + 1);
            let key = Self::require_string(&sort, "key", &context)?;
            let label = Self::require_string(&sort, "label", &context)?;
            if key.is_empty() || label.is_empty() {
                return Err(Error::Internal(anyhow!(
                    "{context}.key and {context}.label must not be empty"
                )));
            }
            sorts.push(DiscoverSort { key, label });
        }
        Ok(sorts)
    }

    fn legacy_discover_filter(tags: Vec<String>) -> Vec<DiscoverFilter> {
        if tags.is_empty() {
            return Vec::new();
        }
        vec![DiscoverFilter {
            id: "tags".to_string(),
            title: "Tags".to_string(),
            ty: DiscoverFilterType::MultiSelect,
            values: tags,
            value_labels: Vec::new(),
            description: String::new(),
            confirmation: String::new(),
        }]
    }

    fn optional_discover_filters(discover_tbl: &LuaTable) -> Result<Option<Vec<DiscoverFilter>>> {
        let Some(filters_tbl) =
            Self::optional_table(discover_tbl, "filters", "info().capabilities.discover")?
        else {
            return Ok(None);
        };

        let mut filters = Vec::new();
        let mut filter_ids = HashSet::new();
        let mut filter_values = HashSet::new();
        for (idx, filter) in filters_tbl.sequence_values::<LuaTable>().enumerate() {
            let filter = filter.map_err(|error| {
                Error::Internal(anyhow!(
                    "info().capabilities.discover.filters[{}] must be a table: {error}",
                    idx + 1
                ))
            })?;
            let context = format!("info().capabilities.discover.filters[{}]", idx + 1);
            let id = Self::require_string(&filter, "id", &context)?;
            let title = Self::require_string(&filter, "title", &context)?;
            if id.is_empty() || title.is_empty() {
                return Err(Error::Internal(anyhow!(
                    "{context}.id and {context}.title must not be empty"
                )));
            }
            if !filter_ids.insert(id.clone()) {
                return Err(Error::Internal(anyhow!(
                    "{context}.id '{id}' is declared more than once"
                )));
            }

            let type_name = Self::require_string(&filter, "type", &context)?;
            let ty = match type_name.as_str() {
                "select" => DiscoverFilterType::Select,
                "multi_select" => DiscoverFilterType::MultiSelect,
                "toggle" => DiscoverFilterType::Toggle,
                _ => {
                    return Err(Error::Internal(anyhow!(
                        "{context}.type '{type_name}' must be select, multi_select, or toggle"
                    )))
                }
            };
            let values = Self::require_string_sequence(&filter, "values", &context)?;
            let value_labels = Self::optional_string_sequence(&filter, "value_labels", &context)?;
            if values.is_empty() {
                return Err(Error::Internal(anyhow!(
                    "{context}.values must not be empty"
                )));
            }
            if ty == DiscoverFilterType::Toggle && values.len() != 1 {
                return Err(Error::Internal(anyhow!(
                    "{context}.values must contain exactly one value for a toggle"
                )));
            }
            if !value_labels.is_empty() && value_labels.len() != values.len() {
                return Err(Error::Internal(anyhow!(
                    "{context}.value_labels must be empty or contain one label per value"
                )));
            }
            if value_labels.iter().any(String::is_empty) {
                return Err(Error::Internal(anyhow!(
                    "{context}.value_labels must not contain an empty label"
                )));
            }
            for value in &values {
                if value.is_empty() {
                    return Err(Error::Internal(anyhow!(
                        "{context}.values must not contain an empty value"
                    )));
                }
                if !filter_values.insert(value.clone()) {
                    return Err(Error::Internal(anyhow!(
                        "{context}.values contains duplicate discover value '{value}'"
                    )));
                }
            }

            let description = Self::optional_string(&filter, "description", &context)?;
            let confirmation = Self::optional_string(&filter, "confirmation", &context)?;
            if !confirmation.is_empty() && ty != DiscoverFilterType::Toggle {
                return Err(Error::Internal(anyhow!(
                    "{context}.confirmation is only supported for toggle filters"
                )));
            }
            filters.push(DiscoverFilter {
                id,
                title,
                ty,
                values,
                value_labels,
                description,
                confirmation,
            });
        }
        Ok(Some(filters))
    }

    fn validate_discover_filter_values(
        plugin_name: &str,
        discover: &DiscoverCapability,
        values: &[String],
    ) -> Result<()> {
        let mut selected = HashSet::new();
        for value in values {
            if !selected.insert(value.as_str()) {
                return Err(Error::InvalidArgument(format!(
                    "source plugin '{plugin_name}' discover filter value '{value}' is duplicated"
                )));
            }
        }
        for value in &selected {
            if !discover
                .filters
                .iter()
                .any(|filter| filter.values.iter().any(|candidate| candidate == *value))
            {
                return Err(Error::InvalidArgument(format!(
                    "source plugin '{plugin_name}' does not declare discover filter value '{value}'"
                )));
            }
        }
        for filter in &discover.filters {
            if filter.ty == DiscoverFilterType::Select
                && filter
                    .values
                    .iter()
                    .filter(|value| selected.contains(value.as_str()))
                    .count()
                    > 1
            {
                return Err(Error::InvalidArgument(format!(
                    "source plugin '{plugin_name}' discover filter '{}' accepts one value",
                    filter.id
                )));
            }
        }
        Ok(())
    }

    fn require_table_function(tbl: &LuaTable, fn_name: &str, context: &str) -> Result<()> {
        tbl.get::<LuaFunction>(fn_name)
            .map(|_| ())
            .map_err(|e| Error::Internal(anyhow!("{context}.{fn_name} required: {e}")))
    }

    fn require_module_table(module: &LuaTable, name: &str) -> Result<LuaTable> {
        module
            .get::<LuaTable>(name)
            .map_err(|e| Error::Internal(anyhow!("module.{name} table required: {e}")))
    }

    fn optional_table(tbl: &LuaTable, key: &str, context: &str) -> Result<Option<LuaTable>> {
        match tbl
            .get::<LuaValue>(key)
            .map_err(|e| Error::Internal(anyhow!("{context}.{key}: {e}")))?
        {
            LuaValue::Nil => Ok(None),
            LuaValue::Table(t) => Ok(Some(t)),
            other => Err(Error::Internal(anyhow!(
                "{context}.{key} must be a table, got {}",
                other.type_name()
            ))),
        }
    }

    fn optional_bool(tbl: &LuaTable, key: &str, context: &str, default: bool) -> Result<bool> {
        match tbl
            .get::<LuaValue>(key)
            .map_err(|e| Error::Internal(anyhow!("{context}.{key}: {e}")))?
        {
            LuaValue::Nil => Ok(default),
            LuaValue::Boolean(v) => Ok(v),
            other => Err(Error::Internal(anyhow!(
                "{context}.{key} must be a boolean, got {}",
                other.type_name()
            ))),
        }
    }

    fn require_field_absent(tbl: &LuaTable, key: &str, context: &str) -> Result<()> {
        let value = tbl
            .get::<LuaValue>(key)
            .map_err(|error| Error::Internal(anyhow!("{context}.{key}: {error}")))?;
        if !matches!(value, LuaValue::Nil) {
            return Err(Error::Internal(anyhow!(
                "{context}.{key} must be absent unless its capability is declared"
            )));
        }
        Ok(())
    }

    fn parse_plugin_info(
        &self,
        module: &LuaTable,
        plugin_id: &str,
        plugin_version: &str,
        entry_version: u32,
    ) -> Result<LoadedPluginInfo> {
        let info_fn: LuaFunction = module
            .get("info")
            .map_err(|e| Error::Internal(anyhow!("plugin must export info(): {e}")))?;
        let info_table: LuaTable = self
            .call_callback(&info_fn, ())
            .map_err(|e| Error::Internal(anyhow!("info() failed: {e}")))?;
        let name: String = info_table
            .get("name")
            .map_err(|e| Error::Internal(anyhow!("info().name required: {e}")))?;
        let caps_tbl: LuaTable = info_table
            .get("capabilities")
            .map_err(|e| Error::Internal(anyhow!("info().capabilities required: {e}")))?;
        let source_api = Self::optional_table(module, "source", "module")?;
        let source_item_remove = match &source_api {
            Some(source_api) => match source_api.get::<LuaValue>("remove")? {
                LuaValue::Nil => false,
                LuaValue::Function(_) => true,
                _ => {
                    return Err(Error::Internal(anyhow!(
                        "module.source.remove must be a function when present"
                    )));
                }
            },
            None => false,
        };

        let source = match Self::optional_table(&caps_tbl, "source", "info().capabilities")? {
            Some(source_tbl) => {
                if !Self::optional_bool(&source_tbl, "scan", "info().capabilities.source", false)? {
                    return Err(Error::Internal(anyhow!(
                        "info().capabilities.source.scan must be true"
                    )));
                }
                let source_api = source_api.clone().ok_or_else(|| {
                    Error::Internal(anyhow!(
                        "module.source table required for source capability"
                    ))
                })?;
                Self::require_table_function(&source_api, "scan", "module.source")?;
                let auto_detect = Self::optional_bool(
                    &source_tbl,
                    "auto_detect",
                    "info().capabilities.source",
                    false,
                )?;
                if auto_detect {
                    Self::require_table_function(&source_api, "auto_detect", "module.source")?;
                } else if entry_version == ENTRY_VERSION_V3 {
                    Self::require_field_absent(&source_api, "auto_detect", "module.source")?;
                }
                let types = Self::require_string_sequence(
                    &source_tbl,
                    "types",
                    "info().capabilities.source",
                )?;
                if types.is_empty() {
                    return Err(Error::Internal(anyhow!(
                        "info().capabilities.source.types must not be empty"
                    )));
                }
                Some(SourceCapability {
                    types,
                    library_label: Self::optional_string(
                        &source_tbl,
                        "library_label",
                        "info().capabilities.source",
                    )?,
                    library_hint: Self::optional_string(
                        &source_tbl,
                        "library_hint",
                        "info().capabilities.source",
                    )?,
                    auto_detect,
                })
            }
            None => None,
        };

        let discover = match Self::optional_table(&caps_tbl, "discover", "info().capabilities")? {
            Some(discover_tbl) => {
                if !Self::optional_bool(
                    &discover_tbl,
                    "search",
                    "info().capabilities.discover",
                    false,
                )? {
                    return Err(Error::Internal(anyhow!(
                        "info().capabilities.discover.search must be true"
                    )));
                }
                let discover_api = Self::require_module_table(module, "discover")?;
                Self::require_table_function(&discover_api, "search", "module.discover")?;
                let supports_details = Self::optional_bool(
                    &discover_tbl,
                    "details",
                    "info().capabilities.discover",
                    false,
                )?;
                let supports_download = Self::optional_bool(
                    &discover_tbl,
                    "download",
                    "info().capabilities.discover",
                    false,
                )?;
                let supports_subscription = Self::optional_bool(
                    &discover_tbl,
                    "subscription",
                    "info().capabilities.discover",
                    false,
                )?;
                let supports_resolve = Self::optional_bool(
                    &discover_tbl,
                    "resolve",
                    "info().capabilities.discover",
                    false,
                )?;
                if supports_details {
                    Self::require_table_function(&discover_api, "details", "module.discover")?;
                } else if entry_version == ENTRY_VERSION_V3 {
                    Self::require_field_absent(&discover_api, "details", "module.discover")?;
                }
                if supports_download {
                    Self::require_table_function(&discover_api, "download", "module.discover")?;
                }
                if supports_resolve {
                    Self::require_table_function(&discover_api, "resolve", "module.discover")?;
                }
                let remote = match (supports_download, supports_subscription) {
                    (true, true) => {
                        return Err(Error::Internal(anyhow!(
                            "info().capabilities.discover download and subscription are mutually exclusive"
                        )))
                    }
                    (true, false) => Some(RemoteCapability::Download),
                    (false, true) => {
                        if entry_version != ENTRY_VERSION_V3 {
                            return Err(Error::Internal(anyhow!(
                                "subscription capability requires entry_version {ENTRY_VERSION_V3}"
                            )));
                        }
                        let subscription_api =
                            Self::require_module_table(module, "subscription")?;
                        Self::require_table_function(
                            &subscription_api,
                            "status",
                            "module.subscription",
                        )?;
                        Self::require_table_function(
                            &subscription_api,
                            "subscribe",
                            "module.subscription",
                        )?;
                        Self::require_table_function(
                            &subscription_api,
                            "unsubscribe",
                            "module.subscription",
                        )?;
                        Some(RemoteCapability::Subscription)
                    }
                    (false, false) => None,
                };
                if entry_version == ENTRY_VERSION_V3
                    && supports_resolve
                    && remote != Some(RemoteCapability::Download)
                {
                    return Err(Error::Internal(anyhow!(
                        "info().capabilities.discover.resolve requires download capability"
                    )));
                }
                if entry_version == ENTRY_VERSION_V3 {
                    for (callback, declared) in [
                        ("download", supports_download),
                        ("resolve", supports_resolve),
                    ] {
                        if !declared {
                            Self::require_field_absent(&discover_api, callback, "module.discover")?;
                        }
                    }
                    let subscription_api = Self::optional_table(module, "subscription", "module")?;
                    if supports_subscription != subscription_api.is_some() {
                        return Err(Error::Internal(anyhow!(
                            "module.subscription presence must match the subscription capability"
                        )));
                    }
                }
                let sorts = Self::optional_discover_sorts(&discover_tbl)?;
                let declared_filters = Self::optional_discover_filters(&discover_tbl)?;
                let legacy_tags = Self::optional_string_sequence(
                    &discover_tbl,
                    "tags",
                    "info().capabilities.discover",
                )?;
                let dynamic_tags = discover_api.get::<LuaFunction>("tags").is_ok();
                if declared_filters.is_some() && (!legacy_tags.is_empty() || dynamic_tags) {
                    return Err(Error::Internal(anyhow!(
                        "info().capabilities.discover.filters cannot be combined with legacy tags"
                    )));
                }
                let filters =
                    declared_filters.unwrap_or_else(|| Self::legacy_discover_filter(legacy_tags));
                Some(DiscoverCapability {
                    supports_search: true,
                    supports_details,
                    supports_download,
                    supports_resolve,
                    remote,
                    remote_hint: Self::optional_string(
                        &discover_tbl,
                        "remote_hint",
                        "info().capabilities.discover",
                    )?,
                    sorts,
                    filters,
                    dynamic_tags,
                })
            }
            None => None,
        };

        let mut wallpaper = WallpaperCapability::default();
        if let Some(wallpaper_tbl) =
            Self::optional_table(&caps_tbl, "wallpaper", "info().capabilities")?
        {
            let wallpaper_api = Self::require_module_table(module, "wallpaper")?;
            wallpaper.extras = Self::optional_bool(
                &wallpaper_tbl,
                "extras",
                "info().capabilities.wallpaper",
                false,
            )?;
            wallpaper.properties = Self::optional_bool(
                &wallpaper_tbl,
                "properties",
                "info().capabilities.wallpaper",
                false,
            )?;
            if wallpaper.extras {
                Self::require_table_function(&wallpaper_api, "extras", "module.wallpaper")?;
            } else if entry_version == ENTRY_VERSION_V3 {
                Self::require_field_absent(&wallpaper_api, "extras", "module.wallpaper")?;
            }
            if wallpaper.properties {
                Self::require_table_function(&wallpaper_api, "properties", "module.wallpaper")?;
            } else if entry_version == ENTRY_VERSION_V3 {
                Self::require_field_absent(&wallpaper_api, "properties", "module.wallpaper")?;
            }
        }

        let settings = Self::parse_source_settings(&info_table)?;
        let actions = Self::parse_source_actions(&info_table, entry_version)?;
        let status = Self::parse_source_status(&info_table)?;
        let state_migrations = Self::parse_state_migrations(&info_table)?;
        let lifecycle = Self::optional_table(module, "lifecycle", "module")?;
        let actions_api = Self::optional_table(module, "actions", "module")?;
        let qrlogin = Self::optional_table(module, "qrlogin", "module")?;
        if entry_version == ENTRY_VERSION_V3 {
            if let Some(lifecycle) = &lifecycle {
                Self::require_table_function(lifecycle, "load", "module.lifecycle")?;
                Self::require_table_function(lifecycle, "save", "module.lifecycle")?;
                Self::require_table_function(lifecycle, "check", "module.lifecycle")?;
                if !state_migrations.is_empty() {
                    Self::require_table_function(lifecycle, "migrate", "module.lifecycle")?;
                }
            } else if !state_migrations.is_empty() {
                return Err(Error::Internal(anyhow!(
                    "info().state_migrations requires module.lifecycle"
                )));
            }

            let has_action_surface = !actions.is_empty() || !status.is_empty();
            if has_action_surface != actions_api.is_some() {
                return Err(Error::Internal(anyhow!(
                    "module.actions presence must match declared actions/status"
                )));
            }
            if has_action_surface {
                let actions_api = actions_api.as_ref().ok_or_else(|| {
                    Error::Internal(anyhow!(
                        "module.actions table required for declared actions/status"
                    ))
                })?;
                Self::require_table_function(actions_api, "status", "module.actions")?;
                if actions.iter().any(|action| {
                    matches!(
                        action.kind,
                        SourceActionKind::Invoke | SourceActionKind::Form
                    )
                }) {
                    Self::require_table_function(actions_api, "invoke", "module.actions")?;
                } else {
                    Self::require_field_absent(actions_api, "invoke", "module.actions")?;
                }
            }
            let has_qr_action = actions
                .iter()
                .any(|action| action.kind == SourceActionKind::QrLogin);
            if has_qr_action != qrlogin.is_some() {
                return Err(Error::Internal(anyhow!(
                    "module.qrlogin presence must match declared qr_login actions"
                )));
            }
            if has_qr_action {
                let qrlogin = qrlogin.as_ref().ok_or_else(|| {
                    Error::Internal(anyhow!("module.qrlogin table required for qr_login action"))
                })?;
                Self::require_table_function(qrlogin, "begin", "module.qrlogin")?;
                Self::require_table_function(qrlogin, "poll", "module.qrlogin")?;
            }
        }
        let display_name = {
            let dn = Self::optional_string(&info_table, "display_name", "info()")?;
            if dn.is_empty() {
                name.clone()
            } else {
                dn
            }
        };

        Ok(LoadedPluginInfo {
            name,
            display_name,
            plugin_id: plugin_id.to_owned(),
            version: plugin_version.to_owned(),
            capabilities: PluginCapabilities {
                source,
                source_item_remove,
                discover,
                wallpaper,
                lifecycle: lifecycle.is_some(),
            },
            settings,
            actions,
            status,
            state_migrations,
        })
    }

    /// Parse the optional `info().actions` sequence.
    fn parse_source_actions(info: &LuaTable, entry_version: u32) -> Result<Vec<SourceAction>> {
        let mut out = Vec::new();
        let mut ids = HashSet::new();
        for entry in Self::info_sequence(info, "actions")? {
            let id: String = entry
                .get("id")
                .map_err(|e| Error::Internal(anyhow!("info().actions.id: {e}")))?;
            if id.trim().is_empty() {
                return Err(Error::Internal(anyhow!(
                    "info().actions entry requires a non-empty id"
                )));
            }
            if !ids.insert(id.clone()) {
                return Err(Error::Internal(anyhow!(
                    "info().actions contains duplicate id '{id}'"
                )));
            }
            let kind = match Self::optional_string(&entry, "kind", "info().actions")?.as_str() {
                "" | "invoke" => SourceActionKind::Invoke,
                "qr_login" if entry_version == ENTRY_VERSION_V3 => SourceActionKind::QrLogin,
                "form" if entry_version == ENTRY_VERSION_V3 => SourceActionKind::Form,
                value => {
                    return Err(Error::Internal(anyhow!(
                        "info().actions kind '{value}' is unsupported for entry_version {entry_version}"
                    )))
                }
            };
            let mut fields = Vec::new();
            let mut field_keys = HashSet::new();
            for field in Self::info_sequence(&entry, "fields")? {
                let key = Self::require_string(&field, "key", "info().actions.fields")?;
                if key.trim().is_empty() {
                    return Err(Error::Internal(anyhow!(
                        "info().actions.fields entry requires a non-empty key"
                    )));
                }
                if !field_keys.insert(key.clone()) {
                    return Err(Error::Internal(anyhow!(
                        "info().actions contains duplicate field key '{key}'"
                    )));
                }
                fields.push(SourceActionField {
                    key,
                    label: Self::optional_string(&field, "label", "info().actions.fields")?,
                    description: Self::optional_string(
                        &field,
                        "description",
                        "info().actions.fields",
                    )?,
                    placeholder: Self::optional_string(
                        &field,
                        "placeholder",
                        "info().actions.fields",
                    )?,
                    secret: Self::optional_bool(&field, "secret", "info().actions.fields", false)?,
                    required: Self::optional_bool(
                        &field,
                        "required",
                        "info().actions.fields",
                        false,
                    )?,
                });
            }
            if kind == SourceActionKind::Form && fields.is_empty() {
                return Err(Error::Internal(anyhow!(
                    "info().actions form action requires at least one field"
                )));
            }
            if kind != SourceActionKind::Form && !fields.is_empty() {
                return Err(Error::Internal(anyhow!(
                    "info().actions fields are only valid for form actions"
                )));
            }
            out.push(SourceAction {
                id,
                label: Self::optional_string(&entry, "label", "info().actions")?,
                description: Self::optional_string(&entry, "description", "info().actions")?,
                browse_description: Self::optional_string(
                    &entry,
                    "browse_description",
                    "info().actions",
                )?,
                browse_button_label: Self::optional_string(
                    &entry,
                    "browse_button_label",
                    "info().actions",
                )?,
                group: Self::optional_string(&entry, "group", "info().actions")?,
                order: entry
                    .get::<Option<i32>>("order")
                    .unwrap_or(None)
                    .unwrap_or(0),
                kind,
                visible: true,
                enabled: true,
                fields,
                required_for_browsing: Self::optional_bool(
                    &entry,
                    "required_for_browsing",
                    "info().actions",
                    kind == SourceActionKind::QrLogin,
                )?,
            });
        }
        Ok(out)
    }

    fn parse_state_migrations(info: &LuaTable) -> Result<Vec<PluginStateMigration>> {
        let mut out = Vec::new();
        for entry in Self::info_sequence(info, "state_migrations")? {
            let schema_id = Self::require_string(&entry, "schema_id", "info().state_migrations")?;
            let file = Self::require_string(&entry, "file", "info().state_migrations")?;
            if schema_id.trim().is_empty() || file.trim().is_empty() {
                return Err(Error::Internal(anyhow!(
                    "info().state_migrations schema_id/file must not be empty"
                )));
            }
            out.push(PluginStateMigration { schema_id, file });
        }
        Ok(out)
    }

    fn callback_key(
        &self,
        module: &LuaTable,
        table_name: &str,
        function_name: &str,
    ) -> Result<Option<LuaRegistryKey>> {
        let Some(table) = Self::optional_table(module, table_name, "module")? else {
            return Ok(None);
        };
        match table.get::<LuaValue>(function_name).map_err(|error| {
            Error::Internal(anyhow!("module.{table_name}.{function_name}: {error}"))
        })? {
            LuaValue::Nil => Ok(None),
            LuaValue::Function(function) => self
                .lua
                .create_registry_value(function)
                .map(Some)
                .map_err(Error::from),
            other => Err(Error::Internal(anyhow!(
                "module.{table_name}.{function_name} must be a function, got {}",
                other.type_name()
            ))),
        }
    }

    fn parse_callbacks(&self, module: &LuaTable) -> Result<PluginCallbacks> {
        Ok(PluginCallbacks {
            source_scan: self.callback_key(module, "source", "scan")?,
            source_auto_detect: self.callback_key(module, "source", "auto_detect")?,
            source_remove: self.callback_key(module, "source", "remove")?,
            discover_search: self.callback_key(module, "discover", "search")?,
            discover_tags: self.callback_key(module, "discover", "tags")?,
            discover_details: self.callback_key(module, "discover", "details")?,
            discover_download: self.callback_key(module, "discover", "download")?,
            discover_resolve: self.callback_key(module, "discover", "resolve")?,
            wallpaper_extras: self.callback_key(module, "wallpaper", "extras")?,
            wallpaper_properties: self.callback_key(module, "wallpaper", "properties")?,
            lifecycle_load: self.callback_key(module, "lifecycle", "load")?,
            lifecycle_save: self.callback_key(module, "lifecycle", "save")?,
            lifecycle_check: self.callback_key(module, "lifecycle", "check")?,
            lifecycle_migrate: self.callback_key(module, "lifecycle", "migrate")?,
            actions_status: self.callback_key(module, "actions", "status")?,
            actions_invoke: self.callback_key(module, "actions", "invoke")?,
            qrlogin_begin: self.callback_key(module, "qrlogin", "begin")?,
            qrlogin_poll: self.callback_key(module, "qrlogin", "poll")?,
            qrlogin_cancel: self.callback_key(module, "qrlogin", "cancel")?,
            subscription_status: self.callback_key(module, "subscription", "status")?,
            subscription_subscribe: self.callback_key(module, "subscription", "subscribe")?,
            subscription_unsubscribe: self.callback_key(module, "subscription", "unsubscribe")?,
        })
    }

    /// Parse the optional `info().status` sequence (`{id,label,group,order}`).
    fn parse_source_status(info: &LuaTable) -> Result<Vec<SourceStatus>> {
        let mut out = Vec::new();
        let mut ids = HashSet::new();
        for entry in Self::info_sequence(info, "status")? {
            let id: String = entry
                .get("id")
                .map_err(|e| Error::Internal(anyhow!("info().status.id: {e}")))?;
            if id.trim().is_empty() {
                return Err(Error::Internal(anyhow!(
                    "info().status entry requires a non-empty id"
                )));
            }
            if !ids.insert(id.clone()) {
                return Err(Error::Internal(anyhow!(
                    "info().status contains duplicate id '{id}'"
                )));
            }
            out.push(SourceStatus {
                id,
                label: Self::optional_string(&entry, "label", "info().status")?,
                group: Self::optional_string(&entry, "group", "info().status")?,
                order: entry
                    .get::<Option<i32>>("order")
                    .unwrap_or(None)
                    .unwrap_or(0),
                value: String::new(),
            });
        }
        Ok(out)
    }

    /// Read an optional top-level `info()` sequence-of-tables field.
    fn info_sequence(info: &LuaTable, field: &str) -> Result<Vec<LuaTable>> {
        let val: LuaValue = info
            .get(field)
            .map_err(|e| Error::Internal(anyhow!("info().{field}: {e}")))?;
        let tbl = match val {
            LuaValue::Nil => return Ok(Vec::new()),
            LuaValue::Table(t) => t,
            _ => {
                return Err(Error::Internal(anyhow!(
                    "info().{field} must be a sequence of tables"
                )));
            }
        };
        let mut out = Vec::new();
        for entry in tbl.sequence_values::<LuaTable>() {
            out.push(entry.map_err(|e| Error::Internal(anyhow!("info().{field} entry: {e}")))?);
        }
        Ok(out)
    }

    /// Parse the optional `info().settings` sequence into UI-renderable specs.
    fn parse_source_settings(info: &LuaTable) -> Result<Vec<SourceSetting>> {
        let val: LuaValue = info
            .get("settings")
            .map_err(|e| Error::Internal(anyhow!("info().settings: {e}")))?;
        let tbl = match val {
            LuaValue::Nil => return Ok(Vec::new()),
            LuaValue::Table(t) => t,
            _ => {
                return Err(Error::Internal(anyhow!(
                    "info().settings must be a sequence of tables"
                )));
            }
        };
        let mut out = Vec::new();
        let mut keys = HashSet::new();
        for entry in tbl.sequence_values::<LuaTable>() {
            let s = entry.map_err(|e| Error::Internal(anyhow!("info().settings entry: {e}")))?;
            let key: String = s
                .get("key")
                .map_err(|e| Error::Internal(anyhow!("info().settings.key: {e}")))?;
            if key.trim().is_empty() {
                return Err(Error::Internal(anyhow!(
                    "info().settings entry requires a non-empty key"
                )));
            }
            if !keys.insert(key.clone()) {
                return Err(Error::Internal(anyhow!(
                    "info().settings contains duplicate key '{key}'"
                )));
            }
            let ty = Self::optional_string(&s, "type", "info().settings")?;
            let ty = if ty.is_empty() { "string".into() } else { ty };
            if !matches!(ty.as_str(), "string" | "bool" | "u32" | "i32" | "f32") {
                return Err(Error::Internal(anyhow!(
                    "info().settings type '{ty}' is unsupported"
                )));
            }
            let default = match s
                .get::<LuaValue>("default")
                .map_err(|e| Error::Internal(anyhow!("info().settings.default: {e}")))?
            {
                LuaValue::Nil => String::new(),
                LuaValue::Boolean(b) => b.to_string(),
                LuaValue::Integer(i) => i.to_string(),
                LuaValue::Number(n) => n.to_string(),
                LuaValue::String(v) => v.to_str()?.to_owned(),
                _ => String::new(),
            };
            let order: i32 = s
                .get::<Option<i32>>("order")
                .map_err(|e| Error::Internal(anyhow!("info().settings.order: {e}")))?
                .unwrap_or(0);
            let choices = match s
                .get::<LuaValue>("choices")
                .map_err(|e| Error::Internal(anyhow!("info().settings.choices: {e}")))?
            {
                LuaValue::Table(c) => c
                    .sequence_values::<String>()
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| Error::Internal(anyhow!("info().settings.choices: {e}")))?,
                _ => Vec::new(),
            };
            out.push(SourceSetting {
                key,
                ty,
                default,
                label: Self::optional_string(&s, "label", "info().settings")?,
                description: Self::optional_string(&s, "description", "info().settings")?,
                group: Self::optional_string(&s, "group", "info().settings")?,
                order,
                choices,
            });
        }
        Ok(out)
    }

    /// Load a single Lua entry, tagging it with the owning installable
    /// plugin's domain id. Returns the Lua plugin name.
    pub fn load_plugin(
        &mut self,
        path: &Path,
        plugin_id: &str,
        plugin_version: &str,
        entry_version: u32,
    ) -> Result<String> {
        if !supports_entry_version(entry_version) {
            return Err(Error::Internal(anyhow!(
                "unsupported Lua entry_version {entry_version}; supported versions are {SUPPORTED_ENTRY_VERSIONS:?}"
            )));
        }
        let source = std::fs::read_to_string(path)
            .map_err(|e| Error::Internal(anyhow!("read {}: {e}", path.display())))?;
        let root = path.parent().unwrap_or_else(|| Path::new("."));
        let env = self.plugin_lua_env(root)?;
        let module: LuaTable = {
            let _deadline = self.arm_callback_deadline()?;
            self.lua
                .load(&source)
                .set_name(path.to_string_lossy())
                .set_environment(env)
                .eval()
                .map_err(|e| Error::Internal(anyhow!("eval {}: {e}", path.display())))?
        };

        let info = self.parse_plugin_info(&module, plugin_id, plugin_version, entry_version)?;
        let callbacks = self.parse_callbacks(&module)?;
        let name = info.name.clone();

        let key = self.lua.create_registry_value(module)?;
        self.plugins.insert(name.clone(), key);
        self.callbacks.insert(name.clone(), callbacks);
        self.plugin_infos.insert(name.clone(), info);
        if let Err(error) = self
            .restore_http_session(plugin_id)
            .and_then(|()| self.initialize_state(&name))
        {
            self.plugins.remove(&name);
            self.callbacks.remove(&name);
            self.plugin_infos.remove(&name);
            return Err(error);
        }
        log::info!(
            "loaded source plugin: {name} (plugin {plugin_id}) from {}",
            path.display()
        );
        Ok(name)
    }

    fn initialize_state(&mut self, plugin_name: &str) -> Result<()> {
        let info = self
            .plugin_infos
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?
            .clone();
        if !info.capabilities.lifecycle {
            return Ok(());
        }
        let callbacks = self
            .callbacks
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
        let load_fn: LuaFunction = self.lua.registry_value(
            callbacks
                .lifecycle_load
                .as_ref()
                .ok_or_else(|| Error::Internal(anyhow!("module.lifecycle.load required")))?,
        )?;

        if let Some(state) = self.state_store.load(&info.plugin_id)? {
            self.call_callback::<()>(&load_fn, state.clone())?;
            self.saved_states
                .insert(info.plugin_id.clone(), Some(state));
            return Ok(());
        }

        for migration in &info.state_migrations {
            let Some(raw) = self.state_store.load_legacy(&migration.file)? else {
                continue;
            };
            let migrate_fn: LuaFunction =
                self.lua
                    .registry_value(callbacks.lifecycle_migrate.as_ref().ok_or_else(|| {
                        Error::Internal(anyhow!("module.lifecycle.migrate required"))
                    })?)?;
            let state: String =
                self.call_callback(&migrate_fn, (migration.schema_id.clone(), raw))?;
            self.call_callback::<()>(&load_fn, state.clone())?;
            self.state_store
                .save_if_changed(&info.plugin_id, None, &state)?;
            self.state_store.preserve_legacy(&migration.file)?;
            self.saved_states
                .insert(info.plugin_id.clone(), Some(state));
            return Ok(());
        }

        self.call_callback::<()>(&load_fn, LuaValue::Nil)?;
        self.saved_states.insert(info.plugin_id, None);
        Ok(())
    }

    fn persist_state(&mut self, plugin_name: &str) -> Result<()> {
        let (plugin_id, has_lifecycle) = self
            .plugin_infos
            .get(plugin_name)
            .map(|info| (info.plugin_id.clone(), info.capabilities.lifecycle))
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
        let lifecycle_result = (|| -> Result<()> {
            if !has_lifecycle {
                return Ok(());
            }
            let callbacks = self
                .callbacks
                .get(plugin_name)
                .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
            let save_fn: LuaFunction = self.lua.registry_value(
                callbacks
                    .lifecycle_save
                    .as_ref()
                    .ok_or_else(|| Error::Internal(anyhow!("module.lifecycle.save required")))?,
            )?;
            let state: String = self.call_callback(&save_fn, ())?;
            let previous = self
                .saved_states
                .get(&plugin_id)
                .and_then(|state| state.as_deref());
            if self
                .state_store
                .save_if_changed(&plugin_id, previous, &state)?
            {
                self.saved_states.insert(plugin_id.clone(), Some(state));
            }
            Ok(())
        })();
        let http_result = self.persist_http_session_by_id(&plugin_id);
        lifecycle_result?;
        http_result
    }

    fn persist_all_http_sessions(&self) {
        if self.generation_state.load(Ordering::Acquire) == RUNTIME_INACTIVE {
            return;
        }
        let mut plugin_ids = HashSet::new();
        for info in self.plugin_infos.values() {
            if plugin_ids.insert(info.plugin_id.as_str()) {
                if let Err(error) = self.persist_http_session_by_id(&info.plugin_id) {
                    log::warn!(
                        "persist HTTP session while releasing {}: {error:#}",
                        info.plugin_id
                    );
                }
            }
        }
    }

    /// Run `scan(ctx)` on all loaded plugins and merge results.
    /// `libs_by_plugin` is the per-plugin library list from the DB.
    pub async fn scan_all(&mut self, libs_by_plugin: &HashMap<String, Vec<String>>) -> Result<()> {
        self.entries.clear();
        self.by_type.clear();

        let mut plugin_names: Vec<String> = self
            .plugin_infos
            .iter()
            .filter_map(|(name, info)| info.capabilities.source.is_some().then(|| name.clone()))
            .collect();
        plugin_names.sort();
        // Every plugin is scanned even if an earlier one fails, so one broken
        // source does not hide the wallpapers the others found. The failures
        // are still reported, otherwise a failed scan is indistinguishable from
        // a scan that legitimately found nothing.
        let mut failures: Vec<String> = Vec::new();
        for name in &plugin_names {
            let libs = libs_by_plugin
                .get(name)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            if let Err(e) = self.scan_plugin(name, libs).await {
                log::warn!("scan plugin {name} failed: {e}");
                failures.push(format!("{name}: {e:#}"));
            }
        }
        if !failures.is_empty() {
            return Err(Error::Internal(anyhow!(
                "source scan failed for {}",
                failures.join("; ")
            )));
        }
        Ok(())
    }

    /// Run `scan(ctx)` on a single plugin by name with the supplied
    /// library list exposed as `ctx.libraries()`.
    async fn scan_plugin(&mut self, name: &str, libraries: &[String]) -> Result<()> {
        let callbacks = self
            .callbacks
            .get(name)
            .ok_or_else(|| Error::SourcePluginNotFound(name.to_string()))?;
        let info = self
            .plugin_infos
            .get(name)
            .ok_or_else(|| Error::SourcePluginNotFound(name.to_string()))?;
        if info.capabilities.source.is_none() {
            return Ok(());
        }
        let scan_fn: LuaFunction = self.lua.registry_value(
            callbacks
                .source_scan
                .as_ref()
                .ok_or_else(|| Error::SourcePluginNotFound(name.to_string()))?,
        )?;

        let ctx = self.build_ctx(Some(name), libraries)?;
        let results: LuaTable = self
            .call_plugin_callback_async(name, &scan_fn, ctx)
            .await
            .map_err(|error| {
                Error::Internal(anyhow!(
                    "source plugin '{name}' scan failed: {}",
                    redact_secrets(&error.to_string())
                ))
            })?;

        for pair in results.sequence_values::<LuaTable>() {
            let tbl = pair?;
            let entry_name = Self::require_string(&tbl, "name", "module.source.scan result")?;
            let wp_type = Self::require_string(&tbl, "wp_type", "module.source.scan result")?;
            let resource = Self::require_string(&tbl, "resource", "module.source.scan result")?;
            let entry = WallpaperEntry {
                // Identity comes from the DB item.id, assigned after
                // sync; plugins don't supply it.
                item_id: 0,
                name: entry_name,
                wp_type,
                resource,
                preview: tbl.get::<String>("preview").ok(),
                plugin_name: name.to_owned(),
                library_root: tbl.get("library_root").unwrap_or_default(),
                description: tbl.get::<String>("description").ok(),
                tags: tbl.get::<Vec<String>>("tags").unwrap_or_default(),
                external_id: tbl.get::<String>("external_id").ok(),
                // Optional plugin-supplied media metadata.
                // Plugins that know it can skip later probing.
                size: tbl.get::<i64>("size").ok(),
                width: tbl.get::<u32>("width").ok(),
                height: tbl.get::<u32>("height").ok(),
                content_rating: tbl.get::<String>("content_rating").ok(),
                // Daemon-only (filled from DB on read); scan leaves it None.
                modified_at: None,
                create_at: 0,
            };
            let idx = self.entries.len();
            self.by_type
                .entry(entry.wp_type.clone())
                .or_default()
                .push(idx);
            self.entries.push(entry);
        }
        self.persist_state(name)?;
        Ok(())
    }

    fn install_json_context(&self, ctx: &LuaTable) -> LuaResult<()> {
        let json = mlua_extra::json::create_nullable_module(&self.lua)?;
        let parse = json.get::<LuaFunction>("parse")?;
        let encode = json.get::<LuaFunction>("encode")?;
        ctx.set("json_parse", parse)?;
        ctx.set("json_encode", encode)?;
        ctx.set("json", json)
    }

    fn install_remote_utility_context(&self, ctx: &LuaTable) -> LuaResult<()> {
        self.install_json_context(ctx)?;

        let base64 = mlua_extra::base64::create_module(&self.lua)?;
        ctx.set("base64_decode", base64.get::<LuaFunction>("decode")?)?;
        ctx.set("base64", base64)?;

        let time = mlua_extra::time::create_module(&self.lua)?;
        ctx.set("time_unix", time.get::<LuaFunction>("unix")?)?;
        ctx.set("time", time)?;
        ctx.set("random", mlua_extra::random::create_module(&self.lua)?)
    }

    /// Build the `ctx` table passed to Lua callbacks.
    /// `libraries` is exposed through `ctx.libraries()`.
    fn build_ctx(&self, plugin_name: Option<&str>, libraries: &[String]) -> Result<LuaTable> {
        let ctx = self.lua.create_table()?;

        // ctx.glob(pattern) -> list of file paths
        let glob_fn = self.lua.create_function(|lua, pattern: String| {
            let paths = lua.create_table()?;
            let mut i = 1;
            if let Ok(entries) = glob::glob(&pattern) {
                for entry in entries.flatten() {
                    if let Some(s) = entry.to_str() {
                        paths.set(i, s.to_string())?;
                        i += 1;
                    }
                }
            }
            Ok(paths)
        })?;
        ctx.set("glob", glob_fn.clone())?;

        // ctx.list_dirs(path) -> list of subdirectory paths
        let list_dirs_fn = self.lua.create_function(|lua, path: String| {
            let dirs = lua.create_table()?;
            let mut i = 1;
            if let Ok(entries) = std::fs::read_dir(&path) {
                for entry in entries.flatten() {
                    if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        if let Some(s) = entry.path().to_str() {
                            dirs.set(i, s.to_string())?;
                            i += 1;
                        }
                    }
                }
            }
            Ok(dirs)
        })?;
        ctx.set("list_dirs", list_dirs_fn.clone())?;

        // ctx.file_exists(path) -> bool
        let file_exists_fn = self
            .lua
            .create_function(|_, path: String| Ok(std::path::Path::new(&path).exists()))?;
        ctx.set("file_exists", file_exists_fn.clone())?;

        // ctx.read_file(path) -> string|nil (capped at 1MB)
        let read_file_fn =
            self.lua
                .create_function(|lua, path: String| match std::fs::metadata(&path) {
                    Ok(meta) if meta.len() > 1_048_576 => Ok(mlua::Value::Nil),
                    Ok(_) => match std::fs::read_to_string(&path) {
                        Ok(s) => Ok(mlua::Value::String(lua.create_string(&s)?)),
                        Err(_) => Ok(mlua::Value::Nil),
                    },
                    Err(_) => Ok(mlua::Value::Nil),
                })?;
        ctx.set("read_file", read_file_fn.clone())?;

        // ctx.extension(path) -> string|nil
        let extension_fn = self.lua.create_function(|_, path: String| {
            Ok(std::path::Path::new(&path)
                .extension()
                .and_then(|e| e.to_str())
                .map(String::from))
        })?;
        ctx.set("extension", extension_fn.clone())?;

        // ctx.filename(path) -> string|nil
        let filename_fn = self.lua.create_function(|_, path: String| {
            Ok(std::path::Path::new(&path)
                .file_name()
                .and_then(|e| e.to_str())
                .map(String::from))
        })?;
        ctx.set("filename", filename_fn.clone())?;

        // ctx.basename(path) -> string|nil (same as filename on dirs)
        ctx.set("basename", filename_fn.clone())?;

        // ctx.env(name) -> string|nil. Used for auto-detect probing of
        // well-known paths such as $HOME.
        let env_fn = self
            .lua
            .create_function(|_, name: String| Ok(std::env::var(&name).ok()))?;
        ctx.set("env", env_fn)?;

        // ctx.plugin_config(key) -> string|nil. Reads the Lua source component's
        // table from config.toml. No-op without a settings store.
        let cfg_settings = self.settings.clone();
        let cfg_plugin = plugin_name.map(str::to_owned);
        let plugin_config_fn = self.lua.create_function(move |_, key: String| {
            let value = match (cfg_settings.as_ref(), cfg_plugin.as_ref()) {
                (Some(store), Some(name)) => {
                    store.plugin(name).and_then(|kv| kv.get(&key).cloned())
                }
                _ => None,
            };
            Ok(value)
        })?;
        ctx.set("plugin_config", plugin_config_fn.clone())?;
        let config = self.lua.create_table()?;
        config.set("get", plugin_config_fn)?;
        ctx.set("config", config)?;

        // ctx.libraries() -> list of absolute library paths registered
        // for this plugin in the daemon DB.
        let libs_for_closure: Vec<String> = libraries.to_vec();
        let libraries_fn = self.lua.create_function(move |lua, ()| {
            let tbl = lua.create_table()?;
            for (i, lib) in libs_for_closure.iter().enumerate() {
                tbl.set(i + 1, lib.clone())?;
            }
            Ok(tbl)
        })?;
        ctx.set("libraries", libraries_fn)?;

        self.install_json_context(&ctx)?;

        // ctx.log(msg)
        let log_fn = self.lua.create_function(|_, msg: String| {
            log::info!("[lua] {}", redact_secrets(&msg));
            Ok(())
        })?;
        ctx.set("log", log_fn)?;

        // ctx.file_size(path) -> integer|nil
        // Cheap stat-only helper for Lua plugins to pre-fill size metadata.
        let file_size_fn = self.lua.create_function(|_, path: String| {
            let bytes = std::fs::metadata(&path)
                .ok()
                .and_then(|m| i64::try_from(m.len()).ok());
            Ok(bytes)
        })?;
        ctx.set("file_size", file_size_fn.clone())?;

        // ctx.remove_file(path) -> bool
        let remove_file_fn =
            self.lua
                .create_function(|_, path: String| match std::fs::remove_file(&path) {
                    Ok(()) => Ok(true),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                    Err(e) => Err(mlua::Error::external(e)),
                })?;
        ctx.set("remove_file", remove_file_fn.clone())?;

        // ctx.remove_dir(path) -> bool. Recursively removes a directory, guarded
        // to the calling plugin's remote content dir (canonicalized, so `..` and
        // symlink escapes are rejected).
        let rd_plugin = plugin_name.map(str::to_owned);
        let remove_dir_fn = self.lua.create_function(move |_, path: String| {
            let Some(name) = rd_plugin.as_ref() else {
                return Ok(false);
            };
            let root = crate::settings::remote_content_dir(name)
                .canonicalize()
                .map_err(|e| mlua::Error::external(format!("remote dir unavailable: {e}")))?;
            let target = std::path::Path::new(&path)
                .canonicalize()
                .map_err(mlua::Error::external)?;
            if !target.starts_with(&root) {
                return Err(mlua::Error::external(format!(
                    "remove_dir refused: {path} is outside the plugin remote directory"
                )));
            }
            match std::fs::remove_dir_all(&target) {
                Ok(()) => Ok(true),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(mlua::Error::external(e)),
            }
        })?;
        ctx.set("remove_dir", remove_dir_fn.clone())?;

        let fs = self.lua.create_table()?;
        fs.set("glob", glob_fn)?;
        fs.set("list_dirs", list_dirs_fn)?;
        fs.set("exists", file_exists_fn)?;
        fs.set("read", read_file_fn)?;
        fs.set("extension", extension_fn)?;
        fs.set("filename", filename_fn.clone())?;
        fs.set("basename", filename_fn)?;
        fs.set("size", file_size_fn)?;
        fs.set("remove_file", remove_file_fn)?;
        fs.set("remove_dir", remove_dir_fn)?;
        ctx.set("fs", fs)?;

        // ctx.probe(path) -> table|nil
        // Returns present file/media fields, or nil if nothing was found.
        let probe_arc = Arc::clone(&self.probe);
        let probe_fn = self.lua.create_function(move |lua, path: String| {
            let s = crate::probe::stat::stat_file(&path);
            let m = probe_arc.probe_media(&path);
            if s.is_none() && m.width.is_none() && m.height.is_none() {
                return Ok(mlua::Value::Nil);
            }
            let tbl = lua.create_table()?;
            if let Some(s) = s {
                tbl.set("size", s.size)?;
            }
            if let Some(v) = m.width {
                tbl.set("width", v)?;
            }
            if let Some(v) = m.height {
                tbl.set("height", v)?;
            }
            Ok(mlua::Value::Table(tbl))
        })?;
        ctx.set("probe", probe_fn)?;

        // ctx.library_meta_get(library_path, key) -> string|nil
        // ctx.library_meta_set(library_path, key, value_or_nil) -> bool
        {
            let kv_db = self.db.clone();
            let kv_plugin = plugin_name.map(str::to_owned);

            let getter_db = kv_db.clone();
            let getter_plugin = kv_plugin.clone();
            let library_meta_get_fn =
                self.lua
                    .create_async_function(move |lua, (lib_path, key): (String, String)| {
                        let db = getter_db.clone();
                        let plugin_name = getter_plugin.clone();
                        async move {
                            let (Some(db), Some(plugin_name)) = (db, plugin_name) else {
                                return Ok(mlua::Value::Nil);
                            };
                            let res: crate::error::Result<Option<String>> = async {
                                let Some(plugin) =
                                    repo::find_plugin_by_name(&db, &plugin_name).await?
                                else {
                                    return Ok(None);
                                };
                                let Some(lib) =
                                    repo::find_library(&db, plugin.id, &lib_path).await?
                                else {
                                    return Ok(None);
                                };
                                repo::get_library_metadata_value(&db, lib.id, &key).await
                            }
                            .await;
                            match res {
                                Ok(Some(v)) => Ok(mlua::Value::String(lua.create_string(&v)?)),
                                Ok(None) => Ok(mlua::Value::Nil),
                                Err(e) => {
                                    log::warn!("library_meta_get: {e:#}");
                                    Ok(mlua::Value::Nil)
                                }
                            }
                        }
                    })?;
            ctx.set("library_meta_get", library_meta_get_fn)?;

            let setter_db = kv_db;
            let setter_plugin = kv_plugin;
            let library_meta_set_fn = self.lua.create_async_function(
                move |_, (lib_path, key, value): (String, String, Option<String>)| {
                    let db = setter_db.clone();
                    let plugin_name = setter_plugin.clone();
                    async move {
                        let (Some(db), Some(plugin_name)) = (db, plugin_name) else {
                            return Ok(false);
                        };
                        let res: crate::error::Result<bool> = async {
                            let Some(plugin) = repo::find_plugin_by_name(&db, &plugin_name).await?
                            else {
                                return Ok(false);
                            };
                            let Some(lib) = repo::find_library(&db, plugin.id, &lib_path).await?
                            else {
                                return Ok(false);
                            };
                            repo::set_library_metadata_value(&db, lib.id, &key, value.as_deref())
                                .await?;
                            Ok(true)
                        }
                        .await;
                        match res {
                            Ok(b) => Ok(b),
                            Err(e) => {
                                log::warn!("library_meta_set: {e:#}");
                                Ok(false)
                            }
                        }
                    }
                },
            )?;
            ctx.set("library_meta_set", library_meta_set_fn)?;
        }

        // Source plugins write entry fields directly using the canonical
        // schema exposed by WallpaperEntry.

        // ctx.http is a fluent client:
        // ctx.http:get(url):headers({...}):send()
        ctx.set("http", self.http_view())?;
        ctx.set("html", mlua_extra::html::create_module(&self.lua)?)?;
        ctx.set("url", mlua_extra::url::create_module(&self.lua)?)?;

        Ok(ctx)
    }

    fn build_remote_ctx(&self, plugin_name: &str) -> Result<LuaTable> {
        let ctx = self.lua.create_table()?;

        let cfg_settings = self.settings.clone();
        let cfg_plugin = plugin_name.to_owned();
        let plugin_config = self.lua.create_function(move |_, key: String| {
            Ok(cfg_settings
                .as_ref()
                .and_then(|store| store.plugin(&cfg_plugin))
                .and_then(|values| values.get(&key).cloned()))
        })?;
        ctx.set("plugin_config", plugin_config.clone())?;
        let config = self.lua.create_table()?;
        config.set("get", plugin_config)?;
        ctx.set("config", config)?;

        self.install_remote_utility_context(&ctx)?;
        let log_plugin = plugin_name.to_owned();
        ctx.set(
            "log",
            self.lua.create_function(move |_, message: String| {
                log::info!("[lua:{log_plugin}] {}", redact_secrets(&message));
                Ok(())
            })?,
        )?;
        ctx.set("http", self.http_view())?;
        ctx.set("html", mlua_extra::html::create_module(&self.lua)?)?;
        ctx.set("url", mlua_extra::url::create_module(&self.lua)?)?;
        Ok(ctx)
    }

    fn wallpaper_entry_table(&self, entry: &WallpaperEntry) -> Result<LuaTable> {
        let entry_tbl = self.lua.create_table()?;
        entry_tbl.set("id", entry.item_id.to_string())?;
        entry_tbl.set("item_id", entry.item_id)?;
        entry_tbl.set("name", entry.name.clone())?;
        entry_tbl.set("wp_type", entry.wp_type.clone())?;
        entry_tbl.set("path", entry.resource.clone())?;
        entry_tbl.set("resource", entry.resource.clone())?;
        if let Some(p) = &entry.preview {
            entry_tbl.set("preview", p.clone())?;
        }
        if !entry.library_root.is_empty() {
            entry_tbl.set("library_root", entry.library_root.clone())?;
            if let Some(rel) =
                crate::model::sync::relative_under_root(&entry.library_root, &entry.resource)
            {
                entry_tbl.set("relative_path", rel)?;
            }
        }
        if let Some(d) = &entry.description {
            entry_tbl.set("description", d.clone())?;
        }
        if !entry.tags.is_empty() {
            let tags = self.lua.create_table()?;
            for (idx, tag) in entry.tags.iter().enumerate() {
                tags.set(idx + 1, tag.clone())?;
            }
            entry_tbl.set("tags", tags)?;
        }
        if let Some(eid) = &entry.external_id {
            entry_tbl.set("external_id", eid.clone())?;
        }
        if let Some(size) = entry.size {
            entry_tbl.set("size", size)?;
        }
        if let Some(width) = entry.width {
            entry_tbl.set("width", width)?;
        }
        if let Some(height) = entry.height {
            entry_tbl.set("height", height)?;
        }
        if let Some(content_rating) = &entry.content_rating {
            entry_tbl.set("content_rating", content_rating.clone())?;
        }
        Ok(entry_tbl)
    }

    // -----------------------------------------------------------------------
    // Query API

    pub fn list(&self) -> &[WallpaperEntry] {
        &self.entries
    }

    pub fn list_by_type(&self, wp_type: &str) -> Vec<&WallpaperEntry> {
        self.by_type
            .get(wp_type)
            .map(|indices| indices.iter().map(|&i| &self.entries[i]).collect())
            .unwrap_or_default()
    }

    pub fn get(&self, id: &str) -> Option<&WallpaperEntry> {
        self.entries.iter().find(|e| e.item_id.to_string() == id)
    }

    /// Ask the plugin that produced `entry` for the CLI `extras`
    /// dictionary the daemon should pass to the renderer subprocess
    pub async fn call_extras(
        &mut self,
        plugin_name: &str,
        entry: &WallpaperEntry,
    ) -> Result<HashMap<String, String>> {
        let callbacks = self
            .callbacks
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
        let Some(info) = self.plugin_infos.get(plugin_name) else {
            return Err(Error::SourcePluginNotFound(plugin_name.to_string()));
        };
        if !info.capabilities.wallpaper.extras {
            log::warn!("source plugin '{plugin_name}' has no wallpaper.extras capability");
            return Ok(HashMap::new());
        }
        // Keep the Lua body in one block so failures map to one typed
        // SourceExtrasFailed carrying the plugin name.
        let body = async {
            let extras_fn: LuaFunction = self.lua.registry_value(
                callbacks
                    .wallpaper_extras
                    .as_ref()
                    .ok_or_else(|| LuaError::external("module.wallpaper.extras required"))?,
            )?;
            let entry_tbl = self
                .wallpaper_entry_table(entry)
                .map_err(mlua::Error::external)?;
            // Build the same ctx scan(ctx) sees; extras runs per item, so
            // the libraries list is intentionally empty.
            let ctx = self
                .build_ctx(Some(plugin_name), &[])
                .map_err(mlua::Error::external)?;
            let result: LuaTable = self
                .call_plugin_callback_async(plugin_name, &extras_fn, (entry_tbl, ctx))
                .await?;
            let mut out = HashMap::new();
            for pair in result.pairs::<String, String>() {
                let (k, v) = pair?;
                out.insert(k, v);
            }
            Ok(out)
        };
        let result = body
            .await
            .map_err(|e: mlua::Error| Error::SourceExtrasFailed {
                plugin: plugin_name.to_string(),
                message: redact_secrets(&e.to_string()),
            })?;
        self.persist_state(plugin_name)?;
        Ok(result)
    }

    /// Ask the plugin that produced `entry` for the wallpaper's
    /// editable property schema as a JSON string.
    pub async fn call_properties(
        &mut self,
        plugin_name: &str,
        entry: &WallpaperEntry,
    ) -> Result<Option<String>> {
        let callbacks = self
            .callbacks
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
        let Some(info) = self.plugin_infos.get(plugin_name) else {
            return Err(Error::SourcePluginNotFound(plugin_name.to_string()));
        };
        if !info.capabilities.wallpaper.properties {
            return Ok(None);
        }
        let props_fn: LuaFunction =
            self.lua
                .registry_value(callbacks.wallpaper_properties.as_ref().ok_or_else(|| {
                    Error::Internal(anyhow!("module.wallpaper.properties required"))
                })?)?;
        let entry_tbl = self.wallpaper_entry_table(entry)?;
        let ctx = self.build_ctx(Some(plugin_name), &[])?;
        let result: mlua::Value = self
            .call_plugin_callback_async(plugin_name, &props_fn, (entry_tbl, ctx))
            .await
            .map_err(|e| {
                Error::Internal(anyhow!(
                    "properties({plugin_name}): {}",
                    redact_secrets(&e.to_string())
                ))
            })?;
        let result = match result {
            mlua::Value::Nil => None,
            mlua::Value::String(s) => Some(s.to_str()?.to_string()),
            other => mlua_extra::json::encode(&other),
        };
        self.persist_state(plugin_name)?;
        Ok(result)
    }

    /// Ask every plugin that exports `auto_detect(ctx)` to probe
    /// well-known filesystem locations and report any that exist.
    pub async fn auto_detect_all(&mut self) -> Result<HashMap<String, Vec<String>>> {
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        let empty: [String; 0] = [];
        let mut plugin_names: Vec<String> = self.plugin_infos.keys().cloned().collect();
        plugin_names.sort();
        for name in plugin_names {
            let Some(info) = self.plugin_infos.get(&name) else {
                continue;
            };
            let Some(source) = &info.capabilities.source else {
                continue;
            };
            if !source.auto_detect {
                continue;
            }
            let callbacks = self
                .callbacks
                .get(&name)
                .ok_or_else(|| Error::SourcePluginNotFound(name.clone()))?;
            let auto_fn: LuaFunction =
                self.lua
                    .registry_value(callbacks.source_auto_detect.as_ref().ok_or_else(|| {
                        Error::Internal(anyhow!("module.source.auto_detect required"))
                    })?)?;
            let ctx = self.build_ctx(None, &empty)?;
            let results: LuaTable =
                match self.call_plugin_callback_async(&name, &auto_fn, ctx).await {
                    Ok(t) => t,
                    Err(e) => {
                        log::warn!(
                            "auto_detect plugin {name}: {}",
                            redact_secrets(&e.to_string())
                        );
                        continue;
                    }
                };
            let paths: Vec<String> = results
                .sequence_values::<String>()
                .filter_map(|v| v.ok())
                .collect();
            self.persist_state(&name)?;
            if !paths.is_empty() {
                out.insert(name, paths);
            }
        }
        Ok(out)
    }

    // -----------------------------------------------------------------------
    // Discover API — generic remote browsing relayed into plugin Lua.

    /// List plugins that opt into discovery and their declared sort/filter
    /// options.
    pub fn discover_sources(&self) -> Result<Vec<DiscoverSourceInfo>> {
        let mut out = Vec::new();
        for info in self.plugin_infos.values() {
            let Some(disc) = &info.capabilities.discover else {
                continue;
            };
            out.push(DiscoverSourceInfo {
                plugin_id: info.name.clone(),
                name: info.name.clone(),
                display_name: info.display_name.clone(),
                supports_search: disc.supports_search,
                remote_capability: disc.remote,
                remote_hint: disc.remote_hint.clone(),
                sorts: disc.sorts.clone(),
                filters: disc.filters.clone(),
                owner_plugin_id: info.plugin_id.clone(),
                settings: info.settings.clone(),
                actions: info.actions.clone(),
                status: info.status.clone(),
                avatar_url: String::new(),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Relay a discover/search request to a plugin's `discover.search(ctx, params)`
    /// Lua function. `params` is `{ query, sort, page, tags }`.
    pub async fn call_discover(
        &mut self,
        plugin_name: &str,
        query: &str,
        sort: &str,
        page: u32,
        tags: &[String],
    ) -> Result<DiscoverSearchResult> {
        let callbacks = self
            .callbacks
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
        let Some(info) = self.plugin_infos.get(plugin_name) else {
            return Err(Error::SourcePluginNotFound(plugin_name.to_string()));
        };
        let discover = info
            .capabilities
            .discover
            .as_ref()
            .ok_or_else(|| Error::DiscoverUnsupported(plugin_name.to_string()))?;
        Self::validate_discover_filter_values(plugin_name, discover, tags)?;
        let default_wp_type = info
            .capabilities
            .source
            .as_ref()
            .and_then(|source| source.types.first())
            .cloned()
            .unwrap_or_default();
        let discover_fn: LuaFunction = self.lua.registry_value(
            callbacks
                .discover_search
                .as_ref()
                .ok_or_else(|| Error::DiscoverUnsupported(plugin_name.to_string()))?,
        )?;

        let params = self.lua.create_table()?;
        params.set("query", query)?;
        params.set("sort", sort)?;
        params.set("page", page)?;
        let tags_tbl = self.lua.create_table()?;
        for (i, t) in tags.iter().enumerate() {
            tags_tbl.set(i + 1, t.clone())?;
        }
        params.set("tags", tags_tbl)?;

        let ctx = self.build_remote_ctx(plugin_name)?;
        let result: LuaTable = self
            .call_plugin_callback_async(plugin_name, &discover_fn, (ctx, params))
            .await
            .map_err(|e| Error::DiscoverFailed {
                plugin: plugin_name.to_string(),
                message: redact_secrets(&e.to_string()),
            })?;

        let mut items = Vec::new();
        let item_rows: LuaTable = result.get("items").map_err(|e| Error::DiscoverFailed {
            plugin: plugin_name.to_string(),
            message: format!("discover.search result.items required: {e}"),
        })?;
        for (idx, row) in item_rows.sequence_values::<LuaTable>().enumerate() {
            let row = row.map_err(|e| Error::DiscoverFailed {
                plugin: plugin_name.to_string(),
                message: format!(
                    "discover.search result.items[{}] must be a table: {e}",
                    idx + 1
                ),
            })?;
            let context = format!("module.discover.search result.items[{}]", idx + 1);
            items.push(DiscoverItem {
                id: Self::require_string(&row, "id", &context)?,
                title: Self::require_string(&row, "title", &context)?,
                preview_url: Self::require_string(&row, "preview_url", &context)?,
                author: Self::require_string(&row, "author", &context)?,
                wp_type: {
                    let value = Self::optional_string(&row, "wp_type", &context)?;
                    if value.is_empty() {
                        default_wp_type.clone()
                    } else {
                        value
                    }
                },
                extra: parse_lua_string_map(&row, "extra", &context)?,
            });
        }
        let has_more = result
            .get::<bool>("has_more")
            .map_err(|e| Error::DiscoverFailed {
                plugin: plugin_name.to_string(),
                message: format!("discover.search result.has_more required: {e}"),
            })?;
        self.persist_state(plugin_name)?;
        Ok(DiscoverSearchResult { items, has_more })
    }

    /// Fetch a legacy plugin's live tag taxonomy via `discover.tags(ctx)`.
    pub async fn call_tags(&mut self, plugin_name: &str) -> Result<Vec<String>> {
        let callbacks = self
            .callbacks
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
        let tags_fn: LuaFunction = self.lua.registry_value(
            callbacks
                .discover_tags
                .as_ref()
                .ok_or_else(|| Error::DiscoverUnsupported(plugin_name.to_string()))?,
        )?;
        let ctx = self.build_remote_ctx(plugin_name)?;
        let tags = self
            .call_plugin_callback_async(plugin_name, &tags_fn, ctx)
            .await
            .map_err(|e| Error::DiscoverFailed {
                plugin: plugin_name.to_string(),
                message: redact_secrets(&e.to_string()),
            })?;
        self.persist_state(plugin_name)?;
        Ok(tags)
    }

    /// Replace the compatibility filter of every legacy plugin that supplies
    /// tags dynamically. Best-effort: a failed fetch keeps its fallback.
    pub async fn refresh_dynamic_tags(&mut self) {
        let names: Vec<String> = self
            .plugin_infos
            .iter()
            .filter(|(_, info)| {
                info.capabilities
                    .discover
                    .as_ref()
                    .is_some_and(|d| d.dynamic_tags)
            })
            .map(|(name, _)| name.clone())
            .collect();
        for name in names {
            match self.call_tags(&name).await {
                Ok(tags) if !tags.is_empty() => {
                    if let Some(disc) = self
                        .plugin_infos
                        .get_mut(&name)
                        .and_then(|info| info.capabilities.discover.as_mut())
                    {
                        disc.filters = Self::legacy_discover_filter(tags);
                    }
                }
                Ok(_) => {}
                Err(e) => log::warn!("refresh discover tags for {name}: {e:#}"),
            }
        }
    }

    /// Relay a detail request to a plugin's `discover.details(ctx, id)` Lua function.
    pub async fn call_details(&mut self, plugin_name: &str, id: &str) -> Result<DiscoverDetails> {
        let callbacks = self
            .callbacks
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
        let Some(info) = self.plugin_infos.get(plugin_name) else {
            return Err(Error::SourcePluginNotFound(plugin_name.to_string()));
        };
        let Some(discover) = &info.capabilities.discover else {
            return Err(Error::DiscoverUnsupported(plugin_name.to_string()));
        };
        if !discover.supports_details {
            return Err(Error::DiscoverUnsupported(plugin_name.to_string()));
        }
        let details_fn: LuaFunction = self.lua.registry_value(
            callbacks
                .discover_details
                .as_ref()
                .ok_or_else(|| Error::DiscoverUnsupported(plugin_name.to_string()))?,
        )?;

        let ctx = self.build_remote_ctx(plugin_name)?;
        let result: LuaTable = self
            .call_plugin_callback_async(plugin_name, &details_fn, (ctx, id.to_string()))
            .await
            .map_err(|e| Error::DiscoverFailed {
                plugin: plugin_name.to_string(),
                message: redact_secrets(&e.to_string()),
            })?;

        let details = DiscoverDetails {
            author: Self::optional_string(&result, "author", "module.discover.details result")?,
            description: Self::require_string(
                &result,
                "description",
                "module.discover.details result",
            )?,
            size: Self::require_string(&result, "size", "module.discover.details result")?,
            width: result.get::<u32>("width").ok(),
            height: result.get::<u32>("height").ok(),
            tags: Self::require_string_sequence(&result, "tags", "module.discover.details result")?,
            web_url: Self::optional_string(&result, "web_url", "module.discover.details result")?,
            extra: parse_lua_string_map(&result, "extra", "module.discover.details result")?,
        };
        self.persist_state(plugin_name)?;
        Ok(details)
    }

    /// Relay a download-resolution request to a plugin's
    /// `discover.download(ctx, id)` function. The daemon owns the actual file transfer.
    pub async fn call_download(&mut self, plugin_name: &str, id: &str) -> Result<DiscoverDownload> {
        let callbacks = self
            .callbacks
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
        let Some(info) = self.plugin_infos.get(plugin_name) else {
            return Err(Error::SourcePluginNotFound(plugin_name.to_string()));
        };
        let Some(discover) = &info.capabilities.discover else {
            return Err(Error::DiscoverUnsupported(plugin_name.to_string()));
        };
        if !discover.supports_download {
            return Err(Error::DiscoverUnsupported(plugin_name.to_string()));
        }
        let download_fn: LuaFunction = self.lua.registry_value(
            callbacks
                .discover_download
                .as_ref()
                .ok_or_else(|| Error::DiscoverUnsupported(plugin_name.to_string()))?,
        )?;

        let ctx = self.build_remote_ctx(plugin_name)?;
        let result: LuaTable = self
            .call_plugin_callback_async(plugin_name, &download_fn, (ctx, id.to_string()))
            .await
            .map_err(|e| Error::DiscoverFailed {
                plugin: plugin_name.to_string(),
                message: redact_secrets(&e.to_string()),
            })?;

        let download = DiscoverDownload {
            wp_type: Self::require_string(&result, "wp_type", "module.discover.download result")?,
            url: Self::optional_string(&result, "url", "module.discover.download result")?,
            filename: Self::optional_string(
                &result,
                "filename",
                "module.discover.download result",
            )?,
            title: Self::require_string(&result, "title", "module.discover.download result")?,
            preview_url: Self::optional_string(
                &result,
                "preview_url",
                "module.discover.download result",
            )?,
            description: Self::optional_string(
                &result,
                "description",
                "module.discover.download result",
            )?,
            tags: Self::optional_string_sequence(
                &result,
                "tags",
                "module.discover.download result",
            )?,
            external_id: Self::require_string(
                &result,
                "external_id",
                "module.discover.download result",
            )?,
            size: result.get::<i64>("size").ok(),
            width: result.get::<u32>("width").ok(),
            height: result.get::<u32>("height").ok(),
            content_rating: result.get::<String>("content_rating").ok(),
        };
        self.persist_state(plugin_name)?;
        Ok(download)
    }

    /// Classify a directory fetched by a download provider. `dir` is the absolute
    /// path of the fetched item directory; returned paths are relative to it.
    pub async fn call_resolve(
        &mut self,
        plugin_name: &str,
        id: &str,
        dir: &str,
    ) -> Result<DiscoverResolve> {
        let callbacks = self
            .callbacks
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
        let Some(info) = self.plugin_infos.get(plugin_name) else {
            return Err(Error::SourcePluginNotFound(plugin_name.to_string()));
        };
        let Some(discover) = &info.capabilities.discover else {
            return Err(Error::DiscoverUnsupported(plugin_name.to_string()));
        };
        if !discover.supports_resolve {
            return Err(Error::DiscoverUnsupported(plugin_name.to_string()));
        }
        let resolve_fn: LuaFunction = self.lua.registry_value(
            callbacks
                .discover_resolve
                .as_ref()
                .ok_or_else(|| Error::DiscoverUnsupported(plugin_name.to_string()))?,
        )?;

        let ctx = self.build_ctx(Some(plugin_name), &[])?;
        let params = self.lua.create_table()?;
        params.set("id", id.to_string())?;
        params.set("dir", dir.to_string())?;
        let result: LuaTable = self
            .call_plugin_callback_async(plugin_name, &resolve_fn, (ctx, params))
            .await
            .map_err(|e| Error::ResolveFailed {
                plugin: plugin_name.to_string(),
                message: redact_secrets(&e.to_string()),
            })?;

        let resolved = DiscoverResolve {
            name: Self::require_string(&result, "name", "module.discover.resolve result")?,
            wp_type: Self::require_string(&result, "wp_type", "module.discover.resolve result")?,
            resource: Self::require_string(&result, "resource", "module.discover.resolve result")?,
            preview: result
                .get::<String>("preview")
                .ok()
                .filter(|s| !s.is_empty()),
            description: Self::optional_string(
                &result,
                "description",
                "module.discover.resolve result",
            )?,
            tags: Self::optional_string_sequence(
                &result,
                "tags",
                "module.discover.resolve result",
            )?,
            external_id: Self::optional_string(
                &result,
                "external_id",
                "module.discover.resolve result",
            )?,
            size: result.get::<i64>("size").ok(),
            content_rating: result.get::<String>("content_rating").ok(),
        };
        self.persist_state(plugin_name)?;
        Ok(resolved)
    }

    pub async fn check_lifecycle(
        &mut self,
        plugin_name: &str,
    ) -> Result<Option<PluginLifecycleCheck>> {
        let info = self
            .plugin_infos
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
        if !info.capabilities.lifecycle {
            return Ok(None);
        }
        let callbacks = self
            .callbacks
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
        let check_fn: LuaFunction = self.lua.registry_value(
            callbacks
                .lifecycle_check
                .as_ref()
                .ok_or_else(|| Error::Internal(anyhow!("module.lifecycle.check required")))?,
        )?;
        let ctx = self.build_remote_ctx(plugin_name)?;
        let result: LuaTable = self
            .call_plugin_callback_async(plugin_name, &check_fn, ctx)
            .await
            .map_err(|error| Error::DiscoverFailed {
                plugin: plugin_name.to_string(),
                message: redact_secrets(&error.to_string()),
            })?;
        let state_name = Self::require_string(&result, "state", "module.lifecycle.check result")?;
        let state = match state_name.as_str() {
            "signed_out" => PluginLifecycleState::SignedOut,
            "signed_in" => PluginLifecycleState::SignedIn,
            "expired" => PluginLifecycleState::Expired,
            "error" => PluginLifecycleState::Error,
            _ => {
                return Err(Error::Internal(anyhow!(
                    "module.lifecycle.check returned unknown state '{state_name}'"
                )))
            }
        };
        let checked = PluginLifecycleCheck {
            state,
            display_value: Self::optional_string(
                &result,
                "display_value",
                "module.lifecycle.check result",
            )?,
            error: redact_secrets(&Self::optional_string(
                &result,
                "error",
                "module.lifecycle.check result",
            )?),
            avatar_url: Self::optional_string(
                &result,
                "avatar_url",
                "module.lifecycle.check result",
            )?,
        };
        self.persist_state(plugin_name)?;
        Ok(Some(checked))
    }

    pub async fn call_action_status(
        &mut self,
        plugin_name: &str,
    ) -> Result<(Vec<SourceAction>, Vec<SourceStatus>, String)> {
        let avatar_url = self
            .check_lifecycle(plugin_name)
            .await?
            .map(|checked| checked.avatar_url)
            .unwrap_or_default();
        let info = self
            .plugin_infos
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?
            .clone();
        let mut actions = info.actions;
        let mut status = info.status;
        let callbacks = self
            .callbacks
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
        let Some(status_key) = callbacks.actions_status.as_ref() else {
            return Ok((actions, status, avatar_url));
        };
        let status_fn: LuaFunction = self.lua.registry_value(status_key)?;
        let ctx = self.build_remote_ctx(plugin_name)?;
        let result: LuaTable = self
            .call_plugin_callback_async(plugin_name, &status_fn, ctx)
            .await
            .map_err(|error| Error::DiscoverFailed {
                plugin: plugin_name.to_string(),
                message: redact_secrets(&error.to_string()),
            })?;

        if let Some(values) =
            Self::optional_table(&result, "status", "module.actions.status result")?
        {
            for row in &mut status {
                row.value = values.get::<String>(row.id.as_str()).unwrap_or_default();
            }
        }
        if let Some(values) =
            Self::optional_table(&result, "actions", "module.actions.status result")?
        {
            for action in &mut actions {
                if let Ok(state) = values.get::<LuaTable>(action.id.as_str()) {
                    action.visible = Self::optional_bool(
                        &state,
                        "visible",
                        "module.actions.status result.actions",
                        true,
                    )?;
                    action.enabled = Self::optional_bool(
                        &state,
                        "enabled",
                        "module.actions.status result.actions",
                        true,
                    )?;
                }
            }
        }
        self.persist_state(plugin_name)?;
        Ok((actions, status, avatar_url))
    }

    pub async fn invoke_action(
        &mut self,
        plugin_name: &str,
        action_id: &str,
        values: &HashMap<String, String>,
    ) -> Result<()> {
        let info = self
            .plugin_infos
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
        let action = info
            .actions
            .iter()
            .find(|action| action.id == action_id)
            .cloned()
            .ok_or_else(|| {
                Error::InvalidArgument(format!(
                    "plugin action '{plugin_name}:{action_id}' is not invokable"
                ))
            })?;
        if !matches!(
            action.kind,
            SourceActionKind::Invoke | SourceActionKind::Form
        ) {
            return Err(Error::InvalidArgument(format!(
                "plugin action '{plugin_name}:{action_id}' is not invokable"
            )));
        }
        if action.kind == SourceActionKind::Form {
            let fields: HashMap<_, _> = action
                .fields
                .iter()
                .map(|field| (field.key.as_str(), field))
                .collect();
            for key in values.keys() {
                if !fields.contains_key(key.as_str()) {
                    return Err(Error::InvalidArgument(format!(
                        "plugin action '{plugin_name}:{action_id}' has no field '{key}'"
                    )));
                }
            }
            for field in &action.fields {
                if field.required && values.get(&field.key).map_or(true, String::is_empty) {
                    return Err(Error::InvalidArgument(format!(
                        "plugin action '{plugin_name}:{action_id}' requires field '{}'",
                        field.key
                    )));
                }
            }
        } else if !values.is_empty() {
            return Err(Error::InvalidArgument(format!(
                "plugin action '{plugin_name}:{action_id}' does not accept fields"
            )));
        }
        let callbacks = self
            .callbacks
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
        let invoke_fn: LuaFunction = self.lua.registry_value(
            callbacks
                .actions_invoke
                .as_ref()
                .ok_or_else(|| Error::InvalidArgument("plugin action is unsupported".into()))?,
        )?;
        let ctx = self.build_remote_ctx(plugin_name)?;
        let value_table = self.lua.create_table()?;
        for (key, value) in values {
            value_table.set(key.as_str(), value.as_str())?;
        }
        self.call_plugin_callback_async::<()>(
            plugin_name,
            &invoke_fn,
            (ctx, action_id.to_string(), value_table),
        )
        .await
        .map_err(|error| Error::DiscoverFailed {
            plugin: plugin_name.to_string(),
            message: redact_secrets(&error.to_string()),
        })?;
        self.persist_state(plugin_name)
    }

    pub async fn begin_qr_login(
        &mut self,
        plugin_name: &str,
        action_id: &str,
    ) -> Result<QrLoginBegin> {
        let info = self
            .plugin_infos
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
        if !info
            .actions
            .iter()
            .any(|action| action.id == action_id && action.kind == SourceActionKind::QrLogin)
        {
            return Err(Error::InvalidArgument(format!(
                "plugin action '{plugin_name}:{action_id}' is not a QR login"
            )));
        }
        let callbacks = self
            .callbacks
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
        let begin_fn: LuaFunction = self.lua.registry_value(
            callbacks
                .qrlogin_begin
                .as_ref()
                .ok_or_else(|| Error::InvalidArgument("QR login is unsupported".into()))?,
        )?;
        let ctx = self.build_remote_ctx(plugin_name)?;
        let result: LuaTable = self
            .call_plugin_callback_async(plugin_name, &begin_fn, (ctx, action_id.to_string()))
            .await
            .map_err(|error| Error::DiscoverFailed {
                plugin: plugin_name.to_string(),
                message: redact_secrets(&error.to_string()),
            })?;
        let key = result
            .get::<LuaValue>("key")
            .map_err(|error| Error::Internal(anyhow!("qrlogin.begin result.key: {error}")))?;
        if matches!(key, LuaValue::Nil) {
            return Err(Error::Internal(anyhow!(
                "qrlogin.begin result.key must not be nil"
            )));
        }
        let challenge = Self::require_string(&result, "challenge", "qrlogin.begin result")?;
        let poll_after_ms = result.get::<Option<u64>>("poll_after_ms")?.unwrap_or(1000);
        let expires_in_ms = result.get::<Option<u64>>("expires_in_ms")?;
        let title = Self::optional_string(&result, "title", "qrlogin.begin result")?;
        let instruction = Self::optional_string(&result, "instruction", "qrlogin.begin result")?;
        let operation_id = self.next_qr_operation_id;
        self.next_qr_operation_id = self.next_qr_operation_id.wrapping_add(1).max(1);
        self.qr_operations
            .insert(operation_id, self.lua.create_registry_value(key)?);
        Ok(QrLoginBegin {
            operation_id,
            challenge,
            poll_after_ms,
            expires_in_ms,
            title,
            instruction,
        })
    }

    pub async fn poll_qr_login(
        &mut self,
        plugin_name: &str,
        operation_id: u64,
    ) -> Result<QrLoginPoll> {
        let callbacks = self
            .callbacks
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
        let poll_fn: LuaFunction = self.lua.registry_value(
            callbacks
                .qrlogin_poll
                .as_ref()
                .ok_or_else(|| Error::InvalidArgument("QR login is unsupported".into()))?,
        )?;
        let key = self
            .qr_operations
            .get(&operation_id)
            .ok_or_else(|| Error::InvalidArgument("QR login operation is not active".into()))?;
        let opaque: LuaValue = self.lua.registry_value(key)?;
        let ctx = self.build_remote_ctx(plugin_name)?;
        let result: LuaTable = self
            .call_plugin_callback_async(plugin_name, &poll_fn, (ctx, opaque))
            .await
            .map_err(|error| Error::DiscoverFailed {
                plugin: plugin_name.to_string(),
                message: redact_secrets(&error.to_string()),
            })?;
        let state_name = Self::require_string(&result, "state", "qrlogin.poll result")?;
        let state = match state_name.as_str() {
            "awaiting_scan" => QrLoginPollState::AwaitingScan,
            "awaiting_confirmation" => QrLoginPollState::AwaitingConfirmation,
            "challenge_changed" => QrLoginPollState::ChallengeChanged,
            "succeeded" => QrLoginPollState::Succeeded,
            "expired" => QrLoginPollState::Expired,
            "failed" => QrLoginPollState::Failed,
            _ => {
                return Err(Error::Internal(anyhow!(
                    "qrlogin.poll returned unknown state '{state_name}'"
                )))
            }
        };
        if matches!(
            state,
            QrLoginPollState::Succeeded | QrLoginPollState::Expired | QrLoginPollState::Failed
        ) {
            if let Some(key) = self.qr_operations.remove(&operation_id) {
                self.lua.remove_registry_value(key)?;
            }
        }
        if state == QrLoginPollState::Succeeded {
            self.persist_state(plugin_name)?;
        }
        Ok(QrLoginPoll {
            state,
            challenge: Self::optional_string(&result, "challenge", "qrlogin.poll result")?,
            poll_after_ms: result.get::<Option<u64>>("poll_after_ms")?,
            display_value: Self::optional_string(&result, "display_value", "qrlogin.poll result")?,
            error: redact_secrets(&Self::optional_string(
                &result,
                "error",
                "qrlogin.poll result",
            )?),
        })
    }

    pub async fn cancel_qr_login(&mut self, plugin_name: &str, operation_id: u64) -> Result<()> {
        let Some(key) = self.qr_operations.remove(&operation_id) else {
            return Ok(());
        };
        let opaque: LuaValue = self.lua.registry_value(&key)?;
        let callback_result = if let Some(cancel_key) = self
            .callbacks
            .get(plugin_name)
            .and_then(|callbacks| callbacks.qrlogin_cancel.as_ref())
        {
            let cancel_fn: LuaFunction = self.lua.registry_value(cancel_key)?;
            let ctx = self.build_remote_ctx(plugin_name)?;
            self.call_plugin_callback_async::<()>(plugin_name, &cancel_fn, (ctx, opaque))
                .await
                .map_err(|error| Error::DiscoverFailed {
                    plugin: plugin_name.to_string(),
                    message: redact_secrets(&error.to_string()),
                })
        } else {
            Ok(())
        };
        let remove_result = self.lua.remove_registry_value(key);
        callback_result?;
        remove_result?;
        Ok(())
    }

    pub async fn subscription_status(
        &mut self,
        plugin_name: &str,
        ids: &[String],
    ) -> Result<Vec<SubscriptionItemState>> {
        self.require_remote_capability(plugin_name, RemoteCapability::Subscription)?;
        let callbacks = self
            .callbacks
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
        let status_fn: LuaFunction = self.lua.registry_value(
            callbacks
                .subscription_status
                .as_ref()
                .ok_or_else(|| Error::DiscoverUnsupported(plugin_name.to_string()))?,
        )?;
        let id_table = self.lua.create_table()?;
        for (index, id) in ids.iter().enumerate() {
            id_table.set(index + 1, id.clone())?;
        }
        let ctx = self.build_remote_ctx(plugin_name)?;
        let result: LuaTable = self
            .call_plugin_callback_async(plugin_name, &status_fn, (ctx, id_table))
            .await
            .map_err(|error| Error::DiscoverFailed {
                plugin: plugin_name.to_string(),
                message: redact_secrets(&error.to_string()),
            })?;
        let mut states = Vec::with_capacity(ids.len());
        for id in ids {
            let value = result.get::<String>(id.as_str()).unwrap_or_default();
            let state = match value.as_str() {
                "subscribed" => SubscriptionState::Subscribed,
                "unsubscribed" => SubscriptionState::Unsubscribed,
                _ => SubscriptionState::Unknown,
            };
            states.push(SubscriptionItemState {
                id: id.clone(),
                state,
            });
        }
        self.persist_state(plugin_name)?;
        Ok(states)
    }

    pub async fn set_subscription(
        &mut self,
        plugin_name: &str,
        id: &str,
        subscribed: bool,
    ) -> Result<()> {
        self.require_remote_capability(plugin_name, RemoteCapability::Subscription)?;
        let callbacks = self
            .callbacks
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
        let callback = if subscribed {
            callbacks.subscription_subscribe.as_ref()
        } else {
            callbacks.subscription_unsubscribe.as_ref()
        }
        .ok_or_else(|| Error::DiscoverUnsupported(plugin_name.to_string()))?;
        let function: LuaFunction = self.lua.registry_value(callback)?;
        let ctx = self.build_remote_ctx(plugin_name)?;
        let result: LuaTable = self
            .call_plugin_callback_async(plugin_name, &function, (ctx, id.to_string()))
            .await
            .map_err(|error| Error::DiscoverFailed {
                plugin: plugin_name.to_string(),
                message: redact_secrets(&error.to_string()),
            })?;
        let accepted = result
            .get::<bool>("accepted")
            .map_err(|error| Error::DiscoverFailed {
                plugin: plugin_name.to_string(),
                message: format!("subscription mutation result.accepted required: {error}"),
            })?;
        if !accepted {
            let message =
                Self::optional_string(&result, "error", "module.subscription mutation result")?;
            return Err(Error::DiscoverFailed {
                plugin: plugin_name.to_string(),
                message: if message.is_empty() {
                    "subscription mutation was not accepted".into()
                } else {
                    redact_secrets(&message)
                },
            });
        }
        self.persist_state(plugin_name)
    }

    fn require_remote_capability(
        &self,
        plugin_name: &str,
        expected: RemoteCapability,
    ) -> Result<()> {
        let actual = self
            .plugin_infos
            .get(plugin_name)
            .and_then(|info| info.capabilities.discover.as_ref())
            .and_then(|discover| discover.remote);
        if actual != Some(expected) {
            return Err(Error::InvalidArgument(format!(
                "remote capability mismatch for '{plugin_name}': expected {expected:?}, got {actual:?}"
            )));
        }
        Ok(())
    }

    pub fn plugins(&self) -> Result<Vec<SourcePluginInfo>> {
        let mut out = Vec::new();
        for info in self.plugin_infos.values() {
            let Some(source) = &info.capabilities.source else {
                continue;
            };
            out.push(SourcePluginInfo {
                name: info.name.clone(),
                plugin_id: info.plugin_id.clone(),
                types: source.types.clone(),
                version: info.version.clone(),
                library_label: source.library_label.clone(),
                library_hint: source.library_hint.clone(),
                settings: info.settings.clone(),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub fn supports_item_remove(&self, plugin_name: &str) -> bool {
        self.plugin_infos
            .get(plugin_name)
            .map(|info| info.capabilities.source_item_remove)
            .unwrap_or(false)
    }

    pub async fn remove_item(
        &mut self,
        plugin_name: &str,
        entry: &WallpaperEntry,
        libraries: &[String],
    ) -> Result<()> {
        let callbacks = self
            .callbacks
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
        let Some(info) = self.plugin_infos.get(plugin_name) else {
            return Err(Error::SourcePluginNotFound(plugin_name.to_string()));
        };
        if !info.capabilities.source_item_remove {
            return Err(Error::SourceItemRemoveUnsupported(plugin_name.to_string()));
        }

        let remove_fn: LuaFunction = self.lua.registry_value(
            callbacks
                .source_remove
                .as_ref()
                .ok_or_else(|| Error::SourceItemRemoveUnsupported(plugin_name.to_string()))?,
        )?;
        let ctx = self.build_ctx(Some(plugin_name), libraries)?;
        let entry_tbl = self.wallpaper_entry_table(entry)?;
        self.call_plugin_callback_async::<()>(plugin_name, &remove_fn, (ctx, entry_tbl))
            .await
            .map_err(|e| Error::SourceItemRemoveFailed {
                plugin: plugin_name.to_string(),
                message: redact_secrets(&e.to_string()),
            })?;
        self.persist_state(plugin_name)?;
        Ok(())
    }

    pub fn plugin_version(&self, plugin_name: &str) -> Option<String> {
        self.plugin_infos
            .get(plugin_name)
            .map(|info| info.version.clone())
    }
}

impl Drop for LuaPluginRuntime {
    fn drop(&mut self) {
        self.persist_all_http_sessions();
    }
}

#[derive(Default)]
pub struct SourceCatalog {
    entries: Vec<WallpaperEntry>,
    by_type: HashMap<WallpaperType, Vec<usize>>,
}

impl SourceCatalog {
    fn replace(&mut self, entries: Vec<WallpaperEntry>) {
        self.by_type.clear();
        for (idx, entry) in entries.iter().enumerate() {
            self.by_type
                .entry(entry.wp_type.clone())
                .or_default()
                .push(idx);
        }
        self.entries = entries;
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.by_type.clear();
    }
}

struct PluginHandle {
    runtime: Arc<tokio::sync::Mutex<LuaPluginRuntime>>,
    info: StdRwLock<LoadedPluginInfo>,
    generation_state: Arc<AtomicU8>,
}

/// Registry and source catalog for Lua plugins.
///
/// Each plugin owns its Lua VM and async mutex. Registry/catalog locks are only
/// held while cloning handles or replacing snapshots, so callbacks belonging to
/// different plugins can make progress concurrently.
pub struct LuaPluginRegistry {
    plugins: StdRwLock<HashMap<String, Arc<PluginHandle>>>,
    catalog: StdRwLock<SourceCatalog>,
    probe: Arc<dyn MediaProbe>,
    db: StdRwLock<Option<DatabaseConnection>>,
    settings: StdRwLock<Option<Arc<crate::settings::SettingsStore>>>,
    state_store: crate::plugin::state_store::PluginStateStore,
}

pub type SourceManager = LuaPluginRegistry;

impl LuaPluginRegistry {
    pub fn new() -> Result<Self> {
        Self::with_probe(Arc::new(AvFormatProbe::new()))
    }

    pub fn with_probe(probe: Arc<dyn MediaProbe>) -> Result<Self> {
        Self::with_probe_and_state_store(
            probe,
            crate::plugin::state_store::PluginStateStore::standard(),
        )
    }

    fn with_probe_and_state_store(
        probe: Arc<dyn MediaProbe>,
        state_store: crate::plugin::state_store::PluginStateStore,
    ) -> Result<Self> {
        Ok(Self {
            plugins: StdRwLock::new(HashMap::new()),
            catalog: StdRwLock::new(SourceCatalog::default()),
            probe,
            db: StdRwLock::new(None),
            settings: StdRwLock::new(None),
            state_store,
        })
    }

    pub fn attach_db(&self, db: DatabaseConnection) {
        *self.db.write().expect("plugin DB lock poisoned") = Some(db);
    }

    pub fn attach_settings(&self, settings: Arc<crate::settings::SettingsStore>) {
        *self
            .settings
            .write()
            .expect("plugin settings lock poisoned") = Some(settings);
    }

    pub fn clear_plugins(&self) {
        let mut plugins = self.plugins.write().expect("plugin registry lock poisoned");
        for handle in plugins.values() {
            handle
                .generation_state
                .store(RUNTIME_INACTIVE, Ordering::Release);
        }
        plugins.clear();
        drop(plugins);
        self.catalog
            .write()
            .expect("source catalog lock poisoned")
            .clear();
    }

    pub async fn suspend_plugins(&self) {
        let handles = self.handles();
        for (_, handle) in &handles {
            handle
                .generation_state
                .store(RUNTIME_DRAINING, Ordering::Release);
        }
        for (_, handle) in handles {
            let _runtime = handle.runtime.lock().await;
        }
    }

    pub fn resume_plugins(&self) {
        for (_, handle) in self.handles() {
            let _ = handle.generation_state.compare_exchange(
                RUNTIME_DRAINING,
                RUNTIME_ACTIVE,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }

    pub fn retain_plugins_from(&self, current: &Self, plugin_ids: &HashSet<String>) -> Result<()> {
        let current = current
            .plugins
            .read()
            .expect("plugin registry lock poisoned");
        let mut replacement = self.plugins.write().expect("plugin registry lock poisoned");
        for (name, handle) in current.iter() {
            let plugin_id = &handle
                .info
                .read()
                .expect("plugin info lock poisoned")
                .plugin_id;
            if !plugin_ids.contains(plugin_id) {
                continue;
            }
            if replacement.contains_key(name) {
                return Err(Error::Internal(anyhow!(
                    "duplicate retained Lua plugin name '{name}'"
                )));
            }
            replacement.insert(name.clone(), handle.clone());
        }
        Ok(())
    }

    pub fn replace_plugins(&self, replacement: Self) -> Result<()> {
        let plugins = replacement
            .plugins
            .into_inner()
            .map_err(|_| Error::Internal(anyhow!("replacement plugin registry lock poisoned")))?;
        let catalog = replacement
            .catalog
            .into_inner()
            .map_err(|_| Error::Internal(anyhow!("replacement source catalog lock poisoned")))?;
        let mut current = self.plugins.write().expect("plugin registry lock poisoned");
        for handle in current.values() {
            handle
                .generation_state
                .store(RUNTIME_INACTIVE, Ordering::Release);
        }
        *current = plugins;
        for handle in current.values() {
            handle
                .generation_state
                .store(RUNTIME_ACTIVE, Ordering::Release);
        }
        drop(current);
        *self.catalog.write().expect("source catalog lock poisoned") = catalog;
        Ok(())
    }

    pub fn load_plugin(
        &self,
        path: &Path,
        plugin_id: &str,
        plugin_version: &str,
        entry_version: u32,
    ) -> Result<String> {
        let mut runtime = LuaPluginRuntime::with_probe(self.probe.clone())?;
        runtime.state_store = self.state_store.clone();
        if let Some(db) = self.db.read().expect("plugin DB lock poisoned").clone() {
            runtime.attach_db(db);
        }
        if let Some(settings) = self
            .settings
            .read()
            .expect("plugin settings lock poisoned")
            .clone()
        {
            runtime.attach_settings(settings);
        }
        let name = runtime.load_plugin(path, plugin_id, plugin_version, entry_version)?;
        let info = runtime
            .plugin_infos
            .get(&name)
            .cloned()
            .ok_or_else(|| Error::SourcePluginNotFound(name.clone()))?;
        let generation_state = runtime.generation_state.clone();
        let handle = Arc::new(PluginHandle {
            runtime: Arc::new(tokio::sync::Mutex::new(runtime)),
            info: StdRwLock::new(info),
            generation_state,
        });
        let mut plugins = self.plugins.write().expect("plugin registry lock poisoned");
        if plugins.contains_key(&name) {
            return Err(Error::Internal(anyhow!(
                "duplicate Lua plugin name '{name}'"
            )));
        }
        plugins.insert(name.clone(), handle);
        Ok(name)
    }

    fn handle(&self, plugin_name: &str) -> Result<Arc<PluginHandle>> {
        self.plugins
            .read()
            .expect("plugin registry lock poisoned")
            .get(plugin_name)
            .cloned()
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))
    }

    fn handles(&self) -> Vec<(String, Arc<PluginHandle>)> {
        self.plugins
            .read()
            .expect("plugin registry lock poisoned")
            .iter()
            .map(|(name, handle)| (name.clone(), handle.clone()))
            .collect()
    }

    pub async fn scan_all(&self, libs_by_plugin: &HashMap<String, Vec<String>>) -> Result<()> {
        let scans = self.handles().into_iter().filter_map(|(name, handle)| {
            let has_source = handle
                .info
                .read()
                .expect("plugin info lock poisoned")
                .capabilities
                .source
                .is_some();
            has_source.then(|| {
                let libraries = libs_by_plugin.get(&name).cloned().unwrap_or_default();
                async move {
                    let mut runtime = handle.runtime.lock().await;
                    let mut only_this = HashMap::new();
                    only_this.insert(name.clone(), libraries);
                    runtime.scan_all(&only_this).await?;
                    Ok::<_, Error>(runtime.list().to_vec())
                }
            })
        });
        let results = futures_util::future::join_all(scans).await;
        let mut entries = Vec::new();
        // The catalog is still replaced with whatever the healthy plugins
        // returned, so one broken source does not hide the rest. The failures
        // are reported afterwards: a scan that failed must not be presented as
        // a scan that simply found nothing.
        let mut failures: Vec<String> = Vec::new();
        for result in results {
            match result {
                Ok(mut plugin_entries) => entries.append(&mut plugin_entries),
                Err(e) => {
                    log::warn!("scan Lua plugin failed: {e}");
                    failures.push(format!("{e:#}"));
                }
            }
        }
        entries.sort_by(|a, b| {
            a.plugin_name
                .cmp(&b.plugin_name)
                .then_with(|| a.resource.cmp(&b.resource))
        });
        self.catalog
            .write()
            .expect("source catalog lock poisoned")
            .replace(entries);
        if !failures.is_empty() {
            return Err(Error::Internal(anyhow!(
                "source scan failed: {}",
                failures.join("; ")
            )));
        }
        Ok(())
    }

    pub fn list(&self) -> Vec<WallpaperEntry> {
        self.catalog
            .read()
            .expect("source catalog lock poisoned")
            .entries
            .clone()
    }

    pub fn list_by_type(&self, wp_type: &str) -> Vec<WallpaperEntry> {
        let catalog = self.catalog.read().expect("source catalog lock poisoned");
        catalog
            .by_type
            .get(wp_type)
            .map(|indices| {
                indices
                    .iter()
                    .map(|&idx| catalog.entries[idx].clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn get(&self, id: &str) -> Option<WallpaperEntry> {
        self.catalog
            .read()
            .expect("source catalog lock poisoned")
            .entries
            .iter()
            .find(|entry| entry.item_id.to_string() == id)
            .cloned()
    }

    pub async fn call_extras(
        &self,
        plugin_name: &str,
        entry: &WallpaperEntry,
    ) -> Result<HashMap<String, String>> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .call_extras(plugin_name, entry)
            .await
    }

    pub fn action_kind(&self, plugin_name: &str, action_id: &str) -> Option<SourceActionKind> {
        self.handle(plugin_name).ok().and_then(|handle| {
            handle
                .info
                .read()
                .expect("plugin info lock poisoned")
                .actions
                .iter()
                .find(|action| action.id == action_id)
                .map(|action| action.kind)
        })
    }

    pub async fn check_lifecycle(&self, plugin_name: &str) -> Result<Option<PluginLifecycleCheck>> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .check_lifecycle(plugin_name)
            .await
    }

    pub async fn invoke_action(
        &self,
        plugin_name: &str,
        action_id: &str,
        values: &HashMap<String, String>,
    ) -> Result<()> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .invoke_action(plugin_name, action_id, values)
            .await
    }

    pub async fn begin_qr_login(&self, plugin_name: &str, action_id: &str) -> Result<QrLoginBegin> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .begin_qr_login(plugin_name, action_id)
            .await
    }

    pub async fn poll_qr_login(&self, plugin_name: &str, operation_id: u64) -> Result<QrLoginPoll> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .poll_qr_login(plugin_name, operation_id)
            .await
    }

    pub async fn cancel_qr_login(&self, plugin_name: &str, operation_id: u64) -> Result<()> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .cancel_qr_login(plugin_name, operation_id)
            .await
    }

    pub async fn subscription_status(
        &self,
        plugin_name: &str,
        ids: &[String],
    ) -> Result<Vec<SubscriptionItemState>> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .subscription_status(plugin_name, ids)
            .await
    }

    pub async fn set_subscription(
        &self,
        plugin_name: &str,
        id: &str,
        subscribed: bool,
    ) -> Result<()> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .set_subscription(plugin_name, id, subscribed)
            .await
    }

    pub async fn call_properties(
        &self,
        plugin_name: &str,
        entry: &WallpaperEntry,
    ) -> Result<Option<String>> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .call_properties(plugin_name, entry)
            .await
    }

    pub async fn auto_detect_all(&self) -> Result<HashMap<String, Vec<String>>> {
        let calls = self
            .handles()
            .into_iter()
            .map(|(_, handle)| async move { handle.runtime.lock().await.auto_detect_all().await });
        let mut out = HashMap::new();
        for result in futures_util::future::join_all(calls).await {
            out.extend(result?);
        }
        Ok(out)
    }

    pub fn discover_sources(&self) -> Result<Vec<DiscoverSourceInfo>> {
        let mut out = Vec::new();
        for (_, handle) in self.handles() {
            let info = handle.info.read().expect("plugin info lock poisoned");
            let Some(disc) = &info.capabilities.discover else {
                continue;
            };
            out.push(DiscoverSourceInfo {
                plugin_id: info.name.clone(),
                name: info.name.clone(),
                display_name: info.display_name.clone(),
                supports_search: disc.supports_search,
                remote_capability: disc.remote,
                remote_hint: disc.remote_hint.clone(),
                sorts: disc.sorts.clone(),
                filters: disc.filters.clone(),
                owner_plugin_id: info.plugin_id.clone(),
                settings: info.settings.clone(),
                actions: info.actions.clone(),
                status: info.status.clone(),
                avatar_url: String::new(),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Validate and canonicalize a partial settings update for one discover
    /// source. The Lua declaration is authoritative; callers cannot add
    /// undeclared keys to the source's config table.
    pub fn validate_remote_settings_patch(
        &self,
        source_id: &str,
        values: HashMap<String, String>,
    ) -> Result<HashMap<String, String>> {
        let handle = self.handle(source_id)?;
        let info = handle.info.read().expect("plugin info lock poisoned");
        if info.capabilities.discover.is_none() {
            return Err(Error::DiscoverUnsupported(source_id.to_string()));
        }

        let schemas: HashMap<_, _> = info
            .settings
            .iter()
            .map(|setting| (setting.key.as_str(), setting))
            .collect();
        let mut validated = HashMap::with_capacity(values.len());
        for (key, raw) in values {
            let setting = schemas.get(key.as_str()).ok_or_else(|| {
                Error::SettingsValidationFailed(format!(
                    "{source_id}.{key} is not declared by the remote source"
                ))
            })?;
            let value = validate_source_setting(setting, &raw)
                .map_err(|error| Error::SettingsValidationFailed(format!("{source_id}.{error}")))?;
            validated.insert(key, value);
        }
        Ok(validated)
    }

    pub async fn discover_sources_with_status(&self) -> Result<Vec<DiscoverSourceInfo>> {
        let calls = self.handles().into_iter().filter_map(|(name, handle)| {
            let has_discover = handle
                .info
                .read()
                .expect("plugin info lock poisoned")
                .capabilities
                .discover
                .is_some();
            has_discover.then(|| async move {
                let result = handle.runtime.lock().await.call_action_status(&name).await;
                (name, result)
            })
        });
        let dynamic: HashMap<_, _> = futures_util::future::join_all(calls)
            .await
            .into_iter()
            .filter_map(|(name, result)| match result {
                Ok(value) => Some((name, value)),
                Err(error) => {
                    log::warn!("plugin action status failed: {error}");
                    None
                }
            })
            .collect();
        let mut sources = self.discover_sources()?;
        for source in &mut sources {
            if let Some((actions, status, avatar_url)) = dynamic.get(&source.plugin_id) {
                source.actions = actions.clone();
                source.status = status.clone();
                source.avatar_url = avatar_url.clone();
            }
        }
        Ok(sources)
    }

    pub async fn call_discover(
        &self,
        plugin_name: &str,
        query: &str,
        sort: &str,
        page: u32,
        tags: &[String],
    ) -> Result<DiscoverSearchResult> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .call_discover(plugin_name, query, sort, page, tags)
            .await
    }

    pub async fn call_tags(&self, plugin_name: &str) -> Result<Vec<String>> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .call_tags(plugin_name)
            .await
    }

    pub async fn refresh_dynamic_tags(&self) {
        let handles: Vec<_> = self
            .handles()
            .into_iter()
            .filter(|(_, handle)| {
                handle
                    .info
                    .read()
                    .expect("plugin info lock poisoned")
                    .capabilities
                    .discover
                    .as_ref()
                    .is_some_and(|discover| discover.dynamic_tags)
            })
            .collect();
        for (name, handle) in handles {
            match handle.runtime.lock().await.call_tags(&name).await {
                Ok(tags) if !tags.is_empty() => {
                    if let Some(discover) = handle
                        .info
                        .write()
                        .expect("plugin info lock poisoned")
                        .capabilities
                        .discover
                        .as_mut()
                    {
                        discover.filters = LuaPluginRuntime::legacy_discover_filter(tags);
                    }
                }
                Ok(_) => {}
                Err(e) => log::warn!("refresh discover tags for {name}: {e:#}"),
            }
        }
    }

    pub async fn call_details(&self, plugin_name: &str, id: &str) -> Result<DiscoverDetails> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .call_details(plugin_name, id)
            .await
    }

    pub async fn call_download(&self, plugin_name: &str, id: &str) -> Result<DiscoverDownload> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .call_download(plugin_name, id)
            .await
    }

    pub async fn call_resolve(
        &self,
        plugin_name: &str,
        id: &str,
        dir: &str,
    ) -> Result<DiscoverResolve> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .call_resolve(plugin_name, id, dir)
            .await
    }

    pub fn plugins(&self) -> Result<Vec<SourcePluginInfo>> {
        let mut out = Vec::new();
        for (_, handle) in self.handles() {
            let info = handle.info.read().expect("plugin info lock poisoned");
            let Some(source) = &info.capabilities.source else {
                continue;
            };
            out.push(SourcePluginInfo {
                name: info.name.clone(),
                plugin_id: info.plugin_id.clone(),
                types: source.types.clone(),
                version: info.version.clone(),
                library_label: source.library_label.clone(),
                library_hint: source.library_hint.clone(),
                settings: info.settings.clone(),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub fn supports_item_remove(&self, plugin_name: &str) -> bool {
        self.handle(plugin_name).ok().is_some_and(|handle| {
            handle
                .info
                .read()
                .expect("plugin info lock poisoned")
                .capabilities
                .source_item_remove
        })
    }

    pub fn supports_item_unsubscribe(&self, entry: &WallpaperEntry) -> bool {
        if entry.external_id.as_deref().is_none_or(str::is_empty) {
            return false;
        }
        self.handle(&entry.plugin_name).ok().is_some_and(|handle| {
            handle
                .info
                .read()
                .expect("plugin info lock poisoned")
                .capabilities
                .discover
                .as_ref()
                .and_then(|discover| discover.remote)
                == Some(RemoteCapability::Subscription)
        })
    }

    pub async fn remove_item(
        &self,
        plugin_name: &str,
        entry: &WallpaperEntry,
        libraries: &[String],
    ) -> Result<()> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .remove_item(plugin_name, entry, libraries)
            .await
    }

    pub fn plugin_version(&self, plugin_name: &str) -> Option<String> {
        self.handle(plugin_name).ok().map(|handle| {
            handle
                .info
                .read()
                .expect("plugin info lock poisoned")
                .version
                .clone()
        })
    }

    #[cfg(test)]
    fn test_runtime(&self, plugin_name: &str) -> Arc<tokio::sync::Mutex<LuaPluginRuntime>> {
        self.handle(plugin_name).unwrap().runtime.clone()
    }

    #[cfg(test)]
    pub(crate) async fn set_test_callback_timeout(&self, plugin_name: &str, timeout: Duration) {
        self.test_runtime(plugin_name).lock().await.callback_timeout = timeout;
    }
}

// ---------------------------------------------------------------------------
// Helpers

fn parse_lua_string_map(
    tbl: &LuaTable,
    key: &str,
    context: &str,
) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    let Some(meta) = LuaPluginRuntime::optional_table(tbl, key, context)? else {
        return Ok(map);
    };
    for pair in meta.pairs::<String, String>() {
        let (k, v) = pair
            .map_err(|e| Error::Internal(anyhow!("{context}.{key} must be a string map: {e}")))?;
        map.insert(k, v);
    }
    Ok(map)
}

fn redact_secrets(message: &str) -> String {
    let mut out = message.to_string();
    for marker in ["Authorization:", "authorization:", "Cookie:", "cookie:"] {
        let mut from = 0;
        while let Some(relative) = out[from..].find(marker) {
            let start = from + relative + marker.len();
            let end = out[start..]
                .find(['\r', '\n'])
                .map(|relative| start + relative)
                .unwrap_or(out.len());
            out.replace_range(start..end, " [REDACTED]");
            from = start + " [REDACTED]".len();
        }
    }
    for marker in [
        "access_token=",
        "refresh_token=",
        "authorization=",
        "cookie=",
        "\"access_token\":\"",
        "\"access_token\": \"",
        "\"refresh_token\":\"",
        "\"refresh_token\": \"",
    ] {
        let mut from = 0;
        while let Some(relative) = out[from..].find(marker) {
            let start = from + relative + marker.len();
            let end = out[start..]
                .find(|character: char| {
                    character.is_whitespace()
                        || matches!(character, '&' | ',' | '}' | ']' | '"' | '\'')
                })
                .map(|relative| start + relative)
                .unwrap_or(out.len());
            out.replace_range(start..end, "[REDACTED]");
            from = start + "[REDACTED]".len();
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::media::{MediaMeta, MediaProbe};
    use std::io::Write;
    use std::time::Duration;

    struct FakeProbe {
        meta: MediaMeta,
    }

    #[test]
    fn remote_errors_redact_credentials() {
        let redacted = redact_secrets(
            "access_token=access-secret&refresh_token=refresh-secret\nAuthorization: Bearer bearer-secret\nCookie: session-secret\n{\"access_token\": \"json-secret\"}",
        );
        assert!(!redacted.contains("access-secret"));
        assert!(!redacted.contains("refresh-secret"));
        assert!(!redacted.contains("bearer-secret"));
        assert!(!redacted.contains("session-secret"));
        assert!(!redacted.contains("json-secret"));
    }

    #[test]
    fn discover_callback_errors_do_not_expose_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("redaction.lua");
        std::fs::write(
            &entry,
            r#"
local M = {}
function M.info()
    return { name = "redaction", capabilities = { discover = { search = true } } }
end
M.discover = {}
function M.discover.search(ctx, params)
    error("request failed: access_token=access-secret&refresh_token=refresh-secret")
end
return M
"#,
        )
        .unwrap();
        let manager = SourceManager::new().unwrap();
        manager
            .load_plugin(&entry, "org.redaction", "1", ENTRY_VERSION_V3)
            .unwrap();
        let error = block_value(async { manager.call_discover("redaction", "", "", 1, &[]).await })
            .unwrap_err()
            .to_string();
        assert!(!error.contains("access-secret"));
        assert!(!error.contains("refresh-secret"));
        assert!(error.contains("[REDACTED]"));
    }

    #[test]
    fn remote_settings_patch_uses_source_schema() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("settings.lua");
        std::fs::write(
            &entry,
            r#"
local M = {}
function M.info()
    return {
        name = "settings_remote",
        settings = {
            { key = "count", type = "u32", default = 1 },
            { key = "enabled", type = "bool", default = false },
            { key = "mode", type = "string", default = "one", choices = { "one", "two" } },
        },
        capabilities = { discover = { search = true } },
    }
end
M.discover = {}
function M.discover.search(ctx, params)
    return { items = {}, has_more = false }
end
return M
"#,
        )
        .unwrap();
        let manager = SourceManager::new().unwrap();
        manager
            .load_plugin(&entry, "org.settings", "1", ENTRY_VERSION_V3)
            .unwrap();

        let values = HashMap::from([
            ("count".to_string(), "007".to_string()),
            ("enabled".to_string(), "true".to_string()),
            ("mode".to_string(), "two".to_string()),
        ]);
        let validated = manager
            .validate_remote_settings_patch("settings_remote", values)
            .unwrap();
        assert_eq!(validated.get("count").map(String::as_str), Some("7"));
        assert_eq!(validated.get("enabled").map(String::as_str), Some("true"));
        assert_eq!(validated.get("mode").map(String::as_str), Some("two"));

        for values in [
            HashMap::from([("missing".to_string(), "value".to_string())]),
            HashMap::from([("enabled".to_string(), "yes".to_string())]),
            HashMap::from([("mode".to_string(), "three".to_string())]),
        ] {
            assert!(manager
                .validate_remote_settings_patch("settings_remote", values)
                .is_err());
        }
    }

    #[test]
    fn source_settings_reject_invalid_schema() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("settings.lua");
        let script = |settings: &str| {
            format!(
                r#"
local M = {{}}
function M.info()
    return {{
        name = "invalid_settings",
        settings = {{ {settings} }},
        capabilities = {{ discover = {{ search = true }} }},
    }}
end
M.discover = {{}}
function M.discover.search(ctx, params)
    return {{ items = {{}}, has_more = false }}
end
return M
"#,
            )
        };

        std::fs::write(&entry, script(r#"{ key = "same" }, { key = "same" }"#)).unwrap();
        assert!(SourceManager::new()
            .unwrap()
            .load_plugin(&entry, "org.invalid", "1", ENTRY_VERSION_V3)
            .is_err());

        std::fs::write(&entry, script(r#"{ key = "value", type = "object" }"#)).unwrap();
        assert!(SourceManager::new()
            .unwrap()
            .load_plugin(&entry, "org.invalid", "1", ENTRY_VERSION_V3)
            .is_err());
    }

    impl MediaProbe for FakeProbe {
        fn probe_media(&self, _path: &str) -> MediaMeta {
            self.meta.clone()
        }
    }

    /// Drive an async scan from a sync `#[test]` — these tests don't
    /// touch the DB so a single-thread runtime is fine.
    fn block(fut: impl std::future::Future<Output = ()>) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }

    fn block_value<T>(fut: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }

    #[test]
    fn ctx_probe_callable_from_lua() {
        let probe = Arc::new(FakeProbe {
            meta: MediaMeta {
                width: Some(1920),
                height: Some(1080),
            },
        });
        let dir = tempfile::tempdir().unwrap();
        let plugin_path = dir.path().join("probe_test.lua");
        let mut f = std::fs::File::create(&plugin_path).unwrap();
        write!(
            f,
            r#"
local M = {{}}
function M.info()
    return {{
        name = "probe_test",
        capabilities = {{
            source = {{ types = {{"video"}}, scan = true }},
        }},
    }}
end
M.source = {{}}
function M.source.scan(ctx)
    local m = ctx.probe("/fake/path/video.mp4")
    if m == nil then error("probe returned nil") end
    return {{
        {{
            id = "v1",
            name = "Video",
            wp_type = "video",
            resource = "/lib/v1.mp4",
            library_root = "/lib",
            metadata = {{}},
            _probe_size = m.size,
            _probe_width = m.width,
            _probe_height = m.height,
        }},
    }}
end
return M
"#
        )
        .unwrap();

        let mgr = SourceManager::with_probe(probe as Arc<dyn MediaProbe>).unwrap();
        mgr.load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION)
            .unwrap();
        block(async { mgr.scan_all(&HashMap::new()).await.unwrap() });

        let entries = mgr.list();
        assert_eq!(entries.len(), 1);
        // The Lua plugin called ctx.probe successfully (it would error() otherwise).
        // Verify the entry was emitted correctly.
        assert_eq!(entries[0].resource, "/lib/v1.mp4");
    }

    #[test]
    fn v3_source_context_has_grouped_interfaces_and_v2_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_path = dir.path().join("grouped_ctx.lua");
        std::fs::write(
            &plugin_path,
            r#"
local M = {}
function M.info()
    return {
        name = "grouped_ctx",
        capabilities = { source = { types = {"image"}, scan = true } },
    }
end
M.source = {}
function M.source.scan(ctx)
    assert(type(ctx.fs) == "table" and type(ctx.fs.exists) == "function")
    assert(type(ctx.config) == "table" and type(ctx.config.get) == "function")
    assert(type(ctx.json) == "table" and type(ctx.json.parse) == "function")
    assert(type(ctx.file_exists) == "function")
    assert(type(ctx.plugin_config) == "function")
    assert(type(ctx.json_parse) == "function")
    return {}
end
return M
"#,
        )
        .unwrap();

        let manager = SourceManager::new().unwrap();
        manager
            .load_plugin(&plugin_path, "org.grouped", "1", ENTRY_VERSION_V3)
            .unwrap();
        block_value(async { manager.scan_all(&HashMap::new()).await }).unwrap();
    }

    #[test]
    fn scan_all_reports_a_failing_plugin_instead_of_reporting_success() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_path = dir.path().join("failing_source.lua");
        let mut f = std::fs::File::create(&plugin_path).unwrap();
        write!(
            f,
            r#"
local M = {{}}
function M.info()
    return {{
        name = "failing",
        capabilities = {{
            source = {{ types = {{"image"}}, scan = true }},
        }},
    }}
end
M.source = {{}}
function M.source.scan(ctx)
    error("library root is unreadable")
end
return M
"#
        )
        .unwrap();

        let mgr = SourceManager::new().unwrap();
        mgr.load_plugin(&plugin_path, "failing.plugin", "1.0", ENTRY_VERSION)
            .unwrap();

        let result = block_value(async { mgr.scan_all(&HashMap::new()).await });

        let err = result.expect_err("a failing source scan must not report success");
        assert!(
            err.to_string().contains("failing"),
            "error should name the plugin that failed, got: {err}"
        );
        assert!(mgr.list().is_empty());
    }

    #[test]
    fn test_load_and_scan_plugin() {
        let dir = tempfile::tempdir().unwrap();

        // Write a minimal source plugin
        let plugin_path = dir.path().join("test_source.lua");
        let mut f = std::fs::File::create(&plugin_path).unwrap();
        write!(
            f,
            r#"
local M = {{}}
function M.info()
    return {{
        name = "test",
        capabilities = {{
            source = {{ types = {{"image"}}, scan = true }},
        }},
    }}
end
M.source = {{}}
function M.source.scan(ctx)
    return {{
        {{ id = "w1", name = "Test Wallpaper", wp_type = "image",
           resource = "/tmp/test.png", metadata = {{}} }},
    }}
end
return M
"#
        )
        .unwrap();

        let mgr = SourceManager::new().unwrap();
        let name = mgr
            .load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION)
            .unwrap();
        assert_eq!(name, "test");

        block(async { mgr.scan_all(&HashMap::new()).await.unwrap() });
        assert_eq!(mgr.list().len(), 1);
        assert_eq!(mgr.list()[0].name, "Test Wallpaper");
        assert_eq!(mgr.list()[0].wp_type, "image");
        assert_eq!(mgr.list()[0].plugin_name, "test");

        let by_type = mgr.list_by_type("image");
        assert_eq!(by_type.len(), 1);

        let by_type_empty = mgr.list_by_type("video");
        assert!(by_type_empty.is_empty());

        // Identity is the DB item.id, assigned at sync time; this
        // scan-only test leaves it at 0, so look up by "0".
        let found = mgr.get("0");
        assert!(found.is_some());

        let plugins = mgr.plugins().unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "test");
        assert_eq!(plugins[0].version, "1.0");
    }

    #[test]
    fn plugin_import_loads_plugin_local_modules() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("helpers")).unwrap();
        std::fs::write(
            dir.path().join("helpers/names.lua"),
            r#"
local M = {}
function M.name()
    return "Imported"
end
return M
"#,
        )
        .unwrap();
        let plugin_path = dir.path().join("main.lua");
        std::fs::write(
            &plugin_path,
            r#"
local names = import("helpers.names")
local M = {}
function M.info()
    return {
        name = "imported",
        capabilities = {
            source = { types = {"image"}, scan = true },
            discover = { search = true, details = true, download = true },
        },
    }
end
M.source = {}
function M.source.scan(ctx)
    return {
        { name = names.name(), wp_type = "image", resource = "/tmp/imported.png" },
    }
end
M.discover = {}
function M.discover.search(ctx, params)
    return {
        items = {
            { id = "abc", title = names.name(), preview_url = "", author = "" },
        },
        has_more = false,
    }
end
function M.discover.details(ctx, id)
    return {
        author = "Imported Author",
        description = names.name(),
        size = "42",
        width = 10,
        height = 20,
        tags = {"tag"},
        web_url = "https://example.invalid/item/" .. id,
    }
end
function M.discover.download(ctx, id)
    return {
        wp_type = "image",
        url = "https://example.invalid/" .. id,
        filename = id .. ".jpg",
        title = names.name(),
        tags = {"tag"},
        external_id = id,
        size = 42,
        width = 10,
        height = 20,
        content_rating = "Everyone",
    }
end
return M
"#,
        )
        .unwrap();

        let mgr = SourceManager::new().unwrap();
        let name = mgr
            .load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION)
            .unwrap();
        assert_eq!(name, "imported");
        block(async { mgr.scan_all(&HashMap::new()).await.unwrap() });
        assert_eq!(mgr.list()[0].name, "Imported");

        let search =
            block_value(async { mgr.call_discover("imported", "", "", 1, &[]).await.unwrap() });
        assert_eq!(search.items[0].wp_type, "image");

        let dl = block_value(async { mgr.call_download("imported", "abc").await.unwrap() });
        assert_eq!(dl.wp_type, "image");
        assert_eq!(dl.filename, "abc.jpg");
        assert_eq!(dl.title, "Imported");
        assert_eq!(dl.tags, vec!["tag"]);
        assert_eq!(dl.external_id, "abc");
        assert_eq!(dl.size, Some(42));
        let detail = block_value(async { mgr.call_details("imported", "abc").await.unwrap() });
        assert_eq!(detail.width, Some(10));
        assert_eq!(detail.height, Some(20));
        assert_eq!(detail.web_url, "https://example.invalid/item/abc");
        assert_eq!(detail.author, "Imported Author");
    }

    #[test]
    fn source_item_remove_works_without_scan_capability() {
        let dir = tempfile::tempdir().unwrap();
        let item_path = dir.path().join("wallpaper.png");
        std::fs::write(&item_path, b"image").unwrap();
        let plugin_path = dir.path().join("remove.lua");
        std::fs::write(
            &plugin_path,
            r#"
local M = {}
function M.info()
    return {
        name = "remove_only",
        capabilities = {},
    }
end
M.source = {}
function M.source.remove(ctx, item)
    if item.path ~= item.resource then error("path/resource mismatch") end
    if item.relative_path ~= "wallpaper.png" then error("wrong relative path") end
    if item.external_id ~= "ext-1" then error("missing external id") end
    ctx.remove_file(item.path)
end
return M
"#,
        )
        .unwrap();

        let mgr = SourceManager::new().unwrap();
        let name = mgr
            .load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION)
            .unwrap();
        assert_eq!(name, "remove_only");
        assert!(mgr.supports_item_remove("remove_only"));

        let entry = WallpaperEntry {
            item_id: 42,
            name: "Wallpaper".to_string(),
            wp_type: "image".to_string(),
            resource: item_path.to_string_lossy().to_string(),
            preview: None,
            plugin_name: "remove_only".to_string(),
            library_root: dir.path().to_string_lossy().to_string(),
            description: None,
            tags: vec!["tag".to_string()],
            external_id: Some("ext-1".to_string()),
            size: None,
            width: None,
            height: None,
            content_rating: None,
            modified_at: None,
            create_at: 0,
        };
        let libraries = vec![entry.library_root.clone()];
        block_value(async { mgr.remove_item("remove_only", &entry, &libraries).await }).unwrap();
        assert!(!item_path.exists());
    }

    #[test]
    fn source_item_remove_rejects_plugins_without_remove() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_path = dir.path().join("no_remove.lua");
        std::fs::write(
            &plugin_path,
            r#"
local M = {}
function M.info()
    return {
        name = "no_remove",
        capabilities = {},
    }
end
return M
"#,
        )
        .unwrap();

        let mgr = SourceManager::new().unwrap();
        let name = mgr
            .load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION)
            .unwrap();
        assert_eq!(name, "no_remove");
        assert!(!mgr.supports_item_remove("no_remove"));

        let entry = WallpaperEntry {
            item_id: 7,
            name: "Wallpaper".to_string(),
            wp_type: "image".to_string(),
            resource: "/tmp/wallpaper.png".to_string(),
            preview: None,
            plugin_name: "no_remove".to_string(),
            library_root: "/tmp".to_string(),
            description: None,
            tags: Vec::new(),
            external_id: None,
            size: None,
            width: None,
            height: None,
            content_rating: None,
            modified_at: None,
            create_at: 0,
        };
        let err =
            block_value(async { mgr.remove_item("no_remove", &entry, &[]).await }).unwrap_err();
        assert!(matches!(
            err,
            Error::SourceItemRemoveUnsupported(plugin) if plugin == "no_remove"
        ));
    }

    #[test]
    fn plugin_import_rejects_path_escape() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_path = dir.path().join("main.lua");
        std::fs::write(
            &plugin_path,
            r#"
local bad = import("../outside")
return bad
"#,
        )
        .unwrap();

        let mgr = SourceManager::new().unwrap();
        assert!(mgr
            .load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION)
            .is_err());
    }

    #[test]
    fn entry_versions_v2_and_v3_are_supported_and_newer_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_path = dir.path().join("main.lua");
        std::fs::write(
            &plugin_path,
            r#"
local M = {}
function M.info()
    return {
        name = "too_new",
        capabilities = {
            discover = { search = true },
        },
    }
end
M.discover = {}
function M.discover.search(ctx, params)
    return { items = {}, has_more = false }
end
return M
"#,
        )
        .unwrap();

        let mgr = SourceManager::new().unwrap();
        assert!(mgr
            .load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION_V2)
            .is_ok());
        let mgr = SourceManager::new().unwrap();
        assert!(mgr
            .load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION_V3)
            .is_ok());
        let mgr = SourceManager::new().unwrap();
        assert!(mgr
            .load_plugin(&plugin_path, "test.plugin", "1.0", LATEST_ENTRY_VERSION + 1,)
            .is_err());
    }

    #[test]
    fn wallhaven_plugin_supports_optional_api_key_login() {
        let plugin_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins/org.waywallen.wallhaven/main.lua");

        let mgr = SourceManager::new().unwrap();
        let name = mgr
            .load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION_V3)
            .unwrap();
        assert_eq!(name, "wallhaven");
        assert!(mgr.plugins().unwrap().is_empty());

        let sources = mgr.discover_sources().unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].plugin_id, "wallhaven");
        assert!(sources[0].supports_search);
        assert!(sources[0].filters.iter().any(|filter| {
            filter.ty == DiscoverFilterType::MultiSelect
                && filter.values.iter().any(|value| value == "Anime")
        }));
        assert_eq!(sources[0].actions[0].kind, SourceActionKind::Form);
        assert_eq!(sources[0].actions[0].fields.len(), 1);
        assert_eq!(sources[0].actions[0].fields[0].key, "api_key");
        assert!(sources[0].actions[0].fields[0].secret);
        assert!(sources[0].actions[0].fields[0].required);
        assert_eq!(
            block_value(async { mgr.check_lifecycle("wallhaven").await })
                .unwrap()
                .unwrap()
                .state,
            PluginLifecycleState::SignedOut
        );

        let runtime = mgr.test_runtime("wallhaven");
        let runtime = runtime.blocking_lock();
        let env = runtime
            .plugin_lua_env(plugin_path.parent().unwrap())
            .unwrap();
        let import: LuaFunction = env.get("import").unwrap();
        let map: LuaTable = import.call("wallhaven.map").unwrap();
        let search_item: LuaFunction = map.get("search_item").unwrap();
        let item = runtime.lua.create_table().unwrap();
        item.set("id", "abc").unwrap();
        let mapped: LuaTable = search_item.call(item).unwrap();
        assert_eq!(mapped.get::<String>("wp_type").unwrap(), "image");

        let details: LuaFunction = map.get("details").unwrap();
        let detail = runtime.lua.create_table().unwrap();
        detail.set("url", "https://wallhaven.cc/w/abc123").unwrap();
        let mapped: LuaTable = details.call(detail.clone()).unwrap();
        assert_eq!(
            mapped.get::<String>("web_url").unwrap(),
            "https://wallhaven.cc/w/abc123"
        );
        // A wallpaper Wallhaven reports no uploader for keeps the empty author
        // the listing has always produced.
        assert_eq!(mapped.get::<String>("author").unwrap(), "");

        let uploader = runtime.lua.create_table().unwrap();
        uploader.set("username", "Qtn").unwrap();
        detail.set("uploader", uploader).unwrap();
        let mapped: LuaTable = details.call(detail).unwrap();
        assert_eq!(mapped.get::<String>("author").unwrap(), "Qtn");
    }

    #[test]
    fn call_resolve_relays_directory_item() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resolver.lua");
        std::fs::write(
            &path,
            r#"
local M = {}
function M.info()
    return { name = "resolver", capabilities = { discover = { search = true, resolve = true } } }
end
M.discover = {}
function M.discover.search(ctx, params) return { items = {}, has_more = false } end
function M.discover.resolve(ctx, params)
    return {
        name = "R " .. params.id,
        wp_type = "scene",
        resource = "scene.pkg",
        preview = "preview.jpg",
        description = "d",
        tags = { "t" },
        external_id = params.id,
        size = 7,
    }
end
return M
"#,
        )
        .unwrap();

        let mgr = SourceManager::new().unwrap();
        mgr.load_plugin(&path, "test.plugin", "1.0", ENTRY_VERSION)
            .unwrap();
        let got =
            block_value(async { mgr.call_resolve("resolver", "id1", "/some/dir").await }).unwrap();
        assert_eq!(got.name, "R id1");
        assert_eq!(got.wp_type, "scene");
        assert_eq!(got.resource, "scene.pkg");
        assert_eq!(got.preview.as_deref(), Some("preview.jpg"));
        assert_eq!(got.external_id, "id1");
        assert_eq!(got.size, Some(7));
    }

    #[test]
    fn refresh_dynamic_tags_replaces_declared_tags() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dyn.lua");
        std::fs::write(
            &path,
            r#"
local M = {}
function M.info()
    return {
        name = "dyn",
        capabilities = { discover = { search = true, tags = { "fallback" } } },
    }
end
M.discover = {}
function M.discover.search(ctx, params) return { items = {}, has_more = false } end
function M.discover.tags(ctx) return { "Live1", "Live2" } end
return M
"#,
        )
        .unwrap();

        let mgr = SourceManager::new().unwrap();
        mgr.load_plugin(&path, "test.plugin", "1.0", ENTRY_VERSION)
            .unwrap();
        // Before the refresh, discovery advertises the static fallback.
        assert_eq!(
            mgr.discover_sources().unwrap()[0].filters[0].values,
            vec!["fallback"]
        );

        block_value(async { mgr.refresh_dynamic_tags().await });
        assert_eq!(
            mgr.discover_sources().unwrap()[0].filters[0].values,
            vec!["Live1", "Live2"]
        );
    }

    #[test]
    fn discover_filters_validate_selected_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("filters.lua");
        std::fs::write(
            &path,
            r#"
local M = {}
function M.info()
    return {
        name = "filters",
        capabilities = {
            discover = {
                search = true,
                filters = {
                    {
                        id = "kind",
                        title = "Kind",
                        type = "select",
                        values = { "A", "B" },
                        value_labels = { "Alpha", "Beta" },
                    },
                    { id = "tags", title = "Tags", type = "multi_select", values = { "X", "Y" } },
                },
            },
        },
    }
end
M.discover = {}
function M.discover.search(ctx, params) return { items = {}, has_more = false } end
return M
"#,
        )
        .unwrap();

        let mgr = SourceManager::new().unwrap();
        mgr.load_plugin(&path, "test.plugin", "1.0", ENTRY_VERSION_V3)
            .unwrap();
        assert_eq!(
            mgr.discover_sources().unwrap()[0].filters[0].value_labels,
            vec!["Alpha", "Beta"]
        );
        assert!(block_value(async {
            mgr.call_discover("filters", "", "", 1, &["A".to_string(), "X".to_string()])
                .await
        })
        .is_ok());
        assert!(block_value(async {
            mgr.call_discover("filters", "", "", 1, &["A".to_string(), "B".to_string()])
                .await
        })
        .is_err());
        assert!(block_value(async {
            mgr.call_discover("filters", "", "", 1, &["unknown".to_string()])
                .await
        })
        .is_err());
    }

    #[test]
    fn remote_discovery_context_omits_filesystem_mutation() {
        let plugin = tempfile::tempdir().unwrap();
        let path = plugin.path().join("guarded.lua");
        std::fs::write(
            &path,
            r#"
local M = {}
function M.info()
    return { name = "guarded", capabilities = { discover = { search = true } } }
end
M.discover = {}
function M.discover.search(ctx, params)
    local parsed = ctx.json.parse('{"ok":true}')
    ctx.http:set_cookie(
        "https://example.com/",
        "fixture=cookie; Domain=example.com; Path=/; Secure"
    )
    local slept = pcall(function() ctx.time.sleep(0) end)
    return {
        items = { {
            id = tostring(
                ctx.remove_dir == nil and ctx.libraries == nil and ctx.fs == nil
                and ctx.config ~= nil and ctx.json ~= nil
                and ctx.base64 ~= nil and ctx.time ~= nil and ctx.random ~= nil
                and parsed.ok and ctx.json_parse('{"ok":true}').ok
                and ctx.json.encode({1, 2}) == "[1,2]"
                and ctx.json_encode({1, 2}) == "[1,2]"
                and ctx.base64.decode("d2FsbA==") == "wall"
                and ctx.base64_decode("d2FsbA==") == "wall"
                and ctx.time.unix() > 1000000000
                and ctx.time_unix() > 1000000000
                and #ctx.random.hex(12) == 24
                and slept
                and ctx.url.decode_component("wall%7Cpaper") == "wall|paper"
                and ctx.http:cookie("https://example.com/", "fixture") == "cookie"
            ),
            title = "",
            preview_url = "",
            author = "",
        } },
        has_more = false,
    }
end
return M
"#,
        )
        .unwrap();
        let mgr = SourceManager::new().unwrap();
        mgr.load_plugin(&path, "test.plugin", "1.0", ENTRY_VERSION)
            .unwrap();
        let r = block_value(async { mgr.call_discover("guarded", "", "", 1, &[]).await }).unwrap();
        assert_eq!(r.items[0].id, "true");
    }

    #[test]
    fn v3_lifecycle_actions_qrlogin_and_subscription_share_plugin_owned_state() {
        let dir = tempfile::tempdir().unwrap();
        let state_root = dir.path().join("state");
        std::fs::write(dir.path().join("legacy-session.json"), "legacy secret").unwrap();
        let state_store = crate::plugin::state_store::PluginStateStore::new(
            state_root.clone(),
            dir.path().to_path_buf(),
        );
        let manager =
            SourceManager::with_probe_and_state_store(Arc::new(AvFormatProbe::new()), state_store)
                .unwrap();
        let plugin_path = dir.path().join("main.lua");
        std::fs::write(
            &plugin_path,
            r#"
local M = {}
local signed_in = false
local display = ""
local subscriptions = {}

function M.info()
    return {
        name = "account_provider",
        capabilities = {
            discover = { search = true, subscription = true },
        },
        actions = {
            {
                id = "sign_in",
                kind = "qr_login",
                label = "Log in",
                description = "Open the account app",
                browse_description = "Log in to browse this source",
                browse_button_label = "Continue",
            },
            { id = "sign_out", kind = "invoke" },
            {
                id = "set_alias",
                kind = "form",
                fields = {
                    { key = "alias", label = "Alias", required = true },
                },
            },
        },
        status = { { id = "account" } },
        state_migrations = {
            { schema_id = "legacy-session-v1", file = "legacy-session.json" },
        },
    }
end

M.lifecycle = {}
function M.lifecycle.load(blob)
    if blob == nil then return end
    local flag, value = string.match(blob, "^([^|]+)|(.*)$")
    signed_in = flag == "1"
    display = value or ""
end
function M.lifecycle.save()
    return (signed_in and "1" or "0") .. "|" .. display
end
function M.lifecycle.check(ctx)
    return {
        state = signed_in and "signed_in" or "signed_out",
        display_value = display,
        avatar_url = "https://example.invalid/avatar.png",
    }
end
function M.lifecycle.migrate(schema_id, raw)
    if schema_id ~= "legacy-session-v1" or raw ~= "legacy secret" then
        error("wrong migration input")
    end
    return "0|migrated"
end

M.actions = {}
function M.actions.status(ctx)
    return {
        status = { account = display },
        actions = {
            sign_in = { visible = not signed_in, enabled = not signed_in },
            sign_out = { visible = signed_in, enabled = signed_in },
        },
    }
end
function M.actions.invoke(ctx, action_id, values)
    if action_id == "sign_out" then
        signed_in = false
        display = ""
    elseif action_id == "set_alias" then
        display = values.alias
    else
        error("unexpected action")
    end
end

M.qrlogin = {}
function M.qrlogin.begin(ctx, action_id)
    return {
        key = { polls = 0 },
        challenge = "https://example.invalid/challenge",
        poll_after_ms = 25,
        expires_in_ms = 1000,
        title = "Sign in",
        instruction = "Scan",
    }
end
function M.qrlogin.poll(ctx, key)
    key.polls = key.polls + 1
    if key.polls == 1 then
        return { state = "awaiting_confirmation", display_value = "phone" }
    end
    signed_in = true
    display = "alice"
    return { state = "succeeded", display_value = display }
end
function M.qrlogin.cancel(ctx, key)
    key.cancelled = true
end

M.discover = {}
function M.discover.search(ctx, params)
    return { items = {}, has_more = false }
end

M.subscription = {}
function M.subscription.status(ctx, ids)
    local result = {}
    for _, id in ipairs(ids) do result[id] = subscriptions[id] or "unknown" end
    return result
end
function M.subscription.subscribe(ctx, id)
    if id == "rejected" then return { accepted = false, error = "denied" } end
    subscriptions[id] = "subscribed"
    return { accepted = true }
end
function M.subscription.unsubscribe(ctx, id)
    subscriptions[id] = "unsubscribed"
    return { accepted = true }
end

return M
"#,
        )
        .unwrap();

        manager
            .load_plugin(&plugin_path, "org.test", "1.0", ENTRY_VERSION_V3)
            .unwrap();
        assert!(dir.path().join("legacy-session.migrated.bak").is_file());
        assert_eq!(
            std::fs::read_to_string(state_root.join("org.test.state")).unwrap(),
            "0|migrated"
        );
        assert_eq!(
            manager.discover_sources().unwrap()[0].remote_capability,
            Some(RemoteCapability::Subscription)
        );

        let sources = block_value(async { manager.discover_sources_with_status().await }).unwrap();
        assert_eq!(sources[0].status[0].value, "migrated");
        assert_eq!(sources[0].avatar_url, "https://example.invalid/avatar.png");
        assert_eq!(sources[0].actions[0].label, "Log in");
        assert_eq!(sources[0].actions[0].description, "Open the account app");
        assert_eq!(sources[0].actions[0].browse_button_label, "Continue");
        assert_eq!(
            sources[0].actions[0].browse_description,
            "Log in to browse this source"
        );
        assert!(sources[0].actions[0].visible);
        assert!(!sources[0].actions[1].visible);
        assert_eq!(sources[0].actions[2].fields[0].key, "alias");

        let begin =
            block_value(async { manager.begin_qr_login("account_provider", "sign_in").await })
                .unwrap();
        assert_eq!(begin.challenge, "https://example.invalid/challenge");
        assert_eq!(begin.poll_after_ms, 25);
        let first = block_value(async {
            manager
                .poll_qr_login("account_provider", begin.operation_id)
                .await
        })
        .unwrap();
        assert_eq!(first.state, QrLoginPollState::AwaitingConfirmation);
        let second = block_value(async {
            manager
                .poll_qr_login("account_provider", begin.operation_id)
                .await
        })
        .unwrap();
        assert_eq!(second.state, QrLoginPollState::Succeeded);
        assert_eq!(
            block_value(async { manager.check_lifecycle("account_provider").await })
                .unwrap()
                .unwrap()
                .state,
            PluginLifecycleState::SignedIn
        );
        assert_eq!(
            std::fs::read_to_string(state_root.join("org.test.state")).unwrap(),
            "1|alice"
        );

        let ids = vec!["item".to_string(), "missing".to_string()];
        let before =
            block_value(async { manager.subscription_status("account_provider", &ids).await })
                .unwrap();
        assert!(before
            .iter()
            .all(|item| item.state == SubscriptionState::Unknown));
        block_value(async {
            manager
                .set_subscription("account_provider", "item", true)
                .await
        })
        .unwrap();
        let subscribed =
            block_value(async { manager.subscription_status("account_provider", &ids).await })
                .unwrap();
        assert_eq!(subscribed[0].state, SubscriptionState::Subscribed);
        assert_eq!(subscribed[1].state, SubscriptionState::Unknown);
        let rejected = block_value(async {
            manager
                .set_subscription("account_provider", "rejected", true)
                .await
        })
        .unwrap_err();
        assert!(rejected.to_string().contains("denied"));
        block_value(async {
            manager
                .set_subscription("account_provider", "item", false)
                .await
        })
        .unwrap();
        let unsubscribed =
            block_value(async { manager.subscription_status("account_provider", &ids).await })
                .unwrap();
        assert_eq!(unsubscribed[0].state, SubscriptionState::Unsubscribed);

        let missing = block_value(async {
            manager
                .invoke_action("account_provider", "set_alias", &HashMap::new())
                .await
        });
        assert!(missing
            .unwrap_err()
            .to_string()
            .contains("requires field 'alias'"));
        let values = HashMap::from([("alias".to_string(), "configured".to_string())]);
        block_value(async {
            manager
                .invoke_action("account_provider", "set_alias", &values)
                .await
        })
        .unwrap();
        assert_eq!(
            block_value(async { manager.check_lifecycle("account_provider").await })
                .unwrap()
                .unwrap()
                .display_value,
            "configured"
        );

        block_value(async {
            manager
                .invoke_action("account_provider", "sign_out", &HashMap::new())
                .await
        })
        .unwrap();
        assert_eq!(
            block_value(async { manager.check_lifecycle("account_provider").await })
                .unwrap()
                .unwrap()
                .state,
            PluginLifecycleState::SignedOut
        );
    }

    #[test]
    fn v3_remote_capabilities_are_mutually_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.lua");
        let script = |flags: &str, api: &str| {
            format!(
                r#"
local M = {{}}
function M.info()
    return {{ name = "remote", capabilities = {{ discover = {{ search = true, {flags} }} }} }}
end
M.discover = {{}}
function M.discover.search(ctx, params) return {{ items = {{}}, has_more = false }} end
{api}
return M
"#
            )
        };

        std::fs::write(&path, script("", "")).unwrap();
        assert!(SourceManager::new()
            .unwrap()
            .load_plugin(&path, "org.test", "1", ENTRY_VERSION_V3)
            .is_ok());

        let orphan_details = "function M.discover.details(ctx, id) return {} end";
        std::fs::write(&path, script("", orphan_details)).unwrap();
        assert!(SourceManager::new()
            .unwrap()
            .load_plugin(&path, "org.test", "1", ENTRY_VERSION_V3)
            .is_err());
        assert!(SourceManager::new()
            .unwrap()
            .load_plugin(&path, "org.test", "1", ENTRY_VERSION_V2)
            .is_ok());

        let orphan_actions = "M.actions = {}\nfunction M.actions.status(ctx) return {} end";
        std::fs::write(&path, script("", orphan_actions)).unwrap();
        assert!(SourceManager::new()
            .unwrap()
            .load_plugin(&path, "org.test", "1", ENTRY_VERSION_V3)
            .is_err());

        std::fs::write(
            &path,
            script(
                "download = true",
                "function M.discover.download(ctx, id) return { wp_type = \"image\" } end",
            ),
        )
        .unwrap();
        let download_manager = SourceManager::new().unwrap();
        download_manager
            .load_plugin(&path, "org.test", "1", ENTRY_VERSION_V3)
            .unwrap();
        assert!(block_value(async {
            download_manager
                .subscription_status("remote", &["item".to_string()])
                .await
        })
        .is_err());

        let subscription_api = r#"
M.subscription = {}
function M.subscription.status(ctx, ids) return {} end
function M.subscription.subscribe(ctx, id) return {} end
        function M.subscription.unsubscribe(ctx, id) return {} end
"#;
        std::fs::write(&path, script("subscription = true", subscription_api)).unwrap();
        let subscription_manager = SourceManager::new().unwrap();
        subscription_manager
            .load_plugin(&path, "org.test", "1", ENTRY_VERSION_V3)
            .unwrap();
        assert!(
            block_value(async { subscription_manager.call_download("remote", "item").await })
                .is_err()
        );

        std::fs::write(
            &path,
            script(
                "subscription = true",
                &format!(
                    "function M.discover.download(ctx, id) return {{}} end\n{subscription_api}"
                ),
            ),
        )
        .unwrap();
        assert!(SourceManager::new()
            .unwrap()
            .load_plugin(&path, "org.test", "1", ENTRY_VERSION_V3)
            .is_err());

        std::fs::write(
            &path,
            script(
                "download = true",
                &format!(
                    "function M.discover.download(ctx, id) return {{}} end\n{subscription_api}"
                ),
            ),
        )
        .unwrap();
        assert!(SourceManager::new()
            .unwrap()
            .load_plugin(&path, "org.test", "1", ENTRY_VERSION_V3)
            .is_err());

        std::fs::write(
            &path,
            script(
                "download = true, subscription = true",
                &format!(
                    "function M.discover.download(ctx, id) return {{ wp_type = \"image\" }} end\n{subscription_api}"
                ),
            ),
        )
        .unwrap();
        assert!(SourceManager::new()
            .unwrap()
            .load_plugin(&path, "org.test", "1", ENTRY_VERSION_V3)
            .is_err());
    }

    #[test]
    fn failed_state_migration_preserves_the_legacy_blob() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("legacy.json");
        std::fs::write(&legacy, "legacy secret").unwrap();
        let state_root = dir.path().join("state");
        let state_store = crate::plugin::state_store::PluginStateStore::new(
            state_root.clone(),
            dir.path().to_path_buf(),
        );
        let manager =
            SourceManager::with_probe_and_state_store(Arc::new(AvFormatProbe::new()), state_store)
                .unwrap();
        let entry = dir.path().join("main.lua");
        std::fs::write(
            &entry,
            r#"
local M = {}
function M.info()
    return {
        name = "migration_failure",
        capabilities = {},
        state_migrations = { { schema_id = "legacy", file = "legacy.json" } },
    }
end
M.lifecycle = {}
function M.lifecycle.load(blob) end
function M.lifecycle.save() return "new" end
function M.lifecycle.check(ctx) return { state = "signed_out" } end
function M.lifecycle.migrate(schema_id, raw) error("migration rejected") end
return M
"#,
        )
        .unwrap();

        assert!(manager
            .load_plugin(&entry, "org.failure", "1", ENTRY_VERSION_V3)
            .is_err());
        assert_eq!(std::fs::read_to_string(&legacy).unwrap(), "legacy secret");
        assert!(!dir.path().join("legacy.migrated.bak").exists());
        assert!(!state_root.join("org.failure.state").exists());
    }

    #[test]
    fn invalid_qr_begin_result_does_not_retain_the_opaque_key() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("main.lua");
        std::fs::write(
            &entry,
            r#"
local M = {}
function M.info()
    return {
        name = "invalid_qr",
        capabilities = {},
        actions = { { id = "sign_in", kind = "qr_login" } },
    }
end
M.actions = {}
function M.actions.status(ctx) return { actions = {} } end
M.qrlogin = {}
function M.qrlogin.begin(ctx, action_id) return { key = {} } end
function M.qrlogin.poll(ctx, key) return { state = "awaiting_scan" } end
return M
"#,
        )
        .unwrap();
        let manager = SourceManager::new().unwrap();
        manager
            .load_plugin(&entry, "org.invalid", "1", ENTRY_VERSION_V3)
            .unwrap();
        assert!(
            block_value(async { manager.begin_qr_login("invalid_qr", "sign_in").await }).is_err()
        );
        assert!(manager
            .test_runtime("invalid_qr")
            .blocking_lock()
            .qr_operations
            .is_empty());
    }

    #[test]
    fn subscription_mutation_does_not_scan_source_libraries() {
        let dir = tempfile::tempdir().unwrap();
        let state_root = dir.path().join("state");
        let state_store = crate::plugin::state_store::PluginStateStore::new(
            state_root.clone(),
            dir.path().to_path_buf(),
        );
        let manager =
            SourceManager::with_probe_and_state_store(Arc::new(AvFormatProbe::new()), state_store)
                .unwrap();
        let entry = dir.path().join("main.lua");
        std::fs::write(
            &entry,
            r#"
local M = {}
local scans = 0
function M.info()
    return {
        name = "separate_flows",
        capabilities = {
            source = { scan = true, types = { "image" } },
            discover = { search = true, subscription = true },
        },
    }
end
M.lifecycle = {}
function M.lifecycle.load(blob) end
function M.lifecycle.save() return tostring(scans) end
function M.lifecycle.check(ctx)
    return { state = "signed_in", display_value = "test" }
end
M.source = {}
function M.source.scan(ctx) scans = scans + 1 return {} end
M.discover = {}
function M.discover.search(ctx, params) return { items = {}, has_more = false } end
M.subscription = {}
function M.subscription.status(ctx, ids)
    local result = {}
    for _, id in ipairs(ids) do result[id] = "unknown" end
    return result
end
function M.subscription.subscribe(ctx, id) return { accepted = true } end
function M.subscription.unsubscribe(ctx, id) return { accepted = true } end
return M
"#,
        )
        .unwrap();
        manager
            .load_plugin(&entry, "org.test", "1", ENTRY_VERSION_V3)
            .unwrap();

        let subscription_entry = WallpaperEntry {
            item_id: 1,
            name: "Workshop item".to_string(),
            wp_type: "image".to_string(),
            resource: "item.jpg".to_string(),
            preview: None,
            description: None,
            tags: Vec::new(),
            external_id: Some("item".to_string()),
            size: None,
            width: None,
            height: None,
            content_rating: None,
            modified_at: None,
            create_at: 0,
            plugin_name: "separate_flows".to_string(),
            library_root: String::new(),
        };
        assert!(manager.supports_item_unsubscribe(&subscription_entry));
        assert!(!manager.supports_item_unsubscribe(&WallpaperEntry {
            external_id: None,
            ..subscription_entry
        }));

        block_value(async {
            manager
                .set_subscription("separate_flows", "item", true)
                .await
        })
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(state_root.join("org.test.state")).unwrap(),
            "0"
        );
    }

    #[test]
    fn plugin_http_sessions_are_isolated_persisted_and_cleared_by_the_owner() {
        let dir = tempfile::tempdir().unwrap();
        let state_root = dir.path().join("state");
        let state_store = crate::plugin::state_store::PluginStateStore::new(
            state_root.clone(),
            dir.path().to_path_buf(),
        );
        let script = |name: &str, cookie: &str| {
            format!(
                r#"
local M = {{}}
function M.info()
    return {{
        name = "{name}",
        capabilities = {{ discover = {{ search = true }} }},
        actions = {{ {{ id = "sign_out", kind = "invoke" }} }},
    }}
end
M.discover = {{}}
function M.discover.search(ctx, params)
    if params.query == "set" then
        ctx.http:set_cookie(
            "https://example.com/",
            "session={cookie}; Domain=example.com; Path=/; Secure"
        )
    elseif params.query == "fail" then
        ctx.http:set_cookie(
            "https://example.com/",
            "session=failed; Domain=example.com; Path=/; Secure"
        )
        error("callback failed")
    end
    return {{
        items = {{ {{
            id = ctx.http:cookie("https://example.com/", "session") or "none",
            title = "",
            preview_url = "",
            author = "",
        }} }},
        has_more = false,
    }}
end
M.actions = {{}}
function M.actions.status(ctx)
    return {{ actions = {{ sign_out = {{ visible = true, enabled = true }} }} }}
end
function M.actions.invoke(ctx, action_id)
    if action_id ~= "sign_out" then error("unexpected action") end
    ctx.http:clear_cookies()
end
return M
"#
            )
        };
        let first_path = dir.path().join("first.lua");
        let second_path = dir.path().join("second.lua");
        std::fs::write(&first_path, script("first", "first")).unwrap();
        std::fs::write(&second_path, script("second", "second")).unwrap();

        let manager = SourceManager::with_probe_and_state_store(
            Arc::new(AvFormatProbe::new()),
            state_store.clone(),
        )
        .unwrap();
        manager
            .load_plugin(&first_path, "org.first", "1", ENTRY_VERSION_V3)
            .unwrap();
        manager
            .load_plugin(&second_path, "org.second", "1", ENTRY_VERSION_V3)
            .unwrap();
        let first =
            block_value(async { manager.call_discover("first", "set", "", 1, &[]).await }).unwrap();
        assert_eq!(first.items[0].id, "first");
        let second =
            block_value(async { manager.call_discover("second", "", "", 1, &[]).await }).unwrap();
        assert_eq!(second.items[0].id, "none");
        assert!(state_root.join("org.first.cookies").is_file());
        assert!(!state_root.join("org.first.state").exists());

        let failed =
            block_value(async { manager.call_discover("first", "fail", "", 1, &[]).await });
        assert!(failed.is_err());

        let restored = SourceManager::with_probe_and_state_store(
            Arc::new(AvFormatProbe::new()),
            state_store.clone(),
        )
        .unwrap();
        restored
            .load_plugin(&first_path, "org.first", "1", ENTRY_VERSION_V3)
            .unwrap();
        let after_restart =
            block_value(async { restored.call_discover("first", "", "", 1, &[]).await }).unwrap();
        assert_eq!(after_restart.items[0].id, "failed");

        block_value(async {
            restored
                .invoke_action("first", "sign_out", &HashMap::new())
                .await
        })
        .unwrap();
        let signed_out =
            SourceManager::with_probe_and_state_store(Arc::new(AvFormatProbe::new()), state_store)
                .unwrap();
        signed_out
            .load_plugin(&first_path, "org.first", "1", ENTRY_VERSION_V3)
            .unwrap();
        let after_sign_out =
            block_value(async { signed_out.call_discover("first", "", "", 1, &[]).await }).unwrap();
        assert_eq!(after_sign_out.items[0].id, "none");
    }

    #[tokio::test]
    async fn plugins_have_independent_runtime_locks() {
        let dir = tempfile::tempdir().unwrap();
        let script = |name: &str| {
            format!(
                r#"
local M = {{}}
function M.info()
    return {{
        name = "{name}",
        capabilities = {{ discover = {{ search = true }} }},
    }}
end
M.discover = {{}}
function M.discover.search(ctx, params)
    return {{ items = {{}}, has_more = false }}
end
return M
"#,
            )
        };
        let first = dir.path().join("first.lua");
        let second = dir.path().join("second.lua");
        std::fs::write(&first, script("first")).unwrap();
        std::fs::write(&second, script("second")).unwrap();
        let manager = SourceManager::new().unwrap();
        manager
            .load_plugin(&first, "org.first", "1", ENTRY_VERSION_V3)
            .unwrap();
        manager
            .load_plugin(&second, "org.second", "1", ENTRY_VERSION_V3)
            .unwrap();

        let first_handle = manager.handle("first").unwrap();
        let first_guard = first_handle.runtime.lock().await;
        assert!(tokio::time::timeout(
            Duration::from_millis(50),
            manager.call_discover("second", "", "", 1, &[]),
        )
        .await
        .is_ok());
        assert!(tokio::time::timeout(
            Duration::from_millis(20),
            manager.call_discover("first", "", "", 1, &[]),
        )
        .await
        .is_err());
        drop(first_guard);
    }

    #[test]
    fn plugin_registry_replacement_keeps_current_runtime_until_candidate_is_valid() {
        let dir = tempfile::tempdir().unwrap();
        let script = |name: &str| {
            format!(
                r#"
local M = {{}}
function M.info()
    return {{ name = "{name}", capabilities = {{ discover = {{ search = true }} }} }}
end
M.discover = {{}}
function M.discover.search(ctx, params) return {{ items = {{}}, has_more = false }} end
return M
"#
            )
        };
        let old_path = dir.path().join("old.lua");
        let new_path = dir.path().join("new.lua");
        let invalid_path = dir.path().join("invalid.lua");
        std::fs::write(&old_path, script("old")).unwrap();
        std::fs::write(&new_path, script("new")).unwrap();
        std::fs::write(&invalid_path, "error('invalid replacement')").unwrap();

        let current = SourceManager::new().unwrap();
        current
            .load_plugin(&old_path, "org.old", "1", ENTRY_VERSION_V3)
            .unwrap();
        block_value(current.suspend_plugins());
        let invalid = SourceManager::new().unwrap();
        assert!(invalid
            .load_plugin(&invalid_path, "org.new", "1", ENTRY_VERSION_V3)
            .is_err());
        assert_eq!(current.discover_sources().unwrap()[0].name, "old");
        assert!(block_value(async { current.call_discover("old", "", "", 1, &[]).await }).is_err());
        current.resume_plugins();
        assert!(block_value(async { current.call_discover("old", "", "", 1, &[]).await }).is_ok());

        let replacement = SourceManager::new().unwrap();
        replacement
            .load_plugin(&new_path, "org.new", "1", ENTRY_VERSION_V3)
            .unwrap();
        replacement
            .retain_plugins_from(&current, &HashSet::from(["org.old".to_string()]))
            .unwrap();
        block_value(current.suspend_plugins());
        current.replace_plugins(replacement).unwrap();
        assert_eq!(
            current
                .discover_sources()
                .unwrap()
                .into_iter()
                .map(|source| source.name)
                .collect::<Vec<_>>(),
            vec!["new", "old"]
        );
    }

    #[tokio::test]
    async fn lua_callbacks_have_a_host_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("deadline.lua");
        std::fs::write(
            &entry,
            r#"
local M = {}
function M.info()
    return { name = "deadline", capabilities = { discover = { search = true } } }
end
M.discover = {}
function M.discover.search(ctx, params)
    while true do end
end
return M
"#,
        )
        .unwrap();

        let manager = SourceManager::new().unwrap();
        manager
            .load_plugin(&entry, "org.deadline", "1", ENTRY_VERSION_V3)
            .unwrap();
        manager
            .set_test_callback_timeout("deadline", Duration::from_millis(40))
            .await;

        let error = tokio::time::timeout(
            Duration::from_secs(1),
            manager.call_discover("deadline", "", "", 1, &[]),
        )
        .await
        .expect("host deadline did not interrupt Lua")
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
    }

    #[test]
    fn video_source_plugin_discovers_video_files() {
        let lib = tempfile::tempdir().unwrap();
        let nested = lib.path().join("album");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(lib.path().join("clip.MP4"), b"video bytes").unwrap();
        std::fs::write(lib.path().join("animated.gif"), b"image source owns gif").unwrap();
        std::fs::write(nested.join("poster.png"), b"not a video").unwrap();
        std::fs::write(nested.join("loop.webm"), b"more video bytes").unwrap();

        let plugin_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins/org.waywallen.video/main.lua");

        let mgr = SourceManager::new().unwrap();
        let name = mgr
            .load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION)
            .unwrap();
        assert_eq!(name, "video");

        let mut libs = HashMap::new();
        libs.insert(
            "video".to_string(),
            vec![lib.path().to_string_lossy().to_string()],
        );
        block(async { mgr.scan_all(&libs).await.unwrap() });

        let entries = mgr.list();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.wp_type == "video"));
        assert!(entries.iter().all(|e| e.plugin_name == "video"));
        assert!(entries.iter().all(|e| e.preview.is_none()));
        assert!(entries.iter().all(|e| e.size.is_some()));
        assert!(entries.iter().all(|e| e.width.is_none()));
        assert!(entries.iter().all(|e| e.height.is_none()));
        assert!(entries.iter().all(|e| e.content_rating.is_none()));
        // SPAWN_VERSION 3 keeps the canonical resource path in
        // `entry.resource`.

        let clip_path = lib.path().join("clip.MP4").to_string_lossy().to_string();
        let clip = entries
            .iter()
            .find(|entry| entry.resource == clip_path)
            .unwrap()
            .clone();
        assert_eq!(clip.name, "clip");
        assert_eq!(clip.resource, clip_path);

        let extras = block_value(async { mgr.call_extras("video", &clip).await.unwrap() });
        assert_eq!(extras.get("path"), Some(&clip.resource));

        assert_eq!(mgr.list_by_type("video").len(), 2);
        assert!(mgr.list_by_type("image").is_empty());
    }
}
