use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context};
use ashpd::desktop::wallpaper::{SetOn, WallpaperRequest};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::error::{Error, Result};
use crate::events::GlobalEvent;
use crate::model::{repo, sync};
use crate::plugin::renderer_registry::{PluginPackageMeta, PluginScan, RendererRegistry};
use crate::plugin::update::{PluginUpdateInfo, PluginUpdateState};
use crate::queue::rotator::RotationConfig;
use crate::queue::Mode;
use crate::renderer_manager;
use crate::scheduler::DisplayId;
use crate::wallpaper::types::WallpaperEntry;
use crate::AppState;

pub const APPLY_FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(15);
const PLUGIN_UPDATE_NOTIFICATION_ID: &str = "org.waywallen.waywallen.plugin-updates";

/// Re-export so callers that already wrote `control::QueueState`
/// don't have to chase the move into the `playlist` module.
pub use crate::queue::QueueState;

pub struct ApplyResult {
    pub renderer_id: String,
    pub entry: WallpaperEntry,
}

#[derive(Clone, Debug, Default)]
pub struct ApplyOptions {
    pub display_ids: Option<Vec<DisplayId>>,
    pub renderer_name: Option<String>,
    pub first_frame_timeout: Option<Duration>,
    pub require_display: bool,
}

pub struct PluginInstallResult {
    pub plugin_id: String,
    pub needs_restart: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActivePluginIdentity {
    version: String,
    system: bool,
}

fn active_plugin_identity(
    packages: &[PluginPackageMeta],
    plugin_id: &str,
) -> Option<ActivePluginIdentity> {
    packages
        .iter()
        .find(|p| p.id == plugin_id)
        .map(|p| ActivePluginIdentity {
            version: p.version.clone(),
            system: p.system,
        })
}

fn registry_from_scan(scan: &PluginScan) -> RendererRegistry {
    let mut registry = RendererRegistry::new();
    for def in &scan.renderers {
        registry.register(def.clone());
    }
    registry
}

async fn reload_source_entries(
    app: &Arc<AppState>,
    entries: Vec<crate::plugin::renderer_registry::EntryRef>,
    installed_plugin_id: &str,
) -> Result<()> {
    let installed_plugin_id = installed_plugin_id.to_string();
    let source_manager = app.source_manager.clone();
    let load_result = tokio::task::spawn_blocking(move || {
        let mut sm = source_manager.blocking_lock();
        sm.clear_plugins();

        let mut installed_failures = Vec::new();
        for r in &entries {
            if let Err(e) =
                sm.load_plugin(&r.entry, &r.plugin_id, &r.plugin_version, r.entry_version)
            {
                let msg = format!("load entry {}: {e:#}", r.entry.display());
                log::warn!("{msg}");
                if r.plugin_id == installed_plugin_id {
                    installed_failures.push(msg);
                }
            }
        }

        if installed_failures.is_empty() {
            Ok(())
        } else {
            Err(installed_failures.join("; "))
        }
    })
    .await
    .map_err(|e| Error::Internal(anyhow!("source reload join: {e}")))?;

    load_result.map_err(Error::PluginInstallFailed)?;

    let infos = {
        let sm = app.source_manager.lock().await;
        sm.plugins()?
    };
    for info in &infos {
        repo::upsert_plugin(&app.db, &info.name, &info.version)
            .await
            .map_err(|e| Error::Internal(anyhow!("upsert plugin {}: {e:#}", info.name)))?;
    }
    *app.source_plugins.write().await = infos;
    Ok(())
}

async fn apply_plugin_scan(
    app: &Arc<AppState>,
    scan: PluginScan,
    installed_plugin_id: &str,
) -> Result<Vec<PluginPackageMeta>> {
    let registry = registry_from_scan(&scan);
    let packages = scan.packages();

    app.renderer_manager.replace_registry(registry.clone());
    *app.plugins.write().await = packages.clone();
    *app.inactive_system.write().await = scan.inactive_system.clone();
    *app.inactive_user.write().await = scan.inactive_user.clone();
    app.plugin_updates.write().await.remove(installed_plugin_id);

    if app.settings.reconcile(&registry) {
        app.events
            .publish(crate::events::GlobalEvent::SettingsChanged);
        app.settings.flush_now().await;
    }

    reload_source_entries(app, scan.entries, installed_plugin_id).await?;
    app.events.publish(GlobalEvent::PluginChanged);
    Ok(packages)
}

pub async fn check_plugin_updates(
    app: &Arc<AppState>,
    plugin_id: Option<&str>,
) -> Vec<crate::plugin::update::PluginUpdateInfo> {
    let _guard = app.plugin_update_check.lock().await;
    let mut packages = app.plugins.read().await.clone();
    let plugin_id = plugin_id.filter(|id| !id.is_empty());
    if let Some(plugin_id) = plugin_id {
        packages.retain(|pkg| pkg.id == plugin_id);
    }
    let updates =
        crate::plugin::update::check_packages(&app.plugin_updates, packages, plugin_id.is_none())
            .await;
    if !updates.is_empty() {
        app.events.publish(GlobalEvent::PluginUpdateChanged);
    }
    updates
}

async fn check_plugin_updates_with_progress<F>(
    app: &Arc<AppState>,
    plugin_id: Option<&str>,
    on_progress: F,
) -> Vec<crate::plugin::update::PluginUpdateInfo>
where
    F: FnMut(f32) + Send,
{
    let _guard = app.plugin_update_check.lock().await;
    let mut packages = app.plugins.read().await.clone();
    let plugin_id = plugin_id.filter(|id| !id.is_empty());
    if let Some(plugin_id) = plugin_id {
        packages.retain(|pkg| pkg.id == plugin_id);
    }
    let updates = crate::plugin::update::check_packages_with_progress(
        &app.plugin_updates,
        packages,
        plugin_id.is_none(),
        on_progress,
    )
    .await;
    if !updates.is_empty() {
        app.events.publish(GlobalEvent::PluginUpdateChanged);
    }
    updates
}

pub async fn plugin_update_snapshots(
    app: &Arc<AppState>,
    plugin_id: Option<&str>,
) -> Vec<crate::plugin::update::PluginUpdateInfo> {
    let mut packages = app.plugins.read().await.clone();
    let plugin_id = plugin_id.filter(|id| !id.is_empty());
    if let Some(plugin_id) = plugin_id {
        packages.retain(|pkg| pkg.id == plugin_id);
    }
    let mut out = Vec::with_capacity(packages.len());
    for pkg in packages {
        out.push(crate::plugin::update::snapshot_for_package(&app.plugin_updates, &pkg).await);
    }
    out
}

async fn notify_new_plugin_updates(
    app: &Arc<AppState>,
    previous: &HashMap<String, PluginUpdateInfo>,
    updates: &[PluginUpdateInfo],
) {
    if !app.settings.global().plugin_update_notifications {
        return;
    }

    let available = updates
        .iter()
        .filter(|info| crate::plugin::update::became_available(previous.get(&info.plugin_id), info))
        .collect::<Vec<_>>();
    if available.is_empty() {
        return;
    }

    let plugin_names = app
        .plugins
        .read()
        .await
        .iter()
        .map(|pkg| (pkg.id.clone(), pkg.name.clone()))
        .collect::<HashMap<_, _>>();
    let (summary, body) = plugin_update_notification_text(&available, &plugin_names);
    if let Err(e) =
        crate::notifications::notify(PLUGIN_UPDATE_NOTIFICATION_ID, &summary, &body).await
    {
        log::warn!("plugin update notification failed: {e}");
    }
}

fn plugin_update_notification_text(
    updates: &[&PluginUpdateInfo],
    plugin_names: &HashMap<String, String>,
) -> (String, String) {
    if let [info] = updates {
        return (
            "Plugin update available".into(),
            format!("{} is available.", plugin_update_label(info, plugin_names)),
        );
    }

    let mut labels = updates
        .iter()
        .take(3)
        .map(|info| plugin_update_label(info, plugin_names))
        .collect::<Vec<_>>();
    if updates.len() > labels.len() {
        let remaining = updates.len() - labels.len();
        labels.push(format!("{remaining} more"));
    }
    (
        format!("{} plugin updates available", updates.len()),
        format!("Available: {}.", labels.join(", ")),
    )
}

fn plugin_update_label(info: &PluginUpdateInfo, plugin_names: &HashMap<String, String>) -> String {
    let name = plugin_names
        .get(&info.plugin_id)
        .filter(|name| !name.is_empty())
        .unwrap_or(&info.plugin_id);
    if info.latest_version.is_empty() {
        name.clone()
    } else {
        format!("{name} {}", info.latest_version)
    }
}

pub fn plugin_update_check_query_id(plugin_id: Option<&str>) -> String {
    match plugin_id.filter(|id| !id.is_empty()) {
        Some(plugin_id) => format!("plugin/update-check/{plugin_id}"),
        None => "plugin/update-check/all".into(),
    }
}

pub fn spawn_plugin_update_check(
    app: &Arc<AppState>,
    plugin_id: Option<String>,
) -> crate::tasks::ProgressTaskSubmission {
    let query_id = plugin_update_check_query_id(plugin_id.as_deref());
    let event_sender = app.events.sender();
    let sink: crate::tasks::ProgressSink = Arc::new(move |progress| {
        let _ = event_sender.send(GlobalEvent::TaskProgress(progress));
    });
    let task_app = app.clone();
    let task_plugin_id = plugin_id.clone();
    app.tasks.spawn_progress_async_once(
        crate::tasks::TaskKind::Generic,
        query_id.clone(),
        query_id,
        sink,
        move |reporter| async move {
            let progress_reporter = reporter.clone();
            let _ = check_plugin_updates_with_progress(
                &task_app,
                task_plugin_id.as_deref(),
                move |progress| progress_reporter.report(progress, ""),
            )
            .await;
            Ok(())
        },
    )
}

pub fn plugin_update_install_query_id(plugin_id: &str) -> String {
    format!("plugin/update-install/{plugin_id}")
}

pub fn spawn_plugin_update_install(
    app: &Arc<AppState>,
    plugin_id: String,
) -> Result<crate::tasks::ProgressTaskSubmission> {
    let plugin_id = plugin_id.trim().to_string();
    if plugin_id.is_empty() {
        return Err(Error::InvalidArgument("plugin id is empty".into()));
    }

    let query_id = plugin_update_install_query_id(&plugin_id);
    let event_sender = app.events.sender();
    let sink: crate::tasks::ProgressSink = Arc::new(move |progress| {
        let _ = event_sender.send(GlobalEvent::TaskProgress(progress));
    });
    let task_app = app.clone();
    let task_plugin_id = plugin_id.clone();
    Ok(app.tasks.spawn_progress_async_once(
        crate::tasks::TaskKind::Generic,
        query_id.clone(),
        query_id,
        sink,
        move |reporter| async move {
            let info = plugin_update_info_for_install(&task_app, &task_plugin_id)
                .await
                .map_err(anyhow::Error::from)?;
            reporter.report(0.05, "");
            let archive = download_plugin_update_archive(&info, reporter.clone())
                .await
                .map_err(anyhow::Error::from)?;
            let result =
                install_downloaded_plugin_update(&task_app, &task_plugin_id, &archive, reporter)
                    .await
                    .map_err(anyhow::Error::from);
            let _ = tokio::fs::remove_file(&archive).await;
            result
        },
    ))
}

async fn plugin_update_info_for_install(
    app: &Arc<AppState>,
    plugin_id: &str,
) -> Result<PluginUpdateInfo> {
    let active = app
        .plugins
        .read()
        .await
        .iter()
        .any(|pkg| pkg.id == plugin_id);
    if !active {
        return Err(Error::InvalidArgument(format!(
            "plugin '{plugin_id}' is not active"
        )));
    }

    let Some(info) = app.plugin_updates.read().await.get(plugin_id).cloned() else {
        return Err(Error::FailedPrecondition(format!(
            "plugin '{plugin_id}' has no checked update"
        )));
    };
    if info.state != PluginUpdateState::Available {
        return Err(Error::FailedPrecondition(format!(
            "plugin '{plugin_id}' has no available update"
        )));
    }
    if info.zip_url.trim().is_empty() {
        return Err(Error::PluginInstallFailed(format!(
            "plugin '{plugin_id}' update has no zip url"
        )));
    }
    if info.sha256.trim().is_empty() {
        return Err(Error::PluginInstallFailed(format!(
            "plugin '{plugin_id}' update has no sha256"
        )));
    }
    Ok(info)
}

async fn download_plugin_update_archive(
    info: &PluginUpdateInfo,
    reporter: crate::tasks::ProgressReporter,
) -> Result<PathBuf> {
    let tmp_dir = std::env::temp_dir().join("waywallen-plugin-updates");
    tokio::fs::create_dir_all(&tmp_dir).await?;
    let unique = uuid::Uuid::new_v4();
    let part = tmp_dir.join(format!("{unique}.part"));
    let archive = tmp_dir.join(format!("{unique}.zip"));
    let result = download_plugin_update_archive_inner(info, &part, &archive, reporter).await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
        let _ = tokio::fs::remove_file(&archive).await;
    }
    result
}

