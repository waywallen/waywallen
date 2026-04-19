//! Router — owns a `RoutingTable` plus a per-renderer subscription
//! task. Translates renderer broadcasts and table mutations into
//! per-display `DisplayOutEvent` streams that `display_endpoint`
//! consumes via plain mpsc.
//!
//! Phase 1 policy:
//!   * One enabled link per display (single-wallpaper mode).
//!   * `register_display` auto-creates a link to whichever renderer is
//!     currently "first" in the table.
//!   * `relink_all_displays_to(id)` re-points every display at the
//!     same renderer (used by `WallpaperApply`).
//!
//! Each display has a `last_renderer` / `last_buffer_generation`
//! sentinel; `sync_display` is the single point where the router
//! decides whether to push `Unbind`/`Bind`/`SetConfig`. The sentinels
//! make `sync_display` idempotent — it can be called multiple times
//! safely after one mutation.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, broadcast::error::RecvError, mpsc, Mutex as TokioMutex};
use tokio::task::JoinHandle;

/// Backstop only. The mainline path is `Router::reap_orphans`, which
/// is called from every `relink_*` mutation and synchronously kills
/// any renderer that just lost its last link. This timeout exists
/// purely to catch renderers that somehow ended up paused without
/// going through the apply path (defensive — should never fire in
/// practice).
const IDLE_KILL_TIMEOUT: Duration = Duration::from_secs(3600);
/// How often the backstop reaper task wakes up to scan for stragglers.
const IDLE_SCAN_INTERVAL: Duration = Duration::from_secs(60);

use crate::ipc::proto::{ControlMsg, EventMsg};
use crate::renderer_manager::{RendererHandle, RendererId, RendererManager};
use crate::scheduler::{DisplayId, DisplayInfo, ProjectedConfig};

use super::table::{Link, LinkDstRect, LinkId, LinkSrcRect, RoutingTable};

/// Wire-translated event streamed from router to a display endpoint.
/// The endpoint owns translation to the on-the-wire `Event`.
pub enum DisplayOutEvent {
    /// Bind the buffer pool currently published by `renderer`. The
    /// endpoint reads `renderer.bind_snapshot()` itself so the router
    /// doesn't have to clone fds for every subscriber.
    Bind {
        renderer: Arc<RendererHandle>,
    },
    /// Retire the named buffer pool generation.
    Unbind { buffer_generation: u64 },
    /// Update composition geometry / clear color.
    SetConfig(ProjectedConfig),
    /// A frame is ready on `renderer` at `buffer_index` for the named
    /// generation. The endpoint pulls the matching sync_fd from
    /// `renderer.clone_sync_fd(seq)` itself.
    Frame {
        renderer: Arc<RendererHandle>,
        buffer_generation: u64,
        buffer_index: u32,
        seq: u64,
    },
}

/// Initial-registration payload from `display_endpoint::do_handshake`.
pub struct DisplayRegistration {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
    pub properties: Vec<(String, String)>,
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
/// translates these into `pb::Event`s on the wire; tests can also
/// subscribe and observe router state changes without going through the
/// protobuf layer.
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
}

/// Lifecycle state of a renderer as seen by the router.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererStatus {
    Playing,
    Paused,
}

impl RendererStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Playing => "playing",
            Self::Paused => "paused",
        }
    }
}

/// Read-only view of a registered renderer. Returned from
/// `Router::snapshot_renderers`; mirrors the fields surfaced on the
/// control-plane `RendererInstance` message.
#[derive(Debug, Clone)]
pub struct RendererSnapshot {
    pub id: RendererId,
    pub wp_type: String,
    pub name: String,
    pub fps: u32,
    pub status: RendererStatus,
    pub pid: u32,
}

/// Read-only view of a registered display. Returned from
/// `Router::snapshot_displays`; carries metadata from `DisplayInfo`
/// plus the enabled links currently pointing at this display.
#[derive(Debug, Clone)]
pub struct DisplaySnapshot {
    pub id: DisplayId,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
    pub links: Vec<DisplayLinkSnapshot>,
}

