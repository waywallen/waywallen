use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, broadcast::error::RecvError, mpsc, Mutex as TokioMutex, Notify};
use tokio::task::JoinHandle;

/// Grace period an orphan renderer keeps running before it is killed.
/// Only granted to the last renderer while the daemon has zero displays.
const ORPHAN_REAP_TIMEOUT: Duration = Duration::from_secs(5);
const AUTO_REPLAY_RESUME_DELAY: Duration = Duration::from_millis(500);

use crate::display::layout::{FillMode, LayoutInput};
use crate::ipc::proto::{ControlMsg, EventMsg};
use crate::renderer_manager::{DrmNode, RendererHandle, RendererId, RendererManager};
use crate::scheduler::{DisplayId, DisplayInfo, ProjectedConfig};
use crate::settings::{AutoAction, AutoReplayPolicy, ResolvedLayout, SettingsStore};
use crate::wallpaper::properties::WallpaperLayoutOverride;

use super::auto_replay;
use super::table::{Link, LinkDstRect, LinkId, LinkSrcRect, RoutingTable};

/// Wire-translated event streamed from router to a display endpoint.
/// The endpoint owns translation to the on-the-wire `Event`.
pub enum DisplayOutEvent {
    /// Bind the buffer pool currently published by `renderer`. The
    /// endpoint reads the snapshot from the handle.
    Bind { renderer: Arc<RendererHandle> },
    /// Retire the named buffer pool generation.
    Unbind { buffer_generation: u64 },
    /// Update composition geometry / clear color.
    SetConfig(ProjectedConfig),
    /// A frame is ready on `renderer` at `buffer_index` for the named
    /// generation. The endpoint pulls the matching sync_fd from the handle.
    Frame {
        renderer: Arc<RendererHandle>,
        buffer_generation: u64,
        buffer_index: u32,
        seq: u64,
        /// Timeline value the producer assigned to this frame on its
        /// release_syncobj; the endpoint records it for release tracking.
        release_point: u64,
        /// Total number of consumer endpoints the router dispatched
        /// this release point to, used by the reaper as fan-out width.
        expected_count: u32,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct AutoStopEvent {
    pub display_id: DisplayId,
    pub stopped: bool,
}

enum AutoStateAction {
    Reconcile,
    ScheduleResume { display_id: DisplayId, gen: u64 },
    Noop,
}

/// Initial-registration payload from `display::endpoint::do_handshake`.
pub struct DisplayRegistration {
    pub name: String,
    /// Stable identifier persisted by the consumer (e.g. UUID4 stored in
    /// the shell extension config). Used as the settings key when present.
    pub instance_id: Option<String>,
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
    /// DRM render-node id of the GPU this display will sample dmabufs
    /// on (i.e. the GPU backing the consumer's EGL/Vulkan context).
    pub gpu: DrmNode,
    pub properties: Vec<(String, String)>,
    /// Modifier-negotiation capabilities the consumer declared in
    /// its `consumer_caps` request.
    pub consumer_caps: Option<crate::dma::negotiate::PeerCaps>,
}

/// Returned from `register_display` — the assigned id plus the rx end
/// of the dispatcher's per-display channel.
pub struct DisplayHandle {
    pub id: DisplayId,
    pub rx: mpsc::UnboundedReceiver<DisplayOutEvent>,
}

/// Read-only view of a single (renderer → display) link for UI
/// consumers. Subset of `table::Link` that hides table-internal ids.
#[derive(Debug, Clone)]
pub struct DisplayLinkSnapshot {
    pub renderer_id: RendererId,
    pub z_order: i32,
}

/// Transport-agnostic router event. `ws_server` subscribes and
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
pub enum PausedRendererStatus {
    Muted,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManualLifecycleState {
    pub paused: bool,
    pub muted: bool,
}

/// Lifecycle state of a renderer as seen by the router.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererStatus {
    Playing,
    Paused(PausedRendererStatus),
}

impl Default for RendererStatus {
    fn default() -> Self {
        Self::Playing
    }
}

impl RendererStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Playing => "playing",
            Self::Paused(status) => match status {
                PausedRendererStatus::Muted => "muted",
                PausedRendererStatus::Paused => "paused",
            },
        }
    }
}

/// Read-only view of a registered renderer. Returned from
/// `Router::snapshot_renderers`; mirrors UI-visible renderer fields.
#[derive(Debug, Clone)]
pub struct RendererSnapshot {
    pub id: RendererId,
    pub wp_type: String,
    pub name: String,
    pub status: RendererStatus,
    pub pid: u32,
    pub drm_render_major: u32,
    pub drm_render_minor: u32,
    pub texture_width: u32,
    pub texture_height: u32,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutSource {
    Global,
    Display,
    Wallpaper,
}

struct DisplayState {
    info: DisplayInfo,
    /// DRM render-node id of the consumer's GPU. Compared against
    /// `RendererHandle::gpu` during DMA-BUF negotiation.
    gpu: DrmNode,
    tx: mpsc::UnboundedSender<DisplayOutEvent>,
    /// Last renderer this display was bound to (None if currently unbound).
    last_renderer: Option<RendererId>,
    /// Last `buffer_generation` we sent in a `Bind` to this display.
    /// Tracked so a follow-up `Unbind` retires the right gen.
    last_buffer_generation: Option<u64>,
    /// Consumer's modifier-negotiation caps. `None` until the
    /// consumer_caps request is received.
    consumer_caps: Option<crate::dma::negotiate::PeerCaps>,
    /// Per-display auto replay machine driven by display facts and
    /// the resolved rule policy.
    auto_replay: auto_replay::State,
}

struct Inner {
    table: RoutingTable,
    displays: HashMap<DisplayId, DisplayState>,
    renderer_tasks: HashMap<RendererId, JoinHandle<()>>,
    /// The states of the paused renderers we know about.
    /// Used to compute play/pause and mute/unmute state.
    renderer_states: HashMap<RendererId, PausedRendererStatus>,
    /// Set when the screen-saver / lock-screen is active.
    session_locked: bool,
    /// Set when the current login session is inactive.
    session_inactive: bool,
    /// User-requested global pause state. This shares the same
    /// daemon-owned lifecycle path as auto replay.
    manual_paused: bool,
    manual_muted: bool,
    /// Pending orphan-reap timers, keyed by renderer id. Inserted by
    /// `mark_orphan` and cleared by `cancel_orphan_timer`.
    orphan_timers: HashMap<RendererId, JoinHandle<()>>,
    /// Per-renderer set of (display_id, buffer_generation) pairs we've
    /// emitted `Unbind` for and are waiting to be acked.
    unbind_acks_pending: HashMap<RendererId, HashSet<(DisplayId, u64)>>,
    wallpaper_layout_overrides: HashMap<RendererId, WallpaperLayoutOverride>,
    next_display_id: u64,
    next_config_generation: u64,
}

pub struct Router {
    inner: TokioMutex<Inner>,
    /// Renderer manager used for pause/play lifecycle control.
    mgr: Arc<RendererManager>,
    /// Fan-out channel for `RouterEvent`s. Always present; `send` errors
    /// when there are no subscribers are logged at debug and ignored.
    events_tx: broadcast::Sender<RouterEvent>,
    auto_stop_tx: broadcast::Sender<AutoStopEvent>,
    /// Settings store used to resolve per-display fillmode/align when
    /// computing set_config. Set once at startup.
    settings: std::sync::OnceLock<Arc<SettingsStore>>,
    /// Wakes any task currently inside `await_unbind_acks_for` whenever
    /// `record_unbind_done` mutates `unbind_acks_pending`.
    unbind_ack_notify: Notify,
}

impl Router {
    /// Borrow the underlying RendererManager. Used by the display
    /// endpoint to forward pointer events to the bound renderer.
    pub fn renderer_manager(&self) -> &Arc<RendererManager> {
        &self.mgr
    }

    pub fn new(mgr: Arc<RendererManager>) -> Arc<Self> {
        let (events_tx, _) = broadcast::channel(128);
        let (auto_stop_tx, _) = broadcast::channel(128);
        let router = Arc::new(Self {
            inner: TokioMutex::new(Inner {
                table: RoutingTable::new(),
                displays: HashMap::new(),
                renderer_tasks: HashMap::new(),
                renderer_states: HashMap::new(),
                orphan_timers: HashMap::new(),
                unbind_acks_pending: HashMap::new(),
                wallpaper_layout_overrides: HashMap::new(),
                next_display_id: 0,
                next_config_generation: 0,
                session_locked: false,
                session_inactive: false,
                manual_paused: false,
                manual_muted: false,
            }),
            mgr,
            events_tx,
            auto_stop_tx,
            settings: std::sync::OnceLock::new(),
            unbind_ack_notify: Notify::new(),
        });
        router
    }

