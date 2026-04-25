//! Background media-probe scheduler.
//!
//! Decoupled from the scan/sync hot path: when an item lands in the DB
//! without media metadata (size/width/height/format), this module runs
//! the probe out-of-band on a periodic tick (and once after each sync).
//!
//! Design constraints:
//!
//! - Only items whose path extension is in [`PROBABLE_EXTS`] are
//!   considered. Scenes (`.pkg`), Lua plugins, archives, etc. are
//!   silently skipped — `MediaProbe::probe` would return all-`None`
//!   for them anyway and we don't want to retry forever.
//! - Each item gets at most one probe attempt per [`PROBE_COOLDOWN`]
//!   window (tracked via `item.sync_at`); a permanently-unprobeable
//!   file (libavformat unavailable, or metadata-poor format) is not
//!   re-tried until cooldown elapses.
//! - The whole pass is idempotent and bounded — at most
//!   [`PROBE_BATCH`] rows per tick — so a freshly-imported giant
//!   library doesn't monopolize the blocking pool.
//!
//! The probe call itself can be slow (libavformat dlopen + file open),
//! so we spawn it onto `tokio::task::spawn_blocking` per item.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use sea_orm::DatabaseConnection;
use tokio::sync::watch;

use crate::media_probe::MediaProbe;
use crate::model::repo;
use crate::tasks::now_ms;

/// How often the scheduler wakes up to drain pending items.
pub const PROBE_TICK: Duration = Duration::from_secs(300);

/// Minimum gap between two probe attempts for the same item. Items
/// whose `sync_at` is newer than `now - PROBE_COOLDOWN` are skipped
/// even if their media columns are still NULL.
pub const PROBE_COOLDOWN: Duration = Duration::from_secs(6 * 60 * 60);

/// Hard cap on items processed per tick.
pub const PROBE_BATCH: usize = 64;

/// Larger cap used by the post-sync one-shot path so a fresh import is
/// drained quickly rather than one tick at a time.
pub const PROBE_REFRESH_BATCH: usize = 256;

/// Extensions we attempt to probe. Lowercased, no leading dot.
pub const PROBABLE_EXTS: &[&str] = &[
    "mp4", "mkv", "webm", "mov", "avi", "png", "jpg", "jpeg", "webp", "gif", "bmp", "tiff", "tif",
    "avif",
];

fn is_probable(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let lower = e.to_ascii_lowercase();
            PROBABLE_EXTS.iter().any(|p| *p == lower)
        })
        .unwrap_or(false)
}

/// Per-pass statistics. Returned by [`run_pending`] and emitted as a
/// structured info log line so operators can see at a glance how the
/// probe scheduler is progressing.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProbeStats {
    /// Total items the cooldown query returned (pre extension filter).
    pub candidates: usize,
    /// Items skipped because their extension is not in [`PROBABLE_EXTS`].
    pub skipped_extension: usize,
    /// Items handed to `MediaProbe::probe`.
    pub probed: usize,
    /// Items where the probe returned at least one of width / height.
    pub gained_dimensions: usize,
    /// Items where the probe returned a non-empty `format` string.
    pub gained_format: usize,
    /// Items whose DB write failed; counted but not fatal to the pass.
    pub write_errors: usize,
    /// Wall time the pass took, in milliseconds.
    pub elapsed_ms: u128,
}