async fn download_plugin_update_archive_inner(
    info: &PluginUpdateInfo,
    part: &Path,
    archive: &Path,
    reporter: crate::tasks::ProgressReporter,
) -> Result<PathBuf> {
    let expected = normalize_sha256(&info.sha256)?;
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) waywallen")
        .build()
        .context("build plugin update download client")?;
    let response = client
        .get(&info.zip_url)
        .send()
        .await
        .with_context(|| format!("download plugin update {}", info.zip_url))?
        .error_for_status()
        .with_context(|| format!("download plugin update response {}", info.zip_url))?;
    let total = response.content_length();
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(part).await?;
    let mut hasher = Sha256::new();
    let mut received = 0u64;

    reporter.report(0.10, "");
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("download plugin update chunk")?;
        file.write_all(&chunk).await?;
        hasher.update(&chunk);
        received = received.saturating_add(chunk.len() as u64);
        let progress = total
            .filter(|total| *total > 0)
            .map(|total| received as f32 / total as f32)
            .unwrap_or(0.5);
        reporter.report(0.10 + progress.clamp(0.0, 1.0) * 0.65, "");
    }
    file.flush().await?;
    drop(file);

    let actual = hex_lower(&hasher.finalize());
    if actual != expected {
        return Err(Error::PluginInstallFailed(format!(
            "plugin '{}' update sha256 mismatch",
            info.plugin_id
        )));
    }
    reporter.report(0.78, "");
    tokio::fs::rename(part, archive).await?;
    Ok(archive.to_path_buf())
}