    /// Wire the daemon's `SettingsStore` so `sync_display` can resolve
    /// per-display layout when projecting set_config.
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
        new_fillmode: Option<crate::display::layout::FillMode>,
        new_location: Option<crate::display::layout::Location>,
        new_align: Option<crate::display::layout::Align>,
        new_rotation: Option<crate::display::layout::Rotation>,
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
        self.resync_display_set_config(target_id).await;
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

    /// Re-emit `set_config` for a single display to pick up new
    /// settings without rebinding buffers.
    async fn resync_display_set_config(self: &Arc<Self>, display_id: DisplayId) {
        let mut inner = self.inner.lock().await;
        if !inner.displays.contains_key(&display_id) {
            return;
        }
        let display_links = inner.table.links_for_display(display_id);
        let target = display_links.into_iter().find(|l| l.enabled).and_then(|l| {
            let renderer = inner.table.get_renderer(&l.renderer_id)?;
            let gen = renderer
                .bind_snapshot()
                .lock()
                .ok()
                .and_then(|g| g.as_ref().map(|s| s.generation))?;
            Some((l, renderer, gen))
        });
        let Some((link, renderer, _gen)) = target else {
            return;
        };
        inner.next_config_generation += 1;
        let cfg_gen = inner.next_config_generation;
        let info = inner.displays.get(&display_id).unwrap().info.clone();
        let layout = self.resolved_layout_for_renderer(&info, &link.renderer_id, &inner);
        let cfg = project_link(&link, &renderer, &info, cfg_gen, &layout);
        if let Some(state) = inner.displays.get(&display_id) {
            let _ = state.tx.send(DisplayOutEvent::SetConfig(cfg));
        }
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

    /// Re-emit `set_config` for every registered display. Called from
    /// the control surface after global layout settings change.
    pub async fn resync_all_set_configs(self: &Arc<Self>) {
        let ids: Vec<DisplayId> = {
            let inner = self.inner.lock().await;
            inner.displays.keys().copied().collect()
        };
        for did in ids {
            self.resync_display_set_config(did).await;
        }
    }

    /// Push a DisplaysReplace router event after a settings-only
    /// change so subscribed UIs refresh effective layout fields.
    pub fn emit_displays_replace_for_settings_change(self: &Arc<Self>, snap: Vec<DisplaySnapshot>) {
        self.emit(RouterEvent::DisplaysReplace(snap));
    }

    // ---------------------------------------------------------------
    // Renderer lifecycle

    pub async fn register_renderer(self: &Arc<Self>, handle: Arc<RendererHandle>) {
        let id = handle.id.clone();
        let task = {
            let mut events = handle.events();
            let router = Arc::clone(self);
            let rid = id.clone();
            tokio::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(EventMsg::BindBuffers { .. }) => {
                            router.on_renderer_bind(&rid).await;
                        }
                        Ok(EventMsg::FrameReady {
                            image_index,
                            seq,
                            release_point,
                            ..
                        }) => {
                            router
                                .on_renderer_frame(&rid, image_index, seq, release_point)
                                .await;
                        }
                        Ok(EventMsg::FormatCaps { .. }) => {
                            // Renderer caps arrived; recompute negotiation
                            // for affected display links.
                            router.reconcile_buffer_flags().await;
                        }
                        Ok(EventMsg::BindFailed {
                            fourcc, modifier, ..
                        }) => {
                            // Renderer rejected the picked format; blacklist
                            // it on the producer side and retry.
                            router.on_renderer_bind_failed(&rid, fourcc, modifier).await;
                        }
                        Ok(EventMsg::ReportState { .. }) => {
                            // Reader parsed recognised keys onto the handle;
                            // resync display config from that cached state.
                            router.on_renderer_state_changed(&rid).await;
                        }
                        Ok(_) => {}
                        Err(RecvError::Closed) => {
                            log::info!("router: renderer {rid} broadcast closed");
                            return;
                        }
                        Err(RecvError::Lagged(n)) => {
                            log::warn!("router: renderer {rid} lagged {n} events");
                        }
                    }
                }
            })
        };
        {
            let mut inner = self.inner.lock().await;
            inner.table.add_renderer(handle);
            inner.renderer_tasks.insert(id, task);
        }
        self.reconcile_lifecycle().await;
    }

    pub async fn unregister_renderer(self: &Arc<Self>, id: &str) {
        let affected: Vec<DisplayId> = {
            let mut inner = self.inner.lock().await;
            let removed = inner.table.remove_renderer(id);
            inner.wallpaper_layout_overrides.remove(id);
            if let Some(task) = inner.renderer_tasks.remove(id) {
                task.abort();
            }
            if let Some(task) = inner.orphan_timers.remove(id) {
                task.abort();
            }
            inner.renderer_states.remove(id);
            removed.into_iter().map(|(_, did)| did).collect()
        };
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

    pub async fn set_renderer_wallpaper_layout_override(
        self: &Arc<Self>,
        renderer_id: &str,
        layout: WallpaperLayoutOverride,
    ) -> bool {
        let display_ids: Vec<DisplayId> = {
            let mut inner = self.inner.lock().await;
            if inner.table.get_renderer(renderer_id).is_none() {
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
        for did in &display_ids {
            self.resync_display_set_config(*did).await;
        }
        if !display_ids.is_empty() {
            let all = self.snapshot_displays().await;
            self.emit(RouterEvent::DisplaysReplace(all));
        }
        true
    }

    /// Arm `unbind_done` ack tracking for `renderer_id`. MUST be called
    /// before any sync_display that emits Unbind for this renderer.
    pub async fn begin_unbind_ack_tracking(self: &Arc<Self>, renderer_id: &str) {
        let mut inner = self.inner.lock().await;
        inner
            .unbind_acks_pending
            .entry(renderer_id.to_string())
            .or_insert_with(HashSet::new);
    }

    /// Record an `unbind_done` request from a display for a specific
    /// generation, draining the matching pending pair if present.
    pub async fn record_unbind_done(
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
                // so concurrent record_unbind_done cannot be missed.
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
        let (display_id, auto_linked) = {
            let mut inner = self.inner.lock().await;
            inner.next_display_id += 1;
            let id = inner.next_display_id;
            let info = DisplayInfo {
                id,
                name: reg.name,
                instance_id: reg.instance_id,
                width: reg.width,
                height: reg.height,
                refresh_mhz: reg.refresh_mhz,
                properties: reg.properties,
                bound: false,
            };
            inner.displays.insert(
                id,
                DisplayState {
                    info,
                    gpu: reg.gpu,
                    tx,
                    last_renderer: None,
                    last_buffer_generation: None,
                    consumer_caps: reg.consumer_caps,
                    auto_replay: auto_replay::State::new(),
                },
            );
            // Auto-link to whichever renderer is first in the routing table.
            let auto = inner.table.first_renderer();
            if let Some(rid) = auto.clone() {
                inner.table.add_link(rid, id);
            }
            (id, auto)
        };
        // A freshly auto-linked renderer just gained an audience —
        // cancel any pending orphan timer so it survives.
        if let Some(rid) = auto_linked.as_deref() {
            self.cancel_orphan_timer(rid).await;
        }
        self.sync_display(display_id).await;
        self.reconcile_lifecycle().await;
        self.reconcile_buffer_flags().await;
        if let Some(snap) = self.snapshot_display(display_id).await {
            self.emit(RouterEvent::DisplayUpsert(snap));
        }
        DisplayHandle { id: display_id, rx }
    }

    pub async fn unregister_display(self: &Arc<Self>, display_id: DisplayId) {
        {
            let mut inner = self.inner.lock().await;
            inner.displays.remove(&display_id);
            inner.table.remove_display(display_id);
        }
        // Any renderer that just lost its last link enters the 5s
        // grace window; no new renderer is protected during unplug.
        self.mark_orphans(None).await;
        self.reconcile_lifecycle().await;
        self.reconcile_buffer_flags().await;
        self.emit(RouterEvent::DisplayRemoved(display_id));
    }

    /// Stash the consumer's modifier-negotiation caps on the
    /// display state and re-run the picker. Later caps replace earlier ones.
    pub async fn set_consumer_caps(
        self: &Arc<Self>,
        display_id: DisplayId,
        caps: crate::dma::negotiate::PeerCaps,
    ) {
        {
            let mut inner = self.inner.lock().await;
            if let Some(s) = inner.displays.get_mut(&display_id) {
                s.consumer_caps = Some(caps);
            } else {
                return;
            }
        }
        self.reconcile_buffer_flags().await;
    }

    /// Consumer reported `bind_failed` for `(fourcc, modifier)`.
    /// Add the pair to this consumer's blacklist and retry negotiation.
    pub async fn on_consumer_bind_failed(
        self: &Arc<Self>,
        display_id: DisplayId,
        fourcc: u32,
        modifier: u64,
    ) {
        let inserted = {
            let mut inner = self.inner.lock().await;
            let Some(state) = inner.displays.get_mut(&display_id) else {
                return;
            };
            let Some(caps) = state.consumer_caps.as_mut() else {
                return;
            };
            caps.blacklist.insert((fourcc, modifier))
        };
        if inserted {
            log::info!(
                "router: display {display_id}: blacklisted (0x{fourcc:08x}, 0x{modifier:x}) — re-running picker"
            );
        }
        self.reconcile_buffer_flags().await;
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
    /// already merged recognised keys onto the handle.
    pub async fn on_renderer_state_changed(self: &Arc<Self>, renderer_id: &str) {
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
                .map(|l| l.id)
                .collect();
            let mut affected = Vec::new();
            for lid in link_ids {
                let changed =
                    inner
                        .table
                        .update_link_geometry(lid, None, None, None, Some(new_clear), None);
                if changed {
                    if let Some(link) = inner.table.get_link(lid) {
                        affected.push(link.display_id);
                    }
                }
            }
            affected
        };
        for did in affected {
            self.resync_display_set_config(did).await;
        }
    }

    pub async fn update_display_size(
        self: &Arc<Self>,
        display_id: DisplayId,
        width: u32,
        height: u32,
    ) {
        if width == 0 || height == 0 {
            log::warn!(
                "update_display_size: ignoring zero dim ({width}x{height}) for display {display_id:?}",
            );
            return;
        }
        let changed = {
            let mut inner = self.inner.lock().await;
            if let Some(s) = inner.displays.get_mut(&display_id) {
                let differs = s.info.width != width || s.info.height != height;
                s.info.width = width;
                s.info.height = height;
                differs
            } else {
                return;
            }
        };
        // Layout depends on disp_w/disp_h, so any size change must
        // trigger a fresh set_config under the resolved fillmode/align.
        if changed {
            self.resync_display_set_config(display_id).await;
        }
        if let Some(snap) = self.snapshot_display(display_id).await {
            self.emit(RouterEvent::DisplayUpsert(snap));
        }
    }

    /// Update the per-display auto replay machine from a consumer's
    /// `window_state` request.
    pub async fn update_display_window_state(self: &Arc<Self>, display_id: DisplayId, flags: u32) {
        let action = self.update_auto_state(display_id, Some(flags)).await;
        self.run_auto_state_action(action).await;
    }

    /// Update the session-level state driven by the
    /// `session_monitor` task. `None` leaves that flag unchanged.
    pub async fn is_session_locked(&self) -> bool {
        self.inner.lock().await.session_locked
    }

    pub async fn update_session_state(
        self: &Arc<Self>,
        locked: Option<bool>,
        inactive: Option<bool>,
    ) {
        let display_ids = {
            let mut inner = self.inner.lock().await;
            let mut changed = false;
            if let Some(v) = locked {
                if inner.session_locked != v {
                    inner.session_locked = v;
                    changed = true;
                }
            }
            if let Some(v) = inactive {
                if inner.session_inactive != v {
                    inner.session_inactive = v;
                    changed = true;
                }
            }
            if !changed {
                Vec::new()
            } else {
                inner.displays.keys().copied().collect()
            }
        };
        for display_id in display_ids {
            let action = self.update_auto_state(display_id, None).await;
            self.run_auto_state_action(action).await;
        }
    }

    async fn update_auto_state(
        self: &Arc<Self>,
        display_id: DisplayId,
        flags: Option<u32>,
    ) -> AutoStateAction {
        let mut inner = self.inner.lock().await;
        let session_locked = inner.session_locked;
        let session_inactive = inner.session_inactive;
        let Some(state) = inner.displays.get_mut(&display_id) else {
            return AutoStateAction::Noop;
        };
        let next_flags = flags.unwrap_or(state.auto_replay.last_flags);
        let policy = self.resolved_auto_replay(&state.info);
        let new_raw = auto_replay::decide(
            &policy,
            auto_replay::Facts {
                flags: next_flags,
                session_locked,
                session_inactive,
            },
        );
        let same_input = flags.is_some_and(|v| v == state.auto_replay.last_flags);
        if flags.is_some() {
            state.auto_replay.last_flags = next_flags;
        }
        if same_input && new_raw == state.auto_replay.raw {
            return AutoStateAction::Noop;
        }
        state.auto_replay.raw = new_raw;
        if new_raw.is_active() {
            state.auto_replay.gen = state.auto_replay.gen.wrapping_add(1);
            if state.auto_replay.requested != new_raw {
                state.auto_replay.requested = new_raw;
                AutoStateAction::Reconcile
            } else {
                AutoStateAction::Noop
            }
        } else if state.auto_replay.requested.is_active() {
            state.auto_replay.gen = state.auto_replay.gen.wrapping_add(1);
            AutoStateAction::ScheduleResume {
                display_id,
                gen: state.auto_replay.gen,
            }
        } else {
            state.auto_replay.requested = new_raw;
            AutoStateAction::Noop
        }
    }

    async fn run_auto_state_action(self: &Arc<Self>, action: AutoStateAction) {
        match action {
            AutoStateAction::Noop => {}
            AutoStateAction::Reconcile => {
                self.apply_auto_stop_links().await;
                self.reconcile_lifecycle().await;
            }
            AutoStateAction::ScheduleResume { display_id, gen } => {
                let router = Arc::clone(self);
                tokio::spawn(async move {
                    tokio::time::sleep(AUTO_REPLAY_RESUME_DELAY).await;
                    let need_reconcile = {
                        let mut inner = router.inner.lock().await;
                        let Some(state) = inner.displays.get_mut(&display_id) else {
                            return;
                        };
                        if state.auto_replay.gen != gen || state.auto_replay.raw.is_active() {
                            return;
                        }
                        if state.auto_replay.requested.is_active() {
                            state.auto_replay.requested = state.auto_replay.raw;
                            true
                        } else {
                            false
                        }
                    };
                    if need_reconcile {
                        router.apply_auto_stop_links().await;
                        router.reconcile_lifecycle().await;
                    }
                });
            }
        }
    }

    async fn apply_auto_stop_links(self: &Arc<Self>) {
        let mut changed_displays = Vec::new();
        let mut stop_events = Vec::new();
        let mut reenabled_renderers = Vec::new();
        let mut disabled_any = false;
        {
            let mut inner = self.inner.lock().await;
            let plans: Vec<(DisplayId, bool)> = inner
                .displays
                .iter()
                .filter_map(|(display_id, state)| {
                    let should_stop = state.auto_replay.requested.action == AutoAction::Stop;
                    (state.auto_replay.stop_applied != should_stop)
                        .then_some((*display_id, should_stop))
                })
                .collect();
            for (display_id, should_stop) in plans {
                if let Some(state) = inner.displays.get_mut(&display_id) {
                    state.auto_replay.stop_applied = should_stop;
                }
                for link in inner.table.links_for_display(display_id) {
                    if inner.table.set_link_enabled(link.id, !should_stop) {
                        if should_stop {
                            disabled_any = true;
                        } else {
                            reenabled_renderers.push(link.renderer_id);
                        }
                    }
                }
                changed_displays.push(display_id);
                stop_events.push(AutoStopEvent {
                    display_id,
                    stopped: should_stop,
                });
            }
        }
        for renderer_id in reenabled_renderers {
            self.cancel_orphan_timer(&renderer_id).await;
        }
        for display_id in &changed_displays {
            self.sync_display(*display_id).await;
        }
        if disabled_any {
            self.mark_orphans(None).await;
        }
        if !changed_displays.is_empty() {
            self.reconcile_buffer_flags().await;
            let all = self.snapshot_displays().await;
            self.emit(RouterEvent::DisplaysReplace(all));
        }
        for evt in stop_events {
            if let Err(e) = self.auto_stop_tx.send(evt) {
                log::debug!("router: no auto-stop subscribers ({e})");
            }
        }
    }

    pub async fn set_manual_pause(self: &Arc<Self>, paused: bool) {
        let changed = {
            let mut inner = self.inner.lock().await;
            if inner.manual_paused == paused {
                false
            } else {
                inner.manual_paused = paused;
                true
            }
        };
        if changed {
            self.reconcile_lifecycle().await;
        }
    }

    pub async fn toggle_manual_pause(self: &Arc<Self>) -> bool {
        let paused = {
            let mut inner = self.inner.lock().await;
            inner.manual_paused = !inner.manual_paused;
            inner.manual_paused
        };
        self.reconcile_lifecycle().await;
        paused
    }

    pub async fn set_manual_mute(self: &Arc<Self>, muted: bool) {
        let changed = {
            let mut inner = self.inner.lock().await;
            if inner.manual_muted == muted {
                false
            } else {
                inner.manual_muted = muted;
                true
            }
        };
        if changed {
            self.reconcile_lifecycle().await;
        }
    }

    pub async fn toggle_manual_mute(self: &Arc<Self>) -> bool {
        let muted = {
            let mut inner = self.inner.lock().await;
            inner.manual_muted = !inner.manual_muted;
            inner.manual_muted
        };
        self.reconcile_lifecycle().await;
        muted
    }

    pub async fn manual_lifecycle_state(self: &Arc<Self>) -> ManualLifecycleState {
        let inner = self.inner.lock().await;
        ManualLifecycleState {
            paused: inner.manual_paused,
            muted: inner.manual_muted,
        }
    }

    /// Whether this renderer is currently in the paused set (zero
    /// enabled links). Returns `false` for unknown ids.
    pub async fn is_paused(self: &Arc<Self>, renderer_id: &str) -> bool {
        self.inner
            .lock()
            .await
            .renderer_states
            .get(renderer_id)
            .is_some_and(|status| *status == PausedRendererStatus::Paused)
    }

    pub async fn is_muted(self: &Arc<Self>, renderer_id: &str) -> bool {
        self.inner
            .lock()
            .await
            .renderer_states
            .get(renderer_id)
            .is_some_and(|status| *status == PausedRendererStatus::Muted)
    }

    /// Subscribe to router events (display add/change/remove). The
    /// returned receiver is lagged-on-overflow.
    pub fn subscribe_events(self: &Arc<Self>) -> broadcast::Receiver<RouterEvent> {
        self.events_tx.subscribe()
    }

    pub fn subscribe_auto_stop(self: &Arc<Self>) -> broadcast::Receiver<AutoStopEvent> {
        self.auto_stop_tx.subscribe()
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

    /// Walk every renderer in the table and schedule a 5s reap timer
    /// for those with no enabled links, except the optional `keep` id.
    pub async fn mark_orphans(self: &Arc<Self>, keep: Option<&str>) -> Vec<RendererId> {
        // Snapshot candidates and grace eligibility in one critical section
        // so all orphans in this batch agree on policy.
        let (candidates, lone_renderer_no_displays) = {
            let inner = self.inner.lock().await;
            let cs: Vec<RendererId> = inner
                .table
                .renderer_ids()
                .into_iter()
                .filter(|rid| {
                    if Some(rid.as_str()) == keep {
                        return false;
                    }
                    inner
                        .table
                        .links_for_renderer(rid)
                        .iter()
                        .all(|l| !l.enabled)
                })
                .collect();
            let lone = inner.displays.is_empty() && inner.table.renderer_ids().len() == 1;
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
            inner.displays.is_empty() && inner.table.renderer_ids().len() == 1
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
        self.unregister_renderer(renderer_id).await;
        if let Err(e) = self.mgr.kill(renderer_id).await {
            log::warn!("router: kill orphan {renderer_id}: {e}");
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
            // Renderer might have been removed via `unregister_renderer`
            // already (manual kill, etc.) — bail in that case.
            if !inner.table.renderer_ids().iter().any(|r| r == renderer_id) {
                return;
            }
            inner
                .table
                .links_for_renderer(renderer_id)
                .iter()
                .all(|l| !l.enabled)
        };
        if !still_orphan {
            return;
        }
        log::info!("router: reaping orphan renderer {renderer_id} after grace");
        self.unregister_renderer(renderer_id).await;
        if let Err(e) = self.mgr.kill(renderer_id).await {
            log::warn!("router: kill orphan {renderer_id}: {e}");
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
        let link_rows: Vec<Link> = inner
            .table
            .links_for_display(id)
            .into_iter()
            .filter(|l| l.enabled)
            .collect();
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
            })
            .collect();
        Some(DisplaySnapshot {
            id,
            name: s.info.name.clone(),
            instance_id: s.info.instance_id.clone(),
            width: s.info.width,
            height: s.info.height,
            refresh_mhz: s.info.refresh_mhz,
            links,
            drm_render_major: s.gpu.major,
            drm_render_minor: s.gpu.minor,
            display_layout,
            effective_layout,
            effective_layout_source,
        })
    }

    /// Snapshot of a single renderer by id. Returns `None` if the
    /// renderer has been unregistered from the routing table.
    pub async fn snapshot_renderer(self: &Arc<Self>, id: &str) -> Option<RendererSnapshot> {
        let inner = self.inner.lock().await;
        let handle = inner.table.get_renderer(id)?;
        let status = inner
            .renderer_states
            .get(id)
            .map(|status| RendererStatus::Paused(*status))
            .unwrap_or(RendererStatus::Playing);
        let (tw, th) = handle.texture_size();
        Some(RendererSnapshot {
            id: handle.id.clone(),
            wp_type: handle.wp_type.clone(),
            name: handle.name.clone(),
            status,
            pid: handle.pid.unwrap_or(0),
            drm_render_major: handle.gpu.major,
            drm_render_minor: handle.gpu.minor,
            texture_width: tw,
            texture_height: th,
        })
    }

    /// Snapshot of every registered renderer, ordered by ascending id
    /// for UI stability.
    pub async fn snapshot_renderers(self: &Arc<Self>) -> Vec<RendererSnapshot> {
        let inner = self.inner.lock().await;
        let mut ids = inner.table.renderer_ids();
        ids.sort_unstable();
        ids.into_iter()
            .filter_map(|id| {
                let handle = inner.table.get_renderer(&id)?;
                let status = inner
                    .renderer_states
                    .get(&id)
                    .map(|status| RendererStatus::Paused(*status))
                    .unwrap_or(RendererStatus::Playing);
                let (tw, th) = handle.texture_size();
                Some(RendererSnapshot {
                    id: handle.id.clone(),
                    wp_type: handle.wp_type.clone(),
                    name: handle.name.clone(),
                    status,
                    pid: handle.pid.unwrap_or(0),
                    drm_render_major: handle.gpu.major,
                    drm_render_minor: handle.gpu.minor,
                    texture_width: tw,
                    texture_height: th,
                })
            })
            .collect()
    }

    /// Snapshot of every registered display plus the enabled links
    /// pointing at it, ordered by ascending id for UI stability.
    pub async fn snapshot_displays(self: &Arc<Self>) -> Vec<DisplaySnapshot> {
        let inner = self.inner.lock().await;
        let mut ids: Vec<DisplayId> = inner.displays.keys().copied().collect();
        ids.sort_unstable();
        ids.into_iter()
            .filter_map(|id| {
                let s = inner.displays.get(&id)?;
                let link_rows: Vec<Link> = inner
                    .table
                    .links_for_display(id)
                    .into_iter()
                    .filter(|l| l.enabled)
                    .collect();
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
                    })
                    .collect();
                Some(DisplaySnapshot {
                    id,
                    name: s.info.name.clone(),
                    instance_id: s.info.instance_id.clone(),
                    width: s.info.width,
                    height: s.info.height,
                    refresh_mhz: s.info.refresh_mhz,
                    links,
                    drm_render_major: s.gpu.major,
                    drm_render_minor: s.gpu.minor,
                    display_layout,
                    effective_layout,
                    effective_layout_source,
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

    // ---------------------------------------------------------------
    // Routing policy

    /// Return the renderers whose every enabled display link is
    /// covered by `target`, meaning an imminent relink fully replaces them.
    pub async fn renderers_fully_replaced_by(
        self: &Arc<Self>,
        target: Option<&[DisplayId]>,
    ) -> Vec<RendererId> {
        let inner = self.inner.lock().await;
        inner
            .table
            .renderer_ids()
            .into_iter()
            .filter(|rid| {
                let links = inner.table.links_for_renderer(rid);
                let enabled: Vec<_> = links.iter().filter(|l| l.enabled).collect();
                if enabled.is_empty() {
                    // Already orphaned (no enabled links). Counts as
                    // fully replaced so the caller can clean it up too.
                    return true;
                }
                match target {
                    None => true, // relink_all replaces every display
                    Some(ts) => enabled.iter().all(|l| ts.contains(&l.display_id)),
                }
            })
            .collect()
    }

    /// Synchronously unregister + kill each `id` in `ids`. Used by
    /// the apply path to drop fully replaced renderers.
    pub async fn stop_renderers(self: &Arc<Self>, ids: &[RendererId]) {
        for id in ids {
            self.unregister_renderer(id).await;
            if let Err(e) = self.mgr.kill(id).await {
                log::warn!("router: stop_renderers: kill {id}: {e}");
            }
        }
    }

    /// Stop the listed renderers with the wallpaper-switch shutdown
    /// handshake: track unbind acks, unregister, wait, then kill.
    pub async fn stop_renderers_orderly(
        self: &Arc<Self>,
        ids: &[RendererId],
        ack_timeout: Duration,
    ) {
        for id in ids {
            self.begin_unbind_ack_tracking(id).await;
        }
        for id in ids {
            self.unregister_renderer(id).await;
        }
        for id in ids {
            if self.await_unbind_acks_for(id, ack_timeout).await.is_err() {
                log::warn!(
                    "router: stop_renderers_orderly: unbind_done ack timeout \
                     for renderer {id}; proceeding with kill anyway"
                );
            }
        }
        for id in ids {
            if let Err(e) = self.mgr.kill(id).await {
                log::warn!("router: stop_renderers_orderly: kill {id}: {e}");
            }
        }
    }

    /// Re-point every enabled link to `new_renderer_id`. Used by
    /// `WallpaperApply` in single-wallpaper mode.
    pub async fn relink_displays_to(
        self: &Arc<Self>,
        display_ids: &[DisplayId],
        new_renderer_id: &str,
    ) {
        let applied: Vec<DisplayId> = {
            let mut inner = self.inner.lock().await;
            let mut out = Vec::with_capacity(display_ids.len());
            for did in display_ids {
                if !inner.displays.contains_key(did) {
                    continue;
                }
                let existing = inner.table.links_for_display(*did);
                for link in existing {
                    inner.table.remove_link(link.id);
                }
                inner.table.add_link(new_renderer_id.to_string(), *did);
                out.push(*did);
            }
            out
        };
        for did in &applied {
            self.sync_display(*did).await;
        }
        self.reconcile_lifecycle().await;
        // See `relink_all_displays_to` for the GC rationale. We always
        // run the mark pass so partially displaced renderers are handled.
        self.mark_orphans(Some(new_renderer_id)).await;
        self.reconcile_buffer_flags().await;
        if !applied.is_empty() {
            let all = self.snapshot_displays().await;
            self.emit(RouterEvent::DisplaysReplace(all));
        }
    }

    pub async fn relink_all_displays_to(self: &Arc<Self>, new_renderer_id: &str) {
        let display_ids: Vec<DisplayId> = {
            let mut inner = self.inner.lock().await;
            let ids: Vec<DisplayId> = inner.displays.keys().copied().collect();
            for did in &ids {
                let existing = inner.table.links_for_display(*did);
                for link in existing {
                    inner.table.remove_link(link.id);
                }
                inner.table.add_link(new_renderer_id.to_string(), *did);
            }
            ids
        };
        let had_ids = !display_ids.is_empty();
        for did in display_ids {
            self.sync_display(did).await;
        }
        self.reconcile_lifecycle().await;
        // Active GC: any renderer that is no longer referenced by any
        // display gets a reap timer; the new renderer is kept.
        self.mark_orphans(Some(new_renderer_id)).await;
        self.reconcile_buffer_flags().await;
        if had_ids {
            let all = self.snapshot_displays().await;
            self.emit(RouterEvent::DisplaysReplace(all));
        }
    }

    /// Mutate a link's geometry/clear color and re-emit `SetConfig` to
    /// the affected display, without Bind or Unbind.
    pub async fn set_link_geometry(
        self: &Arc<Self>,
        link_id: LinkId,
        src: Option<LinkSrcRect>,
        dst: Option<LinkDstRect>,
        transform: Option<u32>,
        clear_rgba: Option<[f32; 4]>,
        z_order: Option<i32>,
    ) -> bool {
        let payload: Option<(DisplayId, ProjectedConfig)> = {
            let mut inner = self.inner.lock().await;
            let changed = inner
                .table
                .update_link_geometry(link_id, src, dst, transform, clear_rgba, z_order);
            if !changed {
                return false;
            }
            let Some(link) = inner.table.get_link(link_id).cloned() else {
                return false;
            };
            let Some(renderer) = inner.table.get_renderer(&link.renderer_id) else {
                return false;
            };
            let (info, bound_to_this) = match inner.displays.get(&link.display_id) {
                Some(state) => (
                    state.info.clone(),
                    state.last_renderer.as_deref() == Some(link.renderer_id.as_str()),
                ),
                None => return false,
            };
            if !bound_to_this {
                return true;
            }
            inner.next_config_generation += 1;
            let cfg_gen = inner.next_config_generation;
            let layout = self.resolved_layout_for_renderer(&info, &link.renderer_id, &inner);
            let cfg = project_link(&link, &renderer, &info, cfg_gen, &layout);
            Some((link.display_id, cfg))
        };
        let affected_display = payload.as_ref().map(|(d, _)| *d);
        if let Some((did, cfg)) = payload {
            let inner = self.inner.lock().await;
            if let Some(state) = inner.displays.get(&did) {
                let _ = state.tx.send(DisplayOutEvent::SetConfig(cfg));
            }
        }
        if let Some(did) = affected_display {
            if let Some(snap) = self.snapshot_display(did).await {
                self.emit(RouterEvent::DisplayUpsert(snap));
            }
        }
        true
    }

    // ---------------------------------------------------------------
    // Internal — renderer event handlers and sync core

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
        for did in display_ids {
            self.sync_display(did).await;
        }
        // The first BindBuffers exposes the renderer's flags so the
        // router can compare them against consumer caps.
        self.reconcile_buffer_flags().await;
        // BindBuffers is also when the renderer's actual texture dims
        // become known; push a fresh renderer snapshot for the UI.
        if let Some(snap) = self.snapshot_renderer(renderer_id).await {
            self.emit(RouterEvent::RendererUpsert(snap));
        }
    }

    async fn on_renderer_frame(
        self: &Arc<Self>,
        renderer_id: &str,
        buffer_index: u32,
        seq: u64,
        release_point: u64,
    ) {
        let inner = self.inner.lock().await;
        let Some(renderer) = inner.table.get_renderer(renderer_id) else {
            return;
        };
        let gen = renderer
            .bind_snapshot()
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|s| s.generation));
        let Some(gen) = gen else { return };

        // First pass: collect every display that should get this frame
        // so we can pre-compute fan-out width for the reaper.
        let recipients: Vec<&DisplayState> = inner
            .table
            .links_for_renderer(renderer_id)
            .into_iter()
            .filter(|link| link.enabled)
            .filter_map(|link| inner.displays.get(&link.display_id))
            .filter(|state| {
                state.last_buffer_generation == Some(gen)
                    && state.last_renderer.as_deref() == Some(renderer_id)
            })
            .collect();
        let expected_count = recipients.len() as u32;
        if expected_count == 0 {
            // No enabled recipients: still hand the producer's release
            // timeline a synthetic signal at this point.
            if let Err(e) = renderer.submit_frame_record(crate::sync::FrameRecord {
                release_point,
                consumer_handle: None,
                expected_count: 0,
            }) {
                log::warn!(
                    "router: renderer {renderer_id}: failed to enqueue \
                     advance-only FrameRecord (point {release_point}): {e}"
                );
            }
            return;
        }
        for state in recipients {
            let _ = state.tx.send(DisplayOutEvent::Frame {
                renderer: renderer.clone(),
                buffer_generation: gen,
                buffer_index,
                seq,
                release_point,
                expected_count,
            });
        }
    }

    /// Compute the current Pause/Play diff and dispatch control
    /// messages outside the inner lock after lifecycle mutations.
    async fn reconcile_lifecycle(self: &Arc<Self>) {
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
                let manual_paused = inner.manual_paused;
                let manual_muted = inner.manual_muted;
                let should_pause = manual_paused || !has_active_link || auto_pause_requested;
                let should_mute = manual_muted || !has_active_link || auto_mute_decision.is_some();
                let previous_state = inner
                    .renderer_states
                    .get(&rid)
                    .map(|status| RendererStatus::Paused(*status))
                    .unwrap_or(RendererStatus::Playing);
                let target_state = if should_pause {
                    RendererStatus::Paused(PausedRendererStatus::Paused)
                } else if should_mute {
                    RendererStatus::Paused(PausedRendererStatus::Muted)
                } else {
                    RendererStatus::Playing
                };
                if previous_state == target_state {
                    continue;
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
                } else if has_active_link {
                    "auto-action"
                } else {
                    "ref-count"
                };
                match (previous_state, target_state) {
                    (
                        RendererStatus::Playing,
                        RendererStatus::Paused(PausedRendererStatus::Paused),
                    ) => {
                        inner
                            .renderer_states
                            .insert(rid.clone(), PausedRendererStatus::Paused);
                        out.push((
                            rid,
                            ControlMsg::Pause {
                                fade_ms: audio_fade_ms,
                            },
                            pause_cause,
                        ));
                    }
                    (
                        RendererStatus::Playing,
                        RendererStatus::Paused(PausedRendererStatus::Muted),
                    ) => {
                        inner
                            .renderer_states
                            .insert(rid.clone(), PausedRendererStatus::Muted);
                        out.push((
                            rid,
                            ControlMsg::Mute {
                                fade_ms: audio_fade_ms,
                            },
                            mute_cause,
                        ));
                    }
                    (
                        RendererStatus::Paused(PausedRendererStatus::Paused),
                        RendererStatus::Playing,
                    ) => {
                        inner.renderer_states.remove(&rid);
                        out.push((
                            rid,
                            ControlMsg::Play {
                                fade_ms: audio_fade_ms,
                            },
                            clear_cause,
                        ));
                    }
                    (
                        RendererStatus::Paused(PausedRendererStatus::Muted),
                        RendererStatus::Playing,
                    ) => {
                        inner.renderer_states.remove(&rid);
                        out.push((
                            rid,
                            ControlMsg::Unmute {
                                fade_ms: audio_fade_ms,
                            },
                            clear_cause,
                        ));
                    }
                    (
                        RendererStatus::Paused(PausedRendererStatus::Paused),
                        RendererStatus::Paused(PausedRendererStatus::Muted),
                    ) => {
                        inner
                            .renderer_states
                            .insert(rid.clone(), PausedRendererStatus::Muted);
                        out.push((rid.clone(), ControlMsg::Mute { fade_ms: 0 }, mute_cause));
                        out.push((rid, ControlMsg::Play { fade_ms: 0 }, "state-switch"));
                    }
                    (
                        RendererStatus::Paused(PausedRendererStatus::Muted),
                        RendererStatus::Paused(PausedRendererStatus::Paused),
                    ) => {
                        inner
                            .renderer_states
                            .insert(rid.clone(), PausedRendererStatus::Paused);
                        out.push((rid.clone(), ControlMsg::Pause { fade_ms: 0 }, pause_cause));
                        out.push((rid, ControlMsg::Unmute { fade_ms: 0 }, "state-switch"));
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
            let label = match msg {
                ControlMsg::Pause { .. } => "pause",
                ControlMsg::Play { .. } => "play",
                _ => "ctl",
            };
            if let Err(e) = self.mgr.send_control(&id, msg).await {
                log::warn!("router: {label} {id}: {e}");
            } else {
                log::info!("router: {label} renderer {id} ({cause})");
            }
        }
        for id in changed_ids {
            if let Some(snap) = self.snapshot_renderer(&id).await {
                self.emit(RouterEvent::RendererUpsert(snap));
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
            producer: crate::dma::negotiate::PeerCaps,
            consumer: crate::dma::negotiate::PeerCaps,
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
                    let Some(consumer_caps) = state.consumer_caps.clone() else {
                        continue; // legacy consumer — skip silently
                    };
                    out.push(Pair {
                        rid: rid.clone(),
                        did: link.display_id,
                        producer: producer_caps.clone(),
                        consumer: consumer_caps,
                    });
                }
            }
            out
        };
        // Dispatch the picked scheme via NegotiateBuffers; for fan-out,
        // the last compatible per-display pick currently wins.
        let mut by_renderer: std::collections::HashMap<
            RendererId,
            crate::dma::negotiate::NegotiatedScheme,
        > = std::collections::HashMap::new();
        for p in pairs {
            match crate::dma::negotiate::pick(&p.producer, &p.consumer) {
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

    /// Bring `display_id`'s sent state in line with its current link
    /// target (renderer + generation). Idempotent.
    async fn sync_display(self: &Arc<Self>, display_id: DisplayId) {
        let mut inner = self.inner.lock().await;
        if !inner.displays.contains_key(&display_id) {
            return;
        }
        // Compute target (link + renderer + generation) under immutable borrows.
        let display_links = inner.table.links_for_display(display_id);
        debug_assert!(
            display_links.iter().filter(|l| l.enabled).count() <= 1,
            "display {display_id} has multiple enabled links — invariant violated"
        );
        let target: Option<(Link, Arc<RendererHandle>, u64)> =
            display_links.into_iter().find(|l| l.enabled).and_then(|l| {
                let renderer = inner.table.get_renderer(&l.renderer_id)?;
                let gen = renderer
                    .bind_snapshot()
                    .lock()
                    .ok()
                    .and_then(|g| g.as_ref().map(|s| s.generation))?;
                Some((l, renderer, gen))
            });

        // When both producer and consumer have caps, only bind a snapshot
        // that satisfies the last negotiated scheme.
        if let Some((_, ref renderer, _)) = target {
            let state = inner.displays.get(&display_id).unwrap();
            let v2_both = renderer.format_caps().is_some() && state.consumer_caps.is_some();
            if v2_both && !renderer.scheme_satisfied() {
                log::debug!(
                    "router: sync_display({display_id}) gated — renderer {} \
                     bind_snapshot does not yet match last-dispatched scheme",
                    renderer.id
                );
                return;
            }
        }

        // Snapshot what was last sent.
        let (last_renderer, last_gen, info) = {
            let s = inner.displays.get(&display_id).unwrap();
            (
                s.last_renderer.clone(),
                s.last_buffer_generation,
                s.info.clone(),
            )
        };

        let needs_update = match (&last_renderer, last_gen, &target) {
            (Some(or), Some(og), Some((link, _, ng))) => or != &link.renderer_id || og != *ng,
            (None, None, None) => false,
            _ => true,
        };
        if !needs_update {
            return;
        }

        // Retire the prior pool if one was bound.
        if let Some(og) = last_gen {
            let s = inner.displays.get(&display_id).unwrap();
            let _ = s.tx.send(DisplayOutEvent::Unbind {
                buffer_generation: og,
            });
            // If the OLD renderer is currently being torn down with
            // ack tracking active, record this unbind as pending.
            if let Some(old_r) = last_renderer.as_ref() {
                if let Some(pending) = inner.unbind_acks_pending.get_mut(old_r) {
                    pending.insert((display_id, og));
                }
            }
        }

        // Bind the new pool if a target renderer is ready.
        if let Some((link, renderer, new_g)) = target {
            inner.next_config_generation += 1;
            let cfg_gen = inner.next_config_generation;
            let layout = self.resolved_layout_for_renderer(&info, &link.renderer_id, &inner);
            let cfg = project_link(&link, &renderer, &info, cfg_gen, &layout);
            let new_r = link.renderer_id.clone();
            let replay = renderer
                .wp_type
                .eq_ignore_ascii_case("image")
                .then(|| renderer.latest_frame())
                .flatten()
                .filter(|frame| frame.buffer_generation == new_g);
            let s = inner.displays.get_mut(&display_id).unwrap();
            let _ = s.tx.send(DisplayOutEvent::Bind {
                renderer: renderer.clone(),
            });
            let _ = s.tx.send(DisplayOutEvent::SetConfig(cfg));
            if let Some(frame) = replay {
                let _ = s.tx.send(DisplayOutEvent::Frame {
                    renderer: renderer.clone(),
                    buffer_generation: frame.buffer_generation,
                    buffer_index: frame.buffer_index,
                    seq: frame.seq,
                    release_point: frame.release_point,
                    expected_count: 1,
                });
            }
            s.last_renderer = Some(new_r);
            s.last_buffer_generation = Some(new_g);
        } else {
            let s = inner.displays.get_mut(&display_id).unwrap();
            s.last_renderer = None;
            s.last_buffer_generation = None;
        }
    }
}

/// Resolve a `Link`'s geometry into a wire-ready `ProjectedConfig`.
///
fn project_link(
    link: &Link,
    renderer: &Arc<RendererHandle>,
    info: &DisplayInfo,
    config_generation: u64,
    layout: &ResolvedLayout,
) -> ProjectedConfig {
    let src_full = link.src_rect == super::table::FULL_SRC;
    let dst_full = link.dst_rect == super::table::FULL_DST;

    if src_full && dst_full {
        let (tex_w, tex_h) = renderer.texture_size();
        // The consumer (waywallen-display) draws into pre-rotation
        // display space, then rotates the rect onto the actual display.
        let (eff_disp_w, eff_disp_h) = match layout.rotation {
            crate::display::layout::Rotation::Cw90 | crate::display::layout::Rotation::Cw270 => {
                (info.height as f32, info.width as f32)
            }
            _ => (info.width as f32, info.height as f32),
        };
        let out = crate::display::layout::compute(LayoutInput {
            tex_w: tex_w as f32,
            tex_h: tex_h as f32,
            disp_w: eff_disp_w,
            disp_h: eff_disp_h,
            fillmode: layout.fillmode,
            location: layout.location,
            clear_rgba: link.clear_rgba,
        });
        return ProjectedConfig {
            config_generation,
            source_x: out.source.0,
            source_y: out.source.1,
            source_w: out.source.2,
            source_h: out.source.3,
            dest_x: out.dest.0,
            dest_y: out.dest.1,
            dest_w: out.dest.2,
            dest_h: out.dest.3,
            transform: layout.rotation.to_wl_transform(),
            clear_rgba: out.clear_rgba,
        };
    }

    // Explicit per-link geometry: keep the legacy resolve-sentinels
    // path for tests and future manual routing APIs.
    let (rtex_w, rtex_h) = renderer.texture_size();
    let resolve_src = |r: LinkSrcRect| -> (f32, f32, f32, f32) {
        let w = if r.w.is_infinite() {
            rtex_w as f32
        } else {
            r.w
        };
        let h = if r.h.is_infinite() {
            rtex_h as f32
        } else {
            r.h
        };
        (r.x, r.y, w, h)
    };
    let resolve_dst = |r: LinkDstRect| -> (f32, f32, f32, f32) {
        let w = if r.w.is_infinite() {
            info.width as f32
        } else {
            r.w
        };
        let h = if r.h.is_infinite() {
            info.height as f32
        } else {
            r.h
        };
        (r.x, r.y, w, h)
    };
    let (sx, sy, sw, sh) = resolve_src(link.src_rect);
    let (dx, dy, dw, dh) = resolve_dst(link.dst_rect);
    ProjectedConfig {
        config_generation,
        source_x: sx,
        source_y: sy,
        source_w: sw,
        source_h: sh,
        dest_x: dx,
        dest_y: dy,
        dest_w: dw,
        dest_h: dh,
        transform: layout.rotation.to_wl_transform(),
        clear_rgba: link.clear_rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer_manager::RendererManager;

    fn reg(name: &str, w: u32, h: u32) -> DisplayRegistration {
        DisplayRegistration {
            name: name.into(),
            instance_id: None,
            width: w,
            height: h,
            refresh_mhz: 60_000,
            gpu: DrmNode::UNKNOWN,
            properties: vec![],
            consumer_caps: None,
        }
    }

    #[tokio::test]
    async fn snapshot_displays_empty() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr);
        assert!(router.snapshot_displays().await.is_empty());
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
        DisplayRegistration {
            name: name.into(),
            instance_id: Some(iid.into()),
            width: 1920,
            height: 1080,
            refresh_mhz: 60_000,
            gpu: DrmNode::UNKNOWN,
            properties: vec![],
            consumer_caps: None,
        }
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
        *r.bind_snapshot().lock().unwrap() = Some(fake_bind_snapshot(1, 1920, 1080));
        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r.clone()).await;

        let mut h1 = router
            .register_display(reg_iid("KDE Screen", "iid-1"))
            .await;
        let mut h2 = router
            .register_display(reg_iid("KDE Screen", "iid-2"))
            .await;
        let _ = last_set_config(&mut h1.rx);
        let _ = last_set_config(&mut h2.rx);

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
        assert!(last_set_config(&mut h1.rx).is_none());
        assert!(last_set_config(&mut h2.rx).is_some());

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

        // Third apply: same wallpaper as r2 → caller would `find_reusable`
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
                assert_eq!(
                    snap.status,
                    RendererStatus::Paused(PausedRendererStatus::Paused)
                );
                assert_eq!(snap.name, "test-stub");
            }
            other => panic!("expected RendererUpsert, got {other:?}"),
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
        let evt = recv_event(&mut rx).await.expect("no event");
        match evt {
            RouterEvent::RendererRemoved(id) => assert_eq!(id, "R1"),
            other => panic!("expected RendererRemoved, got {other:?}"),
        }
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
                if snap.id == "R1"
                    && snap.status == RendererStatus::Paused(PausedRendererStatus::Paused)
                {
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
    ) -> crate::dma::negotiate::PeerCaps {
        use crate::dma::negotiate as N;
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
    async fn consumer_bind_failed_inserts_blacklist() {
        // Wire a v2 consumer + producer, then push a BindFailed via
        // the router. The display blacklist must record the pair.
        use crate::dma::negotiate as N;
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        add_stub_renderer(&mgr, &router, "R1").await;
        let h = router.register_display(reg("D1", 1920, 1080)).await;

        let caps = build_caps(
            N::DRM_FORMAT_ABGR8888,
            &[(N::DRM_FORMAT_MOD_LINEAR, 1)],
            0xAA,
        );
        router.set_consumer_caps(h.id, caps).await;

        let nl: u64 = 0x0100_0000_0000_0001;
        router
            .on_consumer_bind_failed(h.id, N::DRM_FORMAT_ABGR8888, nl)
            .await;

        let inner = router.inner.lock().await;
        let state = inner.displays.get(&h.id).unwrap();
        let bl = &state.consumer_caps.as_ref().unwrap().blacklist;
        assert!(bl.contains(&(N::DRM_FORMAT_ABGR8888, nl)));
    }

    #[tokio::test]
    async fn renderer_bind_failed_inserts_blacklist() {
        // Same shape as the consumer test, but on the producer side.
        use crate::dma::negotiate as N;
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let h = RendererHandle::test_stub("R1", "scene");
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
        use crate::dma::negotiate as N;
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());

        let nl: u64 = 0x0100_0000_0000_0001;
        let h = RendererHandle::test_stub("R1", "scene");
        h.test_set_format_caps(build_caps(
            N::DRM_FORMAT_ABGR8888,
            &[(N::DRM_FORMAT_MOD_LINEAR, 1), (nl, 1)],
            0xAA,
        ));
        mgr.register_test_handle(h.clone()).await;
        router.register_renderer(h.clone()).await;

        let dh = router.register_display(reg("D1", 1920, 1080)).await;
        router
            .set_consumer_caps(
                dh.id,
                build_caps(
                    N::DRM_FORMAT_ABGR8888,
                    &[(N::DRM_FORMAT_MOD_LINEAR, 1), (nl, 1)],
                    0xAA,
                ),
            )
            .await;

        // Pre-blacklist pick must land on the non-LINEAR (same-device preference).
        {
            let inner = router.inner.lock().await;
            let prod = h.format_caps().expect("producer caps");
            let cons = inner.displays[&dh.id]
                .consumer_caps
                .clone()
                .expect("consumer caps");
            let s = N::pick(&prod, &cons).expect("pick ok");
            assert_eq!(s.modifier, nl, "pre-blacklist must prefer non-LINEAR");
        }

        // Consumer reports the non-LINEAR is unimportable.
        router
            .on_consumer_bind_failed(dh.id, N::DRM_FORMAT_ABGR8888, nl)
            .await;

        // Post-blacklist pick must fall back to LINEAR.
        let inner = router.inner.lock().await;
        let prod = h.format_caps().expect("producer caps");
        let cons = inner.displays[&dh.id]
            .consumer_caps
            .clone()
            .expect("consumer caps");
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
            width: w,
            height: h,
            refresh_mhz: 60_000,
            properties: vec![],
            bound: true,
        }
    }

    #[test]
    fn project_link_explicit_link_geometry_skips_layout() {
        // A link with explicit (non-sentinel) src/dst rects should
        // bypass display::layout::compute and pass rects through.
        let renderer = RendererHandle::test_stub("r1", "scene");
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
        let cfg = project_link(&link, &renderer, &info, 1, &layout);
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
    // update_display_size resync

    use crate::renderer_manager::{BindSnapshot, FrameSnapshot};

    fn fake_bind_snapshot(generation: u64, w: u32, h: u32) -> BindSnapshot {
        BindSnapshot {
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
    /// last `SetConfig` payload, matching what the consumer would use.
    fn last_set_config(
        rx: &mut mpsc::UnboundedReceiver<DisplayOutEvent>,
    ) -> Option<ProjectedConfig> {
        let mut out = None;
        while let Ok(ev) = rx.try_recv() {
            if let DisplayOutEvent::SetConfig(c) = ev {
                out = Some(c);
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

    fn drain_renderer_controls(peer: &std::os::unix::net::UnixStream) {
        peer.set_read_timeout(Some(Duration::from_millis(10)))
            .unwrap();
        loop {
            match crate::ipc::uds::recv_control(peer) {
                Ok(_) => {}
                Err(crate::ipc::uds::CodecError::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(crate::ipc::uds::CodecError::Nix(nix::errno::Errno::EAGAIN)) => break,
                Err(e) => panic!("drain renderer control failed: {e}"),
            }
        }
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    }

    #[tokio::test]
    async fn relink_to_reused_renderer_replays_latest_frame() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());

        let r1 = RendererHandle::test_stub("r1", "image");
        *r1.bind_snapshot().lock().unwrap() = Some(fake_bind_snapshot(1, 1920, 1080));
        mgr.register_test_handle(r1.clone()).await;
        router.register_renderer(r1.clone()).await;

        let r2 = RendererHandle::test_stub("r2", "image");
        *r2.bind_snapshot().lock().unwrap() = Some(fake_bind_snapshot(1, 1920, 1080));
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
        let mut saw_frame = false;
        for ev in events {
            if let DisplayOutEvent::Frame {
                renderer,
                buffer_generation,
                buffer_index,
                seq,
                release_point,
                expected_count,
            } = ev
            {
                assert_eq!(renderer.id, "r1");
                assert_eq!(buffer_generation, 1);
                assert_eq!(buffer_index, 0);
                assert_eq!(seq, 42);
                assert_eq!(release_point, 7);
                assert_eq!(expected_count, 1);
                saw_frame = true;
            }
        }
        assert!(saw_frame, "relinked display did not receive current frame");
    }

    #[tokio::test]
    async fn update_display_size_resyncs_set_config() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());

        // Renderer with a bind snapshot so resync_display_set_config can
        // read a generation and texture size.
        let r = RendererHandle::test_stub("r1", "scene"); // 1920x1080
        *r.bind_snapshot().lock().unwrap() = Some(fake_bind_snapshot(1, 1920, 1080));
        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r.clone()).await;

        // Register display 1920x1080 — auto-link + initial Bind/SetConfig.
        let mut h = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;
        let initial = last_set_config(&mut h.rx).expect("initial SetConfig");
        assert_eq!((initial.dest_w, initial.dest_h), (1920.0, 1080.0));

        // Resize to 1280x720 — Stretched + Center default → identity at new dims.
        router.update_display_size(h.id, 1280, 720).await;
        let resized = last_set_config(&mut h.rx).expect("SetConfig after resize");
        assert_eq!((resized.dest_x, resized.dest_y), (0.0, 0.0));
        assert_eq!((resized.dest_w, resized.dest_h), (1280.0, 720.0));
        assert!(resized.config_generation > initial.config_generation);
    }

    #[tokio::test]
    async fn update_display_size_same_dims_no_resync() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let r = RendererHandle::test_stub("r1", "scene");
        *r.bind_snapshot().lock().unwrap() = Some(fake_bind_snapshot(1, 1920, 1080));
        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r.clone()).await;

        let mut h = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;
        // Drain initial events.
        let _ = last_set_config(&mut h.rx);

        router.update_display_size(h.id, 1920, 1080).await;
        // No new SetConfig should land on the rx.
        assert!(last_set_config(&mut h.rx).is_none());
    }

    // -----------------------------------------------------------------
    // auto replay - daemon-side decision driven by display state

    use super::auto_replay as ar;
    use crate::settings::{AutoAction, AutoCondition, AutoReplayPolicy, SettingsStore};

    async fn settings_with_auto_replay(policy: AutoReplayPolicy) -> Arc<SettingsStore> {
        let tmp = tempfile::tempdir().unwrap();
        let store = SettingsStore::load_or_default(tmp.path().join("settings.toml")).await;
        store.update(|s| {
            s.global.auto_replay = Some(policy);
        });
        std::mem::forget(tmp);
        store
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
    async fn auto_replay_stop_unlinks_and_reaps_renderer() {
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

        assert!(router
            .snapshot_display(h.id)
            .await
            .unwrap()
            .links
            .is_empty());
        assert!(mgr.get("r1").await.is_none());
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
            }
        );
        assert!(router.toggle_manual_pause().await);
        assert!(router.toggle_manual_mute().await);
        assert_eq!(
            router.manual_lifecycle_state().await,
            ManualLifecycleState {
                paused: true,
                muted: true,
            }
        );
        assert!(!router.toggle_manual_pause().await);
        assert!(!router.toggle_manual_mute().await);
        assert_eq!(
            router.manual_lifecycle_state().await,
            ManualLifecycleState {
                paused: false,
                muted: false,
            }
        );
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
                let (msg, _fds) = crate::ipc::uds::recv_control(&peer).expect("recv control");
                match msg {
                    ControlMsg::Mute { fade_ms } => got.push(("mute", fade_ms)),
                    ControlMsg::Unmute { fade_ms } => got.push(("unmute", fade_ms)),
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
                let (msg, _fds) = crate::ipc::uds::recv_control(&peer).expect("recv control");
                match msg {
                    ControlMsg::Pause { fade_ms } => got.push(("pause", fade_ms)),
                    ControlMsg::Play { fade_ms } => got.push(("play", fade_ms)),
                    ControlMsg::Mute { fade_ms } => got.push(("mute", fade_ms)),
                    ControlMsg::Unmute { fade_ms } => got.push(("unmute", fade_ms)),
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
        tokio::task::yield_now().await;
        tokio::time::advance(AUTO_REPLAY_RESUME_DELAY + Duration::from_millis(50)).await;
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }

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
                let (msg, _fds) = crate::ipc::uds::recv_control(&peer).expect("recv control");
                match msg {
                    ControlMsg::Pause { fade_ms } => got.push(("pause", fade_ms)),
                    ControlMsg::Play { fade_ms } => got.push(("play", fade_ms)),
                    ControlMsg::Mute { fade_ms } => got.push(("mute", fade_ms)),
                    ControlMsg::Unmute { fade_ms } => got.push(("unmute", fade_ms)),
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
        tokio::task::yield_now().await;
        tokio::time::advance(AUTO_REPLAY_RESUME_DELAY + Duration::from_millis(50)).await;
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }

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
        *r1.bind_snapshot().lock().unwrap() = Some(fake_bind_snapshot(1, 1920, 1080));
        mgr.register_test_handle(r1.clone()).await;
        router.register_renderer(r1.clone()).await;
        let r2 = RendererHandle::test_stub("r2", "scene");
        *r2.bind_snapshot().lock().unwrap() = Some(fake_bind_snapshot(1, 1920, 1080));
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
    async fn auto_replay_resume_is_debounced() {
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

        // Pause.
        router
            .update_display_window_state(h.id, ar::FLAG_NON_MINIMIZED | ar::FLAG_FULLSCREEN)
            .await;
        assert!(router.is_paused("r1").await);

        // Flag drops -> state machine schedules a delayed resume. Still
        // paused immediately afterwards.
        router
            .update_display_window_state(h.id, ar::FLAG_NON_MINIMIZED)
            .await;
        assert!(router.is_paused("r1").await);

        tokio::task::yield_now().await;
        // Advance past the resume window. The spawned timer fires and
        // flips `requested`, then reconcile_lifecycle sends Play.
        tokio::time::advance(AUTO_REPLAY_RESUME_DELAY + std::time::Duration::from_millis(50)).await;
        let mut flipped = false;
        for _ in 0..50 {
            tokio::task::yield_now().await;
            if !router.is_paused("r1").await {
                flipped = true;
                break;
            }
        }
        assert!(
            flipped,
            "resume timer did not flip renderer back to playing"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn auto_replay_resume_cancelled_by_new_pause() {
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

        // Pause, then start a resume window, then immediately re-enter
        // fullscreen - the pending resume timer must be invalidated.
        router
            .update_display_window_state(h.id, ar::FLAG_NON_MINIMIZED | ar::FLAG_FULLSCREEN)
            .await;
        router
            .update_display_window_state(h.id, ar::FLAG_NON_MINIMIZED)
            .await;
        router
            .update_display_window_state(h.id, ar::FLAG_NON_MINIMIZED | ar::FLAG_FULLSCREEN)
            .await;

        tokio::time::advance(AUTO_REPLAY_RESUME_DELAY + std::time::Duration::from_millis(50)).await;
        // Give any in-flight tasks a chance to run; renderer must
        // remain paused because the middle resume timer was invalidated.
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert!(router.is_paused("r1").await);
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
    async fn update_display_size_zero_dim_ignored() {
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        let r = RendererHandle::test_stub("r1", "scene");
        *r.bind_snapshot().lock().unwrap() = Some(fake_bind_snapshot(1, 1920, 1080));
        mgr.register_test_handle(r.clone()).await;
        router.register_renderer(r.clone()).await;

        let mut h = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;
        let _ = last_set_config(&mut h.rx);

        // Zero dim → drop on the floor; field stays at 1920x1080.
        router.update_display_size(h.id, 0, 720).await;
        router.update_display_size(h.id, 1280, 0).await;
        assert!(last_set_config(&mut h.rx).is_none());
        let snap = router.snapshot_display(h.id).await.unwrap();
        assert_eq!((snap.width, snap.height), (1920, 1080));
    }
}
