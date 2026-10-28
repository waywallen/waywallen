use super::runtime::LoadedPluginInfo;
use super::*;

#[derive(Default)]
pub struct SourceCatalog {
    entries: Vec<WallpaperEntry>,
    by_type: HashMap<WallpaperType, Vec<usize>>,
}

impl SourceCatalog {
    fn replace(&mut self, entries: Vec<WallpaperEntry>) {
        self.by_type.clear();
        for (idx, entry) in entries.iter().enumerate() {
            self.by_type
                .entry(entry.wp_type.clone())
                .or_default()
                .push(idx);
        }
        self.entries = entries;
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.by_type.clear();
    }
}

pub(super) struct PluginHandle {
    pub(super) runtime: Arc<tokio::sync::Mutex<LuaPluginRuntime>>,
    info: StdRwLock<LoadedPluginInfo>,
    generation_state: Arc<AtomicU8>,
}

/// Registry and source catalog for Lua plugins.
///
/// Each plugin owns its Lua VM and async mutex. Registry/catalog locks are only
/// held while cloning handles or replacing snapshots, so callbacks belonging to
/// different plugins can make progress concurrently.
pub struct LuaPluginRegistry {
    plugins: StdRwLock<HashMap<String, Arc<PluginHandle>>>,
    catalog: StdRwLock<SourceCatalog>,
    probe: Arc<dyn MediaProbe>,
    db: StdRwLock<Option<DatabaseConnection>>,
    settings: StdRwLock<Option<Arc<crate::settings::SettingsStore>>>,
    state_store: crate::plugin::state_store::PluginStateStore,
}

pub type SourceManager = LuaPluginRegistry;

impl LuaPluginRegistry {
    pub fn new() -> Result<Self> {
        Self::with_probe(Arc::new(AvFormatProbe::new()))
    }

    pub fn with_probe(probe: Arc<dyn MediaProbe>) -> Result<Self> {
        Self::with_probe_and_state_store(
            probe,
            crate::plugin::state_store::PluginStateStore::standard(),
        )
    }

    pub(super) fn with_probe_and_state_store(
        probe: Arc<dyn MediaProbe>,
        state_store: crate::plugin::state_store::PluginStateStore,
    ) -> Result<Self> {
        Ok(Self {
            plugins: StdRwLock::new(HashMap::new()),
            catalog: StdRwLock::new(SourceCatalog::default()),
            probe,
            db: StdRwLock::new(None),
            settings: StdRwLock::new(None),
            state_store,
        })
    }

    pub fn attach_db(&self, db: DatabaseConnection) {
        *self.db.write().expect("plugin DB lock poisoned") = Some(db);
    }

    pub fn attach_settings(&self, settings: Arc<crate::settings::SettingsStore>) {
        *self
            .settings
            .write()
            .expect("plugin settings lock poisoned") = Some(settings);
    }

    pub fn clear_plugins(&self) {
        let mut plugins = self.plugins.write().expect("plugin registry lock poisoned");
        for handle in plugins.values() {
            handle
                .generation_state
                .store(RUNTIME_INACTIVE, Ordering::Release);
        }
        plugins.clear();
        drop(plugins);
        self.catalog
            .write()
            .expect("source catalog lock poisoned")
            .clear();
    }

    pub async fn suspend_plugins(&self) {
        let handles = self.handles();
        for (_, handle) in &handles {
            handle
                .generation_state
                .store(RUNTIME_DRAINING, Ordering::Release);
        }
        for (_, handle) in handles {
            let _runtime = handle.runtime.lock().await;
        }
    }

