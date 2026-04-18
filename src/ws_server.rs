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

use crate::control_proto as pb;
use crate::ipc::proto::ControlMsg;
use crate::renderer_manager;
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
                sink.send(Message::Binary(resp.encode_to_vec())).await?;
                continue;
            }
        };

        let resp = dispatch(&state, req).await;
        sink.send(Message::Binary(resp.encode_to_vec())).await?;
    }

    log::debug!("ws conn {peer} closed");
    Ok(())
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

async fn dispatch(state: &AppState, req: pb::Request) -> pb::Response {
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
            ok(rid, Res::RendererList(pb::RendererListResponse { renderers: ids }))
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
                    types: def.types.clone(),
                    priority: def.priority,
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
            let entries: Vec<pb::WallpaperEntry> = if r.wp_type.is_empty() {
                mgr.list().iter().map(entry_to_pb).collect()
            } else {
                mgr.list_by_type(&r.wp_type)
                    .into_iter()
                    .map(entry_to_pb)
                    .collect()
            };
            let count = entries.len() as u32;
            ok(
                rid,
                Res::WallpaperList(pb::WallpaperListResponse {
                    wallpapers: entries,
                    count,
                }),
            )
        }

        Req::WallpaperScan(_) => {
            let mut mgr = state.source_manager.lock().await;
            match mgr.scan_all() {
                Ok(()) => {
                    let count = mgr.list().len() as u32;
                    ok(rid, Res::WallpaperScan(pb::WallpaperScanResponse { count }))
                }
                Err(e) => error_response(rid, pb::Status::Internal, format!("scan failed: {e}")),
            }
        }

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
            // Single-wallpaper mode, spawn-before-kill: bring the new
            // renderer up first so any active display frame loop has a
            // replacement to rebind to before the old broadcast closes.
            let pre_existing: Vec<String> = state.renderer_manager.list().await;
            let width = if r.width == 0 { 1920 } else { r.width };
            let height = if r.height == 0 { 1080 } else { r.height };
            let fps = if r.fps == 0 { 30 } else { r.fps };
            let spawn_req = renderer_manager::SpawnRequest {
                wp_type: entry.wp_type.clone(),
                metadata: entry.metadata.clone(),
                width,
                height,
                fps,
                test_pattern: false,
            };
            match state.renderer_manager.spawn(spawn_req).await {
                Ok(renderer_id) => {
                    if let Some(handle) = state.renderer_manager.get(&renderer_id).await {
                        state.router.register_renderer(handle).await;
                    }
                    state.router.relink_all_displays_to(&renderer_id).await;
                    for old_id in pre_existing {
                        if old_id != renderer_id {
                            state.router.unregister_renderer(&old_id).await;
                            let _ = state.renderer_manager.kill(&old_id).await;
                        }
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
                Err(e) => error_response(rid, pb::Status::Internal, format!("spawn failed: {e}")),
            }
        }
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

fn error_response(request_id: u64, status: pb::Status, message: String) -> pb::Response {
    pb::Response {
        request_id,
        status: status as i32,
        message,
        payload: None,
    }
}

fn entry_to_pb(e: &crate::wallpaper_type::WallpaperEntry) -> pb::WallpaperEntry {
    pb::WallpaperEntry {
        id: e.id.clone(),
        name: e.name.clone(),
        wp_type: e.wp_type.clone(),
        resource: e.resource.clone(),
        preview: e.preview.clone().unwrap_or_default(),
        metadata: e.metadata.clone(),
    }
}
