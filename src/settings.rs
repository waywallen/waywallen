use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::RwLock as StdRwLock;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tokio::sync::Notify;

use crate::display::layout::{Align, FillMode, Location, Rotation};

/// Quiet period after the last `update()` before the debounced writer
/// flushes to disk.
const DEBOUNCE_WRITE: Duration = Duration::from_secs(2);
pub const DEFAULT_AUDIO_FADE_MS: u32 = 500;
pub const MAX_AUDIO_FADE_MS: u32 = 2000;

/// Daemon-wide layout defaults applied to displays that have no
/// `[displays.<name>]` override.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutDefaults {
    pub fillmode: FillMode,
    pub location: Option<Location>,
    pub align: Align,
    pub rotation: Rotation,
}

/// Per-display overrides keyed by display name.
/// `None` fields inherit from the global defaults.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayPrefs {
    pub fillmode: Option<FillMode>,
    pub location: Option<Location>,
    pub align: Option<Align>,
    pub rotation: Option<Rotation>,
    pub auto_replay: Option<AutoReplayPolicy>,
    /// Last wallpaper id applied to this display.
    /// Used to restore per-display assignment on restart.
    pub last_wallpaper: Option<String>,
    pub lock_wallpaper: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_lock_screen: bool,
    pub alias: Option<String>,
    pub active_playlist_id: Option<i64>,
}

impl DisplayPrefs {
    pub fn is_empty(&self) -> bool {
        self.fillmode.is_none()
            && self.location.is_none()
            && self.align.is_none()
            && self.rotation.is_none()
            && self.auto_replay.is_none()
            && self.last_wallpaper.is_none()
            && self.lock_wallpaper.is_none()
            && !self.has_lock_screen
            && self.alias.is_none()
            && self.active_playlist_id.is_none()
    }
}

/// Layout values resolved against (per-display override → global → built-in defaults).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedLayout {
    pub fillmode: FillMode,
    pub location: Location,
    pub rotation: Rotation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutoCondition {
    #[default]
    AnyWindow,
    Focused,
    Maximized,
    Fullscreen,
    SessionLocked,
    SessionInactive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutoAction {
    #[default]
    None,
    Mute,
    Pause,
    Stop,
}

impl AutoAction {
    pub fn priority(self) -> u8 {
        match self {
            AutoAction::None => 0,
            AutoAction::Mute => 1,
            AutoAction::Pause => 2,
            AutoAction::Stop => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoReplayPolicy {
    pub any_window: AutoAction,
    pub focused: AutoAction,
    pub maximized: AutoAction,
    pub fullscreen: AutoAction,
    pub session_locked: AutoAction,
    pub session_inactive: AutoAction,
}

impl Default for AutoReplayPolicy {
    fn default() -> Self {
        Self {
            any_window: AutoAction::None,
            focused: AutoAction::None,
            maximized: AutoAction::None,
            fullscreen: AutoAction::Pause,
            session_locked: AutoAction::Stop,
            session_inactive: AutoAction::Stop,
        }
    }
}

impl AutoReplayPolicy {
    pub fn action_for(self, condition: AutoCondition) -> AutoAction {
        match condition {
            AutoCondition::AnyWindow => self.any_window,
            AutoCondition::Focused => self.focused,
            AutoCondition::Maximized => self.maximized,
            AutoCondition::Fullscreen => self.fullscreen,
            AutoCondition::SessionLocked => self.session_locked,
            AutoCondition::SessionInactive => self.session_inactive,
        }
    }

    pub fn set_action(&mut self, condition: AutoCondition, action: AutoAction) {
        let slot = match condition {
            AutoCondition::AnyWindow => &mut self.any_window,
            AutoCondition::Focused => &mut self.focused,
            AutoCondition::Maximized => &mut self.maximized,
            AutoCondition::Fullscreen => &mut self.fullscreen,
            AutoCondition::SessionLocked => &mut self.session_locked,
            AutoCondition::SessionInactive => &mut self.session_inactive,
        };
        *slot = action;
    }
}

/// Daemon-wide defaults consumed by `WallpaperApply` when a renderer
/// has no per-plugin override.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalSettings {
    pub last_wallpaper: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_wallpaper: Option<String>,
    /// Queue playback mode: `"sequential"` / `"shuffle"` / `"random"`.
    /// Restored on startup so the rotator resumes the same behavior.
    #[serde(alias = "playlist_mode")]
    pub queue_mode: String,
    /// Auto-rotation interval in seconds; `0` = disabled.
    pub rotation_secs: u32,
    /// Fade duration shared by mute and unmute control messages.
    pub audio_fade_ms: u32,
    /// Manual global mute requested through daemon controls.
    pub manual_muted: bool,
    /// Default layout used when a display has no override.
    /// Drives daemon-side projection.
    pub layout: LayoutDefaults,
    #[serde(
        default,
        alias = "auto_actions",
        skip_serializing_if = "Option::is_none"
    )]
    pub auto_replay: Option<AutoReplayPolicy>,
    /// Structured wallpaper-browser filter state.
    /// Kept typed in memory but serialized as a JSON string.
    #[serde(
        default,
        rename = "wallpaper_filter_json",
        alias = "wallpaper_filter",
        serialize_with = "serialize_wallpaper_filter_state",
        deserialize_with = "deserialize_wallpaper_filter_state"
    )]
    pub wallpaper_filter: WallpaperFilterState,

    #[serde(default)]
    pub wallpaper_sorts: Vec<WallpaperSortRuleState>,

    /// Wallpaper types hidden by the browser's quick type toggles.
    #[serde(default)]
    pub wallpaper_skip_types: Vec<String>,

    /// Quick tag filter: show only wallpapers having any of these tags.
    /// Empty = no constraint.
    #[serde(default)]
    pub wallpaper_filter_tags: Vec<String>,

    /// Content ratings hidden by the browser's quick toggles.
    #[serde(default)]
    pub wallpaper_skip_content_ratings: Vec<String>,

    #[serde(default)]
    pub auto_attach_playlist_id: Option<i64>,

    pub plugin_update_notifications: bool,

    pub duplicate_renderers_for_same_wallpaper: bool,

    /// Last autostart state successfully accepted by the Flatpak portal.
    pub autostart_enabled: bool,
}

