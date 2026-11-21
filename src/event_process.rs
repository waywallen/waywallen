use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use crate::events::GlobalEvent;
use crate::tasks;
use crate::wallframe::routing;
use crate::wallframe::scheduler;
use crate::DaemonContext;

const DISPLAY_CONNECTION_NOTIFICATION_ID: &str =
    "org.waywallen.waywallen.display-connection-failed";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct RecallKey {
    wallpaper_id: String,
    canvas_id: Option<String>,
}

/// Spawn the dispatcher. `restore_last` mirrors `cli.restore_last` —
/// when false the wallpaper-recall watcher is never started even
pub fn spawn(state: Arc<DaemonContext>, restore_last: bool) {
    // Subscribe before submitting the task so publishers cannot win the
    // scheduler race and lose a transient event during startup.
    let mut bus = state.events.subscribe();
    let tasks_h = state.tasks.clone();
    tasks_h.spawn_async(
        tasks::TaskKind::Service,
        "service/event-process",
        async move {
            let mut recall_started = !restore_last;
            let mut shutdown = state.shutdown_subscribe();

            if !recall_started && state.events.is_sources_ready() {
                spawn_wallpaper_recall(state.clone());
                recall_started = true;
            }

            loop {
                tokio::select! {
                    event = bus.recv() => match event {
                        Ok(GlobalEvent::SourcesReady) => {
                            if !recall_started {
                                spawn_wallpaper_recall(state.clone());
                                recall_started = true;
                            }
                        }
                        Ok(GlobalEvent::DisplayConnectionFailed {
                            client_name,
                            reason,
                            ..
                        }) => {
                            let body = display_connection_notification_body(&client_name, &reason);
                            if let Err(e) = crate::system::notifications::notify(
                                DISPLAY_CONNECTION_NOTIFICATION_ID,
                                "Display connection failed",
                                &body,
                            )
                            .await
                            {
                                log::warn!("display connection notification failed: {e}");
                            }
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            if !recall_started && state.events.is_sources_ready() {
                                spawn_wallpaper_recall(state.clone());
                                recall_started = true;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return Ok(()),
                    },
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            return Ok(());
                        }
                    }
                }
            }
        },
    );
}

fn display_connection_notification_body(client_name: &str, reason: &str) -> String {
    if client_name.is_empty() {
        reason.to_string()
    } else {
        format!("{client_name}: {reason}")
    }
}

/// Long-lived watcher: re-apply each display's persisted wallpaper as
/// it becomes visible. Spawned by the dispatcher when `SourcesReady`
fn spawn_wallpaper_recall(state: Arc<DaemonContext>) {
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
            let mut pending: HashMap<RecallKey, (tokio::time::Instant, Vec<scheduler::DisplayId>)> =
                HashMap::new();
            let mut events_rx = state.router.subscribe_events();
            let mut shutdown = state.shutdown_subscribe();

            // Initial sweep of already-registered displays.
            for snap in state.router.snapshot_displays().await {
                if seen.insert(snap.id) {
                    record(&state, &mut pending, snap, SETTLE);
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
                        for snap in snaps {
                            if seen.insert(snap.id) {
                                record(&state, &mut pending, snap, SETTLE);
                            }
                        }
                    }
                    _ = &mut sleep => {
                        let now = tokio::time::Instant::now();
                        let due: Vec<RecallKey> = pending
                            .iter()
                            .filter_map(|(k, (d, _))| (*d <= now).then(|| k.clone()))
                            .collect();
                        for key in due {
                            if let Some((_, ids)) = pending.remove(&key) {
                                let state2 = state.clone();
                                let wp_id = key.wallpaper_id.clone();
                                let canvas_id = key.canvas_id.clone();
                                let task_name = format!("wallpaper/recall/{wp_id}");
                                state.tasks.spawn_async(
                                    tasks::TaskKind::Apply,
                                    task_name,
                                    async move {
                                        log::info!(
                                            "wallpaper recall: applying {wp_id} to {} display(s)",
                                            ids.len()
                                        );
                                        let result = if let Some(canvas_id) = canvas_id {
                                            crate::application::restore_wallpaper_canvas(
                                                &state2,
                                                &wp_id,
                                                Some(crate::application::apply_first_frame_timeout()),
                                                canvas_id,
                                            ).await
                                        } else {
                                            crate::application::apply_wallpaper_to_displays_with_first_frame_timeout(
                                                &state2,
                                                &wp_id,
                                                &ids,
                                                crate::application::apply_first_frame_timeout(),
                                                crate::application::ApplySource::DisplayRecall,
                                            ).await
                                        };
                                        result
                                        .map(|_| ())
                                        .map_err(anyhow::Error::from)
                                    },
                                );
                            }
                        }
                    }
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            return Ok(());
                        }
                    }
                }
            }
        },
    );
}

fn record(
    state: &Arc<DaemonContext>,
    pending: &mut HashMap<RecallKey, (tokio::time::Instant, Vec<scheduler::DisplayId>)>,
    snap: routing::DisplaySnapshot,
    settle: Duration,
) {
    let key = snap.instance_id.as_deref().unwrap_or(&snap.name);
    let playlist_owned = state.settings.resolved_playlist_id(key).is_some();
    if playlist_owned {
        return;
    }
    let (wp_id, canvas_id) = match state.settings.canvas_for_member(key) {
        Some((canvas_id, canvas)) => {
            let Some(wallpaper_id) = canvas.last_wallpaper else {
                return;
            };
            (wallpaper_id, Some(canvas_id))
        }
        None => {
            let Some(wallpaper_id) = state.settings.resolved_last_wallpaper(key) else {
                return;
            };
            (wallpaper_id, None)
        }
    };
    let entry = pending
        .entry(RecallKey {
            wallpaper_id: wp_id,
            canvas_id,
        })
        .or_insert_with(|| (tokio::time::Instant::now() + settle, Vec::new()));
    entry.1.push(snap.id);
}
