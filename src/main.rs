use std::sync::Arc;

mod control_proto;
mod display_endpoint;
mod display_proto;
mod dummy_fence;
mod ipc;
mod plugin;
mod renderer_manager;
mod scheduler;
mod wallpaper_type;
mod ws_server;

/// Shared state handed to every ws connection.
pub struct AppState {
    pub renderer_manager: Arc<renderer_manager::RendererManager>,
    pub source_manager: Arc<tokio::sync::Mutex<plugin::source_manager::SourceManager>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let registry = plugin::renderer_registry::build_default_registry()
        .expect("failed to build renderer registry");

    // Source management: load Lua plugins from the canonical paths
    // (bundled `<exec>/../share/waywallen/sources/` first, then user-local
    // `$XDG_DATA_HOME/waywallen/sources/` — user plugins can shadow bundled
    // ones by name).
    let mut source_mgr =
        plugin::source_manager::SourceManager::new(std::collections::HashMap::new())
            .expect("failed to create source manager");
    for dir in plugin::renderer_registry::standard_plugin_dirs("sources") {
        if dir.is_dir() {
            let _ = source_mgr.load_all(&dir);
        }
    }
    let _ = source_mgr.scan_all();

    let state = Arc::new(AppState {
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

    ws_server::serve(state, "0.0.0.0:8080").await?;
    Ok(())
}