/// Drain up to `max` pending items in one pass. Returns per-pass
/// statistics. Always emits an info log line with the same numbers
/// when at least one candidate was considered, so the user can tail
/// the daemon log to see scheduler progress.
pub async fn run_pending(
    db: &DatabaseConnection,
    probe: Arc<dyn MediaProbe>,
    max: usize,
) -> Result<ProbeStats> {
    let mut stats = ProbeStats::default();
    if max == 0 {
        return Ok(stats);
    }
    let started = std::time::Instant::now();
    let cooldown_cutoff = now_ms() - PROBE_COOLDOWN.as_millis() as i64;
    // Pull a generous candidate window from DB then extension-filter
    // in Rust so the SQL stays portable. A multiplier of 4 is enough
    // for the practical mix of probable / non-probable items.
    let candidates =
        repo::list_items_pending_probe(db, cooldown_cutoff, (max as u64).saturating_mul(4)).await?;
    stats.candidates = candidates.len();

    for (item, library_root) in candidates {
        if stats.probed >= max {
            break;
        }
        if !is_probable(&item.path) {
            stats.skipped_extension += 1;
            continue;
        }
        let abs = join_path(&library_root, &item.path);
        let probe_for_blocking = probe.clone();
        let abs_for_blocking = abs.clone();
        let meta = tokio::task::spawn_blocking(move || {
            probe_for_blocking.probe(&abs_for_blocking)
        })
        .await
        .map_err(|e| anyhow::anyhow!("probe join id={}: {e}", item.id))?;

        if meta.width.is_some() || meta.height.is_some() {
            stats.gained_dimensions += 1;
        }
        if meta.format.is_some() {
            stats.gained_format += 1;
        }

        if let Err(e) = repo::update_item_media(db, item.id, &meta).await {
            log::warn!(
                "probe write failed id={} path={}: {e:#}",
                item.id,
                abs
            );
            stats.write_errors += 1;
            continue;
        }
        stats.probed += 1;
    }

    stats.elapsed_ms = started.elapsed().as_millis();

    log::info!(
        target: "waywallen::probe_task",
        "probe pass done: candidates={} probed={} ext_skipped={} +dims={} +format={} errors={} took={}ms",
        stats.candidates,
        stats.probed,
        stats.skipped_extension,
        stats.gained_dimensions,
        stats.gained_format,
        stats.write_errors,
        stats.elapsed_ms,
    );
    Ok(stats)
}