    pub fn resume_plugins(&self) {
        for (_, handle) in self.handles() {
            let _ = handle.generation_state.compare_exchange(
                RUNTIME_DRAINING,
                RUNTIME_ACTIVE,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }

    pub fn retain_plugins_from(&self, current: &Self, plugin_ids: &HashSet<String>) -> Result<()> {
        let current = current
            .plugins
            .read()
            .expect("plugin registry lock poisoned");
        let mut replacement = self.plugins.write().expect("plugin registry lock poisoned");
        for (name, handle) in current.iter() {
            let plugin_id = &handle
                .info
                .read()
                .expect("plugin info lock poisoned")
                .plugin_id;
            if !plugin_ids.contains(plugin_id) {
                continue;
            }
            if replacement.contains_key(name) {
                return Err(Error::Internal(anyhow!(
                    "duplicate retained Lua plugin name '{name}'"
                )));
            }
            replacement.insert(name.clone(), handle.clone());
        }
        Ok(())
    }

    pub fn replace_plugins(&self, replacement: Self) -> Result<()> {
        let plugins = replacement
            .plugins
            .into_inner()
            .map_err(|_| Error::Internal(anyhow!("replacement plugin registry lock poisoned")))?;
        let catalog = replacement
            .catalog
            .into_inner()
            .map_err(|_| Error::Internal(anyhow!("replacement source catalog lock poisoned")))?;
        let mut current = self.plugins.write().expect("plugin registry lock poisoned");
        for handle in current.values() {
            handle
                .generation_state
                .store(RUNTIME_INACTIVE, Ordering::Release);
        }
        *current = plugins;
        for handle in current.values() {
            handle
                .generation_state
                .store(RUNTIME_ACTIVE, Ordering::Release);
        }
        drop(current);
        *self.catalog.write().expect("source catalog lock poisoned") = catalog;
        Ok(())
    }

    pub fn load_plugin(
        &self,
        path: &Path,
        plugin_id: &str,
        plugin_version: &str,
        entry_version: u32,
    ) -> Result<String> {
        let mut runtime = LuaPluginRuntime::with_probe(self.probe.clone())?;
        runtime.set_state_store(self.state_store.clone());
        if let Some(db) = self.db.read().expect("plugin DB lock poisoned").clone() {
            runtime.attach_db(db);
        }
        if let Some(settings) = self
            .settings
            .read()
            .expect("plugin settings lock poisoned")
            .clone()
        {
            runtime.attach_settings(settings);
        }
        let name = runtime.load_plugin(path, plugin_id, plugin_version, entry_version)?;
        let info = runtime
            .loaded_plugin_info(&name)
            .ok_or_else(|| Error::SourcePluginNotFound(name.clone()))?;
        let generation_state = runtime.generation_state();
        let handle = Arc::new(PluginHandle {
            runtime: Arc::new(tokio::sync::Mutex::new(runtime)),
            info: StdRwLock::new(info),
            generation_state,
        });
        let mut plugins = self.plugins.write().expect("plugin registry lock poisoned");
        if plugins.contains_key(&name) {
            return Err(Error::Internal(anyhow!(
                "duplicate Lua plugin name '{name}'"
            )));
        }
        plugins.insert(name.clone(), handle);
        Ok(name)
    }

    pub(super) fn handle(&self, plugin_name: &str) -> Result<Arc<PluginHandle>> {
        self.plugins
            .read()
            .expect("plugin registry lock poisoned")
            .get(plugin_name)
            .cloned()
            .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))
    }

    fn handles(&self) -> Vec<(String, Arc<PluginHandle>)> {
        self.plugins
            .read()
            .expect("plugin registry lock poisoned")
            .iter()
            .map(|(name, handle)| (name.clone(), handle.clone()))
            .collect()
    }

