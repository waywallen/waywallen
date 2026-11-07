use super::*;
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct CanvasDraft {
    pub name: String,
    pub members: HashMap<String, CanvasMemberPrefs>,
    pub layout: Option<CanvasLayoutPrefs>,
}

#[derive(Debug, Clone, Copy)]
pub enum CanvasMemberSizeChange {
    Width(u32),
    Height(u32),
    Reset,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CanvasMemberConfig {
    pub size: Option<CanvasMemberSizeChange>,
    pub aspect_locked: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct CanvasMutationReceipt {
    pub canvas_id: String,
    pub canvas: CanvasPrefs,
    pub revision: u64,
    pub affected_display_keys: Vec<String>,
    pub previous: Option<CanvasPrefs>,
}

#[derive(Debug, Clone)]
pub struct CanvasDeleteReceipt {
    pub canvas_id: String,
    pub revision: u64,
    pub affected_display_keys: Vec<String>,
    pub canvas: CanvasPrefs,
}

fn normalize_canvas_draft(mut draft: CanvasDraft) -> crate::error::Result<CanvasPrefs> {
    draft.name = draft.name.trim().to_string();
    if draft.name.is_empty() {
        return Err(crate::error::Error::CanvasInvalid(
            "canvas name is empty".to_string(),
        ));
    }
    if draft.members.len() > 128 {
        return Err(crate::error::Error::CanvasInvalid(
            "canvas has more than 128 members".to_string(),
        ));
    }
    let mut keys = draft.members.keys().cloned().collect::<Vec<_>>();
    keys.sort();
    if keys.iter().any(|key| key.trim().is_empty()) {
        return Err(crate::error::Error::CanvasInvalid(
            "canvas member has an empty display key".to_string(),
        ));
    }
    let mut rects = keys
        .iter()
        .map(|key| draft.members[key].rect)
        .collect::<Vec<_>>();
    crate::wallframe::display::placement::canonicalize(&mut rects)
        .map_err(|message| crate::error::Error::CanvasInvalid(message.to_string()))?;
    for (key, rect) in keys.into_iter().zip(rects) {
        draft
            .members
            .get_mut(&key)
            .expect("canvas member missing")
            .rect = rect;
    }
    Ok(CanvasPrefs {
        name: draft.name,
        members: draft.members,
        last_wallpaper: None,
        layout: draft.layout,
    })
}

fn aspect_size_from_width(width: u32, aspect: CanvasSize) -> crate::error::Result<CanvasSize> {
    aspect
        .validate()
        .map_err(|message| crate::error::Error::CanvasInvalid(message.to_string()))?;
    let height = u64::from(width)
        .checked_mul(u64::from(aspect.height))
        .and_then(|value| value.checked_add(u64::from(aspect.width) / 2))
        .map(|value| value / u64::from(aspect.width))
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| {
            crate::error::Error::CanvasInvalid(
                "canvas member aspect ratio calculation overflowed".to_string(),
            )
        })?;
    CanvasSize { width, height }
        .validate()
        .map_err(|message| crate::error::Error::CanvasInvalid(message.to_string()))
}

fn aspect_size_from_height(height: u32, aspect: CanvasSize) -> crate::error::Result<CanvasSize> {
    aspect
        .validate()
        .map_err(|message| crate::error::Error::CanvasInvalid(message.to_string()))?;
    let width = u64::from(height)
        .checked_mul(u64::from(aspect.width))
        .and_then(|value| value.checked_add(u64::from(aspect.height) / 2))
        .map(|value| value / u64::from(aspect.height))
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| {
            crate::error::Error::CanvasInvalid(
                "canvas member aspect ratio calculation overflowed".to_string(),
            )
        })?;
    CanvasSize { width, height }
        .validate()
        .map_err(|message| crate::error::Error::CanvasInvalid(message.to_string()))
}

