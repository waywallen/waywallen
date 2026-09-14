use super::*;

fn is_false(value: &bool) -> bool {
    !*value
}

fn default_true() -> bool {
    true
}

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
    pub alias: Option<String>,
    pub active_playlist_id: Option<i64>,
    /// Prevent this display from inheriting the global auto-attach playlist.
    #[serde(skip_serializing_if = "is_false")]
    pub playlist_auto_attach_disabled: bool,
}

impl DisplayPrefs {
    pub fn is_empty(&self) -> bool {
        self.fillmode.is_none()
            && self.location.is_none()
            && self.align.is_none()
            && self.rotation.is_none()
            && self.auto_replay.is_none()
            && self.last_wallpaper.is_none()
            && self.alias.is_none()
            && self.active_playlist_id.is_none()
            && !self.playlist_auto_attach_disabled
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CanvasPrefs {
    pub name: String,
    pub members: HashMap<String, CanvasMemberPrefs>,
    pub last_wallpaper: Option<String>,
    pub layout: Option<CanvasLayoutPrefs>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanvasMemberPrefs {
    pub rect: CanvasRect,
    #[serde(default = "default_true")]
    pub aspect_locked: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CanvasLayoutPrefs {
    pub fillmode: Option<FillMode>,
    pub location: Option<Location>,
    pub rotation: Option<Rotation>,
}

impl CanvasLayoutPrefs {
    pub fn is_empty(self) -> bool {
        self.fillmode.is_none() && self.location.is_none() && self.rotation.is_none()
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
    pub resume_delay_ms: u32,
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
            resume_delay_ms: DEFAULT_AUTO_REPLAY_RESUME_DELAY_MS,
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

    pub fn effective_resume_delay_ms(self) -> u32 {
        self.resume_delay_ms.min(MAX_AUTO_REPLAY_RESUME_DELAY_MS)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PauseEffectKind {
    #[default]
    None,
    Blur,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BlurEffectConfig {
    pub radius: u32,
}

impl Default for BlurEffectConfig {
    fn default() -> Self {
        Self {
            radius: DEFAULT_BLUR_EFFECT_RADIUS,
        }
    }
}

impl BlurEffectConfig {
    pub fn effective_radius(self) -> u32 {
        self.radius
            .clamp(MIN_BLUR_EFFECT_RADIUS, MAX_BLUR_EFFECT_RADIUS)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PauseEffectConfig {
    pub kind: PauseEffectKind,
    pub blur: BlurEffectConfig,
}

impl PauseEffectConfig {
    pub fn effective(self) -> Self {
        Self {
            kind: self.kind,
            blur: BlurEffectConfig {
                radius: self.blur.effective_radius(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    #[default]
    None,
    Fade,
    Wipe,
    Grow,
}

/// Grow transition center as percentages of the display surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TransitionOrigin {
    pub x: u32,
    pub y: u32,
}

impl Default for TransitionOrigin {
    fn default() -> Self {
        Self {
            x: DEFAULT_TRANSITION_ORIGIN_PERCENT,
            y: DEFAULT_TRANSITION_ORIGIN_PERCENT,
        }
    }
}

/// Animation used when a display switches to different wallpaper content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TransitionConfig {
    pub kind: TransitionKind,
    pub duration_ms: u32,
    /// Wipe direction in degrees, clockwise; 0 wipes left to right.
    pub angle: u32,
    pub origin: TransitionOrigin,
}

impl Default for TransitionConfig {
    fn default() -> Self {
        Self {
            kind: TransitionKind::None,
            duration_ms: DEFAULT_TRANSITION_DURATION_MS,
            angle: 0,
            origin: TransitionOrigin::default(),
        }
    }
}

impl TransitionConfig {
    pub fn effective(self) -> Self {
        Self {
            kind: self.kind,
            duration_ms: self
                .duration_ms
                .clamp(MIN_TRANSITION_DURATION_MS, MAX_TRANSITION_DURATION_MS),
            angle: self.angle % 360,
            origin: TransitionOrigin {
                x: self.origin.x.min(100),
                y: self.origin.y.min(100),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalRendererSettings {
    pub enable_audio: bool,
    pub volume: u32,
}

impl Default for GlobalRendererSettings {
    fn default() -> Self {
        Self {
            enable_audio: true,
            volume: MAX_RENDERER_VOLUME,
        }
    }
}

impl GlobalRendererSettings {
    pub fn effective_volume(&self) -> u32 {
        self.volume.min(MAX_RENDERER_VOLUME)
    }
}

/// Daemon-wide settings shared by control and rendering flows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalSettings {
    pub last_wallpaper: Option<String>,
    /// Queue playback mode: `"sequential"` / `"shuffle"` / `"random"`.
    /// Restored on startup so the rotator resumes the same behavior.
    #[serde(alias = "playlist_mode")]
    pub queue_mode: String,
    /// Auto-rotation interval in seconds; `0` = disabled.
    pub rotation_secs: u32,
    /// Fade duration shared by mute and unmute control messages.
    pub audio_fade_ms: u32,
    /// Mute renderer audio while another PulseAudio playback stream is active.
    pub mute_when_other_audio: bool,
    /// Allow audio-response wallpapers to capture the system output spectrum.
    pub audio_capture_enabled: bool,
    pub renderer: GlobalRendererSettings,
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
    pub pause_effect: PauseEffectConfig,
    pub transition: TransitionConfig,
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

    /// Forward pointer input received by the daemon to subscribed renderers.
    pub pointer_forwarding_enabled: bool,

    /// Last autostart state successfully accepted by the Flatpak portal.
    pub autostart_enabled: bool,

    /// Hide the StatusNotifierItem tray icon. Applied live; the
    /// `--no-tray` CLI flag forces the tray off regardless.
    pub hide_tray_icon: bool,

    pub debug_logging_enabled: bool,
}

impl Default for GlobalSettings {
    fn default() -> Self {
        Self {
            last_wallpaper: None,
            queue_mode: "sequential".to_string(),
            rotation_secs: 0,
            audio_fade_ms: DEFAULT_AUDIO_FADE_MS,
            mute_when_other_audio: false,
            audio_capture_enabled: true,
            renderer: GlobalRendererSettings::default(),
            manual_muted: false,
            layout: LayoutDefaults::default(),
            auto_replay: None,
            pause_effect: PauseEffectConfig::default(),
            transition: TransitionConfig::default(),
            wallpaper_filter: WallpaperFilterState::default(),
            wallpaper_sorts: Vec::new(),
            wallpaper_skip_types: Vec::new(),
            wallpaper_filter_tags: Vec::new(),
            wallpaper_skip_content_ratings: Vec::new(),
            auto_attach_playlist_id: None,
            plugin_update_notifications: true,
            duplicate_renderers_for_same_wallpaper: false,
            pointer_forwarding_enabled: true,
            autostart_enabled: false,
            hide_tray_icon: false,
            debug_logging_enabled: false,
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
        Vec<crate::catalog::FilterRule>,
        Vec<crate::catalog::FilterLogic>,
    ) {
        use crate::catalog::query::{FilterPredicate, FilterRule, StringMatch};
        let (mut filters, logics) = self.wallpaper_filter.to_catalog();
        let mut next_group = filters
            .iter()
            .map(|f| f.group)
            .max()
            .map(|g| g + 1)
            .unwrap_or(0);
        for ty in &self.wallpaper_skip_types {
            filters.push(FilterRule {
                group: next_group,
                predicate: FilterPredicate::WallpaperType {
                    value: ty.clone(),
                    condition: StringMatch::IsNot,
                },
            });
            next_group += 1;
        }
        if !self.wallpaper_filter_tags.is_empty() {
            filters.push(FilterRule {
                group: next_group,
                predicate: FilterPredicate::Tags {
                    values: self.wallpaper_filter_tags.clone(),
                    condition: StringMatch::Is,
                },
            });
            next_group += 1;
        }
        for rating in &self.wallpaper_skip_content_ratings {
            filters.push(FilterRule {
                group: next_group,
                predicate: FilterPredicate::ContentRating {
                    value: rating.clone(),
                    condition: StringMatch::IsNot,
                },
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
    pub fn vec_to_catalog(v: &[WallpaperSortRuleState]) -> Vec<crate::catalog::SortRule> {
        use crate::catalog::query::{SortDirection, SortKey, SortRule};
        v.iter()
            .filter_map(|r| {
                Some(SortRule {
                    key: match r.key {
                        1 => SortKey::Name,
                        2 => SortKey::WallpaperType,
                        3 => SortKey::Size,
                        4 => SortKey::LastModified,
                        _ => return None,
                    },
                    direction: if r.direction == 2 {
                        SortDirection::Descending
                    } else {
                        SortDirection::Ascending
                    },
                })
            })
            .collect()
    }

    pub fn vec_from_catalog(v: &[crate::catalog::SortRule]) -> Vec<WallpaperSortRuleState> {
        use crate::catalog::query::{SortDirection, SortKey};
        v.iter()
            .map(|r| WallpaperSortRuleState {
                key: match r.key {
                    SortKey::Name => 1,
                    SortKey::WallpaperType => 2,
                    SortKey::Size => 3,
                    SortKey::LastModified => 4,
                },
                direction: match r.direction {
                    SortDirection::Ascending => 1,
                    SortDirection::Descending => 2,
                },
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
    pub fn to_catalog(
        &self,
    ) -> (
        Vec<crate::catalog::FilterRule>,
        Vec<crate::catalog::FilterLogic>,
    ) {
        use crate::catalog::query::{
            FilterLogic, FilterPredicate, FilterRule, IntMatch, LogicOperator, StringMatch,
        };
        let rules = self
            .filters
            .iter()
            .filter_map(|rule| {
                let string = || {
                    let filter = rule.string_filter.as_ref()?;
                    Some((
                        filter.value.clone(),
                        StringMatch::from_code(filter.condition)?,
                    ))
                };
                let int = || {
                    let filter = rule.int_filter.as_ref()?;
                    Some((filter.value, IntMatch::from_code(filter.condition)?))
                };
                let predicate = match rule.r#type {
                    1 => {
                        let (value, condition) = string()?;
                        FilterPredicate::Name { value, condition }
                    }
                    2 => {
                        let (value, condition) = string()?;
                        FilterPredicate::WallpaperType { value, condition }
                    }
                    3 => {
                        let (value, condition) = string()?;
                        FilterPredicate::Library { value, condition }
                    }
                    5 => {
                        let (value, condition) = int()?;
                        FilterPredicate::Width { value, condition }
                    }
                    6 => {
                        let (value, condition) = int()?;
                        FilterPredicate::Height { value, condition }
                    }
                    7 => {
                        let (value, condition) = int()?;
                        FilterPredicate::Size { value, condition }
                    }
                    8 => {
                        let (value, condition) = string()?;
                        FilterPredicate::ContentRating { value, condition }
                    }
                    9 => {
                        if let Some(filter) = &rule.tag_filter {
                            FilterPredicate::Tags {
                                values: filter.values.clone(),
                                condition: StringMatch::from_code(filter.condition)?,
                            }
                        } else {
                            let (value, condition) = string()?;
                            FilterPredicate::Tags {
                                values: vec![value],
                                condition,
                            }
                        }
                    }
                    _ => return None,
                };
                Some(FilterRule {
                    group: rule.group,
                    predicate,
                })
            })
            .collect();
        let logics = self
            .filter_logics
            .iter()
            .map(|logic| FilterLogic {
                operator: LogicOperator::from_code(logic.op),
                group_a: logic.group_a,
                group_b: logic.group_b,
            })
            .collect();
        (rules, logics)
    }

    pub fn from_catalog(
        rules: &[crate::catalog::FilterRule],
        logics: &[crate::catalog::FilterLogic],
    ) -> Self {
        use crate::catalog::query::{FilterPredicate, IntMatch, LogicOperator, StringMatch};
        let string_code = |condition| match condition {
            StringMatch::Is => 1,
            StringMatch::IsNot => 2,
            StringMatch::Contains => 3,
            StringMatch::ContainsNot => 4,
        };
        let int_code = |condition| match condition {
            IntMatch::Equal => 1,
            IntMatch::NotEqual => 2,
            IntMatch::Less => 3,
            IntMatch::LessEqual => 4,
            IntMatch::Greater => 5,
            IntMatch::GreaterEqual => 6,
        };
        Self {
            filters: rules
                .iter()
                .map(|rule| {
                    let mut state = WallpaperFilterRuleState {
                        group: rule.group,
                        ..Default::default()
                    };
                    match &rule.predicate {
                        FilterPredicate::Name { value, condition } => {
                            state.r#type = 1;
                            state.string_filter = Some(WallpaperStringFilterState {
                                value: value.clone(),
                                condition: string_code(*condition),
                            });
                        }
                        FilterPredicate::WallpaperType { value, condition } => {
                            state.r#type = 2;
                            state.string_filter = Some(WallpaperStringFilterState {
                                value: value.clone(),
                                condition: string_code(*condition),
                            });
                        }
                        FilterPredicate::Library { value, condition } => {
                            state.r#type = 3;
                            state.string_filter = Some(WallpaperStringFilterState {
                                value: value.clone(),
                                condition: string_code(*condition),
                            });
                        }
                        FilterPredicate::Width { value, condition } => {
                            state.r#type = 5;
                            state.int_filter = Some(WallpaperIntFilterState {
                                value: *value,
                                condition: int_code(*condition),
                            });
                        }
                        FilterPredicate::Height { value, condition } => {
                            state.r#type = 6;
                            state.int_filter = Some(WallpaperIntFilterState {
                                value: *value,
                                condition: int_code(*condition),
                            });
                        }
                        FilterPredicate::Size { value, condition } => {
                            state.r#type = 7;
                            state.int_filter = Some(WallpaperIntFilterState {
                                value: *value,
                                condition: int_code(*condition),
                            });
                        }
                        FilterPredicate::ContentRating { value, condition } => {
                            state.r#type = 8;
                            state.string_filter = Some(WallpaperStringFilterState {
                                value: value.clone(),
                                condition: string_code(*condition),
                            });
                        }
                        FilterPredicate::Tags { values, condition } => {
                            state.r#type = 9;
                            state.tag_filter = Some(WallpaperTagFilterState {
                                values: values.clone(),
                                condition: string_code(*condition),
                            });
                        }
                    }
                    state
                })
                .collect(),
            filter_logics: logics
                .iter()
                .map(|logic| FilterLogicState {
                    op: match logic.operator {
                        LogicOperator::And => 1,
                        LogicOperator::Or => 2,
                    },
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
    /// Per-component string-to-string bag keyed by renderer or Lua source name.
    /// String values map cleanly to TOML and protobuf.
    #[serde(default, rename = "plugin")]
    pub plugins: HashMap<String, HashMap<String, String>>,
    /// Per-display layout overrides keyed by `register_display` name.
    /// Empty entries are pruned by mutators.
    #[serde(default, rename = "display")]
    pub displays: HashMap<String, DisplayPrefs>,
    #[serde(default, rename = "canvas")]
    pub canvases: HashMap<String, CanvasPrefs>,
}

impl Settings {
    pub fn resolved_renderer_settings(
        &self,
        renderer: &crate::plugin::renderer_registry::RendererDef,
    ) -> HashMap<String, String> {
        use crate::plugin::renderer_registry::setting_default_value;

        let mut values = self
            .plugins
            .get(&renderer.name)
            .cloned()
            .unwrap_or_default();
        for (key, setting) in &renderer.settings {
            if !values.contains_key(key) {
                values.insert(key.clone(), setting_default_value(setting));
            }
        }
        self.apply_global_renderer_settings(renderer, &mut values);
        values
    }

    pub fn apply_global_renderer_settings(
        &self,
        renderer: &crate::plugin::renderer_registry::RendererDef,
        values: &mut HashMap<String, String>,
    ) {
        use crate::plugin::renderer_registry::{
            coerce_and_validate, setting_default_value, SettingType,
        };

        if !self.global.renderer.enable_audio
            && renderer.settings.contains_key(RENDERER_ENABLE_AUDIO_KEY)
        {
            values.insert(RENDERER_ENABLE_AUDIO_KEY.to_string(), "false".to_string());
        }
        let Some(volume_setting) = renderer.settings.get(RENDERER_VOLUME_KEY) else {
            return;
        };
        if volume_setting.ty != SettingType::U32 {
            return;
        }
        let base_volume = values
            .get(RENDERER_VOLUME_KEY)
            .and_then(|value| coerce_and_validate(RENDERER_VOLUME_KEY, value, volume_setting).ok())
            .and_then(|value| value.parse::<u32>().ok())
            .or_else(|| setting_default_value(volume_setting).parse::<u32>().ok())
            .unwrap_or(MAX_RENDERER_VOLUME);
        let volume =
            ((u64::from(base_volume) * u64::from(self.global.renderer.effective_volume())) + 50)
                / 100;
        values.insert(RENDERER_VOLUME_KEY.to_string(), volume.to_string());
    }
}
