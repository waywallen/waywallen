use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, broadcast::error::RecvError, mpsc, Mutex as TokioMutex, Notify};
use tokio::task::JoinHandle;

/// Grace period an orphan renderer keeps running before it is killed.
/// Only granted to the last renderer while the daemon has zero displays.
const ORPHAN_REAP_TIMEOUT: Duration = Duration::from_secs(5);
const RESUME_RETRY_INITIAL: Duration = Duration::from_millis(100);
const RESUME_RETRY_SECOND: Duration = Duration::from_secs(2);
const RESUME_RETRY_THIRD: Duration = Duration::from_secs(5);
const RESUME_RETRY_MAX: Duration = Duration::from_secs(10);
const RUNTIME_WAITING_SOFT: Duration = Duration::from_secs(2);
const RUNTIME_PROGRESS_HARD: Duration = Duration::from_secs(10);
const RUNTIME_HEALTH_POLL: Duration = Duration::from_millis(500);
const PROCESS_RESTART_MAX_FAILURES: u32 = 5;

use crate::catalog::properties::WallpaperLayoutOverride;
use crate::plugin::renderer_registry::RendererActivityMode;
use crate::settings::{
    AutoAction, AutoReplayPolicy, PauseEffectConfig as StoredPauseEffectConfig, PauseEffectKind,
    ResolvedLayout, SettingsStore,
};
use crate::wallframe::display::layout::{FillMode, LayoutInput};
use crate::wallframe::ipc::proto::{
    ControlMsg, ControlTransition, EventMsg, RENDERER_STATE_FIELD_CLEAR_COLOR,
    RENDERER_STATE_FIELD_RUNTIME_TAGS,
};
use crate::wallframe::renderer_manager::{
    DrmNode, PublishedPool, RendererHandle, RendererId, RendererManager, RendererRuntimeTag,
};
use crate::wallframe::scheduler::{CompositionConfig, DisplayId, DisplayInfo, DisplayMetrics};

use super::auto_replay;
use super::table::{Link, LinkDstRect, LinkId, LinkSrcRect, RoutingTable};

mod composition;
mod deadline;
mod display_sync;
mod lifecycle;
mod slot;
mod snapshot;

pub use composition::{ActiveRenderer, ApplyAssignment, ApplyReceipt, AssignmentActivation};
use slot::{
    PendingRendererStart, RendererLifecycleEvent, RendererSlot, RendererStartCause,
    RendererTransition,
};
pub use slot::{RendererActivity, RendererExitSnapshot, RendererLifecycleState};
use snapshot::project_link;

/// Wire-translated event streamed from router to a display endpoint.
/// The endpoint owns translation to the on-the-wire `Event`.
pub enum DisplayOutEvent {
    /// Bind this exact immutable pool using the display-local generation.
    Bind {
        renderer: Arc<RendererHandle>,
        pool: Arc<PublishedPool>,
        buffer_generation: u64,
        initial_config: CompositionConfig,
    },
    /// Retire the named buffer pool generation.
    Unbind { buffer_generation: u64 },
    /// Update composition geometry / clear color.
    SetCompositionConfig(CompositionConfig),
    /// Replace persistent presentation config and its current dynamic result.
    SetPresentationSnapshot(PresentationSnapshot),
    /// Update high-frequency presentation state for the current config.
    SetPresentationState(PresentationState),
    /// A frame is ready on `renderer` at `buffer_index` for the named
    /// generation. The endpoint pulls the matching sync_fd from the handle.
    Frame {
        renderer: Arc<RendererHandle>,
        buffer_generation: u64,
        buffer_index: u32,
        seq: u64,
        consumption: DisplayConsumptionPermit,
        member: Option<crate::wallframe::sync::FrameConsumerMember>,
    },
}

