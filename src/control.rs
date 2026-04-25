//! Shared wallpaper control logic.
//!
//! The same operations (apply, next, previous, pause, resume, rescan) are
//! driven from two surfaces — the WebSocket control plane (`ws_server`)
//! and the session-bus `Daemon1` interface (`dbus_iface`) plus the tray.
//! This module owns the canonical implementation so both paths converge
//! on identical semantics (spawn-before-kill, router relink, playlist
//! cursor tracking).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};

use crate::ipc::proto::ControlMsg;
use crate::model::{repo, sync};
use crate::playlist::rotator::RotationConfig;
use crate::playlist::Mode;
use crate::renderer_manager;
use crate::wallpaper_type::WallpaperEntry;
use crate::AppState;

/// Re-export so callers that already wrote `control::PlaylistState`
/// don't have to chase the move into the `playlist` module.
pub use crate::playlist::PlaylistState;

pub struct ApplyResult {
    pub renderer_id: String,
    pub entry: WallpaperEntry,
}

/// Apply a wallpaper by id, with single-flight semantics across the
/// daemon: only one apply is in flight at a time. A subsequent call
/// supersedes any in-flight prior call (the prior caller observes
/// `apply task superseded or cancelled` and the prior renderer-spawn
/// in progress is dropped, which kills its child via `kill_on_drop`).
///
/// This sits on top of `crate::tasks::TaskManager::spawn_async_unique`
/// using the fixed key `apply/global` — Iter 3 only serializes globally;
/// per-display keys land when displays can be assigned distinct
/// wallpapers.
pub async fn apply_wallpaper_by_id(
    app: &Arc<AppState>,
    id: &str,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<ApplyResult> {
    let app_clone = app.clone();
    let id_owned = id.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<ApplyResult>>();
    app.tasks.spawn_async_unique(
        crate::tasks::TaskKind::Apply,
        "apply/global",
        format!("apply/{id_owned}"),
        async move {
            let res = apply_wallpaper_inner(&app_clone, &id_owned, width, height, fps).await;
            // If the receiver is gone the caller already moved on (or
            // was itself cancelled); silently drop the result.
            let _ = tx.send(res);
            Ok(())
        },
    );
    rx.await
        .map_err(|_| anyhow!("apply task superseded or cancelled"))?
}

/// The actual apply work — spawn renderer, relink displays, kill old
/// renderers, update playlist. Caller is the unique apply task.
async fn apply_wallpaper_inner(
    app: &Arc<AppState>,
    id: &str,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<ApplyResult> {
    let entry = {
        let mgr = app.source_manager.lock().await;
        mgr.get(id).cloned()
    };
    let entry = entry.ok_or_else(|| anyhow!("wallpaper '{id}' not found"))?;

    if app
        .renderer_manager
        .registry()
        .resolve(&entry.wp_type)
        .is_none()
    {
        return Err(anyhow!("no renderer for wallpaper type '{}'", entry.wp_type));
    }

    let pre_existing: Vec<String> = app.renderer_manager.list().await;

    let width = if width == 0 { 1920 } else { width };
    let height = if height == 0 { 1080 } else { height };
    let fps = if fps == 0 { 30 } else { fps };
    let spawn_req = renderer_manager::SpawnRequest {
        wp_type: entry.wp_type.clone(),
        metadata: entry.metadata.clone(),
        width,
        height,
        fps,
        test_pattern: false,
    };
    let renderer_id = app.renderer_manager.spawn(spawn_req).await?;
    if let Some(handle) = app.renderer_manager.get(&renderer_id).await {
        app.router.register_renderer(handle).await;
    }
    app.router.relink_all_displays_to(&renderer_id).await;
    for old_id in pre_existing {
        if old_id != renderer_id {
            app.router.unregister_renderer(&old_id).await;
            let _ = app.renderer_manager.kill(&old_id).await;
        }
    }

    {
        let mut playlist = app.playlist.lock().await;
        playlist.locate(&entry.id);
        playlist.current = Some(entry.id.clone());
    }

    app.settings.update(|s| {
        s.global.last_wallpaper = Some(entry.id.clone());
    });
    // Push the just-applied wallpaper to disk synchronously instead of
    // waiting on the 2s debounce. A kill / SIGTERM inside the debounce
    // window would otherwise lose `last_wallpaper`, which is exactly
    // the value the next start needs to reproduce playback. flush_now
    // is a cheap no-op when nothing actually changed.
    app.settings.flush_now().await;

    Ok(ApplyResult { renderer_id, entry })
}

/// Advance the playlist cursor by `delta` and apply the result.
///
/// For the "All" pseudo-playlist (`active_id = None`) and curated
/// playlists where membership is already cached on the state, this is
/// a thin step over the in-memory cursor. Smart playlists also work
/// here as long as `refresh_sources` has populated `ids` from the
/// filter; the rotator (P5) calls this on every interval tick.
pub async fn step(app: &Arc<AppState>, delta: i32) -> Result<String> {
    // For All, refresh from source on every step so newly-imported
    // wallpapers join the rotation without waiting for an explicit
    // rescan. For curated/smart playlists, ids are already pinned —
    // do not regenerate them here.
    let next_id = {
        let mut playlist = app.playlist.lock().await;
        if playlist.active_id.is_none() {
            let snapshot: Vec<String> = {
                let mgr = app.source_manager.lock().await;
                mgr.list().iter().map(|e| e.id.clone()).collect()
            };
            playlist.refresh(snapshot);
        }
        if playlist.ids.is_empty() {
            return Err(anyhow!("playlist is empty"));
        }
        playlist
            .step(delta)
            .ok_or_else(|| anyhow!("playlist is empty"))?
    };
    apply_wallpaper_by_id(app, &next_id, 0, 0, 0).await?;
    // Reset the rotator deadline so the user gets the full quiet
    // window after a manual advance instead of being walked over by
    // the next auto tick.
    app.rotation.kick();
    Ok(next_id)
}

/// Set the rotation mode on the active playlist. Pure in-memory; the
/// caller is responsible for persistence (settings + DB) when the
/// active playlist is a real DB row.
pub async fn set_mode(app: &Arc<AppState>, mode: Mode) {
    app.playlist.lock().await.set_mode(mode);
    app.settings.update(|s| {
        s.global.playlist_mode = mode.as_str().to_owned();
    });
}

/// Set the auto-rotation interval (seconds; `0` disables). Updates
/// the live rotator via the watch handle and persists the value to
/// settings so a daemon restart resumes the same cadence.
pub async fn set_rotation_interval(app: &Arc<AppState>, secs: u32) {
    app.rotation.set_interval(secs);
    app.settings.update(|s| {
        s.global.rotation_secs = secs;
    });
}

/// Convenience: flip shuffle on/off without exposing the [`Mode`]
/// enum to D-Bus / WS callers. `true` → Shuffle, `false` → Sequential.
pub async fn set_shuffle(app: &Arc<AppState>, on: bool) {
    let mode = if on { Mode::Shuffle } else { Mode::Sequential };
    set_mode(app, mode).await;
}

/// Summary row used by `ListPlaylists`. Stays string-typed so the
/// D-Bus signature `a(isxs)` (id, name, source_kind, item_count)
/// stays human-readable.
#[derive(Debug, Clone)]
pub struct PlaylistSummary {
    pub id: i64,
    pub name: String,
    pub source_kind: String,
    pub mode: String,
    pub interval_secs: i32,
    pub item_count: u32,
}

pub async fn list_playlists(app: &Arc<AppState>) -> Result<Vec<PlaylistSummary>> {
    let rows = repo::list_playlists(&app.db).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        // Curated count = playlist_item rows. Smart count is left at
        // 0 here since computing it would require resolving against
        // the snapshot — `PlaylistStatus` (active playlist only) is
        // the right place for that, not a list summary.
        let item_count = if r.source_kind == repo::PLAYLIST_KIND_CURATED {
            repo::list_playlist_item_ids(&app.db, r.id)
                .await
                .unwrap_or_default()
                .len() as u32
        } else {
            0
        };
        out.push(PlaylistSummary {
            id: r.id,
            name: r.name,
            source_kind: r.source_kind,
            mode: r.mode,
            interval_secs: r.interval_secs,
            item_count,
        });
    }
    Ok(out)
}

/// Snapshot of the live playlist state for status reporting.
#[derive(Debug, Clone)]
pub struct PlaylistStatus {
    pub active_id: Option<i64>,
    pub mode: String,
    pub interval_secs: u32,
    pub current: Option<String>,
    pub position: Option<u32>,
    pub count: u32,
    pub is_smart: bool,
}

pub async fn playlist_status(app: &Arc<AppState>) -> PlaylistStatus {
    let g = app.playlist.lock().await;
    PlaylistStatus {
        active_id: g.active_id,
        mode: g.mode.as_str().to_owned(),
        interval_secs: app.rotation.interval(),
        current: g.current.clone(),
        position: g.position().map(|p| p as u32),
        count: g.count() as u32,
        is_smart: g.filter.is_some(),
    }
}

/// Activate a persisted playlist. Loads its row, installs the
/// associated mode/filter/seed, and resolves member ids against the
/// current source-manager snapshot. After this returns, `step()`
/// walks the activated playlist instead of the All pseudo-list.
/// Persists `active_playlist_id` to settings so a daemon restart
/// re-enters the same playlist.
pub async fn activate_playlist(app: &Arc<AppState>, id: i64) -> Result<()> {
    let snapshot: Vec<WallpaperEntry> = {
        let mgr = app.source_manager.lock().await;
        mgr.list().to_vec()
    };
    {
        let mut state = app.playlist.lock().await;
        crate::playlist::resolve::activate(&app.db, &snapshot, &mut state, id).await?;
    }
    app.settings.update(|s| {
        s.global.active_playlist_id = Some(id);
    });
    Ok(())
}

/// Switch back to the All pseudo-playlist. Membership is recomputed
/// against the current snapshot so the cursor stays usable
/// immediately, no rescan required.
pub async fn deactivate_playlist(app: &Arc<AppState>) -> Result<()> {
    let snapshot: Vec<WallpaperEntry> = {
        let mgr = app.source_manager.lock().await;
        mgr.list().to_vec()
    };
    {
        let mut state = app.playlist.lock().await;
        crate::playlist::resolve::deactivate(&mut state);
        let ids: Vec<String> = snapshot.iter().map(|e| e.id.clone()).collect();
        state.refresh(ids);
    }
    app.settings.update(|s| {
        s.global.active_playlist_id = None;
    });
    Ok(())
}

/// Restore the persisted wallpaper + playlist state. Idempotent —
/// callable on demand if a future feature wants to "re-load saved
/// state" without a full daemon restart. Publishes `RestoreApplied`
/// or `RestoreFailed` on the global event bus on completion so
/// observers (logs, integration tests, future UI status) can react.
pub async fn run_restore(app: &Arc<AppState>, restore_last: bool) -> Result<()> {
    use crate::events::GlobalEvent;

    let mut applied: Option<String> = None;

    if restore_last {
        if let Some(last_id) = app.settings.global().last_wallpaper.clone() {
            log::info!("restoring last wallpaper: {last_id}");
            match apply_wallpaper_by_id(app, &last_id, 0, 0, 0).await {
                Ok(_) => applied = Some(last_id),
                Err(e) => {
                    log::warn!("failed to restore last wallpaper: {e:#}");
                    app.events
                        .publish(GlobalEvent::RestoreFailed(format!("apply: {e:#}")));
                }
            }
        }
    }

    // Order matters: activate (which sets mode/seed from the DB row)
    // BEFORE applying the saved in-memory mode preference, so the
    // user's last All-pseudo-list mode wins when there's no DB row to
    // override.
    let g = app.settings.global();
    if let Some(pl_id) = g.active_playlist_id {
        if let Err(e) = activate_playlist(app, pl_id).await {
            log::warn!("failed to activate playlist id={pl_id}: {e:#}");
            app.events
                .publish(GlobalEvent::RestoreFailed(format!("activate: {e:#}")));
        }
    }
    if let Some(mode) = crate::playlist::Mode::from_str(&g.playlist_mode) {
        app.playlist.lock().await.set_mode(mode);
    }
    if g.rotation_secs > 0 {
        app.rotation.set_interval(g.rotation_secs);
    }

    app.events.publish(GlobalEvent::RestoreApplied(applied));
    Ok(())
}

/// Block until at least one display is registered with the router
/// (or `timeout` elapses, whichever comes first). Returns `true` if
/// a display is up by the time we return, `false` on timeout.
///
/// Used by the startup-restore path so applying the saved wallpaper
/// doesn't race the display backend's first connect — without this
/// gate the renderer spawns into a vacuum, the relink-all-displays
/// step is a no-op (no displays yet), and the wallpaper never
/// actually shows up on screen.
pub async fn wait_for_display(app: &Arc<AppState>, timeout: Duration) -> bool {
    // Fast path: a display is already registered (e.g. KDE wallpaper
    // plugin connected before the startup task got around to running).
    if !app.router.snapshot_displays().await.is_empty() {
        return true;
    }
    let mut events = app.router.subscribe_events();
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => return false,
            evt = events.recv() => match evt {
                Ok(crate::routing::RouterEvent::DisplayUpsert(_)) => return true,
                Ok(crate::routing::RouterEvent::DisplaysReplace(list)) if !list.is_empty() => {
                    return true;
                }
                Ok(_) => continue,
                Err(_) => {
                    // Broadcast lag or channel close — fall back to a
                    // direct snapshot. Either we missed the upsert
                    // event (and the snapshot is now non-empty, return
                    // true) or the router shut down (snapshot empty,
                    // restore won't help anyway).
                    return !app.router.snapshot_displays().await.is_empty();
                }
            }
        }
    }
}

