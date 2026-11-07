use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use crate::application::ApplySource;
use crate::error::Result;
use crate::model::repo::playlists as repository;
use crate::playback::playlist::{
    Activation, ApplyPort, ApplyRequest, ApplySource as PlaylistApplySource, Definition, Target,
    TargetId,
};
use crate::playback::Mode;
use crate::wallframe::scheduler::DisplayId;
use crate::DaemonContext;

#[derive(Clone, Copy)]
enum AutoAttachUpdate {
    Preserve,
    Inherit,
    Disable,
    ClearGlobal,
}

fn apply_source(source: PlaylistApplySource, activation_source: ApplySource) -> ApplySource {
    match source {
        PlaylistApplySource::Activation => activation_source,
        PlaylistApplySource::Rotation => ApplySource::PlaylistRotation,
        PlaylistApplySource::Jump => ApplySource::UserPlaylistJump,
        PlaylistApplySource::Step => ApplySource::UserPlaylistStep,
        PlaylistApplySource::Rebuild => ApplySource::PlaylistRebuild,
        PlaylistApplySource::Attach => ApplySource::PlaylistAttach,
    }
}

fn publishes_position_change(source: PlaylistApplySource) -> bool {
    matches!(
        source,
        PlaylistApplySource::Rotation | PlaylistApplySource::Jump | PlaylistApplySource::Step
    )
}

// Activation publishes in the caller after the engine finishes any rollback.
// Other failed applies retain their cursor changes and must notify subscribers.
fn publishes_after_apply(source: PlaylistApplySource, apply_failed: bool) -> bool {
    publishes_position_change(source)
        || (apply_failed && !matches!(source, PlaylistApplySource::Activation))
}