pub const PRESENTATION_CAP_PAUSE_BLUR: u32 = 1 << 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumerImportFailureKind {
    Unsupported,
    ResourceExhausted,
    BackendFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumerImportFailureOutcome {
    Retry { fourcc: u32, modifier: u64 },
    Stale,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlurEffectConfig {
    pub radius: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PauseEffectConfig {
    pub kind: PauseEffectKind,
    pub blur: BlurEffectConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PauseEffectState {
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationConfig {
    pub generation: u64,
    pub pause_effect: PauseEffectConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationState {
    pub generation: u64,
    pub config_generation: u64,
    pub pause_effect: PauseEffectState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationSnapshot {
    pub config: PresentationConfig,
    pub state: PresentationState,
}

#[derive(Clone)]
pub struct DisplayConsumptionPermit {
    current: Arc<AtomicU64>,
    epoch: u64,
}

impl DisplayConsumptionPermit {
    pub fn is_current(&self) -> bool {
        self.current.load(Ordering::Acquire) == self.epoch
    }
}

enum AutoStateAction {
    Reconcile,
    Noop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResumeControl {
    Play { fade_ms: u32 },
    Unmute { fade_ms: u32 },
}

impl ResumeControl {
    fn from_message(message: &ControlMsg) -> Option<Self> {
        match message {
            ControlMsg::Play { transition } => Some(Self::Play {
                fade_ms: transition.fade_ms,
            }),
            ControlMsg::Unmute { transition } => Some(Self::Unmute {
                fade_ms: transition.fade_ms,
            }),
            _ => None,
        }
    }

    fn into_message(self) -> ControlMsg {
        match self {
            Self::Play { fade_ms } => ControlMsg::Play {
                transition: ControlTransition { fade_ms },
            },
            Self::Unmute { fade_ms } => ControlMsg::Unmute {
                transition: ControlTransition { fade_ms },
            },
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Play { .. } => "play",
            Self::Unmute { .. } => "unmute",
        }
    }
}

fn lifecycle_control_label(message: &ControlMsg) -> &'static str {
    match message {
        ControlMsg::Pause { .. } => "pause",
        ControlMsg::Play { .. } => "play",
        ControlMsg::Mute { .. } => "mute",
        ControlMsg::Unmute { .. } => "unmute",
        _ => "control",
    }
}

#[derive(Debug, Clone, Copy)]
struct ResumeRetry {
    control: ResumeControl,
    failures: u32,
    generation: u64,
}

fn resume_retry_delay(failures: u32) -> Duration {
    match failures {
        0 | 1 => RESUME_RETRY_INITIAL,
        2 => RESUME_RETRY_SECOND,
        3 => RESUME_RETRY_THIRD,
        _ => RESUME_RETRY_MAX,
    }
}

/// Initial-registration payload from `display::endpoint::do_handshake`.
pub struct DisplayRegistration {
    pub name: String,
    /// Stable identifier persisted by the consumer (e.g. UUID4 stored in
    /// the shell extension config). Used as the settings key when present.
    pub instance_id: Option<String>,
    pub metrics: DisplayMetrics,
    pub presentation_caps: u32,
    pub consumer_caps: crate::wallframe::dma::negotiate::PeerCaps,
    pub window_state_flags: u32,
}

/// Returned from `register_display` — the assigned id plus the rx end
/// of the dispatcher's per-display channel.
pub struct DisplayHandle {
    pub id: DisplayId,
    pub session_id: crate::wallframe::sync::DisplaySessionId,
    pub presentation: PresentationSnapshot,
    pub rx: mpsc::UnboundedReceiver<DisplayOutEvent>,
}

/// Read-only view of a single (renderer → display) link for UI
/// consumers. Subset of `table::Link` that hides table-internal ids.
#[derive(Debug, Clone)]
pub struct DisplayLinkSnapshot {
    pub renderer_id: RendererId,
    pub z_order: i32,
    pub active: bool,
}

/// Transport-agnostic router event. The WebSocket API subscribes and
/// translates these into wire events.
#[derive(Debug, Clone)]
pub enum RouterEvent {
    /// A single display was added or its fields changed (links, size).
    /// Receivers should upsert by `snap.id`.
    DisplayUpsert(DisplaySnapshot),
    /// A display was unregistered. Receivers should drop the entry.
    DisplayRemoved(DisplayId),
    /// A batch mutation affected many displays — send the whole list
    /// as a single replace instead of N upserts.
    DisplaysReplace(Vec<DisplaySnapshot>),
    /// A renderer was added or its runtime fields changed (status, fps).
    /// Receivers should upsert by `snap.id`.
    RendererUpsert(RendererSnapshot),
    /// A renderer was unregistered. Receivers should drop the entry.
    RendererRemoved(RendererId),
    /// A batch mutation affected many renderers — send the whole list
    /// as a single replace.
    RenderersReplace(Vec<RendererSnapshot>),
    /// A single library was added or its fields changed.
    LibraryUpsert(LibrarySnapshot),
    /// A library was removed.
    LibraryRemoved(i64),
    /// A batch mutation affected many libraries.
    LibrariesReplace(Vec<LibrarySnapshot>),
}

/// Read-only view of a registered library.
#[derive(Debug, Clone)]
pub struct LibrarySnapshot {
    pub id: i64,
    pub path: String,
    pub plugin_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManualLifecycleState {
    pub paused: bool,
    pub muted: bool,
    pub stopped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuntimeConditionKind {
    Loading,
    Waiting,
    Hang,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuntimeConditionOrigin {
    Renderer,
    Display,
    Release,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RuntimeCondition {
    pub kind: RuntimeConditionKind,
    pub origin: RuntimeConditionOrigin,
    pub reason: String,
    pub related_renderer_id: Option<RendererId>,
    pub related_display_id: Option<DisplayId>,
}

/// Read-only view of a registered renderer. Returned from
/// `Router::snapshot_renderers`; mirrors UI-visible renderer fields.
#[derive(Debug, Clone)]
pub struct RendererSnapshot {
    pub id: RendererId,
    pub wp_type: String,
    pub name: String,
    pub state: RendererLifecycleState,
    pub pid: u32,
    pub drm_render_major: u32,
    pub drm_render_minor: u32,
    pub texture_width: u32,
    pub texture_height: u32,
    pub runtime_tags: Vec<RendererRuntimeTag>,
    pub conditions: Vec<RuntimeCondition>,
}

/// Read-only view of a registered display. Returned from
/// `Router::snapshot_displays`; carries metadata from DisplayInfo.
#[derive(Debug, Clone)]
pub struct DisplaySnapshot {
    pub id: DisplayId,
    pub name: String,
    /// Stable per-display key advertised by v4 consumers, used as the
    /// settings store key for layout overrides.
    pub instance_id: Option<String>,
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
    pub links: Vec<DisplayLinkSnapshot>,
    pub drm_render_major: u32,
    pub drm_render_minor: u32,
    pub display_layout: ResolvedLayout,
    pub effective_layout: ResolvedLayout,
    pub effective_layout_source: LayoutSource,
    pub conditions: Vec<RuntimeCondition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutSource {
    Global,
    Display,
    Wallpaper,
}

#[derive(Clone)]
struct DisplayBinding {
    renderer: Arc<RendererHandle>,
    pool: Arc<PublishedPool>,
    wire_generation: u64,
}

struct DisplayState {
    info: DisplayInfo,
    session_id: crate::wallframe::sync::DisplaySessionId,
    /// DRM render-node id of the consumer's GPU. Compared against
    /// `RendererHandle::gpu` during DMA-BUF negotiation.
    gpu: DrmNode,
    tx: mpsc::UnboundedSender<DisplayOutEvent>,
    binding: Option<DisplayBinding>,
    next_wire_buffer_generation: u64,
    consumer_caps: crate::wallframe::dma::negotiate::PeerCaps,
    failed_binding_generation: Option<u64>,
    presentation_caps: u32,
    presentation: PresentationSnapshot,
    accepted: bool,
    /// Per-display auto replay machine driven by display facts and
    /// the resolved rule policy.
    auto_replay: auto_replay::State,
    consumption_epoch: Arc<AtomicU64>,
}

#[derive(Clone)]
struct ReleaseWaitFact {
    consumer: crate::wallframe::sync::FrameConsumerIdentity,
    state: crate::wallframe::sync::ReleaseWaitState,
    since: Instant,
}

impl DisplayState {
    fn consumption_permit(&self) -> DisplayConsumptionPermit {
        DisplayConsumptionPermit {
            current: Arc::clone(&self.consumption_epoch),
            epoch: self.consumption_epoch.load(Ordering::Acquire),
        }
    }

    fn invalidate_consumption(&self) {
        self.consumption_epoch.fetch_add(1, Ordering::AcqRel);
    }
}

fn evaluate_renderer_conditions(
    now: Instant,
    renderer_id: &str,
    activity: RendererActivity,
    has_audience: bool,
    activity_mode: RendererActivityMode,
    progress: crate::wallframe::renderer_manager::RendererProgressSnapshot,
    release_waits: impl Iterator<Item = ReleaseWaitFact>,
    poisoned: Option<&crate::wallframe::sync::FrameConsumerIdentity>,
) -> Vec<RuntimeCondition> {
    let mut conditions = Vec::new();
    if progress.first_frame_at.is_none() {
        conditions.push(RuntimeCondition {
            kind: RuntimeConditionKind::Loading,
            origin: RuntimeConditionOrigin::Renderer,
            reason: "first_frame".to_owned(),
            related_renderer_id: Some(renderer_id.to_owned()),
            related_display_id: None,
        });
    }

    let oldest_wait = release_waits.min_by_key(|wait| wait.since);
    if let Some(wait) = oldest_wait {
        let _state = wait.state;
        let age = now.saturating_duration_since(wait.since);
        if age >= RUNTIME_WAITING_SOFT {
            conditions.push(RuntimeCondition {
                kind: RuntimeConditionKind::Waiting,
                origin: RuntimeConditionOrigin::Release,
                reason: "consumer_release".to_owned(),
                related_renderer_id: Some(renderer_id.to_owned()),
                related_display_id: Some(wait.consumer.display_id),
            });
        }
        if age >= RUNTIME_PROGRESS_HARD {
            conditions.push(RuntimeCondition {
                kind: RuntimeConditionKind::Hang,
                origin: RuntimeConditionOrigin::Release,
                reason: "consumer_release".to_owned(),
                related_renderer_id: Some(renderer_id.to_owned()),
                related_display_id: Some(wait.consumer.display_id),
            });
        }
    }

    if activity_mode == RendererActivityMode::Continuous
        && activity == RendererActivity::Playing
        && has_audience
    {
        let last_progress = progress
            .last_frame_at
            .or(progress.bind_at)
            .unwrap_or(progress.registered_at);
        if now.saturating_duration_since(last_progress) >= RUNTIME_PROGRESS_HARD {
            conditions.push(RuntimeCondition {
                kind: RuntimeConditionKind::Hang,
                origin: RuntimeConditionOrigin::Renderer,
                reason: "frame_progress".to_owned(),
                related_renderer_id: Some(renderer_id.to_owned()),
                related_display_id: None,
            });
        }
    }

    if let Some(consumer) = poisoned {
        conditions.push(RuntimeCondition {
            kind: RuntimeConditionKind::Hang,
            origin: RuntimeConditionOrigin::Release,
            reason: "generation_poisoned".to_owned(),
            related_renderer_id: Some(renderer_id.to_owned()),
            related_display_id: Some(consumer.display_id),
        });
    }
    conditions.sort();
    conditions.dedup();
    conditions
}

struct Inner {
    table: RoutingTable,
    renderer_slots: HashMap<RendererId, RendererSlot>,
    displays: HashMap<DisplayId, DisplayState>,
    disconnected_assignments: HashMap<String, RendererId>,
    renderer_tasks: HashMap<RendererId, JoinHandle<()>>,
    renderer_manual_paused: HashSet<RendererId>,
    resume_retries: HashMap<RendererId, ResumeRetry>,
    resume_retry_tasks: HashMap<RendererId, JoinHandle<()>>,
    next_start_token: u64,
    next_resume_retry_generation: u64,
    /// Set when the screen-saver / lock-screen is active.
    session_locked: bool,
    /// Set when the current login session is inactive.
    session_inactive: bool,
    /// User-requested global pause state. This shares the same
    /// daemon-owned lifecycle path as auto replay.
    manual_paused: bool,
    manual_muted: bool,
    manual_stopped: bool,
    other_playback_active: bool,
    /// Pending orphan-reap timers, keyed by renderer id. Inserted by
    /// `mark_orphan` and cleared by `cancel_orphan_timer`.
    orphan_timers: HashMap<RendererId, JoinHandle<()>>,
    /// Per-renderer set of (display_id, buffer_generation) pairs we've
    /// emitted `Unbind` for and are waiting to be acked.
    unbind_acks_pending: HashMap<RendererId, HashSet<(DisplayId, u64)>>,
    wallpaper_layout_overrides: HashMap<RendererId, WallpaperLayoutOverride>,
    release_waits: HashMap<
        RendererId,
        HashMap<crate::wallframe::sync::FrameConsumerIdentity, ReleaseWaitFact>,
    >,
    generation_poisoned: HashMap<RendererId, crate::wallframe::sync::FrameConsumerIdentity>,
    renderer_conditions: HashMap<RendererId, Vec<RuntimeCondition>>,
    display_conditions: HashMap<DisplayId, Vec<RuntimeCondition>>,
    generation_recoveries: HashSet<RendererId>,
    next_display_id: u64,
    next_display_session_id: crate::wallframe::sync::DisplaySessionId,
    next_config_generation: u64,
}

pub struct Router {
    inner: TokioMutex<Inner>,
    lifecycle_lock: TokioMutex<()>,
    /// Renderer manager used for pause/play lifecycle control.
    mgr: Arc<RendererManager>,
    /// Fan-out channel for `RouterEvent`s. Always present; `send` errors
    /// when there are no subscribers are logged at debug and ignored.
    events_tx: broadcast::Sender<RouterEvent>,
    /// Settings store used to resolve per-display fillmode/align when
    /// computing composition config. Set once at startup.
    settings: std::sync::OnceLock<Arc<SettingsStore>>,
    /// Wakes any task currently inside `await_unbind_acks_for` whenever
    /// `record_ack_unbind` mutates `unbind_acks_pending`.
    unbind_ack_notify: Notify,
    deadlines: deadline::DeadlineScheduler,
}

impl Router {
    pub async fn spawn_renderer(
        self: &Arc<Self>,
        request: crate::wallframe::renderer_manager::SpawnRequest,
    ) -> crate::error::Result<RendererId> {
        self.spawn_unassigned_renderer(request).await
    }

    pub async fn forward_pointer_motion(
        &self,
        renderer_id: &str,
        event: crate::wallframe::ipc::proto::PointerMotion,
    ) -> crate::error::Result<()> {
        self.mgr.send_pointer_motion(renderer_id, event).await
    }

    pub async fn forward_pointer_button(
        &self,
        renderer_id: &str,
        event: crate::wallframe::ipc::proto::PointerButton,
    ) -> crate::error::Result<()> {
        self.mgr.send_pointer_button(renderer_id, event).await
    }

    pub async fn forward_pointer_axis(
        &self,
        renderer_id: &str,
        event: crate::wallframe::ipc::proto::PointerAxis,
    ) -> crate::error::Result<()> {
        self.mgr.send_pointer_axis(renderer_id, event).await
    }

    pub fn new(mgr: Arc<RendererManager>) -> Arc<Self> {
        let (events_tx, _) = broadcast::channel(128);
        let (deadlines, mut deadline_events) = deadline::DeadlineScheduler::start();
        let router = Arc::new(Self {
            inner: TokioMutex::new(Inner {
                table: RoutingTable::new(),
                renderer_slots: HashMap::new(),
                displays: HashMap::new(),
                disconnected_assignments: HashMap::new(),
                renderer_tasks: HashMap::new(),
                renderer_manual_paused: HashSet::new(),
                resume_retries: HashMap::new(),
                resume_retry_tasks: HashMap::new(),
                next_start_token: 0,
                next_resume_retry_generation: 0,
                orphan_timers: HashMap::new(),
                unbind_acks_pending: HashMap::new(),
                wallpaper_layout_overrides: HashMap::new(),
                release_waits: HashMap::new(),
                generation_poisoned: HashMap::new(),
                renderer_conditions: HashMap::new(),
                display_conditions: HashMap::new(),
                generation_recoveries: HashSet::new(),
                next_display_id: 0,
                next_display_session_id: 0,
                next_config_generation: 0,
                session_locked: false,
                session_inactive: false,
                manual_paused: false,
                manual_muted: false,
                manual_stopped: false,
                other_playback_active: false,
            }),
            lifecycle_lock: TokioMutex::new(()),
            mgr,
            events_tx,
            settings: std::sync::OnceLock::new(),
            unbind_ack_notify: Notify::new(),
            deadlines,
        });
        let weak = Arc::downgrade(&router);
        tokio::spawn(async move {
            while let Some(event) = deadline_events.recv().await {
                let Some(router) = weak.upgrade() else {
                    return;
                };
                router.on_deadline_reached(event).await;
            }
        });
        router
    }

    pub fn start_process_exit_listener(
        self: &Arc<Self>,
        mut exits: tokio::sync::mpsc::UnboundedReceiver<
            crate::wallframe::renderer_manager::RendererProcessExit,
        >,
    ) {
        let router = Arc::downgrade(self);
        tokio::spawn(async move {
            while let Some(exit) = exits.recv().await {
                let Some(router) = router.upgrade() else {
                    return;
                };
                router.on_renderer_process_exit(exit).await;
            }
        });
    }

    /// Wire the daemon's `SettingsStore` so `sync_display` can resolve
    /// per-display layout when projecting composition config.
    pub fn attach_settings(self: &Arc<Self>, settings: Arc<SettingsStore>) {
        if self.settings.set(settings).is_err() {
            log::warn!("router: attach_settings called twice; ignoring second call");
        }
    }

    /// Resolve effective layout for a display, defaulting to identity
    /// when settings have not been attached.
    fn resolved_layout(&self, info: &DisplayInfo) -> ResolvedLayout {
        let Some(s) = self.settings.get() else {
            return ResolvedLayout {
                fillmode: FillMode::default(),
                location: Default::default(),
                rotation: Default::default(),
            };
        };
        if let Some(iid) = info.instance_id.as_deref() {
            if s.display_prefs(iid).is_some() {
                return s.resolved_layout(iid);
            }
            // No instance_id-keyed entry yet — fall back to the legacy
            // name-keyed entry so old config keeps working.
        }
        s.resolved_layout(&info.name)
    }

    fn resolved_layout_for_renderer(
        &self,
        info: &DisplayInfo,
        renderer_id: &str,
        inner: &Inner,
    ) -> ResolvedLayout {
        inner
            .wallpaper_layout_overrides
            .get(renderer_id)
            .copied()
            .unwrap_or_default()
            .apply_to(self.resolved_layout(info))
    }

    fn display_layout_source(&self, info: &DisplayInfo) -> LayoutSource {
        let Some(s) = self.settings.get() else {
            return LayoutSource::Global;
        };
        let prefs = if let Some(iid) = info.instance_id.as_deref() {
            s.display_prefs(iid).or_else(|| s.display_prefs(&info.name))
        } else {
            s.display_prefs(&info.name)
        };
        if prefs.as_ref().is_some_and(|p| {
            p.fillmode.is_some()
                || p.location.is_some()
                || p.align.is_some()
                || p.rotation.is_some()
        }) {
            LayoutSource::Display
        } else {
            LayoutSource::Global
        }
    }

    /// Settings TOML key used for this display's persistent prefs.
    /// Prefers stable `instance_id`; falls back to display name.
    fn settings_key_for(info: &DisplayInfo) -> &str {
        info.instance_id.as_deref().unwrap_or(&info.name)
    }

    fn resolved_auto_replay(&self, info: &DisplayInfo) -> AutoReplayPolicy {
        let Some(s) = self.settings.get() else {
            return AutoReplayPolicy::default();
        };
        if let Some(iid) = info.instance_id.as_deref() {
            if s.display_prefs(iid).is_some() {
                return s.resolved_auto_replay(iid);
            }
        }
        s.resolved_auto_replay(&info.name)
    }

    fn resolved_pause_effect(&self, presentation_caps: u32) -> PauseEffectConfig {
        let stored = self
            .settings
            .get()
            .map(|settings| settings.global().pause_effect)
            .unwrap_or_else(StoredPauseEffectConfig::default)
            .effective();
        let kind = if stored.kind == PauseEffectKind::Blur
            && presentation_caps & PRESENTATION_CAP_PAUSE_BLUR != 0
        {
            PauseEffectKind::Blur
        } else {
            PauseEffectKind::None
        };
        PauseEffectConfig {
            kind,
            blur: BlurEffectConfig {
                radius: if kind == PauseEffectKind::Blur {
                    stored.blur.radius
                } else {
                    crate::settings::DEFAULT_BLUR_EFFECT_RADIUS
                },
            },
        }
    }

    fn pause_effect_active(inner: &Inner, display: &DisplayState, kind: PauseEffectKind) -> bool {
        if kind == PauseEffectKind::None {
            return false;
        }
        let Some(binding) = display.binding.as_ref() else {
            return false;
        };
        inner
            .renderer_slots
            .get(&binding.renderer.id)
            .is_some_and(|slot| slot.state.activity() == Some(RendererActivity::Paused))
    }

    fn resolved_audio_fade_ms(&self) -> u32 {
        self.settings
            .get()
            .map(|s| s.global().effective_audio_fade_ms())
            .unwrap_or(crate::settings::DEFAULT_AUDIO_FADE_MS)
    }

    /// Set or clear per-display layout fields. `None` for a field
    /// means "no change"; explicit clear flags unset persisted fields.
    pub async fn set_display_layout(
        self: &Arc<Self>,
        display_id: Option<DisplayId>,
        display_name: String,
        new_fillmode: Option<crate::wallframe::display::layout::FillMode>,
        new_location: Option<crate::wallframe::display::layout::Location>,
        new_align: Option<crate::wallframe::display::layout::Align>,
        new_rotation: Option<crate::wallframe::display::layout::Rotation>,
        clear_fillmode: bool,
        clear_align: bool,
        clear_rotation: bool,
    ) -> Option<DisplayId> {
        let Some(settings) = self.settings.get().cloned() else {
            log::warn!(
                "router: set_display_layout({display_name}) called before settings attached"
            );
            return None;
        };
        let Some((target_id, key)) = self
            .resolve_display_mutation_target(display_id, &display_name, "set_display_layout")
            .await
        else {
            return None;
        };
        settings.update(|s| {
            let entry = s.displays.entry(key.clone()).or_default();
            if clear_fillmode {
                entry.fillmode = None;
            }
            if let Some(v) = new_fillmode {
                entry.fillmode = Some(v);
            }
            if clear_align {
                entry.location = None;
                entry.align = None;
            }
            if let Some(v) = new_location {
                entry.location = Some(v);
                entry.align = None;
            }
            if let Some(v) = new_align {
                if new_location.is_none() {
                    entry.align = Some(v);
                    entry.location = None;
                }
            }
            if clear_rotation {
                entry.rotation = None;
            }
            if let Some(v) = new_rotation {
                entry.rotation = Some(v);
            }
            // Prune empty entry to keep the on-disk file tidy.
            if entry.is_empty() {
                s.displays.remove(&key);
            }
        });
        self.resync_display_composition(target_id).await;
        if let Some(snap) = self.snapshot_display(target_id).await {
            self.emit(RouterEvent::DisplayUpsert(snap));
        }
        Some(target_id)
    }

    pub async fn set_display_alias(
        self: &Arc<Self>,
        display_id: Option<DisplayId>,
        display_name: String,
        new_alias: Option<String>,
        clear: bool,
    ) -> Option<DisplayId> {
        let Some(settings) = self.settings.get().cloned() else {
            log::warn!("router: set_display_alias({display_name}) called before settings attached");
            return None;
        };
        let Some((target_id, key)) = self
            .resolve_display_mutation_target(display_id, &display_name, "set_display_alias")
            .await
        else {
            return None;
        };
        settings.update(|s| {
            let entry = s.displays.entry(key.clone()).or_default();
            if clear {
                entry.alias = None;
            }
            if let Some(v) = new_alias {
                let trimmed = v.trim();
                entry.alias = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                };
            }
            if entry.is_empty() {
                s.displays.remove(&key);
            }
        });
        if let Some(snap) = self.snapshot_display(target_id).await {
            self.emit(RouterEvent::DisplayUpsert(snap));
        }
        Some(target_id)
    }

    /// Re-emit composition config before asking each affected renderer
    /// for one current frame. Each display's outbound channel preserves
    /// this order.
    async fn resync_display_compositions(
        self: &Arc<Self>,
        display_ids: impl IntoIterator<Item = DisplayId>,
    ) {
        let display_ids: HashSet<DisplayId> = display_ids.into_iter().collect();
        let mut renderer_requests: HashMap<String, u64> = HashMap::new();
        let mut inner = self.inner.lock().await;
        for display_id in display_ids {
            if !inner.displays.contains_key(&display_id) {
                continue;
            }
            let display_links = inner.table.links_for_display(display_id);
            let target = display_links.into_iter().find(|l| l.enabled).and_then(|l| {
                let binding = inner.displays.get(&display_id)?.binding.as_ref()?;
                (binding.renderer.id == l.renderer_id).then(|| {
                    (
                        l,
                        Arc::clone(&binding.pool),
                        binding.wire_generation,
                        binding.renderer.id.clone(),
                    )
                })
            });
            let Some((link, pool, buffer_generation, renderer_id)) = target else {
                continue;
            };
            inner.next_config_generation += 1;
            let cfg_gen = inner.next_config_generation;
            let info = inner.displays.get(&display_id).unwrap().info.clone();
            let layout = self.resolved_layout_for_renderer(&info, &link.renderer_id, &inner);
            let cfg = project_link(&link, &pool, &info, cfg_gen, buffer_generation, &layout);
            if let Some(state) = inner.displays.get(&display_id) {
                if state
                    .tx
                    .send(DisplayOutEvent::SetCompositionConfig(cfg))
                    .is_ok()
                {
                    renderer_requests
                        .entry(renderer_id)
                        .and_modify(|generation| *generation = (*generation).max(cfg_gen))
                        .or_insert(cfg_gen);
                }
            }
        }
        drop(inner);

        for (renderer_id, config_generation) in renderer_requests {
            if let Err(error) = self.mgr.request_frame(&renderer_id).await {
                log::warn!(
                    "router: request current frame from renderer {renderer_id} after composition config generation {config_generation}: {error}"
                );
            }
        }
    }

    async fn resync_display_composition(self: &Arc<Self>, display_id: DisplayId) {
        self.resync_display_compositions([display_id]).await;
    }

    async fn resolve_display_mutation_target(
        self: &Arc<Self>,
        display_id: Option<DisplayId>,
        display_name: &str,
        op: &str,
    ) -> Option<(DisplayId, String)> {
        let inner = self.inner.lock().await;
        let Some(display_id) = display_id else {
            log::warn!("router: {op}: missing display_id for {display_name}");
            return None;
        };
        let Some(state) = inner.displays.get(&display_id) else {
            log::warn!("router: {op}: display_id={display_id} not registered");
            return None;
        };
        Some((display_id, Self::settings_key_for(&state.info).to_string()))
    }

    /// Re-emit composition config for every registered display. Called from
    /// the control surface after global layout settings change.
    pub async fn resync_all_compositions(self: &Arc<Self>) {
        let ids: Vec<DisplayId> = {
            let inner = self.inner.lock().await;
            inner.displays.keys().copied().collect()
        };
        self.resync_display_compositions(ids).await;
    }

    /// Push a DisplaysReplace router event after a settings-only
    /// change so subscribed UIs refresh effective layout fields.
    pub fn emit_displays_replace_for_settings_change(self: &Arc<Self>, snap: Vec<DisplaySnapshot>) {
        self.emit(RouterEvent::DisplaysReplace(snap));
    }

    // ---------------------------------------------------------------
    // Renderer lifecycle

    pub async fn register_renderer(self: &Arc<Self>, handle: Arc<RendererHandle>) {
        let _ = self.register_renderer_current(handle, None).await;
    }

    async fn register_renderer_current(
        self: &Arc<Self>,
        handle: Arc<RendererHandle>,
        expected_start: Option<(u64, u64, u64)>,
    ) -> bool {
        let id = handle.id.clone();
        let task = {
            let mut events = handle.events();
            let mut release_events = handle.take_release_events();
            if release_events.is_none() {
                log::warn!("router: renderer {id} release event receiver already taken");
            }
            let router = Arc::clone(self);
            let rid = id.clone();
            tokio::spawn(async move {
                let mut health_tick = tokio::time::interval(RUNTIME_HEALTH_POLL);
                health_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tokio::select! {
                        event = events.recv() => {
                            match event {
                                Ok(event) => match event.message {
                                    EventMsg::BindBuffers { .. } if event.pool_generation.is_some() => {
                                        router.on_renderer_bind(&rid).await;
                                    }
                                    EventMsg::FrameReady { frame } => {
                                        if let Some(buffer_generation) = event.pool_generation {
                                            router
                                                .on_renderer_frame(
                                                    &rid,
                                                    buffer_generation,
                                                    frame.image_index,
                                                    frame.sequence,
                                                    frame.release_point,
                                                )
                                                .await;
                                        }
                                    }
                                    EventMsg::FormatCaps { .. } => {
                                        router.reconcile_buffer_flags().await;
                                    }
                                    EventMsg::BindFailed { failure } => {
                                        router
                                            .on_renderer_bind_failed(
                                                &rid,
                                                failure.format.fourcc,
                                                failure.format.modifier,
                                            )
                                            .await;
                                    }
                                    EventMsg::ReportState { .. } => {
                                        router
                                            .on_renderer_state_changed(
                                                &rid,
                                                event.state_changed_fields,
                                            )
                                            .await;
                                    }
                                    _ => {}
                                },
                                Err(RecvError::Closed) => {
                                    log::info!("router: renderer {rid} broadcast closed");
                                    return;
                                }
                                Err(RecvError::Lagged(n)) => {
                                    log::warn!("router: renderer {rid} lagged {n} events");
                                }
                            }
                            router.refresh_runtime_health().await;
                        }
                        event = async {
                            release_events
                                .as_mut()
                                .expect("release event branch is enabled only with a receiver")
                                .recv()
                                .await
                        }, if release_events.is_some() => {
                            match event {
                                Some(event) => {
                                    let recover = router.record_release_event(event).await;
                                    router.refresh_runtime_health().await;
                                    if recover {
                                        router.recover_poisoned_renderer(&rid).await;
                                    }
                                }
                                None => return,
                            }
                        }
                        _ = health_tick.tick() => {
                            router.refresh_runtime_health().await;
                        }
                    }
                }
            })
        };
        {
            let mut inner = self.inner.lock().await;
            if let Some((spec_revision, process_generation, start_token)) = expected_start {
                let current = inner.renderer_slots.get(&id).is_some_and(|slot| {
                    slot.spec_revision == spec_revision
                        && slot.active_start_token == Some(start_token)
                        && matches!(
                            slot.state,
                            RendererLifecycleState::Starting { generation }
                                if generation == process_generation
                        )
                });
                if !current {
                    task.abort();
                    return false;
                }
            }
            inner
                .renderer_slots
                .entry(id.clone())
                .and_modify(|slot| {
                    let _ = slot.transition(RendererLifecycleEvent::ProcessAttached {
                        generation: handle.process_generation,
                    });
                })
                .or_insert_with(|| RendererSlot::running(&handle));
            inner.table.add_renderer(handle);
            inner.renderer_tasks.insert(id, task);
        }
        self.reconcile_lifecycle().await;
        self.refresh_runtime_health().await;
        true
    }

    pub async fn unregister_renderer(self: &Arc<Self>, id: &str) {
        let affected: Vec<DisplayId> = {
            let mut inner = self.inner.lock().await;
            let removed = inner.table.remove_renderer(id);
            inner.renderer_slots.remove(id);
            inner
                .disconnected_assignments
                .retain(|_, renderer_id| renderer_id != id);
            inner.wallpaper_layout_overrides.remove(id);
            if let Some(task) = inner.renderer_tasks.remove(id) {
                task.abort();
            }
            if let Some(task) = inner.orphan_timers.remove(id) {
                task.abort();
            }
            inner.renderer_manual_paused.remove(id);
            inner.resume_retries.remove(id);
            if let Some(task) = inner.resume_retry_tasks.remove(id) {
                task.abort();
            }
            inner.release_waits.remove(id);
            inner.generation_poisoned.remove(id);
            inner.renderer_conditions.remove(id);
            inner.generation_recoveries.remove(id);
            removed.into_iter().map(|(_, did)| did).collect()
        };
        self.deadlines
            .cancel(deadline::DeadlineKey::renderer_start(id));
        self.emit(RouterEvent::RendererRemoved(id.to_string()));
        let had_affected = !affected.is_empty();
        for did in affected {
            self.sync_display(did).await;
        }
        self.reconcile_lifecycle().await;
        if had_affected {
            let all = self.snapshot_displays().await;
            self.emit(RouterEvent::DisplaysReplace(all));
        }
    }

    pub async fn on_renderer_process_exit(
        self: &Arc<Self>,
        exit: crate::wallframe::renderer_manager::RendererProcessExit,
    ) {
        let Some((renderer_id, remove_slot, _state)) =
            self.settle_renderer_process_exit(exit).await
        else {
            return;
        };
        if remove_slot {
            self.deadlines
                .cancel(deadline::DeadlineKey::renderer_start(&renderer_id));
        } else {
            self.resume_renderer_after_exit(&renderer_id).await;
        }
    }

    async fn settle_renderer_process_exit(
        self: &Arc<Self>,
        exit: crate::wallframe::renderer_manager::RendererProcessExit,
    ) -> Option<(RendererId, bool, RendererLifecycleState)> {
        let renderer_id = exit.renderer_id.clone();
        let (affected, remove_slot, state) = {
            let mut inner = self.inner.lock().await;
            let Some(slot) = inner.renderer_slots.get_mut(&exit.renderer_id) else {
                return None;
            };
            if slot.state.generation() != Some(exit.process_generation) {
                log::debug!(
                    "renderer {}: ignore stale process exit generation={} current={:?}",
                    exit.renderer_id,
                    exit.process_generation,
                    slot.state.generation()
                );
                return None;
            }
            let transition = slot.transition(RendererLifecycleEvent::ProcessExited {
                generation: exit.process_generation,
                kind: exit.kind,
                exit: RendererExitSnapshot::from(&exit),
            });
            if transition == RendererTransition::Unchanged {
                return None;
            }
            let remove_slot = transition == RendererTransition::Remove;
            let state = slot.state.clone();
            inner.table.detach_renderer(&exit.renderer_id);
            if let Some(task) = inner.renderer_tasks.remove(&exit.renderer_id) {
                task.abort();
            }
            if let Some(task) = inner.orphan_timers.remove(&exit.renderer_id) {
                task.abort();
            }
            inner.resume_retries.remove(&exit.renderer_id);
            if let Some(task) = inner.resume_retry_tasks.remove(&exit.renderer_id) {
                task.abort();
            }
            inner.release_waits.remove(&exit.renderer_id);
            inner.generation_poisoned.remove(&exit.renderer_id);
            inner.renderer_conditions.remove(&exit.renderer_id);
            inner.generation_recoveries.remove(&exit.renderer_id);
            let affected = inner
                .table
                .links_for_renderer(&exit.renderer_id)
                .into_iter()
                .map(|link| link.display_id)
                .collect::<Vec<_>>();
            if remove_slot {
                inner.renderer_manual_paused.remove(&exit.renderer_id);
                inner.table.remove_renderer(&exit.renderer_id);
                inner.renderer_slots.remove(&exit.renderer_id);
                inner
                    .disconnected_assignments
                    .retain(|_, renderer_id| renderer_id != &exit.renderer_id);
            }
            (affected, remove_slot, state)
        };
        for display_id in &affected {
            self.sync_display(*display_id).await;
        }
        if remove_slot {
            self.emit(RouterEvent::RendererRemoved(renderer_id.clone()));
        } else if let Some(snapshot) = self.snapshot_renderer(&renderer_id).await {
            self.emit(RouterEvent::RendererUpsert(snapshot));
        }
        if !affected.is_empty() {
            self.emit(RouterEvent::DisplaysReplace(self.snapshot_displays().await));
        }
        self.reconcile_lifecycle().await;
        self.refresh_runtime_health().await;
        Some((renderer_id, remove_slot, state))
    }

    pub async fn set_renderer_wallpaper_layout_override(
        self: &Arc<Self>,
        renderer_id: &str,
        layout: WallpaperLayoutOverride,
    ) -> bool {
        let display_ids: Vec<DisplayId> = {
            let mut inner = self.inner.lock().await;
            if !inner.renderer_slots.contains_key(renderer_id) {
                return false;
            }
            if layout.is_empty() {
                inner.wallpaper_layout_overrides.remove(renderer_id);
            } else {
                inner
                    .wallpaper_layout_overrides
                    .insert(renderer_id.to_string(), layout);
            }
            inner
                .table
                .links_for_renderer(renderer_id)
                .into_iter()
                .filter(|l| l.enabled)
                .map(|l| l.display_id)
                .collect()
        };
        self.resync_display_compositions(display_ids.iter().copied())
            .await;
        if !display_ids.is_empty() {
            let all = self.snapshot_displays().await;
            self.emit(RouterEvent::DisplaysReplace(all));
        }
        true
    }

    /// Arm `ack_unbind` tracking for `renderer_id`. MUST be called
    /// before any sync_display that emits Unbind for this renderer.
    pub async fn begin_unbind_ack_tracking(self: &Arc<Self>, renderer_id: &str) {
        let mut inner = self.inner.lock().await;
        inner
            .unbind_acks_pending
            .entry(renderer_id.to_string())
            .or_insert_with(HashSet::new);
    }

    /// Record an `ack_unbind` request from a display for a specific
    /// generation, draining the matching pending pair if present.
    pub async fn record_ack_unbind(
        self: &Arc<Self>,
        display_id: DisplayId,
        buffer_generation: u64,
    ) {
        {
            let mut inner = self.inner.lock().await;
            for pending in inner.unbind_acks_pending.values_mut() {
                pending.remove(&(display_id, buffer_generation));
            }
        }
        self.unbind_ack_notify.notify_waiters();
    }

    /// Wait for every (display, generation) pair recorded under
    /// `renderer_id` to be acked, or for `timeout` to elapse.
    pub async fn await_unbind_acks_for(
        self: &Arc<Self>,
        renderer_id: &str,
        timeout: Duration,
    ) -> Result<(), tokio::time::error::Elapsed> {
        let deadline = tokio::time::Instant::now() + timeout;
        let result = tokio::time::timeout_at(deadline, async {
            loop {
                // Create the notified future before checking pending state
                // so a concurrent record_ack_unbind cannot be missed.
                let notified = self.unbind_ack_notify.notified();
                tokio::pin!(notified);
                {
                    let inner = self.inner.lock().await;
                    match inner.unbind_acks_pending.get(renderer_id) {
                        None => return,
                        Some(set) if set.is_empty() => return,
                        _ => {}
                    }
                }
                notified.await;
            }
        })
        .await;

        // Drop the tracking entry whether we succeeded or timed out;
        // leaving it would delay later waits for the same renderer.
        let mut inner = self.inner.lock().await;
        if let Some(remaining) = inner.unbind_acks_pending.remove(renderer_id) {
            if !remaining.is_empty() {
                log::warn!(
                    "router: await_unbind_acks_for({renderer_id}) cleared {} \
                     un-acked entries (timeout or shutdown)",
                    remaining.len()
                );
            }
        }
        result
    }

    // ---------------------------------------------------------------
    // Display lifecycle

    pub async fn register_display(self: &Arc<Self>, reg: DisplayRegistration) -> DisplayHandle {
        // One-time legacy migration: if the consumer advertised a v4
        // instance_id, copy any legacy name-keyed settings once.
        if let (Some(iid), Some(settings)) =
            (reg.instance_id.as_deref(), self.settings.get().cloned())
        {
            if settings.display_prefs(iid).is_none() {
                if let Some(legacy) = settings.display_prefs(&reg.name) {
                    let iid_owned = iid.to_string();
                    settings.update(|s| {
                        s.displays.entry(iid_owned).or_insert(legacy);
                    });
                    log::info!(
                        "display settings: migrated [display.{}] → [display.{}]",
                        reg.name,
                        iid
                    );
                }
            }
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let initial_window_state_flags = reg.window_state_flags;
        let (display_id, display_session_id, auto_linked) = {
            let mut inner = self.inner.lock().await;
            inner.next_display_id += 1;
            inner.next_display_session_id = inner
                .next_display_session_id
                .checked_add(1)
                .expect("display session id exhausted");
            let id = inner.next_display_id;
            let session_id = inner.next_display_session_id;
            let info = DisplayInfo {
                id,
                name: reg.name,
                instance_id: reg.instance_id,
                metrics: reg.metrics,
                bound: false,
            };
            let pause_effect = self.resolved_pause_effect(reg.presentation_caps);
            let presentation = PresentationSnapshot {
                config: PresentationConfig {
                    generation: 1,
                    pause_effect,
                },
                state: PresentationState {
                    generation: 1,
                    config_generation: 1,
                    pause_effect: PauseEffectState { active: false },
                },
            };
            let restored_renderer = info
                .instance_id
                .as_ref()
                .and_then(|instance_id| inner.disconnected_assignments.get(instance_id))
                .filter(|renderer_id| inner.renderer_slots.contains_key(*renderer_id))
                .cloned();
            inner.displays.insert(
                id,
                DisplayState {
                    info,
                    session_id,
                    gpu: reg.consumer_caps.identity.drm,
                    tx,
                    binding: None,
                    next_wire_buffer_generation: 0,
                    consumer_caps: reg.consumer_caps,
                    failed_binding_generation: None,
                    presentation_caps: reg.presentation_caps,
                    presentation,
                    accepted: false,
                    auto_replay: auto_replay::State::new(),
                    consumption_epoch: Arc::new(AtomicU64::new(1)),
                },
            );
            let auto = restored_renderer.or_else(|| {
                let mut ids = inner.renderer_slots.keys().cloned().collect::<Vec<_>>();
                ids.sort();
                ids.into_iter().next()
            });
            if let Some(rid) = auto.clone() {
                let enabled = !inner.manual_stopped;
                inner.table.add_link_with_enabled(rid, id, enabled);
            }
            (id, session_id, auto)
        };
        // A freshly auto-linked renderer just gained an audience —
        // cancel any pending orphan timer so it survives.
        if let Some(rid) = auto_linked.as_deref() {
            self.cancel_orphan_timer(rid).await;
        }
        let auto_action = self
            .update_auto_state(display_id, Some(initial_window_state_flags))
            .await;
        self.run_auto_state_action(auto_action).await;
        if let Some(renderer_id) = auto_linked.as_deref() {
            if let Err(error) = self
                .request_renderer_start(renderer_id, RendererStartCause::DisplayReconnect)
                .await
            {
                log::warn!("renderer {renderer_id}: display reconnect start failed: {error}");
            }
        }
        self.reconcile_buffer_flags().await;
        self.sync_display(display_id).await;
        self.reconcile_lifecycle().await;
        self.refresh_runtime_health().await;
        if let Some(snap) = self.snapshot_display(display_id).await {
            self.emit(RouterEvent::DisplayUpsert(snap));
        }
        let presentation = {
            let mut inner = self.inner.lock().await;
            let state = inner
                .displays
                .get_mut(&display_id)
                .expect("registered display missing before acceptance");
            state.accepted = true;
            state.presentation
        };
        DisplayHandle {
            id: display_id,
            session_id: display_session_id,
            presentation,
            rx,
        }
    }

    pub async fn unregister_display(self: &Arc<Self>, display_id: DisplayId) {
        let cancelled_starts = {
            let mut inner = self.inner.lock().await;
            if let Some(display) = inner.displays.get(&display_id) {
                if let (Some(instance_id), Some(link)) = (
                    display.info.instance_id.clone(),
                    inner.table.links_for_display(display_id).into_iter().next(),
                ) {
                    inner
                        .disconnected_assignments
                        .insert(instance_id, link.renderer_id);
                }
            }
            inner.displays.remove(&display_id);
            inner.display_conditions.remove(&display_id);
            let removed_links = inner.table.remove_display(display_id);
            let mut renderer_ids = removed_links
                .into_iter()
                .map(|link| link.renderer_id)
                .collect::<Vec<_>>();
            renderer_ids.sort();
            renderer_ids.dedup();
            renderer_ids
                .into_iter()
                .filter(|renderer_id| {
                    inner.table.links_for_renderer(renderer_id).is_empty()
                        && inner
                            .renderer_slots
                            .get_mut(renderer_id)
                            .is_some_and(|slot| slot.pending_start.take().is_some())
                })
                .collect::<Vec<_>>()
        };
        for renderer_id in cancelled_starts {
            self.deadlines
                .cancel(deadline::DeadlineKey::renderer_start(&renderer_id));
        }
        // Any renderer that just lost its last link enters the 5s
        // grace window; no new renderer is protected during unplug.
        self.mark_orphans(None).await;
        self.reconcile_lifecycle().await;
        self.reconcile_buffer_flags().await;
        self.refresh_runtime_health().await;
        self.emit(RouterEvent::DisplayRemoved(display_id));
    }

    pub async fn on_consumer_import_failed(
        self: &Arc<Self>,
        display_id: DisplayId,
        buffer_generation: u64,
        kind: ConsumerImportFailureKind,
    ) -> ConsumerImportFailureOutcome {
        let (fourcc, modifier, inserted) = {
            let mut inner = self.inner.lock().await;
            let Some(state) = inner.displays.get_mut(&display_id) else {
                return ConsumerImportFailureOutcome::Stale;
            };
            let Some(binding) = state.binding.as_ref() else {
                return ConsumerImportFailureOutcome::Stale;
            };
            if binding.wire_generation != buffer_generation {
                return ConsumerImportFailureOutcome::Stale;
            }
            let fourcc = binding.pool.fourcc;
            let modifier = binding.pool.modifier;
            state.failed_binding_generation = Some(buffer_generation);
            state.invalidate_consumption();
            if kind != ConsumerImportFailureKind::Unsupported {
                return ConsumerImportFailureOutcome::Terminal;
            }
            let inserted = state.consumer_caps.blacklist.insert((fourcc, modifier));
            (fourcc, modifier, inserted)
        };
        if inserted {
            log::info!(
                "router: display {display_id}: blacklisted (0x{fourcc:08x}, 0x{modifier:x}) — re-running picker"
            );
        }
        self.reconcile_buffer_flags().await;
        ConsumerImportFailureOutcome::Retry { fourcc, modifier }
    }

    /// Renderer reported `bind_failed` for `(fourcc, modifier)`.
    /// Add the pair to this producer's blacklist and retry negotiation.
    pub async fn on_renderer_bind_failed(
        self: &Arc<Self>,
        renderer_id: &str,
        fourcc: u32,
        modifier: u64,
    ) {
        let inserted = {
            let inner = self.inner.lock().await;
            let Some(renderer) = inner.table.get_renderer(renderer_id) else {
                return;
            };
            renderer.blacklist_format(fourcc, modifier)
        };
        if inserted {
            log::info!(
                "router: renderer {renderer_id}: blacklisted (0x{fourcc:08x}, 0x{modifier:x}) — re-running picker"
            );
        }
        self.reconcile_buffer_flags().await;
    }

    /// Renderer published a `ReportState` event. The reader thread
    /// already committed the validated fields onto the handle.
    pub async fn on_renderer_state_changed(
        self: &Arc<Self>,
        renderer_id: &str,
        changed_fields: u32,
    ) {
        if changed_fields & RENDERER_STATE_FIELD_CLEAR_COLOR != 0 {
            let new_clear = {
                let inner = self.inner.lock().await;
                let Some(renderer) = inner.table.get_renderer(renderer_id) else {
                    return;
                };
                renderer.clear_rgba()
            };
            let affected: Vec<DisplayId> = {
                let mut inner = self.inner.lock().await;
                let link_ids: Vec<LinkId> = inner
                    .table
                    .links_for_renderer(renderer_id)
                    .into_iter()
                    .map(|link| link.id)
                    .collect();
                let mut affected = Vec::new();
                for link_id in link_ids {
                    let changed = inner.table.update_link_geometry(
                        link_id,
                        None,
                        None,
                        None,
                        Some(new_clear),
                        None,
                    );
                    if changed {
                        if let Some(link) = inner.table.get_link(link_id) {
                            affected.push(link.display_id);
                        }
                    }
                }
                affected
            };
            self.resync_display_compositions(affected).await;
        }

        if changed_fields & RENDERER_STATE_FIELD_RUNTIME_TAGS != 0 {
            if let Some(snapshot) = self.snapshot_renderer(renderer_id).await {
                self.emit(RouterEvent::RendererUpsert(snapshot));
            }
        }
    }

    pub async fn set_display_metrics(
        self: &Arc<Self>,
        display_id: DisplayId,
        metrics: DisplayMetrics,
    ) {
        if metrics.width == 0 || metrics.height == 0 {
            log::warn!(
                "set_display_metrics: ignoring zero dim ({}x{}) for display {display_id:?}",
                metrics.width,
                metrics.height,
            );
            return;
        }
        let changed = {
            let mut inner = self.inner.lock().await;
            if let Some(s) = inner.displays.get_mut(&display_id) {
                let differs = s.info.metrics != metrics;
                s.info.metrics = metrics;
                differs
            } else {
                return;
            }
        };
        // Layout depends on disp_w/disp_h, so any size change must
        // trigger a fresh composition config under the resolved fillmode/align.
        if changed {
            self.resync_display_composition(display_id).await;
        }
        if let Some(snap) = self.snapshot_display(display_id).await {
            self.emit(RouterEvent::DisplayUpsert(snap));
        }
    }

    /// Update the per-display auto replay machine from a consumer's
    /// `set_window_state` request.
    pub async fn update_display_window_state(self: &Arc<Self>, display_id: DisplayId, flags: u32) {
        let action = self.update_auto_state(display_id, Some(flags)).await;
        self.run_auto_state_action(action).await;
    }

    async fn reconcile_presentation_config(self: &Arc<Self>, display_id: DisplayId) {
        let mut inner = self.inner.lock().await;
        let Some(current) = inner.displays.get(&display_id) else {
            return;
        };
        let desired_config = self.resolved_pause_effect(current.presentation_caps);
        let desired_dynamic = PauseEffectState {
            active: Self::pause_effect_active(&inner, current, desired_config.kind),
        };
        let state = inner
            .displays
            .get_mut(&display_id)
            .expect("display checked above");

        if state.presentation.config.pause_effect != desired_config {
            let config_generation = state
                .presentation
                .config
                .generation
                .checked_add(1)
                .expect("presentation config generation exhausted");
            let dynamic_generation = state
                .presentation
                .state
                .generation
                .checked_add(1)
                .expect("presentation dynamic generation exhausted");
            state.presentation = PresentationSnapshot {
                config: PresentationConfig {
                    generation: config_generation,
                    pause_effect: desired_config,
                },
                state: PresentationState {
                    generation: dynamic_generation,
                    config_generation,
                    pause_effect: desired_dynamic,
                },
            };
            if state.accepted {
                let _ = state
                    .tx
                    .send(DisplayOutEvent::SetPresentationSnapshot(state.presentation));
            }
        } else if state.presentation.state.pause_effect != desired_dynamic {
            state.presentation.state = PresentationState {
                generation: state
                    .presentation
                    .state
                    .generation
                    .checked_add(1)
                    .expect("presentation dynamic generation exhausted"),
                config_generation: state.presentation.config.generation,
                pause_effect: desired_dynamic,
            };
            if state.accepted {
                let _ = state.tx.send(DisplayOutEvent::SetPresentationState(
                    state.presentation.state,
                ));
            }
        }
    }

    pub async fn resync_presentation_configs(self: &Arc<Self>) {
        let display_ids: Vec<DisplayId> = {
            let inner = self.inner.lock().await;
            inner.displays.keys().copied().collect()
        };
        for display_id in display_ids {
            self.reconcile_presentation_config(display_id).await;
        }
    }

    pub async fn resync_auto_replay(self: &Arc<Self>) {
        let display_ids: Vec<DisplayId> = {
            let inner = self.inner.lock().await;
            inner.displays.keys().copied().collect()
        };
        let mut reconcile = false;
        for display_id in display_ids {
            reconcile |= matches!(
                self.update_auto_state(display_id, None).await,
                AutoStateAction::Reconcile
            );
        }
        if reconcile {
            self.apply_auto_stop_links().await;
            self.reconcile_lifecycle().await;
        }
    }

    /// Subscribe to router events (display add/change/remove). The
    /// returned receiver is lagged-on-overflow.
    pub fn subscribe_events(self: &Arc<Self>) -> broadcast::Receiver<RouterEvent> {
        self.events_tx.subscribe()
    }

    async fn record_release_event(
        self: &Arc<Self>,
        event: crate::wallframe::sync::ReleaseEvent,
    ) -> bool {
        let mut inner = self.inner.lock().await;
        match event {
            crate::wallframe::sync::ReleaseEvent::Waiting { consumer, state } => {
                let waits = inner
                    .release_waits
                    .entry(consumer.renderer_id.clone())
                    .or_default();
                waits
                    .entry(consumer.clone())
                    .and_modify(|wait| wait.state = state)
                    .or_insert(ReleaseWaitFact {
                        consumer,
                        state,
                        since: Instant::now(),
                    });
                false
            }
            crate::wallframe::sync::ReleaseEvent::Resolved { consumer } => {
                if let Some(waits) = inner.release_waits.get_mut(&consumer.renderer_id) {
                    waits.remove(&consumer);
                    if waits.is_empty() {
                        inner.release_waits.remove(&consumer.renderer_id);
                    }
                }
                false
            }
            crate::wallframe::sync::ReleaseEvent::GenerationPoisoned { consumer, reason } => {
                log::error!(
                    "renderer {} release generation {} poisoned by display {} session {}: {}",
                    consumer.renderer_id,
                    consumer.frame.buffer_generation,
                    consumer.display_id,
                    consumer.display_session_id,
                    reason,
                );
                inner
                    .generation_poisoned
                    .insert(consumer.renderer_id.clone(), consumer.clone());
                inner.generation_recoveries.insert(consumer.renderer_id)
            }
        }
    }

    async fn refresh_runtime_health(self: &Arc<Self>) {
        let now = Instant::now();
        let (renderer_changes, display_changes) = {
            let mut inner = self.inner.lock().await;
            let mut renderer_conditions = HashMap::new();
            for renderer_id in inner.table.renderer_ids() {
                let Some(handle) = inner.table.get_renderer(&renderer_id) else {
                    continue;
                };
                let status = inner
                    .renderer_slots
                    .get(&renderer_id)
                    .and_then(|slot| slot.state.activity())
                    .unwrap_or(RendererActivity::Playing);
                let has_audience = inner
                    .table
                    .links_for_renderer(&renderer_id)
                    .into_iter()
                    .any(|link| link.enabled && inner.displays.contains_key(&link.display_id));
                let waits = inner
                    .release_waits
                    .get(&renderer_id)
                    .into_iter()
                    .flat_map(|waits| waits.values().cloned());
                let conditions = evaluate_renderer_conditions(
                    now,
                    &renderer_id,
                    status,
                    has_audience,
                    handle.activity_mode,
                    handle.progress(),
                    waits,
                    inner.generation_poisoned.get(&renderer_id),
                );
                renderer_conditions.insert(renderer_id, conditions);
            }

            let mut display_conditions = HashMap::new();
            for display_id in inner.displays.keys().copied() {
                let mut conditions = Vec::new();
                for link in inner
                    .table
                    .links_for_display(display_id)
                    .into_iter()
                    .filter(|link| link.enabled)
                {
                    if let Some(renderer) = renderer_conditions.get(&link.renderer_id) {
                        conditions.extend(renderer.iter().cloned());
                    }
                }
                conditions.sort();
                conditions.dedup();
                display_conditions.insert(display_id, conditions);
            }

            let renderer_changes = renderer_conditions
                .iter()
                .filter_map(|(id, conditions)| {
                    (inner.renderer_conditions.get(id) != Some(conditions)).then(|| id.clone())
                })
                .collect::<Vec<_>>();
            let display_changes = display_conditions
                .iter()
                .filter_map(|(id, conditions)| {
                    (inner.display_conditions.get(id) != Some(conditions)).then_some(*id)
                })
                .collect::<Vec<_>>();
            inner.renderer_conditions = renderer_conditions;
            inner.display_conditions = display_conditions;
            (renderer_changes, display_changes)
        };

        for renderer_id in renderer_changes {
            if let Some(snapshot) = self.snapshot_renderer(&renderer_id).await {
                self.emit(RouterEvent::RendererUpsert(snapshot));
            }
        }
        for display_id in display_changes {
            if let Some(snapshot) = self.snapshot_display(display_id).await {
                self.emit(RouterEvent::DisplayUpsert(snapshot));
            }
        }
    }

    fn recover_poisoned_renderer<'a>(
        self: &'a Arc<Self>,
        renderer_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let (spawn_request, layout) = {
                let inner = self.inner.lock().await;
                if !inner.generation_recoveries.contains(renderer_id) {
                    return;
                }
                let Some(handle) = inner.table.get_renderer(renderer_id) else {
                    return;
                };
                (
                    handle.spawn_request(),
                    inner.wallpaper_layout_overrides.get(renderer_id).copied(),
                )
            };

            log::warn!("renderer {renderer_id}: rebuilding poisoned release generation");
            let new_id = match self.spawn_unassigned_renderer(spawn_request).await {
                Ok(id) => id,
                Err(error) => {
                    log::error!(
                        "renderer {renderer_id}: poisoned generation rebuild failed: {error}"
                    );
                    return;
                }
            };
            if let Some(layout) = layout {
                self.set_renderer_wallpaper_layout_override(&new_id, layout)
                    .await;
            }
            self.begin_unbind_ack_tracking(renderer_id).await;
            let affected = {
                let mut inner = self.inner.lock().await;
                inner.table.retarget_renderer_links(renderer_id, &new_id)
            };
            for display_id in &affected {
                self.sync_display(*display_id).await;
            }
            self.reconcile_lifecycle().await;
            self.reconcile_buffer_flags().await;
            if !affected.is_empty() {
                self.emit(RouterEvent::DisplaysReplace(self.snapshot_displays().await));
            }
            if self
                .await_unbind_acks_for(renderer_id, Duration::from_secs(1))
                .await
                .is_err()
            {
                log::warn!(
                    "renderer {renderer_id}: poisoned generation unbind acknowledgement timed out"
                );
            }
            if let Err(error) = self
                .stop_renderer_drop(renderer_id, Duration::from_secs(1))
                .await
            {
                log::warn!("renderer {renderer_id}: poisoned generation stop failed: {error}");
            }
            log::info!("renderer {renderer_id}: recovered as {new_id}");
        })
    }

    /// Number of currently registered displays. Cheap (O(1) on the
    /// inner displays map) read for apply-path preconditions.
    pub async fn display_count(self: &Arc<Self>) -> usize {
        self.inner.lock().await.displays.len()
    }

    /// Registered display ids for an apply target. `None` means all
    /// displays; explicit ids are filtered to currently registered displays.
    pub async fn registered_display_ids(
        self: &Arc<Self>,
        target: Option<&[DisplayId]>,
    ) -> Vec<DisplayId> {
        let inner = self.inner.lock().await;
        let mut ids: Vec<DisplayId> = match target {
            None => inner.displays.keys().copied().collect(),
            Some(target) => target
                .iter()
                .copied()
                .filter(|id| inner.displays.contains_key(id))
                .collect(),
        };
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Enabled display links currently using `renderer_id`, ordered by id.
    pub async fn renderer_display_ids(self: &Arc<Self>, renderer_id: &str) -> Vec<DisplayId> {
        let inner = self.inner.lock().await;
        let mut ids: Vec<DisplayId> = inner
            .table
            .links_for_renderer(renderer_id)
            .into_iter()
            .filter(|link| link.enabled)
            .map(|link| link.display_id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Schedule or remove slots that no longer have an assignment.
    pub async fn mark_orphans(self: &Arc<Self>, keep: Option<&str>) -> Vec<RendererId> {
        let (candidates, lone_renderer_no_displays) = {
            let inner = self.inner.lock().await;
            let cs: Vec<RendererId> = inner
                .renderer_slots
                .keys()
                .cloned()
                .into_iter()
                .filter(|rid| {
                    if Some(rid.as_str()) == keep {
                        return false;
                    }
                    inner.table.links_for_renderer(rid).is_empty()
                        && inner
                            .renderer_slots
                            .get(rid)
                            .is_some_and(|slot| !slot.state.is_failed())
                })
                .collect();
            let lone = inner.displays.is_empty() && inner.renderer_slots.len() == 1;
            (cs, lone)
        };
        for rid in &candidates {
            if lone_renderer_no_displays {
                self.schedule_orphan_grace(rid.clone()).await;
            } else {
                self.kill_orphan_now(rid).await;
            }
        }
        if let Some(k) = keep {
            self.cancel_orphan_timer(k).await;
        }
        candidates
    }

    /// Mark `renderer_id` as orphaned. Reaps immediately unless this
    /// is the only renderer and no displays are registered.
    pub async fn mark_orphan(self: &Arc<Self>, renderer_id: RendererId) {
        let lone_renderer_no_displays = {
            let inner = self.inner.lock().await;
            if inner
                .renderer_slots
                .get(&renderer_id)
                .is_some_and(|slot| slot.state.is_failed())
            {
                return;
            }
            inner.displays.is_empty() && inner.renderer_slots.len() == 1
        };
        if lone_renderer_no_displays {
            self.schedule_orphan_grace(renderer_id).await;
        } else {
            self.kill_orphan_now(&renderer_id).await;
        }
    }

    async fn schedule_orphan_grace(self: &Arc<Self>, renderer_id: RendererId) {
        let weak = Arc::downgrade(self);
        let rid_for_task = renderer_id.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(ORPHAN_REAP_TIMEOUT).await;
            let Some(this) = weak.upgrade() else { return };
            this.fire_orphan_reap(&rid_for_task).await;
        });
        let mut inner = self.inner.lock().await;
        if let Some(prev) = inner.orphan_timers.insert(renderer_id.clone(), task) {
            prev.abort();
        }
        log::debug!(
            "router: orphan timer scheduled for {renderer_id} ({:?})",
            ORPHAN_REAP_TIMEOUT
        );
    }

    async fn kill_orphan_now(self: &Arc<Self>, renderer_id: &str) {
        log::info!("router: reaping orphan renderer {renderer_id} immediately");
        if let Err(error) = self.kill_renderer_drop(renderer_id).await {
            log::warn!("router: kill orphan {renderer_id}: {error}");
        }
    }

    /// Cancel a pending orphan-reap timer for `renderer_id` (if any).
    /// Called when a renderer gains a display again.
    pub async fn cancel_orphan_timer(self: &Arc<Self>, renderer_id: &str) {
        let removed = self.inner.lock().await.orphan_timers.remove(renderer_id);
        if let Some(task) = removed {
            task.abort();
            log::debug!("router: orphan timer cancelled for {renderer_id}");
        }
    }

    /// Timer body: re-check the orphan condition under the lock and
    /// kill if it still holds, clearing the timer entry first.
    async fn fire_orphan_reap(self: &Arc<Self>, renderer_id: &str) {
        let still_orphan = {
            let mut inner = self.inner.lock().await;
            // Drop our own entry first so a concurrent re-mark sees an
            // empty slot and schedules a fresh timer.
            inner.orphan_timers.remove(renderer_id);
            let Some(slot) = inner.renderer_slots.get(renderer_id) else {
                return;
            };
            !slot.state.is_failed() && inner.table.links_for_renderer(renderer_id).is_empty()
        };
        if !still_orphan {
            return;
        }
        log::info!("router: reaping orphan renderer {renderer_id} after grace");
        if let Err(error) = self.kill_renderer_drop(renderer_id).await {
            log::warn!("router: kill orphan {renderer_id}: {error}");
        }
    }

    /// Fire an event to all subscribers. Send errors (no subscribers)
    /// are downgraded to debug logs.
    pub fn emit(&self, evt: RouterEvent) {
        if let Err(e) = self.events_tx.send(evt) {
            log::debug!("router: no event subscribers ({e})");
        }
    }

    /// Snapshot of a single display by id. Returns `None` if the
    /// display has been unregistered.
    pub async fn snapshot_display(self: &Arc<Self>, id: DisplayId) -> Option<DisplaySnapshot> {
        let inner = self.inner.lock().await;
        let s = inner.displays.get(&id)?;
        let link_rows = inner.table.links_for_display(id);
        let display_layout = self.resolved_layout(&s.info);
        let display_layout_source = self.display_layout_source(&s.info);
        let wallpaper_layout_override = link_rows.first().and_then(|l| {
            inner
                .wallpaper_layout_overrides
                .get(&l.renderer_id)
                .copied()
                .filter(|layout| !layout.is_empty())
        });
        let (effective_layout, effective_layout_source) =
            if let Some(layout) = wallpaper_layout_override {
                (layout.apply_to(display_layout), LayoutSource::Wallpaper)
            } else {
                (display_layout, display_layout_source)
            };
        let links = link_rows
            .into_iter()
            .map(|l| DisplayLinkSnapshot {
                renderer_id: l.renderer_id,
                z_order: l.z_order,
                active: l.enabled,
            })
            .collect();
        Some(DisplaySnapshot {
            id,
            name: s.info.name.clone(),
            instance_id: s.info.instance_id.clone(),
            width: s.info.metrics.width,
            height: s.info.metrics.height,
            refresh_mhz: s.info.metrics.refresh_mhz,
            links,
            drm_render_major: s.gpu.major,
            drm_render_minor: s.gpu.minor,
            display_layout,
            effective_layout,
            effective_layout_source,
            conditions: inner
                .display_conditions
                .get(&id)
                .cloned()
                .unwrap_or_default(),
        })
    }

    /// Snapshot of a single logical renderer slot by id.
    pub async fn snapshot_renderer(self: &Arc<Self>, id: &str) -> Option<RendererSnapshot> {
        let inner = self.inner.lock().await;
        let slot = inner.renderer_slots.get(id)?;
        let handle = inner.table.get_renderer(id);
        let (tw, th) = handle
            .as_ref()
            .map(|handle| handle.texture_size())
            .unwrap_or_default();
        Some(RendererSnapshot {
            id: id.to_string(),
            wp_type: slot.spawn_request.wp_type.clone(),
            name: slot.name.clone(),
            state: slot.state.clone(),
            pid: handle.as_ref().and_then(|handle| handle.pid).unwrap_or(0),
            drm_render_major: handle.as_ref().map(|handle| handle.gpu.major).unwrap_or(0),
            drm_render_minor: handle.as_ref().map(|handle| handle.gpu.minor).unwrap_or(0),
            texture_width: tw,
            texture_height: th,
            runtime_tags: handle
                .as_ref()
                .map(|handle| handle.runtime_tags())
                .unwrap_or_default(),
            conditions: inner
                .renderer_conditions
                .get(id)
                .cloned()
                .unwrap_or_default(),
        })
    }

    /// Snapshot of every registered renderer, ordered by ascending id
    /// for UI stability.
    pub async fn snapshot_renderers(self: &Arc<Self>) -> Vec<RendererSnapshot> {
        let inner = self.inner.lock().await;
        let mut ids: Vec<_> = inner.renderer_slots.keys().cloned().collect();
        ids.sort_unstable();
        ids.into_iter()
            .filter_map(|id| {
                let slot = inner.renderer_slots.get(&id)?;
                let handle = inner.table.get_renderer(&id);
                let (tw, th) = handle
                    .as_ref()
                    .map(|handle| handle.texture_size())
                    .unwrap_or_default();
                Some(RendererSnapshot {
                    id: id.clone(),
                    wp_type: slot.spawn_request.wp_type.clone(),
                    name: slot.name.clone(),
                    state: slot.state.clone(),
                    pid: handle.as_ref().and_then(|handle| handle.pid).unwrap_or(0),
                    drm_render_major: handle.as_ref().map(|handle| handle.gpu.major).unwrap_or(0),
                    drm_render_minor: handle.as_ref().map(|handle| handle.gpu.minor).unwrap_or(0),
                    texture_width: tw,
                    texture_height: th,
                    runtime_tags: handle
                        .as_ref()
                        .map(|handle| handle.runtime_tags())
                        .unwrap_or_default(),
                    conditions: inner
                        .renderer_conditions
                        .get(&id)
                        .cloned()
                        .unwrap_or_default(),
                })
            })
            .collect()
    }

    /// Snapshot of every registered display plus its assignments
    /// pointing at it, ordered by ascending id for UI stability.
    pub async fn snapshot_displays(self: &Arc<Self>) -> Vec<DisplaySnapshot> {
        let inner = self.inner.lock().await;
        let mut ids: Vec<DisplayId> = inner.displays.keys().copied().collect();
        ids.sort_unstable();
        ids.into_iter()
            .filter_map(|id| {
                let s = inner.displays.get(&id)?;
                let link_rows = inner.table.links_for_display(id);
                let display_layout = self.resolved_layout(&s.info);
                let display_layout_source = self.display_layout_source(&s.info);
                let wallpaper_layout_override = link_rows.first().and_then(|l| {
                    inner
                        .wallpaper_layout_overrides
                        .get(&l.renderer_id)
                        .copied()
                        .filter(|layout| !layout.is_empty())
                });
                let (effective_layout, effective_layout_source) =
                    if let Some(layout) = wallpaper_layout_override {
                        (layout.apply_to(display_layout), LayoutSource::Wallpaper)
                    } else {
                        (display_layout, display_layout_source)
                    };
                let links = link_rows
                    .into_iter()
                    .map(|l| DisplayLinkSnapshot {
                        renderer_id: l.renderer_id,
                        z_order: l.z_order,
                        active: l.enabled,
                    })
                    .collect();
                Some(DisplaySnapshot {
                    id,
                    name: s.info.name.clone(),
                    instance_id: s.info.instance_id.clone(),
                    width: s.info.metrics.width,
                    height: s.info.metrics.height,
                    refresh_mhz: s.info.metrics.refresh_mhz,
                    links,
                    drm_render_major: s.gpu.major,
                    drm_render_minor: s.gpu.minor,
                    display_layout,
                    effective_layout,
                    effective_layout_source,
                    conditions: inner
                        .display_conditions
                        .get(&id)
                        .cloned()
                        .unwrap_or_default(),
                })
            })
            .collect()
    }

    /// For each requested `DisplayId`, return its settings key —
    /// `instance_id` when present, else display name.
    pub async fn display_settings_keys(
        self: &Arc<Self>,
        ids: &[DisplayId],
    ) -> Vec<(DisplayId, String)> {
        let inner = self.inner.lock().await;
        ids.iter()
            .filter_map(|did| {
                let s = inner.displays.get(did)?;
                Some((*did, Self::settings_key_for(&s.info).to_string()))
            })
            .collect()
    }

    /// Emit a `LibraryUpsert` event so subscribers (UI) refresh their
    /// view. The router no longer caches library state.
    pub fn upsert_library(self: &Arc<Self>, snap: LibrarySnapshot) {
        self.emit(RouterEvent::LibraryUpsert(snap));
    }

    pub fn remove_library(self: &Arc<Self>, id: i64) {
        self.emit(RouterEvent::LibraryRemoved(id));
    }

    async fn on_renderer_bind(self: &Arc<Self>, renderer_id: &str) {
        let display_ids: Vec<DisplayId> = {
            let inner = self.inner.lock().await;
            inner
                .table
                .links_for_renderer(renderer_id)
                .into_iter()
                .filter(|l| l.enabled)
                .map(|l| l.display_id)
                .collect()
        };
        self.reconcile_buffer_flags().await;
        for did in display_ids {
            self.sync_display(did).await;
            self.reconcile_presentation_config(did).await;
        }
        // BindBuffers is also when the renderer's actual texture dims
        // become known; push a fresh renderer snapshot for the UI.
        if let Some(snap) = self.snapshot_renderer(renderer_id).await {
            self.emit(RouterEvent::RendererUpsert(snap));
        }
    }

    async fn on_renderer_frame(
        self: &Arc<Self>,
        renderer_id: &str,
        producer_generation: u64,
        buffer_index: u32,
        seq: u64,
        release_point: u64,
    ) {
        let mut inner = self.inner.lock().await;
        if let Some(slot) = inner.renderer_slots.get_mut(renderer_id) {
            slot.restart_failures = 0;
        }
        let Some(renderer) = inner.table.get_renderer(renderer_id) else {
            return;
        };
        // First pass: collect every display that should get this frame
        // so we can pre-compute fan-out width for the reaper.
        let recipients: Vec<(&DisplayState, u64)> = inner
            .table
            .links_for_renderer(renderer_id)
            .into_iter()
            .filter(|link| link.enabled)
            .filter_map(|link| inner.displays.get(&link.display_id))
            .filter_map(|state| {
                let binding = state.binding.as_ref()?;
                (binding.pool.generation == producer_generation
                    && Arc::ptr_eq(&binding.renderer, &renderer))
                .then_some(binding)
                .filter(|binding| state.failed_binding_generation != Some(binding.wire_generation))
                .map(|binding| (state, binding.wire_generation))
            })
            .collect();
        let identity = crate::wallframe::sync::FrameIdentity {
            buffer_generation: producer_generation,
            buffer_index,
            release_point,
        };
        let consumers = recipients
            .iter()
            .map(|(state, _)| crate::wallframe::sync::FrameConsumerIdentity {
                frame: identity,
                renderer_id: renderer_id.to_string(),
                display_id: state.info.id,
                display_session_id: state.session_id,
                display_name: state.info.name.clone(),
                frame_seq: seq,
            })
            .collect();
        let members = match renderer.register_frame_consumers(identity, consumers) {
            Ok(members) => members,
            Err(error) => {
                log::warn!(
                    "router: renderer {renderer_id}: failed to register release point \
                     {release_point} before fan-out: {error}"
                );
                return;
            }
        };
        for ((state, wire_generation), member) in recipients.into_iter().zip(members) {
            let _ = state.tx.send(DisplayOutEvent::Frame {
                renderer: renderer.clone(),
                buffer_generation: wire_generation,
                buffer_index,
                seq,
                consumption: state.consumption_permit(),
                member: Some(member),
            });
        }
    }

    /// Compute the current Pause/Play diff and dispatch control
    /// messages outside the inner lock after lifecycle mutations.
    async fn reconcile_lifecycle(self: &Arc<Self>) {
        let _lifecycle = self.lifecycle_lock.lock().await;
        let audio_fade_ms = self.resolved_audio_fade_ms();
        let actions: Vec<(RendererId, ControlMsg, &'static str)> = {
            let mut inner = self.inner.lock().await;
            let mut out: Vec<(RendererId, ControlMsg, &'static str)> = Vec::new();
            for rid in inner.table.renderer_ids() {
                let links: Vec<Link> = inner
                    .table
                    .links_for_renderer(&rid)
                    .into_iter()
                    .filter(|l| l.enabled)
                    .collect();
                let has_active_link = !links.is_empty();
                // Auto replay only matters when at least one active link
                // exists; no-link pause is handled by ref-count.
                let (auto_pause_requested, auto_mute_decision) = if has_active_link {
                    links.iter().fold(
                        (false, None::<auto_replay::Decision>),
                        |(auto_pause_requested, auto_mute_decision), l| {
                            if let Some(display) = inner.displays.get(&l.display_id) {
                                match display.auto_replay.requested.action {
                                    AutoAction::Pause => (true, auto_mute_decision),
                                    AutoAction::Mute => {
                                        let decision = display.auto_replay.requested;
                                        let next = auto_mute_decision.or(Some(decision));
                                        (auto_pause_requested, next)
                                    }
                                    AutoAction::Stop | AutoAction::None => {
                                        (auto_pause_requested, auto_mute_decision)
                                    }
                                }
                            } else {
                                (auto_pause_requested, auto_mute_decision)
                            }
                        },
                    )
                } else {
                    (false, None)
                };
                let manual_paused =
                    inner.manual_paused || inner.renderer_manual_paused.contains(&rid);
                let manual_muted = inner.manual_muted;
                let other_playback_active = inner.other_playback_active;
                let should_pause = manual_paused || !has_active_link || auto_pause_requested;
                let should_mute = manual_muted
                    || other_playback_active
                    || !has_active_link
                    || auto_mute_decision.is_some();
                let previous_state = inner
                    .renderer_slots
                    .get(&rid)
                    .and_then(|slot| slot.state.activity());
                let target_state = if should_pause {
                    RendererActivity::Paused
                } else if should_mute {
                    RendererActivity::Muted
                } else {
                    RendererActivity::Playing
                };
                if target_state != RendererActivity::Playing {
                    inner.resume_retries.remove(&rid);
                }
                let Some(previous_state) = previous_state else {
                    continue;
                };
                if previous_state == target_state {
                    continue;
                }
                if let Some(slot) = inner.renderer_slots.get_mut(&rid) {
                    let _ = slot.transition(RendererLifecycleEvent::ActivityResolved(target_state));
                }
                let clear_cause = if has_active_link {
                    "pause-clear"
                } else {
                    "ref-count"
                };
                let pause_cause = if manual_paused {
                    "manual"
                } else if has_active_link {
                    "auto-action"
                } else {
                    "ref-count"
                };
                let mute_cause = if manual_muted {
                    "manual"
                } else if other_playback_active {
                    "external-audio"
                } else if has_active_link {
                    "auto-action"
                } else {
                    "ref-count"
                };
                match (previous_state, target_state) {
                    (RendererActivity::Playing, RendererActivity::Paused) => {
                        out.push((
                            rid,
                            ControlMsg::Pause {
                                transition: ControlTransition {
                                    fade_ms: audio_fade_ms,
                                },
                            },
                            pause_cause,
                        ));
                    }
                    (RendererActivity::Playing, RendererActivity::Muted) => {
                        out.push((
                            rid,
                            ControlMsg::Mute {
                                transition: ControlTransition {
                                    fade_ms: audio_fade_ms,
                                },
                            },
                            mute_cause,
                        ));
                    }
                    (RendererActivity::Paused, RendererActivity::Playing) => {
                        out.push((
                            rid,
                            ControlMsg::Play {
                                transition: ControlTransition {
                                    fade_ms: audio_fade_ms,
                                },
                            },
                            clear_cause,
                        ));
                    }
                    (RendererActivity::Muted, RendererActivity::Playing) => {
                        out.push((
                            rid,
                            ControlMsg::Unmute {
                                transition: ControlTransition {
                                    fade_ms: audio_fade_ms,
                                },
                            },
                            clear_cause,
                        ));
                    }
                    (RendererActivity::Paused, RendererActivity::Muted) => {
                        out.push((
                            rid.clone(),
                            ControlMsg::Mute {
                                transition: ControlTransition { fade_ms: 0 },
                            },
                            mute_cause,
                        ));
                        out.push((
                            rid,
                            ControlMsg::Play {
                                transition: ControlTransition { fade_ms: 0 },
                            },
                            "state-switch",
                        ));
                    }
                    (RendererActivity::Muted, RendererActivity::Paused) => {
                        out.push((
                            rid.clone(),
                            ControlMsg::Pause {
                                transition: ControlTransition { fade_ms: 0 },
                            },
                            pause_cause,
                        ));
                        out.push((
                            rid,
                            ControlMsg::Unmute {
                                transition: ControlTransition { fade_ms: 0 },
                            },
                            "state-switch",
                        ));
                    }
                    _ => {}
                }
            }
            out
        };
        let mut changed_ids: Vec<RendererId> = Vec::new();
        for (id, _, _) in &actions {
            if !changed_ids.contains(id) {
                changed_ids.push(id.clone());
            }
        }
        for (id, msg, cause) in actions {
            let resume_control = ResumeControl::from_message(&msg);
            let label = lifecycle_control_label(&msg);
            if let Err(e) = self.mgr.send_control(&id, msg).await {
                log::warn!("{label} renderer {id}: {e}");
                if let Some(control) = resume_control {
                    self.schedule_resume_retry(&id, control).await;
                }
            } else {
                log::info!("{label} renderer {id} ({cause})");
                if resume_control.is_some() {
                    self.clear_resume_retry(&id).await;
                }
            }
        }
        let display_ids: Vec<DisplayId> = {
            let inner = self.inner.lock().await;
            inner.displays.keys().copied().collect()
        };
        for display_id in display_ids {
            self.reconcile_presentation_config(display_id).await;
        }
        self.refresh_runtime_health().await;
        for id in changed_ids {
            if let Some(snap) = self.snapshot_renderer(&id).await {
                self.emit(RouterEvent::RendererUpsert(snap));
            }
        }
    }

    async fn clear_resume_retry(&self, renderer_id: &str) {
        let mut inner = self.inner.lock().await;
        inner.resume_retries.remove(renderer_id);
        if let Some(task) = inner.resume_retry_tasks.remove(renderer_id) {
            task.abort();
        }
    }

    async fn schedule_resume_retry(self: &Arc<Self>, renderer_id: &str, control: ResumeControl) {
        let scheduled = {
            let mut inner = self.inner.lock().await;
            if inner
                .renderer_slots
                .get(renderer_id)
                .is_none_or(|slot| slot.state.activity() != Some(RendererActivity::Playing))
                || inner.table.get_renderer(renderer_id).is_none()
            {
                inner.resume_retries.remove(renderer_id);
                None
            } else if inner.resume_retries.contains_key(renderer_id) {
                None
            } else {
                inner.next_resume_retry_generation =
                    inner.next_resume_retry_generation.wrapping_add(1);
                let generation = inner.next_resume_retry_generation;
                inner.resume_retries.insert(
                    renderer_id.to_string(),
                    ResumeRetry {
                        control,
                        failures: 1,
                        generation,
                    },
                );
                Some(generation)
            }
        };
        let Some(generation) = scheduled else {
            return;
        };
        let delay = resume_retry_delay(1);
        log::warn!(
            "router: {} {renderer_id} failed 1 time(s); retrying in {delay:?}",
            control.label()
        );
        let router = Arc::clone(self);
        let renderer_id = renderer_id.to_string();
        let task_renderer_id = renderer_id.clone();
        let task = tokio::spawn(async move {
            router
                .run_renderer_resume_retry(task_renderer_id, generation)
                .await;
        });
        self.inner
            .lock()
            .await
            .resume_retry_tasks
            .insert(renderer_id.to_string(), task);
    }

    async fn run_renderer_resume_retry(self: &Arc<Self>, renderer_id: RendererId, generation: u64) {
        loop {
            let (control, failures) = {
                let mut inner = self.inner.lock().await;
                let Some(retry) = inner.resume_retries.get(&renderer_id).copied() else {
                    return;
                };
                if retry.generation != generation {
                    return;
                }
                if inner
                    .renderer_slots
                    .get(&renderer_id)
                    .is_none_or(|slot| slot.state.activity() != Some(RendererActivity::Playing))
                    || inner.table.get_renderer(&renderer_id).is_none()
                {
                    inner.resume_retries.remove(&renderer_id);
                    return;
                }
                (retry.control, retry.failures)
            };

            tokio::time::sleep(resume_retry_delay(failures)).await;

            let _lifecycle = self.lifecycle_lock.lock().await;
            {
                let mut inner = self.inner.lock().await;
                let Some(retry) = inner.resume_retries.get(&renderer_id).copied() else {
                    return;
                };
                if retry.generation != generation {
                    return;
                }
                if inner
                    .renderer_slots
                    .get(&renderer_id)
                    .is_none_or(|slot| slot.state.activity() != Some(RendererActivity::Playing))
                    || inner.table.get_renderer(&renderer_id).is_none()
                {
                    inner.resume_retries.remove(&renderer_id);
                    return;
                }
            }

            match self
                .mgr
                .send_control(&renderer_id, control.into_message())
                .await
            {
                Ok(()) => {
                    let mut inner = self.inner.lock().await;
                    if inner
                        .resume_retries
                        .get(&renderer_id)
                        .is_some_and(|retry| retry.generation == generation)
                    {
                        inner.resume_retries.remove(&renderer_id);
                    }
                    log::info!(
                        "router: {} renderer {renderer_id} retry succeeded",
                        control.label()
                    );
                    return;
                }
                Err(e) => {
                    let failures = {
                        let mut inner = self.inner.lock().await;
                        let Some(retry) = inner.resume_retries.get_mut(&renderer_id) else {
                            return;
                        };
                        if retry.generation != generation {
                            return;
                        }
                        retry.failures = retry.failures.saturating_add(1);
                        retry.failures
                    };
                    let delay = resume_retry_delay(failures);
                    log::warn!(
                        "router: {} {renderer_id} failed {failures} time(s): {e}; retrying in {delay:?}",
                        control.label()
                    );
                }
            }
        }
    }

    /// Re-run the modifier picker for every (renderer, display) link
    /// the router knows about.
    async fn reconcile_buffer_flags(self: &Arc<Self>) {
        // Snapshot caps under the inner lock; pick() is pure and runs
        // outside the critical section.
        struct Pair {
            rid: RendererId,
            did: DisplayId,
            producer: crate::wallframe::dma::negotiate::PeerCaps,
            consumer: crate::wallframe::dma::negotiate::PeerCaps,
        }
        let pairs: Vec<Pair> = {
            let inner = self.inner.lock().await;
            let mut out = Vec::new();
            for rid in inner.table.renderer_ids() {
                let Some(renderer) = inner.table.get_renderer(&rid) else {
                    continue;
                };
                let Some(producer_caps) = renderer.format_caps() else {
                    continue; // legacy renderer — skip silently
                };
                for link in inner.table.links_for_renderer(&rid) {
                    if !link.enabled {
                        continue;
                    }
                    let Some(state) = inner.displays.get(&link.display_id) else {
                        continue;
                    };
                    out.push(Pair {
                        rid: rid.clone(),
                        did: link.display_id,
                        producer: producer_caps.clone(),
                        consumer: state.consumer_caps.clone(),
                    });
                }
            }
            out
        };
        // Dispatch the picked scheme via NegotiateBuffers; for fan-out,
        // the last compatible per-display pick currently wins.
        let mut by_renderer: std::collections::HashMap<
            RendererId,
            crate::wallframe::dma::negotiate::NegotiatedScheme,
        > = std::collections::HashMap::new();
        for p in pairs {
            match crate::wallframe::dma::negotiate::pick(&p.producer, &p.consumer) {
                Ok(scheme) => {
                    log::info!(
                        "router: pick({rid}, display {did}) = \
                         path={path:?} mem_source={ms:?} \
                         fourcc=0x{fourcc:08x} modifier=0x{modifier:x} \
                         plane_count={pc} sync=0x{sync:x} color=0x{color:x} \
                         mem_hint=0x{mem:x} count={count}",
                        rid = p.rid,
                        did = p.did,
                        path = scheme.path,
                        ms = scheme.mem_source,
                        fourcc = scheme.fourcc,
                        modifier = scheme.modifier,
                        pc = scheme.plane_count,
                        sync = scheme.sync_mode,
                        color = scheme.color,
                        mem = scheme.mem_hint,
                        count = scheme.count,
                    );
                    by_renderer.insert(p.rid.clone(), scheme);
                }
                Err(e) => {
                    log::warn!(
                        "router: pick({rid}, display {did}) failed: {e:?}",
                        rid = p.rid,
                        did = p.did,
                    );
                }
            }
        }
        // Outside the inner lock — send_negotiate_buffers takes its own.
        for (rid, scheme) in by_renderer {
            if let Err(e) = self.mgr.send_negotiate_buffers(&rid, scheme).await {
                log::warn!("router: NegotiateBuffers {rid}: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wallframe::renderer_manager::RendererManager;

    #[test]
    fn lifecycle_control_labels_name_the_actual_action() {
        let transition = || ControlTransition { fade_ms: 0 };
        assert_eq!(
            lifecycle_control_label(&ControlMsg::Pause {
                transition: transition()
            }),
            "pause"
        );
        assert_eq!(
            lifecycle_control_label(&ControlMsg::Play {
                transition: transition()
            }),
            "play"
        );
        assert_eq!(
            lifecycle_control_label(&ControlMsg::Mute {
                transition: transition()
            }),
            "mute"
        );
        assert_eq!(
            lifecycle_control_label(&ControlMsg::Unmute {
                transition: transition()
            }),
            "unmute"
        );
    }

    fn progress_at(
        now: Instant,
        first_frame: bool,
    ) -> crate::wallframe::renderer_manager::RendererProgressSnapshot {
        crate::wallframe::renderer_manager::RendererProgressSnapshot {
            registered_at: now,
            bind_at: Some(now),
            buffer_generation: Some(1),
            first_frame_at: first_frame.then_some(now),
            last_frame_at: first_frame.then_some(now),
        }
    }

    fn release_wait_at(now: Instant) -> ReleaseWaitFact {
        let frame = crate::wallframe::sync::FrameIdentity {
            buffer_generation: 1,
            buffer_index: 0,
            release_point: 1,
        };
        ReleaseWaitFact {
            consumer: crate::wallframe::sync::FrameConsumerIdentity {
                frame,
                renderer_id: "r1".into(),
                display_id: 7,
                display_session_id: 70,
                display_name: "lock-screen".into(),
                frame_seq: 1,
            },
            state: crate::wallframe::sync::ReleaseWaitState::Armed,
            since: now,
        }
    }

    #[test]
    fn runtime_loading_clears_on_first_frame() {
        let now = Instant::now();
        let loading = evaluate_renderer_conditions(
            now,
            "r1",
            RendererActivity::Playing,
            true,
            RendererActivityMode::Continuous,
            progress_at(now, false),
            std::iter::empty(),
            None,
        );
        assert!(loading.iter().any(|condition| {
            condition.kind == RuntimeConditionKind::Loading && condition.reason == "first_frame"
        }));

        let ready = evaluate_renderer_conditions(
            now,
            "r1",
            RendererActivity::Playing,
            true,
            RendererActivityMode::Continuous,
            progress_at(now, true),
            std::iter::empty(),
            None,
        );
        assert!(!ready
            .iter()
            .any(|condition| condition.kind == RuntimeConditionKind::Loading));
    }

    #[test]
    fn runtime_release_wait_uses_soft_and_hard_deadlines() {
        let started = Instant::now();
        let evaluate = |elapsed| {
            evaluate_renderer_conditions(
                started + elapsed,
                "r1",
                RendererActivity::Playing,
                true,
                RendererActivityMode::OnDemand,
                progress_at(started, true),
                std::iter::once(release_wait_at(started)),
                None,
            )
        };
        assert!(!evaluate(Duration::from_millis(1999))
            .iter()
            .any(|condition| condition.kind == RuntimeConditionKind::Waiting));
        assert!(evaluate(Duration::from_secs(2))
            .iter()
            .any(|condition| condition.kind == RuntimeConditionKind::Waiting));
        let hard = evaluate(Duration::from_secs(10));
        assert!(hard
            .iter()
            .any(|condition| condition.kind == RuntimeConditionKind::Waiting));
        assert!(hard
            .iter()
            .any(|condition| condition.kind == RuntimeConditionKind::Hang));
    }

    #[test]
    fn runtime_frame_hang_is_gated_by_activity_lifecycle_and_audience() {
        let started = Instant::now();
        let now = started + Duration::from_secs(10);
        let has_frame_hang = |activity, status, audience| {
            evaluate_renderer_conditions(
                now,
                "r1",
                status,
                audience,
                activity,
                progress_at(started, true),
                std::iter::empty(),
                None,
            )
            .iter()
            .any(|condition| condition.reason == "frame_progress")
        };
        assert!(has_frame_hang(
            RendererActivityMode::Continuous,
            RendererActivity::Playing,
            true
        ));
        assert!(!has_frame_hang(
            RendererActivityMode::OnDemand,
            RendererActivity::Playing,
            true
        ));
        assert!(!has_frame_hang(
            RendererActivityMode::Continuous,
            RendererActivity::Paused,
            true
        ));
        assert!(!has_frame_hang(
            RendererActivityMode::Continuous,
            RendererActivity::Playing,
            false
        ));
    }

    fn reg(name: &str, w: u32, h: u32) -> DisplayRegistration {
        use crate::wallframe::dma::negotiate as N;
        DisplayRegistration {
            name: name.into(),
            instance_id: None,
            metrics: DisplayMetrics {
                width: w,
                height: h,
                refresh_mhz: 60_000,
            },
            presentation_caps: 0,
            consumer_caps: build_caps(N::DRM_FORMAT_ABGR8888, &[(N::DRM_FORMAT_MOD_LINEAR, 1)], 0),
            window_state_flags: 0,
        }
    }

    async fn retained_renderer_with_display(router: &Arc<Router>) -> DisplayId {
        let display = router.register_display(reg("DP-1", 1920, 1080)).await;
        let mut inner = router.inner.lock().await;
        inner.renderer_slots.insert(
            "r1".into(),
            RendererSlot::retained(
                crate::wallframe::renderer_manager::SpawnRequest {
                    wp_type: "image".into(),
                    renderer_name: Some("image".into()),
                    ..Default::default()
                },
                "image".into(),
            ),
        );
        inner.table.add_link("r1".into(), display.id);
        display.id
    }

    #[tokio::test]
    async fn duplicate_renderers_each_get_their_own_display_size() {
        let router = Router::new(Arc::new(RendererManager::new_default()));
        let a = router.register_display(reg("DP-1", 1920, 1080)).await;
        let b = router.register_display(reg("DP-2", 3440, 1440)).await;

        // Start attempt fails (no registered renderer def); irrelevant here.
        let _ = router
            .apply_assignment(ApplyAssignment {
                spawn_request: crate::wallframe::renderer_manager::SpawnRequest {
                    wp_type: "web".into(),
                    renderer_name: Some("web".into()),
                    ..Default::default()
                },
                display_ids: vec![a.id, b.id],
                duplicate_renderers: true,
                wallpaper_layout_override: WallpaperLayoutOverride::default(),
                preempt_pending_start: false,
            })
            .await;

        let inner = router.inner.lock().await;
        let renderer_for = |display_id: DisplayId| {
            inner
                .table
                .links_for_display(display_id)
                .first()
                .expect("display linked")
                .renderer_id
                .clone()
        };
        let size_for = |renderer_id: &str| {
            inner
                .renderer_slots
                .get(renderer_id)
                .expect("renderer slot")
                .spawn_request
                .display_size
        };

        let renderer_a = renderer_for(a.id);
        let renderer_b = renderer_for(b.id);
        assert_ne!(
            renderer_a, renderer_b,
            "duplicate_renderers must not share one renderer"
        );
        assert_eq!(size_for(&renderer_a), Some((1920, 1080)));
        assert_eq!(size_for(&renderer_b), Some((3440, 1440)));
    }

    #[tokio::test(start_paused = true)]
    async fn auto_replay_start_waits_for_a_stable_window() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        retained_renderer_with_display(&router).await;

        router
            .request_renderer_start("r1", RendererStartCause::AutoReplayResume)
            .await
            .unwrap();
        tokio::time::advance(Duration::from_millis(1999)).await;
        tokio::task::yield_now().await;

        let snapshot = router.snapshot_renderer("r1").await.unwrap();
        assert!(matches!(
            snapshot.state,
            RendererLifecycleState::Stopped { keep: true, .. }
        ));
        assert!(mgr.get("r1").await.is_none());

        tokio::time::advance(Duration::from_millis(1)).await;
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        assert!(matches!(
            router.snapshot_renderer("r1").await.unwrap().state,
            RendererLifecycleState::Failed { .. }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn stopping_cancels_and_a_later_resume_restarts_the_window() {
        let router = Router::new(Arc::new(RendererManager::new_default()));
        retained_renderer_with_display(&router).await;

        router
            .request_renderer_start("r1", RendererStartCause::AutoReplayResume)
            .await
            .unwrap();
        let first = router
            .inner
            .lock()
            .await
            .renderer_slots
            .get("r1")
            .unwrap()
            .pending_start
            .unwrap();
        tokio::time::advance(Duration::from_secs(1)).await;
        router.begin_retained_stop("r1").await;
        assert!(router
            .inner
            .lock()
            .await
            .renderer_slots
            .get("r1")
            .is_some_and(|slot| slot.pending_start.is_none()));

        router
            .request_renderer_start("r1", RendererStartCause::AutoReplayResume)
            .await
            .unwrap();
        let second = router
            .inner
            .lock()
            .await
            .renderer_slots
            .get("r1")
            .unwrap()
            .pending_start
            .unwrap();
        assert_ne!(first.token, second.token);
        assert!(second.not_before > first.not_before);

        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert!(matches!(
            router.snapshot_renderer("r1").await.unwrap().state,
            RendererLifecycleState::Stopped { keep: true, .. }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn background_apply_updates_spec_without_resetting_replay_deadline() {
        let router = Router::new(Arc::new(RendererManager::new_default()));
        let display_id = retained_renderer_with_display(&router).await;
        router
            .request_renderer_start("r1", RendererStartCause::AutoReplayResume)
            .await
            .unwrap();
        let pending = router
            .inner
            .lock()
            .await
            .renderer_slots
            .get("r1")
            .unwrap()
            .pending_start
            .unwrap();
        let mut spawn_request = crate::wallframe::renderer_manager::SpawnRequest {
            wp_type: "image".into(),
            renderer_name: Some("image".into()),
            ..Default::default()
        };
        spawn_request
            .extras
            .insert("path".into(), "/latest.png".into());

        let receipt = router
            .apply_assignment(ApplyAssignment {
                spawn_request,
                display_ids: vec![display_id],
                duplicate_renderers: false,
                wallpaper_layout_override: WallpaperLayoutOverride::default(),
                preempt_pending_start: false,
            })
            .await
            .unwrap();

        assert_eq!(receipt.activation, AssignmentActivation::Deferred);
        let inner = router.inner.lock().await;
        let slot = inner.renderer_slots.get("r1").unwrap();
        let current = slot.pending_start.unwrap();
        assert_eq!(current.token, pending.token);
        assert_eq!(current.not_before, pending.not_before);
        assert_eq!(
            slot.spawn_request.extras.get("path").unwrap(),
            "/latest.png"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn interactive_apply_preempts_pending_replay_start() {
        let router = Router::new(Arc::new(RendererManager::new_default()));
        let display_id = retained_renderer_with_display(&router).await;
        router
            .request_renderer_start("r1", RendererStartCause::AutoReplayResume)
            .await
            .unwrap();

        let result = router
            .apply_assignment(ApplyAssignment {
                spawn_request: crate::wallframe::renderer_manager::SpawnRequest {
                    wp_type: "video".into(),
                    renderer_name: Some("video".into()),
                    ..Default::default()
                },
                display_ids: vec![display_id],
                duplicate_renderers: false,
                wallpaper_layout_override: WallpaperLayoutOverride::default(),
                preempt_pending_start: true,
            })
            .await;

        assert!(matches!(
            result,
            Err(crate::error::Error::RendererNotFound(renderer)) if renderer == "video"
        ));
        let inner = router.inner.lock().await;
        let slot = inner.renderer_slots.get("r1").unwrap();
        assert!(slot.pending_start.is_none());
        assert!(matches!(slot.state, RendererLifecycleState::Failed { .. }));
    }

    #[tokio::test(start_paused = true)]
    async fn interactive_apply_does_not_bypass_auto_stop() {
        let router = Router::new(Arc::new(RendererManager::new_default()));
        let display_id = retained_renderer_with_display(&router).await;
        {
            let mut inner = router.inner.lock().await;
            inner
                .displays
                .get_mut(&display_id)
                .unwrap()
                .auto_replay
                .stop_applied = true;
            for link in inner.table.links_for_display(display_id) {
                inner.table.set_link_enabled(link.id, false);
            }
        }

        let receipt = router
            .apply_assignment(ApplyAssignment {
                spawn_request: crate::wallframe::renderer_manager::SpawnRequest {
                    wp_type: "video".into(),
                    renderer_name: Some("video".into()),
                    ..Default::default()
                },
                display_ids: vec![display_id],
                duplicate_renderers: false,
                wallpaper_layout_override: WallpaperLayoutOverride::default(),
                preempt_pending_start: true,
            })
            .await
            .unwrap();

        assert_eq!(receipt.activation, AssignmentActivation::Deferred);
        assert!(matches!(
            router.snapshot_renderer("r1").await.unwrap().state,
            RendererLifecycleState::Stopped { keep: true, .. }
        ));
    }

    #[tokio::test]
    async fn explicit_spawn_uses_lifecycle_and_preserves_typed_errors() {
        let router = Router::new(Arc::new(RendererManager::new_default()));
        let result = router
            .spawn_renderer(crate::wallframe::renderer_manager::SpawnRequest {
                wp_type: "video".into(),
                renderer_name: Some("video".into()),
                ..Default::default()
            })
            .await;

        assert!(matches!(
            result,
            Err(crate::error::Error::RendererNotFound(renderer)) if renderer == "video"
        ));
        assert!(router.snapshot_renderers().await.is_empty());
    }

    async fn stopping_renderer_with_display(mgr: &Arc<RendererManager>, router: &Arc<Router>) {
        let renderer = RendererHandle::test_stub("r1", "image");
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;
        router.register_display(reg("DP-1", 1920, 1080)).await;
        router.begin_retained_stop("r1").await;
    }

    #[tokio::test(start_paused = true)]
    async fn elapsed_replay_deadline_waits_for_stopping_process_exit() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        stopping_renderer_with_display(&mgr, &router).await;
        router
            .request_renderer_start("r1", RendererStartCause::AutoReplayResume)
            .await
            .unwrap();

        tokio::time::advance(lifecycle::AUTO_REPLAY_START_DELAY).await;
        tokio::task::yield_now().await;
        let slot = router
            .inner
            .lock()
            .await
            .renderer_slots
            .get("r1")
            .unwrap()
            .state
            .clone();
        assert!(matches!(
            slot,
            RendererLifecycleState::Stopping { keep: true, .. }
        ));

        let exit = mgr.stop("r1").await.unwrap();
        router.on_renderer_process_exit(exit).await;
        assert!(matches!(
            router.snapshot_renderer("r1").await.unwrap().state,
            RendererLifecycleState::Failed { .. }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn stopped_process_keeps_the_remaining_replay_deadline() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        stopping_renderer_with_display(&mgr, &router).await;
        router
            .request_renderer_start("r1", RendererStartCause::AutoReplayResume)
            .await
            .unwrap();

        let exit = mgr.stop("r1").await.unwrap();
        router.on_renderer_process_exit(exit).await;
        tokio::time::advance(Duration::from_millis(1999)).await;
        tokio::task::yield_now().await;
        assert!(matches!(
            router.snapshot_renderer("r1").await.unwrap().state,
            RendererLifecycleState::Stopped { keep: true, .. }
        ));

        tokio::time::advance(Duration::from_millis(1)).await;
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        assert!(matches!(
            router.snapshot_renderer("r1").await.unwrap().state,
            RendererLifecycleState::Failed { .. }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn killed_renderer_restart_uses_the_shared_deadline_scheduler() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let renderer = RendererHandle::test_stub("r1", "image");
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;
        router.register_display(reg("DP-1", 1920, 1080)).await;

        let mut exit = mgr.stop("r1").await.unwrap();
        exit.kind = crate::wallframe::renderer_manager::RendererProcessExitKind::Killed;
        router.on_renderer_process_exit(exit).await;
        let pending = router
            .inner
            .lock()
            .await
            .renderer_slots
            .get("r1")
            .unwrap()
            .pending_start
            .unwrap();
        assert_eq!(pending.cause, RendererStartCause::ProcessRestart);

        tokio::time::advance(Duration::from_millis(99)).await;
        tokio::task::yield_now().await;
        assert!(matches!(
            router.snapshot_renderer("r1").await.unwrap().state,
            RendererLifecycleState::Killed { keep: true, .. }
        ));
        tokio::time::advance(Duration::from_millis(1)).await;
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        assert!(matches!(
            router.snapshot_renderer("r1").await.unwrap().state,
            RendererLifecycleState::Failed { .. }
        ));
    }

    #[tokio::test]
    async fn snapshot_displays_empty() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr);
        assert!(router.snapshot_displays().await.is_empty());
    }

    #[tokio::test]
    async fn runtime_conditions_are_projected_to_linked_displays() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(Arc::clone(&mgr));
        let renderer = RendererHandle::test_stub("r-health", "image");
        mgr.register_test_handle(Arc::clone(&renderer)).await;
        router.register_renderer(renderer).await;
        let display = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;

        let renderer_snapshot = router.snapshot_renderer("r-health").await.unwrap();
        let display_snapshot = router.snapshot_display(display.id).await.unwrap();
        assert!(renderer_snapshot.conditions.iter().any(|condition| {
            condition.kind == RuntimeConditionKind::Loading && condition.reason == "first_frame"
        }));
        assert_eq!(display_snapshot.conditions, renderer_snapshot.conditions);
    }

    #[tokio::test]
    async fn poisoned_generation_recovery_is_single_flight() {
        let router = Router::new(Arc::new(RendererManager::new_default()));
        let consumer = release_wait_at(Instant::now()).consumer;
        let event = || crate::wallframe::sync::ReleaseEvent::GenerationPoisoned {
            consumer: consumer.clone(),
            reason: "kernel state unavailable".into(),
        };

        assert!(router.record_release_event(event()).await);
        assert!(!router.record_release_event(event()).await);
    }

    #[tokio::test]
    async fn snapshot_displays_sorted_by_id_with_metadata() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr);

        // register_display has no registered renderer, so no auto-link —
        // each display shows up with an empty link vector.
        let _h1 = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;
        let _h2 = router.register_display(reg("DP-1", 2560, 1440)).await;
        let _h3 = router.register_display(reg("eDP-1", 1366, 768)).await;

        let snap = router.snapshot_displays().await;
        assert_eq!(snap.len(), 3);

        // Stable ascending ordering by id — matches register order here.
        let ids: Vec<u64> = snap.iter().map(|d| d.id).collect();
        assert_eq!(ids, vec![1, 2, 3]);

        // Metadata round-trips unchanged.
        assert_eq!(snap[0].name, "HDMI-A-1");
        assert_eq!((snap[0].width, snap[0].height), (1920, 1080));
        assert_eq!(snap[1].name, "DP-1");
        assert_eq!((snap[1].width, snap[1].height), (2560, 1440));
        assert_eq!(snap[2].name, "eDP-1");
        assert_eq!((snap[2].width, snap[2].height), (1366, 768));

        // No renderers registered → every link vector is empty.
        for d in &snap {
            assert!(
                d.links.is_empty(),
                "display {} unexpectedly has links",
                d.id
            );
        }
    }

    fn reg_iid(name: &str, iid: &str) -> DisplayRegistration {
        let mut registration = reg(name, 1920, 1080);
        registration.instance_id = Some(iid.into());
        registration
    }

    async fn test_settings_store() -> Arc<crate::settings::SettingsStore> {
        let tmp = tempfile::tempdir().unwrap();
        let store =
            crate::settings::SettingsStore::load_or_default(tmp.path().join("settings.toml")).await;
        std::mem::forget(tmp);
        store
    }

    #[tokio::test]
    async fn display_settings_keys_prefers_instance_id() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr);

        let h1 = router.register_display(reg_iid("HDMI-A-1", "uuid-1")).await;
        let h2 = router.register_display(reg("DP-1", 2560, 1440)).await;

        let keys = router.display_settings_keys(&[h1.id, h2.id]).await;
        assert_eq!(keys, vec![(h1.id, "uuid-1".into()), (h2.id, "DP-1".into())]);

        // Unknown ids are dropped.
        let keys = router.display_settings_keys(&[h1.id, 9999]).await;
        assert_eq!(keys, vec![(h1.id, "uuid-1".into())]);
    }

    #[tokio::test]
    async fn registered_display_ids_and_renderer_display_ids_are_sorted() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());

        let r = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r).await;
        let h1 = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;
        let h2 = router.register_display(reg("DP-1", 2560, 1440)).await;

        assert_eq!(
            router.registered_display_ids(None).await,
            vec![h1.id, h2.id]
        );
        assert_eq!(
            router
                .registered_display_ids(Some(&[h2.id, 9999, h1.id, h2.id]))
                .await,
            vec![h1.id, h2.id]
        );
        assert_eq!(router.renderer_display_ids("r1").await, vec![h1.id, h2.id]);
    }

    #[tokio::test]
    async fn display_layout_set_targets_display_id_when_names_collide() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let settings = test_settings_store().await;
        router.attach_settings(settings.clone());

        let r = RendererHandle::test_stub("r1", "scene");
        r.test_publish_pool(fake_published_pool(1, 1920, 1080));
        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r.clone()).await;

        let mut h1 = router
            .register_display(reg_iid("KDE Screen", "iid-1"))
            .await;
        let mut h2 = router
            .register_display(reg_iid("KDE Screen", "iid-2"))
            .await;
        let _ = last_composition_config(&mut h1.rx);
        let _ = last_composition_config(&mut h2.rx);

        let target = router
            .set_display_layout(
                Some(h2.id),
                "KDE Screen".into(),
                Some(FillMode::PreserveAspectFit),
                None,
                None,
                None,
                false,
                false,
                false,
            )
            .await;

        assert_eq!(target, Some(h2.id));
        assert!(last_composition_config(&mut h1.rx).is_none());
        assert!(last_composition_config(&mut h2.rx).is_some());

        let snap = settings.snapshot();
        assert_eq!(
            snap.displays.get("iid-2").and_then(|p| p.fillmode),
            Some(FillMode::PreserveAspectFit)
        );
        assert!(snap.displays.get("iid-1").is_none());
        assert!(snap.displays.get("KDE Screen").is_none());
    }

    #[tokio::test]
    async fn snapshot_reflects_display_unregister() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr);

        let h1 = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;
        let h2 = router.register_display(reg("DP-1", 2560, 1440)).await;
        assert_eq!(router.snapshot_displays().await.len(), 2);

        router.unregister_display(h1.id).await;
        let snap = router.snapshot_displays().await;
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].id, h2.id);
        assert_eq!(snap[0].name, "DP-1");
    }

    // -----------------------------------------------------------------
    // Orphan reaping

    /// Register a stub renderer with both the manager and the router
    /// so apply-side lookups can find it in both ownership structures.
    async fn add_stub_renderer(mgr: &Arc<RendererManager>, router: &Arc<Router>, id: &str) {
        let h = RendererHandle::test_stub(id, "scene");
        mgr.register_test_handle(h.clone()).await;
        router.register_renderer(h).await;
    }

    /// Are these ids still in the manager's live list?
    async fn live_renderers(mgr: &Arc<RendererManager>) -> Vec<RendererId> {
        let mut ids = mgr.list().await;
        ids.sort();
        ids
    }

    /// Yield enough times that any spawned task chains awaiting on
    /// inner-lock + spawn_blocking + child-wait paths can complete.
    async fn drain_executor() {
        for _ in 0..256 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn renderers_fully_replaced_by_target_subset() {
        // r1 binds {A, B}, r2 binds {C}. relink target {A, B}: r1 is
        // fully replaced because every enabled link is in the target.
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        add_stub_renderer(&mgr, &router, "r1").await;
        add_stub_renderer(&mgr, &router, "r2").await;
        let a = router.register_display(reg("A", 1920, 1080)).await;
        let b = router.register_display(reg("B", 1920, 1080)).await;
        let c = router.register_display(reg("C", 1920, 1080)).await;
        // Initial auto-link picks the first renderer ("r1") for every
        // display. Move C onto r2.
        router.relink_displays_to(&[c.id], "r2").await;
        drain_executor().await;
        // After this point the table is: r1 ↔ {A, B}, r2 ↔ {C}.

        let mut killable = router
            .renderers_fully_replaced_by(Some(&[a.id, b.id]))
            .await;
        killable.sort();
        assert_eq!(
            killable,
            vec!["r1".to_string()],
            "only r1's enabled links are within {{A,B}}",
        );

        let mut all = router.renderers_fully_replaced_by(None).await;
        all.sort();
        assert_eq!(
            all,
            vec!["r1".to_string(), "r2".to_string()],
            "target=None means relink_all → every renderer gets fully replaced",
        );
    }

    #[tokio::test]
    async fn reusable_renderer_is_selected_by_slot_identity() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        add_stub_renderer(&mgr, &router, "r2").await;
        add_stub_renderer(&mgr, &router, "r1").await;
        let request = crate::wallframe::renderer_manager::SpawnRequest {
            wp_type: "scene".into(),
            ..Default::default()
        };

        assert_eq!(
            router
                .reusable_renderer_for_target(&request, &[], false)
                .await
                .as_deref(),
            Some("r1")
        );

        let mut different = request;
        different.extras.insert("path".into(), "/other".into());
        assert!(router
            .reusable_renderer_for_target(&different, &[], false)
            .await
            .is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn stop_renderers_unregisters_and_kills() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        add_stub_renderer(&mgr, &router, "r1").await;
        add_stub_renderer(&mgr, &router, "r2").await;
        router.stop_renderers(&["r1".to_string()]).await;
        drain_executor().await;
        assert_eq!(live_renderers(&mgr).await, vec!["r2".to_string()]);
    }

    #[tokio::test(start_paused = true)]
    async fn reap_kills_orphan_after_relink_all() {
        // Single display starts on r1; relink_all → r2 must reap r1
        // immediately because the daemon still has a display.
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        add_stub_renderer(&mgr, &router, "r1").await;
        add_stub_renderer(&mgr, &router, "r2").await;

        let _h = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;
        // r1 was registered first → first_renderer() picked it for the auto-link.
        router.relink_all_displays_to("r2").await;
        drain_executor().await;

        let live = live_renderers(&mgr).await;
        assert_eq!(
            live,
            vec!["r2".to_string()],
            "r1 must be reaped immediately — display present, so no grace"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn reap_keeps_renderer_still_referenced() {
        // Two displays both on r1. Relink only display A → r2; r1 must
        // survive because display B still uses it.
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        add_stub_renderer(&mgr, &router, "r1").await;
        add_stub_renderer(&mgr, &router, "r2").await;

        let a = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;
        let _b = router.register_display(reg("DP-1", 1920, 1080)).await;

        router.relink_displays_to(&[a.id], "r2").await;
        drain_executor().await;
        // r1 is alive — display B still links it.
        let live = live_renderers(&mgr).await;
        assert_eq!(live, vec!["r1".to_string(), "r2".to_string()]);

        // Now move display B over too — r1 fully orphaned; reaped
        // immediately (displays present + 2 renderers → no grace).
        router.relink_all_displays_to("r2").await;
        drain_executor().await;
        let live = live_renderers(&mgr).await;
        assert_eq!(live, vec!["r2".to_string()]);
    }

    #[tokio::test(start_paused = true)]
    async fn relink_all_with_zero_displays_replaces_old_renderer() {
        // Apply path semantics with no displays attached: the current
        // renderer is preserved only while it is the lone renderer.
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());

        // First apply: r1 spawn + relink_all (no displays).
        add_stub_renderer(&mgr, &router, "r1").await;
        router.relink_all_displays_to("r1").await;
        assert_eq!(live_renderers(&mgr).await, vec!["r1".to_string()]);

        // Second apply: r2 spawn + relink_all (still no displays).
        add_stub_renderer(&mgr, &router, "r2").await;
        router.relink_all_displays_to("r2").await;
        drain_executor().await;
        assert_eq!(
            live_renderers(&mgr).await,
            vec!["r2".to_string()],
            "r1 must be reaped immediately — 2 renderers means no grace",
        );
        tokio::time::advance(Duration::from_secs(6)).await;
        drain_executor().await;
        assert_eq!(
            live_renderers(&mgr).await,
            vec!["r2".to_string()],
            "r1 must be reaped after the orphan grace window",
        );

        // Third apply: the slot identity lookup reuses r2.
        // and reuse r2; relink_all("r2") is a no-op + mark_orphans keeps r2.
        router.relink_all_displays_to("r2").await;
        drain_executor().await;
        tokio::time::advance(Duration::from_secs(6)).await;
        drain_executor().await;
        assert_eq!(live_renderers(&mgr).await, vec!["r2".to_string()]);
    }

    #[tokio::test(start_paused = true)]
    async fn unregister_last_display_reaps_after_grace() {
        // After all displays unplug, the lone renderer enters the
        // orphan grace window and can survive a quick hot-replug.
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        add_stub_renderer(&mgr, &router, "r1").await;
        let h = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;
        assert_eq!(live_renderers(&mgr).await, vec!["r1".to_string()]);

        router.unregister_display(h.id).await;
        drain_executor().await;
        // Hot-replug within the window: timer cancelled, r1 lives on.
        tokio::time::advance(Duration::from_secs(4)).await;
        drain_executor().await;
        let h2 = router.register_display(reg("DP-1", 1920, 1080)).await;
        let snap = router.snapshot_displays().await;
        let entry = snap.iter().find(|d| d.id == h2.id).unwrap();
        assert_eq!(entry.links.len(), 1);
        assert_eq!(entry.links[0].renderer_id, "r1");
        tokio::time::advance(Duration::from_secs(2)).await;
        drain_executor().await;
        assert_eq!(live_renderers(&mgr).await, vec!["r1".to_string()]);

        // Now unplug again and let the grace window elapse — r1 dies.
        router.unregister_display(h2.id).await;
        drain_executor().await;
        tokio::time::advance(Duration::from_secs(6)).await;
        drain_executor().await;
        assert!(
            live_renderers(&mgr).await.is_empty(),
            "renderer must be reaped past the orphan grace window",
        );
    }

    #[tokio::test(start_paused = true)]
    async fn mark_preserves_keep_id_with_no_displays() {
        // 0-display: spawn r1 → it has no link, but `keep=Some("r1")`
        // protects it from orphan reaping.
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        add_stub_renderer(&mgr, &router, "r1").await;
        let scheduled = router.mark_orphans(Some("r1")).await;
        assert!(scheduled.is_empty(), "keep id must not be marked");
        drain_executor().await;
        tokio::time::advance(Duration::from_secs(6)).await;
        drain_executor().await;
        assert_eq!(live_renderers(&mgr).await, vec!["r1".to_string()]);

        add_stub_renderer(&mgr, &router, "r2").await;
        let scheduled = router.mark_orphans(Some("r2")).await;
        assert_eq!(scheduled, vec!["r1".to_string()]);
        drain_executor().await;
        tokio::time::advance(Duration::from_secs(6)).await;
        drain_executor().await;
        assert_eq!(live_renderers(&mgr).await, vec!["r2".to_string()]);
    }

    #[tokio::test(start_paused = true)]
    async fn orphan_mark_then_cancel_keeps_renderer() {
        // Mark r1, advance 4s, cancel — r1 must outlive the original
        // 5s deadline.
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        add_stub_renderer(&mgr, &router, "r1").await;

        router.mark_orphan("r1".to_string()).await;
        drain_executor().await;
        tokio::time::advance(Duration::from_secs(4)).await;
        drain_executor().await;
        router.cancel_orphan_timer("r1").await;
        tokio::time::advance(Duration::from_secs(2)).await;
        drain_executor().await;
        assert_eq!(live_renderers(&mgr).await, vec!["r1".to_string()]);
    }

    #[tokio::test(start_paused = true)]
    async fn orphan_mark_fires_after_grace() {
        // Mark r1, advance past 5s — r1 must be reaped.
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        add_stub_renderer(&mgr, &router, "r1").await;

        router.mark_orphan("r1".to_string()).await;
        drain_executor().await;
        tokio::time::advance(Duration::from_secs(6)).await;
        drain_executor().await;
        assert!(live_renderers(&mgr).await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn failed_orphan_is_retained_without_a_timer() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        add_stub_renderer(&mgr, &router, "r1").await;
        let generation = router
            .snapshot_renderer("r1")
            .await
            .unwrap()
            .state
            .generation()
            .unwrap();
        router
            .on_renderer_process_exit(crate::wallframe::renderer_manager::RendererProcessExit {
                renderer_id: "r1".into(),
                process_generation: generation,
                kind: crate::wallframe::renderer_manager::RendererProcessExitKind::Failed,
                code: Some(1),
                signal: None,
                reason: "initial failure".into(),
            })
            .await;

        assert!(router.mark_orphans(None).await.is_empty());
        assert!(!router.inner.lock().await.orphan_timers.contains_key("r1"));
        tokio::time::advance(Duration::from_secs(6)).await;
        drain_executor().await;
        assert!(matches!(
            router.snapshot_renderer("r1").await.unwrap().state,
            RendererLifecycleState::Failed { .. }
        ));
    }

    // -----------------------------------------------------------------
    // Active-sync RouterEvent::Renderer* emission

    async fn recv_event(rx: &mut broadcast::Receiver<RouterEvent>) -> Option<RouterEvent> {
        match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Ok(ev)) => Some(ev),
            _ => None,
        }
    }

    #[tokio::test]
    async fn renderer_upsert_on_register() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let mut rx = router.subscribe_events();

        add_stub_renderer(&mgr, &router, "R1").await;

        let evt = recv_event(&mut rx).await.expect("no event");
        match evt {
            RouterEvent::RendererUpsert(snap) => {
                assert_eq!(snap.id, "R1");
                assert_eq!(snap.wp_type, "scene");
                assert_eq!(snap.state.activity(), Some(RendererActivity::Paused));
                assert_eq!(snap.name, "test-stub");
            }
            other => panic!("expected RendererUpsert, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn runtime_tag_change_emits_renderer_upsert() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let handle = RendererHandle::test_stub("R1", "video");
        mgr.register_test_handle(handle.clone()).await;
        router.register_renderer(handle.clone()).await;
        let mut rx = router.subscribe_events();

        handle.test_set_runtime_tags(vec![
            crate::wallframe::renderer_manager::RendererRuntimeTag {
                key: "hwdec".to_string(),
                value: "vulkan".to_string(),
            },
        ]);

        let snapshots = router.snapshot_renderers().await;
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].runtime_tags[0].value, "vulkan");

        router
            .on_renderer_state_changed("R1", RENDERER_STATE_FIELD_RUNTIME_TAGS)
            .await;

        match recv_event(&mut rx).await.expect("no runtime-tag upsert") {
            RouterEvent::RendererUpsert(snapshot) => {
                assert_eq!(snapshot.runtime_tags.len(), 1);
                assert_eq!(snapshot.runtime_tags[0].key, "hwdec");
                assert_eq!(snapshot.runtime_tags[0].value, "vulkan");
            }
            event => panic!("expected RendererUpsert, got {event:?}"),
        }
    }

    #[tokio::test]
    async fn renderer_removed_on_unregister() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let mut rx = router.subscribe_events();

        add_stub_renderer(&mgr, &router, "R1").await;
        let _ = recv_event(&mut rx).await; // consume the RendererUpsert

        router.unregister_renderer("R1").await;
        for _ in 0..3 {
            let evt = recv_event(&mut rx).await.expect("no event");
            if let RouterEvent::RendererRemoved(id) = evt {
                assert_eq!(id, "R1");
                return;
            }
        }
        panic!("expected RendererRemoved");
    }

    #[tokio::test]
    async fn renderer_upsert_on_pause_transition() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());

        add_stub_renderer(&mgr, &router, "R1").await;
        let display = router.register_display(reg("D1", 1920, 1080)).await;

        // Subscribe *after* setup so we only observe the unregister path.
        let mut rx = router.subscribe_events();

        router.unregister_display(display.id).await;

        let mut saw_paused = false;
        for _ in 0..6 {
            let Some(evt) = recv_event(&mut rx).await else {
                break;
            };
            if let RouterEvent::RendererUpsert(snap) = evt {
                if snap.id == "R1" && snap.state.activity() == Some(RendererActivity::Paused) {
                    saw_paused = true;
                    break;
                }
            }
        }
        assert!(
            saw_paused,
            "expected R1 Paused upsert after display unregister"
        );
    }

    // -----------------------------------------------------------------
    // bind_failed + per-peer blacklist + retry

    /// Build a single-fourcc PeerCaps with the given (modifier,plane_count) list.
    /// Mirrors `negotiate::tests::caps_one_fourcc` but in scope here.
    fn build_caps(
        fourcc: u32,
        mods: &[(u64, u32)],
        uuid_byte: u8,
    ) -> crate::wallframe::dma::negotiate::PeerCaps {
        use crate::wallframe::dma::negotiate as N;
        let mod_count = mods.len() as u32;
        let modifiers: Vec<u64> = mods.iter().map(|(m, _)| *m).collect();
        let plane_counts: Vec<u32> = mods.iter().map(|(_, p)| *p).collect();
        let dev_words = [u32::from_le_bytes([uuid_byte; 4]); 4];
        let drv_words = [u32::from_le_bytes([uuid_byte; 4]); 4];
        N::unflatten_caps(
            &[fourcc],
            &[mod_count],
            &modifiers,
            &plane_counts,
            &dev_words,
            &drv_words,
            DrmNode {
                major: 226,
                minor: 128,
            },
            N::SYNC_SYNCOBJ_TIMELINE,
            N::DEFAULT_COLOR,
            N::MEM_HINT_HOST_VISIBLE,
            (1920, 1080),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn consumer_import_failure_resolves_binding_and_inserts_blacklist() {
        use crate::wallframe::dma::negotiate as N;
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let nl: u64 = 0x0100_0000_0000_0001;
        let (renderer, _peer) = RendererHandle::test_stub_with_peer("R1", "scene");
        renderer.test_set_format_caps(build_caps(
            N::DRM_FORMAT_ABGR8888,
            &[(N::DRM_FORMAT_MOD_LINEAR, 1), (nl, 1)],
            0xAA,
        ));
        let mut pool = fake_published_pool(1, 1920, 1080);
        pool.fourcc = N::DRM_FORMAT_ABGR8888;
        pool.modifier = nl;
        renderer.test_publish_pool(pool);
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;

        let mut registration = reg("D1", 1920, 1080);
        registration.consumer_caps = build_caps(
            N::DRM_FORMAT_ABGR8888,
            &[(N::DRM_FORMAT_MOD_LINEAR, 1), (nl, 1)],
            0xAA,
        );
        let h = router.register_display(registration).await;
        let generation = router.inner.lock().await.displays[&h.id]
            .binding
            .as_ref()
            .expect("display binding")
            .wire_generation;

        assert_eq!(
            router
                .on_consumer_import_failed(
                    h.id,
                    generation + 1,
                    ConsumerImportFailureKind::Unsupported,
                )
                .await,
            ConsumerImportFailureOutcome::Stale
        );
        assert_eq!(
            router
                .on_consumer_import_failed(
                    h.id,
                    generation,
                    ConsumerImportFailureKind::BackendFailure,
                )
                .await,
            ConsumerImportFailureOutcome::Terminal
        );
        assert_eq!(
            router
                .on_consumer_import_failed(
                    h.id,
                    generation,
                    ConsumerImportFailureKind::ResourceExhausted,
                )
                .await,
            ConsumerImportFailureOutcome::Terminal
        );
        assert!(router.inner.lock().await.displays[&h.id]
            .consumer_caps
            .blacklist
            .is_empty());

        let outcome = router
            .on_consumer_import_failed(h.id, generation, ConsumerImportFailureKind::Unsupported)
            .await;
        assert_eq!(
            outcome,
            ConsumerImportFailureOutcome::Retry {
                fourcc: N::DRM_FORMAT_ABGR8888,
                modifier: nl,
            }
        );

        let inner = router.inner.lock().await;
        let state = inner.displays.get(&h.id).unwrap();
        let bl = &state.consumer_caps.blacklist;
        assert!(bl.contains(&(N::DRM_FORMAT_ABGR8888, nl)));
        assert_eq!(state.failed_binding_generation, Some(generation));
    }

    #[tokio::test]
    async fn renderer_bind_failed_inserts_blacklist() {
        // Same shape as the consumer test, but on the producer side.
        use crate::wallframe::dma::negotiate as N;
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let (h, _peer) = RendererHandle::test_stub_with_peer("R1", "scene");
        let nl: u64 = 0x0100_0000_0000_0001;
        h.test_set_format_caps(build_caps(
            N::DRM_FORMAT_ABGR8888,
            &[(N::DRM_FORMAT_MOD_LINEAR, 1), (nl, 1)],
            0xAA,
        ));
        mgr.register_test_handle(h.clone()).await;
        router.register_renderer(h.clone()).await;

        assert_eq!(h.test_blacklist_len(), 0);
        router
            .on_renderer_bind_failed("R1", N::DRM_FORMAT_ABGR8888, nl)
            .await;
        assert_eq!(h.test_blacklist_len(), 1);
    }

    #[tokio::test]
    async fn picker_falls_back_after_consumer_blacklist() {
        // End-to-end: producer + consumer both advertise LINEAR + a
        // non-LINEAR modifier with a matching device UUID.
        use crate::wallframe::dma::negotiate as N;
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());

        let nl: u64 = 0x0100_0000_0000_0001;
        let (h, _peer) = RendererHandle::test_stub_with_peer("R1", "scene");
        h.test_set_format_caps(build_caps(
            N::DRM_FORMAT_ABGR8888,
            &[(N::DRM_FORMAT_MOD_LINEAR, 1), (nl, 1)],
            0xAA,
        ));
        let mut pool = fake_published_pool(1, 1920, 1080);
        pool.fourcc = N::DRM_FORMAT_ABGR8888;
        pool.modifier = nl;
        h.test_publish_pool(pool);
        mgr.register_test_handle(h.clone()).await;
        router.register_renderer(h.clone()).await;

        let mut registration = reg("D1", 1920, 1080);
        registration.consumer_caps = build_caps(
            N::DRM_FORMAT_ABGR8888,
            &[(N::DRM_FORMAT_MOD_LINEAR, 1), (nl, 1)],
            0xAA,
        );
        let dh = router.register_display(registration).await;

        // Pre-blacklist pick must land on the non-LINEAR (same-device preference).
        {
            let inner = router.inner.lock().await;
            let prod = h.format_caps().expect("producer caps");
            let cons = inner.displays[&dh.id].consumer_caps.clone();
            let s = N::pick(&prod, &cons).expect("pick ok");
            assert_eq!(s.modifier, nl, "pre-blacklist must prefer non-LINEAR");
        }

        // Consumer reports the non-LINEAR is unimportable.
        let generation = router.inner.lock().await.displays[&dh.id]
            .binding
            .as_ref()
            .expect("display binding")
            .wire_generation;
        assert!(matches!(
            router
                .on_consumer_import_failed(
                    dh.id,
                    generation,
                    ConsumerImportFailureKind::Unsupported,
                )
                .await,
            ConsumerImportFailureOutcome::Retry { .. }
        ));

        // Post-blacklist pick must fall back to LINEAR.
        let inner = router.inner.lock().await;
        let prod = h.format_caps().expect("producer caps");
        let cons = inner.displays[&dh.id].consumer_caps.clone();
        let s = N::pick(&prod, &cons).expect("post-blacklist pick ok");
        assert_eq!(
            s.modifier,
            N::DRM_FORMAT_MOD_LINEAR,
            "after consumer blacklist, picker must fall back to LINEAR"
        );
    }

    // -----------------------------------------------------------------
    // project_link layout integration

    fn make_link(rid: &str, did: DisplayId) -> Link {
        Link {
            id: 1,
            renderer_id: rid.to_string(),
            display_id: did,
            enabled: true,
            src_rect: super::super::table::FULL_SRC,
            dst_rect: super::super::table::FULL_DST,
            transform: 0,
            clear_rgba: [0.0, 0.0, 0.0, 1.0],
            z_order: 0,
        }
    }

    fn make_info(name: &str, w: u32, h: u32) -> DisplayInfo {
        DisplayInfo {
            id: 1,
            name: name.into(),
            instance_id: None,
            metrics: DisplayMetrics {
                width: w,
                height: h,
                refresh_mhz: 60_000,
            },
            bound: true,
        }
    }

    #[test]
    fn project_link_explicit_link_geometry_skips_layout() {
        // A link with explicit (non-sentinel) src/dst rects should
        // bypass display::layout::compute and pass rects through.
        let pool = fake_published_pool(1, 1920, 1080);
        let info = make_info("eDP-1", 1280, 720);
        let mut link = make_link("r1", 1);
        link.src_rect = super::super::table::LinkSrcRect {
            x: 100.0,
            y: 200.0,
            w: 800.0,
            h: 600.0,
        };
        link.dst_rect = super::super::table::LinkDstRect {
            x: 50.0,
            y: 75.0,
            w: 400.0,
            h: 300.0,
        };
        link.clear_rgba = [1.0, 0.0, 0.0, 1.0];
        let layout = ResolvedLayout {
            // Even with PreserveAspectFit, explicit geometry must win.
            fillmode: FillMode::PreserveAspectFit,
            location: Default::default(),
            rotation: Default::default(),
        };
        let cfg = project_link(&link, &pool, &info, 1, 7, &layout);
        assert_eq!((cfg.display_w, cfg.display_h), (1280.0, 720.0));
        assert_eq!(
            (cfg.source_x, cfg.source_y, cfg.source_w, cfg.source_h),
            (100.0, 200.0, 800.0, 600.0)
        );
        assert_eq!(
            (cfg.dest_x, cfg.dest_y, cfg.dest_w, cfg.dest_h),
            (50.0, 75.0, 400.0, 300.0)
        );
        // Explicit clear color survives.
        assert_eq!(cfg.clear_rgba, [1.0, 0.0, 0.0, 1.0]);
    }

    // -----------------------------------------------------------------
    // Display metrics resync

    use crate::wallframe::renderer_manager::{FrameSnapshot, PublishedPool};

    fn fake_published_pool(generation: u64, w: u32, h: u32) -> PublishedPool {
        PublishedPool {
            generation,
            flags: 0,
            count: 0,
            fourcc: 0x34325258, // XR24
            width: w,
            height: h,
            modifier: 0,
            planes_per_buffer: 1,
            stride: vec![],
            plane_offset: vec![],
            size: vec![],
            fds: vec![],
        }
    }

    /// Drain everything currently sitting on the rx and return only the
    /// last `SetCompositionConfig` payload, matching what the consumer would use.
    fn last_composition_config(
        rx: &mut mpsc::UnboundedReceiver<DisplayOutEvent>,
    ) -> Option<CompositionConfig> {
        let mut out = None;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                DisplayOutEvent::Bind { initial_config, .. }
                | DisplayOutEvent::SetCompositionConfig(initial_config) => {
                    out = Some(initial_config);
                }
                _ => {}
            }
        }
        out
    }

    fn drain_display_events(
        rx: &mut mpsc::UnboundedReceiver<DisplayOutEvent>,
    ) -> Vec<DisplayOutEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        out
    }

    #[tokio::test]
    async fn queued_bind_keeps_the_exact_published_pool() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let renderer = RendererHandle::test_stub("r1", "image");
        renderer.test_publish_pool(fake_published_pool(1, 1920, 1080));
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer.clone()).await;

        let mut display = router.register_display(reg("HDMI-A-1", 1280, 720)).await;
        let old_pool = renderer.published_pool().unwrap();
        let old_pool_weak = Arc::downgrade(&old_pool);
        drop(old_pool);
        renderer.test_publish_pool(fake_published_pool(2, 3840, 2160));
        router.on_renderer_bind("r1").await;

        assert!(old_pool_weak.upgrade().is_some());

        let events = drain_display_events(&mut display.rx);
        {
            let (bound_pool, config, wire_generation) = events
                .iter()
                .find_map(|event| match event {
                    DisplayOutEvent::Bind {
                        pool,
                        buffer_generation,
                        initial_config,
                        ..
                    } => Some((pool, initial_config, buffer_generation)),
                    _ => None,
                })
                .expect("initial Bind");

            assert_eq!(bound_pool.generation, 1);
            assert_eq!((bound_pool.width, bound_pool.height), (1920, 1080));
            assert_eq!((config.source_w, config.source_h), (1920.0, 1080.0));
            assert_eq!(config.buffer_generation, *wire_generation);
        }
        assert_eq!(renderer.published_pool().unwrap().generation, 2);
        drop(events);
        assert!(old_pool_weak.upgrade().is_none());
    }

    #[tokio::test]
    async fn frame_keeps_the_generation_from_receive_time() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let (renderer, mut frame_records) =
            RendererHandle::test_stub_with_frame_records("r1", "video");
        renderer.test_publish_pool(fake_published_pool(1, 1920, 1080));
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer.clone()).await;

        let mut display = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;
        let initial_events = drain_display_events(&mut display.rx);
        let wire_generation = initial_events
            .iter()
            .find_map(|event| match event {
                DisplayOutEvent::Bind {
                    buffer_generation, ..
                } => Some(*buffer_generation),
                _ => None,
            })
            .expect("initial Bind");

        renderer.test_publish_pool(fake_published_pool(2, 3840, 2160));
        router.on_renderer_frame("r1", 1, 0, 42, 7).await;

        let events = drain_display_events(&mut display.rx);
        assert!(events.iter().any(|event| matches!(
            event,
            DisplayOutEvent::Frame {
                buffer_generation,
                seq: 42,
                ..
            } if *buffer_generation == wire_generation
        )));
        match frame_records.try_recv().expect("frame registration") {
            crate::wallframe::sync::FrameRecord::Register {
                identity,
                consumers,
            } => {
                assert_eq!(identity.buffer_generation, 1);
                assert_eq!(consumers.len(), 1);
            }
            _ => panic!("unexpected frame record"),
        }
    }

    #[test]
    fn consumption_permit_is_invalidated_without_waiting_for_endpoint() {
        let epoch = Arc::new(AtomicU64::new(4));
        let permit = DisplayConsumptionPermit {
            current: Arc::clone(&epoch),
            epoch: 4,
        };

        assert!(permit.is_current());
        epoch.fetch_add(1, Ordering::AcqRel);
        assert!(!permit.is_current());
    }

    fn drain_renderer_controls(peer: &std::os::unix::net::UnixStream) {
        peer.set_read_timeout(Some(Duration::from_millis(10)))
            .unwrap();
        loop {
            match crate::wallframe::ipc::uds::recv_control(peer) {
                Ok(_) => {}
                Err(crate::wallframe::ipc::uds::CodecError::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(crate::wallframe::ipc::uds::CodecError::Nix(nix::errno::Errno::EAGAIN)) => {
                    break
                }
                Err(e) => panic!("drain renderer control failed: {e}"),
            }
        }
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    }

    #[tokio::test]
    async fn relink_with_reused_renderer_generation_maps_replayed_frame() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());

        let r1 = RendererHandle::test_stub("r1", "image");
        r1.test_publish_pool(fake_published_pool(1, 1920, 1080));
        mgr.register_test_handle(r1.clone()).await;
        router.register_renderer(r1.clone()).await;

        let r2 = RendererHandle::test_stub("r2", "image");
        r2.test_publish_pool(fake_published_pool(1, 1920, 1080));
        mgr.register_test_handle(r2.clone()).await;
        router.register_renderer(r2.clone()).await;

        let mut a = router.register_display(reg("A", 1920, 1080)).await;
        let mut b = router.register_display(reg("B", 1920, 1080)).await;
        let _ = drain_display_events(&mut a.rx);
        let _ = drain_display_events(&mut b.rx);

        router.relink_displays_to(&[b.id], "r2").await;
        let _ = drain_display_events(&mut b.rx);

        r1.test_set_latest_frame(FrameSnapshot {
            buffer_generation: 1,
            buffer_index: 0,
            seq: 42,
            release_point: 7,
        });

        router.relink_displays_to(&[b.id], "r1").await;
        let events = drain_display_events(&mut b.rx);
        let mut unbind_generation = None;
        let mut bind_generation = None;
        let mut saw_frame = false;
        for ev in events {
            match ev {
                DisplayOutEvent::Unbind { buffer_generation } => {
                    unbind_generation = Some(buffer_generation);
                }
                DisplayOutEvent::Bind {
                    renderer,
                    pool: _,
                    buffer_generation,
                    initial_config: _,
                } => {
                    assert_eq!(renderer.id, "r1");
                    assert!(buffer_generation > 1);
                    bind_generation = Some(buffer_generation);
                }
                DisplayOutEvent::Frame {
                    renderer,
                    buffer_generation,
                    buffer_index,
                    seq,
                    consumption: _,
                    member: _,
                } => {
                    assert_eq!(renderer.id, "r1");
                    assert_eq!(Some(buffer_generation), bind_generation);
                    assert_eq!(buffer_index, 0);
                    assert_eq!(seq, 42);
                    saw_frame = true;
                }
                _ => {}
            }
        }
        let unbind_generation = unbind_generation.expect("relink did not emit unbind");
        let bind_generation = bind_generation.expect("relink did not emit a new bind");
        assert!(unbind_generation < bind_generation);
        assert!(saw_frame, "relinked display did not receive current frame");
    }

    #[tokio::test]
    async fn update_display_metrics_resyncs_composition_config() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());

        // Renderer with a bind snapshot so resync_display_composition can
        // read a generation and texture size.
        let r = RendererHandle::test_stub("r1", "scene"); // 1920x1080
        r.test_publish_pool(fake_published_pool(1, 1920, 1080));
        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r.clone()).await;

        // Register display 1920x1080 — auto-link + initial Bind/SetCompositionConfig.
        let mut h = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;
        let initial = last_composition_config(&mut h.rx).expect("initial composition config");
        assert_eq!((initial.display_w, initial.display_h), (1920.0, 1080.0));
        assert_eq!((initial.dest_w, initial.dest_h), (1920.0, 1080.0));

        // Resize to 1280x720 — Stretched + Center default → identity at new dims.
        router
            .set_display_metrics(
                h.id,
                DisplayMetrics {
                    width: 1280,
                    height: 720,
                    refresh_mhz: 75_000,
                },
            )
            .await;
        let resized = last_composition_config(&mut h.rx).expect("composition config after resize");
        assert_eq!((resized.display_w, resized.display_h), (1280.0, 720.0));
        assert_eq!((resized.dest_x, resized.dest_y), (0.0, 0.0));
        assert_eq!((resized.dest_w, resized.dest_h), (1280.0, 720.0));
        assert!(resized.generation > initial.generation);
        assert_eq!(resized.buffer_generation, initial.buffer_generation);
        assert_eq!(
            router.snapshot_display(h.id).await.unwrap().refresh_mhz,
            75_000
        );
    }

    #[tokio::test]
    async fn composition_resync_requests_one_frame_per_renderer() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let (renderer, peer) = RendererHandle::test_stub_with_peer("r1", "scene");
        renderer.test_publish_pool(fake_published_pool(1, 1920, 1080));
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;

        let mut first = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;
        let mut second = router.register_display(reg("DP-1", 1920, 1080)).await;
        let _ = last_composition_config(&mut first.rx);
        let _ = last_composition_config(&mut second.rx);
        drain_renderer_controls(&peer);

        router.resync_all_compositions().await;

        assert!(last_composition_config(&mut first.rx).is_some());
        assert!(last_composition_config(&mut second.rx).is_some());
        let (message, fds) =
            crate::wallframe::ipc::uds::recv_control(&peer).expect("request_frame");
        assert_eq!(message, ControlMsg::RequestFrame);
        assert!(fds.is_empty());

        peer.set_read_timeout(Some(Duration::from_millis(10)))
            .unwrap();
        let extra = crate::wallframe::ipc::uds::recv_control(&peer);
        assert!(
            matches!(
                &extra,
                Err(crate::wallframe::ipc::uds::CodecError::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    )
            ) || matches!(
                &extra,
                Err(crate::wallframe::ipc::uds::CodecError::Nix(
                    nix::errno::Errno::EAGAIN
                ))
            ),
            "unexpected control after deduplicated request: {extra:?}"
        );
    }

    #[tokio::test]
    async fn update_display_metrics_same_values_no_resync() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let r = RendererHandle::test_stub("r1", "scene");
        r.test_publish_pool(fake_published_pool(1, 1920, 1080));
        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r.clone()).await;

        let mut h = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;
        // Drain initial events.
        let _ = last_composition_config(&mut h.rx);

        router
            .set_display_metrics(
                h.id,
                DisplayMetrics {
                    width: 1920,
                    height: 1080,
                    refresh_mhz: 60_000,
                },
            )
            .await;
        // No new SetCompositionConfig should land on the rx.
        assert!(last_composition_config(&mut h.rx).is_none());
    }

    // -----------------------------------------------------------------
    // auto replay - daemon-side decision driven by display state

    use super::auto_replay as ar;
    use crate::settings::{
        AutoAction, AutoCondition, AutoReplayPolicy, BlurEffectConfig as StoredBlurEffectConfig,
        PauseEffectConfig as StoredPauseEffectConfig, PauseEffectKind, SettingsStore,
    };

    async fn settings_with_auto_replay(policy: AutoReplayPolicy) -> Arc<SettingsStore> {
        let tmp = tempfile::tempdir().unwrap();
        let store = SettingsStore::load_or_default(tmp.path().join("settings.toml")).await;
        store.update(|s| {
            s.global.auto_replay = Some(policy);
        });
        std::mem::forget(tmp);
        store
    }

    async fn settings_with_pause_effect(config: StoredPauseEffectConfig) -> Arc<SettingsStore> {
        let tmp = tempfile::tempdir().unwrap();
        let store = SettingsStore::load_or_default(tmp.path().join("settings.toml")).await;
        store.update(|s| s.global.pause_effect = config);
        std::mem::forget(tmp);
        store
    }

    fn blur_pause_effect(radius: u32) -> StoredPauseEffectConfig {
        StoredPauseEffectConfig {
            kind: PauseEffectKind::Blur,
            blur: StoredBlurEffectConfig { radius },
        }
    }

    fn last_presentation_state(
        rx: &mut mpsc::UnboundedReceiver<DisplayOutEvent>,
    ) -> Option<PresentationState> {
        drain_display_events(rx)
            .into_iter()
            .filter_map(|event| match event {
                DisplayOutEvent::SetPresentationState(config) => Some(config),
                _ => None,
            })
            .last()
    }

    fn last_presentation_config(
        rx: &mut mpsc::UnboundedReceiver<DisplayOutEvent>,
    ) -> Option<PresentationSnapshot> {
        drain_display_events(rx)
            .into_iter()
            .filter_map(|event| match event {
                DisplayOutEvent::SetPresentationSnapshot(config) => Some(config),
                _ => None,
            })
            .last()
    }

    #[tokio::test]
    async fn pause_effect_uses_initial_snapshot_and_split_updates() {
        let store = settings_with_pause_effect(blur_pause_effect(42)).await;
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        router.attach_settings(store.clone());
        let renderer = RendererHandle::test_stub("r1", "image");
        renderer.test_publish_pool(fake_published_pool(1, 1920, 1080));
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;
        let mut registration = reg("HDMI-A-1", 1920, 1080);
        registration.presentation_caps = PRESENTATION_CAP_PAUSE_BLUR;
        let mut display = router.register_display(registration).await;

        assert_eq!(
            display.presentation,
            PresentationSnapshot {
                config: PresentationConfig {
                    generation: 1,
                    pause_effect: PauseEffectConfig {
                        kind: PauseEffectKind::Blur,
                        blur: BlurEffectConfig { radius: 42 },
                    },
                },
                state: PresentationState {
                    generation: 1,
                    config_generation: 1,
                    pause_effect: PauseEffectState { active: false },
                },
            }
        );
        drain_display_events(&mut display.rx);

        router.set_manual_pause(true).await;
        let dynamic = last_presentation_state(&mut display.rx)
            .expect("manual pause should update presentation activity");
        assert_eq!(dynamic.generation, 2);
        assert_eq!(dynamic.config_generation, 1);
        assert!(dynamic.pause_effect.active);

        store.update(|s| s.global.pause_effect.blur.radius = 55);
        router.resync_presentation_configs().await;
        let snapshot = last_presentation_config(&mut display.rx)
            .expect("radius update should send an atomic presentation snapshot");
        assert_eq!(snapshot.config.generation, 2);
        assert_eq!(snapshot.config.pause_effect.blur.radius, 55);
        assert_eq!(snapshot.state.generation, 3);
        assert_eq!(snapshot.state.config_generation, 2);
        assert!(snapshot.state.pause_effect.active);

        router.resync_presentation_configs().await;
        assert!(display.rx.try_recv().is_err());

        store.update(|s| s.global.pause_effect.kind = PauseEffectKind::None);
        router.resync_presentation_configs().await;
        let snapshot = last_presentation_config(&mut display.rx)
            .expect("disabling the effect should send an atomic presentation snapshot");
        assert_eq!(snapshot.config.pause_effect.kind, PauseEffectKind::None);
        assert!(!snapshot.state.pause_effect.active);
    }

    #[tokio::test]
    async fn paused_binding_is_reflected_in_initial_registration_snapshot() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        router.attach_settings(settings_with_pause_effect(blur_pause_effect(30)).await);
        let renderer = RendererHandle::test_stub("r1", "image");
        renderer.test_publish_pool(fake_published_pool(1, 1920, 1080));
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;
        router.set_manual_pause(true).await;

        let mut registration = reg("HDMI-A-1", 1920, 1080);
        registration.presentation_caps = PRESENTATION_CAP_PAUSE_BLUR;
        let mut display = router.register_display(registration).await;

        assert!(display.presentation.state.pause_effect.active);
        assert_eq!(
            display.presentation.state.config_generation,
            display.presentation.config.generation
        );
        assert!(last_presentation_state(&mut display.rx).is_none());
    }

    #[tokio::test]
    async fn shared_renderer_pause_activates_all_bound_displays_but_mute_does_not() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        router.attach_settings(settings_with_pause_effect(blur_pause_effect(30)).await);
        let renderer = RendererHandle::test_stub("r1", "image");
        renderer.test_publish_pool(fake_published_pool(1, 1920, 1080));
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;
        let mut a_registration = reg("A", 1920, 1080);
        a_registration.presentation_caps = PRESENTATION_CAP_PAUSE_BLUR;
        let mut b_registration = reg("B", 1920, 1080);
        b_registration.presentation_caps = PRESENTATION_CAP_PAUSE_BLUR;
        let mut a = router.register_display(a_registration).await;
        let mut b = router.register_display(b_registration).await;
        drain_display_events(&mut a.rx);
        drain_display_events(&mut b.rx);

        router.set_manual_mute(true).await;
        assert!(router.is_muted("r1").await);
        assert!(last_presentation_state(&mut a.rx).is_none());
        assert!(b.rx.try_recv().is_err());

        router.set_manual_mute(false).await;
        router.set_manual_pause(true).await;
        assert!(last_presentation_state(&mut a.rx).is_some_and(|config| config.pause_effect.active));
        assert!(last_presentation_state(&mut b.rx).is_some_and(|config| config.pause_effect.active));

        router.set_manual_pause(false).await;
        assert!(
            last_presentation_state(&mut a.rx).is_some_and(|config| !config.pause_effect.active)
        );
        assert!(
            last_presentation_state(&mut b.rx).is_some_and(|config| !config.pause_effect.active)
        );
    }

    #[tokio::test]
    async fn unsupported_display_downgrades_pause_effect_to_none() {
        let store = settings_with_pause_effect(blur_pause_effect(64)).await;
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        router.attach_settings(store.clone());
        let renderer = RendererHandle::test_stub("r1", "image");
        renderer.test_publish_pool(fake_published_pool(1, 1920, 1080));
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;
        let mut display = router
            .register_display(reg("layer-shell", 1920, 1080))
            .await;
        assert_eq!(
            display.presentation.config.pause_effect.kind,
            PauseEffectKind::None
        );
        drain_display_events(&mut display.rx);

        router.set_manual_pause(true).await;
        store.update(|s| s.global.pause_effect.blur.radius = 10);
        router.resync_presentation_configs().await;
        assert!(display.rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn pause_effect_consumes_auto_replay_renderer_state() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let settings = settings_with_pause_effect(blur_pause_effect(30)).await;
        settings.update(|s| {
            s.global.auto_replay = Some(auto_replay(&[(
                AutoCondition::Fullscreen,
                AutoAction::Pause,
            )]));
        });
        router.attach_settings(settings);

        let renderer = RendererHandle::test_stub("r1", "scene");
        renderer.test_publish_pool(fake_published_pool(1, 1920, 1080));
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;
        let mut registration = reg("HDMI-A-1", 1920, 1080);
        registration.presentation_caps = PRESENTATION_CAP_PAUSE_BLUR;
        let mut display = router.register_display(registration).await;
        drain_display_events(&mut display.rx);

        router
            .update_display_window_state(display.id, ar::FLAG_NON_MINIMIZED | ar::FLAG_FULLSCREEN)
            .await;

        assert!(router.is_paused("r1").await);
        assert!(last_presentation_state(&mut display.rx)
            .is_some_and(|config| config.pause_effect.active));
    }

    #[tokio::test]
    async fn initial_window_state_is_reconciled_before_acceptance() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let settings = settings_with_pause_effect(blur_pause_effect(30)).await;
        settings.update(|s| {
            s.global.auto_replay = Some(auto_replay(&[(
                AutoCondition::Fullscreen,
                AutoAction::Pause,
            )]));
        });
        router.attach_settings(settings);

        let renderer = RendererHandle::test_stub("r1", "scene");
        renderer.test_publish_pool(fake_published_pool(1, 1920, 1080));
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;
        let mut registration = reg("HDMI-A-1", 1920, 1080);
        registration.presentation_caps = PRESENTATION_CAP_PAUSE_BLUR;
        registration.window_state_flags = ar::FLAG_NON_MINIMIZED | ar::FLAG_FULLSCREEN;
        let mut display = router.register_display(registration).await;

        assert!(router.is_paused("r1").await);
        assert!(display.presentation.state.pause_effect.active);
        assert!(last_presentation_state(&mut display.rx).is_none());
    }

    #[tokio::test]
    async fn pause_effect_deactivates_when_auto_stop_removes_binding() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let settings = settings_with_pause_effect(blur_pause_effect(30)).await;
        settings.update(|s| {
            s.global.auto_replay = Some(auto_replay(&[(
                AutoCondition::Fullscreen,
                AutoAction::Stop,
            )]));
        });
        router.attach_settings(settings);

        let renderer = RendererHandle::test_stub("r1", "image");
        renderer.test_publish_pool(fake_published_pool(1, 1920, 1080));
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;
        let mut registration = reg("HDMI-A-1", 1920, 1080);
        registration.presentation_caps = PRESENTATION_CAP_PAUSE_BLUR;
        let mut display = router.register_display(registration).await;
        drain_display_events(&mut display.rx);

        router.set_manual_pause(true).await;
        assert!(last_presentation_state(&mut display.rx)
            .is_some_and(|config| config.pause_effect.active));

        router
            .update_display_window_state(display.id, ar::FLAG_NON_MINIMIZED | ar::FLAG_FULLSCREEN)
            .await;
        assert!(last_presentation_state(&mut display.rx)
            .is_some_and(|config| !config.pause_effect.active));
    }

    fn auto_replay(actions: &[(AutoCondition, AutoAction)]) -> AutoReplayPolicy {
        let mut policy = AutoReplayPolicy::default();
        for (condition, action) in actions {
            policy.set_action(*condition, *action);
        }
        policy
    }

    #[tokio::test]
    async fn auto_replay_pauses_renderer_when_fullscreen_flag_set() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        router.attach_settings(
            settings_with_auto_replay(auto_replay(&[(
                AutoCondition::Fullscreen,
                AutoAction::Pause,
            )]))
            .await,
        );
        let r = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r.clone()).await;
        let h = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;
        // No auto replay condition yet; renderer plays.
        assert!(!router.is_paused("r1").await);

        // Fullscreen window appears; daemon should pause immediately.
        router
            .update_display_window_state(h.id, ar::FLAG_NON_MINIMIZED | ar::FLAG_FULLSCREEN)
            .await;
        assert!(router.is_paused("r1").await);
    }

    #[tokio::test]
    async fn auto_replay_action_priority_prefers_pause_over_mute() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        router.attach_settings(
            settings_with_auto_replay(auto_replay(&[
                (AutoCondition::Fullscreen, AutoAction::Pause),
                (AutoCondition::Focused, AutoAction::Mute),
            ]))
            .await,
        );
        let r = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r.clone()).await;
        let h = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;

        router
            .update_display_window_state(h.id, ar::FLAG_ACTIVE | ar::FLAG_FULLSCREEN)
            .await;

        assert!(router.is_paused("r1").await);
        assert!(!router.is_muted("r1").await);
    }

    #[tokio::test]
    async fn auto_replay_stop_retains_renderer_slot() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        router.attach_settings(
            settings_with_auto_replay(auto_replay(&[(
                AutoCondition::Fullscreen,
                AutoAction::Stop,
            )]))
            .await,
        );
        let r = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r.clone()).await;
        let h = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;
        assert!(mgr.get("r1").await.is_some());

        router
            .update_display_window_state(h.id, ar::FLAG_FULLSCREEN)
            .await;

        let display = router.snapshot_display(h.id).await.unwrap();
        assert_eq!(display.links.len(), 1);
        assert!(!display.links[0].active);
        assert!(mgr.get("r1").await.is_none());
        let snapshot = router.snapshot_renderer("r1").await.unwrap();
        assert!(matches!(
            snapshot.state,
            RendererLifecycleState::Stopped { keep: true, .. }
        ));
        assert_eq!(snapshot.pid, 0);
        assert!(snapshot.runtime_tags.is_empty());

        let mut request = crate::wallframe::renderer_manager::SpawnRequest {
            wp_type: "video".into(),
            renderer_name: Some("video".into()),
            ..Default::default()
        };
        request.extras.insert("path".into(), "/new.mp4".into());
        let retained_id = router
            .apply_assignment(ApplyAssignment {
                spawn_request: request,
                display_ids: vec![h.id],
                duplicate_renderers: false,
                wallpaper_layout_override: WallpaperLayoutOverride::default(),
                preempt_pending_start: false,
            })
            .await
            .unwrap()
            .renderer_id;
        assert_eq!(retained_id, "r1");
        assert!(mgr.get("r1").await.is_none());
        let inner = router.inner.lock().await;
        let slot = inner.renderer_slots.get("r1").unwrap();
        assert_eq!(slot.spawn_request.wp_type, "video");
        assert_eq!(slot.spawn_request.extras.get("path").unwrap(), "/new.mp4");
        assert_eq!(slot.spec_revision, 2);
    }

    #[tokio::test]
    async fn shared_renderer_stops_only_after_every_display_is_inhibited() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        router.attach_settings(
            settings_with_auto_replay(auto_replay(&[(
                AutoCondition::Fullscreen,
                AutoAction::Stop,
            )]))
            .await,
        );
        let renderer = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;
        let first = router.register_display(reg("DP-1", 1920, 1080)).await;
        let second = router.register_display(reg("DP-2", 1920, 1080)).await;

        router
            .update_display_window_state(first.id, ar::FLAG_FULLSCREEN)
            .await;
        assert!(mgr.get("r1").await.is_some());
        assert!(matches!(
            router.snapshot_renderer("r1").await.unwrap().state,
            RendererLifecycleState::Running { .. }
        ));

        router
            .update_display_window_state(second.id, ar::FLAG_FULLSCREEN)
            .await;
        assert!(mgr.get("r1").await.is_none());
        assert!(matches!(
            router.snapshot_renderer("r1").await.unwrap().state,
            RendererLifecycleState::Stopped { keep: true, .. }
        ));
    }

    #[tokio::test]
    async fn disabled_assignment_is_not_an_orphan() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        router.attach_settings(
            settings_with_auto_replay(auto_replay(&[(
                AutoCondition::Fullscreen,
                AutoAction::Stop,
            )]))
            .await,
        );
        let renderer = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;
        let display = router.register_display(reg("DP-1", 1920, 1080)).await;

        router
            .update_display_window_state(display.id, ar::FLAG_FULLSCREEN)
            .await;

        assert!(router.mark_orphans(None).await.is_empty());
        assert!(router.snapshot_renderer("r1").await.is_some());
        let display = router.snapshot_display(display.id).await.unwrap();
        assert_eq!(display.links.len(), 1);
        assert!(!display.links[0].active);
    }

    #[tokio::test]
    async fn retained_apply_during_stop_commits_latest_spec() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let renderer = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;
        let display = router.register_display(reg("DP-1", 1920, 1080)).await;
        {
            let mut inner = router.inner.lock().await;
            inner.manual_stopped = true;
            inner
                .renderer_slots
                .get_mut("r1")
                .unwrap()
                .transition(RendererLifecycleEvent::StopRequested { keep: true });
            for link in inner.table.links_for_renderer("r1") {
                inner.table.set_link_enabled(link.id, false);
            }
        }
        let mut request = crate::wallframe::renderer_manager::SpawnRequest {
            wp_type: "video".into(),
            renderer_name: Some("video".into()),
            ..Default::default()
        };
        request.extras.insert("path".into(), "/latest.mp4".into());

        let retained_id = router
            .apply_assignment(ApplyAssignment {
                spawn_request: request,
                display_ids: vec![display.id],
                duplicate_renderers: false,
                wallpaper_layout_override: WallpaperLayoutOverride::default(),
                preempt_pending_start: false,
            })
            .await
            .unwrap()
            .renderer_id;

        assert_eq!(retained_id, "r1");
        let exit = mgr.stop("r1").await.unwrap();
        router.on_renderer_process_exit(exit).await;
        let inner = router.inner.lock().await;
        let slot = inner.renderer_slots.get("r1").unwrap();
        assert!(matches!(
            slot.state,
            RendererLifecycleState::Stopped { keep: true, .. }
        ));
        assert_eq!(
            slot.spawn_request.extras.get("path").unwrap(),
            "/latest.mp4"
        );
        assert_eq!(slot.spec_revision, 2);
    }

    #[tokio::test]
    async fn manual_pause_intent_survives_retained_stop() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let renderer = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;

        assert!(router.set_renderer_paused("r1", true).await);
        router
            .inner
            .lock()
            .await
            .renderer_slots
            .get_mut("r1")
            .unwrap()
            .transition(RendererLifecycleEvent::StopRequested { keep: true });
        let exit = mgr.stop("r1").await.unwrap();
        router.on_renderer_process_exit(exit).await;

        assert!(router
            .inner
            .lock()
            .await
            .renderer_manual_paused
            .contains("r1"));
        let resumed = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(resumed.clone()).await;
        {
            let mut inner = router.inner.lock().await;
            let slot = inner.renderer_slots.get_mut("r1").unwrap();
            assert_eq!(
                slot.transition(RendererLifecycleEvent::StartRequested {
                    generation: resumed.process_generation,
                    start_token: 1,
                    reactivate_failed: false,
                }),
                RendererTransition::Changed
            );
        }
        assert!(
            router
                .register_renderer_current(
                    resumed.clone(),
                    Some((1, resumed.process_generation, 1)),
                )
                .await
        );
        assert!(router.is_paused("r1").await);
    }

    #[tokio::test]
    async fn resume_during_stopping_starts_after_old_process_exits() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let renderer = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;
        router.register_display(reg("DP-1", 1920, 1080)).await;

        router.begin_retained_stop("r1").await;
        router
            .request_renderer_start("r1", RendererStartCause::ManualStopResume)
            .await
            .unwrap();
        assert!(matches!(
            router.snapshot_renderer("r1").await.unwrap().state,
            RendererLifecycleState::Stopping { keep: true, .. }
        ));

        let exit = mgr.stop("r1").await.unwrap();
        router.on_renderer_process_exit(exit).await;

        let inner = router.inner.lock().await;
        let slot = inner.renderer_slots.get("r1").unwrap();
        assert!(matches!(slot.state, RendererLifecycleState::Failed { .. }));
        assert!(slot
            .state
            .last_exit()
            .is_some_and(|exit| exit.reason.contains("test-stub")));
        assert!(inner
            .renderer_slots
            .get("r1")
            .is_some_and(|slot| slot.pending_start.is_none()));
    }

    #[tokio::test]
    async fn killed_process_is_retained_and_stale_exit_is_ignored() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let renderer = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;

        router
            .on_renderer_process_exit(crate::wallframe::renderer_manager::RendererProcessExit {
                renderer_id: "r1".into(),
                process_generation: 0,
                kind: crate::wallframe::renderer_manager::RendererProcessExitKind::Killed,
                code: None,
                signal: Some(libc::SIGKILL),
                reason: "stale".into(),
            })
            .await;
        assert!(matches!(
            router.snapshot_renderer("r1").await.unwrap().state,
            RendererLifecycleState::Running { .. }
        ));

        router
            .on_renderer_process_exit(crate::wallframe::renderer_manager::RendererProcessExit {
                renderer_id: "r1".into(),
                process_generation: 1,
                kind: crate::wallframe::renderer_manager::RendererProcessExitKind::Killed,
                code: None,
                signal: Some(libc::SIGKILL),
                reason: "signal: 9".into(),
            })
            .await;
        let snapshot = router.snapshot_renderer("r1").await.unwrap();
        assert!(matches!(
            snapshot.state,
            RendererLifecycleState::Killed { keep: true, .. }
        ));
        assert_eq!(
            snapshot.state.last_exit().and_then(|exit| exit.signal),
            Some(libc::SIGKILL)
        );
    }

    #[tokio::test]
    async fn killing_stopped_renderer_drops_slot() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let renderer = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;
        router
            .inner
            .lock()
            .await
            .renderer_slots
            .get_mut("r1")
            .unwrap()
            .transition(RendererLifecycleEvent::StopRequested { keep: true });
        let exit = mgr.stop("r1").await.unwrap();
        router.on_renderer_process_exit(exit).await;
        assert!(matches!(
            router.snapshot_renderer("r1").await.unwrap().state,
            RendererLifecycleState::Stopped { keep: true, .. }
        ));

        router.kill_renderer_drop("r1").await.unwrap();
        assert!(router.snapshot_renderer("r1").await.is_none());
    }

    #[tokio::test]
    async fn auto_replay_falls_back_to_default_policy() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SettingsStore::load_or_default(tmp.path().join("settings.toml")).await;
        store.update(|s| {
            s.global.auto_replay = None;
        });
        std::mem::forget(tmp);

        let policy = store.resolved_auto_replay("HDMI-A-1");
        assert_eq!(policy.any_window, AutoAction::None);
        assert_eq!(policy.fullscreen, AutoAction::Pause);
    }

    #[tokio::test]
    async fn renderer_without_links_is_paused_on_register() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let r = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(r.clone()).await;

        router.register_renderer(r.clone()).await;

        assert!(router.is_paused("r1").await);
    }

    #[tokio::test]
    async fn manual_pause_is_daemon_state() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let r = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r.clone()).await;
        let _h = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;

        assert!(!router.is_paused("r1").await);
        router.set_manual_pause(true).await;
        assert!(router.is_paused("r1").await);
        router.set_manual_pause(false).await;
        assert!(!router.is_paused("r1").await);
    }

    #[tokio::test]
    async fn manual_lifecycle_state_tracks_toggles() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr);

        assert_eq!(
            router.manual_lifecycle_state().await,
            ManualLifecycleState {
                paused: false,
                muted: false,
                stopped: false,
            }
        );
        assert!(router.toggle_manual_pause().await);
        assert!(router.toggle_manual_mute().await);
        assert_eq!(
            router.manual_lifecycle_state().await,
            ManualLifecycleState {
                paused: true,
                muted: true,
                stopped: false,
            }
        );
        assert!(!router.toggle_manual_pause().await);
        assert!(!router.toggle_manual_mute().await);
        assert_eq!(
            router.manual_lifecycle_state().await,
            ManualLifecycleState {
                paused: false,
                muted: false,
                stopped: false,
            }
        );
    }

    #[tokio::test]
    async fn manual_stop_retains_slots_and_reactivates_assignments() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let renderer = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;
        let display = router.register_display(reg("DP-1", 1920, 1080)).await;

        router.set_manual_stop(true).await;

        let renderer = router.snapshot_renderer("r1").await.unwrap();
        assert!(matches!(
            renderer.state,
            RendererLifecycleState::Stopped { keep: true, .. }
        ));
        assert!(mgr.get("r1").await.is_none());
        assert!(!router.snapshot_display(display.id).await.unwrap().links[0].active);
        assert!(router.manual_lifecycle_state().await.stopped);

        router.set_manual_stop(false).await;

        assert!(router.snapshot_display(display.id).await.unwrap().links[0].active);
        assert!(matches!(
            router.snapshot_renderer("r1").await.unwrap().state,
            RendererLifecycleState::Failed { .. }
        ));
        assert!(!router.manual_lifecycle_state().await.stopped);
    }

    #[tokio::test]
    async fn clearing_manual_stop_does_not_reactivate_failed_renderer() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let renderer = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;
        let _display = router.register_display(reg_iid("DP-1", "display-1")).await;
        let generation = router
            .snapshot_renderer("r1")
            .await
            .unwrap()
            .state
            .generation()
            .unwrap();
        router
            .on_renderer_process_exit(crate::wallframe::renderer_manager::RendererProcessExit {
                renderer_id: "r1".into(),
                process_generation: generation,
                kind: crate::wallframe::renderer_manager::RendererProcessExitKind::Failed,
                code: Some(1),
                signal: None,
                reason: "initial failure".into(),
            })
            .await;

        router.set_manual_stop(true).await;
        router.set_manual_stop(false).await;

        let snapshot = router.snapshot_renderer("r1").await.unwrap();
        assert!(matches!(
            snapshot.state,
            RendererLifecycleState::Failed { .. }
        ));
        assert_eq!(
            snapshot.state.last_exit().map(|exit| exit.reason.as_str()),
            Some("initial failure")
        );
        assert!(router
            .inner
            .lock()
            .await
            .renderer_slots
            .get("r1")
            .is_some_and(|slot| slot.pending_start.is_none()));
    }

    #[tokio::test]
    async fn display_reconnect_restores_and_reactivates_failed_assignment() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let renderer = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;
        let display = router.register_display(reg_iid("DP-1", "display-1")).await;
        let generation = router
            .snapshot_renderer("r1")
            .await
            .unwrap()
            .state
            .generation()
            .unwrap();
        router
            .on_renderer_process_exit(crate::wallframe::renderer_manager::RendererProcessExit {
                renderer_id: "r1".into(),
                process_generation: generation,
                kind: crate::wallframe::renderer_manager::RendererProcessExitKind::Failed,
                code: Some(1),
                signal: None,
                reason: "initial failure".into(),
            })
            .await;
        router.unregister_display(display.id).await;

        let reconnected = router.register_display(reg_iid("DP-1", "display-1")).await;

        let display = router.snapshot_display(reconnected.id).await.unwrap();
        assert_eq!(display.links.len(), 1);
        assert_eq!(display.links[0].renderer_id, "r1");
        let snapshot = router.snapshot_renderer("r1").await.unwrap();
        assert!(matches!(
            snapshot.state,
            RendererLifecycleState::Failed { .. }
        ));
        assert_ne!(
            snapshot.state.last_exit().map(|exit| exit.reason.as_str()),
            Some("initial failure")
        );
    }

    #[tokio::test]
    async fn active_apply_reuses_and_reactivates_failed_assignment() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let renderer = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;
        let display = router.register_display(reg("DP-1", 1920, 1080)).await;
        let generation = router
            .snapshot_renderer("r1")
            .await
            .unwrap()
            .state
            .generation()
            .unwrap();
        router
            .on_renderer_process_exit(crate::wallframe::renderer_manager::RendererProcessExit {
                renderer_id: "r1".into(),
                process_generation: generation,
                kind: crate::wallframe::renderer_manager::RendererProcessExitKind::Failed,
                code: Some(1),
                signal: None,
                reason: "initial failure".into(),
            })
            .await;
        let mut request = crate::wallframe::renderer_manager::SpawnRequest {
            wp_type: "video".into(),
            renderer_name: Some("video".into()),
            ..Default::default()
        };
        request.extras.insert("path".into(), "/new.mp4".into());

        let result = router
            .apply_assignment(ApplyAssignment {
                spawn_request: request,
                display_ids: vec![display.id],
                duplicate_renderers: false,
                wallpaper_layout_override: WallpaperLayoutOverride::default(),
                preempt_pending_start: true,
            })
            .await;

        assert!(result.is_err());
        let inner = router.inner.lock().await;
        let slot = inner.renderer_slots.get("r1").unwrap();
        assert_eq!(slot.spec_revision, 2);
        assert_eq!(slot.spawn_request.wp_type, "video");
        assert_eq!(slot.spawn_request.extras.get("path").unwrap(), "/new.mp4");
        assert!(matches!(slot.state, RendererLifecycleState::Failed { .. }));
    }

    #[tokio::test]
    async fn manual_stop_stops_renderer_without_display_assignment() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let renderer = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;

        router.set_manual_stop(true).await;

        assert!(mgr.get("r1").await.is_none());
        assert!(matches!(
            router.snapshot_renderer("r1").await.unwrap().state,
            RendererLifecycleState::Stopped { keep: true, .. }
        ));
    }

    #[tokio::test]
    async fn retained_apply_without_displays_replaces_the_logical_assignment() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let renderer = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;
        router.set_manual_stop(true).await;
        let mut request = crate::wallframe::renderer_manager::SpawnRequest {
            wp_type: "video".into(),
            renderer_name: Some("video".into()),
            ..Default::default()
        };
        request.extras.insert("path".into(), "/new.mp4".into());

        let renderer_id = router
            .apply_assignment(ApplyAssignment {
                spawn_request: request,
                display_ids: Vec::new(),
                duplicate_renderers: false,
                wallpaper_layout_override: WallpaperLayoutOverride::default(),
                preempt_pending_start: false,
            })
            .await
            .unwrap()
            .renderer_id;

        assert_eq!(renderer_id, "r1");
        let inner = router.inner.lock().await;
        assert_eq!(inner.renderer_slots.len(), 1);
        let slot = inner.renderer_slots.get("r1").unwrap();
        assert_eq!(slot.spec_revision, 2);
        assert_eq!(slot.spawn_request.extras.get("path").unwrap(), "/new.mp4");
    }

    #[tokio::test]
    async fn clearing_manual_stop_preserves_auto_stop_inhibit() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        router.attach_settings(
            settings_with_auto_replay(auto_replay(&[(
                AutoCondition::Fullscreen,
                AutoAction::Stop,
            )]))
            .await,
        );
        let renderer = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;
        let display = router.register_display(reg("DP-1", 1920, 1080)).await;

        router.set_manual_stop(true).await;
        router
            .update_display_window_state(display.id, ar::FLAG_FULLSCREEN)
            .await;
        router.set_manual_stop(false).await;

        assert!(!router.snapshot_display(display.id).await.unwrap().links[0].active);
        assert!(matches!(
            router.snapshot_renderer("r1").await.unwrap().state,
            RendererLifecycleState::Stopped { keep: true, .. }
        ));
    }

    #[tokio::test]
    async fn manual_mute_uses_global_audio_fade() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let settings = test_settings_store().await;
        settings.update(|s| {
            s.global.audio_fade_ms = 750;
        });
        router.attach_settings(settings);

        let (r, peer) = RendererHandle::test_stub_with_peer("r1", "scene");
        peer.set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let reader = std::thread::spawn(move || {
            let mut got = Vec::new();
            while got.len() < 2 {
                let (msg, _fds) =
                    crate::wallframe::ipc::uds::recv_control(&peer).expect("recv control");
                match msg {
                    ControlMsg::Mute { transition } => got.push(("mute", transition.fade_ms)),
                    ControlMsg::Unmute { transition } => got.push(("unmute", transition.fade_ms)),
                    _ => {}
                }
            }
            got
        });

        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r.clone()).await;
        let _h = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;

        router.set_manual_mute(true).await;
        router.set_manual_mute(false).await;

        let got = reader.join().expect("reader joined");
        assert_eq!(got, vec![("mute", 750), ("unmute", 750)]);
    }

    #[tokio::test]
    async fn external_audio_composes_with_manual_mute() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());

        let (renderer, peer) = RendererHandle::test_stub_with_peer("r1", "scene");
        peer.set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let reader = std::thread::spawn(move || {
            let mut got = Vec::new();
            while got.len() < 2 {
                let (msg, _fds) =
                    crate::wallframe::ipc::uds::recv_control(&peer).expect("recv control");
                match msg {
                    ControlMsg::Mute { .. } => got.push("mute"),
                    ControlMsg::Unmute { .. } => got.push("unmute"),
                    _ => {}
                }
            }
            got
        });

        mgr.register_test_handle(renderer.clone()).await;
        router.register_renderer(renderer).await;
        let _display = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;

        router.set_other_playback_active(true).await;
        router.set_manual_mute(true).await;
        router.set_other_playback_active(false).await;
        assert!(router.is_muted("r1").await);
        router.set_manual_mute(false).await;

        assert_eq!(
            reader.join().expect("reader joined"),
            vec!["mute", "unmute"]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn auto_replay_pause_to_mute_restores_playback() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let settings = settings_with_auto_replay(auto_replay(&[
            (AutoCondition::Maximized, AutoAction::Pause),
            (AutoCondition::Focused, AutoAction::Mute),
        ]))
        .await;
        settings.update(|s| {
            s.global.audio_fade_ms = 750;
        });
        router.attach_settings(settings);

        let (r, peer) = RendererHandle::test_stub_with_peer("r1", "scene");
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r.clone()).await;
        let h = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;
        drain_renderer_controls(&peer);

        let reader = std::thread::spawn(move || {
            let mut got = Vec::new();
            while got.len() < 4 {
                let (msg, _fds) =
                    crate::wallframe::ipc::uds::recv_control(&peer).expect("recv control");
                match msg {
                    ControlMsg::Pause { transition } => got.push(("pause", transition.fade_ms)),
                    ControlMsg::Play { transition } => got.push(("play", transition.fade_ms)),
                    ControlMsg::Mute { transition } => got.push(("mute", transition.fade_ms)),
                    ControlMsg::Unmute { transition } => got.push(("unmute", transition.fade_ms)),
                    _ => {}
                }
            }
            got
        });

        router
            .update_display_window_state(h.id, ar::FLAG_NON_MINIMIZED | ar::FLAG_MAXIMIZED)
            .await;
        assert!(router.is_paused("r1").await);

        router
            .update_display_window_state(h.id, ar::FLAG_NON_MINIMIZED | ar::FLAG_ACTIVE)
            .await;
        assert!(router.is_muted("r1").await);

        router
            .update_display_window_state(h.id, ar::FLAG_NON_MINIMIZED)
            .await;

        assert!(!router.is_paused("r1").await);
        assert!(!router.is_muted("r1").await);
        let got = reader.join().expect("reader joined");
        assert_eq!(
            got,
            vec![("pause", 750), ("mute", 0), ("play", 0), ("unmute", 750)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn auto_replay_mute_to_pause_clears_mute_before_resume() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let settings = settings_with_auto_replay(auto_replay(&[
            (AutoCondition::Focused, AutoAction::Mute),
            (AutoCondition::Maximized, AutoAction::Pause),
        ]))
        .await;
        settings.update(|s| {
            s.global.audio_fade_ms = 640;
        });
        router.attach_settings(settings);

        let (r, peer) = RendererHandle::test_stub_with_peer("r1", "scene");
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r.clone()).await;
        let h = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;
        drain_renderer_controls(&peer);

        let reader = std::thread::spawn(move || {
            let mut got = Vec::new();
            while got.len() < 4 {
                let (msg, _fds) =
                    crate::wallframe::ipc::uds::recv_control(&peer).expect("recv control");
                match msg {
                    ControlMsg::Pause { transition } => got.push(("pause", transition.fade_ms)),
                    ControlMsg::Play { transition } => got.push(("play", transition.fade_ms)),
                    ControlMsg::Mute { transition } => got.push(("mute", transition.fade_ms)),
                    ControlMsg::Unmute { transition } => got.push(("unmute", transition.fade_ms)),
                    _ => {}
                }
            }
            got
        });

        router
            .update_display_window_state(h.id, ar::FLAG_NON_MINIMIZED | ar::FLAG_ACTIVE)
            .await;
        assert!(router.is_muted("r1").await);

        router
            .update_display_window_state(h.id, ar::FLAG_NON_MINIMIZED | ar::FLAG_MAXIMIZED)
            .await;
        assert!(router.is_paused("r1").await);

        router
            .update_display_window_state(h.id, ar::FLAG_NON_MINIMIZED)
            .await;

        assert!(!router.is_paused("r1").await);
        assert!(!router.is_muted("r1").await);
        let got = reader.join().expect("reader joined");
        assert_eq!(
            got,
            vec![("mute", 640), ("pause", 0), ("unmute", 0), ("play", 640)]
        );
    }

    #[tokio::test]
    async fn auto_replay_state_applies_after_relink_to_playing_renderer() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        router.attach_settings(
            settings_with_auto_replay(auto_replay(&[(
                AutoCondition::Fullscreen,
                AutoAction::Pause,
            )]))
            .await,
        );

        let r1 = RendererHandle::test_stub("r1", "scene");
        r1.test_publish_pool(fake_published_pool(1, 1920, 1080));
        mgr.register_test_handle(r1.clone()).await;
        router.register_renderer(r1.clone()).await;
        let r2 = RendererHandle::test_stub("r2", "scene");
        r2.test_publish_pool(fake_published_pool(1, 1920, 1080));
        mgr.register_test_handle(r2.clone()).await;
        router.register_renderer(r2.clone()).await;

        let a = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;
        let b = router.register_display(reg("DP-1", 1920, 1080)).await;
        router.relink_displays_to(&[b.id], "r2").await;
        assert!(!router.is_paused("r2").await);

        router
            .update_display_window_state(a.id, ar::FLAG_NON_MINIMIZED | ar::FLAG_FULLSCREEN)
            .await;
        assert!(router.is_paused("r1").await);
        assert!(!router.is_paused("r2").await);

        router.relink_displays_to(&[a.id], "r2").await;

        assert!(router.is_paused("r2").await);
    }

    #[tokio::test(start_paused = true)]
    async fn auto_replay_resume_is_immediate() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        router.attach_settings(
            settings_with_auto_replay(auto_replay(&[(
                AutoCondition::Fullscreen,
                AutoAction::Pause,
            )]))
            .await,
        );
        let (r, _peer) = RendererHandle::test_stub_with_peer("r1", "scene");
        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r.clone()).await;
        let h = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;

        // Pause.
        router
            .update_display_window_state(h.id, ar::FLAG_NON_MINIMIZED | ar::FLAG_FULLSCREEN)
            .await;
        assert!(router.is_paused("r1").await);

        // Flag drops -> state machine resumes immediately.
        router
            .update_display_window_state(h.id, ar::FLAG_NON_MINIMIZED)
            .await;
        assert!(!router.is_paused("r1").await);
    }

    #[tokio::test(start_paused = true)]
    async fn auto_replay_pause_reapplies_after_immediate_resume() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        router.attach_settings(
            settings_with_auto_replay(auto_replay(&[(
                AutoCondition::Fullscreen,
                AutoAction::Pause,
            )]))
            .await,
        );
        let (r, _peer) = RendererHandle::test_stub_with_peer("r1", "scene");
        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r.clone()).await;
        let h = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;

        // Pause, resume, then immediately re-enter fullscreen.
        router
            .update_display_window_state(h.id, ar::FLAG_NON_MINIMIZED | ar::FLAG_FULLSCREEN)
            .await;
        router
            .update_display_window_state(h.id, ar::FLAG_NON_MINIMIZED)
            .await;
        assert!(!router.is_paused("r1").await);
        router
            .update_display_window_state(h.id, ar::FLAG_NON_MINIMIZED | ar::FLAG_FULLSCREEN)
            .await;
        assert!(router.is_paused("r1").await);
    }

    #[test]
    fn renderer_resume_retry_delay_increases_and_caps() {
        assert_eq!(resume_retry_delay(1), Duration::from_millis(100));
        assert_eq!(resume_retry_delay(2), Duration::from_secs(2));
        assert_eq!(resume_retry_delay(3), Duration::from_secs(5));
        assert_eq!(resume_retry_delay(4), Duration::from_secs(10));
        assert_eq!(resume_retry_delay(32), RESUME_RETRY_MAX);
    }

    #[tokio::test(start_paused = true)]
    async fn renderer_resume_failures_increase_retry_count() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        router.attach_settings(
            settings_with_auto_replay(auto_replay(&[(
                AutoCondition::Fullscreen,
                AutoAction::Pause,
            )]))
            .await,
        );
        let r = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r).await;
        let h = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;

        router
            .update_display_window_state(h.id, ar::FLAG_NON_MINIMIZED | ar::FLAG_FULLSCREEN)
            .await;
        router
            .update_display_window_state(h.id, ar::FLAG_NON_MINIMIZED)
            .await;
        assert_eq!(
            router
                .inner
                .lock()
                .await
                .resume_retries
                .get("r1")
                .map(|retry| retry.failures),
            Some(1)
        );

        tokio::task::yield_now().await;
        tokio::time::advance(RESUME_RETRY_INITIAL).await;
        for _ in 0..50 {
            tokio::task::yield_now().await;
            if router
                .inner
                .lock()
                .await
                .resume_retries
                .get("r1")
                .is_some_and(|retry| retry.failures == 2)
            {
                return;
            }
        }
        panic!("second resume failure was not recorded");
    }

    #[tokio::test(start_paused = true)]
    async fn renderer_resume_retry_clears_after_success() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let (r, _peer) = RendererHandle::test_stub_with_peer("r1", "scene");
        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r).await;
        let _display = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;

        router
            .schedule_resume_retry("r1", ResumeControl::Play { fade_ms: 0 })
            .await;
        assert!(router.inner.lock().await.resume_retries.contains_key("r1"));

        tokio::task::yield_now().await;
        tokio::time::advance(RESUME_RETRY_INITIAL).await;
        for _ in 0..50 {
            tokio::task::yield_now().await;
            if !router.inner.lock().await.resume_retries.contains_key("r1") {
                return;
            }
        }
        panic!("successful resume retry did not clear backoff state");
    }

    #[tokio::test(start_paused = true)]
    async fn renderer_resume_retry_is_cancelled_by_pause() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let (r, _peer) = RendererHandle::test_stub_with_peer("r1", "scene");
        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r).await;
        let _display = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;

        router
            .schedule_resume_retry("r1", ResumeControl::Play { fade_ms: 0 })
            .await;
        router.set_manual_pause(true).await;
        assert!(!router.inner.lock().await.resume_retries.contains_key("r1"));

        tokio::time::advance(RESUME_RETRY_INITIAL).await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(router.is_paused("r1").await);
        assert!(!router.inner.lock().await.resume_retries.contains_key("r1"));
    }

    #[tokio::test]
    async fn auto_replay_none_action_is_inert() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        router.attach_settings(
            settings_with_auto_replay(auto_replay(&[(
                AutoCondition::Fullscreen,
                AutoAction::None,
            )]))
            .await,
        );
        let r = RendererHandle::test_stub("r1", "scene");
        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r.clone()).await;
        let h = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;

        router
            .update_display_window_state(h.id, ar::FLAG_NON_MINIMIZED | ar::FLAG_FULLSCREEN)
            .await;
        assert!(!router.is_paused("r1").await);
    }

    #[tokio::test]
    async fn update_display_metrics_zero_dim_ignored() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let r = RendererHandle::test_stub("r1", "scene");
        r.test_publish_pool(fake_published_pool(1, 1920, 1080));
        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r.clone()).await;

        let mut h = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;
        let _ = last_composition_config(&mut h.rx);

        // Zero dim → drop on the floor; field stays at 1920x1080.
        router
            .set_display_metrics(
                h.id,
                DisplayMetrics {
                    width: 0,
                    height: 720,
                    refresh_mhz: 60_000,
                },
            )
            .await;
        router
            .set_display_metrics(
                h.id,
                DisplayMetrics {
                    width: 1280,
                    height: 0,
                    refresh_mhz: 60_000,
                },
            )
            .await;
        assert!(last_composition_config(&mut h.rx).is_none());
        let snap = router.snapshot_display(h.id).await.unwrap();
        assert_eq!((snap.width, snap.height), (1920, 1080));
    }
}