/// Auto-rotation task body. Lives here (not in `playlist::rotator`)
/// because it depends on `AppState` + `control::step`, both private
/// to the binary. Reads the live `RotationConfig` from a watch and
/// either parks (interval = 0) or fires `step(+1)` every
/// `interval_secs`. Any config edit (new interval, manual kick)
/// resets the deadline.
pub async fn run_rotator(
    app: Arc<AppState>,
    mut rx: tokio::sync::watch::Receiver<RotationConfig>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    log::info!("playlist rotator started");
    loop {
        let cfg = *rx.borrow();
        if cfg.interval_secs == 0 {
            tokio::select! {
                _ = rx.changed() => continue,
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() { break; }
                }
            }
        } else {
            let dur = std::time::Duration::from_secs(cfg.interval_secs as u64);
            tokio::select! {
                _ = tokio::time::sleep(dur) => {
                    if rx.borrow().interval_secs == 0 {
                        continue;
                    }
                    if let Err(e) = step(&app, 1).await {
                        log::warn!("rotator tick step failed: {e:#}");
                    }
                    // step() calls rotation.kick() on success which
                    // emits a watch change; the next iteration's
                    // rx.changed arm wakes immediately and we re-arm
                    // the sleep — the user-pressed-Next branch is
                    // identical, so manual + auto share one code path.
                }
                _ = rx.changed() => continue,
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() { break; }
                }
            }
        }
    }
    log::info!("playlist rotator exited");
}

