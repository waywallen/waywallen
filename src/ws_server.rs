//! WebSocket + protobuf control plane.
//!
//! Single `/` endpoint. Each connection carries length-prefixed-by-WS-frame
//! `waywallen.control.v1.Request` / `Response` envelopes. All RPCs are
//! multiplexed via `request_id` and the `payload` oneof.

use std::sync::Arc;

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::protocol::Message;

use crate::control;
use crate::control_proto as pb;
use crate::ipc::proto::ControlMsg;
use crate::model::repo;
use crate::renderer_manager;
use crate::routing::{DisplaySnapshot, LibrarySnapshot, RendererSnapshot, RouterEvent};
use crate::AppState;

/// Bind the WebSocket control plane and return the actual local address
/// (useful when binding to port 0 for OS-assigned ports).  The returned
/// future runs the accept loop and never returns under normal operation.
pub async fn bind(state: Arc<AppState>, addr: &str) -> Result<(std::net::SocketAddr, impl std::future::Future<Output = Result<()>>)> {
    let listener = TcpListener::bind(addr).await?;
    let local_addr = listener.local_addr()?;
    log::info!("ws control plane listening on {local_addr}");
    let fut = accept_loop(state, listener);
    Ok((local_addr, fut))
}

pub async fn serve(state: Arc<AppState>, addr: &str) -> Result<()> {
    let (_, fut) = bind(state, addr).await?;
    fut.await
}

async fn accept_loop(state: Arc<AppState>, listener: TcpListener) -> Result<()> {
    loop {
        let (stream, peer) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_conn(state, stream, peer).await {
                log::warn!("ws conn {peer} ended: {e}");
            }
        });
    }
}

async fn handle_conn(
    state: Arc<AppState>,
    stream: TcpStream,
    peer: std::net::SocketAddr,
) -> Result<()> {
    let ws = tokio_tungstenite::accept_async(stream).await?;
    log::debug!("ws conn {peer} open");
    let (mut sink, mut src) = ws.split();

    // Subscribe to router events *before* snapshotting so no updates
    // get dropped between the snapshot and the live stream starting.
    let mut events_rx = state.router.subscribe_events();
    {
        let snap = state.router.snapshot_displays().await;
        let evt = displays_replace_event(snap);
        sink.send(Message::Binary(wrap_event(evt).encode_to_vec())).await?;
    }
    {
        let snap = state.router.snapshot_renderers().await;
        let evt = renderers_replace_event(snap);
        sink.send(Message::Binary(wrap_event(evt).encode_to_vec())).await?;
    }

    {
        let snap = control::list_library_snapshots(&state.db).await;
        let evt = libraries_replace_event(snap);
        sink.send(Message::Binary(wrap_event(evt).encode_to_vec())).await?;
    }

    loop {
        tokio::select! {
            msg = src.next() => {
                let Some(msg) = msg else { break };
                let msg = msg?;
                let bytes = match msg {
                    Message::Binary(b) => b,
                    Message::Text(t) => t.into_bytes(),
                    Message::Ping(_) | Message::Pong(_) => continue,
                    Message::Close(_) => break,
                    Message::Frame(_) => continue,
                };

                let req = match pb::Request::decode(&bytes[..]) {
                    Ok(r) => r,
                    Err(e) => {
                        let resp = error_response(0, pb::Status::InvalidArgument, format!("decode: {e}"));
                        sink.send(Message::Binary(wrap_response(resp).encode_to_vec())).await?;
                        continue;
                    }
                };

                let resp = dispatch(&state, req).await;
                sink.send(Message::Binary(wrap_response(resp).encode_to_vec())).await?;
            }
            evt = events_rx.recv() => {
                match evt {
                    Ok(e) => {
                        let pe = router_event_to_pb(e);
                        sink.send(Message::Binary(wrap_event(pe).encode_to_vec())).await?;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        log::warn!("ws {peer}: event lag {n}; resending full snapshot");
                        let snap = state.router.snapshot_displays().await;
                        let evt = displays_replace_event(snap);
                        sink.send(Message::Binary(wrap_event(evt).encode_to_vec())).await?;
                        let rsnap = state.router.snapshot_renderers().await;
                        let revt = renderers_replace_event(rsnap);
                        sink.send(Message::Binary(wrap_event(revt).encode_to_vec())).await?;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        // Router shut down; stop emitting but keep the
                        // request path alive until the client closes.
                        log::info!("ws {peer}: router event channel closed");
                        // Drain remaining requests without event select.
                        while let Some(msg) = src.next().await {
                            let msg = msg?;
                            let bytes = match msg {
                                Message::Binary(b) => b,
                                Message::Text(t) => t.into_bytes(),
                                Message::Ping(_) | Message::Pong(_) => continue,
                                Message::Close(_) => break,
                                Message::Frame(_) => continue,
                            };
                            let req = match pb::Request::decode(&bytes[..]) {
                                Ok(r) => r,
                                Err(e) => {
                                    let resp = error_response(0, pb::Status::InvalidArgument, format!("decode: {e}"));
                                    sink.send(Message::Binary(wrap_response(resp).encode_to_vec())).await?;
                                    continue;
                                }
                            };
                            let resp = dispatch(&state, req).await;
                            sink.send(Message::Binary(wrap_response(resp).encode_to_vec())).await?;
                        }
                        break;
                    }
                }
            }
        }
    }

    log::debug!("ws conn {peer} closed");
    Ok(())
}