pub(crate) fn reconcile_canvas_member_runtime_size(
    current: CanvasSize,
    actual: CanvasSize,
    aspect_locked: bool,
) -> crate::error::Result<CanvasSize> {
    current
        .validate()
        .map_err(|message| crate::error::Error::CanvasInvalid(message.to_string()))?;
    actual
        .validate()
        .map_err(|message| crate::error::Error::CanvasInvalid(message.to_string()))?;
    if aspect_locked {
        aspect_size_from_width(current.width.max(actual.width), actual)
    } else {
        Ok(current.component_max(actual))
    }
}

fn reconcile_canvas_draft_sizes(
    members: &mut HashMap<String, CanvasMemberPrefs>,
    live_sizes: &HashMap<String, CanvasSize>,
) -> crate::error::Result<()> {
    for (key, member) in members {
        let Some(actual) = live_sizes.get(key).copied() else {
            continue;
        };
        let size =
            reconcile_canvas_member_runtime_size(member.rect.size(), actual, member.aspect_locked)?;
        member.rect.width = size.width;
        member.rect.height = size.height;
    }
    Ok(())
}

fn validate_canvas_scale_to(
    members: &HashMap<String, CanvasMemberPrefs>,
    live_minimums: &HashMap<String, CanvasSize>,
    previous: Option<&CanvasPrefs>,
) -> crate::error::Result<()> {
    for (key, member) in members {
        let minimum = live_minimums.get(key).copied().or_else(|| {
            previous
                .and_then(|canvas| canvas.members.get(key))
                .map(|member| member.rect.size())
        });
        let Some(minimum) = minimum else {
            continue;
        };
        if !member.rect.size().contains(minimum) {
            return Err(crate::error::Error::CanvasInvalid(format!(
                "canvas member '{key}' scale to {}x{} is smaller than its minimum {}x{}",
                member.rect.width, member.rect.height, minimum.width, minimum.height
            )));
        }
    }
    Ok(())
}

pub struct SettingsStore {
    inner: Arc<StdRwLock<Settings>>,
    notify: Arc<Notify>,
    path: PathBuf,
    /// Serializes concurrent `flush()` calls.
    /// Covers both the debounced writer and shutdown flush.
    flush_lock: tokio::sync::Mutex<()>,
    /// Set when the in-memory state diverges from disk.
    /// Cleared by a successful `flush()`.
    dirty: AtomicBool,
    canvas_revision: AtomicU64,
    writer_task: StdMutex<Option<tokio::task::JoinHandle<()>>>,
}

