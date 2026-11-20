use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use crate::wallframe::routing::RouterEvent;
use crate::wallframe::scheduler::DisplayId;
use crate::DaemonContext;

const SETTLE: Duration = Duration::from_secs(2);
const IDLE_PARK: Duration = Duration::from_secs(3600);

fn resolve_pid(app: &Arc<DaemonContext>, key: &str) -> Option<i64> {
    app.settings.resolved_playlist_id(key)
}

fn group_displays_by_playlist(
    entries: impl IntoIterator<Item = (DisplayId, i64)>,
) -> HashMap<i64, Vec<DisplayId>> {
    let mut groups: HashMap<i64, Vec<DisplayId>> = HashMap::new();
    for (display_id, pid) in entries {
        groups.entry(pid).or_default().push(display_id);
    }
    groups
}

async fn activate_groups(app: &Arc<DaemonContext>, groups: HashMap<i64, Vec<DisplayId>>) {
    for (pid, display_ids) in groups {
        if display_ids.is_empty() {
            continue;
        }
        if let Err(e) = super::activate_resuming_with_first_frame_timeout(
            app,
            &display_ids,
            pid,
            crate::application::apply_first_frame_timeout(),
        )
        .await
        {
            log::warn!("restore playlist {pid} on displays {display_ids:?} failed: {e:#}");
        }
    }
}

fn queue_pending(
    pending: &mut HashMap<i64, (tokio::time::Instant, Vec<DisplayId>)>,
    pid: i64,
    display_id: DisplayId,
    settle: Duration,
) {
    let entry = pending
        .entry(pid)
        .or_insert_with(|| (tokio::time::Instant::now() + settle, Vec::new()));
    if !entry.1.contains(&display_id) {
        entry.1.push(display_id);
    }
}

fn remove_pending_display(
    pending: &mut HashMap<i64, (tokio::time::Instant, Vec<DisplayId>)>,
    display_id: DisplayId,
) {
    pending.retain(|_, (_, ids)| {
        ids.retain(|d| *d != display_id);
        !ids.is_empty()
    });
}

async fn collect_playlist_targets(app: &Arc<DaemonContext>, pid: i64) -> Vec<DisplayId> {
    let status = app.playlists.status().await;
    let owned_other: HashSet<DisplayId> = status
        .iter()
        .filter(|s| s.active_id != pid)
        .map(|s| s.display_id)
        .collect();
    let owned_same: HashSet<DisplayId> = status
        .iter()
        .filter(|s| s.active_id == pid)
        .map(|s| s.display_id)
        .collect();

    let mut out = Vec::new();
    for d in app.router.snapshot_displays().await {
        if owned_other.contains(&d.id) {
            continue;
        }
        if owned_same.contains(&d.id) {
            out.push(d.id);
            continue;
        }
        let key = d.instance_id.clone().unwrap_or_else(|| d.name.clone());
        if resolve_pid(app, &key) == Some(pid) {
            out.push(d.id);
        }
    }
    out
}

async fn flush_pending(
    app: &Arc<DaemonContext>,
    pending: &mut HashMap<i64, (tokio::time::Instant, Vec<DisplayId>)>,
    due: impl IntoIterator<Item = i64>,
) {
    let mut groups = HashMap::new();
    for pid in due {
        pending.remove(&pid);
        let targets = collect_playlist_targets(app, pid).await;
        if !targets.is_empty() {
            groups.insert(pid, targets);
        }
    }
    activate_groups(app, groups).await;
}

pub async fn restore_all(app: &Arc<DaemonContext>) {
    let displays = app.router.snapshot_displays().await;
    let entries = displays.into_iter().filter_map(|d| {
        let key = d.instance_id.clone().unwrap_or_else(|| d.name.clone());
        resolve_pid(app, &key).map(|pid| (d.id, pid))
    });
    activate_groups(app, group_displays_by_playlist(entries)).await;
}

