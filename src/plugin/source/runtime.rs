use super::parsing::{parse_lua_string_map, redact_secrets};
use super::*;
use crate::plugin::i18n::LocalizedText;

const MAX_LOCALIZED_ARGUMENTS: usize = 16;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(transparent)]
struct LuaLocalizedText(LocalizedText);

impl LuaUserData for LuaLocalizedText {}

#[derive(Debug, Clone)]
pub(super) struct SourceCapability {
    pub(super) types: Vec<WallpaperType>,
    pub(super) library_label: LocalizedText,
    pub(super) library_hint: LocalizedText,
    pub(super) auto_detect: bool,
}

#[derive(Debug, Clone)]
pub(super) struct DiscoverCapability {
    pub(super) supports_search: bool,
    pub(super) supports_details: bool,
    pub(super) supports_download: bool,
    pub(super) supports_resolve: bool,
    pub(super) remote: Option<RemoteCapability>,
    pub(super) remote_hint: LocalizedText,
    pub(super) sorts: Vec<DiscoverSort>,
    pub(super) filters: Vec<DiscoverFilter>,
    /// The plugin exposes `discover.tags(ctx)`; the daemon calls it to refresh
    /// a legacy multi-select filter from a live source.
    pub(super) dynamic_tags: bool,
}

#[derive(Debug, Clone, Default)]
struct WallpaperCapability {
    apply: bool,
    properties: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WallpaperApply {
    pub extras: HashMap<String, String>,
    pub default_user_properties: HashMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct PluginCapabilities {
    pub(super) source: Option<SourceCapability>,
    pub(super) source_item_remove: bool,
    pub(super) discover: Option<DiscoverCapability>,
    wallpaper: WallpaperCapability,
    lifecycle: bool,
}

#[derive(Debug, Clone)]
struct PluginStateMigration {
    schema_id: String,
    file: String,
}

#[derive(Debug, Clone)]
pub(super) struct LoadedPluginInfo {
    pub(super) name: String,
    /// Human-readable display name from `info().display_name`; falls back to
    /// `name` when unset.
    pub(super) display_name: LocalizedText,
    pub(super) plugin_id: String,
    pub(super) version: String,
    pub(super) capabilities: PluginCapabilities,
    pub(super) settings: Vec<SourceSetting>,
    pub(super) actions: Vec<SourceAction>,
    pub(super) status: Vec<SourceStatus>,
    state_migrations: Vec<PluginStateMigration>,
}

struct PluginCallbacks {
    source_scan: Option<LuaRegistryKey>,
    source_auto_detect: Option<LuaRegistryKey>,
    source_remove: Option<LuaRegistryKey>,
    discover_search: Option<LuaRegistryKey>,
    discover_tags: Option<LuaRegistryKey>,
    discover_details: Option<LuaRegistryKey>,
    discover_download: Option<LuaRegistryKey>,
    discover_resolve: Option<LuaRegistryKey>,
    wallpaper_apply: Option<LuaRegistryKey>,
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
    pub(super) lua: Lua,
    callback_deadline: Arc<StdMutex<Option<Instant>>>,
    pub(super) callback_timeout: Duration,
    /// plugin name → registry key for the loaded module table.
    plugins: HashMap<String, LuaRegistryKey>,
    callbacks: HashMap<String, PluginCallbacks>,
    /// source `info().name` → parsed entry metadata.
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
    pub(super) qr_operations: HashMap<u64, LuaRegistryKey>,
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
        lua.set_memory_limit(LUA_PLUGIN_MEMORY_LIMIT)
            .map_err(|error| Error::Internal(anyhow!("set Lua memory limit: {error}")))?;
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

    pub(super) fn set_state_store(
        &mut self,
        state_store: crate::plugin::state_store::PluginStateStore,
    ) {
        self.state_store = state_store;
    }

    pub(super) fn loaded_plugin_info(&self, name: &str) -> Option<LoadedPluginInfo> {
        self.plugin_infos.get(name).cloned()
    }

    pub(super) fn generation_state(&self) -> Arc<AtomicU8> {
        self.generation_state.clone()
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

    pub(super) fn plugin_lua_env(&self, root: &Path, plugin_id: &str) -> Result<LuaTable> {
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

        let plugin_id = plugin_id.to_owned();
        let tr_fn = self
            .lua
            .create_function(move |lua, mut arguments: LuaMultiValue| {
                if arguments.is_empty() || arguments.len() > MAX_LOCALIZED_ARGUMENTS + 1 {
                    return Err(LuaError::RuntimeError(format!(
                        "tr requires a msgid and at most {MAX_LOCALIZED_ARGUMENTS} arguments"
                    )));
                }
                let msgid = match arguments.pop_front().expect("argument count checked") {
                    LuaValue::String(value) => value.to_str().map(|value| value.to_string()),
                    other => Err(LuaError::RuntimeError(format!(
                        "tr msgid must be a string, got {}",
                        other.type_name()
                    ))),
                }?;
                if msgid.is_empty() {
                    return Err(LuaError::RuntimeError(
                        "tr msgid must not be empty".to_string(),
                    ));
                }
                let arguments = arguments
                    .into_iter()
                    .map(|argument| match argument {
                        LuaValue::String(value) => value
                            .to_str()
                            .map(|value| value.to_string())
                            .map_err(LuaError::external),
                        LuaValue::Integer(value) => Ok(value.to_string()),
                        LuaValue::Number(value) => Ok(value.to_string()),
                        LuaValue::Boolean(value) => Ok(value.to_string()),
                        other => Err(LuaError::RuntimeError(format!(
                            "tr arguments must be strings, numbers, or booleans, got {}",
                            other.type_name()
                        ))),
                    })
                    .collect::<LuaResult<Vec<_>>>()?;
                lua.create_ser_userdata(LuaLocalizedText(LocalizedText::translated_with_arguments(
                    plugin_id.clone(),
                    msgid,
                    arguments,
                )))
            })?;
        env.set("tr", tr_fn)?;
        Ok(env)
    }

    fn localized_text(value: LuaValue, field: &str, context: &str) -> Result<LocalizedText> {
        match value {
            LuaValue::Nil => Ok(LocalizedText::default()),
            LuaValue::String(value) => value
                .to_str()
                .map(|value| LocalizedText::raw(value.to_string()))
                .map_err(|error| {
                    Error::Internal(anyhow!("{context}.{field} invalid string: {error}"))
                }),
            LuaValue::UserData(value) => value
                .borrow::<LuaLocalizedText>()
                .map(|value| value.0.clone())
                .map_err(|_| {
                    Error::Internal(anyhow!("{context}.{field} must be a string or tr() result"))
                }),
            other => Err(Error::Internal(anyhow!(
                "{context}.{field} must be a string or tr() result, got {}",
                other.type_name()
            ))),
        }
    }

    fn require_localized_text(tbl: &LuaTable, key: &str, context: &str) -> Result<LocalizedText> {
        let value = tbl
            .get::<LuaValue>(key)
            .map_err(|error| Error::Internal(anyhow!("{context}.{key} required: {error}")))?;
        let value = Self::localized_text(value, key, context)?;
        if value.text().is_empty() {
            return Err(Error::Internal(anyhow!(
                "{context}.{key} must not be empty"
            )));
        }
        Ok(value)
    }

    fn optional_localized_text(tbl: &LuaTable, key: &str, context: &str) -> Result<LocalizedText> {
        let value = tbl
            .get::<LuaValue>(key)
            .map_err(|error| Error::Internal(anyhow!("{context}.{key}: {error}")))?;
        Self::localized_text(value, key, context)
    }

    fn normalize_localized_value(
        lua: &Lua,
        value: LuaValue,
        visited: &mut HashSet<usize>,
    ) -> LuaResult<LuaValue> {
        let LuaValue::Table(table) = value else {
            if let LuaValue::UserData(value) = &value {
                if let Ok(value) = value.borrow::<LuaLocalizedText>() {
                    return lua.to_value(&value.0);
                }
            }
            return Ok(value);
        };

        let pointer = table.to_pointer() as usize;
        if !visited.insert(pointer) {
            return Err(LuaError::SerializeError(
                "recursive property schema table".to_string(),
            ));
        }

        let normalized = lua.create_table()?;
        for pair in table.pairs::<LuaValue, LuaValue>() {
            let (key, value) = pair?;
            if let (LuaValue::String(field), LuaValue::UserData(localized)) = (&key, &value) {
                if let Ok(localized) = localized.borrow::<LuaLocalizedText>() {
                    normalized.raw_set(key.clone(), localized.0.text().to_string())?;
                    let field = field.to_str()?;
                    normalized
                        .raw_set(format!("localized_{field}"), lua.to_value(&localized.0)?)?;
                    continue;
                }
            }
            normalized.raw_set(
                Self::normalize_localized_value(lua, key, visited)?,
                Self::normalize_localized_value(lua, value, visited)?,
            )?;
        }
        visited.remove(&pointer);
        Ok(LuaValue::Table(normalized))
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

    fn redact_localized_text(value: LocalizedText) -> LocalizedText {
        match value {
            LocalizedText::Raw(value) => LocalizedText::raw(redact_secrets(&value)),
            LocalizedText::Message(mut message) => {
                message.msgid = redact_secrets(&message.msgid);
                for argument in &mut message.arguments {
                    *argument = redact_secrets(argument);
                }
                LocalizedText::Message(message)
            }
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
            let label = Self::require_localized_text(&sort, "label", &context)?;
            if key.is_empty() {
                return Err(Error::Internal(anyhow!(
                    "{context}.key and {context}.label must not be empty"
                )));
            }
            sorts.push(DiscoverSort { key, label });
        }
        Ok(sorts)
    }

    pub(super) fn legacy_discover_filter(tags: Vec<String>) -> Vec<DiscoverFilter> {
        if tags.is_empty() {
            return Vec::new();
        }
        vec![DiscoverFilter {
            id: "tags".to_string(),
            title: LocalizedText::raw("Tags"),
            ty: DiscoverFilterType::MultiSelect,
            options: tags
                .into_iter()
                .map(|value| DiscoverFilterOption {
                    label: LocalizedText::raw(value.clone()),
                    value,
                })
                .collect(),
            description: LocalizedText::default(),
            confirmation: LocalizedText::default(),
        }]
    }

    fn discover_filter_options(
        filter: &LuaTable,
        context: &str,
        entry_version: u32,
    ) -> Result<(Vec<DiscoverFilterOption>, &'static str)> {
        let options = Self::optional_table(filter, "options", context)?;
        let values = Self::optional_table(filter, "values", context)?;
        if options.is_some() && values.is_some() {
            return Err(Error::Internal(anyhow!(
                "{context}.options and legacy {context}.values cannot both be present"
            )));
        }

        if let Some(options) = options {
            let mut out = Vec::new();
            for (idx, option) in options.sequence_values::<LuaTable>().enumerate() {
                let option_context = format!("{context}.options[{}]", idx + 1);
                let option = option.map_err(|error| {
                    Error::Internal(anyhow!("{option_context} must be an object: {error}"))
                })?;
                out.push(DiscoverFilterOption {
                    value: Self::require_string(&option, "value", &option_context)?,
                    label: Self::require_localized_text(&option, "label", &option_context)?,
                });
            }
            return Ok((out, "options"));
        }

        if let Some(values) = values {
            if entry_version != ENTRY_VERSION_V3 {
                return Err(Error::Internal(anyhow!(
                    "{context}.values is only supported for entry_version {ENTRY_VERSION_V3}; use {context}.options"
                )));
            }
            let mut out = Vec::new();
            for (idx, value) in values.sequence_values::<String>().enumerate() {
                let value = value.map_err(|error| {
                    Error::Internal(anyhow!(
                        "{context}.values[{}] must be a string: {error}",
                        idx + 1
                    ))
                })?;
                out.push(DiscoverFilterOption {
                    label: LocalizedText::raw(value.clone()),
                    value,
                });
            }
            return Ok((out, "values"));
        }

        Err(Error::Internal(anyhow!("{context}.options required")))
    }

    fn optional_discover_filters(
        discover_tbl: &LuaTable,
        entry_version: u32,
    ) -> Result<Option<Vec<DiscoverFilter>>> {
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
            let title = Self::require_localized_text(&filter, "title", &context)?;
            if id.is_empty() {
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
            let (options, option_field) =
                Self::discover_filter_options(&filter, &context, entry_version)?;
            if options.is_empty() {
                return Err(Error::Internal(anyhow!(
                    "{context}.{option_field} must not be empty"
                )));
            }
            if ty == DiscoverFilterType::Toggle && options.len() != 1 {
                return Err(Error::Internal(anyhow!(
                    "{context}.{option_field} must contain exactly one value for a toggle"
                )));
            }
            for option in &options {
                if option.value.is_empty() {
                    return Err(Error::Internal(anyhow!(
                        "{context}.{option_field} must not contain an empty value"
                    )));
                }
                if !filter_values.insert(option.value.clone()) {
                    return Err(Error::Internal(anyhow!(
                        "{context}.{option_field} contains duplicate discover value '{}'",
                        option.value
                    )));
                }
            }

            let description = Self::optional_localized_text(&filter, "description", &context)?;
            let confirmation = Self::optional_localized_text(&filter, "confirmation", &context)?;
            if !confirmation.text().is_empty() && ty != DiscoverFilterType::Toggle {
                return Err(Error::Internal(anyhow!(
                    "{context}.confirmation is only supported for toggle filters"
                )));
            }
            filters.push(DiscoverFilter {
                id,
                title,
                ty,
                options,
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
                .any(|filter| filter.options.iter().any(|option| option.value == *value))
            {
                return Err(Error::InvalidArgument(format!(
                    "source plugin '{plugin_name}' does not declare discover filter value '{value}'"
                )));
            }
        }
        for filter in &discover.filters {
            if filter.ty == DiscoverFilterType::Select
                && filter
                    .options
                    .iter()
                    .filter(|option| selected.contains(option.value.as_str()))
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

    pub(super) fn optional_table(
        tbl: &LuaTable,
        key: &str,
        context: &str,
    ) -> Result<Option<LuaTable>> {
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
                } else {
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
                    library_label: Self::optional_localized_text(
                        &source_tbl,
                        "library_label",
                        "info().capabilities.source",
                    )?,
                    library_hint: Self::optional_localized_text(
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
                } else {
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
                if supports_resolve && remote != Some(RemoteCapability::Download) {
                    return Err(Error::Internal(anyhow!(
                        "info().capabilities.discover.resolve requires download capability"
                    )));
                }
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
                let sorts = Self::optional_discover_sorts(&discover_tbl)?;
                let declared_filters =
                    Self::optional_discover_filters(&discover_tbl, entry_version)?;
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
                    remote_hint: Self::optional_localized_text(
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
            wallpaper.apply = Self::optional_bool(
                &wallpaper_tbl,
                "apply",
                "info().capabilities.wallpaper",
                false,
            )?;
            wallpaper.properties = Self::optional_bool(
                &wallpaper_tbl,
                "properties",
                "info().capabilities.wallpaper",
                false,
            )?;
            if wallpaper.apply {
                Self::require_table_function(&wallpaper_api, "apply", "module.wallpaper")?;
            } else {
                Self::require_field_absent(&wallpaper_api, "apply", "module.wallpaper")?;
            }
            if wallpaper.properties {
                Self::require_table_function(&wallpaper_api, "properties", "module.wallpaper")?;
            } else {
                Self::require_field_absent(&wallpaper_api, "properties", "module.wallpaper")?;
            }
        }

        let settings = Self::parse_source_settings(&info_table)?;
        let actions = Self::parse_source_actions(&info_table)?;
        let status = Self::parse_source_status(&info_table)?;
        let state_migrations = Self::parse_state_migrations(&info_table)?;
        let lifecycle = Self::optional_table(module, "lifecycle", "module")?;
        let actions_api = Self::optional_table(module, "actions", "module")?;
        let qrlogin = Self::optional_table(module, "qrlogin", "module")?;
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
        let display_name = {
            let dn = Self::optional_localized_text(&info_table, "display_name", "info()")?;
            if dn.text().is_empty() {
                LocalizedText::raw(name.clone())
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
    fn parse_source_actions(info: &LuaTable) -> Result<Vec<SourceAction>> {
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
                "qr_login" => SourceActionKind::QrLogin,
                "form" => SourceActionKind::Form,
                value => {
                    return Err(Error::Internal(anyhow!(
                        "info().actions kind '{value}' is unsupported"
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
                    label: Self::optional_localized_text(&field, "label", "info().actions.fields")?,
                    description: Self::optional_localized_text(
                        &field,
                        "description",
                        "info().actions.fields",
                    )?,
                    placeholder: Self::optional_localized_text(
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
                label: Self::optional_localized_text(&entry, "label", "info().actions")?,
                description: Self::optional_localized_text(
                    &entry,
                    "description",
                    "info().actions",
                )?,
                browse_description: Self::optional_localized_text(
                    &entry,
                    "browse_description",
                    "info().actions",
                )?,
                browse_button_label: Self::optional_localized_text(
                    &entry,
                    "browse_button_label",
                    "info().actions",
                )?,
                group: Self::optional_string(&entry, "group", "info().actions")?,
                group_label: Self::optional_localized_text(
                    &entry,
                    "group_label",
                    "info().actions",
                )?,
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
            wallpaper_apply: self.callback_key(module, "wallpaper", "apply")?,
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
                label: Self::optional_localized_text(&entry, "label", "info().status")?,
                group: Self::optional_string(&entry, "group", "info().status")?,
                group_label: Self::optional_localized_text(&entry, "group_label", "info().status")?,
                order: entry
                    .get::<Option<i32>>("order")
                    .unwrap_or(None)
                    .unwrap_or(0),
                value: LocalizedText::default(),
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
                label: Self::optional_localized_text(&s, "label", "info().settings")?,
                description: Self::optional_localized_text(&s, "description", "info().settings")?,
                group: Self::optional_string(&s, "group", "info().settings")?,
                group_label: Self::optional_localized_text(&s, "group_label", "info().settings")?,
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
        let env = self.plugin_lua_env(root, plugin_id)?;
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
                web_url: tbl.get::<String>("web_url").ok(),
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

        // ctx.fs.read_bytes(path, offset, len) -> string|nil
        // A binary-safe window into a file, for plugins that must read one
        // member of a container without pulling the whole thing in. `read`
        // refuses anything over 1MB and decodes UTF-8; this reads at most 1MB
        // per call and returns the bytes as they are. A short read at EOF
        // returns what is there; nothing readable returns nil.
        let read_bytes_fn =
            self.lua
                .create_function(|lua, (path, offset, len): (String, i64, i64)| {
                    use std::io::{Read, Seek, SeekFrom};
                    if offset < 0 || len <= 0 || len > 1_048_576 {
                        return Ok(mlua::Value::Nil);
                    }
                    let Ok(mut file) = std::fs::File::open(&path) else {
                        return Ok(mlua::Value::Nil);
                    };
                    if file.seek(SeekFrom::Start(offset as u64)).is_err() {
                        return Ok(mlua::Value::Nil);
                    }
                    let mut buf = Vec::new();
                    if file.take(len as u64).read_to_end(&mut buf).is_err() || buf.is_empty() {
                        return Ok(mlua::Value::Nil);
                    }
                    Ok(mlua::Value::String(lua.create_string(&buf)?))
                })?;

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
        fs.set("read_bytes", read_bytes_fn)?;
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
                crate::catalog::path::relative_under_root(&entry.library_root, &entry.resource)
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
        if let Some(web_url) = &entry.web_url {
            entry_tbl.set("web_url", web_url.clone())?;
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

    /// Resolve renderer resources and authored property defaults for `entry`.
    pub async fn call_apply(
        &mut self,
        plugin_name: &str,
        entry: &WallpaperEntry,
    ) -> Result<WallpaperApply> {
        let callbacks = self
            .callbacks
            .get(plugin_name)
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
        let Some(info) = self.plugin_infos.get(plugin_name) else {
            return Err(Error::SourcePluginNotFound(plugin_name.to_string()));
        };
        if !info.capabilities.wallpaper.apply {
            log::warn!("source plugin '{plugin_name}' has no wallpaper.apply capability");
            return Ok(WallpaperApply::default());
        }
        let body = async {
            let apply_fn: LuaFunction = self.lua.registry_value(
                callbacks
                    .wallpaper_apply
                    .as_ref()
                    .ok_or_else(|| LuaError::external("module.wallpaper.apply required"))?,
            )?;
            let entry_tbl = self
                .wallpaper_entry_table(entry)
                .map_err(mlua::Error::external)?;
            // Build the same ctx scan(ctx) sees; apply runs per item, so
            // the libraries list is intentionally empty.
            let ctx = self
                .build_ctx(Some(plugin_name), &[])
                .map_err(mlua::Error::external)?;
            let result: LuaTable = self
                .call_plugin_callback_async(plugin_name, &apply_fn, (entry_tbl, ctx))
                .await?;
            Ok(WallpaperApply {
                extras: parse_lua_string_map(&result, "extras", "wallpaper.apply result")
                    .map_err(mlua::Error::external)?,
                default_user_properties: parse_lua_string_map(
                    &result,
                    "default_user_properties",
                    "wallpaper.apply result",
                )
                .map_err(mlua::Error::external)?,
            })
        };
        let result = body
            .await
            .map_err(|e: mlua::Error| Error::SourceApplyFailed {
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
            other => {
                let normalized =
                    Self::normalize_localized_value(&self.lua, other, &mut HashSet::new())?;
                mlua_extra::json::encode(&normalized)
            }
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
            web_url: Self::optional_string(&result, "web_url", "module.discover.download result")?,
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
            display_value: Self::optional_localized_text(
                &result,
                "display_value",
                "module.lifecycle.check result",
            )?,
            error: Self::redact_localized_text(Self::optional_localized_text(
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
                row.value = Self::optional_localized_text(
                    &values,
                    row.id.as_str(),
                    "module.actions.status result.status",
                )?;
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
        let title = Self::optional_localized_text(&result, "title", "qrlogin.begin result")?;
        let instruction =
            Self::optional_localized_text(&result, "instruction", "qrlogin.begin result")?;
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
            display_value: Self::optional_localized_text(
                &result,
                "display_value",
                "qrlogin.poll result",
            )?,
            error: Self::redact_localized_text(Self::optional_localized_text(
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
    ) -> Result<SubscriptionStatusResult> {
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
        let wrapped_items = match result.get::<LuaValue>("items").map_err(|error| {
            Error::Internal(anyhow!("module.subscription.status result.items: {error}"))
        })? {
            LuaValue::Table(items) => Some(items),
            _ => None,
        };
        let (items, error) = if let Some(items) = wrapped_items {
            let error = Self::redact_localized_text(Self::optional_localized_text(
                &result,
                "error",
                "module.subscription.status result",
            )?);
            (items, error)
        } else {
            (result, LocalizedText::default())
        };
        let mut states = Vec::with_capacity(ids.len());
        for id in ids {
            let value = items.get::<String>(id.as_str()).unwrap_or_default();
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
        Ok(SubscriptionStatusResult {
            items: states,
            error,
        })
    }

    pub async fn set_subscription(
        &mut self,
        plugin_name: &str,
        id: &str,
        subscribed: bool,
    ) -> Result<SubscriptionSetResult> {
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
        let error = Self::redact_localized_text(Self::optional_localized_text(
            &result,
            "error",
            "module.subscription mutation result",
        )?);
        self.persist_state(plugin_name)?;
        Ok(SubscriptionSetResult { accepted, error })
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
