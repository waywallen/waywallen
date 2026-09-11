use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use ashpd::desktop::wallpaper::{SetOn, WallpaperRequest};

use crate::application::{
    should_duplicate_renderers, ApplyActivation, ApplyRequest, ApplyResult, ApplySource,
    ApplyTarget, RendererSharingPolicy,
};
use crate::catalog::entry::WallpaperEntry;
use crate::error::{Error, Result};
use crate::model::repo;
use crate::plugin::renderer_registry::RendererDef;
use crate::wallframe::renderer_manager;
use crate::wallframe::scheduler::DisplayId;
use crate::DaemonContext;

async fn apply_targets_for_displays(
    app: &Arc<DaemonContext>,
    display_ids: &[DisplayId],
) -> Result<Vec<ApplyTarget>> {
    Ok(app
        .router
        .config_targets_for_displays(display_ids)
        .await?
        .into_iter()
        .map(|target| match target {
            crate::wallframe::routing::ConfigTargetId::Display(display_id) => {
                ApplyTarget::Display(display_id)
            }
            crate::wallframe::routing::ConfigTargetId::Canvas(canvas_id) => {
                ApplyTarget::Canvas(canvas_id)
            }
        })
        .collect())
}

/// Apply a wallpaper by id to every registered display.
/// Supersedes any in-flight global apply task.
pub async fn apply_wallpaper_by_id(
    app: &Arc<DaemonContext>,
    id: &str,
    source: ApplySource,
) -> Result<ApplyResult> {
    let app_clone = app.clone();
    let id_owned = id.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<ApplyResult>>();
    app.tasks.spawn_async_unique(
        crate::tasks::TaskKind::Apply,
        "apply/global",
        format!("apply/{id_owned}"),
        async move {
            let res = apply_wallpaper(
                &app_clone,
                &id_owned,
                ApplyRequest {
                    source,
                    targets: None,
                    renderer_name: None,
                    first_frame_timeout: None,
                    require_display: false,
                    sharing: RendererSharingPolicy::UseSettings,
                },
            )
            .await;
            // If the receiver is gone the caller already moved on (or
            // was itself cancelled); silently drop the result.
            let _ = tx.send(res);
            Ok(())
        },
    );
    rx.await
        .map_err(|_| Error::Internal(anyhow!("apply task superseded or cancelled")))?
}

/// Apply a wallpaper to a specific display subset.
/// Hot-plug recall uses this without cancelling global apply work.
pub async fn apply_wallpaper_to_displays(
    app: &Arc<DaemonContext>,
    id: &str,
    target: &[DisplayId],
    source: ApplySource,
) -> Result<ApplyResult> {
    if target.is_empty() {
        return Err(Error::Internal(anyhow!(
            "apply_wallpaper_to_displays: empty target"
        )));
    }
    apply_wallpaper(
        app,
        id,
        ApplyRequest {
            source,
            targets: Some(apply_targets_for_displays(app, target).await?),
            renderer_name: None,
            first_frame_timeout: None,
            require_display: false,
            sharing: RendererSharingPolicy::UseSettings,
        },
    )
    .await
}

pub async fn apply_wallpaper_to_displays_with_first_frame_timeout(
    app: &Arc<DaemonContext>,
    id: &str,
    target: &[DisplayId],
    timeout: Duration,
    source: ApplySource,
) -> Result<ApplyResult> {
    if target.is_empty() {
        return Err(Error::Internal(anyhow!(
            "apply_wallpaper_to_displays: empty target"
        )));
    }
    apply_wallpaper(
        app,
        id,
        ApplyRequest {
            source,
            targets: Some(apply_targets_for_displays(app, target).await?),
            renderer_name: None,
            first_frame_timeout: Some(timeout),
            require_display: false,
            sharing: RendererSharingPolicy::UseSettings,
        },
    )
    .await
}

pub async fn apply_wallpaper_to_canvas(
    app: &Arc<DaemonContext>,
    id: &str,
    canvas_id: &str,
    first_frame_timeout: Option<Duration>,
    source: ApplySource,
) -> Result<ApplyResult> {
    apply_wallpaper(
        app,
        id,
        ApplyRequest {
            source,
            targets: Some(vec![ApplyTarget::Canvas(canvas_id.to_string())]),
            renderer_name: None,
            first_frame_timeout,
            require_display: false,
            sharing: RendererSharingPolicy::Shared,
        },
    )
    .await
}

pub async fn restore_wallpaper_canvas(
    app: &Arc<DaemonContext>,
    id: &str,
    first_frame_timeout: Option<Duration>,
    canvas_id: String,
) -> Result<ApplyResult> {
    apply_wallpaper(
        app,
        id,
        ApplyRequest {
            source: ApplySource::DisplayRecall,
            targets: Some(vec![ApplyTarget::Canvas(canvas_id)]),
            renderer_name: None,
            first_frame_timeout,
            require_display: false,
            sharing: RendererSharingPolicy::Shared,
        },
    )
    .await
}

