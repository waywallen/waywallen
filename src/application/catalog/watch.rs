use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::error::Result;
use crate::model::repo;
use crate::DaemonContext;

/// How often the library directories are checked for changes.
pub const LIBRARY_WATCH_TICK: Duration = Duration::from_secs(60);

/// Directory mtimes from one pass. `None` means the directory was unreadable.
type DirTimes = BTreeMap<PathBuf, Option<SystemTime>>;

/// Decides from directory mtimes alone whether a rescan is due.
#[derive(Default)]
struct LibraryWatch {
    seen: Option<DirTimes>,
    recheck: bool,
}

impl LibraryWatch {
    /// The first pass only records. A directory that changed since the last
    /// pass asks for a rescan now and once more on the next pass, so an item
    /// that was still being copied is picked up complete. Directories that
    /// are new to the set come from a rescan and are not a change.
    fn observe(&mut self, now: DirTimes) -> bool {
        let changed = self.seen.as_ref().is_some_and(|seen| {
            seen.iter()
                .any(|(dir, was)| now.get(dir).is_some_and(|is| is != was))
        });
        let rescan = changed || self.recheck;
        self.recheck = changed;
        self.seen = Some(now);
        rescan
    }
}

/// Directories whose mtime moves when an item is added to or removed from
/// `root`: the directory of every known item and the one above it. An empty
/// library has only the root to look at.
fn watched_dirs<'a>(root: &Path, items: impl Iterator<Item = &'a str>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for rel in items {
        let item = root.join(rel.trim_start_matches('/'));
        dirs.extend(
            item.ancestors()
                .skip(1)
                .take(2)
                .filter(|dir| dir.starts_with(root))
                .map(Path::to_path_buf),
        );
    }
    if dirs.is_empty() {
        dirs.push(root.to_path_buf());
    }
    dirs
}

async fn library_dir_times(app: &Arc<DaemonContext>) -> Result<DirTimes> {
    let mut items_by_library: HashMap<i64, Vec<String>> = HashMap::new();
    for item in repo::list_items_all(&app.db).await? {
        items_by_library
            .entry(item.library_id)
            .or_default()
            .push(item.path);
    }
    let mut dirs = Vec::new();
    for lib in repo::list_libraries(&app.db).await? {
        // Remote-managed libraries are written by the daemon itself.
        let metadata = repo::get_library_metadata(&app.db, lib.id)
            .await
            .unwrap_or_default();
        if metadata
            .get(repo::LIBRARY_METADATA_MANAGED_KEY)
            .is_some_and(|v| v == repo::LIBRARY_METADATA_MANAGED_REMOTE)
        {
            continue;
        }
        let items = items_by_library.remove(&lib.id).unwrap_or_default();
        dirs.extend(watched_dirs(
            Path::new(&lib.path),
            items.iter().map(String::as_str),
        ));
    }
    let times = tokio::task::spawn_blocking(move || {
        dirs.into_iter()
            .map(|dir| {
                let mtime = std::fs::metadata(&dir).and_then(|m| m.modified()).ok();
                (dir, mtime)
            })
            .collect()
    })
    .await?;
    Ok(times)
}

/// Rescan when a library directory changed on disk, so wallpapers that
/// arrive while the daemon runs (a Workshop download, a copied file) show up
/// without a manual refresh.
pub async fn run_library_watcher(
    app: Arc<DaemonContext>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let mut watch = LibraryWatch::default();
    let mut interval = tokio::time::interval(LIBRARY_WATCH_TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            res = shutdown.changed() => {
                if res.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
            _ = interval.tick() => {
                let now = match library_dir_times(&app).await {
                    Ok(now) => now,
                    Err(e) => {
                        log::warn!("library watch tick failed: {e:#}");
                        continue;
                    }
                };
                if !watch.observe(now) {
                    continue;
                }
                if app.scan_in_progress.load(Ordering::SeqCst) {
                    // Let the running scan finish and look again next pass.
                    watch.recheck = true;
                    continue;
                }
                log::info!("library changed on disk; rescanning");
                if let Err(e) = super::rescan(&app).await {
                    log::warn!("library watch rescan failed: {e:#}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn times(dirs: &[(&str, Option<u64>)]) -> DirTimes {
        dirs.iter()
            .map(|(dir, secs)| {
                let mtime = secs.map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(s));
                (PathBuf::from(dir), mtime)
            })
            .collect()
    }

    #[test]
    fn first_pass_only_records() {
        let mut watch = LibraryWatch::default();
        assert!(!watch.observe(times(&[("/lib", Some(1))])));
        assert!(!watch.observe(times(&[("/lib", Some(1))])));
    }

    #[test]
    fn changed_mtime_rescans_now_and_on_the_next_pass() {
        let mut watch = LibraryWatch::default();
        assert!(!watch.observe(times(&[("/lib", Some(1))])));
        assert!(watch.observe(times(&[("/lib", Some(2))])));
        assert!(watch.observe(times(&[("/lib", Some(2))])));
        assert!(!watch.observe(times(&[("/lib", Some(2))])));
    }

    #[test]
    fn vanished_directory_is_a_change() {
        let mut watch = LibraryWatch::default();
        assert!(!watch.observe(times(&[("/lib", Some(1)), ("/lib/a", Some(1))])));
        assert!(watch.observe(times(&[("/lib", Some(1)), ("/lib/a", None)])));
    }

    #[test]
    fn directories_added_or_dropped_by_a_rescan_are_not_a_change() {
        let mut watch = LibraryWatch::default();
        assert!(!watch.observe(times(&[("/lib", Some(1)), ("/lib/a", Some(1))])));
        assert!(!watch.observe(times(&[("/lib", Some(1)), ("/lib/b", Some(5))])));
        assert!(!watch.observe(times(&[("/lib", Some(1)), ("/lib/b", Some(5))])));
    }

    #[test]
    fn watches_the_item_directory_and_the_one_above_it() {
        let dirs = watched_dirs(
            Path::new("/steam"),
            ["content/431960/1/scene.pkg", "content/431960/2/scene.pkg"].into_iter(),
        );
        assert_eq!(
            dirs,
            [
                "/steam/content/431960/1",
                "/steam/content/431960",
                "/steam/content/431960/2",
                "/steam/content/431960",
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn stays_inside_the_library_root() {
        let dirs = watched_dirs(Path::new("/pictures"), ["a.png"].into_iter());
        assert_eq!(dirs, [PathBuf::from("/pictures")]);
    }

    #[test]
    fn empty_library_watches_its_root() {
        let dirs = watched_dirs(Path::new("/pictures"), std::iter::empty());
        assert_eq!(dirs, [PathBuf::from("/pictures")]);
    }
}
