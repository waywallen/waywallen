use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;

mod control;
mod control_proto;
mod dbus_iface;
mod display_endpoint;
mod display_proto;
mod display_spawner;
mod ipc;
mod model;
mod plugin;
mod renderer_manager;
mod routing;
mod scheduler;
mod settings;
mod tasks;
mod tray;
mod wallpaper_type;
mod ws_server;

/// Shared state handed to every ws connection.
pub struct AppState {
    pub renderer_manager: Arc<renderer_manager::RendererManager>,
    pub source_manager: Arc<tokio::sync::Mutex<plugin::source_manager::SourceManager>>,
    pub router: Arc<routing::Router>,
    pub settings: Arc<settings::SettingsStore>,
    pub db: sea_orm::DatabaseConnection,
    pub playlist: tokio::sync::Mutex<control::PlaylistState>,
    pub ws_port: std::sync::atomic::AtomicU16,
    pub ui_path: std::sync::Mutex<Option<PathBuf>>,
    /// Daemon-wide shutdown signal. Flips `false` → `true` exactly once.
    /// Every long-lived task (display endpoint, per-client loops, tray,
    /// ws server) should race its work against
    /// `shutdown.subscribe().wait_for(|v| *v)` so that a D-Bus `Quit`
    /// (or Ctrl-C) tears everything down without leaving blocking I/O
    /// parked in `recvmsg`.
    pub shutdown: tokio::sync::watch::Sender<bool>,
    /// Background task supervisor. Used to off-load startup scanning,
    /// DB sync, and similar work so `async_main` stays responsive.
    pub tasks: Arc<tasks::TaskManager>,
}

impl AppState {
    /// Flip the shutdown flag. Idempotent — safe to call from multiple
    /// places (DBus `Quit`, tray "Quit", Ctrl-C handler).
    pub fn shutdown_now(&self) {
        let _ = self.shutdown.send(true);
    }

    /// Subscribe for shutdown notification. Await with
    /// `rx.wait_for(|v| *v).await` — that returns immediately if we're
    /// already shutting down, otherwise parks until the flag flips.
    pub fn shutdown_subscribe(&self) -> tokio::sync::watch::Receiver<bool> {
        self.shutdown.subscribe()
    }
}

struct Args {
    ws_port: u16,
    ui_path: Option<PathBuf>,
    no_ui: bool,
    no_tray: bool,
    plugin_dirs: Vec<PathBuf>,
    /// Force a specific display backend by manifest `name`, bypassing
    /// DE auto-detection. Still subject to "exists in the registry".
    display_backend: Option<String>,
    /// Disable the daemon's display-backend auto-spawn entirely. The
    /// UDS endpoint still listens for external consumers (e.g. an
    /// already-installed waywallen-kde kpackage).
    no_display: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        ws_port: 0,
        ui_path: None,
        no_ui: false,
        no_tray: false,
        plugin_dirs: Vec::new(),
        display_backend: None,
        no_display: false,
    };

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--ws-port" => {
                let val = it.next().expect("--ws-port requires a value");
                args.ws_port = val.parse().expect("--ws-port must be a valid port number");
            }
            "--display-backend" => {
                let val = it.next().expect("--display-backend requires a name");
                args.display_backend = Some(val);
            }
            "--no-display" => {
                args.no_display = true;
            }
            "--ui" => {
                let val = it.next().expect("--ui requires a path");
                args.ui_path = Some(PathBuf::from(val));
            }
            "--no-ui" => {
                args.no_ui = true;
            }
            "--no-tray" => {
                args.no_tray = true;
            }
            "--plugin" => {
                let val = it.next().expect("--plugin requires a path");
                args.plugin_dirs.push(PathBuf::from(val));
            }
            other => {
                eprintln!("unknown argument: {other}");
                eprintln!("usage: waywallen [--ws-port PORT] [--ui PATH] [--no-ui] [--no-tray] [--plugin PATH]... [--display-backend NAME] [--no-display]");
                std::process::exit(1);
            }
        }
    }

    args
}