pub async fn pause_all(app: &Arc<AppState>) -> Result<()> {
    send_all(app, ControlMsg::Pause).await
}

pub async fn resume_all(app: &Arc<AppState>) -> Result<()> {
    send_all(app, ControlMsg::Play).await
}

async fn send_all(app: &Arc<AppState>, msg: ControlMsg) -> Result<()> {
    let ids = app.renderer_manager.list().await;
    for id in ids {
        if let Err(e) = app.renderer_manager.send_control(&id, msg.clone()).await {
            log::warn!("control {id}: {e}");
        }
    }
    Ok(())
}

pub async fn rescan(app: &Arc<AppState>) -> Result<usize> {
    refresh_sources(app).await
}

/// Run every source plugin's `auto_detect(ctx)` against well-known
/// locations and register whatever exists as a library. Duplicates
/// (paths already registered for the same plugin) are silently
/// skipped. Emits `LibraryUpsert` events and kicks off a full
/// rescan so the newly-detected libraries immediately show up in the
/// UI. Returns the snapshots that were actually added.
pub async fn auto_detect_libraries(
    app: &Arc<AppState>,
) -> Result<Vec<crate::routing::LibrarySnapshot>> {
    use crate::routing::LibrarySnapshot;

    let detected = {
        let sm = app.source_manager.lock().await;
        sm.auto_detect_all()?
    };
    if detected.is_empty() {
        return Ok(Vec::new());
    }

    let mut added: Vec<LibrarySnapshot> = Vec::new();
    for (plugin_name, paths) in detected {
        let plugin = match repo::find_plugin_by_name(&app.db, &plugin_name).await? {
            Some(p) => p,
            None => {
                log::warn!("auto_detect: plugin '{plugin_name}' not registered in DB, skipping");
                continue;
            }
        };
        for path in paths {
            match repo::find_library(&app.db, plugin.id, &path).await {
                Ok(Some(_)) => continue,
                Ok(None) => {}
                Err(e) => {
                    log::warn!("auto_detect: find_library({path}): {e:#}");
                    continue;
                }
            }
            match repo::add_library(&app.db, plugin.id, &path).await {
                Ok(lib) => {
                    let snap = LibrarySnapshot {
                        id: lib.id,
                        path: lib.path,
                        plugin_name: plugin_name.clone(),
                    };
                    app.router.upsert_library(snap.clone());
                    added.push(snap);
                }
                Err(e) => log::warn!("auto_detect: add_library({path}): {e:#}"),
            }
        }
    }

    if !added.is_empty() {
        let app_clone = app.clone();
        tokio::spawn(async move {
            if let Err(e) = refresh_sources(&app_clone).await {
                log::warn!("rescan after auto_detect failed: {e:#}");
            }
        });
    }
    Ok(added)
}

