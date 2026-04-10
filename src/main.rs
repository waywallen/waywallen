use actix_web::{web, App, HttpServer, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

mod ipc;
mod renderer_manager;
mod display_proto;
mod dummy_fence;
mod scheduler;
mod display_endpoint;

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
    scene_pkg: String,
    assets: String,
    width: u32,
    height: u32,
    fps: u32,
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
        scene_pkg: r.scene_pkg,
        assets: r.assets,
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

    let state = web::Data::new(AppState {
        renderer_manager: Arc::new(renderer_manager::RendererManager::new()),
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
            )
    })
    .bind("0.0.0.0:8080")?
    .run()
    .await
}