fn normalize_sha256(value: &str) -> Result<String> {
    let trimmed = value.trim().to_ascii_lowercase();
    if trimmed.len() == 64 && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(trimmed)
    } else {
        Err(Error::PluginInstallFailed("invalid update sha256".into()))
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

async fn install_downloaded_plugin_update(
    app: &Arc<AppState>,
    plugin_id: &str,
    archive: &Path,
    reporter: crate::tasks::ProgressReporter,
) -> Result<()> {
    reporter.report(0.82, "");
    let inspect_path = archive.to_string_lossy().to_string();
    let info =
        tokio::task::spawn_blocking(move || crate::plugin::installer::inspect_zip(&inspect_path))
            .await
            .map_err(|e| Error::Internal(anyhow!("plugin update inspect join: {e}")))??;
    if info.id != plugin_id {
        return Err(Error::PluginInstallFailed(format!(
            "update archive id '{}' does not match '{}'",
            info.id, plugin_id
        )));
    }

    reporter.report(0.90, "");
    let install_path = archive.to_string_lossy().to_string();
    let result = install_plugin_archive(app, install_path).await?;
    if result.plugin_id != plugin_id {
        return Err(Error::PluginInstallFailed(format!(
            "installed plugin '{}' does not match '{}'",
            result.plugin_id, plugin_id
        )));
    }
    app.events.publish(GlobalEvent::PluginUpdateChanged);
    Ok(())
}

pub async fn run_plugin_update_checker(
    app: Arc<AppState>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    if *shutdown.borrow() {
        return Ok(());
    }

    let initial_delay = tokio::time::sleep(Duration::from_secs(10));
    tokio::pin!(initial_delay);
    tokio::select! {
        _ = shutdown.changed() => return Ok(()),
        _ = &mut initial_delay => {}
    }

    loop {
        let previous = app.plugin_updates.read().await.clone();
        let updates = check_plugin_updates(&app, None).await;
        notify_new_plugin_updates(&app, &previous, &updates).await;

        let wait = tokio::time::sleep(Duration::from_secs(30 * 60));
        tokio::pin!(wait);
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            _ = &mut wait => {}
        }
    }
}

async fn affected_display_plan(
    app: &Arc<AppState>,
    renderer_ids: &[renderer_manager::RendererId],
) -> BTreeMap<String, Vec<DisplayId>> {
    let affected: BTreeSet<_> = renderer_ids.iter().cloned().collect();
    let mut plan: BTreeMap<String, BTreeSet<DisplayId>> = BTreeMap::new();

    for display in app.router.snapshot_displays().await {
        if !display
            .links
            .iter()
            .any(|link| affected.contains(&link.renderer_id))
        {
            continue;
        }

        let key = display.instance_id.as_deref().unwrap_or(&display.name);
        let Some(wallpaper_id) = app.settings.resolved_last_wallpaper(key) else {
            log::warn!(
                "plugin install: display {} has no last wallpaper; cannot restart renderer link",
                display.name
            );
            continue;
        };
        plan.entry(wallpaper_id).or_default().insert(display.id);
    }

    plan.into_iter()
        .map(|(wallpaper_id, display_ids)| {
            (wallpaper_id, display_ids.into_iter().collect::<Vec<_>>())
        })
        .collect()
}