pub async fn reconcile_presentation_configs(
    app: &Arc<DaemonContext>,
    affected_display_keys: &[String],
    touched_canvas_ids: &[String],
) -> Result<()> {
    let displays = app.router.snapshot_displays().await;
    let affected_keys = affected_display_keys
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let mut canvas_ids = touched_canvas_ids
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    for display in &displays {
        if affected_keys.contains(display.settings_key.as_str()) {
            if let Some(canvas_id) = &display.canvas_id {
                canvas_ids.insert(canvas_id.clone());
            }
        }
    }

    let mut handled_displays = std::collections::HashSet::new();
    for canvas_id in canvas_ids {
        let Some(canvas) = app.settings.canvas(&canvas_id) else {
            continue;
        };
        let member_ids = displays
            .iter()
            .filter(|display| display.canvas_id.as_deref() == Some(canvas_id.as_str()))
            .map(|display| display.id)
            .collect::<Vec<_>>();
        handled_displays.extend(member_ids.iter().copied());
        if member_ids.is_empty() {
            continue;
        }
        if let Some(wallpaper_id) = canvas.last_wallpaper {
            apply_wallpaper_to_canvas(
                app,
                &wallpaper_id,
                &canvas_id,
                None,
                ApplySource::DisplayRecall,
            )
            .await?;
        } else {
            app.router.clear_display_assignments(&member_ids).await;
        }
    }

    for display in displays.into_iter().filter(|display| {
        affected_keys.contains(display.settings_key.as_str())
            && display.canvas_id.is_none()
            && !handled_displays.contains(&display.id)
    }) {
        let wallpaper_id = app
            .settings
            .display_prefs(&display.settings_key)
            .and_then(|prefs| prefs.last_wallpaper);
        if let Some(wallpaper_id) = wallpaper_id {
            apply_wallpaper_to_displays(
                app,
                &wallpaper_id,
                &[display.id],
                ApplySource::DisplayRecall,
            )
            .await?;
        } else {
            app.router.clear_display_assignments(&[display.id]).await;
        }
    }
    Ok(())
}

pub struct PortalApplyResult {
    pub wallpaper_id: String,
    pub uri: String,
}

/// Apply an image wallpaper through `org.freedesktop.portal.Wallpaper`.
/// The portal owns preview, prompting, and final rendering.
pub async fn apply_wallpaper_via_portal(
    app: &Arc<DaemonContext>,
    id: &str,
    source: ApplySource,
) -> Result<PortalApplyResult> {
    let app_clone = app.clone();
    let id_owned = id.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<PortalApplyResult>>();
    app.tasks.spawn_async_unique(
        crate::tasks::TaskKind::Apply,
        "apply/portal",
        format!("apply-portal/{id_owned}"),
        async move {
            log::debug!(
                "portal wallpaper apply: id={id_owned} source={}",
                source.as_str()
            );
            let res = apply_via_portal_inner(&app_clone, &id_owned).await;
            let _ = tx.send(res);
            Ok(())
        },
    );
    rx.await
        .map_err(|_| Error::Internal(anyhow!("apply task superseded or cancelled")))?
}

async fn apply_via_portal_inner(app: &Arc<DaemonContext>, id: &str) -> Result<PortalApplyResult> {
    let entry = match id.parse::<i64>() {
        Ok(iid) => repo::get_entry(&app.db, iid).await?,
        Err(_) => None,
    };
    let entry = entry.ok_or_else(|| Error::WallpaperNotFound(id.to_string()))?;

    if !entry.wp_type.eq_ignore_ascii_case("image") {
        return Err(Error::WallpaperTypeNotSupported(entry.wp_type.clone()));
    }
    if !entry.resource.starts_with('/') {
        return Err(Error::InvalidArgument(format!(
            "portal apply: resource must be an absolute path, got '{}'",
            entry.resource
        )));
    }
    let uri = ashpd::url::Url::from_file_path(&entry.resource).map_err(|()| {
        Error::InvalidArgument(format!(
            "portal apply: invalid absolute path '{}'",
            entry.resource
        ))
    })?;
    let request = WallpaperRequest::default()
        .set_on(SetOn::Background)
        .show_preview(false)
        .build_uri(&uri)
        .await
        .map_err(|e| Error::PortalCallFailed(format!("SetWallpaperURI: {e}")))?;
    request
        .response()
        .map_err(|e| Error::PortalCallFailed(format!("SetWallpaperURI response: {e}")))?;

    Ok(PortalApplyResult {
        wallpaper_id: entry.item_id.to_string(),
        uri: uri.into(),
    })
}

