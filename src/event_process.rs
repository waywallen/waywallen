use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use crate::events::GlobalEvent;
use crate::routing;
use crate::scheduler;
use crate::tasks;
use crate::AppState;

/// Spawn the dispatcher. `restore_last` mirrors `cli.restore_last` —
/// when false the wallpaper-recall watcher is never started even
pub fn spawn(state: Arc<AppState>, restore_last: bool) {
    let tasks_h = state.tasks.clone();
    tasks_h.spawn_async(
        tasks::TaskKind::Service,
        "service/event-process",
        async move {
            // Subscribe BEFORE re-reading the latches so an event that
            // fired between AppState construction and the first poll
            let mut bus = state.events.subscribe();
            let mut recall_started = !restore_last;

            if !recall_started && state.events.is_sources_ready() {
                spawn_wallpaper_recall(state.clone());
                recall_started = true;
            }

            loop {
                match bus.recv().await {
                    Ok(GlobalEvent::SourcesReady) => {
                        if !recall_started {
                            spawn_wallpaper_recall(state.clone());
                            recall_started = true;
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        if !recall_started && state.events.is_sources_ready() {
                            spawn_wallpaper_recall(state.clone());
                            recall_started = true;
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

/// Long-lived watcher: re-apply each display's persisted wallpaper as
/// it becomes visible. Spawned by the dispatcher when `SourcesReady`
fn spawn_wallpaper_recall(state: Arc<AppState>) {
    let tasks_h = state.tasks.clone();
    tasks_h.spawn_async(
        tasks::TaskKind::Service,
        "service/wallpaper-recall",
        async move {
            // Settle window: how long to wait after the first display
            // for the group joins before firing the apply.
            const SETTLE: Duration = Duration::from_secs(2);
            // Far-future placeholder when nothing is pending, so the
            // select loop has a real deadline to wait on without an
            const IDLE_PARK: Duration = Duration::from_secs(3600);

            let mut seen: HashSet<scheduler::DisplayId> = HashSet::new();
            // wp_id -> (deadline, accumulated display ids)
            let mut pending: HashMap<String, (tokio::time::Instant, Vec<scheduler::DisplayId>)> =
                HashMap::new();
            let mut events_rx = state.router.subscribe_events();

            // Initial sweep of already-registered displays.
            let locked = state.router.is_session_locked().await;
            for snap in state.router.snapshot_displays().await {
                if seen.insert(snap.id) {
                    record(&state, &mut pending, snap, locked, SETTLE);
                }
            }

            loop {
                let next_deadline = pending
                    .values()
                    .map(|(d, _)| *d)
                    .min()
                    .unwrap_or_else(|| tokio::time::Instant::now() + IDLE_PARK);
                let sleep = tokio::time::sleep_until(next_deadline);
                tokio::pin!(sleep);

                tokio::select! {
                    ev = events_rx.recv() => {
                        let snaps: Vec<routing::DisplaySnapshot> = match ev {
                            Ok(routing::RouterEvent::DisplayUpsert(s)) => vec![s],
                            Ok(routing::RouterEvent::DisplaysReplace(list)) => list,
                            Ok(_) => continue,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                state.router.snapshot_displays().await
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                return Ok(());
                            }
                        };
                        let locked = state.router.is_session_locked().await;
                        for snap in snaps {
                            if seen.insert(snap.id) {
                                record(&state, &mut pending, snap, locked, SETTLE);
                            }
                        }
                    }
                    _ = &mut sleep => {
                        let now = tokio::time::Instant::now();
                        let due: Vec<String> = pending
                            .iter()
                            .filter_map(|(k, (d, _))| (*d <= now).then(|| k.clone()))
                            .collect();
                        for wp_id in due {
                            if let Some((_, ids)) = pending.remove(&wp_id) {
                                let state2 = state.clone();
                                tokio::spawn(async move {
                                    log::info!(
                                        "wallpaper recall: applying {wp_id} to {} display(s)",
                                        ids.len()
                                    );
                                    if let Err(e) =
                                        crate::control::apply_wallpaper_to_displays_with_first_frame_timeout(
                                            &state2,
                                            &wp_id,
                                            &ids,
                                            crate::control::APPLY_FIRST_FRAME_TIMEOUT,
                                        )
                                        .await
                                    {
                                        log::warn!(
                                            "wallpaper recall failed for {wp_id}: {e:#}"
                                        );
                                    }
                                });
                            }
                        }
                    }
                }
            }
        },
    );
}

fn record(
    state: &Arc<AppState>,
    pending: &mut HashMap<String, (tokio::time::Instant, Vec<scheduler::DisplayId>)>,
    snap: routing::DisplaySnapshot,
    locked: bool,
    settle: Duration,
) {
    let key = snap.instance_id.as_deref().unwrap_or(&snap.name);
    if locked {
        if let Some(wp_id) = state.settings.resolved_lock_wallpaper(key) {
            pending
                .entry(wp_id)
                .or_insert_with(|| (tokio::time::Instant::now(), Vec::new()))
                .1
                .push(snap.id);
        }
        return;
    }
    let playlist_owned = state
        .settings
        .display_prefs(key)
        .and_then(|p| p.active_playlist_id)
        .or_else(|| state.settings.global().auto_attach_playlist_id)
        .is_some();
    if playlist_owned {
        return;
    }
    let Some(wp_id) = state.settings.resolved_last_wallpaper(key) else {
        return;
    };
    let entry = pending
        .entry(wp_id)
        .or_insert_with(|| (tokio::time::Instant::now() + settle, Vec::new()));
    entry.1.push(snap.id);
}