fn apply_port(app: &Arc<DaemonContext>, activation_source: ApplySource) -> ApplyPort {
    let app = app.clone();
    ApplyPort::new(move |request: ApplyRequest| {
        let app = app.clone();
        async move {
            let playlist_source = request.source;
            let source = apply_source(playlist_source, activation_source);
            let calls = request.assignments.into_iter().map(|assignment| {
                let app = app.clone();
                let entry_id = assignment.entry_id;
                let target_count = assignment.targets.len();
                async move {
                    let targets = assignment
                        .targets
                        .into_iter()
                        .map(|target| match target {
                            TargetId::Display(display_id) => {
                                crate::application::ApplyTarget::Display(display_id)
                            }
                            TargetId::Canvas(canvas_id) => {
                                crate::application::ApplyTarget::Canvas(canvas_id)
                            }
                        })
                        .collect();
                    super::apply_wallpaper(
                        &app,
                        &entry_id,
                        crate::application::ApplyRequest {
                            source,
                            targets: Some(targets),
                            renderer_name: None,
                            first_frame_timeout: request.first_frame_timeout,
                            require_display: false,
                            sharing: crate::application::RendererSharingPolicy::UseSettings,
                        },
                    )
                    .await
                    .map(|_| ())
                    .map_err(|error| (entry_id, target_count, error))
                }
            });
            let mut first_error = None;
            for result in futures_util::future::join_all(calls).await {
                if let Err((entry_id, target_count, error)) = result {
                    log::warn!(
                        "playlist apply entry={entry_id} targets={target_count} failed: {error:#}"
                    );
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
            if publishes_after_apply(playlist_source, first_error.is_some()) {
                publish_changed(&app);
            }
            if let Some(error) = first_error {
                return Err(error);
            }
            Ok(())
        }
    })
}

async fn playlist_targets(
    app: &Arc<DaemonContext>,
    display_ids: &[DisplayId],
) -> Result<Vec<Target>> {
    let target_ids = app.router.config_targets_for_displays(display_ids).await?;
    Ok(app
        .router
        .resolve_config_targets(Some(&target_ids))
        .await?
        .into_iter()
        .map(|target| Target {
            id: match target.id {
                crate::wallframe::routing::ConfigTargetId::Display(display_id) => {
                    TargetId::Display(display_id)
                }
                crate::wallframe::routing::ConfigTargetId::Canvas(canvas_id) => {
                    TargetId::Canvas(canvas_id)
                }
            },
            display_ids: target
                .members
                .into_iter()
                .map(|member| member.display_id)
                .collect(),
        })
        .collect())
}

async fn definition(app: &Arc<DaemonContext>, playlist_id: i64) -> Result<Definition> {
    let detail = repository::detail(&app.db, playlist_id).await?;
    Ok(definition_from_detail(detail))
}

fn definition_from_detail(detail: repository::Detail) -> Definition {
    let playlist = detail.summary;
    Definition {
        id: playlist.id,
        mode: playlist.mode,
        interval_secs: playlist.interval_secs,
        synchronized_selection: playlist.synchronized_selection,
        keep_empty: playlist.source.is_smart(),
        items: detail
            .entries
            .iter()
            .map(|e| e.item_id.to_string())
            .collect(),
    }
}

async fn resume_ids(
    app: &Arc<DaemonContext>,
    definition: &Definition,
    display_ids: &[DisplayId],
) -> HashMap<DisplayId, String> {
    if definition.mode == Mode::Random {
        return HashMap::new();
    }
    let mut ids = HashMap::new();
    let snapshots = app.router.snapshot_displays().await;
    for display_id in display_ids {
        let Some(display) = snapshots.iter().find(|display| display.id == *display_id) else {
            continue;
        };
        let entry_id = if let Some(canvas_id) = &display.canvas_id {
            app.settings
                .canvas(canvas_id)
                .and_then(|canvas| canvas.last_wallpaper)
        } else {
            app.settings
                .display_prefs(&display.settings_key)
                .and_then(|prefs| prefs.last_wallpaper)
        };
        let Some(entry_id) = entry_id.filter(|entry_id| definition.items.contains(entry_id)) else {
            continue;
        };
        ids.insert(*display_id, entry_id);
    }
    ids
}

async fn persist_assignments(
    app: &Arc<DaemonContext>,
    display_ids: &[DisplayId],
    playlist_id: Option<i64>,
    auto_attach: AutoAttachUpdate,
) {
    let keys = app.router.display_settings_keys(display_ids).await;
    app.settings.update(|settings| {
        for (_, key) in &keys {
            let prefs = settings.displays.entry(key.clone()).or_default();
            prefs.active_playlist_id = playlist_id;
            match auto_attach {
                AutoAttachUpdate::Preserve => {}
                AutoAttachUpdate::Inherit => prefs.playlist_auto_attach_disabled = false,
                AutoAttachUpdate::Disable => prefs.playlist_auto_attach_disabled = true,
                AutoAttachUpdate::ClearGlobal => prefs.playlist_auto_attach_disabled = false,
            }
        }
        if matches!(auto_attach, AutoAttachUpdate::ClearGlobal) {
            settings.global.auto_attach_playlist_id = None;
        }
    });
    app.settings.flush_now().await;
}

fn publish_changed(app: &Arc<DaemonContext>) {
    app.events
        .publish(crate::events::GlobalEvent::PlaylistChanged);
}

pub async fn activate(
    app: &Arc<DaemonContext>,
    display_ids: &[DisplayId],
    playlist_id: i64,
) -> Result<()> {
    activate_inner(
        app,
        display_ids,
        playlist_id,
        false,
        None,
        ApplySource::UserPlaylistActivation,
    )
    .await
}

pub async fn activate_resuming_with_first_frame_timeout(
    app: &Arc<DaemonContext>,
    display_ids: &[DisplayId],
    playlist_id: i64,
    timeout: Duration,
) -> Result<()> {
    activate_inner(
        app,
        display_ids,
        playlist_id,
        true,
        Some(timeout),
        ApplySource::StartupRestore,
    )
    .await
}

async fn activate_inner(
    app: &Arc<DaemonContext>,
    display_ids: &[DisplayId],
    playlist_id: i64,
    resume: bool,
    first_frame_timeout: Option<Duration>,
    activation_source: ApplySource,
) -> Result<()> {
    let _edit = app.playlist_mutation.lock().await;
    let definition = definition(app, playlist_id).await?;
    let targets = if display_ids.is_empty() {
        app.router
            .snapshot_displays()
            .await
            .into_iter()
            .map(|display| display.id)
            .collect()
    } else {
        display_ids.to_vec()
    };
    let playlist_targets = playlist_targets(app, &targets).await?;
    let targets = playlist_targets
        .iter()
        .flat_map(|target| target.display_ids.iter().copied())
        .collect::<Vec<_>>();
    let resume_by_display = if resume {
        resume_ids(app, &definition, &targets).await
    } else {
        HashMap::new()
    };
    let result = app
        .playlists
        .activate(
            Activation {
                definition,
                targets: playlist_targets,
                resume_by_display,
                first_frame_timeout,
            },
            apply_port(app, activation_source),
            app.shutdown_subscribe(),
        )
        .await;
    if let Err(error) = result {
        publish_changed(app);
        return Err(error);
    }
    persist_assignments(app, &targets, Some(playlist_id), AutoAttachUpdate::Inherit).await;
    publish_changed(app);
    app.wallpaper_changes
        .send_modify(|generation| *generation += 1);
    Ok(())
}

pub async fn attach(
    app: &Arc<DaemonContext>,
    display_id: DisplayId,
    playlist_id: i64,
) -> Result<bool> {
    let _edit = app.playlist_mutation.lock().await;
    let targets = playlist_targets(app, &[display_id]).await?;
    let Some(target) = targets.into_iter().next() else {
        return Ok(false);
    };
    let definition = definition(app, playlist_id).await?;
    let resume_by_display = resume_ids(app, &definition, &target.display_ids).await;
    let target_display_ids = target.display_ids.clone();
    let attached = app
        .playlists
        .attach(
            target,
            playlist_id,
            resume_by_display,
            crate::application::APPLY_FIRST_FRAME_TIMEOUT,
            apply_port(app, ApplySource::UserPlaylistActivation),
        )
        .await?;
    if attached {
        persist_assignments(
            app,
            &target_display_ids,
            Some(playlist_id),
            AutoAttachUpdate::Inherit,
        )
        .await;
        publish_changed(app);
    }
    Ok(attached)
}

pub async fn deactivate(app: &Arc<DaemonContext>, display_ids: &[DisplayId]) -> Result<()> {
    let _edit = app.playlist_mutation.lock().await;
    let targets = if display_ids.is_empty() {
        app.playlists.owned_display_ids().await
    } else {
        app.router
            .expand_display_config_members(display_ids)
            .await?
    };
    app.playlists.deactivate(&targets).await;
    persist_assignments(app, &targets, None, AutoAttachUpdate::Preserve).await;
    publish_changed(app);
    Ok(())
}

pub async fn deactivate_for_playlist(app: &Arc<DaemonContext>, playlist_id: i64) {
    let displays = app.playlists.deactivate_playlist(playlist_id).await;
    if displays.is_empty() {
        return;
    }
    persist_assignments(app, &displays, None, AutoAttachUpdate::Preserve).await;
    publish_changed(app);
}

pub async fn jump_to(app: &Arc<DaemonContext>, playlist_id: i64, entry_id: &str) -> Result<()> {
    app.playlists
        .jump_to(
            playlist_id,
            entry_id,
            apply_port(app, ApplySource::UserPlaylistActivation),
        )
        .await
}

pub(super) async fn step_sessions(app: &Arc<DaemonContext>, delta: i32) -> Result<bool> {
    app.playlists
        .step(delta, apply_port(app, ApplySource::UserPlaylistActivation))
        .await
}

pub async fn rebuild_for_playlist(app: &Arc<DaemonContext>, playlist_id: i64) {
    let Ok(definition) = definition(app, playlist_id).await else {
        return;
    };
    match app
        .playlists
        .rebuild(
            definition,
            apply_port(app, ApplySource::UserPlaylistActivation),
        )
        .await
    {
        Ok(cleared) if !cleared.is_empty() => {
            persist_assignments(app, &cleared, None, AutoAttachUpdate::Preserve).await;
            publish_changed(app);
        }
        Ok(_) => {}
        Err(error) => log::warn!("playlist rebuild {playlist_id} failed: {error:#}"),
    }
}

pub async fn refresh_smart_playlists(app: Arc<DaemonContext>) {
    let mut changes = app.wallpaper_changes.subscribe();
    changes.mark_changed();
    let mut shutdown = app.shutdown_subscribe();
    let mut failures: HashMap<i64, (u64, tokio::time::Instant)> = HashMap::new();
    let mut last_generation = None;
    loop {
        let next_retry = failures.values().map(|(_, deadline)| *deadline).min();
        let deadline =
            next_retry.unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(30));
        tokio::select! {
            changed = shutdown.changed() => if changed.is_err() || *shutdown.borrow() { break; },
            changed = changes.changed() => if changed.is_err() { break; },
            _ = tokio::time::sleep_until(deadline), if next_retry.is_some() => {},
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(100)) => {},
            _ = shutdown.changed() => break,
        }
        let generation = *changes.borrow_and_update();
        let data_changed = last_generation != Some(generation);
        if data_changed {
            publish_changed(&app);
        }
        last_generation = Some(generation);
        let tickets = app.playlists.refresh_tickets().await;
        let active: HashSet<_> = tickets.iter().map(|ticket| ticket.playlist_id()).collect();
        failures.retain(|id, _| active.contains(id));
        for ticket in tickets {
            let id = ticket.playlist_id();
            if !data_changed && !failures.contains_key(&id) {
                continue;
            }
            if failures
                .get(&id)
                .is_some_and(|(_, deadline)| *deadline > tokio::time::Instant::now())
            {
                continue;
            }
            let result: Result<()> = async {
                let detail = repository::detail(&app.db, id).await?;
                let revision = detail.summary.revision;
                let definition = definition_from_detail(detail);
                let _edit = app.playlist_mutation.lock().await;
                if *changes.borrow() != generation
                    || repository::revision(&app.db, id).await? != revision
                {
                    return Ok(());
                }
                let before = app.playlists.status().await;
                app.playlists
                    .refresh(
                        ticket.clone(),
                        definition,
                        apply_port(&app, ApplySource::UserPlaylistActivation),
                    )
                    .await?;
                let after = app.playlists.status().await;
                if before
                    .iter()
                    .map(|s| (&s.current_id, s.count, &s.error))
                    .collect::<Vec<_>>()
                    != after
                        .iter()
                        .map(|s| (&s.current_id, s.count, &s.error))
                        .collect::<Vec<_>>()
                {
                    publish_changed(&app);
                }
                Ok(())
            }
            .await;
            if let Err(error) = result {
                log::warn!("smart playlist {id} refresh failed: {error:#}");
                if app
                    .playlists
                    .refresh_error(&ticket, error.to_string())
                    .await
                {
                    publish_changed(&app);
                }
                let delay = failures
                    .get(&id)
                    .map_or(2, |(delay, _)| (delay * 2).min(30));
                failures.insert(
                    id,
                    (
                        delay,
                        tokio::time::Instant::now() + Duration::from_secs(delay),
                    ),
                );
            } else {
                failures.remove(&id);
            }
        }
    }
}

