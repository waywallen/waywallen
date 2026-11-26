use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;

use crate::error::{Error, Result};
use crate::events::GlobalEvent;
use crate::model::repo;
use crate::plugin::renderer_registry::PluginPackageMeta;
use crate::wallframe::renderer_manager;
use crate::DaemonContext;

use super::reload;

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
        .find(|package| package.id == plugin_id)
        .map(|package| ActivePluginIdentity {
            version: package.version.clone(),
            system: package.system,
        })
}

async fn restart_affected_renderers(
    app: &Arc<DaemonContext>,
    renderer_ids: Vec<renderer_manager::RendererId>,
) -> Result<()> {
    app.router
        .restart_renderers_orderly(
            &renderer_ids,
            Duration::from_secs(1),
            crate::application::apply_first_frame_timeout(),
        )
        .await
}

fn spawn_affected_renderer_restart(
    app: &Arc<DaemonContext>,
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
        if let Err(error) = restart_affected_renderers(&app, renderer_ids).await {
            let error = format!("{error:#}");
            log::warn!("plugin restart failed for {plugin_id}: {error}");
            app.events
                .publish(GlobalEvent::PluginRestartFailed { plugin_id, error });
        }
        Ok(())
    });
}

fn spawn_source_refresh(app: &Arc<DaemonContext>, plugin_id: &str) {
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
                .map(|libraries| libraries.is_empty())
                .unwrap_or(false);
            if !skip_refresh {
                crate::application::refresh_sources(&app)
                    .await
                    .map(|_| ())?;
            }
            Ok(())
        },
    );
}

pub async fn install_plugin_archive(
    app: &Arc<DaemonContext>,
    zip_path: String,
) -> Result<PluginInstallResult> {
    let _guard = app.plugin_mutation.lock().await;
    let prepared = tokio::task::spawn_blocking(move || {
        crate::plugin::installer::prepare_install_zip(&zip_path)
    })
    .await
    .map_err(|error| Error::Internal(anyhow!("install join: {error}")))??;
    let plugin_id = prepared.plugin_id().to_string();
    app.browser_login.suspend_helper_plugin(&plugin_id).await?;
    let install_result = tokio::task::spawn_blocking(move || prepared.install()).await;
    let old_active = active_plugin_identity(&app.plugins.read().await, &plugin_id);
    let old_renderer_ids = app
        .renderer_manager
        .live_renderer_ids_by_plugin_id(&plugin_id)
        .await;
    let plugin_roots = app.plugin_roots.clone();
    let plugin_scan = tokio::task::spawn_blocking(move || {
        crate::plugin::renderer_registry::scan_plugin_roots(plugin_roots.as_slice())
    })
    .await
    .map_err(|error| Error::Internal(anyhow!("plugin scan join: {error}")))?;
    app.browser_login
        .replace_helpers(plugin_scan.web_helpers.clone());
    install_result.map_err(|error| Error::Internal(anyhow!("install join: {error}")))??;

    let new_packages = reload::apply_scan(app, plugin_scan, &plugin_id).await?;
    let new_active = active_plugin_identity(&new_packages, &plugin_id);
    let active_user_install = new_active.as_ref().is_some_and(|plugin| !plugin.system);
    let should_restart =
        !old_renderer_ids.is_empty() && (active_user_install || old_active != new_active);
    if should_restart {
        spawn_affected_renderer_restart(app, plugin_id.clone(), old_renderer_ids);
    }
    spawn_source_refresh(app, &plugin_id);
    Ok(PluginInstallResult {
        plugin_id,
        needs_restart: false,
    })
}

pub async fn delete_plugin(app: &Arc<DaemonContext>, plugin_id: String) -> Result<String> {
    let _guard = app.plugin_mutation.lock().await;
    app.browser_login.suspend_helper_plugin(&plugin_id).await?;
    let deletion = tokio::task::spawn_blocking(move || {
        crate::plugin::installer::delete_user_plugin(&plugin_id)
    })
    .await;
    let roots = app.plugin_roots.clone();
    let scan = tokio::task::spawn_blocking(move || {
        crate::plugin::renderer_registry::scan_plugin_roots(&roots)
    })
    .await
    .map_err(|error| Error::Internal(anyhow!("plugin scan join: {error}")))?;
    app.browser_login.replace_helpers(scan.web_helpers);
    let plugin_id =
        deletion.map_err(|error| Error::Internal(anyhow!("plugin delete join: {error}")))??;
    app.events.publish(GlobalEvent::PluginChanged);
    Ok(plugin_id)
}