impl SettingsStore {
    #[cfg(test)]
    pub(super) fn from_test_settings(settings: Settings) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(StdRwLock::new(settings)),
            notify: Arc::new(Notify::new()),
            path: PathBuf::from("/dev/null"),
            flush_lock: tokio::sync::Mutex::new(()),
            dirty: AtomicBool::new(false),
            canvas_revision: AtomicU64::new(1),
            writer_task: StdMutex::new(None),
        })
    }

    /// Load from `path`, or fall back to defaults and seed the file.
    /// Seeding makes the config visible to users immediately.
    pub async fn load_or_default(path: PathBuf) -> Arc<Self> {
        let mut seed_on_disk = false;
        let mut initial = match tokio::fs::read_to_string(&path).await {
            Ok(s) => match toml::from_str::<Settings>(&s) {
                Ok(parsed) => {
                    log::info!("settings loaded from {}", path.display());
                    parsed
                }
                Err(e) => {
                    log::warn!(
                        "settings parse {}: {e}; continuing with defaults",
                        path.display()
                    );
                    Settings::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                log::info!(
                    "settings file {} not found, seeding defaults",
                    path.display()
                );
                seed_on_disk = true;
                let mut settings = Settings::default();
                settings.global.auto_replay = Some(AutoReplayPolicy::default());
                settings
            }
            Err(e) => {
                log::warn!(
                    "settings file {} not readable ({e}); using defaults",
                    path.display()
                );
                Settings::default()
            }
        };

        seed_on_disk |= initial.migrate_auto_replay();
        let store = Arc::new(Self {
            inner: Arc::new(StdRwLock::new(initial)),
            notify: Arc::new(Notify::new()),
            path,
            flush_lock: tokio::sync::Mutex::new(()),
            // Persist defaults and migrated settings before starting the writer.
            dirty: AtomicBool::new(seed_on_disk),
            canvas_revision: AtomicU64::new(1),
            writer_task: StdMutex::new(None),
        });

        if seed_on_disk {
            store.flush().await;
        }

        // Debounced writer task.
        let writer = Arc::clone(&store);
        let task = tokio::spawn(async move {
            writer.writer_loop().await;
        });
        *store.writer_task.lock().expect("settings writer poisoned") = Some(task);

        store
    }

    pub async fn stop_writer(&self) {
        let task = self
            .writer_task
            .lock()
            .expect("settings writer poisoned")
            .take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
    }

    /// Snapshot the current settings by cloning under a read lock.
    /// Callers needing only globals should use narrower helpers.
    pub fn snapshot(&self) -> Settings {
        self.inner.read().expect("settings poisoned").clone()
    }

    /// Copy the `GlobalSettings` subset.
    pub fn global(&self) -> GlobalSettings {
        self.inner.read().expect("settings poisoned").global.clone()
    }

    pub fn pointer_forwarding_enabled(&self) -> bool {
        self.inner
            .read()
            .expect("settings poisoned")
            .global
            .pointer_forwarding_enabled
    }

    /// Clone the value map for a single plugin, or `None` if the
    /// plugin has no recorded settings.
    pub fn plugin(&self, plugin_name: &str) -> Option<HashMap<String, String>> {
        self.inner
            .read()
            .expect("settings poisoned")
            .plugins
            .get(plugin_name)
            .cloned()
    }

    pub fn resolved_renderer_settings(
        &self,
        renderer: &crate::plugin::renderer_registry::RendererDef,
    ) -> HashMap<String, String> {
        self.inner
            .read()
            .expect("settings poisoned")
            .resolved_renderer_settings(renderer)
    }

    pub fn apply_global_renderer_settings(
        &self,
        renderer: &crate::plugin::renderer_registry::RendererDef,
        values: &mut HashMap<String, String>,
    ) {
        self.inner
            .read()
            .expect("settings poisoned")
            .apply_global_renderer_settings(renderer, values);
    }

    /// Resolve the effective layout for a display name.
    /// Per-display overrides win field by field.
    pub fn resolved_layout(&self, display_name: &str) -> ResolvedLayout {
        let g = self.inner.read().expect("settings poisoned");
        let defaults = &g.global.layout;
        let prefs = g.displays.get(display_name);
        let default_location = defaults
            .location
            .unwrap_or_else(|| Location::from_align(defaults.align));
        ResolvedLayout {
            fillmode: prefs.and_then(|p| p.fillmode).unwrap_or(defaults.fillmode),
            location: prefs
                .and_then(|p| p.location)
                .or_else(|| prefs.and_then(|p| p.align.map(Location::from_align)))
                .unwrap_or(default_location),
            rotation: prefs.and_then(|p| p.rotation).unwrap_or(defaults.rotation),
            flip: prefs.and_then(|p| p.flip).unwrap_or_default(),
        }
    }

    pub fn resolved_global_layout(&self) -> ResolvedLayout {
        let g = self.inner.read().expect("settings poisoned");
        let defaults = &g.global.layout;
        ResolvedLayout {
            fillmode: defaults.fillmode,
            location: defaults
                .location
                .unwrap_or_else(|| Location::from_align(defaults.align)),
            rotation: defaults.rotation,
            flip: Flip::None,
        }
    }

    pub fn canvas_for_member(&self, display_key: &str) -> Option<(String, CanvasPrefs)> {
        let g = self.inner.read().expect("settings poisoned");
        g.canvases
            .iter()
            .find(|(_, canvas)| canvas.members.contains_key(display_key))
            .map(|(id, canvas)| (id.clone(), canvas.clone()))
    }

    pub fn resolved_canvas_layout(
        &self,
        canvas_id: &str,
        inherited: ResolvedLayout,
    ) -> ResolvedLayout {
        let g = self.inner.read().expect("settings poisoned");
        let canvas = g.canvases.get(canvas_id).and_then(|canvas| canvas.layout);
        ResolvedLayout {
            fillmode: canvas
                .and_then(|layout| layout.fillmode)
                .unwrap_or(inherited.fillmode),
            location: canvas
                .and_then(|layout| layout.location)
                .unwrap_or(inherited.location),
            rotation: canvas
                .and_then(|layout| layout.rotation)
                .unwrap_or(inherited.rotation),
            flip: canvas
                .and_then(|layout| layout.flip)
                .unwrap_or(inherited.flip),
        }
    }

    pub fn resolved_auto_replay(&self, display_name: &str) -> AutoReplayPolicy {
        let g = self.inner.read().expect("settings poisoned");
        if let Some(policy) = g
            .displays
            .get(display_name)
            .and_then(|prefs| prefs.auto_replay.clone())
        {
            return policy;
        }
        if let Some(policy) = &g.global.auto_replay {
            return policy.clone();
        }
        AutoReplayPolicy::default()
    }

    /// Per-display wallpaper id with fallback to global `last_wallpaper`.
    /// Used by hot-plug recall and startup restore.
    pub fn resolved_last_wallpaper(&self, display_key: &str) -> Option<String> {
        let g = self.inner.read().expect("settings poisoned");
        if let Some(prefs) = g.displays.get(display_key) {
            if let Some(id) = &prefs.last_wallpaper {
                return Some(id.clone());
            }
        }
        g.global.last_wallpaper.clone()
    }

    pub fn resolved_playlist_id(&self, display_key: &str) -> Option<i64> {
        let g = self.inner.read().expect("settings poisoned");
        let prefs = g.displays.get(display_key);
        if let Some(id) = prefs.and_then(|prefs| prefs.active_playlist_id) {
            return Some(id);
        }
        if prefs.is_some_and(|prefs| prefs.playlist_auto_attach_disabled) {
            return None;
        }
        g.global.auto_attach_playlist_id
    }

    /// Snapshot just the cloned per-display preferences.
    /// Used to expose overrides over the control plane.
    pub fn display_prefs(&self, display_name: &str) -> Option<DisplayPrefs> {
        self.inner
            .read()
            .expect("settings poisoned")
            .displays
            .get(display_name)
            .cloned()
    }

    /// Snapshot every registered display name in the prefs map.
    pub fn display_pref_names(&self) -> Vec<String> {
        self.inner
            .read()
            .expect("settings poisoned")
            .displays
            .keys()
            .cloned()
            .collect()
    }

    pub fn canvas_revision(&self) -> u64 {
        self.canvas_revision.load(Ordering::Acquire)
    }

    pub fn canvases(&self) -> HashMap<String, CanvasPrefs> {
        self.inner
            .read()
            .expect("settings poisoned")
            .canvases
            .clone()
    }

    pub fn canvas(&self, canvas_id: &str) -> Option<CanvasPrefs> {
        self.inner
            .read()
            .expect("settings poisoned")
            .canvases
            .get(canvas_id)
            .cloned()
    }

    pub fn configure_canvas_member(
        &self,
        canvas_id: &str,
        expected_revision: u64,
        display_key: &str,
        config: CanvasMemberConfig,
        actual: Option<CanvasSize>,
    ) -> crate::error::Result<CanvasMutationReceipt> {
        let display_key = display_key.trim();
        if display_key.is_empty() {
            return Err(crate::error::Error::CanvasInvalid(
                "canvas member has an empty display key".to_string(),
            ));
        }
        if let Some(actual) = actual {
            actual
                .validate()
                .map_err(|message| crate::error::Error::CanvasInvalid(message.to_string()))?;
        }
        let (canvas, previous, revision, changed) = {
            let mut settings = self.inner.write().expect("settings poisoned");
            let current_revision = self.canvas_revision();
            if expected_revision != current_revision {
                return Err(crate::error::Error::CanvasRevisionConflict {
                    expected: expected_revision,
                    current: current_revision,
                });
            }
            let previous = settings
                .canvases
                .get(canvas_id)
                .cloned()
                .ok_or_else(|| crate::error::Error::CanvasNotFound(canvas_id.to_string()))?;
            let current_member = previous.members.get(display_key).copied().ok_or_else(|| {
                crate::error::Error::CanvasInvalid(format!(
                    "canvas member '{display_key}' does not exist"
                ))
            })?;
            let aspect_locked = config.aspect_locked.unwrap_or(current_member.aspect_locked);
            let current_size = current_member.rect.size();
            let aspect = actual.unwrap_or(current_size);
            let next_size = match config.size {
                Some(CanvasMemberSizeChange::Reset) => actual.ok_or_else(|| {
                    crate::error::Error::CanvasInvalid(format!(
                        "canvas member '{display_key}' is offline"
                    ))
                })?,
                Some(CanvasMemberSizeChange::Width(width)) => {
                    CanvasSize {
                        width,
                        height: current_size.height,
                    }
                    .validate()
                    .map_err(|message| crate::error::Error::CanvasInvalid(message.to_string()))?;
                    if aspect_locked {
                        let width = if let Some(actual) = actual {
                            width.max(actual.width)
                        } else {
                            width.max(current_size.width)
                        };
                        aspect_size_from_width(width, aspect)?
                    } else {
                        let requested = CanvasSize {
                            width,
                            height: current_size.height,
                        };
                        actual.map_or(requested, |actual| requested.component_max(actual))
                    }
                }
                Some(CanvasMemberSizeChange::Height(height)) => {
                    CanvasSize {
                        width: current_size.width,
                        height,
                    }
                    .validate()
                    .map_err(|message| crate::error::Error::CanvasInvalid(message.to_string()))?;
                    if aspect_locked {
                        let height = if let Some(actual) = actual {
                            height.max(actual.height)
                        } else {
                            height.max(current_size.height)
                        };
                        aspect_size_from_height(height, aspect)?
                    } else {
                        let requested = CanvasSize {
                            width: current_size.width,
                            height,
                        };
                        actual.map_or(requested, |actual| requested.component_max(actual))
                    }
                }
                None => current_size,
            };
            let next_member = CanvasMemberPrefs {
                rect: CanvasRect {
                    x: current_member.rect.x,
                    y: current_member.rect.y,
                    width: next_size.width,
                    height: next_size.height,
                },
                aspect_locked,
            };
            if next_member == current_member {
                (previous.clone(), previous, current_revision, false)
            } else {
                let extent = crate::wallframe::display::placement::union(
                    previous.members.iter().map(|(key, member)| {
                        if key == display_key {
                            next_member.rect
                        } else {
                            member.rect
                        }
                    }),
                );
                if extent.is_none() {
                    return Err(crate::error::Error::CanvasInvalid(
                        "canvas extent is invalid after member configuration".to_string(),
                    ));
                }
                let canvas = settings
                    .canvases
                    .get_mut(canvas_id)
                    .expect("canvas disappeared while configuring member");
                *canvas
                    .members
                    .get_mut(display_key)
                    .expect("canvas member disappeared while configuring") = next_member;
                let canvas = canvas.clone();
                let revision = self.canvas_revision.fetch_add(1, Ordering::AcqRel) + 1;
                (canvas, previous, revision, true)
            }
        };
        if changed {
            self.dirty.store(true, Ordering::Release);
            self.notify.notify_one();
        }
        Ok(CanvasMutationReceipt {
            canvas_id: canvas_id.to_string(),
            canvas,
            revision,
            affected_display_keys: vec![display_key.to_string()],
            previous: Some(previous),
        })
    }

    pub fn set_canvas_layout(
        &self,
        canvas_id: &str,
        layout: Option<CanvasLayoutPrefs>,
    ) -> crate::error::Result<bool> {
        let changed = {
            let mut settings = self.inner.write().expect("settings poisoned");
            let canvas = settings
                .canvases
                .get_mut(canvas_id)
                .ok_or_else(|| crate::error::Error::CanvasNotFound(canvas_id.to_string()))?;
            if canvas.layout == layout {
                false
            } else {
                canvas.layout = layout;
                true
            }
        };
        if changed {
            self.dirty.store(true, Ordering::Release);
            self.notify.notify_one();
        }
        Ok(changed)
    }

    fn check_canvas_members(
        settings: &Settings,
        except_canvas_id: Option<&str>,
        members: &HashMap<String, CanvasMemberPrefs>,
    ) -> crate::error::Result<()> {
        for key in members.keys() {
            if let Some((canvas_id, canvas)) =
                settings.canvases.iter().find(|(canvas_id, canvas)| {
                    Some(canvas_id.as_str()) != except_canvas_id && canvas.members.contains_key(key)
                })
            {
                return Err(crate::error::Error::CanvasMemberConflict {
                    display_key: key.clone(),
                    canvas_id: canvas_id.clone(),
                    canvas_name: canvas.name.clone(),
                });
            }
        }
        Ok(())
    }

    pub fn create_canvas(
        &self,
        draft: CanvasDraft,
        live_minimums: &HashMap<String, CanvasSize>,
    ) -> crate::error::Result<CanvasMutationReceipt> {
        let mut canvas = normalize_canvas_draft(draft)?;
        reconcile_canvas_draft_sizes(&mut canvas.members, live_minimums)?;
        validate_canvas_scale_to(&canvas.members, live_minimums, None)?;
        let canvas_id = uuid::Uuid::new_v4().to_string();
        let affected_display_keys = canvas.members.keys().cloned().collect::<Vec<_>>();
        let revision = {
            let mut settings = self.inner.write().expect("settings poisoned");
            Self::check_canvas_members(&settings, None, &canvas.members)?;
            settings.canvases.insert(canvas_id.clone(), canvas.clone());
            self.canvas_revision.fetch_add(1, Ordering::AcqRel) + 1
        };
        self.dirty.store(true, Ordering::Release);
        self.notify.notify_one();
        Ok(CanvasMutationReceipt {
            canvas_id,
            canvas,
            revision,
            affected_display_keys,
            previous: None,
        })
    }

    pub fn update_canvas(
        &self,
        canvas_id: &str,
        expected_revision: u64,
        draft: CanvasDraft,
        live_minimums: &HashMap<String, CanvasSize>,
    ) -> crate::error::Result<CanvasMutationReceipt> {
        let mut canvas = normalize_canvas_draft(draft)?;
        let (affected_display_keys, previous, revision) = {
            let mut settings = self.inner.write().expect("settings poisoned");
            let current_revision = self.canvas_revision();
            if expected_revision != current_revision {
                return Err(crate::error::Error::CanvasRevisionConflict {
                    expected: expected_revision,
                    current: current_revision,
                });
            }
            Self::check_canvas_members(&settings, Some(canvas_id), &canvas.members)?;
            let old = settings
                .canvases
                .get(canvas_id)
                .cloned()
                .ok_or_else(|| crate::error::Error::CanvasNotFound(canvas_id.to_string()))?;
            for (key, member) in &mut canvas.members {
                if old
                    .members
                    .get(key)
                    .is_some_and(|old| old.rect.size() == member.rect.size())
                {
                    continue;
                }
                if let Some(actual) = live_minimums.get(key) {
                    let size = reconcile_canvas_member_runtime_size(
                        member.rect.size(),
                        *actual,
                        member.aspect_locked,
                    )?;
                    member.rect.width = size.width;
                    member.rect.height = size.height;
                }
            }
            let changed_members = canvas
                .members
                .iter()
                .filter(|(key, member)| {
                    old.members
                        .get(*key)
                        .is_none_or(|old| old.rect.size() != member.rect.size())
                })
                .map(|(key, member)| (key.clone(), *member))
                .collect();
            validate_canvas_scale_to(&changed_members, live_minimums, Some(&old))?;
            canvas.last_wallpaper = old.last_wallpaper.clone();
            canvas.layout = old.layout;
            let mut affected = old.members.keys().cloned().collect::<HashSet<_>>();
            affected.extend(canvas.members.keys().cloned());
            settings
                .canvases
                .insert(canvas_id.to_string(), canvas.clone());
            let revision = self.canvas_revision.fetch_add(1, Ordering::AcqRel) + 1;
            (affected.into_iter().collect::<Vec<_>>(), old, revision)
        };
        self.dirty.store(true, Ordering::Release);
        self.notify.notify_one();
        Ok(CanvasMutationReceipt {
            canvas_id: canvas_id.to_string(),
            canvas,
            revision,
            affected_display_keys,
            previous: Some(previous),
        })
    }

    pub fn delete_canvas(
        &self,
        canvas_id: &str,
        expected_revision: u64,
    ) -> crate::error::Result<CanvasDeleteReceipt> {
        let (canvas, revision) = {
            let mut settings = self.inner.write().expect("settings poisoned");
            let current_revision = self.canvas_revision();
            if expected_revision != current_revision {
                return Err(crate::error::Error::CanvasRevisionConflict {
                    expected: expected_revision,
                    current: current_revision,
                });
            }
            let canvas = settings
                .canvases
                .remove(canvas_id)
                .ok_or_else(|| crate::error::Error::CanvasNotFound(canvas_id.to_string()))?;
            let revision = self.canvas_revision.fetch_add(1, Ordering::AcqRel) + 1;
            (canvas, revision)
        };
        let affected_display_keys = canvas.members.keys().cloned().collect::<Vec<_>>();
        self.dirty.store(true, Ordering::Release);
        self.notify.notify_one();
        Ok(CanvasDeleteReceipt {
            canvas_id: canvas_id.to_string(),
            revision,
            affected_display_keys,
            canvas,
        })
    }

    pub fn set_canvas_wallpaper(
        &self,
        canvas_id: &str,
        wallpaper_id: Option<String>,
    ) -> crate::error::Result<()> {
        let mut settings = self.inner.write().expect("settings poisoned");
        let canvas = settings
            .canvases
            .get_mut(canvas_id)
            .ok_or_else(|| crate::error::Error::CanvasNotFound(canvas_id.to_string()))?;
        if canvas.last_wallpaper == wallpaper_id {
            return Ok(());
        }
        canvas.last_wallpaper = wallpaper_id;
        drop(settings);
        self.dirty.store(true, Ordering::Release);
        self.notify.notify_one();
        Ok(())
    }

    /// Apply an in-memory mutation and compare before/after state.
    /// Only changed settings mark the store dirty.
    pub fn update<F>(&self, f: F)
    where
        F: FnOnce(&mut Settings),
    {
        let changed = {
            let mut g = self.inner.write().expect("settings poisoned");
            let before = g.clone();
            f(&mut g);
            *g != before
        };
        if changed {
            self.dirty.store(true, Ordering::SeqCst);
            self.notify.notify_one();
        }
    }

    async fn writer_loop(self: Arc<Self>) {
        loop {
            // Block until something needs to be written.
            self.notify.notified().await;
            // Debounce: keep resetting the timer until DEBOUNCE_WRITE
            // elapses without another update.
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(DEBOUNCE_WRITE) => break,
                    _ = self.notify.notified() => {}
                }
            }
            self.flush().await;
        }
    }

    /// Force a synchronous flush of current settings to disk.
    /// Bypasses the debounce window for shutdown.
    pub async fn flush_now(&self) {
        self.flush().await;
    }

    async fn flush(&self) {
        // Cheap fast path before grabbing the lock: if nothing has
        // changed since the last successful flush, skip entirely.
        if !self.dirty.load(Ordering::SeqCst) {
            return;
        }
        let _g = self.flush_lock.lock().await;
        // Re-check under the lock — another flush may have just
        // raced us to the same state.
        if !self.dirty.swap(false, Ordering::SeqCst) {
            return;
        }

        let snapshot = self.snapshot();
        let serialized = match toml::to_string_pretty(&snapshot) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("settings serialize failed: {e}");
                self.dirty.store(true, Ordering::SeqCst);
                return;
            }
        };

        if let Some(parent) = self.path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                log::warn!("settings create_dir_all {}: {e}", parent.display());
                self.dirty.store(true, Ordering::SeqCst);
                return;
            }
        }

        let tmp = {
            let mut p = self.path.clone();
            let new_name = match p.file_name() {
                Some(n) => {
                    let mut s = n.to_os_string();
                    s.push(".tmp");
                    s
                }
                None => {
                    self.dirty.store(true, Ordering::SeqCst);
                    return;
                }
            };
            p.set_file_name(new_name);
            p
        };
        if let Err(e) = tokio::fs::write(&tmp, serialized).await {
            log::warn!("settings write {}: {e}", tmp.display());
            self.dirty.store(true, Ordering::SeqCst);
            return;
        }
        if let Err(e) = tokio::fs::rename(&tmp, &self.path).await {
            log::warn!(
                "settings rename {} → {}: {e}",
                tmp.display(),
                self.path.display()
            );
            self.dirty.store(true, Ordering::SeqCst);
            return;
        }
        log::debug!("settings flushed to {}", self.path.display());
    }

    /// Read-only view of the on-disk path.
    /// Useful before the rest of `DaemonContext` is constructed.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Bring in-memory plugin tables in line with loaded renderer
    /// manifest schemas.
    pub fn reconcile(&self, registry: &crate::plugin::renderer_registry::RendererRegistry) -> bool {
        use crate::plugin::renderer_registry::{
            check_setting_bounds, setting_default_value, SettingDef,
        };

        let mut changed = false;
        let mut g = self.inner.write().expect("settings poisoned");

        // Pre-compute manifest schemas keyed by plugin name so user
        // tables can be checked for unknown plugins.
        let manifests: HashMap<String, &HashMap<String, SettingDef>> = registry
            .all_renderers()
            .into_iter()
            .map(|d| (d.name.clone(), &d.settings))
            .collect();

        // 1) Reconcile each known plugin's table.
        for (plugin_name, schema) in &manifests {
            if schema.is_empty() {
                continue;
            }
            let entry = g.plugins.entry(plugin_name.clone()).or_default();

            // Drop keys that aren't in the manifest anymore.
            let stale: Vec<String> = entry
                .keys()
                .filter(|k| !schema.contains_key(*k))
                .cloned()
                .collect();
            for k in stale {
                log::warn!(
                    "settings: dropping unknown key '{plugin_name}.{k}' \
                     (no longer in manifest schema)"
                );
                entry.remove(&k);
                changed = true;
            }

            // Fill in / reset bad values for declared keys.
            for (key, def) in schema.iter() {
                let needs_default = match entry.get(key) {
                    None => true,
                    Some(v) => match check_setting_bounds(key, v, def) {
                        Ok(()) => false,
                        Err(e) => {
                            log::warn!(
                                "settings: '{plugin_name}.{key}' = {v:?} \
                                 violates schema ({e}); resetting to default"
                            );
                            true
                        }
                    },
                };
                if needs_default {
                    let default = setting_default_value(def);
                    if entry.get(key) != Some(&default) {
                        entry.insert(key.clone(), default);
                        changed = true;
                    }
                }
            }
        }

        // Keep tables without a renderer manifest. They may belong to a Lua
        // source, or to a temporarily unavailable renderer.
        for plugin_name in g.plugins.keys() {
            if !manifests.contains_key(plugin_name) {
                log::warn!(
                    "settings: component '{plugin_name}' has persisted values \
                     but no matching renderer manifest is loaded; leaving as-is"
                );
            }
        }

        if changed {
            self.dirty.store(true, Ordering::SeqCst);
            self.notify.notify_one();
        }
        changed
    }
}