pub(super) async fn stop_for_wallpaper_override(
    app: &Arc<DaemonContext>,
    display_ids: &[DisplayId],
    all_displays: bool,
) -> Result<Vec<crate::application::StoppedPlaylist>> {
    let _edit = app.playlist_mutation.lock().await;
    if display_ids.is_empty() {
        return Ok(Vec::new());
    }

    let targets: HashSet<DisplayId> = display_ids.iter().copied().collect();
    let mut groups: BTreeMap<i64, Vec<DisplayId>> = BTreeMap::new();
    for status in app.playlists.status().await {
        if targets.contains(&status.display_id) {
            groups
                .entry(status.active_id)
                .or_default()
                .push(status.display_id);
        }
    }
    for ids in groups.values_mut() {
        ids.sort_unstable();
        ids.dedup();
    }

    let mut stopped = Vec::with_capacity(groups.len());
    for (id, display_ids) in groups {
        let name = repository::get(&app.db, id)
            .await?
            .map(|playlist| playlist.name)
            .unwrap_or_default();
        stopped.push(crate::application::StoppedPlaylist {
            id,
            name,
            all_displays: all_displays && display_ids.len() == targets.len(),
            display_ids,
        });
    }

    app.playlists.deactivate(display_ids).await;
    persist_assignments(
        app,
        display_ids,
        None,
        if all_displays {
            AutoAttachUpdate::ClearGlobal
        } else {
            AutoAttachUpdate::Disable
        },
    )
    .await;
    if !stopped.is_empty() {
        publish_changed(app);
    }
    Ok(stopped)
}

pub async fn set_interval_for_playlist(
    app: &Arc<DaemonContext>,
    playlist_id: i64,
    interval_secs: u32,
) {
    app.playlists.set_interval(playlist_id, interval_secs).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCES: [PlaylistApplySource; 6] = [
        PlaylistApplySource::Activation,
        PlaylistApplySource::Rotation,
        PlaylistApplySource::Jump,
        PlaylistApplySource::Step,
        PlaylistApplySource::Rebuild,
        PlaylistApplySource::Attach,
    ];

    #[test]
    fn failed_apply_defers_activation_publication_to_caller() {
        for source in SOURCES {
            assert_eq!(
                publishes_after_apply(source, true),
                !matches!(source, PlaylistApplySource::Activation),
                "{source:?}"
            );
        }
    }

    #[test]
    fn successful_apply_publishes_only_position_changes() {
        for source in SOURCES {
            assert_eq!(
                publishes_after_apply(source, false),
                publishes_position_change(source),
                "{source:?}"
            );
        }
    }
}