fn resolve_renderer(
    app: &Arc<DaemonContext>,
    entry: &WallpaperEntry,
    renderer_name: Option<&str>,
) -> Result<RendererDef> {
    let registry = app.renderer_manager.registry_snapshot();
    match renderer_name {
        Some(name) if !name.is_empty() => {
            let def = registry
                .resolve_by_name(name)
                .ok_or_else(|| Error::RendererNotFound(name.to_string()))?;
            if !def.types.iter().any(|ty| ty == &entry.wp_type) {
                return Err(Error::RendererTypeMismatch {
                    renderer: name.to_string(),
                    ty: entry.wp_type.clone(),
                });
            }
            Ok(def.clone())
        }
        _ => registry
            .resolve(&entry.wp_type)
            .cloned()
            .ok_or_else(|| Error::NoRendererForType(entry.wp_type.clone())),
    }
}

async fn wait_for_apply_frame(
    app: &Arc<DaemonContext>,
    renderer: &crate::wallframe::routing::ActiveRenderer,
    timeout: Option<Duration>,
) -> Result<()> {
    let Some(timeout) = timeout else {
        return Ok(());
    };
    if let Err(e) = app.router.wait_for_first_frame(renderer, timeout).await {
        if let Err(stop_error) = app
            .router
            .kill_renderer_generation_drop(&renderer.renderer_id, renderer.process_generation)
            .await
        {
            log::warn!(
                "renderer {}: cleanup after first-frame failure: {stop_error}",
                renderer.renderer_id
            );
        }
        return Err(e);
    }
    Ok(())
}

