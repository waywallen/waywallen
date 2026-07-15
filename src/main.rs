use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;

use probe::media::{AvFormatProbe, MediaProbe};

mod autostart;
mod control;
mod control_proto;
mod dbus_iface;
mod display;
mod dma;
mod error;
mod event_process;
mod events;
mod gpu;
mod ipc;
mod model;
mod mpris;
mod notifications;
pub mod playlist;
mod plugin;
mod probe;
mod queue;
mod renderer_manager;
mod routing;
mod scheduler;
mod session_monitor;
mod settings;
mod sync;
mod tasks;
mod tray;
mod wallpaper {
    pub mod properties;
    pub mod sort;
    pub mod types;
}
mod ws_server;

/// Shared state handed to every ws connection.
pub struct AppState {
    pub renderer_manager: Arc<renderer_manager::RendererManager>,
    pub source_manager: Arc<tokio::sync::Mutex<plugin::source_manager::SourceManager>>,
    /// Active installable-plugin metadata from the startup scan.
    pub plugins: Arc<tokio::sync::RwLock<Vec<plugin::renderer_registry::PluginPackageMeta>>>,
    pub inactive_system: Arc<tokio::sync::RwLock<Vec<String>>>,
    pub inactive_user: Arc<tokio::sync::RwLock<Vec<String>>>,
    pub plugin_updates: plugin::update::PluginUpdateStore,
    pub plugin_update_check: tokio::sync::Mutex<()>,
    /// Plugin scan roots reused when an explicit install changes plugin files.
    pub plugin_roots: Arc<Vec<plugin::renderer_registry::PluginRoot>>,
    /// The installed source plugins (types/labels/hints). The only
    /// scan-derived state outside the DB for the Add-Library UI.
    pub source_plugins: Arc<tokio::sync::RwLock<Vec<plugin::source_manager::SourcePluginInfo>>>,
    pub plugin_mutation: tokio::sync::Mutex<()>,
    pub autostart: autostart::AutostartService,
    pub router: Arc<routing::Router>,
    pub display_backend_status: std::sync::RwLock<display::spawner::DisplayBackendStatus>,
    pub settings: Arc<settings::SettingsStore>,
    /// Snapshot of `/dev/dri` taken at startup. Read-only after construction;
    /// surfaced to UI and used by RendererManager spawn resolution.
    pub gpus: Arc<Vec<gpu::GpuInfo>>,
    pub db: sea_orm::DatabaseConnection,
    pub queue: tokio::sync::Mutex<control::QueueState>,
    /// Auto-rotation control handle. The rotator task watches the
    /// matching receiver and re-arms its deadline on config changes.
    pub rotation: queue::RotationHandle,
    /// Process-wide event bus.
    /// Carries readiness markers and transient status events.
    pub events: events::EventBus,
    pub ws_port: std::sync::atomic::AtomicU16,
    /// True while `control::refresh_sources` is between `ScanStarted`
    /// and completion. Snapshotted into status events.
    pub scan_in_progress: std::sync::atomic::AtomicBool,
    pub ui_path: std::sync::Mutex<Option<PathBuf>>,
    /// Live DBus connection. Populated by `dbus_iface::serve` once the
    /// Daemon1 interface is published for property notifications.
    pub dbus_conn: std::sync::Mutex<Option<Arc<zbus::Connection>>>,
    /// Daemon-wide shutdown signal. Flips `false` → `true` exactly once.
    /// Long-lived tasks subscribe and exit cooperatively.
    pub shutdown: tokio::sync::watch::Sender<bool>,
    /// Background task supervisor. Used to off-load startup scanning,
    /// DB sync, and similar work so `async_main` stays responsive.
    pub tasks: Arc<tasks::TaskManager>,
    /// Shared media probe. Constructed once at startup; reused by both
    /// SourceManager and the sync layer so dlopen happens at most once.
    pub probe: Arc<dyn MediaProbe>,
    pub playlists: playlist::engine::Engine,
    /// `--no-tray` CLI override. Forces the tray off and makes the
    /// `hide_tray_icon` live toggle a no-op for this run.
    pub no_tray: bool,
    /// Live tray handle; `Some` while the tray icon is registered.
    pub tray: tokio::sync::Mutex<Option<tray::TrayHandle>>,
}

