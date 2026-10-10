use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;

use super::{DaemonConfig, DaemonContext};
use crate::probe::media::{AvFormatProbe, MediaProbe};
use crate::wallframe::{display, renderer_manager, routing};
use crate::{
    api, application, event_process, events, model, playback, plugin, probe, settings, system,
    tasks,
};

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

fn build_display_registry(plugin_dirs: &[PathBuf]) -> plugin::display_registry::DisplayRegistry {
    let mut registry = plugin::display_registry::build_default_registry();
    for plugin_dir in plugin_dirs {
        let displays_dir = plugin_dir.join("displays");
        if !displays_dir.is_dir() {
            continue;
        }
        match plugin::display_registry::DisplayRegistry::scan(&displays_dir) {
            Ok(scanned) => {
                for def in scanned.all() {
                    registry.register(def.clone());
                }
            }
            Err(error) => log::warn!("scan {}: {error}", displays_dir.display()),
        }
    }
    registry
}

fn select_display_backend(
    registry: &plugin::display_registry::DisplayRegistry,
    caps: &display::spawner::DeCaps,
    requested: Option<&str>,
) -> anyhow::Result<display::spawner::PickOutcome> {
    let Some(name) = requested else {
        return Ok(display::spawner::pick_backend(registry, caps));
    };
    let Some(def) = registry.find(name) else {
        let available = registry
            .all()
            .iter()
            .map(|def| def.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!("unknown display backend '{name}'; available backends: {available}");
    };

    log::info!("display backend pinned by --display-backend: {name}");
    Ok(display::spawner::PickOutcome::Matched(def.clone()))
}

pub async fn run(cli: DaemonConfig) -> anyhow::Result<()> {
    let ui_bin: Option<PathBuf> = resolve_ui_path(cli.ui_path.clone());
    let display_caps = display::spawner::detect_de();
    let display_pick = if cli.no_display {
        None
    } else {
        let registry = build_display_registry(&cli.plugin_dirs);
        Some(select_display_backend(
            &registry,
            &display_caps,
            cli.display_backend.as_deref(),
        )?)
    };

    // Single-instance gate.
    let handoff_ui = if cli.no_ui { None } else { ui_bin.as_deref() };
    let dbus_conn = system::dbus::acquire_or_handoff(handoff_ui).await;
    log::info!("DBus name acquired: {}", system::dbus::BUS_NAME);

    let display_sock_path = display::endpoint::default_socket_path();
    let display_listener = display::endpoint::bind_socket(&display_sock_path).await?;

    let mut plugin_roots = plugin::renderer_registry::standard_plugin_roots("plugins");
    for plugin_dir in &cli.plugin_dirs {
        plugin_roots.push(plugin::renderer_registry::PluginRoot::system(
            plugin_dir.join("plugins"),
        ));
    }
    let mut plugin_scan = plugin::renderer_registry::scan_plugin_roots(&plugin_roots);
    let plugin_translations = plugin_scan
        .translation_documents()
        .map_err(|error| anyhow::anyhow!("load plugin translations: {error}"))?;
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
    let source_mgr = Arc::new(
        plugin::source::SourceManager::with_probe(probe.clone())
            .expect("failed to create source manager"),
    );

    let renderer_mgr = Arc::new(renderer_manager::RendererManager::new(registry));
    let router = routing::Router::new(renderer_mgr.clone());
    let process_exits = renderer_mgr
        .take_process_exits()
        .expect("renderer process exit receiver already taken");
    router.start_process_exit_listener(process_exits);
    renderer_mgr.start_reaper();
    let settings_store =
        settings::SettingsStore::load_or_default(settings::default_config_path()).await;
    crate::logging::apply_debug_setting(settings_store.global().debug_logging_enabled);
    renderer_mgr
        .initialize_renderer_log_level(settings_store.global().debug_logging_enabled)
        .await;
    renderer_mgr.attach_settings(settings_store.clone());
    router.attach_settings(settings_store.clone());
    let registry_snapshot = renderer_mgr.registry_snapshot();
    settings_store.reconcile(&registry_snapshot);

    let system_info = Arc::new(system::SystemInfo::load());
    renderer_mgr.attach_system_info(system_info.clone());
    let gpu_count = system_info.gpus().len();
    if gpu_count == 0 {
        log::warn!(
            target: "waywallen::gpu",
            "no GPU render nodes detected; Vulkan/VA-API hardware paths will be unavailable"
        );
    } else {
        log::info!(target: "waywallen::gpu", "system: discovered {gpu_count} GPU(s)");
    }
    for g in system_info.gpus() {
        log::info!(
            target: "waywallen::gpu",
            "gpu: render={:?} primary={:?} drm={}:{} pci={:?} name={:?} {} ({:#06x}:{:#06x})",
            g.render_node,
            g.primary_node,
            g.render_major,
            g.render_minor,
            g.pci_bdf,
            g.name,
            g.driver,
            g.vendor_id,
            g.device_id,
        );
    }
    {
        settings_store.update(|s| {
            for (plugin_name, kv) in s.plugins.iter_mut() {
                let stale = kv
                    .get(system::GPU_DRM_DEV_KEY)
                    .is_some_and(|value| !system_info.has_render_device(value));
                if stale {
                    let removed = kv.remove(system::GPU_DRM_DEV_KEY);
                    log::warn!(
                        "clearing stale {} for plugin {}: was {:?}",
                        system::GPU_DRM_DEV_KEY,
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
    source_mgr.attach_db(db.clone());
    source_mgr.attach_settings(settings_store.clone());

    let (shutdown_tx, shutdown_rx_for_tasks) = tokio::sync::watch::channel(false);
    let task_mgr = tasks::TaskManager::spawn(shutdown_rx_for_tasks);
    let events = events::EventBus::default();
    let qr_login = plugin::qr_login::QrLoginManager::new(
        source_mgr.clone(),
        events.sender(),
        shutdown_tx.subscribe(),
    );

    let (rotation_handle, rotation_rx) = playback::rotation::make_handle();

    let source_plugins = Arc::new(tokio::sync::RwLock::new(Vec::new()));
    let plugin_translations = Arc::new(tokio::sync::RwLock::new(
        plugin::i18n::PluginTranslationSnapshot {
            generation: 1,
            documents: plugin_translations,
        },
    ));

    let audio_service = system::audio::AudioService::start(
        renderer_mgr.clone(),
        router.clone(),
        settings_store.clone(),
        events.subscribe(),
        shutdown_tx.subscribe(),
        task_mgr.as_ref(),
    );
    let state = Arc::new(DaemonContext {
        renderer_manager: renderer_mgr,
        _audio: audio_service,
        source_manager: source_mgr.clone(),
        qr_login,
        plugins: plugin_packages,
        inactive_system,
        inactive_user,
        plugin_updates,
        plugin_update_check: tokio::sync::Mutex::new(()),
        plugin_roots,
        source_plugins,
        plugin_translations,
        plugin_mutation: tokio::sync::Mutex::new(()),
        autostart: system::autostart::AutostartService::default(),
        router: router.clone(),
        display_backend_status: std::sync::RwLock::new(
            display::spawner::DisplayBackendStatus::default(),
        ),
        settings: settings_store,
        system_info,
        db: db.clone(),
        queue: tokio::sync::Mutex::new(playback::QueueState::default()),
        rotation: rotation_handle,
        events,
        ws_port: std::sync::atomic::AtomicU16::new(0),
        scan_in_progress: std::sync::atomic::AtomicBool::new(false),
        ui_path: std::sync::Mutex::new(None),
        xdg_activation_token: std::sync::Mutex::new(None),
        dbus_conn: std::sync::Mutex::new(None),
        shutdown: shutdown_tx,
        tasks: task_mgr.clone(),
        probe: probe.clone(),
        playlists: playback::playlist::Engine::new(),
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
                application::run_rotator(app_for_rot, rotation_rx, shutdown_for_rot).await;
                Ok(())
            });
    }
    {
        let update_state = state.clone();
        let shutdown_for_updates = state.shutdown_subscribe();
        state.tasks.spawn_async(
            tasks::TaskKind::Service,
            "plugin/update-checker",
            async move {
                application::run_plugin_update_checker(update_state, shutdown_for_updates).await
            },
        );
    }

    // Session monitor. Watches D-Bus for lock-screen, user-switch, and
    // login-session removal events.
    {
        let router = router.clone();
        let session_bus = dbus_conn.clone();
        let shutdown = state.shutdown_subscribe();
        let session_state = state.clone();
        state.tasks.spawn_async(
            tasks::TaskKind::Service,
            "service/session-monitor",
            async move {
                let reason = system::session::run(router, session_bus, shutdown).await?;
                if reason != system::session::ExitReason::DaemonShutdown {
                    log::info!("session monitor requested shutdown: {reason:?}");
                    session_state.shutdown_now();
                }
                Ok(())
            },
        );
    }
    {
        let router = router.clone();
        let session_bus = dbus_conn.clone();
        let shutdown = state.shutdown_subscribe();
        state.tasks.spawn_async(
            tasks::TaskKind::Service,
            "service/gamemode-monitor",
            async move {
                system::gamemode::run(router, session_bus, shutdown).await;
                Ok(())
            },
        );
    }

    system::mpris::spawn(state.clone());

    // Start display infrastructure before work that may need a display.
    // This covers both UDS endpoint and daemon-managed backends.
    let display_backend: Option<plugin::display_registry::DisplayDef> = if cli.no_display {
        log::info!("--no-display: skipping display backend selection");
        *state.display_backend_status.write().unwrap() =
            display::spawner::DisplayBackendStatus::disabled(&display_caps);
        None
    } else {
        let pick = display_pick.expect("display pick missing without --no-display");
        display::spawner::log_outcome(&pick, &display_caps);
        let should_spawn = display::spawner::should_daemon_spawn(&pick);
        let (status, backend) = match pick {
            display::spawner::PickOutcome::KdeHardMatch(def)
            | display::spawner::PickOutcome::Matched(def)
                if should_spawn =>
            {
                let (status, backend) =
                    display::spawner::preflight_daemon_backend(def, &display_caps);
                if status.state == display::spawner::DISPLAY_BACKEND_STATE_BINARY_MISSING
                    || status.state == display::spawner::DISPLAY_BACKEND_STATE_FLATPAK_RESTRICTED
                {
                    log::error!("{}", status.reason);
                }
                (status, backend)
            }
            display::spawner::PickOutcome::KdeHardMatch(def)
            | display::spawner::PickOutcome::Matched(def) => (
                display::spawner::DisplayBackendStatus::external(&def, &display_caps),
                None,
            ),
            display::spawner::PickOutcome::None => (
                display::spawner::DisplayBackendStatus::unmatched(&display_caps),
                None,
            ),
        };
        *state.display_backend_status.write().unwrap() = status;
        backend
    };

    // Subscribe the process-wide handler before display clients can publish
    // transient handshake failures.
    event_process::spawn(state.clone(), cli.restore_last);

    {
        let router = router.clone();
        let sock_path = display_sock_path.clone();
        let shutdown_rx = state.shutdown_subscribe();
        let events_tx = state.events.sender();
        state
            .tasks
            .spawn_async(tasks::TaskKind::Service, "display/endpoint", async move {
                display::endpoint::serve_bound(
                    &sock_path,
                    display_listener,
                    router,
                    events_tx,
                    shutdown_rx,
                )
                .await
                .map_err(|e| anyhow::anyhow!("display endpoint exited: {e}"))
            });
    }
    if let Some(def) = display_backend {
        let sock_path = display_sock_path.clone();
        let shutdown_rx = state.shutdown_subscribe();
        let name = def.name.clone();
        state.tasks.spawn_async(
            tasks::TaskKind::Service,
            format!("display/backend/{name}"),
            async move {
                display::spawner::run_backend(def, sock_path, shutdown_rx)
                    .await
                    .map_err(|e| anyhow::anyhow!("display backend supervisor exited: {e}"))
            },
        );
    }

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
                    for r in &entry_refs {
                        if let Err(e) = source_mgr.load_plugin(
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
                    let infos = state_for_task.source_manager.plugins();
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
                application::refresh_source_plugins(&state_for_task).await;

                // Scan DB-driven libraries and sync results. Skip when no
                // libraries are configured.
                let skip_refresh = crate::model::repo::list_libraries(&state_for_task.db)
                    .await
                    .map(|v| v.is_empty())
                    .unwrap_or(false);
                if skip_refresh {
                    log::debug!("no libraries configured; skipping initial source refresh");
                } else if let Err(e) = application::refresh_sources(&state_for_task).await {
                    log::warn!("initial source refresh failed: {e:#}");
                }

                // Sources and initial DB sync are done; publish the latched
                // marker for external observers.
                state_for_task
                    .events
                    .publish(events::GlobalEvent::SourcesReady);

                state_for_task.source_manager.refresh_dynamic_tags().await;
                state_for_task
                    .events
                    .publish(events::GlobalEvent::SettingsChanged);
                Ok(())
            });
    }

    // Bridge router display events to the global event bus.
    // Fires `DisplayReady` once, on the first display.
    {
        let watcher_state = state.clone();
        state.tasks.spawn_async(
            tasks::TaskKind::Startup,
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
                application::run_restore(&restore_state)
                    .await
                    .map_err(anyhow::Error::from)
            });
    }

    {
        let app_for_pl = state.clone();
        state
            .tasks
            .spawn_async(tasks::TaskKind::Service, "playlist/hotplug", async move {
                application::playback::restore::watch_hotplug(app_for_pl).await;
                Ok(())
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

    // Library watcher. Rescans when a library directory changes on disk.
    {
        let watch_state = state.clone();
        let shutdown_for_watch = state.shutdown_subscribe();
        state
            .tasks
            .spawn_async(tasks::TaskKind::Service, "catalog/watcher", async move {
                application::run_library_watcher(watch_state, shutdown_for_watch).await
            });
    }

    // Bind the WS control plane (port 0 = OS picks an available port).
    let bind_addr = format!("127.0.0.1:{}", cli.ws_port);
    let (local_addr, ws_fut) = api::websocket::bind(state.clone(), &bind_addr).await?;
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
                super::runtime::spawn_ui(&state);
            }
        }
        None => log::info!("waywallen-ui not found, running headless"),
    }

    // Publish the Daemon1 interface on the connection we already own.
    let dbus_conn = system::dbus::serve(
        dbus_conn,
        state.clone(),
        display_sock_path.to_string_lossy().into_owned(),
    )
    .await
    .context("publish DBus interface")?;
    *state.dbus_conn.lock().unwrap() = Some(dbus_conn.clone());
    if let Err(e) = system::dbus::emit_ready(&dbus_conn).await {
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
        state
            .tasks
            .spawn_async(tasks::TaskKind::Service, "tray/startup", async move {
                system::tray::ensure_started(state_t).await;
                Ok(())
            });
    }

    super::runtime::run_until_shutdown(state, ws_fut, dbus_conn).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_builtin_bypasses_desktop_auto_selection() {
        let registry = plugin::display_registry::DisplayRegistry::with_builtins();
        let caps = display::spawner::DeCaps {
            xdg_desktop: vec!["kde".to_string()],
            ..Default::default()
        };

        let selected = select_display_backend(&registry, &caps, Some("layer-shell")).unwrap();

        match selected {
            display::spawner::PickOutcome::Matched(def) => {
                assert_eq!(def.name, "layer-shell");
            }
            other => panic!("expected explicitly matched layer-shell, got {other:?}"),
        }
    }

    #[test]
    fn extra_manifest_overrides_builtin_before_selection() {
        let root = tempfile::tempdir().unwrap();
        let displays = root.path().join("displays");
        std::fs::create_dir(&displays).unwrap();
        std::fs::write(
            displays.join("layer-shell.toml"),
            r#"
[display]
name = "layer-shell"
bin = "custom-layer-shell"
de = ["kde"]
priority = 200
spawn = "daemon"
"#,
        )
        .unwrap();

        let registry = build_display_registry(&[root.path().to_path_buf()]);
        let selected = select_display_backend(
            &registry,
            &display::spawner::DeCaps::default(),
            Some("layer-shell"),
        )
        .unwrap();

        match selected {
            display::spawner::PickOutcome::Matched(def) => {
                assert_eq!(def.bin, displays.join("custom-layer-shell"));
                assert_eq!(def.de, ["kde"]);
                assert_eq!(def.priority, 200);
            }
            other => panic!("expected manifest override, got {other:?}"),
        }
    }

    #[test]
    fn unknown_explicit_backend_is_an_error_with_available_names() {
        let registry = plugin::display_registry::DisplayRegistry::with_builtins();
        let error = select_display_backend(
            &registry,
            &display::spawner::DeCaps::default(),
            Some("missing"),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("unknown display backend 'missing'"));
        assert!(error.contains("kde-plasma"));
        assert!(error.contains("gnome-shell"));
        assert!(error.contains("layer-shell"));
    }
}