async fn restart_affected_renderers(
    app: &Arc<AppState>,
    renderer_ids: Vec<renderer_manager::RendererId>,
) -> Result<()> {
    if renderer_ids.is_empty() {
        return Ok(());
    }

    let plan = affected_display_plan(app, &renderer_ids).await;
    for (wallpaper_id, display_ids) in plan {
        if display_ids.is_empty() {
            continue;
        }
        apply_wallpaper_to_displays_with_first_frame_timeout(
            app,
            &wallpaper_id,
            &display_ids,
            APPLY_FIRST_FRAME_TIMEOUT,
        )
        .await?;
    }

    for renderer_id in renderer_ids {
        if app.renderer_manager.get(&renderer_id).await.is_none() {
            continue;
        }
        app.router
            .stop_renderers_orderly(&[renderer_id], Duration::from_secs(1))
            .await;
    }
    Ok(())
}

fn spawn_affected_renderer_restart(
    app: &Arc<AppState>,
    plugin_id: String,
    renderer_ids: Vec<renderer_manager::RendererId>,
) {
    if renderer_ids.is_empty() {
        return;
    }

    let app = app.clone();
    let tasks = app.tasks.clone();
    let task_name = format!("plugin-restart/{plugin_id}");
    tasks.spawn_async(crate::tasks::TaskKind::Generic, task_name, async move {
        if let Err(e) = restart_affected_renderers(&app, renderer_ids).await {
            let error = format!("{e:#}");
            log::warn!("plugin restart failed for {plugin_id}: {error}");
            app.events
                .publish(GlobalEvent::PluginRestartFailed { plugin_id, error });
        }
        Ok(())
    });
}

fn spawn_source_refresh(app: &Arc<AppState>, plugin_id: &str) {
    let app = app.clone();
    let tasks = app.tasks.clone();
    let task_name = format!("plugin-refresh/{plugin_id}");
    tasks.spawn_async_unique(
        crate::tasks::TaskKind::Generic,
        "source/plugin-refresh",
        task_name,
        async move {
            let skip_refresh = repo::list_libraries(&app.db)
                .await
                .map(|v| v.is_empty())
                .unwrap_or(false);
            if !skip_refresh {
                refresh_sources(&app).await.map(|_| ())?;
            }
            Ok(())
        },
    );
}

pub async fn install_plugin_archive(
    app: &Arc<AppState>,
    zip_path: String,
) -> Result<PluginInstallResult> {
    let _guard = app.plugin_mutation.lock().await;

    let plugin_id =
        tokio::task::spawn_blocking(move || crate::plugin::installer::install_zip(&zip_path))
            .await
            .map_err(|e| Error::Internal(anyhow!("install join: {e}")))??;

    let old_packages = app.plugins.read().await.clone();
    let old_active = active_plugin_identity(&old_packages, &plugin_id);
    let old_renderer_ids = app
        .renderer_manager
        .live_renderer_ids_by_plugin_id(&plugin_id)
        .await;

    let plugin_roots = app.plugin_roots.clone();
    let plugin_scan = tokio::task::spawn_blocking(move || {
        crate::plugin::renderer_registry::scan_plugin_roots(plugin_roots.as_slice())
    })
    .await
    .map_err(|e| Error::Internal(anyhow!("plugin scan join: {e}")))?;

    let new_packages = apply_plugin_scan(app, plugin_scan, &plugin_id).await?;
    let new_active = active_plugin_identity(&new_packages, &plugin_id);
    let active_user_install = new_active.as_ref().is_some_and(|p| !p.system);
    let should_restart_renderers =
        !old_renderer_ids.is_empty() && (active_user_install || old_active != new_active);

    if should_restart_renderers {
        spawn_affected_renderer_restart(app, plugin_id.clone(), old_renderer_ids);
    }

    spawn_source_refresh(app, &plugin_id);

    Ok(PluginInstallResult {
        plugin_id,
        needs_restart: false,
    })
}