/// Long-lived probe scheduler. Wakes every [`PROBE_TICK`] and drains
/// up to [`PROBE_BATCH`] items. Returns when `shutdown_rx` flips to
/// `true`.
pub async fn scheduler_loop(
    db: DatabaseConnection,
    probe: Arc<dyn MediaProbe>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    log::info!(
        "probe scheduler started (tick={:?}, cooldown={:?}, batch={})",
        PROBE_TICK,
        PROBE_COOLDOWN,
        PROBE_BATCH
    );
    let mut interval = tokio::time::interval(PROBE_TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // First tick fires immediately; let it run so newly-installed
    // items get probed promptly on daemon start.
    loop {
        tokio::select! {
            biased;
            res = shutdown_rx.changed() => {
                if res.is_err() || *shutdown_rx.borrow() {
                    log::info!("probe scheduler exiting (shutdown)");
                    return Ok(());
                }
            }
            _ = interval.tick() => {
                // run_pending logs its own structured info line on
                // every non-empty pass; we only need to surface failures.
                if let Err(e) = run_pending(&db, probe.clone(), PROBE_BATCH).await {
                    log::warn!("probe scheduler tick failed: {e:#}");
                }
            }
        }
    }
}

fn join_path(root: &str, rel: &str) -> String {
    let root = root.trim_end_matches('/');
    let rel = rel.trim_start_matches('/');
    if rel.is_empty() {
        root.to_owned()
    } else {
        format!("{root}/{rel}")
    }
}

// Re-export used by main.rs / control.rs so they don't need to know
// about the underlying HashMap detail.
#[allow(dead_code)]
pub(crate) type LibraryRootMap = HashMap<i64, String>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_probe::MediaMetadata;
    use crate::model::connect_url;
    use crate::model::repo::ItemUpsertArgs;
    use crate::model::sync::{sync_plugin_entries, PluginRef};
    use crate::wallpaper_type::WallpaperEntry;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct CaptureProbe {
        meta: MediaMetadata,
        seen: Mutex<Vec<String>>,
    }
    impl MediaProbe for CaptureProbe {
        fn probe(&self, path: &str) -> MediaMetadata {
            self.seen.lock().unwrap().push(path.to_owned());
            self.meta.clone()
        }
    }

    fn entry(plugin: &str, root: &str, resource: &str, ty: &str) -> WallpaperEntry {
        WallpaperEntry {
            id: resource.to_owned(),
            name: resource.to_owned(),
            wp_type: ty.to_owned(),
            resource: resource.to_owned(),
            preview: None,
            metadata: HashMap::new(),
            plugin_name: plugin.to_owned(),
            library_root: root.to_owned(),
            description: None,
            tags: Vec::new(),
            external_id: None,
            size: None,
            width: None,
            height: None,
            format: None,
        }
    }

    async fn mem_db() -> DatabaseConnection {
        connect_url("sqlite::memory:").await.unwrap()
    }

    #[test]
    fn extension_filter_accepts_video_and_image() {
        assert!(is_probable("foo.MP4"));
        assert!(is_probable("a/b/c.jpg"));
        assert!(is_probable("x.JPEG"));
        assert!(!is_probable("12345/scene.pkg"));
        assert!(!is_probable("noext"));
        assert!(!is_probable("init.lua"));
    }

    #[tokio::test]
    async fn run_pending_skips_non_probable_extensions() {
        let db = mem_db().await;
        // Two items: one .pkg (not probable), one .mp4 (probable).
        let entries = [
            entry("p", "/r", "/r/12345/scene.pkg", "scene"),
            entry("p", "/r", "/r/clip.mp4", "video"),
        ];
        let _ = sync_plugin_entries(
            &db,
            PluginRef { name: "p", version: "" },
            &entries,
            &[],
        )
        .await
        .unwrap();

        let probe = Arc::new(CaptureProbe {
            meta: MediaMetadata {
                size: Some(123),
                width: Some(640),
                height: Some(480),
                format: Some("mp4".to_owned()),
            },
            seen: Mutex::new(Vec::new()),
        });
        // Make sure cooldown does not skip rows we just inserted.
        // sync_plugin_entries stamps sync_at = now_ms(), so the first
        // pass would normally skip them; bypass via a far-future cutoff.
        let cutoff = now_ms() + 1;
        let candidates = repo::list_items_pending_probe(&db, cutoff, 100).await.unwrap();
        assert_eq!(candidates.len(), 2, "both rows should be candidates pre-filter");

        // Now invoke the public path. With the real `now()` cutoff the
        // brand-new rows will be skipped (cooldown), so manually
        // backdate sync_at on the .mp4 to force it into the window.
        backdate_sync_at(&db).await;

        let stats = run_pending(&db, probe.clone(), 10).await.unwrap();
        assert_eq!(stats.probed, 1, "only mp4 should have been probed");
        assert_eq!(stats.skipped_extension, 1, "scene.pkg must be ext-skipped");
        let seen = probe.seen.lock().unwrap().clone();
        assert_eq!(seen, vec!["/r/clip.mp4".to_owned()]);
    }

    #[tokio::test]
    async fn run_pending_writes_meta_and_advances_sync_at() {
        let db = mem_db().await;
        let _ = sync_plugin_entries(
            &db,
            PluginRef { name: "p", version: "" },
            &[entry("p", "/r", "/r/clip.mp4", "video")],
            &[],
        )
        .await
        .unwrap();
        backdate_sync_at(&db).await;

        let probe = Arc::new(CaptureProbe {
            meta: MediaMetadata {
                size: Some(7777),
                width: None,
                height: None,
                format: None,
            },
            seen: Mutex::new(Vec::new()),
        });
        let stats = run_pending(&db, probe, 10).await.unwrap();
        assert_eq!(stats.probed, 1);

        let plugin = repo::find_plugin_by_name(&db, "p").await.unwrap().unwrap();
        let items = repo::list_items_by_plugin(&db, plugin.id).await.unwrap();
        let it = &items[0];
        assert_eq!(it.size, Some(7777));
        assert!(it.width.is_none(), "missing field stays None");
        // sync_at must be near now (well above the backdate value).
        let now = now_ms();
        assert!(it.sync_at >= now - 5_000, "sync_at not advanced: {} vs {}", it.sync_at, now);
        // update_at must also advance because size actually changed.
        assert!(it.update_at >= it.create_at);
    }

    #[tokio::test]
    async fn run_pending_no_op_probe_only_advances_sync_at() {
        let db = mem_db().await;
        let _ = sync_plugin_entries(
            &db,
            PluginRef { name: "p", version: "" },
            &[entry("p", "/r", "/r/clip.mp4", "video")],
            &[],
        )
        .await
        .unwrap();
        let plugin = repo::find_plugin_by_name(&db, "p").await.unwrap().unwrap();
        let before = repo::list_items_by_plugin(&db, plugin.id).await.unwrap();
        let before_update = before[0].update_at;
        backdate_sync_at(&db).await;

        let probe = Arc::new(CaptureProbe {
            meta: MediaMetadata::default(),
            seen: Mutex::new(Vec::new()),
        });
        let stats = run_pending(&db, probe, 10).await.unwrap();
        assert_eq!(stats.probed, 1);
        assert_eq!(stats.gained_dimensions, 0);
        assert_eq!(stats.gained_format, 0);

        let after = repo::list_items_by_plugin(&db, plugin.id).await.unwrap();
        let it = &after[0];
        // Probe returned all-None: media columns unchanged, update_at
        // unchanged, sync_at advanced.
        assert_eq!(it.size, None);
        assert_eq!(it.update_at, before_update);
        assert!(it.sync_at >= now_ms() - 5_000);
    }

    /// Test helper kept around for cooldown scenarios that should be
    /// exempted: backdates every row's `sync_at` so any sync_at-based
    /// filter would consider them eligible. Probe cooldown now keys
    /// off `probed_at`, so most callers don't need this anymore.
    #[allow(dead_code)]
    async fn backdate_sync_at(db: &DatabaseConnection) {
        use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
        db.execute(Statement::from_string(
            DatabaseBackend::Sqlite,
            "UPDATE item SET sync_at = 0",
        ))
        .await
        .unwrap();
    }

    /// Regression: a freshly-synced row (probed_at IS NULL,
    /// sync_at = now) MUST be picked up by the post-refresh probe
    /// drain even though its sync_at is well within any cooldown
    /// window. Earlier the cooldown gated on sync_at, which excluded
    /// every just-imported item.
    ///
    /// Second half asserts the cooldown actually fires once probed_at
    /// is stamped: a no-op probe leaves all media columns NULL (so
    /// the outer "any media missing" filter still matches) but the
    /// row must NOT be re-picked because probed_at is fresh.
    #[tokio::test]
    async fn run_pending_picks_fresh_never_probed_items() {
        let db = mem_db().await;
        let _ = sync_plugin_entries(
            &db,
            PluginRef { name: "p", version: "" },
            &[entry("p", "/r", "/r/clip.mp4", "video")],
            &[],
        )
        .await
        .unwrap();
        // No backdate — sync_at is now, but probed_at is NULL.
        let probe = Arc::new(CaptureProbe {
            meta: MediaMetadata::default(),
            seen: Mutex::new(Vec::new()),
        });
        let stats = run_pending(&db, probe, 10).await.unwrap();
        assert_eq!(stats.candidates, 1, "fresh item must be a candidate");
        assert_eq!(stats.probed, 1);

        // Second pass: same row, still all-NULL media (no-op probe
        // didn't change anything) — must be skipped by probed_at
        // cooldown, NOT re-probed.
        let probe2 = Arc::new(CaptureProbe {
            meta: MediaMetadata::default(),
            seen: Mutex::new(Vec::new()),
        });
        let stats2 = run_pending(&db, probe2, 10).await.unwrap();
        assert_eq!(
            stats2.candidates, 0,
            "second pass must hit cooldown via probed_at"
        );
    }

    // Silence "imported-but-unused" if a future refactor removes the
    // direct repo calls — keep the path live.
    #[allow(dead_code)]
    fn _typecheck_upsert_args(args: ItemUpsertArgs<'_>) -> ItemUpsertArgs<'_> {
        args
    }
}