impl Default for GlobalSettings {
    fn default() -> Self {
        Self {
            last_wallpaper: None,
            lock_wallpaper: None,
            queue_mode: "sequential".to_string(),
            rotation_secs: 0,
            audio_fade_ms: DEFAULT_AUDIO_FADE_MS,
            manual_muted: false,
            layout: LayoutDefaults::default(),
            auto_replay: None,
            wallpaper_filter: WallpaperFilterState::default(),
            wallpaper_sorts: Vec::new(),
            wallpaper_skip_types: Vec::new(),
            wallpaper_filter_tags: Vec::new(),
            wallpaper_skip_content_ratings: Vec::new(),
            auto_attach_playlist_id: None,
            plugin_update_notifications: true,
            duplicate_renderers_for_same_wallpaper: false,
            autostart_enabled: false,
        }
    }
}

impl GlobalSettings {
    pub fn effective_auto_replay(&self) -> AutoReplayPolicy {
        self.auto_replay.unwrap_or_default()
    }

    pub fn effective_audio_fade_ms(&self) -> u32 {
        self.audio_fade_ms.min(MAX_AUDIO_FADE_MS)
    }

    /// Filter rules and logic for the queue.
    /// Quick skip toggles are folded into the rule list.
    pub fn wallpaper_queue_filter(
        &self,
    ) -> (
        Vec<crate::control_proto::WallpaperFilterRule>,
        Vec<crate::control_proto::FilterLogic>,
    ) {
        use crate::control_proto as pb;
        let (mut filters, logics) = self.wallpaper_filter.to_pb();
        let mut next_group = filters
            .iter()
            .map(|f| f.group)
            .max()
            .map(|g| g + 1)
            .unwrap_or(0);
        for ty in &self.wallpaper_skip_types {
            filters.push(pb::WallpaperFilterRule {
                r#type: pb::WallpaperFilterType::WpType as i32,
                group: next_group,
                payload: Some(pb::wallpaper_filter_rule::Payload::StringFilter(
                    pb::WallpaperStringFilter {
                        value: ty.clone(),
                        condition: pb::StringCondition::IsNot as i32,
                    },
                )),
            });
            next_group += 1;
        }
        if !self.wallpaper_filter_tags.is_empty() {
            filters.push(pb::WallpaperFilterRule {
                r#type: pb::WallpaperFilterType::Tag as i32,
                group: next_group,
                payload: Some(pb::wallpaper_filter_rule::Payload::TagFilter(
                    pb::WallpaperTagFilter {
                        values: self.wallpaper_filter_tags.clone(),
                        condition: pb::StringCondition::Is as i32,
                    },
                )),
            });
            next_group += 1;
        }
        for rating in &self.wallpaper_skip_content_ratings {
            filters.push(pb::WallpaperFilterRule {
                r#type: pb::WallpaperFilterType::ContentRating as i32,
                group: next_group,
                payload: Some(pb::wallpaper_filter_rule::Payload::StringFilter(
                    pb::WallpaperStringFilter {
                        value: rating.clone(),
                        condition: pb::StringCondition::IsNot as i32,
                    },
                )),
            });
            next_group += 1;
        }
        (filters, logics)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WallpaperSortRuleState {
    pub key: i32,
    pub direction: i32,
}

impl WallpaperSortRuleState {
    pub fn vec_to_pb(v: &[WallpaperSortRuleState]) -> Vec<crate::control_proto::WallpaperSortRule> {
        v.iter()
            .map(|r| crate::control_proto::WallpaperSortRule {
                key: r.key,
                direction: r.direction,
            })
            .collect()
    }

    pub fn vec_from_pb(
        v: &[crate::control_proto::WallpaperSortRule],
    ) -> Vec<WallpaperSortRuleState> {
        v.iter()
            .map(|r| WallpaperSortRuleState {
                key: r.key,
                direction: r.direction,
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WallpaperFilterState {
    pub filters: Vec<WallpaperFilterRuleState>,
    pub filter_logics: Vec<FilterLogicState>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FilterLogicState {
    pub op: i32,
    pub group_a: i32,
    pub group_b: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WallpaperFilterRuleState {
    pub r#type: i32,
    pub group: i32,
    pub string_filter: Option<WallpaperStringFilterState>,
    pub int_filter: Option<WallpaperIntFilterState>,
    pub tag_filter: Option<WallpaperTagFilterState>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WallpaperTagFilterState {
    pub values: Vec<String>,
    pub condition: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WallpaperStringFilterState {
    pub value: String,
    pub condition: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WallpaperIntFilterState {
    pub value: i64,
    pub condition: i32,
}

impl WallpaperFilterState {
    /// Convert the persisted state into the proto rules + logics used
    /// by `model::filter::wallpaper_filters_to_condition`.
    pub fn to_pb(
        &self,
    ) -> (
        Vec<crate::control_proto::WallpaperFilterRule>,
        Vec<crate::control_proto::FilterLogic>,
    ) {
        use crate::control_proto as pb;
        let rules = self
            .filters
            .iter()
            .cloned()
            .map(|rule| {
                let payload = if let Some(f) = rule.tag_filter {
                    Some(pb::wallpaper_filter_rule::Payload::TagFilter(
                        pb::WallpaperTagFilter {
                            values: f.values,
                            condition: f.condition,
                        },
                    ))
                } else if let Some(f) = rule.string_filter {
                    Some(pb::wallpaper_filter_rule::Payload::StringFilter(
                        pb::WallpaperStringFilter {
                            value: f.value,
                            condition: f.condition,
                        },
                    ))
                } else {
                    rule.int_filter.map(|f| {
                        pb::wallpaper_filter_rule::Payload::IntFilter(pb::WallpaperIntFilter {
                            value: f.value,
                            condition: f.condition,
                        })
                    })
                };
                pb::WallpaperFilterRule {
                    r#type: rule.r#type,
                    group: rule.group,
                    payload,
                }
            })
            .collect();
        let logics = self
            .filter_logics
            .iter()
            .map(|logic| pb::FilterLogic {
                op: logic.op,
                group_a: logic.group_a,
                group_b: logic.group_b,
            })
            .collect();
        (rules, logics)
    }

    pub fn from_pb(
        rules: &[crate::control_proto::WallpaperFilterRule],
        logics: &[crate::control_proto::FilterLogic],
    ) -> Self {
        use crate::control_proto as pb;
        Self {
            filters: rules
                .iter()
                .map(|rule| WallpaperFilterRuleState {
                    r#type: rule.r#type,
                    group: rule.group,
                    string_filter: rule.payload.as_ref().and_then(|p| match p {
                        pb::wallpaper_filter_rule::Payload::StringFilter(f) => {
                            Some(WallpaperStringFilterState {
                                value: f.value.clone(),
                                condition: f.condition,
                            })
                        }
                        _ => None,
                    }),
                    int_filter: rule.payload.as_ref().and_then(|p| match p {
                        pb::wallpaper_filter_rule::Payload::IntFilter(f) => {
                            Some(WallpaperIntFilterState {
                                value: f.value,
                                condition: f.condition,
                            })
                        }
                        _ => None,
                    }),
                    tag_filter: rule.payload.as_ref().and_then(|p| match p {
                        pb::wallpaper_filter_rule::Payload::TagFilter(f) => {
                            Some(WallpaperTagFilterState {
                                values: f.values.clone(),
                                condition: f.condition,
                            })
                        }
                        _ => None,
                    }),
                })
                .collect(),
            filter_logics: logics
                .iter()
                .map(|logic| FilterLogicState {
                    op: logic.op,
                    group_a: logic.group_a,
                    group_b: logic.group_b,
                })
                .collect(),
        }
    }
}

fn serialize_wallpaper_filter_state<S>(
    state: &WallpaperFilterState,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let json = serde_json::to_string(state).map_err(serde::ser::Error::custom)?;
    serializer.serialize_str(&json)
}

fn deserialize_wallpaper_filter_state<'de, D>(
    deserializer: D,
) -> Result<WallpaperFilterState, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Json(String),
        Structured(WallpaperFilterState),
    }

    let repr = Repr::deserialize(deserializer)?;
    Ok(match repr {
        Repr::Structured(state) => state,
        Repr::Json(json) => serde_json::from_str(&json).unwrap_or_default(),
    })
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub global: GlobalSettings,
    /// Per-plugin string-to-string bag keyed by `RendererDef.name`.
    /// String values map cleanly to TOML and protobuf.
    #[serde(default, rename = "plugin")]
    pub plugins: HashMap<String, HashMap<String, String>>,
    /// Per-display layout overrides keyed by `register_display` name.
    /// Empty entries are pruned by mutators.
    #[serde(default, rename = "display")]
    pub displays: HashMap<String, DisplayPrefs>,
}

/// Resolve the on-disk location. Order:
///   1. `$XDG_CONFIG_HOME/waywallen/config.toml`
pub fn default_config_path() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("waywallen/config.toml");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".config/waywallen/config.toml");
    }
    PathBuf::from("waywallen.toml")
}

/// Resolve the SQLite database location.
/// Mirrors [`default_config_path`] but targets the XDG data dir.
pub fn default_db_path() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(xdg).join("waywallen/waywallen-v2.db");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".local/share/waywallen/waywallen-v2.db");
    }
    PathBuf::from("waywallen-v2.db")
}

pub struct SettingsStore {
    inner: Arc<StdRwLock<Settings>>,
    notify: Arc<Notify>,
    path: PathBuf,
    /// Serializes concurrent `flush()` calls.
    /// Covers both the debounced writer and shutdown flush.
    flush_lock: tokio::sync::Mutex<()>,
    /// Set when the in-memory state diverges from disk.
    /// Cleared by a successful `flush()`.
    dirty: AtomicBool,
}

impl SettingsStore {
    /// Load from `path`, or fall back to defaults and seed the file.
    /// Seeding makes the config visible to users immediately.
    pub async fn load_or_default(path: PathBuf) -> Arc<Self> {
        let mut seed_on_disk = false;
        let initial = match tokio::fs::read_to_string(&path).await {
            Ok(s) => match toml::from_str::<Settings>(&s) {
                Ok(parsed) => {
                    log::info!("settings loaded from {}", path.display());
                    parsed
                }
                Err(e) => {
                    log::warn!(
                        "settings parse {}: {e}; continuing with defaults",
                        path.display()
                    );
                    Settings::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                log::info!(
                    "settings file {} not found, seeding defaults",
                    path.display()
                );
                seed_on_disk = true;
                let mut settings = Settings::default();
                settings.global.auto_replay = Some(AutoReplayPolicy::default());
                settings
            }
            Err(e) => {
                log::warn!(
                    "settings file {} not readable ({e}); using defaults",
                    path.display()
                );
                Settings::default()
            }
        };

        let store = Arc::new(Self {
            inner: Arc::new(StdRwLock::new(initial)),
            notify: Arc::new(Notify::new()),
            path,
            flush_lock: tokio::sync::Mutex::new(()),
            // Mark dirty when no on-disk file exists so the seed flush
            // writes the default config.
            dirty: AtomicBool::new(seed_on_disk),
        });

        if seed_on_disk {
            store.flush().await;
        }

        // Debounced writer task.
        let writer = Arc::clone(&store);
        tokio::spawn(async move {
            writer.writer_loop().await;
        });

        store
    }

    /// Snapshot the current settings by cloning under a read lock.
    /// Callers needing only globals should use narrower helpers.
    pub fn snapshot(&self) -> Settings {
        self.inner.read().expect("settings poisoned").clone()
    }

    /// Copy the `GlobalSettings` subset.
    pub fn global(&self) -> GlobalSettings {
        self.inner.read().expect("settings poisoned").global.clone()
    }

    /// Clone the value map for a single plugin, or `None` if the
    /// plugin has no recorded settings.
    pub fn plugin(&self, plugin_name: &str) -> Option<HashMap<String, String>> {
        self.inner
            .read()
            .expect("settings poisoned")
            .plugins
            .get(plugin_name)
            .cloned()
    }

    /// Resolve the effective layout for a display name.
    /// Per-display overrides win field by field.
    pub fn resolved_layout(&self, display_name: &str) -> ResolvedLayout {
        let g = self.inner.read().expect("settings poisoned");
        let defaults = &g.global.layout;
        let prefs = g.displays.get(display_name);
        let default_location = defaults
            .location
            .unwrap_or_else(|| Location::from_align(defaults.align));
        ResolvedLayout {
            fillmode: prefs.and_then(|p| p.fillmode).unwrap_or(defaults.fillmode),
            location: prefs
                .and_then(|p| p.location)
                .or_else(|| prefs.and_then(|p| p.align.map(Location::from_align)))
                .unwrap_or(default_location),
            rotation: prefs.and_then(|p| p.rotation).unwrap_or(defaults.rotation),
        }
    }

    pub fn resolved_auto_replay(&self, display_name: &str) -> AutoReplayPolicy {
        let g = self.inner.read().expect("settings poisoned");
        if let Some(policy) = g
            .displays
            .get(display_name)
            .and_then(|prefs| prefs.auto_replay)
        {
            return policy;
        }
        if let Some(policy) = &g.global.auto_replay {
            return *policy;
        }
        AutoReplayPolicy::default()
    }

    /// Per-display wallpaper id with fallback to global `last_wallpaper`.
    /// Used by hot-plug recall and startup restore.
    pub fn resolved_last_wallpaper(&self, display_key: &str) -> Option<String> {
        let g = self.inner.read().expect("settings poisoned");
        if let Some(prefs) = g.displays.get(display_key) {
            if let Some(id) = &prefs.last_wallpaper {
                return Some(id.clone());
            }
        }
        g.global.last_wallpaper.clone()
    }

    pub fn resolved_lock_wallpaper(&self, display_key: &str) -> Option<String> {
        {
            let g = self.inner.read().expect("settings poisoned");
            if let Some(id) = g
                .displays
                .get(display_key)
                .and_then(|prefs| prefs.lock_wallpaper.clone())
                .or_else(|| g.global.lock_wallpaper.clone())
            {
                return Some(id);
            }
        }
        self.resolved_last_wallpaper(display_key)
    }

    /// Snapshot just the cloned per-display preferences.
    /// Used to expose overrides over the control plane.
    pub fn display_prefs(&self, display_name: &str) -> Option<DisplayPrefs> {
        self.inner
            .read()
            .expect("settings poisoned")
            .displays
            .get(display_name)
            .cloned()
    }

    /// Snapshot every registered display name in the prefs map.
    pub fn display_pref_names(&self) -> Vec<String> {
        self.inner
            .read()
            .expect("settings poisoned")
            .displays
            .keys()
            .cloned()
            .collect()
    }

    /// Apply an in-memory mutation and compare before/after state.
    /// Only changed settings mark the store dirty.
    pub fn update<F>(&self, f: F)
    where
        F: FnOnce(&mut Settings),
    {
        let changed = {
            let mut g = self.inner.write().expect("settings poisoned");
            let before = g.clone();
            f(&mut g);
            *g != before
        };
        if changed {
            self.dirty.store(true, Ordering::SeqCst);
            self.notify.notify_one();
        }
    }

    async fn writer_loop(self: Arc<Self>) {
        loop {
            // Block until something needs to be written.
            self.notify.notified().await;
            // Debounce: keep resetting the timer until DEBOUNCE_WRITE
            // elapses without another update.
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(DEBOUNCE_WRITE) => break,
                    _ = self.notify.notified() => {}
                }
            }
            self.flush().await;
        }
    }

    /// Force a synchronous flush of current settings to disk.
    /// Bypasses the debounce window for shutdown.
    pub async fn flush_now(&self) {
        self.flush().await;
    }

    async fn flush(&self) {
        // Cheap fast path before grabbing the lock: if nothing has
        // changed since the last successful flush, skip entirely.
        if !self.dirty.load(Ordering::SeqCst) {
            return;
        }
        let _g = self.flush_lock.lock().await;
        // Re-check under the lock — another flush may have just
        // raced us to the same state.
        if !self.dirty.swap(false, Ordering::SeqCst) {
            return;
        }

        let snapshot = self.snapshot();
        let serialized = match toml::to_string_pretty(&snapshot) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("settings serialize failed: {e}");
                self.dirty.store(true, Ordering::SeqCst);
                return;
            }
        };