// ---------------------------------------------------------------------------
// RouterEvent → pb::Event translation
// ---------------------------------------------------------------------------

fn display_snapshot_to_pb(s: DisplaySnapshot) -> pb::DisplayInfo {
    pb::DisplayInfo {
        display_id: s.id,
        name: s.name,
        width: s.width,
        height: s.height,
        refresh_mhz: s.refresh_mhz,
        links: s
            .links
            .into_iter()
            .map(|l| pb::DisplayLinkInfo {
                renderer_id: l.renderer_id,
                z_order: l.z_order,
            })
            .collect(),
    }
}

fn displays_replace_event(snap: Vec<DisplaySnapshot>) -> pb::Event {
    pb::Event {
        payload: Some(pb::event::Payload::DisplaySnapshot(pb::DisplaySnapshot {
            displays: snap.into_iter().map(display_snapshot_to_pb).collect(),
        })),
    }
}

fn renderer_snapshot_to_pb(s: RendererSnapshot) -> pb::RendererInstance {
    pb::RendererInstance {
        renderer_id: s.id,
        fps: s.fps,
        status: s.status.as_str().to_string(),
        name: s.name,
        pid: s.pid,
    }
}

fn renderers_replace_event(snap: Vec<RendererSnapshot>) -> pb::Event {
    pb::Event {
        payload: Some(pb::event::Payload::RendererSnapshot(pb::RendererSnapshot {
            renderers: snap.into_iter().map(renderer_snapshot_to_pb).collect(),
        })),
    }
}

fn library_instance_to_pb(s: LibrarySnapshot) -> pb::LibraryInstance {
    pb::LibraryInstance {
        id: s.id,
        path: s.path,
        plugin_name: s.plugin_name,
    }
}

fn libraries_replace_event(snap: Vec<LibrarySnapshot>) -> pb::Event {
    pb::Event {
        payload: Some(pb::event::Payload::LibrarySnapshot(pb::LibrarySnapshot {
            libraries: snap.into_iter().map(library_instance_to_pb).collect(),
        })),
    }
}