struct DisplayState {
    info: DisplayInfo,
    tx: mpsc::UnboundedSender<DisplayOutEvent>,
    /// Last renderer this display was bound to (None if currently unbound).
    last_renderer: Option<RendererId>,
    /// Last `buffer_generation` we sent in a `Bind` to this display.
    /// Tracked so a follow-up `Unbind` retires the right gen.
    last_buffer_generation: Option<u64>,
}

struct Inner {
    table: RoutingTable,
    displays: HashMap<DisplayId, DisplayState>,
    renderer_tasks: HashMap<RendererId, JoinHandle<()>>,
    /// Renderers we've already sent `Pause` to. Used to compute the
    /// Play/Pause diff when ref_counts change so we never send the
    /// same control twice.
    paused_renderers: std::collections::HashSet<RendererId>,
    /// Timestamp of the Pause transition for each paused renderer.
    /// Consumed by the reaper task to enforce `IDLE_KILL_TIMEOUT`.
    paused_since: HashMap<RendererId, Instant>,
    next_display_id: u64,
    next_config_generation: u64,
}

pub struct Router {
    inner: TokioMutex<Inner>,
    /// For Pause/Play lifecycle control. Phase 2: a renderer with zero
    /// enabled links is paused; the next link added resumes it.
    mgr: Arc<RendererManager>,
    /// Fan-out channel for `RouterEvent`s. Always present; `send` errors
    /// when there are no subscribers are logged at debug and ignored.
    events_tx: broadcast::Sender<RouterEvent>,
}

