use std::path::PathBuf;
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

struct Args {
    ws_port: u16,
    ui_path: Option<PathBuf>,
    no_ui: bool,
    plugin_dirs: Vec<PathBuf>,
}

fn parse_args() -> Args {
    let mut args = Args {
        ws_port: 0,
        ui_path: None,
        no_ui: false,
        plugin_dirs: Vec::new(),
    };

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--ws-port" => {
                let val = it.next().expect("--ws-port requires a value");
                args.ws_port = val.parse().expect("--ws-port must be a valid port number");
            }
            "--ui" => {
                let val = it.next().expect("--ui requires a path");
                args.ui_path = Some(PathBuf::from(val));
            }
            "--no-ui" => {
                args.no_ui = true;
            }
            "--plugin" => {
                let val = it.next().expect("--plugin requires a path");
                args.plugin_dirs.push(PathBuf::from(val));
            }
            other => {
                eprintln!("unknown argument: {other}");
                eprintln!("usage: waywallen [--ws-port PORT] [--ui PATH] [--no-ui] [--plugin PATH]...");
                std::process::exit(1);
            }
        }
    }

    args
}

/// Resolve the UI executable path.  Order:
/// 1. Explicit `--ui PATH`
/// 2. `waywallen-ui` next to the current executable
fn resolve_ui_path(explicit: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.parent()?.join("waywallen-ui");
        if sibling.exists() {
            return Some(sibling);
        }
    }
    None
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    let cli = parse_args();

    let mut registry = plugin::renderer_registry::build_default_registry()
        .expect("failed to build renderer registry");

    // Scan extra --plugin directories for renderer manifests.
    for plugin_dir in &cli.plugin_dirs {
        let renderers_dir = plugin_dir.join("renderers");
        if renderers_dir.is_dir() {
            match plugin::renderer_registry::RendererRegistry::scan(&renderers_dir) {
                Ok(scanned) => {
                    for def in scanned.all_renderers() {
                        registry.register(def.clone());
                    }
                }
                Err(e) => log::warn!("scan {}: {e}", renderers_dir.display()),
            }
        }
    }

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
    // Scan extra --plugin directories for source plugins.
    for plugin_dir in &cli.plugin_dirs {
        let sources_dir = plugin_dir.join("sources");
        if sources_dir.is_dir() {
            let _ = source_mgr.load_all(&sources_dir);
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

    // Bind the WS control plane (port 0 = OS picks an available port).
    let bind_addr = format!("0.0.0.0:{}", cli.ws_port);
    let (local_addr, ws_fut) = ws_server::bind(state, &bind_addr).await?;
    let ws_port = local_addr.port();
    log::info!("ws port: {ws_port}");

    // Spawn the UI subprocess.
    // Keep the Child handle alive so the process is killed when the daemon exits.
    let _ui_child = if !cli.no_ui {
        if let Some(ui_bin) = resolve_ui_path(cli.ui_path) {
            log::info!("launching ui: {} --ws-port {ws_port}", ui_bin.display());
            match std::process::Command::new(&ui_bin)
                .arg("--ws-port")
                .arg(ws_port.to_string())
                .spawn()
            {
                Ok(child) => {
                    log::info!("ui pid: {}", child.id());
                    Some(child)
                }
                Err(e) => {
                    log::warn!("failed to launch ui {}: {e}", ui_bin.display());
                    None
                }
            }
        } else {
            log::info!("waywallen-ui not found, running headless");
            None
        }
    } else {
        None
    };

    ws_fut.await?;
    Ok(())
}