pub async fn watch_hotplug(app: Arc<DaemonContext>) {
    let mut rx = app.router.subscribe_events();
    let mut shutdown = app.shutdown_subscribe();

    let mut sources = app.events.watch_sources_ready();
    let mut displays = app.events.watch_display_ready();
    tokio::select! {
        _ = async {
            let _ = sources.wait_for(|v| *v).await;
            let _ = displays.wait_for(|v| *v).await;
        } => {}
        changed = shutdown.changed() => {
            if changed.is_err() || *shutdown.borrow() { return; }
        }
    }

    restore_all(&app).await;

    let mut known: HashSet<DisplayId> = app
        .router
        .snapshot_displays()
        .await
        .into_iter()
        .map(|d| d.id)
        .collect();
    let mut pending: HashMap<i64, (tokio::time::Instant, Vec<DisplayId>)> = HashMap::new();

    loop {
        let next_deadline = pending
            .values()
            .map(|(d, _)| *d)
            .min()
            .unwrap_or_else(|| tokio::time::Instant::now() + IDLE_PARK);
        let sleep = tokio::time::sleep_until(next_deadline);
        tokio::pin!(sleep);

        tokio::select! {
            ev = rx.recv() => {
                match ev {
                    Ok(RouterEvent::DisplayUpsert(s)) => {
                        let key = s.instance_id.clone().unwrap_or_else(|| s.name.clone());
                        let is_new = known.insert(s.id);
                        let owner = app.playlists.owner_playlist(s.id).await;
                        let pid = if let Some(pid) = owner {
                            Some(pid)
                        } else if is_new {
                            resolve_pid(&app, &key)
                        } else {
                            app.settings.display_prefs(&key).and_then(|p| p.active_playlist_id)
                        };
                        let Some(pid) = pid else { continue };
                        match super::attach_playlist(&app, s.id, pid).await {
                            Ok(true) => {}
                            Ok(false) => queue_pending(&mut pending, pid, s.id, SETTLE),
                            Err(e) => {
                                log::warn!("hotplug attach playlist {pid} failed: {e:#}");
                            }
                        }
                    }
                    Ok(RouterEvent::DisplayRemoved(id)) => {
                        app.playlists.drop_display(id).await;
                        known.remove(&id);
                        remove_pending_display(&mut pending, id);
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        log::warn!("playlist hotplug watcher lagged {n} events; re-snapshotting");
                        pending.clear();
                        known.clear();
                        let mut pids = HashSet::new();
                        for d in app.router.snapshot_displays().await {
                            known.insert(d.id);
                            let key = d.instance_id.clone().unwrap_or_else(|| d.name.clone());
                            if let Some(pid) = resolve_pid(&app, &key) {
                                pids.insert(pid);
                            }
                        }
                        let mut groups = HashMap::new();
                        for pid in pids {
                            let targets = collect_playlist_targets(&app, pid).await;
                            if !targets.is_empty() {
                                groups.insert(pid, targets);
                            }
                        }
                        activate_groups(&app, groups).await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = &mut sleep => {
                let now = tokio::time::Instant::now();
                let due: Vec<i64> = pending
                    .iter()
                    .filter_map(|(pid, (d, _))| (*d <= now).then_some(*pid))
                    .collect();
                if !due.is_empty() {
                    flush_pending(&app, &mut pending, due).await;
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { break; }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn queue_pending_coalesces_same_playlist() {
        let mut pending = HashMap::new();
        let settle = Duration::from_secs(2);
        queue_pending(&mut pending, 7, 1, settle);
        queue_pending(&mut pending, 7, 2, settle);
        queue_pending(&mut pending, 7, 1, settle);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending.get(&7).unwrap().1, vec![1, 2]);
    }

    #[test]
    fn queue_pending_keeps_separate_playlists() {
        let mut pending = HashMap::new();
        let settle = Duration::from_millis(10);
        queue_pending(&mut pending, 1, 10, settle);
        queue_pending(&mut pending, 2, 20, settle);
        assert_eq!(pending.len(), 2);
        assert_eq!(pending.get(&1).unwrap().1, vec![10]);
        assert_eq!(pending.get(&2).unwrap().1, vec![20]);
    }

    #[test]
    fn remove_pending_display_drops_empty_entries() {
        let mut pending = HashMap::new();
        let settle = Duration::from_secs(1);
        queue_pending(&mut pending, 1, 10, settle);
        queue_pending(&mut pending, 1, 11, settle);
        queue_pending(&mut pending, 2, 20, settle);
        remove_pending_display(&mut pending, 10);
        assert_eq!(pending.get(&1).unwrap().1, vec![11]);
        remove_pending_display(&mut pending, 11);
        assert!(!pending.contains_key(&1));
        assert_eq!(pending.get(&2).unwrap().1, vec![20]);
    }

    #[test]
    fn group_displays_by_playlist_merges_same_pid() {
        let groups = group_displays_by_playlist([(1, 42), (2, 42), (3, 7)]);
        assert_eq!(groups.get(&42).unwrap(), &vec![1, 2]);
        assert_eq!(groups.get(&7).unwrap(), &vec![3]);
    }
}