/// Apply a wallpaper by id to every registered display.
/// Supersedes any in-flight global apply task.
pub async fn apply_wallpaper_by_id(app: &Arc<AppState>, id: &str) -> Result<ApplyResult> {
    let app_clone = app.clone();
    let id_owned = id.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<ApplyResult>>();
    app.tasks.spawn_async_unique(
        crate::tasks::TaskKind::Apply,
        "apply/global",
        format!("apply/{id_owned}"),
        async move {
            let res =
                apply_wallpaper_with_options(&app_clone, &id_owned, ApplyOptions::default()).await;
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
    app: &Arc<AppState>,
    id: &str,
    target: &[DisplayId],
) -> Result<ApplyResult> {
    if target.is_empty() {
        return Err(Error::Internal(anyhow!(
            "apply_wallpaper_to_displays: empty target"
        )));
    }
    apply_wallpaper_with_options(
        app,
        id,
        ApplyOptions {
            display_ids: Some(target.to_vec()),
            ..Default::default()
        },
    )
    .await
}

pub async fn apply_wallpaper_to_displays_with_first_frame_timeout(
    app: &Arc<AppState>,
    id: &str,
    target: &[DisplayId],
    timeout: Duration,
) -> Result<ApplyResult> {
    if target.is_empty() {
        return Err(Error::Internal(anyhow!(
            "apply_wallpaper_to_displays: empty target"
        )));
    }
    apply_wallpaper_with_options(
        app,
        id,
        ApplyOptions {
            display_ids: Some(target.to_vec()),
            first_frame_timeout: Some(timeout),
            ..Default::default()
        },
    )
    .await
}

pub struct PortalApplyResult {
    pub wallpaper_id: String,
    pub uri: String,
}

/// Apply an image wallpaper through `org.freedesktop.portal.Wallpaper`.
/// The portal owns preview, prompting, and final rendering.
pub async fn apply_wallpaper_via_portal(
    app: &Arc<AppState>,
    id: &str,
) -> Result<PortalApplyResult> {
    let app_clone = app.clone();
    let id_owned = id.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<PortalApplyResult>>();
    app.tasks.spawn_async_unique(
        crate::tasks::TaskKind::Apply,
        "apply/portal",
        format!("apply-portal/{id_owned}"),
        async move {
            let res = apply_via_portal_inner(&app_clone, &id_owned).await;
            let _ = tx.send(res);
            Ok(())
        },
    );
    rx.await
        .map_err(|_| Error::Internal(anyhow!("apply task superseded or cancelled")))?
}

async fn apply_via_portal_inner(app: &Arc<AppState>, id: &str) -> Result<PortalApplyResult> {
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

fn resolve_renderer_plugin_name(
    app: &Arc<AppState>,
    entry: &WallpaperEntry,
    renderer_name: Option<&str>,
) -> Result<String> {
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
            Ok(def.name.clone())
        }
        _ => registry
            .resolve(&entry.wp_type)
            .map(|def| def.name.clone())
            .ok_or_else(|| Error::NoRendererForType(entry.wp_type.clone())),
    }
}

async fn reusable_renderer_for_target(
    app: &Arc<AppState>,
    spawn_req: &renderer_manager::SpawnRequest,
    target_ids: &[DisplayId],
    duplicate_renderers: bool,
) -> Option<String> {
    if !duplicate_renderers {
        return app.renderer_manager.find_reusable(spawn_req).await;
    }

    for id in app.renderer_manager.reusable_renderer_ids(spawn_req).await {
        let linked = app.router.renderer_display_ids(&id).await;
        if linked.is_empty() || linked.iter().all(|did| target_ids.contains(did)) {
            return Some(id);
        }
    }
    None
}

async fn spawn_renderer_for_target(
    app: &Arc<AppState>,
    spawn_req: renderer_manager::SpawnRequest,
    target: Option<&[DisplayId]>,
) -> Result<String> {
    let to_stop = app.router.renderers_fully_replaced_by(target).await;
    if !to_stop.is_empty() {
        app.router
            .stop_renderers_orderly(&to_stop, Duration::from_secs(1))
            .await;
    }

    let new_id = app
        .renderer_manager
        .spawn(spawn_req)
        .await
        .map_err(|e| Error::RendererSpawnFailed(e.to_string()))?;
    if let Some(handle) = app.renderer_manager.get(&new_id).await {
        app.router.register_renderer(handle).await;
    }
    Ok(new_id)
}

async fn wait_for_apply_frame(
    app: &Arc<AppState>,
    renderer_id: &str,
    timeout: Option<Duration>,
) -> Result<()> {
    let Some(timeout) = timeout else {
        return Ok(());
    };
    if let Err(e) = app
        .renderer_manager
        .wait_for_first_frame(renderer_id, timeout)
        .await
    {
        app.router.unregister_renderer(renderer_id).await;
        let _ = app.renderer_manager.kill(renderer_id).await;
        return Err(e);
    }
    Ok(())
}

async fn apply_shared_renderer(
    app: &Arc<AppState>,
    spawn_req: renderer_manager::SpawnRequest,
    target: Option<&[DisplayId]>,
    target_ids: &[DisplayId],
    first_frame_timeout: Option<Duration>,
) -> Result<String> {
    let renderer_id = match reusable_renderer_for_target(app, &spawn_req, target_ids, false).await {
        Some(existing) => existing,
        None => spawn_renderer_for_target(app, spawn_req, target).await?,
    };
    match target {
        None => app.router.relink_all_displays_to(&renderer_id).await,
        Some(ids) => app.router.relink_displays_to(ids, &renderer_id).await,
    }
    wait_for_apply_frame(app, &renderer_id, first_frame_timeout).await?;
    Ok(renderer_id)
}

async fn apply_duplicate_renderers(
    app: &Arc<AppState>,
    spawn_req: &renderer_manager::SpawnRequest,
    target_ids: &[DisplayId],
    first_frame_timeout: Option<Duration>,
) -> Result<String> {
    let mut first_renderer_id: Option<String> = None;
    for did in target_ids {
        let single = [*did];
        let renderer_id = match reusable_renderer_for_target(app, spawn_req, &single, true).await {
            Some(existing) => existing,
            None => spawn_renderer_for_target(app, spawn_req.clone(), Some(&single)).await?,
        };
        app.router.relink_displays_to(&single, &renderer_id).await;
        wait_for_apply_frame(app, &renderer_id, first_frame_timeout).await?;
        first_renderer_id.get_or_insert(renderer_id);
    }

    first_renderer_id.ok_or(Error::NoDisplayRegistered)
}

/// Shared global/per-display apply core.
/// Spawns or reuses renderers, relinks displays, and persists recall state.
pub async fn apply_wallpaper_with_options(
    app: &Arc<AppState>,
    id: &str,
    options: ApplyOptions,
) -> Result<ApplyResult> {
    let entry = match id.parse::<i64>() {
        Ok(iid) => repo::get_entry(&app.db, iid).await?,
        Err(_) => None,
    };
    let entry = entry.ok_or_else(|| Error::WallpaperNotFound(id.to_string()))?;

    if options.require_display && app.router.display_count().await == 0 {
        return Err(Error::NoDisplayRegistered);
    }

    let renderer_plugin_name =
        resolve_renderer_plugin_name(app, &entry, options.renderer_name.as_deref())?;
    let extras = app
        .source_manager
        .lock()
        .await
        .call_extras(&entry.plugin_name, &entry)
        .await?;
    let spawn_settings = app
        .settings
        .plugin(&renderer_plugin_name)
        .unwrap_or_default();
    let user_properties_json =
        repo::get_wallpaper_render_properties(&app.db, entry.item_id).await?;
    let spawn_req = renderer_manager::SpawnRequest {
        wp_type: entry.wp_type.clone(),
        extras,
        settings: spawn_settings,
        test_pattern: false,
        renderer_name: options
            .renderer_name
            .as_ref()
            .filter(|name| !name.is_empty())
            .map(|_| renderer_plugin_name.clone()),
        user_properties_json,
    };
    let target = options.display_ids.as_deref();
    let target_ids = app.router.registered_display_ids(target).await;
    if options.require_display && target_ids.is_empty() {
        return Err(Error::NoDisplayRegistered);
    }
    let duplicate_renderers =
        app.settings.global().duplicate_renderers_for_same_wallpaper && !target_ids.is_empty();
    let renderer_id = if duplicate_renderers {
        apply_duplicate_renderers(app, &spawn_req, &target_ids, options.first_frame_timeout).await?
    } else {
        apply_shared_renderer(
            app,
            spawn_req,
            target,
            &target_ids,
            options.first_frame_timeout,
        )
        .await?
    };

    {
        let mut q = app.queue.lock().await;
        q.current = Some(entry.item_id.to_string());
        // Stash the DB id so sequential / random traversal has an anchor.
        q.last_db_id = Some(entry.item_id);
    }

    let keys = app.router.display_settings_keys(&target_ids).await;
    let wp_id = entry.item_id.to_string();
    app.settings.update(|s| {
        for (_did, key) in &keys {
            let prefs = s.displays.entry(key.clone()).or_default();
            prefs.last_wallpaper = Some(wp_id.clone());
        }
        s.global.last_wallpaper = Some(wp_id);
    });
    // Flush recall state now so a crash inside the debounce window does
    // not lose the wallpaper needed by the next startup.
    app.settings.flush_now().await;
    crate::dbus_iface::notify_current_wallpaper_id_changed(app).await;

    Ok(ApplyResult { renderer_id, entry })
}

pub async fn step_pick(app: &Arc<AppState>, delta: i32) -> Result<String> {
    use crate::model::repo::QueueRow;
    use crate::queue::Mode;

    let (filters, logics) = app.settings.global().wallpaper_queue_filter();
    let sorts =
        crate::settings::WallpaperSortRuleState::vec_to_pb(&app.settings.global().wallpaper_sorts);
    let mode = app.queue.lock().await.mode;

    let entry_id: String = match mode {
        Mode::Sequential => step_sequential(app, delta, &filters, &logics, &sorts).await?,
        Mode::Random => {
            let exclude = app.queue.lock().await.last_db_id;
            let row: QueueRow = repo::random_item_by_filter(&app.db, &filters, &logics, exclude)
                .await?
                .ok_or_else(|| Error::FailedPrecondition("queue is empty".into()))?;
            bridge_to_entry_id(&row)
        }
        Mode::Shuffle => {
            let row = step_shuffle(app, &filters, &logics, delta).await?;
            bridge_to_entry_id(&row)
        }
    };
    Ok(entry_id)
}

pub async fn step(app: &Arc<AppState>, delta: i32) -> Result<String> {
    let entry_id = step_pick(app, delta).await?;
    apply_wallpaper_by_id(app, &entry_id).await?;
    app.rotation.kick();
    Ok(entry_id)
}

/// Walk the sorted+filtered entry list by `delta`, wrapping with `rem_euclid`.
/// If the current entry is absent, start at the first or last item.
async fn step_sequential(
    app: &Arc<AppState>,
    delta: i32,
    filters: &[crate::control_proto::WallpaperFilterRule],
    logics: &[crate::control_proto::FilterLogic],
    sorts: &[crate::control_proto::WallpaperSortRule],
) -> Result<String> {
    let ordered = crate::wallpaper::sort::ordered_entry_ids(app, filters, logics, sorts).await?;
    if ordered.is_empty() {
        return Err(Error::FailedPrecondition("queue is empty".into()));
    }
    let len = ordered.len() as i64;
    let current = app.queue.lock().await.current.clone();
    let cur_idx = current
        .as_deref()
        .and_then(|c| ordered.iter().position(|id| id == c));
    let next_idx = match cur_idx {
        Some(i) => ((i as i64) + delta as i64).rem_euclid(len) as usize,
        None => {
            if delta >= 0 {
                0
            } else {
                (len - 1) as usize
            }
        }
    };
    Ok(ordered[next_idx].clone())
}

/// Bridge a DB queue row to the `WallpaperApply` argument. Identity is
/// the DB `item.id`, which the row already carries.
fn bridge_to_entry_id(row: &repo::QueueRow) -> String {
    row.item_id.to_string()
}

async fn step_shuffle(
    app: &Arc<AppState>,
    filters: &[crate::control_proto::WallpaperFilterRule],
    logics: &[crate::control_proto::FilterLogic],
    delta: i32,
) -> Result<repo::QueueRow> {
    // Lock-free preflight: snapshot whether the round is empty so we
    // can fetch ids without holding the queue mutex through the DB call.
    let need_round = {
        let q = app.queue.lock().await;
        q.shuffle_round.is_empty()
    };
    if need_round {
        let ids = repo::list_item_ids_by_filter(&app.db, filters, logics).await?;
        if ids.is_empty() {
            return Err(Error::FailedPrecondition("queue is empty".into()));
        }
        let mut q = app.queue.lock().await;
        let avoid = q.last_db_id;
        q.build_shuffle_round(ids, avoid, 0);
        let pick = q.shuffle_round[0];
        q.shuffle_pos = 0;
        drop(q);
        return repo::get_item_with_library(&app.db, pick)
            .await?
            .ok_or_else(|| Error::FailedPrecondition("queue is empty".into()));
    }

    let pick = {
        let mut q = app.queue.lock().await;
        let len = q.shuffle_round.len() as i64;
        let raw = q.shuffle_pos as i64 + delta as i64;
        if raw >= len || raw < 0 {
            // Wrap: rebuild the round.
            let avoid = q.last_db_id;
            let target = if raw >= len {
                0usize
            } else {
                q.shuffle_round.len().saturating_sub(1)
            };
            let candidates = q.shuffle_round.clone();
            q.build_shuffle_round(candidates, avoid, target);
            q.shuffle_pos = target;
        } else {
            q.shuffle_pos = raw as usize;
        }
        q.shuffle_round[q.shuffle_pos]
    };

    repo::get_item_with_library(&app.db, pick)
        .await?
        .ok_or_else(|| Error::FailedPrecondition("queue is empty".into()))
}

/// Set the rotation mode on the active playlist and persist it to settings.
pub async fn set_mode(app: &Arc<AppState>, mode: Mode) {
    app.queue.lock().await.set_mode(mode);
    app.settings.update(|s| {
        s.global.queue_mode = mode.as_str().to_owned();
    });
    crate::dbus_iface::notify_queue_mode_changed(app).await;
    crate::tray::dbusmenu::notify_menu_changed(app).await;
}

/// Set the auto-rotation interval in seconds; `0` disables rotation.
/// Updates the live rotator and persists the cadence to settings.
pub async fn set_rotation_interval(app: &Arc<AppState>, secs: u32) {
    app.rotation.set_interval(secs);
    app.settings.update(|s| {
        s.global.rotation_secs = secs;
    });
    crate::dbus_iface::notify_rotation_secs_changed(app).await;
    crate::tray::dbusmenu::notify_menu_changed(app).await;
}

/// Convenience: flip shuffle on/off without exposing the [`Mode`]
/// enum to D-Bus / WS callers. `true` → Shuffle, `false` → Sequential.
pub async fn set_shuffle(app: &Arc<AppState>, on: bool) {
    let mode = if on { Mode::Shuffle } else { Mode::Sequential };
    set_mode(app, mode).await;
}

/// Snapshot of the live playlist state for status reporting.
#[derive(Debug, Clone)]
pub struct QueueStatus {
    pub active_id: Option<i64>,
    pub mode: String,
    pub interval_secs: u32,
    pub current: Option<String>,
    pub position: Option<u32>,
    pub count: u32,
    pub is_smart: bool,
}

pub async fn queue_status(app: &Arc<AppState>) -> QueueStatus {
    let (filters, logics) = app.settings.global().wallpaper_queue_filter();
    let count = repo::count_items_by_filter(&app.db, &filters, &logics)
        .await
        .unwrap_or(0) as u32;
    // "smart" reflects user-authored filter rules only; the quick
    // skip-type toggles narrow the queue but don't make it a playlist.
    let is_smart = !app.settings.global().wallpaper_filter.filters.is_empty();
    let g = app.queue.lock().await;
    QueueStatus {
        active_id: None,
        mode: g.mode.as_str().to_owned(),
        interval_secs: app.rotation.interval(),
        current: g.current.clone(),
        position: None,
        count,
        is_smart,
    }
}

/// Restore queue mode, rotation cadence, and manual audio state from disk. Idempotent.
pub async fn run_restore(app: &Arc<AppState>) -> Result<()> {
    use crate::events::GlobalEvent;

    let g = app.settings.global();
    if let Some(mode) = crate::queue::Mode::from_str(&g.queue_mode) {
        app.queue.lock().await.set_mode(mode);
    }
    if g.rotation_secs > 0 {
        app.rotation.set_interval(g.rotation_secs);
    }
    app.router.set_manual_mute(g.manual_muted).await;

    app.events.publish(GlobalEvent::RestoreApplied(None));
    Ok(())
}

/// Auto-rotation task body.
/// Reads live cadence from a watch channel and applies the next wallpaper.
pub async fn run_rotator(
    app: Arc<AppState>,
    mut rx: tokio::sync::watch::Receiver<RotationConfig>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    log::info!("playlist rotator started");
    loop {
        let cfg = *rx.borrow();
        if cfg.interval_secs == 0 {
            tokio::select! {
                _ = rx.changed() => continue,
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() { break; }
                }
            }
        } else {
            let dur = std::time::Duration::from_secs(cfg.interval_secs as u64);
            tokio::select! {
                _ = tokio::time::sleep(dur) => {
                    if rx.borrow().interval_secs == 0 {
                        continue;
                    }
                    let owned = app.playlists.owned_display_ids().await;
                    let all: Vec<crate::scheduler::DisplayId> = app
                        .router
                        .snapshot_displays()
                        .await
                        .into_iter()
                        .map(|d| d.id)
                        .collect();
                    let unowned: Vec<_> =
                        all.into_iter().filter(|d| !owned.contains(d)).collect();
                    if unowned.is_empty() {
                        continue;
                    }
                    match step_pick(&app, 1).await {
                        Ok(id) => {
                            if let Err(e) =
                                apply_wallpaper_to_displays(&app, &id, &unowned).await
                            {
                                log::warn!("rotator apply failed: {e:#}");
                            }
                        }
                        Err(e) => log::warn!("rotator tick step failed: {e:#}"),
                    }
                }
                _ = rx.changed() => continue,
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() { break; }
                }
            }
        }
    }
    log::info!("playlist rotator exited");
}

