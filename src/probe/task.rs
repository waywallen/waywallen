use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use sea_orm::{DatabaseConnection, TransactionTrait};
use tokio::sync::watch;

use crate::error::{Result, ResultExt};
use crate::model::entities::item;
use crate::model::repo;
use crate::probe::media::{MediaMeta, MediaProbe};
use crate::probe::stat::{self, FileStat};

/// How often the scheduler wakes up to drain pending items.
pub const PROBE_TICK: Duration = Duration::from_secs(300);

/// How many probed items to accumulate before flushing them in a single
/// SQLite transaction. Amortizes WAL fsync cost across many `UPDATE`s.
pub const COMMIT_BATCH: usize = 32;

/// Extensions libavformat can probe. Lowercased, no leading dot.
/// Used by [`repo::list_items_needing_probe`] to prefilter DB rows.
pub const PROBABLE_EXTS: &[&str] = &[
    "mp4", "mkv", "webm", "mov", "avi", "png", "jpg", "jpeg", "webp", "gif", "bmp", "tiff", "tif",
    "avif",
];

#[derive(Debug, Clone, Copy, Default)]
pub struct ProbeStats {
    pub stat_done: usize,
    pub stat_changed: usize,
    pub media_probed: usize,
    pub gained_dimensions: usize,
    pub write_errors: usize,
    pub elapsed_ms: u128,
}

struct WorkItem {
    item: item::Model,
    lib_path: String,
    do_stat: bool,
    do_probe: bool,
}

/// One item's probe result, queued up to be flushed in the next batch
/// commit. `path` is kept around purely for error-log context.
struct PendingWrite {
    id: i64,
    path: String,
    stat: Option<FileStat>,
    media: Option<MediaMeta>,
}

/// Drain everything that currently needs work in a single pass. Two
/// unbounded SQL queries (stat-pending and probe-pending) are merged
pub async fn run_pending(
    db: &DatabaseConnection,
    probe: Arc<dyn MediaProbe>,
) -> Result<ProbeStats> {
    run_pending_notifying(db, probe, None).await
}

pub async fn run_pending_notifying(
    db: &DatabaseConnection,
    probe: Arc<dyn MediaProbe>,
    changes: Option<&watch::Sender<u64>>,
) -> Result<ProbeStats> {
    let mut stats = ProbeStats::default();
    let started = std::time::Instant::now();

    let stat_rows = repo::list_items_needing_stat(db).await?;
    let probe_rows = repo::list_items_needing_probe(db, PROBABLE_EXTS).await?;

    let mut work: HashMap<i64, WorkItem> = HashMap::new();
    for (item, lib_path) in stat_rows {
        work.insert(
            item.id,
            WorkItem {
                item,
                lib_path,
                do_stat: true,
                do_probe: false,
            },
        );
    }
    for (item, lib_path) in probe_rows {
        work.entry(item.id)
            .and_modify(|w| w.do_probe = true)
            .or_insert(WorkItem {
                item,
                lib_path,
                do_stat: false,
                do_probe: true,
            });
    }

    let mut pending: Vec<PendingWrite> = Vec::with_capacity(COMMIT_BATCH);
    for w in work.into_values() {
        let abs = join_path(&w.lib_path, &w.item.path);

        let mut stat_result: Option<FileStat> = None;
        if w.do_stat {
            let abs_for_blocking = abs.clone();
            let s = tokio::task::spawn_blocking(move || stat::stat_file(&abs_for_blocking))
                .await
                .with_context(|| format!("stat join id={}", w.item.id))?;
            if let Some(s) = s {
                stat_result = Some(s);
            }
        }

        let mut media_result: Option<MediaMeta> = None;
        if w.do_probe {
            let probe_for_blocking = probe.clone();
            let abs_for_blocking = abs.clone();
            let meta = tokio::task::spawn_blocking(move || {
                probe_for_blocking.probe_media(&abs_for_blocking)
            })
            .await
            .with_context(|| format!("probe join id={}", w.item.id))?;
            if meta.width.is_some() || meta.height.is_some() {
                stats.gained_dimensions += 1;
            }
            media_result = Some(meta);
        }

        if stat_result.is_some() || media_result.is_some() {
            pending.push(PendingWrite {
                id: w.item.id,
                path: abs,
                stat: stat_result,
                media: media_result,
            });
            if pending.len() >= COMMIT_BATCH {
                flush_pending(db, &mut pending, &mut stats, changes).await;
            }
        }
    }

    flush_pending(db, &mut pending, &mut stats, changes).await;

    stats.elapsed_ms = started.elapsed().as_millis();

    log::info!(
        target: "waywallen::probe::task",
        "probe pass done: stat_done={} stat_changed={} media_probed={} +dims={} errors={} took={}ms",
        stats.stat_done,
        stats.stat_changed,
        stats.media_probed,
        stats.gained_dimensions,
        stats.write_errors,
        stats.elapsed_ms,
    );
    Ok(stats)
}

