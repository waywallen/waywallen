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

use tokio::sync::{broadcast::error::RecvError, mpsc, Mutex as TokioMutex};
use tokio::task::JoinHandle;

use crate::ipc::proto::EventMsg;
use crate::renderer_manager::{RendererHandle, RendererId};
use crate::scheduler::{DisplayId, DisplayInfo, ProjectedConfig};

use super::table::RoutingTable;

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
    next_display_id: u64,
    next_config_generation: u64,
}

pub struct Router {
    inner: TokioMutex<Inner>,
}

impl Router {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: TokioMutex::new(Inner {
                table: RoutingTable::new(),
                displays: HashMap::new(),
                renderer_tasks: HashMap::new(),
                next_display_id: 0,
                next_config_generation: 0,
            }),
        })
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
        let mut inner = self.inner.lock().await;
        inner.table.add_renderer(handle);
        inner.renderer_tasks.insert(id, task);
    }

    pub async fn unregister_renderer(self: &Arc<Self>, id: &str) {
        let affected: Vec<DisplayId> = {
            let mut inner = self.inner.lock().await;
            let removed = inner.table.remove_renderer(id);
            if let Some(task) = inner.renderer_tasks.remove(id) {
                task.abort();
            }
            removed.into_iter().map(|(_, did)| did).collect()
        };
        for did in affected {
            self.sync_display(did).await;
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
        DisplayHandle { id: display_id, rx }
    }

    pub async fn unregister_display(self: &Arc<Self>, display_id: DisplayId) {
        let mut inner = self.inner.lock().await;
        inner.displays.remove(&display_id);
        inner.table.remove_display(display_id);
    }

    pub async fn update_display_size(
        self: &Arc<Self>,
        display_id: DisplayId,
        width: u32,
        height: u32,
    ) {
        let mut inner = self.inner.lock().await;
        if let Some(s) = inner.displays.get_mut(&display_id) {
            s.info.width = width;
            s.info.height = height;
        }
        // Phase 3 will re-emit SetConfig on resize. For Phase 1 we
        // mirror the legacy behavior: size update without re-config.
    }

    // ---------------------------------------------------------------
    // Routing policy
    // ---------------------------------------------------------------

    /// Re-point every enabled link to `new_renderer_id`. Used by
    /// `WallpaperApply` in single-wallpaper mode. Idempotent: calling
    /// twice with the same id is a no-op (the link already points
    /// there, sync_display sees no diff).
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
        for did in display_ids {
            self.sync_display(did).await;
        }
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

    /// Bring `display_id`'s sent state in line with its current link
    /// target (renderer + generation). Idempotent.
    async fn sync_display(self: &Arc<Self>, display_id: DisplayId) {
        let mut inner = self.inner.lock().await;
        if !inner.displays.contains_key(&display_id) {
            return;
        }
        // Compute target (renderer + generation) under immutable borrows.
        let target: Option<(RendererId, Arc<RendererHandle>, u64)> = inner
            .table
            .links_for_display(display_id)
            .into_iter()
            .find(|l| l.enabled)
            .and_then(|l| {
                let renderer = inner.table.get_renderer(&l.renderer_id)?;
                let gen = renderer
                    .bind_snapshot()
                    .lock()
                    .ok()
                    .and_then(|g| g.as_ref().map(|s| s.generation))?;
                Some((l.renderer_id.clone(), renderer, gen))
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
            (Some(or), Some(og), Some((nr, _, ng))) => or != nr || og != *ng,
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
        if let Some((new_r, renderer, new_g)) = target {
            inner.next_config_generation += 1;
            let cfg_gen = inner.next_config_generation;
            let cfg = ProjectedConfig {
                config_generation: cfg_gen,
                source_x: 0.0,
                source_y: 0.0,
                source_w: renderer.width as f32,
                source_h: renderer.height as f32,
                dest_x: 0.0,
                dest_y: 0.0,
                dest_w: info.width as f32,
                dest_h: info.height as f32,
                transform: 0,
                clear_rgba: [0.0, 0.0, 0.0, 1.0],
            };
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