impl AppState {
    /// Flip the shutdown flag. Idempotent — safe to call from multiple
    /// places (DBus `Quit`, tray "Quit", Ctrl-C handler).
    pub fn shutdown_now(&self) {
        let _ = self.shutdown.send(true);
    }

    /// Subscribe for shutdown notification.
    /// `rx.wait_for(|v| *v).await` returns immediately once set.
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
    /// Disable daemon-managed display backend auto-spawn.
    /// The UDS endpoint still listens for external consumers.
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

/// Spawn the `waywallen-ui` subprocess fire-and-forget.
/// The UI reads the WS port from the Daemon1 DBus interface.
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

/// Auto-pick (or pin) and preflight a display backend for `caps`.
/// Returns the status to surface plus the def to spawn when the daemon
/// owns the backend process.
fn select_display_backend(
    registry: &plugin::display_registry::DisplayRegistry,
    caps: &display::spawner::DeCaps,
    pinned: Option<&str>,
) -> (
    display::spawner::DisplayBackendStatus,
    Option<plugin::display_registry::DisplayDef>,
) {
    let pick = if let Some(name) = pinned {
        match registry.find(name) {
            Some(def) => {
                log::info!("display backend pinned by --display-backend: {name}");
                display::spawner::PickOutcome::Matched(def.clone())
            }
            None => {
                log::error!(
                    "--display-backend {name} not found in registry; falling back to auto-detect"
                );
                display::spawner::pick_backend(registry, caps)
            }
        }
    } else {
        display::spawner::pick_backend(registry, caps)
    };
    display::spawner::log_outcome(&pick, caps);
    let should_spawn = display::spawner::should_daemon_spawn(&pick);
    match pick {
        display::spawner::PickOutcome::KdeHardMatch(def)
        | display::spawner::PickOutcome::Matched(def)
            if should_spawn =>
        {
            let (status, backend) = display::spawner::preflight_daemon_backend(def, caps);
            if status.state == display::spawner::DISPLAY_BACKEND_STATE_BINARY_MISSING
                || status.state == display::spawner::DISPLAY_BACKEND_STATE_FLATPAK_RESTRICTED
            {
                log::error!("{}", status.reason);
            }
            (status, backend)
        }
        display::spawner::PickOutcome::KdeHardMatch(def)
        | display::spawner::PickOutcome::Matched(def) => (
            display::spawner::DisplayBackendStatus::external(&def, caps),
            None,
        ),
        display::spawner::PickOutcome::None => (
            display::spawner::DisplayBackendStatus::unmatched(caps),
            None,
        ),
    }
}

/// Early-boot backend recovery. Polls the session (systemd user env +
/// on-disk Wayland sockets) until `XDG_CURRENT_DESKTOP` appears, then
/// runs the normal selection once and spawns the supervisor when the
/// daemon owns the backend. Bounded by `DETECT_RETRY_WINDOW`.
fn spawn_backend_detect_retry(
    state: Arc<AppState>,
    registry: Arc<plugin::display_registry::DisplayRegistry>,
    sock_path: PathBuf,
    conn: zbus::Connection,
) {
    let tasks = state.tasks.clone();
    tasks.spawn_async(
        tasks::TaskKind::Service,
        "display/backend-detect",
        async move {
            let mut shutdown_rx = state.shutdown_subscribe();
            let mut delay = display::spawner::DETECT_RETRY_INITIAL;
            let deadline = tokio::time::Instant::now() + display::spawner::DETECT_RETRY_WINDOW;
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown_rx.wait_for(|v| *v) => return Ok(()),
                    _ = tokio::time::sleep(delay) => {}
                }
                let (caps, extra_env) = display::spawner::detect_de_runtime(Some(&conn)).await;
                if caps.xdg_desktop.is_empty() {
                    if tokio::time::Instant::now() >= deadline {
                        log::warn!(
                            "display backend detection gave up after {}s without a session; \
                             pass --display-backend or start waywallen after login",
                            display::spawner::DETECT_RETRY_WINDOW.as_secs()
                        );
                        return Ok(());
                    }
                    delay = std::cmp::min(delay * 2, display::spawner::DETECT_RETRY_MAX);
                    continue;
                }
                log::info!(
                    "session environment appeared (xdg_desktop={:?}); selecting display backend",
                    caps.xdg_desktop
                );
                let (status, backend) = select_display_backend(&registry, &caps, None);
                *state.display_backend_status.write().unwrap() = status;
                state.events.publish(events::GlobalEvent::StatusChanged);
                if let Some(def) = backend {
                    let shutdown_rx = state.shutdown_subscribe();
                    let name = def.name.clone();
                    state.tasks.spawn_async(
                        tasks::TaskKind::Service,
                        format!("display/backend/{name}"),
                        async move {
                            display::spawner::run_backend(def, sock_path, extra_env, shutdown_rx)
                                .await
                                .map_err(|e| {
                                    anyhow::anyhow!("display backend supervisor exited: {e}")
                                })
                        },
                    );
                }
                return Ok(());
            }
        },
    );
}