pub async fn run_auto_stop_restore(
    app: Arc<AppState>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut rx = app.router.subscribe_auto_stop();
    log::info!("auto-stop restore service started");
    loop {
        tokio::select! {
            evt = rx.recv() => {
                match evt {
                    Ok(evt) if !evt.stopped => {
                        restore_auto_stopped_display(&app, evt.display_id).await;
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        log::warn!("auto-stop restore lagged {n} events");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
        }
    }
    log::info!("auto-stop restore service exited");
}

async fn restore_auto_stopped_display(app: &Arc<AppState>, display_id: DisplayId) {
    let Some(display) = app.router.snapshot_display(display_id).await else {
        return;
    };
    if !display.links.is_empty() {
        return;
    }
    let key = display.instance_id.as_deref().unwrap_or(&display.name);
    let Some(wallpaper_id) = app.settings.resolved_last_wallpaper(key) else {
        log::debug!("auto-stop restore: display {display_id} has no saved wallpaper");
        return;
    };
    if let Err(e) = apply_wallpaper_to_displays(app, &wallpaper_id, &[display_id]).await {
        log::warn!("auto-stop restore: apply {wallpaper_id} to display {display_id}: {e:#}");
    }
}

pub async fn pause_all(app: &Arc<AppState>) -> Result<()> {
    app.router.set_manual_pause(true).await;
    crate::tray::dbusmenu::notify_menu_changed(app).await;
    Ok(())
}

pub async fn resume_all(app: &Arc<AppState>) -> Result<()> {
    app.router.set_manual_pause(false).await;
    crate::tray::dbusmenu::notify_menu_changed(app).await;
    Ok(())
}

pub async fn toggle_pause_all(app: &Arc<AppState>) -> Result<bool> {
    let paused = app.router.toggle_manual_pause().await;
    crate::tray::dbusmenu::notify_menu_changed(app).await;
    Ok(paused)
}

pub async fn mute_all(app: &Arc<AppState>) -> Result<()> {
    app.router.set_manual_mute(true).await;
    app.settings.update(|s| {
        s.global.manual_muted = true;
    });
    crate::tray::dbusmenu::notify_menu_changed(app).await;
    Ok(())
}

pub async fn unmute_all(app: &Arc<AppState>) -> Result<()> {
    app.router.set_manual_mute(false).await;
    app.settings.update(|s| {
        s.global.manual_muted = false;
    });
    crate::tray::dbusmenu::notify_menu_changed(app).await;
    Ok(())
}

pub async fn toggle_mute_all(app: &Arc<AppState>) -> Result<bool> {
    let muted = app.router.toggle_manual_mute().await;
    app.settings.update(|s| {
        s.global.manual_muted = muted;
    });
    crate::tray::dbusmenu::notify_menu_changed(app).await;
    Ok(muted)
}

pub async fn rescan(app: &Arc<AppState>) -> Result<usize> {
    refresh_sources(app).await
}

/// Run source-plugin auto-detect and register any discovered libraries.
/// Duplicate libraries are skipped before a refresh is triggered.
pub async fn auto_detect_libraries(
    app: &Arc<AppState>,
) -> Result<Vec<crate::routing::LibrarySnapshot>> {
    use crate::routing::LibrarySnapshot;

    let detected = {
        let sm = app.source_manager.lock().await;
        sm.auto_detect_all().await?
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
        app.events
            .publish(crate::events::GlobalEvent::LibrariesAdded {
                paths: added.iter().map(|s| s.path.clone()).collect(),
            });
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

/// Load DB libraries into the router-wire `LibrarySnapshot` shape.
/// Used by library list queries and the initial WS snapshot.
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
        out.push(crate::routing::LibrarySnapshot {
            id: lib.id,
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
        let canon = std::fs::canonicalize(p).unwrap_or_else(|_| std::path::PathBuf::from(p));
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
pub async fn refresh_source_plugins(app: &Arc<AppState>) {
    let plugins = {
        let sm = app.source_manager.lock().await;
        match sm.plugins() {
            Ok(p) => p,
            Err(e) => {
                log::warn!("refresh_source_plugins: source_manager.plugins() failed: {e:#}");
                Vec::new()
            }
        }
    };
    *app.source_plugins.write().await = plugins;
}

pub async fn refresh_sources(app: &Arc<AppState>) -> Result<usize> {
    use std::sync::atomic::Ordering;
    app.scan_in_progress.store(true, Ordering::SeqCst);
    // Sync start is observable to UIs via `StatusSync.scan_in_progress`.
    app.events
        .publish(crate::events::GlobalEvent::StatusChanged);

    let result = refresh_sources_inner(app).await;

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

pub async fn notify_wallpaper_db_changed(app: &Arc<AppState>, count: usize) {
    app.queue.lock().await.reset_shuffle_round();

    let probe = app.probe.clone();
    let db = app.db.clone();
    app.tasks.spawn_async_unique(
        crate::tasks::TaskKind::Generic,
        "probe/refresh",
        "probe/post-db-change",
        async move {
            crate::probe::task::run_pending(&db, probe)
                .await
                .map(|_| ())
                .map_err(anyhow::Error::from)
        },
    );

    app.events
        .publish(crate::events::GlobalEvent::SyncFinished { count });
}

async fn refresh_sources_inner(app: &Arc<AppState>) -> Result<usize> {
    let libs_by_plugin = libraries_by_plugin_name(&app.db).await?;

    let source_mgr = app.source_manager.clone();
    // Scan each physical directory once; symlinked Steam aliases otherwise
    // emit duplicate workshop entries and duplicate UI rows.
    let libs_for_scan: HashMap<String, Vec<String>> = libs_by_plugin
        .iter()
        .map(|(name, paths)| (name.clone(), dedup_paths_by_canonical(paths)))
        .collect();
    // Hold the Lua VM lock only during the scan; wallpaper reads hit the DB
    // and do not wait behind this section.
    let handle = tokio::runtime::Handle::current();
    let snapshot: Vec<WallpaperEntry> = tokio::task::spawn_blocking(move || {
        let mut sm = source_mgr.blocking_lock();
        handle.block_on(sm.scan_all(&libs_for_scan))?;
        Ok::<_, anyhow::Error>(sm.list().to_vec())
    })
    .await
    .map_err(|e| Error::Internal(anyhow!("source scan join: {e}")))??;

    let plugins = {
        let sm = app.source_manager.lock().await;
        match sm.plugins() {
            Ok(p) => p,
            Err(e) => {
                log::warn!("refresh_sources: source_manager.plugins() failed: {e:#}");
                Vec::new()
            }
        }
    };

    // Sync to the DB first so every entry gets its canonical item id before
    // readers observe the refreshed source-plugin list.
    for info in &plugins {
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
    app.tasks.spawn_async_unique(
        crate::tasks::TaskKind::Generic,
        "probe/refresh",
        "probe/post-refresh",
        async move {
            crate::probe::task::run_pending(&db, probe)
                .await
                .map(|_| ())
                .map_err(anyhow::Error::from)
        },
    );

    Ok(count)
}
