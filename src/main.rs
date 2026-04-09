use actix_web::{web, App, HttpServer, Responder, HttpResponse};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;
use chrono::Utc;

mod producer;
mod consumer;
mod pipewire;
mod data_transfer;
mod vulkan_dma_buf;
mod dma_buf_stream;
mod ipc;
mod renderer_manager;
mod viewer_endpoint;

use vulkan_dma_buf::{VulkanDmaBufProducer, DmaBufImage};
use dma_buf_stream::DmaBufStreamManager;

#[derive(Serialize, Deserialize, Debug)]
struct ApiResponse<T> {
    status: String,
    data: Option<T>,
    message: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Producer {
    id: String,
    name: String,
    #[serde(alias = "type")]
    ty: String,
    created_at: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Consumer {
    id: String,
    name: String,
    #[serde(alias = "type")]
    ty: String,
    created_at: String,
}

#[derive(Debug)]
struct ProducerRegisterRequest {
    name: String,
    ty: String,
}

impl<'de> serde::Deserialize<'de> for ProducerRegisterRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Helper {
            name: String,
            #[serde(alias = "type")]
            ty: Option<String>,
        }

        let helper = Helper::deserialize(deserializer)?;
        Ok(ProducerRegisterRequest {
            name: helper.name,
            ty: helper.ty.unwrap_or_default(),
        })
    }
}

#[derive(Debug)]
struct ConsumerRegisterRequest {
    name: String,
    ty: String,
    producer_id: Option<String>,
}

impl<'de> serde::Deserialize<'de> for ConsumerRegisterRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Helper {
            name: String,
            #[serde(alias = "type")]
            ty: Option<String>,
            producer_id: Option<String>,
        }

        let helper = Helper::deserialize(deserializer)?;
        Ok(ConsumerRegisterRequest {
            name: helper.name,
            ty: helper.ty.unwrap_or_default(),
            producer_id: helper.producer_id,
        })
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct DataSendRequest {
    producer_id: String,
    #[serde(default)]
    data: Option<String>,
    metadata: serde_json::Value,
}