/// Pull every library row out of the DB and rehydrate it into the
/// router-wire `LibrarySnapshot` shape (path + plugin_name). Used by
/// the `LibraryList` query and the initial snapshot sent to WS
/// subscribers; the router no longer caches these — DB is authoritative.
pub async fn list_library_snapshots(
    db: &sea_orm::DatabaseConnection,
) -> Vec<crate::routing::LibrarySnapshot> {
    let libs = match repo::list_libraries(db).await {
        Ok(v) => v,
        Err(e) => {
            log::warn!("list_libraries: {e:#}");
            return Vec::new();
        }
    };
    let mut out = Vec::with_capacity(libs.len());
    for lib in libs {
        let plugin_name = repo::find_plugin_by_id(db, lib.plugin_id)
            .await
            .ok()
            .flatten()
            .map(|p| p.name)
            .unwrap_or_default();
        out.push(crate::routing::LibrarySnapshot {
            id: lib.id,
            path: lib.path,
            plugin_name,
        });
    }
    out.sort_by_key(|l| l.id);
    out
}

/// Query the DB for every registered library, grouped by the plugin
/// name that owns it. Feeds per-plugin library paths into Lua's
/// `ctx.libraries()` and seeds `protected_libraries` on sync so an
/// empty scan doesn't nuke user-configured folders.
pub async fn libraries_by_plugin_name(
    db: &sea_orm::DatabaseConnection,
) -> Result<HashMap<String, Vec<String>>> {
    let libs = repo::list_libraries(db).await?;
    let mut by_plugin_id: HashMap<i64, Vec<String>> = HashMap::new();
    for lib in libs {
        by_plugin_id.entry(lib.plugin_id).or_default().push(lib.path);
    }
    let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
    for (pid, paths) in by_plugin_id {
        if let Ok(Some(p)) = repo::find_plugin_by_id(db, pid).await {
            by_name.insert(p.name, paths);
        }
    }
    Ok(by_name)
}

