use actix_web::{web, App, HttpServer, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

mod ipc;
mod plugin;
mod renderer_manager;
mod display_proto;
mod dummy_fence;
mod scheduler;
mod display_endpoint;
mod wallpaper_type;

// ---------------------------------------------------------------------------
// HTTP API types (renderer management only)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug)]
struct ApiResponse<T> {
    status: String,
    data: Option<T>,
    message: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct RendererSpawnRequest {
    /// Wallpaper type (e.g. "scene", "image", "video").
    #[serde(default = "default_wp_type")]
    wp_type: String,
    /// Type-specific metadata forwarded as CLI args to the renderer.
    /// For "scene": {"scene": "<pkg>", "assets": "<dir>"}.
    #[serde(default)]
    metadata: std::collections::HashMap<String, String>,
    width: u32,
    height: u32,
    fps: u32,
}

fn default_wp_type() -> String {
    "scene".to_string()
}

#[derive(Serialize, Deserialize, Debug)]
struct RendererSpawnResponse {
    renderer_id: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct MouseInput {
    x: f64,
    y: f64,
}

#[derive(Serialize, Deserialize, Debug)]
struct FpsInput {
    fps: u32,
}

// ---------------------------------------------------------------------------
// AppState (slimmed down — renderer manager only)
// ---------------------------------------------------------------------------

struct AppState {
    renderer_manager: Arc<renderer_manager::RendererManager>,
    source_manager: Arc<tokio::sync::Mutex<plugin::source_manager::SourceManager>>,
}

// ---------------------------------------------------------------------------
// /api/renderer/* handlers
// ---------------------------------------------------------------------------

async fn renderer_spawn(
    req: web::Json<RendererSpawnRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let r = req.into_inner();
    let spawn_req = renderer_manager::SpawnRequest {
        wp_type: r.wp_type,
        metadata: r.metadata,
        width: r.width,
        height: r.height,
        fps: r.fps,
        test_pattern: false,
    };
    match state.renderer_manager.spawn(spawn_req).await {
        Ok(id) => HttpResponse::Ok().json(ApiResponse {
            status: "success".to_string(),
            data: Some(RendererSpawnResponse { renderer_id: id }),
            message: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse::<RendererSpawnResponse> {
            status: "error".to_string(),
            data: None,
            message: Some(format!("spawn failed: {e}")),
        }),
    }
}

async fn renderer_list(state: web::Data<AppState>) -> impl Responder {
    let ids = state.renderer_manager.list().await;
    HttpResponse::Ok().json(ApiResponse {
        status: "success".to_string(),
        data: Some(serde_json::json!({ "renderers": ids })),
        message: None,
    })
}

async fn renderer_play(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let id = path.into_inner();
    match state.renderer_manager.send_control(&id, ipc::proto::ControlMsg::Play).await {
        Ok(()) => HttpResponse::Ok().json(ApiResponse::<()> {
            status: "success".to_string(), data: None, message: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse::<()> {
            status: "error".to_string(), data: None, message: Some(e.to_string()),
        }),
    }
}

async fn renderer_pause(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let id = path.into_inner();
    match state.renderer_manager.send_control(&id, ipc::proto::ControlMsg::Pause).await {
        Ok(()) => HttpResponse::Ok().json(ApiResponse::<()> {
            status: "success".to_string(), data: None, message: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse::<()> {
            status: "error".to_string(), data: None, message: Some(e.to_string()),
        }),
    }
}

async fn renderer_mouse(
    path: web::Path<String>,
    body: web::Json<MouseInput>,
    state: web::Data<AppState>,
) -> impl Responder {
    let id = path.into_inner();
    match state.renderer_manager.send_control(&id, ipc::proto::ControlMsg::Mouse { x: body.x, y: body.y }).await {
        Ok(()) => HttpResponse::Ok().json(ApiResponse::<()> {
            status: "success".to_string(), data: None, message: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse::<()> {
            status: "error".to_string(), data: None, message: Some(e.to_string()),
        }),
    }
}

async fn renderer_set_fps(
    path: web::Path<String>,
    body: web::Json<FpsInput>,
    state: web::Data<AppState>,
) -> impl Responder {
    let id = path.into_inner();
    match state.renderer_manager.send_control(&id, ipc::proto::ControlMsg::SetFps { fps: body.fps }).await {
        Ok(()) => HttpResponse::Ok().json(ApiResponse::<()> {
            status: "success".to_string(), data: None, message: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse::<()> {
            status: "error".to_string(), data: None, message: Some(e.to_string()),
        }),
    }
}

async fn renderer_kill(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let id = path.into_inner();
    match state.renderer_manager.kill(&id).await {
        Ok(()) => HttpResponse::Ok().json(ApiResponse::<()> {
            status: "success".to_string(), data: None, message: None,
        }),
        Err(e) => HttpResponse::NotFound().json(ApiResponse::<()> {
            status: "error".to_string(), data: None, message: Some(e.to_string()),
        }),
    }
}

async fn renderer_plugin_list(state: web::Data<AppState>) -> impl Responder {
    let registry = state.renderer_manager.registry();
    let plugins: Vec<serde_json::Value> = registry
        .all_renderers()
        .iter()
        .map(|def| {
            serde_json::json!({
                "name": def.name,
                "bin": def.bin.to_string_lossy(),
                "types": def.types,
                "priority": def.priority,
            })
        })
        .collect();
    HttpResponse::Ok().json(ApiResponse {
        status: "success".to_string(),
        data: Some(serde_json::json!({ "renderers": plugins, "supported_types": registry.supported_types() })),
        message: None,
    })
}

// ---------------------------------------------------------------------------
// /api/wallpaper/* and /api/source/* handlers
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug)]
struct WallpaperListQuery {
    #[serde(rename = "type")]
    wp_type: Option<String>,
}

async fn wallpaper_list(
    query: web::Query<WallpaperListQuery>,
    state: web::Data<AppState>,
) -> impl Responder {
    let mgr = state.source_manager.lock().await;
    let entries: Vec<&wallpaper_type::WallpaperEntry> = match &query.wp_type {
        Some(t) => mgr.list_by_type(t),
        None => mgr.list().iter().collect(),
    };
    HttpResponse::Ok().json(ApiResponse {
        status: "success".to_string(),
        data: Some(serde_json::json!({ "wallpapers": entries, "count": entries.len() })),
        message: None,
    })
}

async fn wallpaper_scan(state: web::Data<AppState>) -> impl Responder {
    let mut mgr = state.source_manager.lock().await;
    match mgr.scan_all() {
        Ok(()) => {
            let count = mgr.list().len();
            HttpResponse::Ok().json(ApiResponse {
                status: "success".to_string(),
                data: Some(serde_json::json!({ "count": count })),
                message: Some(format!("scanned {count} wallpapers")),
            })
        }
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse::<()> {
            status: "error".to_string(),
            data: None,
            message: Some(format!("scan failed: {e}")),
        }),
    }
}

async fn source_list(state: web::Data<AppState>) -> impl Responder {
    let mgr = state.source_manager.lock().await;
    match mgr.plugins() {
        Ok(plugins) => HttpResponse::Ok().json(ApiResponse {
            status: "success".to_string(),
            data: Some(serde_json::json!({ "sources": plugins })),
            message: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse::<()> {
            status: "error".to_string(),
            data: None,
            message: Some(format!("{e}")),
        }),
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct WallpaperApplyRequest {
    wallpaper_id: String,
    #[serde(default = "default_apply_width")]
    width: u32,
    #[serde(default = "default_apply_height")]
    height: u32,
    #[serde(default = "default_apply_fps")]
    fps: u32,
}

fn default_apply_width() -> u32 { 1920 }
fn default_apply_height() -> u32 { 1080 }
fn default_apply_fps() -> u32 { 30 }

async fn wallpaper_apply(
    req: web::Json<WallpaperApplyRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let r = req.into_inner();

    // 1. Lookup wallpaper from source manager.
    let entry = {
        let mgr = state.source_manager.lock().await;
        mgr.get(&r.wallpaper_id).cloned()
    };
    let entry = match entry {
        Some(e) => e,
        None => {
            return HttpResponse::NotFound().json(ApiResponse::<()> {
                status: "error".to_string(),
                data: None,
                message: Some(format!("wallpaper '{}' not found", r.wallpaper_id)),
            });
        }
    };

    // 2. Check that a renderer exists for this wallpaper type.
    if state.renderer_manager.registry().resolve(&entry.wp_type).is_none() {
        return HttpResponse::BadRequest().json(ApiResponse::<()> {
            status: "error".to_string(),
            data: None,
            message: Some(format!("no renderer for wallpaper type '{}'", entry.wp_type)),
        });
    }

    // 3. Kill any currently running renderers (single-wallpaper mode for now).
    let existing = state.renderer_manager.list().await;
    for id in existing {
        let _ = state.renderer_manager.kill(&id).await;
    }

    // 4. Spawn the appropriate renderer.
    let spawn_req = renderer_manager::SpawnRequest {
        wp_type: entry.wp_type.clone(),
        metadata: entry.metadata.clone(),
        width: r.width,
        height: r.height,
        fps: r.fps,
        test_pattern: false,
    };
    match state.renderer_manager.spawn(spawn_req).await {
        Ok(renderer_id) => HttpResponse::Ok().json(ApiResponse {
            status: "success".to_string(),
            data: Some(serde_json::json!({
                "renderer_id": renderer_id,
                "wallpaper_id": entry.id,
                "wp_type": entry.wp_type,
                "name": entry.name,
            })),
            message: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse::<()> {
            status: "error".to_string(),
            data: None,
            message: Some(format!("spawn failed: {e}")),
        }),
    }
}

async fn health_check() -> impl Responder {
    HttpResponse::Ok().json(ApiResponse::<serde_json::Value> {
        status: "success".to_string(),
        data: Some(serde_json::json!({ "status": "healthy", "service": "waywallen" })),
        message: None,
    })
}

// ---------------------------------------------------------------------------
// Entrypoint
// ---------------------------------------------------------------------------

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();

    let registry = plugin::renderer_registry::build_default_registry()
        .expect("failed to build renderer registry");

    // Source management: load Lua plugins from $WAYWALLEN_SOURCE_DIR or
    // $XDG_DATA_HOME/waywallen/sources/, falling back to the bundled
    // sources/ dir next to the binary.
    let mut source_mgr = plugin::source_manager::SourceManager::new(std::collections::HashMap::new())
        .expect("failed to create source manager");
    let source_dir = std::env::var_os("WAYWALLEN_SOURCE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let base = std::env::var_os("XDG_DATA_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    let home = std::env::var_os("HOME").unwrap_or_default();
                    std::path::PathBuf::from(home).join(".local/share")
                });
            base.join("waywallen/sources")
        });
    if source_dir.is_dir() {
        let _ = source_mgr.load_all(&source_dir);
    }
    // Also try bundled sources/ next to the executable.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let bundled = parent.join("sources");
            if bundled.is_dir() {
                let _ = source_mgr.load_all(&bundled);
            }
        }
    }
    // Initial scan.
    let _ = source_mgr.scan_all();

    let state = web::Data::new(AppState {
        renderer_manager: Arc::new(renderer_manager::RendererManager::new(registry)),
        source_manager: Arc::new(tokio::sync::Mutex::new(source_mgr)),
    });

    // Display endpoint on UDS (waywallen-display-v1 protocol).
    {
        let mgr = state.renderer_manager.clone();
        let sock_path = display_endpoint::default_socket_path();
        let sched = Arc::new(std::sync::Mutex::new(scheduler::Scheduler::new()));
        tokio::spawn(async move {
            if let Err(e) = display_endpoint::serve(&sock_path, mgr, sched).await {
                log::error!("display endpoint exited: {e}");
            }
        });
    }

    HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .service(
                web::scope("/api")
                    .service(web::resource("/health").route(web::get().to(health_check)))
                    .service(web::resource("/renderer/spawn").route(web::post().to(renderer_spawn)))
                    .service(web::resource("/renderer/list").route(web::get().to(renderer_list)))
                    .service(web::resource("/renderer/{id}/play").route(web::post().to(renderer_play)))
                    .service(web::resource("/renderer/{id}/pause").route(web::post().to(renderer_pause)))
                    .service(web::resource("/renderer/{id}/mouse").route(web::post().to(renderer_mouse)))
                    .service(web::resource("/renderer/{id}/fps").route(web::post().to(renderer_set_fps)))
                    .service(web::resource("/renderer/{id}").route(web::delete().to(renderer_kill)))
                    .service(web::resource("/renderer-plugin/list").route(web::get().to(renderer_plugin_list)))
                    .service(web::resource("/wallpaper/list").route(web::get().to(wallpaper_list)))
                    .service(web::resource("/wallpaper/scan").route(web::post().to(wallpaper_scan)))
                    .service(web::resource("/source/list").route(web::get().to(source_list)))
                    .service(web::resource("/wallpaper/apply").route(web::post().to(wallpaper_apply)))
            )
    })
    .bind("0.0.0.0:8080")?
    .run()
    .await
}