        if let Some(parent) = self.path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                log::warn!("settings create_dir_all {}: {e}", parent.display());
                self.dirty.store(true, Ordering::SeqCst);
                return;
            }
        }

        let tmp = {
            let mut p = self.path.clone();
            let new_name = match p.file_name() {
                Some(n) => {
                    let mut s = n.to_os_string();
                    s.push(".tmp");
                    s
                }
                None => {
                    self.dirty.store(true, Ordering::SeqCst);
                    return;
                }
            };
            p.set_file_name(new_name);
            p
        };
        if let Err(e) = tokio::fs::write(&tmp, serialized).await {
            log::warn!("settings write {}: {e}", tmp.display());
            self.dirty.store(true, Ordering::SeqCst);
            return;
        }
        if let Err(e) = tokio::fs::rename(&tmp, &self.path).await {
            log::warn!(
                "settings rename {} → {}: {e}",
                tmp.display(),
                self.path.display()
            );
            self.dirty.store(true, Ordering::SeqCst);
            return;
        }
        log::debug!("settings flushed to {}", self.path.display());
    }

    /// Read-only view of the on-disk path.
    /// Useful before the rest of `AppState` is constructed.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Bring in-memory plugin tables in line with loaded renderer
    /// manifest schemas.
    pub fn reconcile(&self, registry: &crate::plugin::renderer_registry::RendererRegistry) -> bool {
        use crate::plugin::renderer_registry::{check_setting_bounds, SettingDef, SettingType};

        let mut changed = false;
        let mut g = self.inner.write().expect("settings poisoned");

        // Pre-compute manifest schemas keyed by plugin name so user
        // tables can be checked for unknown plugins.
        let manifests: HashMap<String, &HashMap<String, SettingDef>> = registry
            .all_renderers()
            .into_iter()
            .map(|d| (d.name.clone(), &d.settings))
            .collect();

        // 1) Reconcile each known plugin's table.
        for (plugin_name, schema) in &manifests {
            if schema.is_empty() {
                continue;
            }
            let entry = g.plugins.entry(plugin_name.clone()).or_default();

            // Drop keys that aren't in the manifest anymore.
            let stale: Vec<String> = entry
                .keys()
                .filter(|k| !schema.contains_key(*k))
                .cloned()
                .collect();
            for k in stale {
                log::warn!(
                    "settings: dropping unknown key '{plugin_name}.{k}' \
                     (no longer in manifest schema)"
                );
                entry.remove(&k);
                changed = true;
            }

            // Fill in / reset bad values for declared keys.
            for (key, def) in schema.iter() {
                let needs_default = match entry.get(key) {
                    None => true,
                    Some(v) => match check_setting_bounds(key, v, def) {
                        Ok(()) => false,
                        Err(e) => {
                            log::warn!(
                                "settings: '{plugin_name}.{key}' = {v:?} \
                                 violates schema ({e}); resetting to default"
                            );
                            true
                        }
                    },
                };
                if needs_default {
                    let default = match def.ty {
                        SettingType::U32 => match &def.default {
                            toml::Value::Integer(i) if *i >= 0 => i.to_string(),
                            toml::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        },
                        SettingType::I32 => match &def.default {
                            toml::Value::Integer(i) => i.to_string(),
                            toml::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        },
                        SettingType::F32 => match &def.default {
                            toml::Value::Float(f) => f.to_string(),
                            toml::Value::Integer(i) => (*i as f32).to_string(),
                            toml::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        },
                        SettingType::Bool => match &def.default {
                            toml::Value::Boolean(b) => b.to_string(),
                            toml::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        },
                        SettingType::String => match &def.default {
                            toml::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        },
                    };
                    if entry.get(key) != Some(&default) {
                        entry.insert(key.clone(), default);
                        changed = true;
                    }
                }
            }
        }

        // Warn about persisted plugin settings whose manifest is absent.
        // Keep them in memory so missing plugins do not lose settings.
        for plugin_name in g.plugins.keys() {
            if !manifests.contains_key(plugin_name) {
                log::warn!(
                    "settings: plugin '{plugin_name}' has persisted values \
                     but no matching renderer manifest is loaded; \
                     leaving as-is"
                );
            }
        }

        if changed {
            self.dirty.store(true, Ordering::SeqCst);
            self.notify.notify_one();
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_roundtrip() {
        let s: Settings = toml::from_str("").unwrap();
        assert!(s.global.last_wallpaper.is_none());
        assert_eq!(s.global.audio_fade_ms, DEFAULT_AUDIO_FADE_MS);
        assert!(!s.global.manual_muted);
        assert!(s.global.plugin_update_notifications);
        assert!(!s.global.duplicate_renderers_for_same_wallpaper);
        assert!(!s.global.autostart_enabled);
        assert!(s.plugins.is_empty());
    }

    #[test]
    fn autostart_setting_roundtrip() {
        let src = r#"
[global]
autostart_enabled = true
"#;
        let s: Settings = toml::from_str(src).unwrap();
        assert!(s.global.autostart_enabled);
        assert!(toml::to_string(&s)
            .unwrap()
            .contains("autostart_enabled = true"));
    }

    #[test]
    fn duplicate_renderers_setting_roundtrip() {
        let src = r#"
[global]
duplicate_renderers_for_same_wallpaper = true
"#;
        let s: Settings = toml::from_str(src).unwrap();
        assert!(s.global.duplicate_renderers_for_same_wallpaper);
        assert!(toml::to_string(&s)
            .unwrap()
            .contains("duplicate_renderers_for_same_wallpaper = true"));
    }

    #[test]
    fn manual_muted_roundtrip() {
        let src = r#"
[global]
manual_muted = true
"#;
        let s: Settings = toml::from_str(src).unwrap();
        assert!(s.global.manual_muted);
        assert!(toml::to_string(&s).unwrap().contains("manual_muted = true"));
    }

    #[test]
    fn audio_fade_ms_is_clamped_for_runtime() {
        let mut g = GlobalSettings::default();
        g.audio_fade_ms = MAX_AUDIO_FADE_MS + 1;
        assert_eq!(g.effective_audio_fade_ms(), MAX_AUDIO_FADE_MS);
    }

    #[test]
    fn layout_defaults_roundtrip() {
        let src = r#"
[global.layout]
fillmode = "preserve_aspect_crop"
align = "top_right"
location = { x = 25, y = 75 }
"#;
        let s: Settings = toml::from_str(src).unwrap();
        assert_eq!(s.global.layout.fillmode, FillMode::PreserveAspectCrop);
        assert_eq!(s.global.layout.align, Align::TopRight);
        assert_eq!(s.global.layout.location, Some(Location::new(25, 75)));
    }

    #[test]
    fn display_override_parses_and_resolves() {
        let src = r#"
[global.layout]
fillmode = "stretched"
align = "center"

[display.HDMI-A-1]
fillmode = "preserve_aspect_fit"
"#;
        let s: Settings = toml::from_str(src).unwrap();
        let prefs = s.displays.get("HDMI-A-1").unwrap();
        assert_eq!(prefs.fillmode, Some(FillMode::PreserveAspectFit));
        assert_eq!(prefs.location, None); // inherits
        assert_eq!(prefs.align, None); // inherits
    }

    #[tokio::test]
    async fn resolved_layout_falls_back_field_by_field() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let store = SettingsStore::load_or_default(path).await;

        // No per-display entry => pure global defaults.
        let r = store.resolved_layout("eDP-1");
        assert_eq!(r.fillmode, FillMode::default());
        assert_eq!(r.location, Location::from_align(Align::default()));

        // Set a partial override for "eDP-1" (only fillmode).
        store.update(|s| {
            s.global.layout.location = Some(Location::new(20, 80));
            s.displays.insert(
                "eDP-1".into(),
                DisplayPrefs {
                    fillmode: Some(FillMode::PreserveAspectCrop),
                    ..Default::default()
                },
            );
        });

        let r = store.resolved_layout("eDP-1");
        assert_eq!(r.fillmode, FillMode::PreserveAspectCrop); // override
        assert_eq!(r.location, Location::new(20, 80)); // global
    }

    #[test]
    fn display_prefs_is_empty_tracks_last_wallpaper() {
        let mut p = DisplayPrefs::default();
        assert!(p.is_empty());
        p.last_wallpaper = Some("wp-1".into());
        assert!(!p.is_empty());
        p.last_wallpaper = None;
        assert!(p.is_empty());
    }

    #[test]
    fn auto_replay_default_actions() {
        let policy = AutoReplayPolicy::default();
        assert_eq!(policy.fullscreen, AutoAction::Pause);
        assert_eq!(policy.session_locked, AutoAction::Stop);
        assert_eq!(policy.session_inactive, AutoAction::Stop);
    }

    #[tokio::test]
    async fn resolved_last_wallpaper_prefers_per_display_then_global() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let store = SettingsStore::load_or_default(path).await;

        // Neither global nor per-display set => None.
        assert_eq!(store.resolved_last_wallpaper("HDMI-A-1"), None);

        // Only global set => returned for any key.
        store.update(|s| s.global.last_wallpaper = Some("wp-global".into()));
        assert_eq!(
            store.resolved_last_wallpaper("HDMI-A-1").as_deref(),
            Some("wp-global"),
        );

        // Per-display override wins; other displays keep falling back.
        store.update(|s| {
            s.displays.insert(
                "HDMI-A-1".into(),
                DisplayPrefs {
                    last_wallpaper: Some("wp-a".into()),
                    ..Default::default()
                },
            );
        });
        assert_eq!(
            store.resolved_last_wallpaper("HDMI-A-1").as_deref(),
            Some("wp-a"),
        );
        assert_eq!(
            store.resolved_last_wallpaper("DP-2").as_deref(),
            Some("wp-global"),
        );
    }

    #[tokio::test]
    async fn resolved_lock_wallpaper_falls_back_to_desktop() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SettingsStore::load_or_default(tmp.path().join("config.toml")).await;

        store.update(|s| {
            s.global.last_wallpaper = Some("desktop".into());
        });
        assert_eq!(
            store.resolved_lock_wallpaper("HDMI-A-1").as_deref(),
            Some("desktop"),
        );

        store.update(|s| s.global.lock_wallpaper = Some("lock-global".into()));
        assert_eq!(
            store.resolved_lock_wallpaper("HDMI-A-1").as_deref(),
            Some("lock-global"),
        );

        store.update(|s| {
            s.displays.insert(
                "HDMI-A-1".into(),
                DisplayPrefs {
                    lock_wallpaper: Some("lock-a".into()),
                    ..Default::default()
                },
            );
        });
        assert_eq!(
            store.resolved_lock_wallpaper("HDMI-A-1").as_deref(),
            Some("lock-a"),
        );
        assert_eq!(
            store.resolved_lock_wallpaper("DP-2").as_deref(),
            Some("lock-global"),
        );
    }

    #[test]
    fn plugin_section_preserved() {
        let src = r#"
[plugin.wescene]
foo = "bar"
baz = "7"
"#;
        let s: Settings = toml::from_str(src).unwrap();
        let wescene = s.plugins.get("wescene").expect("wescene section");
        assert_eq!(wescene.get("foo").map(String::as_str), Some("bar"));
        assert_eq!(wescene.get("baz").map(String::as_str), Some("7"));
    }

    #[tokio::test]
    async fn debounced_write_roundtrips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let store = SettingsStore::load_or_default(path.clone()).await;
        assert_eq!(store.global().rotation_secs, 0);

        store.update(|s| s.global.rotation_secs = 30);
        // Wait past the debounce window.
        tokio::time::sleep(DEBOUNCE_WRITE + Duration::from_millis(500)).await;

        let written = tokio::fs::read_to_string(&path).await.unwrap();
        let parsed: Settings = toml::from_str(&written).unwrap();
        assert_eq!(parsed.global.rotation_secs, 30);
    }

    // --- reconcile() tests --------------------------------------------

    use crate::plugin::renderer_registry::{
        RendererDef, RendererRegistry, SettingDef, SettingType,
    };
    use std::path::PathBuf;

    fn schema_setting(ty: SettingType, default: toml::Value, identity: bool) -> SettingDef {
        SettingDef {
            ty,
            default,
            identity,
            label_key: None,
            description_key: None,
            min: None,
            max: None,
            step: None,
            choices: None,
            group: None,
            order: None,
        }
    }

    fn registry_with_video() -> RendererRegistry {
        let mut r = RendererRegistry::new();
        let mut s: HashMap<String, SettingDef> = HashMap::new();
        s.insert(
            "loop_file".into(),
            schema_setting(
                SettingType::String,
                toml::Value::String("inf".into()),
                false,
            ),
        );
        s.insert(
            "volume".into(),
            SettingDef {
                min: Some(toml::Value::Integer(0)),
                max: Some(toml::Value::Integer(100)),
                ..schema_setting(SettingType::U32, toml::Value::Integer(100), false)
            },
        );
        r.register(RendererDef {
            name: "waywallen-video".into(),
            plugin_id: "test.plugin".to_string(),
            plugin_version: "0.0.0".to_string(),
            plugin_system: false,
            bin: PathBuf::from("/dev/null"),
            types: vec!["video".into()],
            priority: 100,
            spawn_version: Some(1),
            extras: Vec::new(),
            settings: s,
            events: Vec::new(),
        });
        r
    }

    fn make_store_with(plugins: HashMap<String, HashMap<String, String>>) -> Arc<SettingsStore> {
        Arc::new(SettingsStore {
            inner: Arc::new(StdRwLock::new(Settings {
                global: GlobalSettings::default(),
                plugins,
                displays: HashMap::new(),
            })),
            notify: Arc::new(Notify::new()),
            path: PathBuf::from("/dev/null"),
            flush_lock: tokio::sync::Mutex::new(()),
            dirty: AtomicBool::new(false),
        })
    }

    #[test]
    fn reconcile_fills_missing_defaults() {
        let store = make_store_with(HashMap::new());
        let changed = store.reconcile(&registry_with_video());
        assert!(changed, "expected reconcile to fill defaults");
        let snap = store.snapshot();
        let video = snap.plugins.get("waywallen-video").expect("video table");
        assert_eq!(video.get("loop_file").map(String::as_str), Some("inf"));
        assert_eq!(video.get("volume").map(String::as_str), Some("100"));
    }

    #[test]
    fn reconcile_drops_unknown_keys() {
        let mut plugins = HashMap::new();
        let mut video = HashMap::new();
        video.insert("loop_file".into(), "inf".into());
        video.insert("volume".into(), "50".into());
        video.insert("ghost".into(), "should-disappear".into());
        plugins.insert("waywallen-video".into(), video);

        let store = make_store_with(plugins);
        let changed = store.reconcile(&registry_with_video());
        assert!(changed);
        let snap = store.snapshot();
        let video = snap.plugins.get("waywallen-video").unwrap();
        assert!(!video.contains_key("ghost"), "unknown key must be dropped");
        assert_eq!(video.get("volume").map(String::as_str), Some("50"));
    }

    #[test]
    fn reconcile_resets_out_of_range_to_default() {
        let mut plugins = HashMap::new();
        let mut video = HashMap::new();
        video.insert("loop_file".into(), "inf".into());
        video.insert("volume".into(), "999".into());
        plugins.insert("waywallen-video".into(), video);

        let store = make_store_with(plugins);
        let changed = store.reconcile(&registry_with_video());
        assert!(changed);
        let snap = store.snapshot();
        let video = snap.plugins.get("waywallen-video").unwrap();
        assert_eq!(video.get("volume").map(String::as_str), Some("100"));
    }

    #[test]
    fn reconcile_no_change_returns_false() {
        let mut plugins = HashMap::new();
        let mut video = HashMap::new();
        video.insert("loop_file".into(), "inf".into());
        video.insert("volume".into(), "100".into());
        plugins.insert("waywallen-video".into(), video);

        let store = make_store_with(plugins);
        let changed = store.reconcile(&registry_with_video());
        assert!(!changed, "all keys present and valid → no change");
    }

    #[test]
    fn reconcile_keeps_unknown_plugin_section() {
        // A plugin we don't know about should stay untouched (might
        // be a renamed/missing manifest the user'll re-add).
        let mut plugins = HashMap::new();
        let mut wescene = HashMap::new();
        wescene.insert("foo".into(), "bar".into());
        plugins.insert("waywallen-wescene".into(), wescene);

        let store = make_store_with(plugins);
        store.reconcile(&registry_with_video());
        let snap = store.snapshot();
        assert!(snap.plugins.contains_key("waywallen-wescene"));
        assert_eq!(
            snap.plugins
                .get("waywallen-wescene")
                .and_then(|m| m.get("foo"))
                .map(String::as_str),
            Some("bar")
        );
    }
}