/// Re-scan every loaded source plugin against the current DB library
/// set and persist the resulting entries. Returns the playlist size.
/// Called from startup after plugins load, from manual `rescan`, and
/// from `LibraryAdd` / `LibraryRemove` so the in-memory snapshot and
/// DB stay consistent with the user-managed library list.
pub async fn refresh_sources(app: &Arc<AppState>) -> Result<usize> {
    let libs_by_plugin = libraries_by_plugin_name(&app.db).await?;

    let source_mgr = app.source_manager.clone();
    let libs_for_scan = libs_by_plugin.clone();
    let snapshot: Vec<WallpaperEntry> = tokio::task::spawn_blocking(move || {
        let mut sm = source_mgr.blocking_lock();
        sm.scan_all(&libs_for_scan)?;
        Ok::<_, anyhow::Error>(sm.list().to_vec())
    })
    .await
    .map_err(|e| anyhow!("source scan join: {e}"))??;

    let plugins = {
        let sm = app.source_manager.lock().await;
        sm.plugins().unwrap_or_default()
    };
    for info in &plugins {
        let entries: Vec<_> = snapshot
            .iter()
            .filter(|e| e.plugin_name == info.name)
            .cloned()
            .collect();
        let protected = libs_by_plugin
            .get(&info.name)
            .cloned()
            .unwrap_or_default();
        match sync::sync_plugin_entries(
            &app.db,
            sync::PluginRef { name: &info.name, version: &info.version },
            &entries,
            &protected,
        )
        .await
        {
            Ok((summary, _)) => log::info!(
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

    let count = snapshot.len();
    // Re-resolve the active playlist against the fresh snapshot. For
    // the All pseudo-playlist this just clones the snapshot ids;
    // smart playlists re-run their filter; curated playlists prune
    // members that no longer exist on disk.
    if let Err(e) =
        crate::playlist::resolve::rebind_locked(&app.db, &snapshot, &app.playlist).await
    {
        log::warn!("playlist rebind after refresh failed: {e:#}");
    }

    // Kick a one-shot probe drain so newly-imported items don't have
    // to wait for the next scheduler tick. `spawn_async_unique` collapses
    // overlapping refresh→probe bursts (e.g. a flurry of LibraryAdd
    // calls) into a single in-flight pass.
    let probe = app.probe.clone();
    let db = app.db.clone();
    app.tasks.spawn_async_unique(
        crate::tasks::TaskKind::Generic,
        "probe/refresh",
        "probe/post-refresh",
        async move {
            // run_pending emits its own info log; we only care about
            // surfacing the error here.
            crate::probe_task::run_pending(&db, probe, crate::probe_task::PROBE_REFRESH_BATCH)
                .await
                .map(|_| ())
        },
    );

    Ok(count)
}