fn main() -> anyhow::Result<()> {
    env_logger::init();

    // Explicit runtime with a bounded `shutdown_timeout`.
    // Blocking tasks still parked in syscalls cannot stall process exit.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = rt.block_on(async_main());
    rt.shutdown_timeout(std::time::Duration::from_secs(3));
    result
}

async fn async_main() -> anyhow::Result<()> {
    let cli = parse_args();

    let ui_bin: Option<PathBuf> = resolve_ui_path(cli.ui_path.clone());

    // Single-instance gate.
    let handoff_ui = if cli.no_ui { None } else { ui_bin.as_deref() };
    let dbus_conn = dbus_iface::acquire_or_handoff(handoff_ui).await;
    log::info!("DBus name acquired: {}", dbus_iface::BUS_NAME);

    let mut plugin_roots = plugin::renderer_registry::standard_plugin_roots("plugins");
    for plugin_dir in &cli.plugin_dirs {
        plugin_roots.push(plugin::renderer_registry::PluginRoot::system(
            plugin_dir.join("plugins"),
        ));
    }
    let mut plugin_scan = plugin::renderer_registry::scan_plugin_roots(&plugin_roots);
    // Installable-plugin (package) list for the UI's plugin-centric view.
    // Computed before `entries` is taken so entry presence is accurate.
    let plugin_packages = Arc::new(tokio::sync::RwLock::new(plugin_scan.packages()));
    let inactive_system = Arc::new(tokio::sync::RwLock::new(
        plugin_scan.inactive_system.clone(),
    ));
    let inactive_user = Arc::new(tokio::sync::RwLock::new(plugin_scan.inactive_user.clone()));
    let plugin_updates = plugin::update::new_store();
    let plugin_roots = Arc::new(plugin_roots);
    let entry_refs = std::mem::take(&mut plugin_scan.entries);

    let mut registry = plugin::renderer_registry::RendererRegistry::new();
    for def in &plugin_scan.renderers {
        registry.register(def.clone());
    }

    // Shared media probe — constructed once, reused by SourceManager
    // and the sync layer so libavformat is dlopen-ed at most once.
    let probe = Arc::new(AvFormatProbe::new()) as Arc<dyn MediaProbe>;

    // Create an empty source manager now; Lua loading and source scans
    // run later in a background task.
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
    router.attach_settings(settings_store.clone());
    let registry_snapshot = renderer_mgr.registry_snapshot();
    settings_store.reconcile(&registry_snapshot);

    let gpus = Arc::new(gpu::enumerate());
    renderer_mgr.attach_gpus(gpus.clone());
    log::info!("gpu::enumerate found {} GPU(s)", gpus.len());
    for g in gpus.iter() {
        log::debug!(
            "  gpu: render={:?} primary={:?} drm={}:{} pci={:?} {} ({:#06x}:{:#06x})",
            g.render_node,
            g.primary_node,
            g.render_major,
            g.render_minor,
            g.pci_bdf,
            g.driver,
            g.vendor_id,
            g.device_id,
        );
    }
    {
        let valid: std::collections::HashSet<(u32, u32)> = gpus
            .iter()
            .filter(|g| g.render_node.is_some())
            .map(|g| (g.render_major, g.render_minor))
            .collect();
        settings_store.update(|s| {
            for (plugin_name, kv) in s.plugins.iter_mut() {
                let stale = kv.get(gpu::GPU_DRM_DEV_KEY).is_some_and(|v| {
                    gpu::parse_drm_dev(v)
                        .map(|p| !valid.contains(&p))
                        .unwrap_or(true)
                });
                if stale {
                    let removed = kv.remove(gpu::GPU_DRM_DEV_KEY);
                    log::warn!(
                        "clearing stale {} for plugin {}: was {:?}",
                        gpu::GPU_DRM_DEV_KEY,
                        plugin_name,
                        removed
                    );
                }
            }
        });
    }
    let db_path = settings::default_db_path();
    let db = model::connect(&db_path)
        .await
        .with_context(|| format!("open database {}", db_path.display()))?;

    // Hand the DB to the source manager so `ctx.library_meta_*`
    // mlua functions can read and write library metadata.
    {
        let mut sm = source_mgr.lock().await;
        sm.attach_db(db.clone());
    }

    let (shutdown_tx, shutdown_rx_for_tasks) = tokio::sync::watch::channel(false);
    let task_mgr = tasks::TaskManager::spawn(shutdown_rx_for_tasks);

    let (rotation_handle, rotation_rx) = queue::rotator::make_handle();

    let source_plugins = Arc::new(tokio::sync::RwLock::new(Vec::new()));

    let state = Arc::new(AppState {
        renderer_manager: renderer_mgr,
        source_manager: source_mgr.clone(),
        plugins: plugin_packages,
        inactive_system,
        inactive_user,
        plugin_updates,
        plugin_update_check: tokio::sync::Mutex::new(()),
        plugin_roots,
        source_plugins,
        plugin_mutation: tokio::sync::Mutex::new(()),
        autostart: autostart::AutostartService::default(),
        router: router.clone(),
        display_backend_status: std::sync::RwLock::new(
            display::spawner::DisplayBackendStatus::default(),
        ),
        settings: settings_store,
        gpus,
        db: db.clone(),
        queue: tokio::sync::Mutex::new(control::QueueState::default()),
        rotation: rotation_handle,
        events: events::EventBus::default(),
        ws_port: std::sync::atomic::AtomicU16::new(0),
        scan_in_progress: std::sync::atomic::AtomicBool::new(false),
        ui_path: std::sync::Mutex::new(None),
        dbus_conn: std::sync::Mutex::new(None),
        shutdown: shutdown_tx,
        tasks: task_mgr.clone(),
        probe: probe.clone(),
        playlists: playlist::engine::Engine::new(),
        no_tray: cli.no_tray,
        tray: tokio::sync::Mutex::new(None),
    });

    // Auto-rotation service. Runs until shutdown, parked on a watch
    // channel until the user activates a playlist.
    {
        let app_for_rot = state.clone();
        let shutdown_for_rot = state.shutdown_subscribe();
        state
            .tasks
            .spawn_async(tasks::TaskKind::Service, "playlist/rotator", async move {
                control::run_rotator(app_for_rot, rotation_rx, shutdown_for_rot).await;
                Ok(())
            });
    }
    {
        let app_for_restore = state.clone();
        let shutdown_for_restore = state.shutdown_subscribe();
        state
            .tasks
            .spawn_async(tasks::TaskKind::Service, "auto-stop/restore", async move {
                control::run_auto_stop_restore(app_for_restore, shutdown_for_restore).await;
                Ok(())
            });
    }
    {
        let update_state = state.clone();
        let shutdown_for_updates = state.shutdown_subscribe();
        state
            .tasks
            .spawn_async(tasks::TaskKind::Service, "plugin/update-checker", async move {
                control::run_plugin_update_checker(update_state, shutdown_for_updates).await
            });
    }

    // Session state monitor. Watches D-Bus for lock-screen and
    // user-switch events, then forwards them to the router.
    session_monitor::spawn(router.clone(), state.shutdown_subscribe());

    mpris::spawn(state.clone());

    // Start display infrastructure before work that may need a display.
    // This covers both UDS endpoint and daemon-managed backends.
    let mut display_registry =
        plugin::display_registry::build_default_registry().unwrap_or_else(|e| {
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
    let display_registry = Arc::new(display_registry);
    let (display_caps, display_extra_env) =
        display::spawner::detect_de_runtime(Some(&dbus_conn)).await;
    let display_backend: Option<plugin::display_registry::DisplayDef> = if cli.no_display {
        log::info!("--no-display: skipping display backend selection");
        *state.display_backend_status.write().unwrap() =
            display::spawner::DisplayBackendStatus::disabled(&display_caps);
        None
    } else {
        let (status, backend) = select_display_backend(
            &display_registry,
            &display_caps,
            cli.display_backend.as_deref(),
        );
        *state.display_backend_status.write().unwrap() = status;
        backend
    };

    let display_sock_path = display::endpoint::default_socket_path();
    {
        let router = router.clone();
        let sock_path = display_sock_path.clone();
        let shutdown_rx = state.shutdown_subscribe();
        let events_tx = state.events.sender();
        state
            .tasks
            .spawn_async(tasks::TaskKind::Service, "display/endpoint", async move {
                display::endpoint::serve_with_shutdown(&sock_path, router, events_tx, shutdown_rx)
                    .await
                    .map_err(|e| anyhow::anyhow!("display endpoint exited: {e}"))
            });
    }
    if let Some(def) = display_backend {
        let sock_path = display_sock_path.clone();
        let shutdown_rx = state.shutdown_subscribe();
        let extra_env = display_extra_env.clone();
        let name = def.name.clone();
        state.tasks.spawn_async(
            tasks::TaskKind::Service,
            format!("display/backend/{name}"),
            async move {
                display::spawner::run_backend(def, sock_path, extra_env, shutdown_rx)
                    .await
                    .map_err(|e| anyhow::anyhow!("display backend supervisor exited: {e}"))
            },
        );
    } else if !cli.no_display
        && cli.display_backend.is_none()
        && display_caps.xdg_desktop.is_empty()
    {
        // Early-boot recovery: an empty XDG_CURRENT_DESKTOP usually
        // means the daemon started before the session came up (e.g. a
        // systemd unit racing the compositor), not an unknown desktop.
        spawn_backend_detect_retry(
            state.clone(),
            display_registry.clone(),
            display_sock_path.clone(),
            dbus_conn.clone(),
        );
    }

    // Single in-process consumer of the global event bus. Spawn before
    // source/display publishers so no boot marker is missed.
    event_process::spawn(state.clone(), cli.restore_last);

    // Off-load source loading, scanning, DB sync, and playlist seeding so
    // async_main can continue bringing up services.
    {
        let source_mgr = source_mgr.clone();
        let entry_refs = entry_refs.clone();
        let state_for_task = state.clone();
        state
            .tasks
            .spawn_async(tasks::TaskKind::Startup, "startup/sources", async move {
                // Load Lua entries on the blocking pool; each ref carries
                // the owning plugin domain id and entry ABI version.
                tokio::task::spawn_blocking(move || {
                    let mut sm = source_mgr.blocking_lock();
                    for r in &entry_refs {
                        if let Err(e) = sm.load_plugin(
                            &r.entry,
                            &r.plugin_id,
                            &r.plugin_version,
                            r.entry_version,
                        ) {
                            log::warn!("load entry {}: {e:#}", r.entry.display());
                        }
                    }
                })
                .await
                .map_err(|e| anyhow::anyhow!("plugin load join: {e}"))?;

                // Register loaded plugins before auto-detect so names resolve
                // even when no libraries exist yet.
                {
                    let infos = {
                        let sm = state_for_task.source_manager.lock().await;
                        sm.plugins()
                    };
                    match infos {
                        Ok(infos) => {
                            for info in infos {
                                if let Err(e) = crate::model::repo::upsert_plugin(
                                    &state_for_task.db,
                                    &info.name,
                                    &info.version,
                                )
                                .await
                                {
                                    log::warn!("upsert plugin {}: {e:#}", info.name);
                                }
                            }
                        }
                        Err(e) => log::warn!("enumerate loaded plugins: {e:#}"),
                    }
                }

                // Always publish the source-plugin list into the
                // snapshot up front. It's static (from loaded plugins)
                control::refresh_source_plugins(&state_for_task).await;

                // Scan DB-driven libraries and sync results. Skip when no
                // libraries are configured.
                let skip_refresh = crate::model::repo::list_libraries(&state_for_task.db)
                    .await
                    .map(|v| v.is_empty())
                    .unwrap_or(false);
                if skip_refresh {
                    log::debug!("no libraries configured; skipping initial source refresh");
                } else if let Err(e) = control::refresh_sources(&state_for_task).await {
                    log::warn!("initial source refresh failed: {e:#}");
                }

                // Sources and initial DB sync are done; publish the latched
                // marker for external observers.
                state_for_task
                    .events
                    .publish(events::GlobalEvent::SourcesReady);
                Ok(())
            });
    }

    // Bridge router display events to the global event bus.
    // Fires `DisplayReady` once, on the first display.
    {
        let watcher_state = state.clone();
        state.tasks.spawn_async(
            tasks::TaskKind::Service,
            "boot/display-watcher",
            async move {
                if !watcher_state.router.snapshot_displays().await.is_empty() {
                    watcher_state
                        .events
                        .publish(events::GlobalEvent::DisplayReady);
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
                        Ok(routing::RouterEvent::DisplaysReplace(list)) if !list.is_empty() => {
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

    // Restore queue mode, rotation cadence, and manual audio state from disk.
    // Per-display wallpaper restoration is handled elsewhere.
    {
        let restore_state = state.clone();
        state
            .tasks
            .spawn_async(tasks::TaskKind::Startup, "startup/restore", async move {
                control::run_restore(&restore_state)
                    .await
                    .map_err(anyhow::Error::from)
            });
    }

    {
        let app_for_pl = state.clone();
        tokio::spawn(async move {
            playlist::restore::watch_hotplug(app_for_pl).await;
        });
    }

    // Background media-probe scheduler.
    // Pulls unprobed media items from the DB and fills metadata.
    {
        let probe_for_task = probe.clone();
        let db_for_task = db.clone();
        let shutdown_for_task = state.shutdown.subscribe();
        state
            .tasks
            .spawn_async(tasks::TaskKind::Service, "probe/scheduler", async move {
                probe::task::scheduler_loop(db_for_task, probe_for_task, shutdown_for_task)
                    .await
                    .map_err(anyhow::Error::from)
            });
    }

    // Bind the WS control plane (port 0 = OS picks an available port).
    let bind_addr = format!("127.0.0.1:{}", cli.ws_port);
    let (local_addr, ws_fut) = ws_server::bind(state.clone(), &bind_addr).await?;
    let ws_port = local_addr.port();
    state
        .ws_port
        .store(ws_port, std::sync::atomic::Ordering::SeqCst);
    log::info!("ws port: {ws_port}");

    match ui_bin {
        Some(ui_bin) => {
            *state.ui_path.lock().unwrap() = Some(ui_bin);
            if cli.no_ui {
                log::info!("ui auto-start suppressed (--no-ui); open via tray or relaunch");
            } else {
                spawn_ui(&state);
            }
        }
        None => log::info!("waywallen-ui not found, running headless"),
    }

    // Publish the Daemon1 interface on the connection we already own.
    let dbus_conn = dbus_iface::serve(
        dbus_conn,
        state.clone(),
        display_sock_path.to_string_lossy().into_owned(),
    )
    .await
    .context("publish DBus interface")?;
    *state.dbus_conn.lock().unwrap() = Some(dbus_conn.clone());
    if let Err(e) = dbus_iface::emit_ready(&dbus_conn).await {
        log::warn!("DBus Ready emit failed: {e}");
    }

    // Latch DaemonReady and broadcast fresh status.
    // Late connections observe readiness from the latch.
    state
        .events
        .publish(crate::events::GlobalEvent::DaemonReady);
    state
        .events
        .publish(crate::events::GlobalEvent::StatusChanged);

    // Tray icon is best-effort and requires a StatusNotifierWatcher.
    if cli.no_tray {
        log::info!("tray disabled by --no-tray");
    } else if state.settings.global().hide_tray_icon {
        log::info!("tray hidden by hide_tray_icon setting");
    } else {
        let state_t = state.clone();
        tokio::spawn(async move {
            tray::ensure_started(state_t).await;
        });
    }

    // SIGTERM (default `kill <pid>`, systemd stop) needs an explicit
    // listener — `tokio::signal::ctrl_c()` only catches SIGINT.
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

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

    // Whatever woke us, make sure every subscriber observes shutdown.
    state.shutdown_now();

    // Flush settings synchronously so any pending debounced write lands.
    state.settings.flush_now().await;

    if let Err(e) = dbus_iface::emit_shutting_down(&dbus_conn).await {
        log::warn!("DBus ShuttingDown emit failed: {e}");
    }
    drop(dbus_conn);

    Ok(())
}
