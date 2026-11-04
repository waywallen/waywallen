use std::sync::Arc;

use anyhow::anyhow;

use crate::error::{Error, Result};
use crate::events::GlobalEvent;
use crate::model::repo;
use crate::plugin::renderer_registry::{PluginPackageMeta, PluginScan, RendererRegistry};
use crate::DaemonContext;

fn registry_from_scan(scan: &PluginScan) -> RendererRegistry {
    let mut registry = RendererRegistry::new();
    for definition in &scan.renderers {
        registry.register(definition.clone());
    }
    registry
}

struct SourcePluginSuspendGuard {
    manager: Arc<crate::plugin::source::SourceManager>,
    committed: bool,
}

impl SourcePluginSuspendGuard {
    fn new(manager: Arc<crate::plugin::source::SourceManager>) -> Self {
        Self {
            manager,
            committed: false,
        }
    }

    async fn suspend(&self) {
        self.manager.suspend_plugins().await;
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for SourcePluginSuspendGuard {
    fn drop(&mut self) {
        if !self.committed {
            self.manager.resume_plugins();
        }
    }
}

async fn reload_source_entries(
    app: &Arc<DaemonContext>,
    entries: Vec<crate::plugin::renderer_registry::EntryRef>,
    installed_plugin_id: &str,
) -> Result<()> {
    app.qr_login.cancel_all_and_wait().await?;
    let mut suspended = SourcePluginSuspendGuard::new(app.source_manager.clone());
    suspended.suspend().await;
    let installed_plugin_id = installed_plugin_id.to_string();
    let probe = app.probe.clone();
    let db = app.db.clone();
    let settings = app.settings.clone();
    let load_result = tokio::task::spawn_blocking(move || {
        let source_manager = crate::plugin::source::SourceManager::with_probe(probe)?;
        source_manager.attach_db(db);
        source_manager.attach_settings(settings);

        let mut failures = Vec::new();
        for entry in &entries {
            if let Err(error) = source_manager.load_entry(entry) {
                let message = format!("load entry {}: {error:#}", entry.entry.display());
                log::warn!("{message}");
                failures.push((entry.plugin_id.clone(), message));
            }
        }
        Ok((source_manager, failures))
    })
    .await;

    let (replacement, failures) = match load_result {
        Ok(Ok(replacement)) => replacement,
        Ok(Err(error)) => return Err(error),
        Err(error) => return Err(Error::Internal(anyhow!("source reload join: {error}"))),
    };
    if failures
        .iter()
        .any(|(plugin_id, _)| plugin_id == &installed_plugin_id)
    {
        let messages = failures
            .into_iter()
            .map(|(_, message)| message)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(Error::PluginInstallFailed(format!(
            "installed source plugin reload failed: {messages}"
        )));
    }
    let failed_plugin_ids = failures
        .into_iter()
        .map(|(plugin_id, _)| plugin_id)
        .collect::<std::collections::HashSet<_>>();
    replacement.retain_plugins_from(&app.source_manager, &failed_plugin_ids)?;
    app.source_manager.replace_plugins(replacement)?;
    suspended.commit();

    let infos = app.source_manager.plugins()?;
    for info in &infos {
        repo::upsert_plugin(&app.db, &info.name, &info.version)
            .await
            .map_err(|error| Error::Internal(anyhow!("upsert plugin {}: {error:#}", info.name)))?;
    }
    *app.source_plugins.write().await = infos;
    Ok(())
}

pub(super) async fn apply_scan(
    app: &Arc<DaemonContext>,
    scan: PluginScan,
    installed_plugin_id: &str,
) -> Result<Vec<PluginPackageMeta>> {
    let registry = registry_from_scan(&scan);
    let packages = scan.packages();
    let translation_documents = scan.translation_documents().map_err(|error| {
        Error::PluginInstallFailed(format!("load plugin translations: {error}"))
    })?;

    reload_source_entries(app, scan.entries, installed_plugin_id).await?;
    app.renderer_manager.replace_registry(registry.clone());
    *app.plugins.write().await = packages.clone();
    *app.inactive_system.write().await = scan.inactive_system.clone();
    *app.inactive_user.write().await = scan.inactive_user.clone();
    {
        let mut translations = app.plugin_translations.write().await;
        translations.generation = translations.generation.saturating_add(1);
        translations.documents = translation_documents;
    }
    app.plugin_updates.write().await.remove(installed_plugin_id);

    if app.settings.reconcile(&registry) {
        app.events
            .publish(crate::events::GlobalEvent::SettingsChanged);
        app.settings.flush_now().await;
    }

    app.events.publish(GlobalEvent::PluginChanged);
    Ok(packages)
}
