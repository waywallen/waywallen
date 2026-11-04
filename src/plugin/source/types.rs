use super::*;
use crate::plugin::i18n::LocalizedText;

/// User-Agent the `ctx.http` default client sends.
pub(super) const WAYWALLEN_HTTP_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) waywallen";
pub(super) const LUA_CALLBACK_TIMEOUT: Duration = Duration::from_secs(25);
pub(super) const LUA_HOOK_INSTRUCTION_INTERVAL: u32 = 10_000;
/// Per-plugin Lua heap cap. Turns an accidental Lua-side runaway in one plugin into a
/// catchable `MemoryError` instead of unbounded daemon growth; it does not bound
/// Rust-side allocations (HTTP/JSON/HTML bodies).
pub(super) const LUA_PLUGIN_MEMORY_LIMIT: usize = 128 * 1024 * 1024;
pub(super) const RUNTIME_ACTIVE: u8 = 0;
pub(super) const RUNTIME_DRAINING: u8 = 1;
pub(super) const RUNTIME_INACTIVE: u8 = 2;

pub const ENTRY_VERSION_V3: u32 = 3;
pub const ENTRY_VERSION_V4: u32 = 4;
pub const ENTRY_VERSION_V5: u32 = 5;
pub const ENTRY_VERSION_V6: u32 = 6;
pub const ENTRY_VERSION_V7: u32 = 7;
pub const ENTRY_VERSION: u32 = ENTRY_VERSION_V7;
pub const LATEST_ENTRY_VERSION: u32 = ENTRY_VERSION_V7;
pub const SUPPORTED_ENTRY_VERSIONS: &[u32] = &[
    ENTRY_VERSION_V3,
    ENTRY_VERSION_V4,
    ENTRY_VERSION_V5,
    ENTRY_VERSION_V6,
    ENTRY_VERSION_V7,
];

pub(super) const MAX_DISCOVER_CURSOR_BYTES: usize = 4096;

pub fn supports_entry_version(version: u32) -> bool {
    SUPPORTED_ENTRY_VERSIONS.contains(&version)
}

pub(super) fn resolve_plugin_import(root: &Path, name: &str) -> LuaResult<PathBuf> {
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
    pub library_label: LocalizedText,
    /// Longer helper text for choosing a library path.
    /// May contain newlines or inline-code Markdown markers.
    pub library_hint: LocalizedText,
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
    pub label: LocalizedText,
    pub description: LocalizedText,
    pub group: String,
    pub group_label: LocalizedText,
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
    pub label: LocalizedText,
    pub description: LocalizedText,
    pub placeholder: LocalizedText,
    pub secret: bool,
    pub required: bool,
}

/// A button declared by `info().actions` and routed through the generic plugin
/// action contract.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceAction {
    pub id: String,
    pub label: LocalizedText,
    pub description: LocalizedText,
    pub browse_description: LocalizedText,
    pub browse_button_label: LocalizedText,
    pub group: String,
    pub group_label: LocalizedText,
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
    pub label: LocalizedText,
    pub group: String,
    pub group_label: LocalizedText,
    pub order: i32,
    pub value: LocalizedText,
}

/// One sort option a discover-capable plugin advertises via
/// `info().capabilities.discover.sorts`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiscoverSort {
    pub key: String,
    pub label: LocalizedText,
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
    pub title: LocalizedText,
    pub ty: DiscoverFilterType,
    pub options: Vec<DiscoverFilterOption>,
    pub description: LocalizedText,
    pub confirmation: LocalizedText,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DiscoverFilterOption {
    /// Machine-readable value clients submit for filtering.
    pub value: String,
    pub label: LocalizedText,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DiscoverDownloadAction {
    pub enabled: bool,
    pub label: LocalizedText,
    pub tooltip: LocalizedText,
}

impl Default for DiscoverDownloadAction {
    fn default() -> Self {
        Self {
            enabled: true,
            label: LocalizedText::default(),
            tooltip: LocalizedText::default(),
        }
    }
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
    pub display_name: LocalizedText,
    pub supports_search: bool,
    pub pagination: DiscoverPagination,
    pub remote_capability: Option<RemoteCapability>,
    pub remote_hint: LocalizedText,
    pub download_action: DiscoverDownloadAction,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverPagination {
    #[default]
    Page,
    Cursor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteCapability {
    Download,
    Subscription,
}

pub(super) fn validate_source_setting(
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
pub struct SubscriptionStatusResult {
    pub items: Vec<SubscriptionItemState>,
    pub error: LocalizedText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionSetResult {
    pub accepted: bool,
    pub error: LocalizedText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrLoginBegin {
    pub operation_id: u64,
    pub challenge: String,
    pub poll_after_ms: u64,
    pub expires_in_ms: Option<u64>,
    pub title: LocalizedText,
    pub instruction: LocalizedText,
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
    pub display_value: LocalizedText,
    pub error: LocalizedText,
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
    pub display_value: LocalizedText,
    pub error: LocalizedText,
    pub avatar_url: String,
}

/// One remote item returned by a plugin's `discover.search(ctx, params)`.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DiscoverItem {
    pub id: String,
    pub title: String,
    pub preview_url: String,
    pub author: String,
    pub author_avatar_url: String,
    pub wp_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub extra: HashMap<String, String>,
}

/// One provider-owned information row returned with remote details.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DiscoverDetailField {
    pub key: String,
    pub label: LocalizedText,
    pub value: String,
    pub url: String,
}

/// Detail blob returned by a plugin's `discover.details(ctx, id)`.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DiscoverDetails {
    pub author: String,
    pub author_avatar_url: String,
    pub author_url: String,
    pub description: String,
    pub size: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub tags: Vec<String>,
    pub web_url: String,
    pub extra: HashMap<String, String>,
    pub fields: Vec<DiscoverDetailField>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DiscoverSearchResult {
    pub items: Vec<DiscoverItem>,
    pub has_more: bool,
    pub next_cursor: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DownloadFormat {
    #[default]
    File,
    Zip,
}

#[derive(Debug, Clone, Default)]
pub struct DiscoverTransfer {
    pub url: String,
    pub filename: String,
    pub format: DownloadFormat,
    pub resolve: bool,
    pub resources: Vec<DiscoverResource>,
    pub prepare: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoverResource {
    pub id: String,
    pub url: String,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DiscoverAttribution {
    pub author: String,
    pub author_url: String,
    pub license: String,
    pub license_url: String,
    pub requirements: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DiscoverMetadata {
    pub wp_type: String,
    pub title: String,
    pub preview_url: String,
    pub description: String,
    pub tags: Vec<String>,
    pub external_id: String,
    #[serde(default)]
    pub web_url: String,
    pub size: Option<i64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub content_rating: Option<String>,
    #[serde(default)]
    pub attribution: Option<DiscoverAttribution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_data: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Default)]
pub struct DiscoverDownload {
    pub transfer: DiscoverTransfer,
    pub metadata: DiscoverMetadata,
}

/// Retains remote filesystem ownership until the caller finishes deleting its DB row.
#[derive(Debug)]
pub struct SourceRemoval {
    pub(super) _artifact: Option<crate::remote_asset::Removal>,
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
    pub provider_data: Option<serde_json::Map<String, serde_json::Value>>,
}