    pub async fn scan_all(&self, libs_by_plugin: &HashMap<String, Vec<String>>) -> Result<()> {
        let scans = self.handles().into_iter().filter_map(|(name, handle)| {
            let has_source = handle
                .info
                .read()
                .expect("plugin info lock poisoned")
                .capabilities
                .source
                .is_some();
            has_source.then(|| {
                let libraries = libs_by_plugin.get(&name).cloned().unwrap_or_default();
                async move {
                    let mut runtime = handle.runtime.lock().await;
                    let mut only_this = HashMap::new();
                    only_this.insert(name.clone(), libraries);
                    runtime.scan_all(&only_this).await?;
                    Ok::<_, Error>(runtime.list().to_vec())
                }
            })
        });
        let results = futures_util::future::join_all(scans).await;
        let mut entries = Vec::new();
        // The catalog is still replaced with whatever the healthy plugins
        // returned, so one broken source does not hide the rest. The failures
        // are reported afterwards: a scan that failed must not be presented as
        // a scan that simply found nothing.
        let mut failures: Vec<String> = Vec::new();
        for result in results {
            match result {
                Ok(mut plugin_entries) => entries.append(&mut plugin_entries),
                Err(e) => {
                    log::warn!("scan Lua plugin failed: {e}");
                    failures.push(format!("{e:#}"));
                }
            }
        }
        entries.sort_by(|a, b| {
            a.plugin_name
                .cmp(&b.plugin_name)
                .then_with(|| a.resource.cmp(&b.resource))
        });
        self.catalog
            .write()
            .expect("source catalog lock poisoned")
            .replace(entries);
        if !failures.is_empty() {
            return Err(Error::Internal(anyhow!(
                "source scan failed: {}",
                failures.join("; ")
            )));
        }
        Ok(())
    }

    pub fn list(&self) -> Vec<WallpaperEntry> {
        self.catalog
            .read()
            .expect("source catalog lock poisoned")
            .entries
            .clone()
    }

    pub fn list_by_type(&self, wp_type: &str) -> Vec<WallpaperEntry> {
        let catalog = self.catalog.read().expect("source catalog lock poisoned");
        catalog
            .by_type
            .get(wp_type)
            .map(|indices| {
                indices
                    .iter()
                    .map(|&idx| catalog.entries[idx].clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn get(&self, id: &str) -> Option<WallpaperEntry> {
        self.catalog
            .read()
            .expect("source catalog lock poisoned")
            .entries
            .iter()
            .find(|entry| entry.item_id.to_string() == id)
            .cloned()
    }

    pub async fn call_apply(
        &self,
        plugin_name: &str,
        entry: &WallpaperEntry,
    ) -> Result<WallpaperApply> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .call_apply(plugin_name, entry)
            .await
    }

    pub fn action_kind(&self, plugin_name: &str, action_id: &str) -> Option<SourceActionKind> {
        self.handle(plugin_name).ok().and_then(|handle| {
            handle
                .info
                .read()
                .expect("plugin info lock poisoned")
                .actions
                .iter()
                .find(|action| action.id == action_id)
                .map(|action| action.kind)
        })
    }

    pub async fn check_lifecycle(&self, plugin_name: &str) -> Result<Option<PluginLifecycleCheck>> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .check_lifecycle(plugin_name)
            .await
    }

    pub async fn invoke_action(
        &self,
        plugin_name: &str,
        action_id: &str,
        values: &HashMap<String, String>,
    ) -> Result<()> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .invoke_action(plugin_name, action_id, values)
            .await
    }

    pub async fn begin_qr_login(&self, plugin_name: &str, action_id: &str) -> Result<QrLoginBegin> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .begin_qr_login(plugin_name, action_id)
            .await
    }