fn router_event_to_pb(e: RouterEvent) -> pb::Event {
    match e {
        RouterEvent::DisplayUpsert(s) => pb::Event {
            payload: Some(pb::event::Payload::DisplayChanged(pb::DisplayChanged {
                display: Some(display_snapshot_to_pb(s)),
            })),
        },
        RouterEvent::DisplayRemoved(id) => pb::Event {
            payload: Some(pb::event::Payload::DisplayRemoved(pb::DisplayRemoved {
                display_id: id,
            })),
        },
        RouterEvent::DisplaysReplace(list) => displays_replace_event(list),
        RouterEvent::RendererUpsert(s) => pb::Event {
            payload: Some(pb::event::Payload::RendererChanged(pb::RendererChanged {
                renderer: Some(renderer_snapshot_to_pb(s)),
            })),
        },
        RouterEvent::RendererRemoved(id) => pb::Event {
            payload: Some(pb::event::Payload::RendererRemoved(pb::RendererRemoved {
                renderer_id: id,
            })),
        },
        RouterEvent::RenderersReplace(list) => renderers_replace_event(list),
        RouterEvent::LibraryUpsert(s) => pb::Event {
            payload: Some(pb::event::Payload::LibraryChanged(pb::LibraryChanged {
                library: Some(library_instance_to_pb(s)),
            })),
        },
        RouterEvent::LibraryRemoved(id) => pb::Event {
            payload: Some(pb::event::Payload::LibraryRemoved(pb::LibraryRemoved { id })),
        },
        RouterEvent::LibrariesReplace(list) => libraries_replace_event(list),
    }
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

async fn dispatch(state: &Arc<AppState>, req: pb::Request) -> pb::Response {
    let rid = req.request_id;
    let Some(payload) = req.payload else {
        return error_response(rid, pb::Status::InvalidArgument, "empty request payload".into());
    };

    use pb::request::Payload as Req;
    use pb::response::Payload as Res;

    match payload {
        Req::Health(_) => ok(
            rid,
            Res::Health(pb::HealthResponse {
                service: "waywallen".into(),
                state: "healthy".into(),
            }),
        ),

        Req::RendererSpawn(r) => {
            let spawn_req = renderer_manager::SpawnRequest {
                wp_type: if r.wp_type.is_empty() { "scene".into() } else { r.wp_type },
                metadata: r.metadata,
                width: r.width,
                height: r.height,
                fps: r.fps,
                test_pattern: false,
            };
            match state.renderer_manager.spawn(spawn_req).await {
                Ok(id) => {
                    if let Some(handle) = state.renderer_manager.get(&id).await {
                        state.router.register_renderer(handle).await;
                    }
                    ok(rid, Res::RendererSpawn(pb::RendererSpawnResponse { renderer_id: id }))
                }
                Err(e) => error_response(rid, pb::Status::Internal, format!("spawn failed: {e}")),
            }
        }

        Req::RendererList(_) => {
            let ids = state.renderer_manager.list().await;
            let mut instances = Vec::with_capacity(ids.len());
            for id in &ids {
                let (fps, name, pid) = match state.renderer_manager.get(id).await {
                    Some(h) => (h.fps, h.name.clone(), h.pid.unwrap_or(0)),
                    None => (0, String::new(), 0),
                };
                let status = if state.router.is_paused(id).await { "paused" } else { "playing" };
                instances.push(pb::RendererInstance {
                    renderer_id: id.clone(),
                    fps,
                    status: status.into(),
                    name,
                    pid,
                });
            }
            ok(
                rid,
                Res::RendererList(pb::RendererListResponse {
                    renderers: ids,
                    instances,
                }),
            )
        }

        Req::RendererPlay(r) => match state
            .renderer_manager
            .send_control(&r.renderer_id, ControlMsg::Play)
            .await
        {
            Ok(()) => ok(rid, Res::RendererPlay(pb::Empty {})),
            Err(e) => error_response(rid, pb::Status::Internal, e.to_string()),
        },

        Req::RendererPause(r) => match state
            .renderer_manager
            .send_control(&r.renderer_id, ControlMsg::Pause)
            .await
        {
            Ok(()) => ok(rid, Res::RendererPause(pb::Empty {})),
            Err(e) => error_response(rid, pb::Status::Internal, e.to_string()),
        },

        Req::RendererMouse(r) => match state
            .renderer_manager
            .send_control(&r.renderer_id, ControlMsg::Mouse { x: r.x, y: r.y })
            .await
        {
            Ok(()) => ok(rid, Res::RendererMouse(pb::Empty {})),
            Err(e) => error_response(rid, pb::Status::Internal, e.to_string()),
        },

        Req::RendererFps(r) => match state
            .renderer_manager
            .send_control(&r.renderer_id, ControlMsg::SetFps { fps: r.fps })
            .await
        {
            Ok(()) => ok(rid, Res::RendererFps(pb::Empty {})),
            Err(e) => error_response(rid, pb::Status::Internal, e.to_string()),
        },

        Req::RendererKill(r) => {
            state.router.unregister_renderer(&r.renderer_id).await;
            match state.renderer_manager.kill(&r.renderer_id).await {
                Ok(()) => ok(rid, Res::RendererKill(pb::Empty {})),
                Err(e) => error_response(rid, pb::Status::NotFound, e.to_string()),
            }
        }

        Req::RendererPluginList(_) => {
            let registry = state.renderer_manager.registry();
            let renderers = registry
                .all_renderers()
                .iter()
                .map(|def| pb::RendererPluginInfo {
                    name: def.name.clone(),
                    bin: def.bin.to_string_lossy().into_owned(),
                    types: def.types.iter().map(|t| t.to_string()).collect(),
                    priority: def.priority,
                    version: def.version.clone(),
                })
                .collect();
            let supported_types = registry
                .supported_types()
                .into_iter()
                .cloned()
                .collect();
            ok(
                rid,
                Res::RendererPluginList(pb::RendererPluginListResponse {
                    renderers,
                    supported_types,
                }),
            )
        }

        Req::WallpaperList(r) => {
            let mgr = state.source_manager.lock().await;

            // Build a lookup map: (library.path, item.path) -> item::Model
            // so we can overlay DB media-meta (size/width/height/format) onto
            // each WallpaperEntry before sending it to the UI.
            let db_meta_map: std::collections::HashMap<(String, String), crate::model::entities::item::Model> = {
                let libs = repo::list_libraries(&state.db).await.unwrap_or_default();
                let lib_path_by_id: std::collections::HashMap<i64, String> =
                    libs.into_iter().map(|l| (l.id, l.path)).collect();
                let items = repo::list_items_all(&state.db).await.unwrap_or_default();
                items
                    .into_iter()
                    .filter_map(|it| {
                        let lib_path = lib_path_by_id.get(&it.library_id)?.clone();
                        let item_path = it.path.clone();
                        Some(((lib_path, item_path), it))
                    })
                    .collect()
            };

            let raw_entries: Vec<&crate::wallpaper_type::WallpaperEntry> = if r.wp_type.is_empty() {
                mgr.list().iter().collect()
            } else {
                mgr.list_by_type(&r.wp_type)
            };

            let entries: Vec<pb::WallpaperEntry> = raw_entries
                .into_iter()
                .map(|e| {
                    let db_meta = crate::model::sync::relative_under_root(
                        &e.library_root,
                        &e.resource,
                    )
                    .and_then(|rel| db_meta_map.get(&(e.library_root.clone(), rel)));
                    entry_to_pb(e, db_meta)
                })
                .collect();
            let count = entries.len() as u32;
            ok(
                rid,
                Res::WallpaperList(pb::WallpaperListResponse {
                    wallpapers: entries,
                    count,
                }),
            )
        }

        Req::WallpaperScan(_) => match control::refresh_sources(&state).await {
            Ok(count) => ok(
                rid,
                Res::WallpaperScan(pb::WallpaperScanResponse { count: count as u32 }),
            ),
            Err(e) => error_response(rid, pb::Status::Internal, format!("scan failed: {e}")),
        },

        Req::SourceList(_) => {
            let mgr = state.source_manager.lock().await;
            match mgr.plugins() {
                Ok(plugins) => {
                    let sources = plugins
                        .into_iter()
                        .map(|p| pb::SourcePluginInfo {
                            name: p.name,
                            types: p.types,
                            version: p.version,
                        })
                        .collect();
                    ok(rid, Res::SourceList(pb::SourceListResponse { sources }))
                }
                Err(e) => error_response(rid, pb::Status::Internal, e.to_string()),
            }
        }

        Req::DisplayList(_) => {
            let snap = state.router.snapshot_displays().await;
            let displays = snap
                .into_iter()
                .map(|d| pb::DisplayInfo {
                    display_id: d.id,
                    name: d.name,
                    width: d.width,
                    height: d.height,
                    refresh_mhz: d.refresh_mhz,
                    links: d
                        .links
                        .into_iter()
                        .map(|l| pb::DisplayLinkInfo {
                            renderer_id: l.renderer_id,
                            z_order: l.z_order,
                        })
                        .collect(),
                })
                .collect();
            ok(rid, Res::DisplayList(pb::DisplayListResponse { displays }))
        }

        Req::WallpaperApply(r) => {
            let entry = {
                let mgr = state.source_manager.lock().await;
                mgr.get(&r.wallpaper_id).cloned()
            };
            let Some(entry) = entry else {
                return error_response(
                    rid,
                    pb::Status::NotFound,
                    format!("wallpaper '{}' not found", r.wallpaper_id),
                );
            };
            if state
                .renderer_manager
                .registry()
                .resolve(&entry.wp_type)
                .is_none()
            {
                return error_response(
                    rid,
                    pb::Status::InvalidArgument,
                    format!("no renderer for wallpaper type '{}'", entry.wp_type),
                );
            }
            // Resolution comes from settings.global. fps is a per-plugin
            // concern: pulled out of `[plugin.<name>].fps` if present,
            // otherwise hardcoded to 30 as a safe last resort. The
            // remaining `[plugin.<name>]` keys flow into spawn metadata
            // as baseline kv; per-wallpaper metadata wins on collisions.
            let g = state.settings.global();
            let width = g.default_width;
            let height = g.default_height;

            let plugin_name = state
                .renderer_manager
                .registry()
                .resolve(&entry.wp_type)
                .map(|def| def.name.clone());

            let mut plugin_kv = plugin_name
                .as_deref()
                .and_then(|n| state.settings.plugin(n))
                .unwrap_or_default();
            // Promote `fps` out of the plugin kv into the typed spawn
            // field so it ends up as `--fps N` instead of getting
            // double-passed via metadata.
            let fps: u32 = plugin_kv
                .remove("fps")
                .and_then(|v| v.parse().ok())
                .unwrap_or(30);

            let mut metadata: std::collections::HashMap<String, String> = plugin_kv;
            // Wallpaper-supplied keys override plugin defaults.
            metadata.extend(entry.metadata.clone());

            let spawn_req = renderer_manager::SpawnRequest {
                wp_type: entry.wp_type.clone(),
                metadata,
                width,
                height,
                fps,
                test_pattern: false,
            };

            // Reuse an existing renderer if its spawn parameters match
            // exactly (wp_type + metadata + w/h/fps). This makes per-
            // display apply cheap: N displays pointing at the same
            // wallpaper + same settings share one renderer process.
            let renderer_id = match state.renderer_manager.find_reusable(&spawn_req).await {
                Some(existing_id) => {
                    log::info!(
                        "wallpaper_apply: reusing renderer {existing_id} for wallpaper {}",
                        entry.id
                    );
                    existing_id
                }
                None => match state.renderer_manager.spawn(spawn_req).await {
                    Ok(new_id) => {
                        if let Some(handle) = state.renderer_manager.get(&new_id).await {
                            state.router.register_renderer(handle).await;
                        }
                        new_id
                    }
                    Err(e) => {
                        return error_response(
                            rid,
                            pb::Status::Internal,
                            format!("spawn failed: {e}"),
                        );
                    }
                },
            };

            // Relink: empty display_ids means "all currently registered
            // displays" (pre-M4 behaviour). Old renderers left with
            // zero links get paused immediately and reclaimed by the
            // router's idle reaper after IDLE_KILL_TIMEOUT.
            if r.display_ids.is_empty() {
                state.router.relink_all_displays_to(&renderer_id).await;
            } else {
                state
                    .router
                    .relink_displays_to(&r.display_ids, &renderer_id)
                    .await;
            }

            ok(
                rid,
                Res::WallpaperApply(pb::WallpaperApplyResponse {
                    renderer_id,
                    wallpaper_id: entry.id,
                    wp_type: entry.wp_type,
                    name: entry.name,
                }),
            )
        }

        Req::SettingsGet(_) => {
            let snap = state.settings.snapshot();
            ok(
                rid,
                Res::SettingsGet(pb::SettingsGetResponse {
                    global: Some(pb::GlobalSettings {
                        default_width: snap.global.default_width,
                        default_height: snap.global.default_height,
                    }),
                    plugins: snap
                        .plugins
                        .into_iter()
                        .map(|(k, v)| (k, pb::PluginSettings { values: v }))
                        .collect(),
                }),
            )
        }

        Req::SettingsSet(r) => {
            // Full replace. Missing `global` falls back to current
            // values so callers can update plugins alone by sending
            // None for global.
            let new_plugins: std::collections::HashMap<
                String,
                std::collections::HashMap<String, String>,
            > = r
                .plugins
                .into_iter()
                .map(|(k, v)| (k, v.values))
                .collect();
            state.settings.update(|s| {
                if let Some(g) = r.global.as_ref() {
                    s.global.default_width = g.default_width;
                    s.global.default_height = g.default_height;
                }
                s.plugins = new_plugins;
            });
            ok(rid, Res::SettingsSet(pb::Empty {}))
        }

        Req::LibraryList(_) => {
            let snap = control::list_library_snapshots(&state.db).await;
            ok(
                rid,
                Res::LibraryList(pb::LibraryListResponse {
                    libraries: snap.into_iter().map(library_instance_to_pb).collect(),
                }),
            )
        }

        Req::LibraryAdd(r) => {
            let plugin_id = match repo::find_plugin_by_name(&state.db, &r.plugin_name).await {
                Ok(Some(p)) => p.id,
                Ok(None) => {
                    return error_response(
                        rid,
                        pb::Status::NotFound,
                        format!("source plugin '{}' not found", r.plugin_name),
                    )
                }
                Err(e) => return error_response(rid, pb::Status::Internal, e.to_string()),
            };
            match repo::add_library(&state.db, plugin_id, &r.path).await {
                Ok(lib) => {
                    let snap = LibrarySnapshot {
                        id: lib.id,
                        path: lib.path,
                        plugin_name: r.plugin_name,
                    };
                    state.router.upsert_library(snap);
                    // Rescan so the new library's items flow into the
                    // in-memory snapshot + DB without waiting for the
                    // next daemon restart.
                    let rescan_state = state.clone();
                    tokio::spawn(async move {
                        if let Err(e) = control::refresh_sources(&rescan_state).await {
                            log::warn!("rescan after LibraryAdd failed: {e:#}");
                        }
                    });
                    ok(rid, Res::LibraryAdd(pb::Empty {}))
                }
                Err(e) => error_response(rid, pb::Status::Internal, e.to_string()),
            }
        }

        Req::LibraryAutoDetect(_) => match control::auto_detect_libraries(&state).await {
            Ok(added) => ok(
                rid,
                Res::LibraryAutoDetect(pb::LibraryAutoDetectResponse {
                    added: added.into_iter().map(library_instance_to_pb).collect(),
                }),
            ),
            Err(e) => error_response(rid, pb::Status::Internal, e.to_string()),
        },

        Req::LibraryRemove(r) => match repo::remove_library(&state.db, r.id).await {
            Ok(_) => {
                state.router.remove_library(r.id);
                let rescan_state = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = control::refresh_sources(&rescan_state).await {
                        log::warn!("rescan after LibraryRemove failed: {e:#}");
                    }
                });
                ok(rid, Res::LibraryRemove(pb::Empty {}))
            }
            Err(e) => error_response(rid, pb::Status::Internal, e.to_string()),
        },
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn ok(request_id: u64, payload: pb::response::Payload) -> pb::Response {
    pb::Response {
        request_id,
        status: pb::Status::Ok as i32,
        message: String::new(),
        payload: Some(payload),
    }
}

fn wrap_response(resp: pb::Response) -> pb::ServerFrame {
    pb::ServerFrame {
        kind: Some(pb::server_frame::Kind::Response(resp)),
    }
}

#[allow(dead_code)]
pub fn wrap_event(evt: pb::Event) -> pb::ServerFrame {
    pb::ServerFrame {
        kind: Some(pb::server_frame::Kind::Event(evt)),
    }
}

fn error_response(request_id: u64, status: pb::Status, message: String) -> pb::Response {
    pb::Response {
        request_id,
        status: status as i32,
        message,
        payload: None,
    }
}

fn entry_to_pb(
    e: &crate::wallpaper_type::WallpaperEntry,
    db_meta: Option<&crate::model::entities::item::Model>,
) -> pb::WallpaperEntry {
    // Prefer DB values (freshest, written by the probe task); fall back to
    // what the Lua plugin may have pre-filled on the in-memory entry.
    let size = db_meta
        .and_then(|m| m.size)
        .or(e.size)
        .unwrap_or(0);
    let width = db_meta
        .and_then(|m| m.width)
        .map(|v| v as u32)
        .or(e.width)
        .unwrap_or(0);
    let height = db_meta
        .and_then(|m| m.height)
        .map(|v| v as u32)
        .or(e.height)
        .unwrap_or(0);
    let format = db_meta
        .and_then(|m| m.format.clone())
        .or_else(|| e.format.clone())
        .unwrap_or_default();

    pb::WallpaperEntry {
        id: e.id.clone(),
        name: e.name.clone(),
        wp_type: e.wp_type.clone(),
        resource: e.resource.clone(),
        preview: e.preview.clone().unwrap_or_default(),
        metadata: e.metadata.clone(),
        size,
        width,
        height,
        format,
    }
}