impl Router {
    pub fn new(mgr: Arc<RendererManager>) -> Arc<Self> {
        let (events_tx, _) = broadcast::channel(128);
        let router = Arc::new(Self {
            inner: TokioMutex::new(Inner {
                table: RoutingTable::new(),
                displays: HashMap::new(),
                renderer_tasks: HashMap::new(),
                paused_renderers: std::collections::HashSet::new(),
                paused_since: HashMap::new(),
                next_display_id: 0,
                next_config_generation: 0,
            }),
            mgr,
            events_tx,
        });
        // Spawn the idle-renderer reaper.
        {
            let weak = Arc::downgrade(&router);
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(IDLE_SCAN_INTERVAL).await;
                    let Some(this) = weak.upgrade() else { return };
                    this.reap_idle_renderers().await;
                }
            });
        }
        router
    }

    /// Kill renderers that have been paused longer than
    /// `IDLE_KILL_TIMEOUT`. Called periodically by the reaper task
    /// spawned in `new()`.
    async fn reap_idle_renderers(self: &Arc<Self>) {
        let now = Instant::now();
        let victims: Vec<RendererId> = {
            let inner = self.inner.lock().await;
            inner
                .paused_since
                .iter()
                .filter_map(|(id, t)| {
                    if now.duration_since(*t) >= IDLE_KILL_TIMEOUT {
                        Some(id.clone())
                    } else {
                        None
                    }
                })
                .collect()
        };
        for id in victims {
            log::info!("router: reaping idle renderer {id}");
            self.unregister_renderer(&id).await;
            if let Err(e) = self.mgr.kill(&id).await {
                log::warn!("router: reaper kill {id}: {e}");
            }
        }
    }

    // ---------------------------------------------------------------
    // Renderer lifecycle
    // ---------------------------------------------------------------

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
                            image_index, seq, ..
                        }) => {
                            router.on_renderer_frame(&rid, image_index, seq).await;
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
        let snap_id = id.clone();
        {
            let mut inner = self.inner.lock().await;
            inner.table.add_renderer(handle);
            inner.renderer_tasks.insert(id, task);
        }
        if let Some(snap) = self.snapshot_renderer(&snap_id).await {
            self.emit(RouterEvent::RendererUpsert(snap));
        }
    }

    pub async fn unregister_renderer(self: &Arc<Self>, id: &str) {
        let affected: Vec<DisplayId> = {
            let mut inner = self.inner.lock().await;
            let removed = inner.table.remove_renderer(id);
            if let Some(task) = inner.renderer_tasks.remove(id) {
                task.abort();
            }
            inner.paused_renderers.remove(id);
            inner.paused_since.remove(id);
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

    // ---------------------------------------------------------------
    // Display lifecycle
    // ---------------------------------------------------------------

    pub async fn register_display(
        self: &Arc<Self>,
        reg: DisplayRegistration,
    ) -> DisplayHandle {
        let (tx, rx) = mpsc::unbounded_channel();
        let display_id = {
            let mut inner = self.inner.lock().await;
            inner.next_display_id += 1;
            let id = inner.next_display_id;
            let info = DisplayInfo {
                id,
                name: reg.name,
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
                    tx,
                    last_renderer: None,
                    last_buffer_generation: None,
                },
            );
            // Phase 1 policy: auto-link to whichever renderer is "first".
            if let Some(rid) = inner.table.first_renderer() {
                inner.table.add_link(rid, id);
            }
            id
        };
        self.sync_display(display_id).await;
        self.reconcile_lifecycle().await;
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
        self.reconcile_lifecycle().await;
        self.emit(RouterEvent::DisplayRemoved(display_id));
    }

    pub async fn update_display_size(
        self: &Arc<Self>,
        display_id: DisplayId,
        width: u32,
        height: u32,
    ) {
        let existed = {
            let mut inner = self.inner.lock().await;
            if let Some(s) = inner.displays.get_mut(&display_id) {
                s.info.width = width;
                s.info.height = height;
                true
            } else {
                false
            }
            // Phase 3 will re-emit SetConfig on resize. For Phase 1 we
            // mirror the legacy behavior: size update without re-config.
        };
        if existed {
            if let Some(snap) = self.snapshot_display(display_id).await {
                self.emit(RouterEvent::DisplayUpsert(snap));
            }
        }
    }

    /// Whether this renderer is currently in the paused set (zero
    /// enabled links). Returns `false` for unknown ids.
    pub async fn is_paused(self: &Arc<Self>, renderer_id: &str) -> bool {
        self.inner.lock().await.paused_renderers.contains(renderer_id)
    }

    /// Subscribe to router events (display add/change/remove). The
    /// returned receiver is lagged-on-overflow — callers should expect
    /// `RecvError::Lagged` and resync via `snapshot_displays` when it
    /// happens.
    pub fn subscribe_events(self: &Arc<Self>) -> broadcast::Receiver<RouterEvent> {
        self.events_tx.subscribe()
    }

    /// Walk every renderer in the table and kill the ones that no
    /// longer have any enabled link, **except** any id in `keep`. Used
    /// by the apply path to reclaim renderers that were just unlinked
    /// — and to preserve the just-applied renderer in the 0-display
    /// case where it has no links yet but should still hang around for
    /// the next display hotplug.
    ///
    /// Returns the list of ids that were actually killed (useful for
    /// log lines and tests).
    pub async fn reap_orphans(self: &Arc<Self>, keep: Option<&str>) -> Vec<RendererId> {
        let victims: Vec<RendererId> = {
            let inner = self.inner.lock().await;
            inner
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
                .collect()
        };
        for rid in &victims {
            log::info!("router: reaping orphan renderer {rid}");
            self.unregister_renderer(rid).await;
            if let Err(e) = self.mgr.kill(rid).await {
                log::warn!("router: kill orphan {rid}: {e}");
            }
        }
        victims
    }

    /// Fire an event to all subscribers. Send errors (no subscribers)
    /// are downgraded to debug logs.
    fn emit(&self, evt: RouterEvent) {
        if let Err(e) = self.events_tx.send(evt) {
            log::debug!("router: no event subscribers ({e})");
        }
    }

    /// Snapshot of a single display by id. Returns `None` if the
    /// display has been unregistered. Must not be called while the
    /// inner lock is held.
    pub async fn snapshot_display(self: &Arc<Self>, id: DisplayId) -> Option<DisplaySnapshot> {
        let inner = self.inner.lock().await;
        let s = inner.displays.get(&id)?;
        let links = inner
            .table
            .links_for_display(id)
            .into_iter()
            .filter(|l| l.enabled)
            .map(|l| DisplayLinkSnapshot {
                renderer_id: l.renderer_id,
                z_order: l.z_order,
            })
            .collect();
        Some(DisplaySnapshot {
            id,
            name: s.info.name.clone(),
            width: s.info.width,
            height: s.info.height,
            refresh_mhz: s.info.refresh_mhz,
            links,
        })
    }

    /// Snapshot of a single renderer by id. Returns `None` if the
    /// renderer has been unregistered from the routing table.
    pub async fn snapshot_renderer(self: &Arc<Self>, id: &str) -> Option<RendererSnapshot> {
        let inner = self.inner.lock().await;
        let handle = inner.table.get_renderer(id)?;
        let status = if inner.paused_renderers.contains(id) {
            RendererStatus::Paused
        } else {
            RendererStatus::Playing
        };
        Some(RendererSnapshot {
            id: handle.id.clone(),
            wp_type: handle.wp_type.clone(),
            name: handle.name.clone(),
            fps: handle.fps,
            status,
            pid: handle.pid.unwrap_or(0),
        })
    }

    /// Snapshot of every registered renderer, ordered by ascending id
    /// for UI stability. Pure read — does not touch renderer state or
    /// emit events.
    pub async fn snapshot_renderers(self: &Arc<Self>) -> Vec<RendererSnapshot> {
        let inner = self.inner.lock().await;
        let mut ids = inner.table.renderer_ids();
        ids.sort_unstable();
        ids.into_iter()
            .filter_map(|id| {
                let handle = inner.table.get_renderer(&id)?;
                let status = if inner.paused_renderers.contains(&id) {
                    RendererStatus::Paused
                } else {
                    RendererStatus::Playing
                };
                Some(RendererSnapshot {
                    id: handle.id.clone(),
                    wp_type: handle.wp_type.clone(),
                    name: handle.name.clone(),
                    fps: handle.fps,
                    status,
                    pid: handle.pid.unwrap_or(0),
                })
            })
            .collect()
    }

    /// Snapshot of every registered display plus the enabled links
    /// pointing at it, ordered by ascending id for UI stability.
    /// Pure read — does not touch renderer state or emit events.
    pub async fn snapshot_displays(self: &Arc<Self>) -> Vec<DisplaySnapshot> {
        let inner = self.inner.lock().await;
        let mut ids: Vec<DisplayId> = inner.displays.keys().copied().collect();
        ids.sort_unstable();
        ids.into_iter()
            .filter_map(|id| {
                let s = inner.displays.get(&id)?;
                let links = inner
                    .table
                    .links_for_display(id)
                    .into_iter()
                    .filter(|l| l.enabled)
                    .map(|l| DisplayLinkSnapshot {
                        renderer_id: l.renderer_id,
                        z_order: l.z_order,
                    })
                    .collect();
                Some(DisplaySnapshot {
                    id,
                    name: s.info.name.clone(),
                    width: s.info.width,
                    height: s.info.height,
                    refresh_mhz: s.info.refresh_mhz,
                    links,
                })
            })
            .collect()
    }

    // ---------------------------------------------------------------
    // Routing policy
    // ---------------------------------------------------------------

    /// Re-point every enabled link to `new_renderer_id`. Used by
    /// `WallpaperApply` in single-wallpaper mode. Idempotent: calling
    /// twice with the same id is a no-op (the link already points
    /// there, sync_display sees no diff).
    /// Re-point the single enabled link of every display in
    /// `display_ids` at `new_renderer_id`. Displays not in the list
    /// keep their current renderer binding. Unknown display ids are
    /// skipped silently (callers are expected to validate upstream).
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
        // run the reap pass so that switching one display away from a
        // renderer that no other display still uses immediately frees
        // its GPU resources.
        self.reap_orphans(Some(new_renderer_id)).await;
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
        // display dies right now. The new renderer is preserved by id
        // even if no displays were affected (0-display apply path).
        self.reap_orphans(Some(new_renderer_id)).await;
        if had_ids {
            let all = self.snapshot_displays().await;
            self.emit(RouterEvent::DisplaysReplace(all));
        }
    }

    /// Mutate a link's geometry/clear color and re-emit `SetConfig` to
    /// the affected display. Sends only `SetConfig` (no Bind/Unbind):
    /// the buffer pool is unchanged, only the composition geometry.
    /// Returns `true` if the link existed and any field was updated.
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
            let changed = inner.table.update_link_geometry(
                link_id, src, dst, transform, clear_rgba, z_order,
            );
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
            let cfg = project_link(&link, &renderer, &info, cfg_gen);
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
    // ---------------------------------------------------------------

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
    }

    async fn on_renderer_frame(
        self: &Arc<Self>,
        renderer_id: &str,
        buffer_index: u32,
        seq: u64,
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
        for link in inner.table.links_for_renderer(renderer_id) {
            if !link.enabled {
                continue;
            }
            let Some(state) = inner.displays.get(&link.display_id) else {
                continue;
            };
            // Only forward if the display is currently bound to this gen.
            if state.last_buffer_generation != Some(gen)
                || state.last_renderer.as_deref() != Some(renderer_id)
            {
                continue;
            }
            let _ = state.tx.send(DisplayOutEvent::Frame {
                renderer: renderer.clone(),
                buffer_generation: gen,
                buffer_index,
                seq,
            });
        }
    }

    /// Compute the current Pause/Play diff and dispatch control
    /// messages outside the inner lock. Call after any mutation that
    /// can change a renderer's enabled-link count.
    async fn reconcile_lifecycle(self: &Arc<Self>) {
        let actions: Vec<(RendererId, ControlMsg)> = {
            let mut inner = self.inner.lock().await;
            let mut out = Vec::new();
            for rid in inner.table.renderer_ids() {
                let active = inner
                    .table
                    .links_for_renderer(&rid)
                    .iter()
                    .any(|l| l.enabled);
                let was_paused = inner.paused_renderers.contains(&rid);
                if active && was_paused {
                    inner.paused_renderers.remove(&rid);
                    inner.paused_since.remove(&rid);
                    out.push((rid, ControlMsg::Play));
                } else if !active && !was_paused {
                    inner.paused_renderers.insert(rid.clone());
                    inner.paused_since.insert(rid.clone(), Instant::now());
                    out.push((rid, ControlMsg::Pause));
                }
            }
            out
        };
        let changed_ids: Vec<RendererId> = actions.iter().map(|(id, _)| id.clone()).collect();
        for (id, msg) in actions {
            let label = match msg {
                ControlMsg::Pause => "pause",
                ControlMsg::Play => "play",
                _ => "ctl",
            };
            if let Err(e) = self.mgr.send_control(&id, msg).await {
                log::warn!("router: {label} {id}: {e}");
            } else {
                log::info!("router: {label} renderer {id} (ref_count diff)");
            }
        }
        for id in changed_ids {
            if let Some(snap) = self.snapshot_renderer(&id).await {
                self.emit(RouterEvent::RendererUpsert(snap));
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
        let target: Option<(Link, Arc<RendererHandle>, u64)> = display_links
            .into_iter()
            .find(|l| l.enabled)
            .and_then(|l| {
                let renderer = inner.table.get_renderer(&l.renderer_id)?;
                let gen = renderer
                    .bind_snapshot()
                    .lock()
                    .ok()
                    .and_then(|g| g.as_ref().map(|s| s.generation))?;
                Some((l, renderer, gen))
            });

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

        // Phase A: retire the prior pool (if any).
        if let Some(og) = last_gen {
            let s = inner.displays.get(&display_id).unwrap();
            let _ = s.tx.send(DisplayOutEvent::Unbind {
                buffer_generation: og,
            });
        }

        // Phase B: bind the new pool (if any).
        if let Some((link, renderer, new_g)) = target {
            inner.next_config_generation += 1;
            let cfg_gen = inner.next_config_generation;
            let cfg = project_link(&link, &renderer, &info, cfg_gen);
            let new_r = link.renderer_id.clone();
            let s = inner.displays.get_mut(&display_id).unwrap();
            let _ = s.tx.send(DisplayOutEvent::Bind {
                renderer: renderer.clone(),
            });
            let _ = s.tx.send(DisplayOutEvent::SetConfig(cfg));
            s.last_renderer = Some(new_r);
            s.last_buffer_generation = Some(new_g);
        } else {
            let s = inner.displays.get_mut(&display_id).unwrap();
            s.last_renderer = None;
            s.last_buffer_generation = None;
        }
    }
}

/// Resolve a `Link`'s geometry into a wire-ready `ProjectedConfig`,
/// substituting the renderer's full texture / display's full surface
/// for the `FULL_SRC` / `FULL_DST` sentinels.
fn project_link(
    link: &Link,
    renderer: &Arc<RendererHandle>,
    info: &DisplayInfo,
    config_generation: u64,
) -> ProjectedConfig {
    let resolve_src = |r: LinkSrcRect| -> (f32, f32, f32, f32) {
        let w = if r.w.is_infinite() { renderer.width as f32 } else { r.w };
        let h = if r.h.is_infinite() { renderer.height as f32 } else { r.h };
        (r.x, r.y, w, h)
    };
    let resolve_dst = |r: LinkDstRect| -> (f32, f32, f32, f32) {
        let w = if r.w.is_infinite() { info.width as f32 } else { r.w };
        let h = if r.h.is_infinite() { info.height as f32 } else { r.h };
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
        transform: link.transform,
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
            width: w,
            height: h,
            refresh_mhz: 60_000,
            properties: vec![],
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
            assert!(d.links.is_empty(), "display {} unexpectedly has links", d.id);
        }
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
    // M8 — orphan reaping
    // -----------------------------------------------------------------

    /// Register a stub renderer with both the manager and the router
    /// so apply-side lookups (`mgr.kill`, `table.get_renderer`) both
    /// succeed.
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

    #[tokio::test]
    async fn reap_kills_orphan_after_relink_all() {
        // single display starts on r1; relink_all → r2 should reap r1.
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        add_stub_renderer(&mgr, &router, "r1").await;
        add_stub_renderer(&mgr, &router, "r2").await;

        let _h = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;
        // r1 was registered first → first_renderer() picked it for the auto-link.
        router.relink_all_displays_to("r2").await;

        let live = live_renderers(&mgr).await;
        assert_eq!(live, vec!["r2".to_string()], "r1 should have been reaped");
    }

    #[tokio::test]
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
        let live = live_renderers(&mgr).await;
        assert_eq!(live, vec!["r1".to_string(), "r2".to_string()]);

        // Now move display B over too — r1 finally orphaned, gets reaped.
        router.relink_all_displays_to("r2").await;
        let live = live_renderers(&mgr).await;
        assert_eq!(live, vec!["r2".to_string()]);
    }

    #[tokio::test]
    async fn relink_all_with_zero_displays_replaces_old_renderer() {
        // Apply path semantics with no displays attached:
        //   1. apply wp1 → r1 spawned and preserved (no displays to link).
        //   2. apply wp2 → r2 spawned; r1 must be killed even though
        //      relink_all touches no displays.
        // This is what the daemon's WallpaperApply does: register the
        // new renderer, then `relink_all_displays_to(new_id)`. We verify
        // here that the second leg reaps r1 thanks to
        // `reap_orphans(Some(new_id))` running unconditionally.
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());

        // First apply: r1 spawn + relink_all (no displays).
        add_stub_renderer(&mgr, &router, "r1").await;
        router.relink_all_displays_to("r1").await;
        assert_eq!(live_renderers(&mgr).await, vec!["r1".to_string()]);

        // Second apply: r2 spawn + relink_all (still no displays).
        add_stub_renderer(&mgr, &router, "r2").await;
        router.relink_all_displays_to("r2").await;
        assert_eq!(
            live_renderers(&mgr).await,
            vec!["r2".to_string()],
            "r1 must be reaped when r2 takes over with no displays"
        );

        // Third apply: same wallpaper as r2 → caller would `find_reusable`
        // and reuse r2; relink_all("r2") is a no-op + reap_orphans
        // protects r2.
        router.relink_all_displays_to("r2").await;
        assert_eq!(live_renderers(&mgr).await, vec!["r2".to_string()]);
    }

    #[tokio::test]
    async fn unregister_last_display_keeps_renderer_alive() {
        // After all displays unplug, the lone renderer must NOT be
        // reaped — otherwise hotplug would leave the user with nothing
        // to auto-link to. Only an explicit apply should replace it.
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        add_stub_renderer(&mgr, &router, "r1").await;
        let h = router.register_display(reg("HDMI-A-1", 1920, 1080)).await;
        assert_eq!(live_renderers(&mgr).await, vec!["r1".to_string()]);

        router.unregister_display(h.id).await;
        assert_eq!(
            live_renderers(&mgr).await,
            vec!["r1".to_string()],
            "renderer must survive last display unregister"
        );

        // Plug a fresh display in: it auto-links to r1 (first_renderer).
        let h2 = router.register_display(reg("DP-1", 1920, 1080)).await;
        let snap = router.snapshot_displays().await;
        let entry = snap.iter().find(|d| d.id == h2.id).unwrap();
        assert_eq!(entry.links.len(), 1);
        assert_eq!(entry.links[0].renderer_id, "r1");
    }

    #[tokio::test]
    async fn reap_preserves_keep_id_with_no_displays() {
        // 0-display: spawn r1 → it has no link, but `keep=Some("r1")`
        // protects it. Then spawn r2 and reap_orphans(Some("r2")) must
        // kill r1 (no longer protected) and keep r2.
        let mgr = Arc::new(RendererManager::new_default());
        let router = Router::new(mgr.clone());
        add_stub_renderer(&mgr, &router, "r1").await;
        let killed = router.reap_orphans(Some("r1")).await;
        assert!(killed.is_empty());
        assert_eq!(live_renderers(&mgr).await, vec!["r1".to_string()]);

        add_stub_renderer(&mgr, &router, "r2").await;
        let killed = router.reap_orphans(Some("r2")).await;
        assert_eq!(killed, vec!["r1".to_string()]);
        assert_eq!(live_renderers(&mgr).await, vec!["r2".to_string()]);
    }

    // -----------------------------------------------------------------
    // Active-sync RouterEvent::Renderer* emission
    // -----------------------------------------------------------------

    async fn recv_event(
        rx: &mut broadcast::Receiver<RouterEvent>,
    ) -> Option<RouterEvent> {
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
                assert_eq!(snap.status, RendererStatus::Playing);
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
            let Some(evt) = recv_event(&mut rx).await else { break };
            if let RouterEvent::RendererUpsert(snap) = evt {
                if snap.id == "R1" && snap.status == RendererStatus::Paused {
                    saw_paused = true;
                    break;
                }
            }
        }
        assert!(saw_paused, "expected R1 Paused upsert after display unregister");
    }
}