    pub async fn poll_qr_login(&self, plugin_name: &str, operation_id: u64) -> Result<QrLoginPoll> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .poll_qr_login(plugin_name, operation_id)
            .await
    }

    pub async fn cancel_qr_login(&self, plugin_name: &str, operation_id: u64) -> Result<()> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .cancel_qr_login(plugin_name, operation_id)
            .await
    }

    pub async fn subscription_status(
        &self,
        plugin_name: &str,
        ids: &[String],
    ) -> Result<SubscriptionStatusResult> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .subscription_status(plugin_name, ids)
            .await
    }

    pub async fn set_subscription(
        &self,
        plugin_name: &str,
        id: &str,
        subscribed: bool,
    ) -> Result<SubscriptionSetResult> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .set_subscription(plugin_name, id, subscribed)
            .await
    }

    pub async fn call_properties(
        &self,
        plugin_name: &str,
        entry: &WallpaperEntry,
    ) -> Result<Option<String>> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .call_properties(plugin_name, entry)
            .await
    }

    pub async fn auto_detect_all(&self) -> Result<HashMap<String, Vec<String>>> {
        let calls = self
            .handles()
            .into_iter()
            .map(|(_, handle)| async move { handle.runtime.lock().await.auto_detect_all().await });
        let mut out = HashMap::new();
        for result in futures_util::future::join_all(calls).await {
            out.extend(result?);
        }
        Ok(out)
    }

    pub fn discover_sources(&self) -> Result<Vec<DiscoverSourceInfo>> {
        let mut out = Vec::new();
        for (_, handle) in self.handles() {
            let info = handle.info.read().expect("plugin info lock poisoned");
            let Some(disc) = &info.capabilities.discover else {
                continue;
            };
            out.push(DiscoverSourceInfo {
                plugin_id: info.name.clone(),
                name: info.name.clone(),
                display_name: info.display_name.clone(),
                supports_search: disc.supports_search,
                remote_capability: disc.remote,
                remote_hint: disc.remote_hint.clone(),
                sorts: disc.sorts.clone(),
                filters: disc.filters.clone(),
                owner_plugin_id: info.plugin_id.clone(),
                settings: info.settings.clone(),
                actions: info.actions.clone(),
                status: info.status.clone(),
                avatar_url: String::new(),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Validate and canonicalize a partial settings update for one discover
    /// source. The Lua declaration is authoritative; callers cannot add
    /// undeclared keys to the source's config table.
    pub fn validate_remote_settings_patch(
        &self,
        source_id: &str,
        values: HashMap<String, String>,
    ) -> Result<HashMap<String, String>> {
        let handle = self.handle(source_id)?;
        let info = handle.info.read().expect("plugin info lock poisoned");
        if info.capabilities.discover.is_none() {
            return Err(Error::DiscoverUnsupported(source_id.to_string()));
        }

        let schemas: HashMap<_, _> = info
            .settings
            .iter()
            .map(|setting| (setting.key.as_str(), setting))
            .collect();
        let mut validated = HashMap::with_capacity(values.len());
        for (key, raw) in values {
            let setting = schemas.get(key.as_str()).ok_or_else(|| {
                Error::SettingsValidationFailed(format!(
                    "{source_id}.{key} is not declared by the remote source"
                ))
            })?;
            let value = validate_source_setting(setting, &raw)
                .map_err(|error| Error::SettingsValidationFailed(format!("{source_id}.{error}")))?;
            validated.insert(key, value);
        }
        Ok(validated)
    }

    pub async fn discover_sources_with_status(&self) -> Result<Vec<DiscoverSourceInfo>> {
        let calls = self.handles().into_iter().filter_map(|(name, handle)| {
            let has_discover = handle
                .info
                .read()
                .expect("plugin info lock poisoned")
                .capabilities
                .discover
                .is_some();
            has_discover.then(|| async move {
                let result = handle.runtime.lock().await.call_action_status(&name).await;
                (name, result)
            })
        });
        let dynamic: HashMap<_, _> = futures_util::future::join_all(calls)
            .await
            .into_iter()
            .filter_map(|(name, result)| match result {
                Ok(value) => Some((name, value)),
                Err(error) => {
                    log::warn!("plugin action status failed: {error}");
                    None
                }
            })
            .collect();
        let mut sources = self.discover_sources()?;
        for source in &mut sources {
            if let Some((actions, status, avatar_url)) = dynamic.get(&source.plugin_id) {
                source.actions = actions.clone();
                source.status = status.clone();
                source.avatar_url = avatar_url.clone();
            }
        }
        Ok(sources)
    }

    pub async fn call_discover(
        &self,
        plugin_name: &str,
        query: &str,
        sort: &str,
        page: u32,
        tags: &[String],
    ) -> Result<DiscoverSearchResult> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .call_discover(plugin_name, query, sort, page, tags)
            .await
    }

    pub async fn call_tags(&self, plugin_name: &str) -> Result<Vec<String>> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .call_tags(plugin_name)
            .await
    }

    pub async fn refresh_dynamic_tags(&self) {
        let handles: Vec<_> = self
            .handles()
            .into_iter()
            .filter(|(_, handle)| {
                handle
                    .info
                    .read()
                    .expect("plugin info lock poisoned")
                    .capabilities
                    .discover
                    .as_ref()
                    .is_some_and(|discover| discover.dynamic_tags)
            })
            .collect();
        for (name, handle) in handles {
            match handle.runtime.lock().await.call_tags(&name).await {
                Ok(tags) if !tags.is_empty() => {
                    if let Some(discover) = handle
                        .info
                        .write()
                        .expect("plugin info lock poisoned")
                        .capabilities
                        .discover
                        .as_mut()
                    {
                        discover.filters = LuaPluginRuntime::legacy_discover_filter(tags);
                    }
                }
                Ok(_) => {}
                Err(e) => log::warn!("refresh discover tags for {name}: {e:#}"),
            }
        }
    }

    pub async fn call_details(&self, plugin_name: &str, id: &str) -> Result<DiscoverDetails> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .call_details(plugin_name, id)
            .await
    }

    pub async fn call_download(&self, plugin_name: &str, id: &str) -> Result<DiscoverDownload> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .call_download(plugin_name, id)
            .await
    }

    pub async fn call_resolve(
        &self,
        plugin_name: &str,
        id: &str,
        dir: &str,
    ) -> Result<DiscoverResolve> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .call_resolve(plugin_name, id, dir)
            .await
    }

    pub fn plugins(&self) -> Result<Vec<SourcePluginInfo>> {
        let mut out = Vec::new();
        for (_, handle) in self.handles() {
            let info = handle.info.read().expect("plugin info lock poisoned");
            let Some(source) = &info.capabilities.source else {
                continue;
            };
            out.push(SourcePluginInfo {
                name: info.name.clone(),
                plugin_id: info.plugin_id.clone(),
                types: source.types.clone(),
                version: info.version.clone(),
                library_label: source.library_label.clone(),
                library_hint: source.library_hint.clone(),
                settings: info.settings.clone(),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub fn supports_item_remove(&self, plugin_name: &str) -> bool {
        self.handle(plugin_name).ok().is_some_and(|handle| {
            handle
                .info
                .read()
                .expect("plugin info lock poisoned")
                .capabilities
                .source_item_remove
        })
    }

    pub fn supports_entry_remove(&self, entry: &WallpaperEntry) -> bool {
        entry.can_remove != Some(false) && self.supports_item_remove(&entry.plugin_name)
    }

    pub fn supports_item_unsubscribe(&self, entry: &WallpaperEntry) -> bool {
        if entry.external_id.as_deref().is_none_or(str::is_empty) {
            return false;
        }
        self.handle(&entry.plugin_name).ok().is_some_and(|handle| {
            handle
                .info
                .read()
                .expect("plugin info lock poisoned")
                .capabilities
                .discover
                .as_ref()
                .and_then(|discover| discover.remote)
                == Some(RemoteCapability::Subscription)
        })
    }

    pub async fn remove_item(
        &self,
        plugin_name: &str,
        entry: &WallpaperEntry,
        libraries: &[String],
    ) -> Result<()> {
        self.handle(plugin_name)?
            .runtime
            .lock()
            .await
            .remove_item(plugin_name, entry, libraries)
            .await
    }

    pub fn plugin_version(&self, plugin_name: &str) -> Option<String> {
        self.handle(plugin_name).ok().map(|handle| {
            handle
                .info
                .read()
                .expect("plugin info lock poisoned")
                .version
                .clone()
        })
    }

    #[cfg(test)]
    pub(super) fn test_runtime(
        &self,
        plugin_name: &str,
    ) -> Arc<tokio::sync::Mutex<LuaPluginRuntime>> {
        self.handle(plugin_name).unwrap().runtime.clone()
    }

    #[cfg(test)]
    pub(crate) async fn set_test_callback_timeout(&self, plugin_name: &str, timeout: Duration) {
        self.test_runtime(plugin_name).lock().await.callback_timeout = timeout;
    }
}

// ---------------------------------------------------------------------------
// Helpers
