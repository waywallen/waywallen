use std::collections::HashMap;
use std::sync::Arc;

use anyhow::anyhow;

use crate::catalog::entry::WallpaperEntry;
use crate::error::{Error, Result};
use crate::model::{repo, sync};
use crate::DaemonContext;

mod libraries;
mod query;
mod remote;
mod watch;

pub use libraries::{add_library, edit_library, rename_library};
pub use query::ordered_entry_ids;
pub use remote::{
    download_remote, publish_remote_download_progress, remote_capability,
    remove_wallpaper_entry_files_and_db, resolve_remote_source_id,
};
pub use watch::run_library_watcher;

pub async fn rescan(app: &Arc<DaemonContext>) -> Result<usize> {
    refresh_sources(app).await
}

/// Run source-plugin auto-detect and register any discovered libraries.
/// Duplicate libraries are skipped before a refresh is triggered.
pub async fn auto_detect_libraries(
    app: &Arc<DaemonContext>,
) -> Result<Vec<crate::wallframe::routing::LibrarySnapshot>> {
    use crate::wallframe::routing::LibrarySnapshot;

    let detected = app.source_manager.auto_detect_all().await?;
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
        for candidate in paths {
            let path = match libraries::library_directory(&candidate.path).await {
                Ok(path) => path,
                Err(error) => {
                    log::warn!("auto_detect: {error}");
                    continue;
                }
            };
            match repo::find_library(&app.db, plugin.id, &path).await {
                Ok(Some(_)) => continue,
                Ok(None) => {}
                Err(e) => {
                    log::warn!("auto_detect: find_library({path}): {e:#}");
                    continue;
                }
            }
            match repo::add_configured_library(&app.db, plugin.id, &path, None, &candidate.settings)
                .await
            {
                Ok(lib) => {
                    let snap = LibrarySnapshot {
                        settings: candidate.settings,
                        id: lib.id,
                        name: lib.name,
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
        app.events
            .publish(crate::events::GlobalEvent::LibrariesAdded {
                paths: added.iter().map(|s| s.path.clone()).collect(),
            });
    }

    if !added.is_empty() {
        let app_clone = app.clone();
        app.tasks.spawn_async_unique(
            crate::tasks::TaskKind::Generic,
            "catalog/refresh",
            "catalog/auto-detect-refresh",
            async move {
                refresh_sources(&app_clone)
                    .await
                    .map(|_| ())
                    .map_err(anyhow::Error::from)
            },
        );
    }
    Ok(added)
}

/// Load DB libraries into the router-wire `LibrarySnapshot` shape.
/// Used by library list queries and the initial WS snapshot.
pub async fn list_library_snapshots(
    db: &sea_orm::DatabaseConnection,
) -> Vec<crate::wallframe::routing::LibrarySnapshot> {
    let libs = match repo::list_libraries(db).await {
        Ok(v) => v,
        Err(e) => {
            log::warn!("list_libraries: {e:#}");
            return Vec::new();
        }
    };
    let mut out = Vec::with_capacity(libs.len());
    for lib in libs {
        let metadata = crate::model::repo::get_library_metadata(db, lib.id)
            .await
            .unwrap_or_default();
        if metadata
            .get(crate::model::repo::LIBRARY_METADATA_MANAGED_KEY)
            .is_some_and(|v| v == crate::model::repo::LIBRARY_METADATA_MANAGED_REMOTE)
        {
            continue;
        }
        let plugin_name = repo::find_plugin_by_id(db, lib.plugin_id)
            .await
            .ok()
            .flatten()
            .map(|p| p.name)
            .unwrap_or_default();
        let settings = repo::library_settings(&lib).unwrap_or_default();
        out.push(crate::wallframe::routing::LibrarySnapshot {
            settings,
            id: lib.id,
            name: lib.name,
            path: lib.path,
            plugin_name,
        });
    }
    out.sort_by_key(|l| l.id);
    out
}

/// Deduplicate paths by canonical target, preserving first-seen order.
/// Unresolvable paths fall back to their raw string.
fn dedup_paths_by_canonical(paths: &[String]) -> Vec<String> {
    use std::collections::HashSet;
    let mut seen: HashSet<std::path::PathBuf> = HashSet::new();
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        let canon = repo::library_path_key(p);
        if seen.insert(canon) {
            out.push(p.clone());
        }
    }
    out
}

pub async fn libraries_by_plugin_name(
    db: &sea_orm::DatabaseConnection,
) -> Result<HashMap<String, Vec<String>>> {
    let libs = repo::list_libraries(db).await?;
    let mut by_plugin_id: HashMap<i64, Vec<String>> = HashMap::new();
    for lib in libs {
        by_plugin_id
            .entry(lib.plugin_id)
            .or_default()
            .push(lib.path);
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
pub async fn refresh_source_plugins(app: &Arc<DaemonContext>) {
    let plugins = match app.source_manager.plugins() {
        Ok(p) => p,
        Err(e) => {
            log::warn!("refresh_source_plugins: source_manager.plugins() failed: {e:#}");
            Vec::new()
        }
    };
    *app.source_plugins.write().await = plugins;
}

pub async fn refresh_sources(app: &Arc<DaemonContext>) -> Result<usize> {
    refresh_selected_sources(app, None).await
}

async fn refresh_source(app: &Arc<DaemonContext>, name: &str) -> Result<usize> {
    refresh_selected_sources(app, Some(name)).await
}

async fn refresh_selected_sources(
    app: &Arc<DaemonContext>,
    selected: Option<&str>,
) -> Result<usize> {
    let _invalidate = super::wallpaper_query::InvalidateOnDrop(app.wallpaper_changes.clone());
    use std::sync::atomic::Ordering;
    app.scan_in_progress.store(true, Ordering::SeqCst);
    // Sync start is observable to UIs via `StatusSync.scan_in_progress`.
    app.events
        .publish(crate::events::GlobalEvent::StatusChanged);

    let result = refresh_sources_inner(app, selected).await;

    app.scan_in_progress.store(false, Ordering::SeqCst);
    match &result {
        Ok(count) => app
            .events
            .publish(crate::events::GlobalEvent::SyncFinished { count: *count }),
        Err(e) => app
            .events
            .publish(crate::events::GlobalEvent::SyncFailed(format!("{e:#}"))),
    }
    app.events
        .publish(crate::events::GlobalEvent::StatusChanged);
    result
}

pub async fn notify_wallpaper_db_changed(app: &Arc<DaemonContext>, count: usize) {
    app.wallpaper_changes
        .send_modify(|generation| *generation += 1);
    app.queue.lock().await.reset_shuffle_round();

    let probe = app.probe.clone();
    let db = app.db.clone();
    let changes = app.wallpaper_changes.clone();
    app.tasks.spawn_async_unique(
        crate::tasks::TaskKind::Generic,
        "probe/refresh",
        "probe/post-db-change",
        async move {
            crate::probe::task::run_pending_notifying(&db, probe, Some(&changes))
                .await
                .map(|_| ())
                .map_err(anyhow::Error::from)
        },
    );

    app.events
        .publish(crate::events::GlobalEvent::SyncFinished { count });
}

async fn refresh_sources_inner(app: &Arc<DaemonContext>, selected: Option<&str>) -> Result<usize> {
    let libs_by_plugin = libraries_by_plugin_name(&app.db).await?;

    let source_mgr = app.source_manager.clone();
    // Scan each physical directory once so symlinked aliases do not emit
    // duplicate entries and duplicate UI rows.
    let libs_for_scan: HashMap<String, Vec<String>> = libs_by_plugin
        .iter()
        .map(|(name, paths)| (name.clone(), dedup_paths_by_canonical(paths)))
        .collect();
    // Hold the Lua VM lock only during the scan; wallpaper reads hit the DB
    // and do not wait behind this section.
    let handle = tokio::runtime::Handle::current();
    // A failing source plugin must not discard what the others found, so the
    // scan error is carried alongside the snapshot and only surfaced once the
    // successful entries have been synced.
    let selected_owned = selected.map(str::to_owned);
    let (snapshot, scan_error): (Vec<WallpaperEntry>, Option<Error>) =
        tokio::task::spawn_blocking(move || {
            let scan_error = if let Some(name) = selected_owned {
                handle
                    .block_on(
                        source_mgr.scan_source(
                            &name,
                            libs_for_scan
                                .get(&name)
                                .map(Vec::as_slice)
                                .unwrap_or_default(),
                        ),
                    )
                    .err()
            } else {
                handle.block_on(source_mgr.scan_all(&libs_for_scan)).err()
            };
            (source_mgr.list(), scan_error)
        })
        .await
        .map_err(|e| Error::Internal(anyhow!("source scan join: {e}")))?;

    let plugins = match app.source_manager.plugins() {
        Ok(p) => p,
        Err(e) => {
            log::warn!("refresh_sources: source_manager.plugins() failed: {e:#}");
            Vec::new()
        }
    };

    // Sync to the DB first so every entry gets its canonical item id before
    // readers observe the refreshed source-plugin list.
    for info in plugins
        .iter()
        .filter(|info| selected.is_none_or(|name| name == info.name))
    {
        let entries: Vec<_> = snapshot
            .iter()
            .filter(|e| e.plugin_name == info.name)
            .cloned()
            .collect();
        // Only reachable registered roots are swept; missing roots are spared
        // so unmounted libraries do not lose their items.
        let present: Vec<String> = libs_by_plugin
            .get(&info.name)
            .map(|paths| {
                paths
                    .iter()
                    .filter(|p| std::path::Path::new(p.as_str()).exists())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        match sync::sync_plugin_entries(
            &app.db,
            sync::PluginRef {
                name: &info.name,
                version: &info.version,
            },
            &entries,
            &present,
        )
        .await
        {
            Ok((summary, _)) => log::info!(
                "sync plugin={} v{}: +{} / -{} items, {} dropped",
                info.name,
                info.version,
                summary.items_upserted,
                summary.items_deleted,
                summary.dropped,
            ),
            Err(e) => log::warn!("sync plugin={} failed: {e:#}", info.name),
        }
        app.wallpaper_changes
            .send_modify(|generation| *generation += 1);
    }

    // Scan results are now persisted in the DB (the read source of
    // truth); only the source-plugin list is cached in memory.
    let count = snapshot.len();
    *app.source_plugins.write().await = plugins;
    // Queue reads from the DB dynamically; reset the shuffle round so the
    // next pick can include freshly imported items.
    app.queue.lock().await.reset_shuffle_round();

    // Kick one probe drain for newly imported items; spawn_async_unique
    // collapses refresh bursts into one in-flight pass.
    let probe = app.probe.clone();
    let db = app.db.clone();
    let changes = app.wallpaper_changes.clone();
    app.tasks.spawn_async_unique(
        crate::tasks::TaskKind::Generic,
        "probe/refresh",
        "probe/post-refresh",
        async move {
            crate::probe::task::run_pending_notifying(&db, probe, Some(&changes))
                .await
                .map(|_| ())
                .map_err(anyhow::Error::from)
        },
    );

    if let Some(e) = scan_error {
        return Err(e);
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_deduplicates_library_aliases_and_preserves_first_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("wallpapers");
        let alias = dir.path().join("alias");
        std::fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let first = format!("{}/./", alias.display());
        let missing = dir.path().join("missing").to_str().unwrap().to_owned();
        assert_eq!(
            dedup_paths_by_canonical(&[
                first.clone(),
                root.to_str().unwrap().to_owned(),
                alias.to_str().unwrap().to_owned(),
                missing.clone(),
                format!("{missing}/"),
            ]),
            vec![first, missing]
        );
    }
}