#[derive(Serialize, Deserialize, Debug)]
struct CreateDmaBufRequest {
    width: u32,
    height: u32,
    format: String,
    producer_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct FillDmaBufRequest {
    producer_id: String,
    data: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct DmaBufResponse {
    fd: i32,
    width: u32,
    height: u32,
    stride: u32,
    format: u32,
    modifier: u64,
}

#[derive(Serialize, Deserialize, Debug)]
struct DataReceiveRequest {
    consumer_id: String,
    timeout: Option<u64>,
}

#[derive(Serialize, Deserialize, Debug)]
struct CreateStreamRequest {
    stream_id: Option<String>,
    producer_id: String,
    width: u32,
    height: u32,
    format: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct StreamPushRequest {
    stream_id: String,
    width: u32,
    height: u32,
    format: String,
}

struct AppState {
    producers: Arc<Mutex<Vec<Producer>>>,
    consumers: Arc<Mutex<Vec<Consumer>>>,
    pipewire: pipewire::PipewireManager,
    vulkan_producer: Arc<Mutex<Option<VulkanDmaBufProducer>>>,
    dma_buffers: Arc<Mutex<HashMap<String, Arc<Mutex<Option<DmaBufImage>>>>>>,
    stream_manager: Arc<Mutex<DmaBufStreamManager>>,
    renderer_manager: Arc<renderer_manager::RendererManager>,
}

// ---------------------------------------------------------------------------
// Iteration 3 — /api/renderer/* HTTP surface
// ---------------------------------------------------------------------------

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

async fn renderer_play(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let id = path.into_inner();
    match state
        .renderer_manager
        .send_control(&id, ipc::proto::ControlMsg::Play)
        .await
    {
        Ok(()) => HttpResponse::Ok().json(ApiResponse::<()> {
            status: "success".to_string(),
            data: None,
            message: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse::<()> {
            status: "error".to_string(),
            data: None,
            message: Some(e.to_string()),
        }),
    }
}

async fn renderer_pause(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let id = path.into_inner();
    match state
        .renderer_manager
        .send_control(&id, ipc::proto::ControlMsg::Pause)
        .await
    {
        Ok(()) => HttpResponse::Ok().json(ApiResponse::<()> {
            status: "success".to_string(),
            data: None,
            message: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse::<()> {
            status: "error".to_string(),
            data: None,
            message: Some(e.to_string()),
        }),
    }
}

async fn renderer_mouse(
    path: web::Path<String>,
    body: web::Json<MouseInput>,
    state: web::Data<AppState>,
) -> impl Responder {
    let id = path.into_inner();
    match state
        .renderer_manager
        .send_control(&id, ipc::proto::ControlMsg::Mouse { x: body.x, y: body.y })
        .await
    {
        Ok(()) => HttpResponse::Ok().json(ApiResponse::<()> {
            status: "success".to_string(),
            data: None,
            message: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse::<()> {
            status: "error".to_string(),
            data: None,
            message: Some(e.to_string()),
        }),
    }
}

async fn renderer_set_fps(
    path: web::Path<String>,
    body: web::Json<FpsInput>,
    state: web::Data<AppState>,
) -> impl Responder {
    let id = path.into_inner();
    match state
        .renderer_manager
        .send_control(&id, ipc::proto::ControlMsg::SetFps { fps: body.fps })
        .await
    {
        Ok(()) => HttpResponse::Ok().json(ApiResponse::<()> {
            status: "success".to_string(),
            data: None,
            message: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse::<()> {
            status: "error".to_string(),
            data: None,
            message: Some(e.to_string()),
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
            status: "success".to_string(),
            data: None,
            message: None,
        }),
        Err(e) => HttpResponse::NotFound().json(ApiResponse::<()> {
            status: "error".to_string(),
            data: None,
            message: Some(e.to_string()),
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

async fn register_producer(
    req: web::Json<ProducerRegisterRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let producer = Producer {
        id: Uuid::new_v4().to_string(),
        name: req.name.clone(),
        ty: req.ty.clone(),
        created_at: Utc::now().to_rfc3339(),
    };

    {
        let mut producers = state.producers.lock().await;
        producers.push(producer.clone());
    }

    if let Err(e) = state.pipewire.register_producer(&producer.id).await {
        return HttpResponse::InternalServerError().json(ApiResponse::<Producer> {
            status: "error".to_string(),
            data: None,
            message: Some(format!("Failed to register producer: {}", e)),
        });
    }

    HttpResponse::Ok().json(ApiResponse {
        status: "success".to_string(),
        data: Some(producer),
        message: None,
    })
}

async fn register_consumer(
    req: web::Json<ConsumerRegisterRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let consumer = Consumer {
        id: Uuid::new_v4().to_string(),
        name: req.name.clone(),
        ty: req.ty.clone(),
        created_at: Utc::now().to_rfc3339(),
    };

    log::info!("Registering consumer: {} with producer_id: {:?}", consumer.id, req.producer_id);

    {
        let mut consumers = state.consumers.lock().await;
        consumers.push(consumer.clone());
    }

    if let Some(producer_id) = &req.producer_id {
        log::info!("Calling pipewire.register_consumer with consumer_id: {} and producer_id: {}", consumer.id, producer_id);
        if let Err(e) = state.pipewire.register_consumer(&consumer.id, producer_id).await {
            log::error!("Failed to register consumer: {}", e);
            return HttpResponse::InternalServerError().json(ApiResponse::<Consumer> {
                status: "error".to_string(),
                data: None,
                message: Some(format!("Failed to register consumer: {}", e)),
            });
        }
    }

    HttpResponse::Ok().json(ApiResponse {
        status: "success".to_string(),
        data: Some(consumer),
        message: None,
    })
}

async fn send_data(
    req: web::Json<DataSendRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let producer_id = req.producer_id.clone();
    let metadata = req.metadata.clone();

    let mut dma_buffers = state.dma_buffers.lock().await;
    let dma_buf = dma_buffers.get(&producer_id).cloned();

    if let Some(dma_buf) = dma_buf {
        match state.pipewire.send_data(&producer_id, dma_buf, &metadata).await {
            Ok(_) => HttpResponse::Ok().json(ApiResponse::<()> {
                status: "success".to_string(),
                data: None,
                message: Some("Data sent successfully".to_string()),
            }),
            Err(e) => HttpResponse::InternalServerError().json(ApiResponse::<()> {
                status: "error".to_string(),
                data: None,
                message: Some(e.to_string()),
            }),
        }
    } else {
        HttpResponse::BadRequest().json(ApiResponse::<()> {
            status: "error".to_string(),
            data: None,
            message: Some("No DMA-BUF found for producer. Create one first.".to_string()),
        })
    }
}

async fn create_dma_buf(
    req: web::Json<CreateDmaBufRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let format = match req.format.as_str() {
        "RGBA" | "rgbx" => vulkano::format::Format::R8G8B8A8_UNORM,
        "BGRA" | "bgrx" => vulkano::format::Format::B8G8R8A8_UNORM,
        _ => vulkano::format::Format::R8G8B8A8_UNORM,
    };

    if req.producer_id.is_none() {
        return HttpResponse::BadRequest().json(ApiResponse::<DmaBufResponse> {
            status: "error".to_string(),
            data: None,
            message: Some("producer_id is required".to_string()),
        });
    }

    let producer_id = req.producer_id.as_ref().unwrap();

    let mut vulkan_producer = state.vulkan_producer.lock().await;
    if vulkan_producer.is_none() {
        match VulkanDmaBufProducer::new() {
            Ok(producer) => {
                let external_support = producer.check_external_memory_support();
                log::info!("External memory support: {}", external_support);
                *vulkan_producer = Some(producer);
            }
            Err(e) => {
                return HttpResponse::InternalServerError().json(ApiResponse::<()> {
                    status: "error".to_string(),
                    data: None,
                    message: Some(format!("Failed to initialize Vulkan: {}", e)),
                });
            }
        }
    }

    let vulkan = vulkan_producer.as_ref().unwrap();

    match vulkan.create_image(req.width, req.height, format) {
        Ok(dma_buf) => {
            let response = DmaBufResponse {
                fd: dma_buf.as_raw_fd(),
                width: dma_buf.width,
                height: dma_buf.height,
                stride: dma_buf.stride,
                format: dma_buf.format,
                modifier: dma_buf.modifier,
            };

            let mut dma_buffers = state.dma_buffers.lock().await;
            log::info!("Storing DMA-BUF for producer_id: {}", producer_id);
            dma_buffers.insert(producer_id.clone(), Arc::new(Mutex::new(Some(dma_buf))));
            log::info!("DMA-BUF stored. Total buffers: {}", dma_buffers.len());

            HttpResponse::Ok().json(ApiResponse {
                status: "success".to_string(),
                data: Some(response),
                message: None,
            })
        }
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse::<DmaBufResponse> {
            status: "error".to_string(),
            data: None,
            message: Some(format!("Failed to create DMA-BUF: {}", e)),
        }),
    }
}

async fn fill_dma_buf(
    req: web::Json<FillDmaBufRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let producer_id = req.producer_id.clone();

    let mut vulkan_producer = state.vulkan_producer.lock().await;
    if let Some(vulkan) = &*vulkan_producer {
        let mut dma_buffers = state.dma_buffers.lock().await;

        log::info!("Looking for DMA-BUF for producer_id: {}", producer_id);
        log::info!("Total DMA-BUFs in storage: {}", dma_buffers.len());

        if let Some(dma_buf_mutex) = dma_buffers.get(&producer_id) {
            let mut dma_buf_opt = dma_buf_mutex.lock().await;
            if let Some(dma_buf) = dma_buf_opt.as_mut() {
                let _data = base64::decode(&req.data).unwrap_or_default();
                match vulkan.fill_image(&dma_buf.image, &_data) {
                    Ok(_) => HttpResponse::Ok().json(ApiResponse::<()> {
                        status: "success".to_string(),
                        data: None,
                        message: Some("DMA-BUF filled successfully".to_string()),
                    }),
                    Err(e) => HttpResponse::InternalServerError().json(ApiResponse::<()> {
                        status: "error".to_string(),
                        data: None,
                        message: Some(format!("Failed to fill DMA-BUF: {}", e)),
                    }),
                }
            } else {
                HttpResponse::BadRequest().json(ApiResponse::<()> {
                    status: "error".to_string(),
                    data: None,
                    message: Some("DMA-BUF not found".to_string()),
                })
            }
        } else {
            HttpResponse::BadRequest().json(ApiResponse::<()> {
                status: "error".to_string(),
                data: None,
                message: Some("No DMA-BUF found for producer".to_string()),
            })
        }
    } else {
        HttpResponse::InternalServerError().json(ApiResponse::<()> {
            status: "error".to_string(),
            data: None,
            message: Some("Vulkan not initialized".to_string()),
        })
    }
}

async fn receive_data(
    req: web::Query<DataReceiveRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let consumer_id = req.consumer_id.clone();
    let timeout = req.timeout.unwrap_or(5000);

    match state.pipewire.receive_data(&consumer_id, timeout).await {
        Ok((dma_buf_mutex, metadata)) => {
            let dma_buf_opt = dma_buf_mutex.lock().await;
            if let Some(dma_buf) = dma_buf_opt.as_ref() {
                let response = DmaBufResponse {
                    fd: dma_buf.as_raw_fd(),
                    width: dma_buf.width,
                    height: dma_buf.height,
                    stride: dma_buf.stride,
                    format: dma_buf.format,
                    modifier: dma_buf.modifier,
                };

                HttpResponse::Ok().json(ApiResponse {
                    status: "success".to_string(),
                    data: Some(serde_json::json!({
                        "dma_buf": response,
                        "metadata": metadata
                    })),
                    message: None,
                })
            } else {
                HttpResponse::InternalServerError().json(ApiResponse::<serde_json::Value> {
                    status: "error".to_string(),
                    data: None,
                    message: Some("DMA-BUF not available".to_string()),
                })
            }
        }
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse::<serde_json::Value> {
            status: "error".to_string(),
            data: None,
            message: Some(e.to_string()),
        }),
    }
}

async fn health_check() -> impl Responder {
    HttpResponse::Ok().json(ApiResponse::<serde_json::Value> {
        status: "success".to_string(),
        data: Some(serde_json::json!({
            "status": "healthy",
            "service": "waywallen"
        })),
        message: None,
    })
}

async fn get_pipewire_info(state: web::Data<AppState>) -> impl Responder {
    let info = state.pipewire.get_pipewire_info();
    HttpResponse::Ok().json(ApiResponse {
        status: "success".to_string(),
        data: Some(serde_json::json!(info)),
        message: None,
    })
}

async fn create_stream(
    req: web::Json<CreateStreamRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let stream_id = req.stream_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    
    let stream_manager = state.stream_manager.lock().await;
    
    match stream_manager.create_stream(
        stream_id.clone(),
        req.producer_id.clone(),
        req.width,
        req.height,
        req.format.clone(),
    ).await {
        Ok(stream_info) => {
            HttpResponse::Ok().json(ApiResponse {
                status: "success".to_string(),
                data: Some(stream_info),
                message: None,
            })
        }
        Err(e) => {
            HttpResponse::InternalServerError().json(ApiResponse::<dma_buf_stream::StreamInfo> {
                status: "error".to_string(),
                data: None,
                message: Some(e.to_string()),
            })
        }
    }
}

async fn push_frame(
    req: web::Json<StreamPushRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let stream_id = req.stream_id.clone();
    
    let vulkan_producer = state.stream_manager.lock().await.get_vulkan_producer();
    let vulkan = vulkan_producer.lock().await;
    
    if let Some(ref producer) = *vulkan {
        let format = match req.format.as_str() {
            "RGBA" | "rgbx" => vulkano::format::Format::R8G8B8A8_UNORM,
            "BGRA" | "bgrx" => vulkano::format::Format::B8G8R8A8_UNORM,
            _ => vulkano::format::Format::R8G8B8A8_UNORM,
        };
        
        match producer.create_image(req.width, req.height, format) {
            Ok(dma_buf) => {
                let dma_buf_arc = Arc::new(Mutex::new(Some(dma_buf)));
                
                drop(vulkan);
                
                let stream_manager = state.stream_manager.lock().await;
                if let Err(e) = stream_manager.push_frame(&stream_id, dma_buf_arc).await {
                    return HttpResponse::InternalServerError().json(ApiResponse::<()> {
                        status: "error".to_string(),
                        data: None,
                        message: Some(format!("Failed to push frame: {}", e)),
                    });
                }
                
                HttpResponse::Ok().json(ApiResponse::<()> {
                    status: "success".to_string(),
                    data: None,
                    message: Some("Frame pushed successfully".to_string()),
                })
            }
            Err(e) => {
                HttpResponse::InternalServerError().json(ApiResponse::<()> {
                    status: "error".to_string(),
                    data: None,
                    message: Some(format!("Failed to create DMA-BUF: {}", e)),
                })
            }
        }
    } else {
        HttpResponse::InternalServerError().json(ApiResponse::<()> {
            status: "error".to_string(),
            data: None,
            message: Some("Vulkan not initialized".to_string()),
        })
    }
}

async fn list_streams(state: web::Data<AppState>) -> impl Responder {
    let stream_manager = state.stream_manager.lock().await;
    let streams = stream_manager.list_streams().await;
    
    HttpResponse::Ok().json(ApiResponse {
        status: "success".to_string(),
        data: Some(serde_json::json!(streams)),
        message: None,
    })
}

async fn get_stream(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let stream_id = path.into_inner();
    let stream_manager = state.stream_manager.lock().await;
    
    match stream_manager.get_stream(&stream_id).await {
        Some(stream_info) => {
            HttpResponse::Ok().json(ApiResponse {
                status: "success".to_string(),
                data: Some(stream_info),
                message: None,
            })
        }
        None => {
            HttpResponse::NotFound().json(ApiResponse::<dma_buf_stream::StreamInfo> {
                status: "error".to_string(),
                data: None,
                message: Some(format!("Stream {} not found", stream_id)),
            })
        }
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();

    let pipewire_manager = pipewire::PipewireManager::new().await.unwrap();
    let stream_manager = DmaBufStreamManager::new();

    let state = web::Data::new(AppState {
        producers: Arc::new(Mutex::new(Vec::new())),
        consumers: Arc::new(Mutex::new(Vec::new())),
        pipewire: pipewire_manager,
        vulkan_producer: Arc::new(Mutex::new(None)),
        dma_buffers: Arc::new(Mutex::new(HashMap::new())),
        stream_manager: Arc::new(Mutex::new(stream_manager)),
        renderer_manager: Arc::new(renderer_manager::RendererManager::new()),
    });

    // Iteration 4: spawn the viewer endpoint task. It listens on a UDS
    // and streams DMA-BUF metadata + frame events to subscribed clients.
    {
        let mgr = state.renderer_manager.clone();
        let sock_path = viewer_endpoint::default_socket_path();
        tokio::spawn(async move {
            if let Err(e) = viewer_endpoint::serve(&sock_path, mgr).await {
                log::error!("viewer endpoint exited: {e}");
            }
        });
    }

    HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .service(
                web::scope("/api")
                    .service(
                        web::resource("/producer/register")
                            .route(web::post().to(register_producer))
                    )
                    .service(
                        web::resource("/consumer/register")
                            .route(web::post().to(register_consumer))
                    )
                    .service(
                        web::resource("/producer/send")
                            .route(web::post().to(send_data))
                    )
                    .service(
                        web::resource("/consumer/receive")
                            .route(web::get().to(receive_data))
                    )
                    .service(
                        web::resource("/dma-buf/create")
                            .route(web::post().to(create_dma_buf))
                    )
                    .service(
                        web::resource("/dma-buf/fill")
                            .route(web::post().to(fill_dma_buf))
                    )
                    .service(
                        web::resource("/health")
                            .route(web::get().to(health_check))
                    )
                    .service(
                        web::resource("/pipewire/info")
                            .route(web::get().to(get_pipewire_info))
                    )
                    .service(
                        web::resource("/stream/create")
                            .route(web::post().to(create_stream))
                    )
                    .service(
                        web::resource("/stream/push")
                            .route(web::post().to(push_frame))
                    )
                    .service(
                        web::resource("/stream/list")
                            .route(web::get().to(list_streams))
                    )
                    .service(
                        web::resource("/stream/{stream_id}")
                            .route(web::get().to(get_stream))
                    )
                    // Iteration 3 — renderer host management
                    .service(
                        web::resource("/renderer/spawn")
                            .route(web::post().to(renderer_spawn))
                    )
                    .service(
                        web::resource("/renderer/list")
                            .route(web::get().to(renderer_list))
                    )
                    .service(
                        web::resource("/renderer/{id}/play")
                            .route(web::post().to(renderer_play))
                    )
                    .service(
                        web::resource("/renderer/{id}/pause")
                            .route(web::post().to(renderer_pause))
                    )
                    .service(
                        web::resource("/renderer/{id}/mouse")
                            .route(web::post().to(renderer_mouse))
                    )
                    .service(
                        web::resource("/renderer/{id}/fps")
                            .route(web::post().to(renderer_set_fps))
                    )
                    .service(
                        web::resource("/renderer/{id}")
                            .route(web::delete().to(renderer_kill))
                    )
            )
    })
    .bind("0.0.0.0:8080")?
    .run()
    .await
}