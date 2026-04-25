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

use anyhow::{anyhow, Result};

use crate::ipc::proto::ControlMsg;
use crate::model::{repo, sync};
use crate::renderer_manager;
use crate::wallpaper_type::WallpaperEntry;
use crate::AppState;

#[derive(Default)]
pub struct PlaylistState {
    pub ids: Vec<String>,
    pub cursor: usize,
    pub current: Option<String>,
}

impl PlaylistState {
    pub fn refresh(&mut self, ids: Vec<String>) {
        self.ids = ids;
        if self.ids.is_empty() {
            self.cursor = 0;
        } else if self.cursor >= self.ids.len() {
            self.cursor = 0;
        }
    }

    pub fn locate(&mut self, id: &str) {
        if let Some(pos) = self.ids.iter().position(|x| x == id) {
            self.cursor = pos;
        }
    }
}

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

    Ok(ApplyResult { renderer_id, entry })
}

/// Advance the playlist cursor by `delta` and apply the result.
pub async fn step(app: &Arc<AppState>, delta: i32) -> Result<String> {
    let ids: Vec<String> = {
        let mgr = app.source_manager.lock().await;
        mgr.list().iter().map(|e| e.id.clone()).collect()
    };

    if ids.is_empty() {
        return Err(anyhow!("playlist is empty"));
    }

    let next_id = {
        let mut playlist = app.playlist.lock().await;
        let current = playlist.current.clone();
        playlist.refresh(ids);
        if let Some(cur) = current.as_deref() {
            playlist.locate(cur);
        }
        let len = playlist.ids.len() as i32;
        let mut next = (playlist.cursor as i32 + delta) % len;
        if next < 0 {
            next += len;
        }
        playlist.cursor = next as usize;
        playlist.ids[playlist.cursor].clone()
    };

    apply_wallpaper_by_id(app, &next_id, 0, 0, 0).await?;
    Ok(next_id)
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
            &*app.probe,
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

    let ids: Vec<String> = snapshot.iter().map(|e| e.id.clone()).collect();
    let count = ids.len();
    app.playlist.lock().await.refresh(ids);
    Ok(count)
}