/// Spawn the `waywallen-ui` subprocess fire-and-forget. UI reads the WS
/// port from the `org.waywallen.waywallen.Daemon1` DBus interface; its lifecycle is
/// independent of the daemon.
pub fn spawn_ui(state: &AppState) -> bool {
    let ui_bin = match state.ui_path.lock().unwrap().clone() {
        Some(p) => p,
        None => return false,
    };
    log::info!("launching ui: {}", ui_bin.display());
    match std::process::Command::new(&ui_bin).spawn() {
        Ok(child) => {
            log::info!("ui pid: {}", child.id());
            true
        }
        Err(e) => {
            log::warn!("failed to launch ui {}: {e}", ui_bin.display());
            false
        }
    }
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

fn main() -> anyhow::Result<()> {
    env_logger::init();

    // Explicit runtime + `shutdown_timeout` safety net: if any
    // `spawn_blocking` task is still parked in a syscall when the
    // runtime is torn down (e.g. a display-client reader stuck in
    // `recvmsg` because its client never sent anything and didn't
    // drop the socket), we give it a bounded window to unwind and
    // then drop the runtime anyway instead of hanging the process.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = rt.block_on(async_main());
    rt.shutdown_timeout(std::time::Duration::from_secs(3));
    result
}

async fn async_main() -> anyhow::Result<()> {
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

    // Source management: create an empty manager now, defer loading
    // the Lua plugins + scanning their directories to a background
    // task. A cold scan over a large image library is easily seconds
    // of synchronous filesystem work; keeping it on the startup hot
    // path means UDS/WS/DBus/layer-shell spawn all wait on it.
    let source_mgr = Arc::new(tokio::sync::Mutex::new(
        plugin::source_manager::SourceManager::new(std::collections::HashMap::new())
            .expect("failed to create source manager"),
    ));

    let renderer_mgr = Arc::new(renderer_manager::RendererManager::new(registry));
    let router = routing::Router::new(renderer_mgr.clone());
    let settings_store =
        settings::SettingsStore::load_or_default(settings::default_config_path()).await;
    let db_path = settings::default_db_path();
    let db = model::connect(&db_path)
        .await
        .with_context(|| format!("open database {}", db_path.display()))?;

    let (shutdown_tx, shutdown_rx_for_tasks) = tokio::sync::watch::channel(false);
    let task_mgr = tasks::TaskManager::spawn(shutdown_rx_for_tasks);

    let state = Arc::new(AppState {
        renderer_manager: renderer_mgr,
        source_manager: source_mgr.clone(),
        router: router.clone(),
        settings: settings_store,
        db: db.clone(),
        playlist: tokio::sync::Mutex::new(control::PlaylistState::default()),
        ws_port: std::sync::atomic::AtomicU16::new(0),
        ui_path: std::sync::Mutex::new(None),
        shutdown: shutdown_tx,
        tasks: task_mgr.clone(),
    });

    // Off-load source-plugin loading + scanning + DB sync + initial
    // playlist seed onto the TaskManager. `async_main` proceeds
    // immediately to bind UDS/WS/DBus; the UI will see an empty
    // playlist until the task completes and populates it.
    {
        let source_mgr = source_mgr.clone();
        let db_for_task = db.clone();
        let plugin_dirs = cli.plugin_dirs.clone();
        let state_for_task = state.clone();
        state.tasks.spawn_async("startup/sources", async move {
            // Step 1 — load + scan. The Lua plugins run synchronous
            // filesystem globs and stat(2) in large loops, so this
            // part goes to the blocking pool.
            let source_mgr_blocking = source_mgr.clone();
            let snapshot: Vec<crate::wallpaper_type::WallpaperEntry> =
                tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
                    let mut sm = source_mgr_blocking.blocking_lock();
                    for dir in plugin::renderer_registry::standard_plugin_dirs("sources") {
                        if dir.is_dir() {
                            if let Err(e) = sm.load_all(&dir) {
                                log::warn!("load sources {}: {e:#}", dir.display());
                            }
                        }
                    }
                    for plugin_dir in &plugin_dirs {
                        let sources_dir = plugin_dir.join("sources");
                        if sources_dir.is_dir() {
                            if let Err(e) = sm.load_all(&sources_dir) {
                                log::warn!("load sources {}: {e:#}", sources_dir.display());
                            }
                        }
                    }
                    sm.scan_all()?;
                    Ok(sm.list().to_vec())
                })
                .await
                .map_err(|e| anyhow::anyhow!("source scan join: {e}"))??;

            // Step 2 — persist scan to DB (async, needs the runtime).
            let plugins = {
                let sm = source_mgr.lock().await;
                match sm.plugins() {
                    Ok(p) => p,
                    Err(e) => {
                        log::warn!("list plugins for sync failed: {e:#}");
                        Vec::new()
                    }
                }
            };
            for info in &plugins {
                let entries: Vec<_> = snapshot
                    .iter()
                    .filter(|e| e.plugin_name == info.name)
                    .cloned()
                    .collect();
                match model::sync::sync_plugin_entries(
                    &db_for_task,
                    model::sync::PluginRef {
                        name: &info.name,
                        version: &info.version,
                    },
                    &entries,
                )
                .await
                {
                    Ok(summary) => log::info!(
                        "sync plugin={} v{}: +{} / -{} items, -{} libraries, {} dropped",
                        info.name,
                        info.version,
                        summary.items_upserted,
                        summary.items_deleted,
                        summary.libraries_deleted,
                        summary.dropped,
                    ),
                    Err(e) => log::warn!("sync plugin={} failed: {e:#}", info.name),
                }
            }

            // Step 3 — seed the playlist from the fresh snapshot.
            let ids: Vec<String> = snapshot.iter().map(|e| e.id.clone()).collect();
            state_for_task.playlist.lock().await.refresh(ids);
            Ok(())
        });
    }

    // Display backend registry + auto-selection. This does not spawn the
    // backend yet (that lands in a follow-up); for now we log the pick
    // so `journalctl -u waywallen` shows what would have launched.
    let mut display_registry = plugin::display_registry::build_default_registry()
        .unwrap_or_else(|e| {
            log::warn!("display registry init failed: {e:#}");
            plugin::display_registry::DisplayRegistry::new()
        });
    for plugin_dir in &cli.plugin_dirs {
        let displays_dir = plugin_dir.join("displays");
        if displays_dir.is_dir() {
            match plugin::display_registry::DisplayRegistry::scan(&displays_dir) {
                Ok(scanned) => {
                    for def in scanned.all() {
                        display_registry.register(def.clone());
                    }
                }
                Err(e) => log::warn!("scan {}: {e}", displays_dir.display()),
            }
        }
    }
    let display_caps = display_spawner::detect_de();
    // Decide which backend to run. We clone the DisplayDef out of the
    // registry so the spawn task can outlive the borrow.
    let display_backend: Option<plugin::display_registry::DisplayDef> = if cli.no_display {
        log::info!("--no-display: skipping display backend selection");
        None
    } else {
        let pick = if let Some(name) = cli.display_backend.as_deref() {
            match display_registry.find(name) {
                Some(def) => {
                    log::info!("display backend pinned by --display-backend: {name}");
                    display_spawner::PickOutcome::Matched(def)
                }
                None => {
                    log::error!(
                        "--display-backend {name} not found in registry; falling back to auto-detect"
                    );
                    display_spawner::pick_backend(&display_registry, &display_caps)
                }
            }
        } else {
            display_spawner::pick_backend(&display_registry, &display_caps)
        };
        display_spawner::log_outcome(&pick, &display_caps);
        let should_spawn = display_spawner::should_daemon_spawn(&pick);
        match pick {
            display_spawner::PickOutcome::KdeHardMatch(def)
            | display_spawner::PickOutcome::Matched(def)
                if should_spawn =>
            {
                Some(def.clone())
            }
            _ => None,
        }
    };

    // Display endpoint on UDS (waywallen-display-v1 protocol).
    let display_sock_path = display_endpoint::default_socket_path();
    {
        let router = router.clone();
        let sock_path = display_sock_path.clone();
        let shutdown_rx = state.shutdown_subscribe();
        tokio::spawn(async move {
            if let Err(e) =
                display_endpoint::serve_with_shutdown(&sock_path, router, shutdown_rx).await
            {
                log::error!("display endpoint exited: {e}");
            }
        });
    }

    // Daemon-managed display backend (layer-shell etc.). Started after
    // the UDS endpoint is bound so the child's first connect succeeds.
    if let Some(def) = display_backend {
        let sock_path = display_sock_path.clone();
        let shutdown_rx = state.shutdown_subscribe();
        let name = def.name.clone();
        tokio::spawn(async move {
            if let Err(e) = display_spawner::run_backend(def, sock_path, shutdown_rx).await {
                log::error!("display backend '{name}' supervisor exited: {e:#}");
            }
        });
    }

    // Bind the WS control plane (port 0 = OS picks an available port).
    let bind_addr = format!("0.0.0.0:{}", cli.ws_port);
    let (local_addr, ws_fut) = ws_server::bind(state.clone(), &bind_addr).await?;
    let ws_port = local_addr.port();
    state
        .ws_port
        .store(ws_port, std::sync::atomic::Ordering::SeqCst);
    log::info!("ws port: {ws_port}");

    // Resolve + stash the UI path, and launch once at startup (unless --no-ui).
    if !cli.no_ui {
        if let Some(ui_bin) = resolve_ui_path(cli.ui_path) {
            *state.ui_path.lock().unwrap() = Some(ui_bin);
            spawn_ui(&state);
        } else {
            log::info!("waywallen-ui not found, running headless");
        }
    }

    // Session-bus presence: publish org.waywallen.waywallen.Daemon so consumers can
    // detect daemon (re)start via NameOwnerChanged / Ready and reconnect
    // immediately instead of waiting for their local backoff. Optional —
    // if the session bus is absent (e.g. TTY, embedded) we keep running.
    let dbus_conn = match dbus_iface::connect(
        state.clone(),
        display_sock_path.to_string_lossy().into_owned(),
    )
    .await
    {
        Ok(conn) => {
            log::info!("DBus name acquired: {}", dbus_iface::BUS_NAME);
            if let Err(e) = dbus_iface::emit_ready(&conn).await {
                log::warn!("DBus Ready emit failed: {e}");
            }
            Some(conn)
        }
        Err(e) => {
            log::warn!("DBus session bus unavailable: {e} (continuing headless)");
            None
        }
    };

    // Tray icon (StatusNotifierItem) — best-effort. Requires a session bus
    // and a StatusNotifierWatcher (Plasma, AppIndicator extension, waybar
    // tray, ...). No host ⇒ warn & keep running headless.
    if !cli.no_tray {
        if let Some(conn) = dbus_conn.clone() {
            let state_t = state.clone();
            tokio::spawn(async move {
                if let Err(e) = tray::spawn(conn, state_t).await {
                    log::warn!("tray: {e} (continuing without tray)");
                }
            });
        }
    }

    tokio::select! {
        res = ws_fut => {
            if let Err(e) = res {
                log::error!("ws server exited with error: {e}");
            }
        }
        _ = tokio::signal::ctrl_c() => {
            log::info!("SIGINT received, shutting down");
        }
        _ = async {
            let mut rx = state.shutdown_subscribe();
            let _ = rx.wait_for(|v| *v).await;
        } => {
            log::info!("shutdown requested via D-Bus");
        }
    }

    // Belt-and-suspenders: regardless of which arm woke us (ws exit,
    // ctrl-c, D-Bus Quit) make sure every subscriber sees the shutdown
    // flag. This is what lets the display endpoint's blocking reader
    // threads be kicked out of `recvmsg`.
    state.shutdown_now();

    if let Some(conn) = dbus_conn.as_ref() {
        if let Err(e) = dbus_iface::emit_shutting_down(conn).await {
            log::warn!("DBus ShuttingDown emit failed: {e}");
        }
    }
    drop(dbus_conn);

    Ok(())
}
