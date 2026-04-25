use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;

use media_probe::{AvFormatProbe, MediaProbe};

mod control;
mod control_proto;
mod dbus_iface;
mod display_endpoint;
mod display_proto;
mod display_spawner;
mod events;
mod ipc;
mod media_probe;
mod model;
mod playlist;
mod plugin;
mod probe_task;
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
    /// Read-only mirror of the latest scan results. Updated atomically
    /// by `control::refresh_sources` after the Lua scan finishes; read
    /// by `WallpaperList`/`WallpaperApply`/`SourceList` so those handlers
    /// don't contend with an in-flight scan on `source_manager`.
    pub source_snapshot: Arc<tokio::sync::RwLock<plugin::source_snapshot::SourceSnapshot>>,
    pub router: Arc<routing::Router>,
    pub settings: Arc<settings::SettingsStore>,
    pub db: sea_orm::DatabaseConnection,
    pub playlist: tokio::sync::Mutex<control::PlaylistState>,
    /// Auto-rotation control handle. The rotator task watches the
    /// matching `watch::Receiver` and re-arms its deadline on every
    /// edit (interval change OR a manual `kick`).
    pub rotation: playlist::RotationHandle,
    /// Process-wide event bus. Carries phase markers (sources ready,
    /// display ready) the boot coordinator gates on, plus transient
    /// notifications about restore success/failure.
    pub events: events::EventBus,
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
    /// Shared media probe. Constructed once at startup; reused by both
    /// SourceManager and the sync layer so dlopen happens at most once.
    pub probe: Arc<dyn MediaProbe>,
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
    /// Restore the last applied wallpaper on startup.
    restore_last: bool,
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
        restore_last: true,
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
            "--no-restore" => {
                args.restore_last = false;
            }
            other => {
                eprintln!("unknown argument: {other}");
                eprintln!("usage: waywallen [--ws-port PORT] [--ui PATH] [--no-ui] [--no-tray] [--plugin PATH]... [--display-backend NAME] [--no-display] [--no-restore]");
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

    // Shared media probe — constructed once, reused by SourceManager
    // and the sync layer so libavformat is dlopen-ed at most once.
    let probe = Arc::new(AvFormatProbe::new()) as Arc<dyn MediaProbe>;

    // Source management: create an empty manager now, defer loading
    // the Lua plugins + scanning their directories to a background
    // task. A cold scan over a large image library is easily seconds
    // of synchronous filesystem work; keeping it on the startup hot
    // path means UDS/WS/DBus/layer-shell spawn all wait on it.
    let source_mgr = Arc::new(tokio::sync::Mutex::new(
        plugin::source_manager::SourceManager::with_probe(probe.clone())
            .expect("failed to create source manager"),
    ));

    let renderer_mgr = Arc::new(renderer_manager::RendererManager::new(registry));
    let router = routing::Router::new(renderer_mgr.clone());
    renderer_mgr.attach_router(Arc::downgrade(&router));
    renderer_mgr.start_reaper();
    let settings_store =
        settings::SettingsStore::load_or_default(settings::default_config_path()).await;
    let db_path = settings::default_db_path();
    let db = model::connect(&db_path)
        .await
        .with_context(|| format!("open database {}", db_path.display()))?;

    let (shutdown_tx, shutdown_rx_for_tasks) = tokio::sync::watch::channel(false);
    let task_mgr = tasks::TaskManager::spawn(shutdown_rx_for_tasks);

    let (rotation_handle, rotation_rx) = playlist::rotator::make_handle();

    let source_snapshot = Arc::new(tokio::sync::RwLock::new(
        plugin::source_snapshot::SourceSnapshot::default(),
    ));

    let state = Arc::new(AppState {
        renderer_manager: renderer_mgr,
        source_manager: source_mgr.clone(),
        source_snapshot,
        router: router.clone(),
        settings: settings_store,
        db: db.clone(),
        playlist: tokio::sync::Mutex::new(control::PlaylistState::default()),
        rotation: rotation_handle,
        events: events::EventBus::default(),
        ws_port: std::sync::atomic::AtomicU16::new(0),
        ui_path: std::sync::Mutex::new(None),
        shutdown: shutdown_tx,
        tasks: task_mgr.clone(),
        probe: probe.clone(),
    });

    // Auto-rotation service. Runs forever (or until shutdown), parked
    // on a watch channel until the user activates a playlist with a
    // non-zero `interval_secs` or kicks it via Next/Previous.
    {
        let app_for_rot = state.clone();
        let shutdown_for_rot = state.shutdown_subscribe();
        state.tasks.spawn_async(
            tasks::TaskKind::Service,
            "playlist/rotator",
            async move {
                control::run_rotator(app_for_rot, rotation_rx, shutdown_for_rot).await;
                Ok(())
            },
        );
    }

    // Display infrastructure first. The UDS endpoint and (if applicable)
    // the daemon-managed display backend subprocess are queued *before*
    // any source-side work so they hit the runtime as early as
    // possible — display registration must not wait on the Lua scan.
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
    let display_backend: Option<plugin::display_registry::DisplayDef> = if cli.no_display {
        log::info!("--no-display: skipping display backend selection");
        None
    } else {
        let pick = if let Some(name) = cli.display_backend.as_deref() {
            match display_registry.find(name) {
                Some(def) => {
                    log::info!("display backend pinned by --display-backend: {name}");
                    display_spawner::PickOutcome::Matched(def.clone())
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
                Some(def)
            }
            _ => None,
        }
    };

    let display_sock_path = display_endpoint::default_socket_path();
    {
        let router = router.clone();
        let sock_path = display_sock_path.clone();
        let shutdown_rx = state.shutdown_subscribe();
        state.tasks.spawn_async(
            tasks::TaskKind::Service,
            "display/endpoint",
            async move {
                display_endpoint::serve_with_shutdown(&sock_path, router, shutdown_rx)
                    .await
                    .map_err(|e| anyhow::anyhow!("display endpoint exited: {e}"))
            },
        );
    }
    if let Some(def) = display_backend {
        let sock_path = display_sock_path.clone();
        let shutdown_rx = state.shutdown_subscribe();
        let name = def.name.clone();
        state.tasks.spawn_async(
            tasks::TaskKind::Service,
            format!("display/backend/{name}"),
            async move {
                display_spawner::run_backend(def, sock_path, shutdown_rx)
                    .await
                    .map_err(|e| anyhow::anyhow!("display backend supervisor exited: {e}"))
            },
        );
    }

    // Off-load source-plugin loading + scanning + DB sync + initial
    // playlist seed onto the TaskManager. `async_main` proceeds
    // immediately to bind UDS/WS/DBus; the UI will see an empty
    // playlist until the task completes and populates it. Display
    // registration runs in parallel — it does not gate on this task.
    {
        let source_mgr = source_mgr.clone();
        let plugin_dirs = cli.plugin_dirs.clone();
        let state_for_task = state.clone();
        state.tasks.spawn_async(tasks::TaskKind::Startup, "startup/sources", async move {
            // Step 1 — load Lua plugins off the blocking pool.
            tokio::task::spawn_blocking(move || {
                let mut sm = source_mgr.blocking_lock();
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
            })
            .await
            .map_err(|e| anyhow::anyhow!("plugin load join: {e}"))?;

            // Step 2 — scan against DB-driven libraries + sync results
            // + seed the playlist.
            if let Err(e) = control::refresh_sources(&state_for_task).await {
                log::warn!("initial source refresh failed: {e:#}");
            }

            // Sources + initial DB sync done. Publish the latched
            // phase marker; the restore coordinator (separate task)
            // observes this and the matching DisplayReady marker
            // before kicking off the actual restore.
            state_for_task.events.publish(events::GlobalEvent::SourcesReady);
            Ok(())
        });
    }

    // Display watcher: bridge from `Router` events to the global
    // event bus. Fires `DisplayReady` exactly once, on the first
    // display registration. Runs forever (kept simple) but is a
    // no-op after the latch is set.
    {
        let watcher_state = state.clone();
        state.tasks.spawn_async(
            tasks::TaskKind::Service,
            "boot/display-watcher",
            async move {
                if !watcher_state.router.snapshot_displays().await.is_empty() {
                    watcher_state.events.publish(events::GlobalEvent::DisplayReady);
                    return Ok(());
                }
                let mut events_rx = watcher_state.router.subscribe_events();
                loop {
                    match events_rx.recv().await {
                        Ok(routing::RouterEvent::DisplayUpsert(_)) => {
                            watcher_state
                                .events
                                .publish(events::GlobalEvent::DisplayReady);
                            return Ok(());
                        }
                        Ok(routing::RouterEvent::DisplaysReplace(list))
                            if !list.is_empty() =>
                        {
                            watcher_state
                                .events
                                .publish(events::GlobalEvent::DisplayReady);
                            return Ok(());
                        }
                        Ok(_) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            // Re-snapshot in case we missed the upsert
                            // while lagged.
                            if !watcher_state.router.snapshot_displays().await.is_empty() {
                                watcher_state
                                    .events
                                    .publish(events::GlobalEvent::DisplayReady);
                                return Ok(());
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            return Ok(());
                        }
                    }
                }
            },
        );
    }

    // Restore coordinator: gate ONLY on display readiness. The scan
    // is a pure background task and must not delay restore. If the
    // saved wallpaper id is missing from the (still-empty) in-memory
    // snapshot, restore logs and bails — by definition that item is
    // stale and can stay un-restored until the user picks again.
    {
        let coord_state = state.clone();
        let restore_last = cli.restore_last;
        state.tasks.spawn_async(
            tasks::TaskKind::Service,
            "boot/restore-coordinator",
            async move {
                let mut display_rx = coord_state.events.watch_display_ready();
                let _ = display_rx.wait_for(|v| *v).await;
                log::info!(
                    "restore coordinator: display registered, settling 2s before restore"
                );
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;

                let restore_state = coord_state.clone();
                coord_state.tasks.spawn_async(
                    tasks::TaskKind::Startup,
                    "startup/restore",
                    async move { control::run_restore(&restore_state, restore_last).await },
                );
                Ok(())
            },
        );
    }

    // Background media-probe scheduler. Pulls items with NULL media
    // metadata + probable extension out of the DB on a tick and fills
    // them in via libavformat. Decoupled from scan/sync so adding a
    // big library doesn't stall the source refresh path.
    {
        let probe_for_task = probe.clone();
        let db_for_task = db.clone();
        let shutdown_for_task = state.shutdown.subscribe();
        state.tasks.spawn_async(
            tasks::TaskKind::Service,
            "probe/scheduler",
            async move {
                probe_task::scheduler_loop(db_for_task, probe_for_task, shutdown_for_task).await
            },
        );
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

    // SIGTERM (default `kill <pid>`, systemd stop) needs an explicit
    // listener — `tokio::signal::ctrl_c()` only catches SIGINT.
    // Without this branch the runtime tears down abruptly and the
    // settings debounced-writer task is dropped mid-sleep, losing any
    // pending `last_wallpaper` / `active_playlist_id` updates.
    let mut sigterm = tokio::signal::unix::signal(
        tokio::signal::unix::SignalKind::terminate(),
    )?;

    tokio::select! {
        res = ws_fut => {
            if let Err(e) = res {
                log::error!("ws server exited with error: {e}");
            }
        }
        _ = tokio::signal::ctrl_c() => {
            log::info!("SIGINT received, shutting down");
        }
        _ = sigterm.recv() => {
            log::info!("SIGTERM received, shutting down");
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

    // Flush settings synchronously so the in-flight debounced write
    // (last_wallpaper / rotation_secs / active_playlist_id /
    // playlist_mode set within the last DEBOUNCE_WRITE seconds) lands
    // on disk before the runtime tears down. Without this, a SIGTERM
    // that arrives shortly after a setting change loses the change
    // and the next daemon start can't restore playback.
    state.settings.flush_now().await;

    if let Some(conn) = dbus_conn.as_ref() {
        if let Err(e) = dbus_iface::emit_shutting_down(conn).await {
            log::warn!("DBus ShuttingDown emit failed: {e}");
        }
    }
    drop(dbus_conn);

    Ok(())
}