pub async fn scheduler_loop(
    db: DatabaseConnection,
    probe: Arc<dyn MediaProbe>,
    mut shutdown_rx: watch::Receiver<bool>,
    changes: watch::Sender<u64>,
) -> Result<()> {
    log::info!("probe scheduler started (tick={:?})", PROBE_TICK);
    let mut interval = tokio::time::interval(PROBE_TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
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
                if let Err(e) = run_pending_notifying(&db, probe.clone(), Some(&changes)).await {
                    log::warn!("probe scheduler tick failed: {e:#}");
                }
            }
        }
    }
}

/// Commit a buffered batch of stat/media writes in one transaction.
/// Any per-item or commit error rolls back the whole transaction.
async fn flush_pending(
    db: &DatabaseConnection,
    pending: &mut Vec<PendingWrite>,
    stats: &mut ProbeStats,
    changes: Option<&watch::Sender<u64>>,
) {
    if pending.is_empty() {
        return;
    }
    let n = pending.len();
    let mut delta_stat_done = 0usize;
    let mut delta_stat_changed = 0usize;
    let mut delta_media_probed = 0usize;

    let outcome: Result<()> = async {
        let tx = db.begin().await.context("begin probe tx")?;
        for pw in pending.iter() {
            if let Some(s) = &pw.stat {
                let out = repo::update_item_stat(&tx, pw.id, s)
                    .await
                    .with_context(|| format!("stat write id={} path={}", pw.id, pw.path))?;
                delta_stat_done += 1;
                if out.changed {
                    delta_stat_changed += 1;
                }
            }
            if let Some(m) = &pw.media {
                repo::update_item_media(&tx, pw.id, m)
                    .await
                    .with_context(|| format!("probe write id={} path={}", pw.id, pw.path))?;
                delta_media_probed += 1;
            }
        }
        tx.commit().await.context("commit probe tx")?;
        Ok(())
    }
    .await;

    match outcome {
        Ok(_) => {
            if delta_stat_changed > 0 || delta_media_probed > 0 {
                if let Some(changes) = changes {
                    changes.send_modify(|generation| *generation += 1);
                }
            }
            stats.stat_done += delta_stat_done;
            stats.stat_changed += delta_stat_changed;
            stats.media_probed += delta_media_probed;
        }
        Err(e) => {
            log::warn!("probe batch flush failed (n={n}): {e:#}");
            stats.write_errors += n;
        }
    }
    pending.clear();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn probe_batch_notifies_after_metadata_commit() {
        let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
        let plugin = repo::upsert_plugin(&db, "probe-test", "").await.unwrap();
        let library = repo::add_library(&db, plugin.id, "/probe-test")
            .await
            .unwrap();
        let item = repo::upsert_item(
            &db,
            repo::ItemUpsertArgs {
                plugin_id: plugin.id,
                library_id: library.id,
                path: "preview.png",
                ty: "image",
                sub_type: "",
                display_name: "preview",
                preview_path: None,
                description: None,
                external_id: None,
                web_url: None,
                update_available: false,
                size: None,
                width: None,
                height: None,
                content_rating: None,
                can_remove: None,
            },
        )
        .await
        .unwrap();
        let (changes, rx) = watch::channel(0);
        let mut stats = ProbeStats::default();
        let mut pending = vec![PendingWrite {
            id: item.id,
            path: "preview.png".into(),
            stat: None,
            media: Some(MediaMeta {
                width: Some(1920),
                height: Some(1080),
            }),
        }];
        flush_pending(&db, &mut pending, &mut stats, Some(&changes)).await;
        assert_eq!(*rx.borrow(), 1);
        assert_eq!(
            repo::get_entry(&db, item.id).await.unwrap().unwrap().width,
            Some(1920)
        );
        flush_pending(&db, &mut pending, &mut stats, Some(&changes)).await;
        assert_eq!(*rx.borrow(), 1);
        assert_eq!(stats.write_errors, 0);
    }
}