/// Shared global/per-display apply core.
/// Spawns or reuses renderers, relinks displays, and persists recall state.
pub async fn apply_wallpaper(
    app: &Arc<DaemonContext>,
    id: &str,
    request: ApplyRequest,
) -> Result<ApplyResult> {
    let entry = match id.parse::<i64>() {
        Ok(iid) => repo::get_entry(&app.db, iid).await?,
        Err(_) => None,
    };
    let entry = entry.ok_or_else(|| Error::WallpaperNotFound(id.to_string()))?;

    if request.require_display && app.router.display_count().await == 0 {
        return Err(Error::NoDisplayRegistered);
    }

    let requested_targets = request.targets.as_ref().map(|targets| {
        targets
            .iter()
            .map(|target| match target {
                ApplyTarget::Display(display_id) => {
                    crate::wallframe::routing::ConfigTargetId::Display(*display_id)
                }
                ApplyTarget::Canvas(canvas_id) => {
                    crate::wallframe::routing::ConfigTargetId::Canvas(canvas_id.clone())
                }
            })
            .collect::<Vec<_>>()
    });
    let resolved_targets = app
        .router
        .resolve_config_targets(requested_targets.as_deref())
        .await?;
    let target_ids = resolved_targets
        .iter()
        .flat_map(|target| target.members.iter().map(|member| member.display_id))
        .collect::<Vec<_>>();
    if request.require_display && target_ids.is_empty() {
        return Err(Error::NoDisplayRegistered);
    }

    let renderer = resolve_renderer(app, &entry, request.renderer_name.as_deref())?;
    let renderer_plugin_name = renderer.name.clone();
    let apply = app
        .source_manager
        .call_apply(&entry.plugin_name, &entry)
        .await?;
    let spawn_settings = app.settings.resolved_renderer_settings(&renderer);
    let (user_property_overrides, wallpaper_layout_override) =
        repo::get_wallpaper_render_properties(&app.db, entry.item_id).await?;
    // `apply_assignment` resolves display_size per renderer.
    let spawn_req = renderer_manager::SpawnRequest {
        wp_type: entry.wp_type.clone(),
        extras: apply.extras,
        settings: spawn_settings,
        test_pattern: false,
        renderer_name: Some(renderer_plugin_name.clone()),
        user_property_overrides,
        default_user_properties: apply.default_user_properties,
        display_size: None,
    };
    let stopped_playlists = if request.source == ApplySource::UserWallpaper {
        super::playlist::stop_for_wallpaper_override(app, &target_ids, request.targets.is_none())
            .await?
    } else {
        Vec::new()
    };
    let duplicate_renderers = should_duplicate_renderers(
        app.settings.global().duplicate_renderers_for_same_wallpaper,
        !target_ids.is_empty(),
        request.sharing,
    );
    let inherited_layout =
        wallpaper_layout_override.apply_to(app.settings.resolved_global_layout());
    let assignment_targets = resolved_targets
        .iter()
        .map(|target| {
            let projections = match &target.id {
                crate::wallframe::routing::ConfigTargetId::Display(_) => {
                    std::collections::HashMap::new()
                }
                crate::wallframe::routing::ConfigTargetId::Canvas(canvas_id) => {
                    let extent = target.extent.ok_or_else(|| {
                        Error::CanvasInvalid(format!("canvas {canvas_id} has no extent"))
                    })?;
                    let layout = app
                        .settings
                        .resolved_canvas_layout(canvas_id, inherited_layout);
                    target
                        .members
                        .iter()
                        .map(|member| {
                            let rect = member.rect.ok_or_else(|| {
                                Error::CanvasInvalid(format!(
                                    "canvas {canvas_id} member {} has no rect",
                                    member.display_id
                                ))
                            })?;
                            Ok((
                                member.display_id,
                                crate::wallframe::routing::table::LinkProjection::Canvas {
                                    canvas_id: canvas_id.clone(),
                                    extent,
                                    member: rect,
                                    layout,
                                },
                            ))
                        })
                        .collect::<Result<std::collections::HashMap<_, _>>>()?
                }
            };
            Ok(crate::wallframe::routing::AssignmentTarget {
                display_ids: target
                    .members
                    .iter()
                    .map(|member| member.display_id)
                    .collect(),
                projections,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let receipt = app
        .router
        .apply_assignment(crate::wallframe::routing::ApplyAssignment {
            spawn_request: spawn_req,
            targets: assignment_targets,
            duplicate_renderers,
            wallpaper_layout_override,
            preempt_pending_start: request.source.preempts_pending_start(),
        })
        .await?;
    let deferred = receipt.activation == crate::wallframe::routing::AssignmentActivation::Deferred;
    log::debug!(
        "wallpaper apply: id={} source={} targets={:?} sharing={} activation={}",
        entry.item_id,
        request.source.as_str(),
        target_ids,
        if duplicate_renderers {
            "duplicate"
        } else {
            "shared"
        },
        if deferred { "deferred" } else { "active" }
    );
    for renderer in &receipt.active_renderers {
        wait_for_apply_frame(app, renderer, request.first_frame_timeout).await?;
    }
    let renderer_id = receipt.renderer_id;

    {
        let mut q = app.queue.lock().await;
        q.current = Some(entry.item_id.to_string());
        // Stash the DB id so sequential / random traversal has an anchor.
        q.last_db_id = Some(entry.item_id);
    }

    let wp_id = entry.item_id.to_string();
    let independent_ids = resolved_targets
        .iter()
        .filter_map(|target| match target.id {
            crate::wallframe::routing::ConfigTargetId::Display(display_id) => Some(display_id),
            crate::wallframe::routing::ConfigTargetId::Canvas(_) => None,
        })
        .collect::<Vec<_>>();
    let keys = app.router.display_settings_keys(&independent_ids).await;
    let live_displays = app.router.snapshot_displays().await;
    let key_counts = live_displays.iter().fold(
        std::collections::HashMap::<String, usize>::new(),
        |mut counts, display| {
            *counts.entry(display.settings_key.clone()).or_default() += 1;
            counts
        },
    );
    app.settings.update(|s| {
        for (_did, key) in &keys {
            if key_counts.get(key).copied().unwrap_or_default() != 1 {
                continue;
            }
            let prefs = s.displays.entry(key.clone()).or_default();
            prefs.last_wallpaper = Some(wp_id.clone());
        }
        s.global.last_wallpaper = Some(wp_id.clone());
    });
    let canvas_ids = resolved_targets
        .iter()
        .filter_map(|target| match &target.id {
            crate::wallframe::routing::ConfigTargetId::Canvas(canvas_id) => Some(canvas_id.clone()),
            crate::wallframe::routing::ConfigTargetId::Display(_) => None,
        })
        .collect::<Vec<_>>();
    for canvas_id in &canvas_ids {
        app.settings
            .set_canvas_wallpaper(canvas_id, Some(wp_id.clone()))?;
    }
    if !canvas_ids.is_empty() {
        app.router.publish_canvas_snapshot().await;
    }
    // Flush recall state now so a crash inside the debounce window does
    // not lose the wallpaper needed by the next startup.
    app.settings.flush_now().await;
    crate::system::dbus::notify_current_wallpaper_id_changed(app).await;

    Ok(ApplyResult {
        renderer_id,
        entry,
        stopped_playlists,
        activation: if deferred {
            ApplyActivation::Deferred
        } else {
            ApplyActivation::Active
        },
        targets: resolved_targets
            .into_iter()
            .map(|target| match target.id {
                crate::wallframe::routing::ConfigTargetId::Display(display_id) => {
                    ApplyTarget::Display(display_id)
                }
                crate::wallframe::routing::ConfigTargetId::Canvas(canvas_id) => {
                    ApplyTarget::Canvas(canvas_id)
                }
            })
            .collect(),
        display_ids: target_ids,
    })
}
