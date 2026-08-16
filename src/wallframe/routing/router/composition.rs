use super::*;

#[derive(Debug, Clone)]
pub struct ApplyAssignment {
    pub spawn_request: crate::wallframe::renderer_manager::SpawnRequest,
    pub display_ids: Vec<DisplayId>,
    pub duplicate_renderers: bool,
    pub wallpaper_layout_override: WallpaperLayoutOverride,
    pub preempt_pending_start: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentActivation {
    Active,
    Deferred,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveRenderer {
    pub renderer_id: RendererId,
    pub process_generation: u64,
}

#[derive(Debug, Clone)]
pub struct ApplyReceipt {
    pub renderer_id: RendererId,
    pub activation: AssignmentActivation,
    pub active_renderers: Vec<ActiveRenderer>,
}

struct AssignmentCommit {
    primary_renderer_id: RendererId,
    selected_renderers: Vec<RendererId>,
    affected_displays: Vec<DisplayId>,
    dropped_renderers: Vec<RendererId>,
    removed_renderers: Vec<RendererId>,
    cancelled_starts: Vec<RendererId>,
}

impl Router {
    pub async fn wait_for_first_frame(
        self: &Arc<Self>,
        renderer: &ActiveRenderer,
        timeout: Duration,
    ) -> crate::error::Result<()> {
        self.mgr
            .wait_for_first_frame_generation(
                &renderer.renderer_id,
                renderer.process_generation,
                timeout,
            )
            .await
    }

    pub async fn apply_assignment(
        self: &Arc<Self>,
        mut request: ApplyAssignment,
    ) -> crate::error::Result<ApplyReceipt> {
        request.spawn_request.default_user_properties =
            crate::catalog::properties::normalize_renderer_user_properties(
                request.spawn_request.default_user_properties,
            );
        let renderer_name = request
            .spawn_request
            .renderer_name
            .clone()
            .unwrap_or_default();
        let commit = {
            let mut inner = self.inner.lock().await;
            let groups = if request.display_ids.is_empty() {
                vec![Vec::new()]
            } else if request.duplicate_renderers {
                request
                    .display_ids
                    .iter()
                    .map(|display_id| vec![*display_id])
                    .collect::<Vec<_>>()
            } else {
                vec![request.display_ids.clone()]
            };
            let mut primary_renderer_id = None;
            let mut selected_renderers = Vec::new();
            let mut affected_displays = Vec::new();
            let mut displaced_renderers = HashSet::new();

            for group in groups {
                let existing = if group.is_empty() {
                    inner.renderer_slots.keys().cloned().collect::<HashSet<_>>()
                } else {
                    group
                        .iter()
                        .flat_map(|display_id| inner.table.links_for_display(*display_id))
                        .map(|link| link.renderer_id)
                        .collect::<HashSet<_>>()
                };
                displaced_renderers.extend(existing.iter().cloned());
                let has_demand = group.iter().any(|display_id| {
                    inner.displays.get(display_id).is_some_and(|display| {
                        !inner.manual_stopped && !display.auto_replay.stop_applied
                    })
                });
                let reusable_running = has_demand
                    .then(|| {
                        let mut ids = inner
                            .renderer_slots
                            .iter()
                            .filter_map(|(renderer_id, slot)| {
                                let renderer_name_matches = request
                                    .spawn_request
                                    .renderer_name
                                    .as_deref()
                                    .is_none_or(|name| name == slot.name);
                                let identity_matches = slot.state.is_running()
                                    && inner.table.get_renderer(renderer_id).is_some()
                                    && slot.spawn_request.wp_type == request.spawn_request.wp_type
                                    && renderer_name_matches
                                    && slot.spawn_request.extras == request.spawn_request.extras
                                    && slot.spawn_request.default_user_properties
                                        == request.spawn_request.default_user_properties;
                                let target_matches = !request.duplicate_renderers
                                    || inner
                                        .table
                                        .links_for_renderer(renderer_id)
                                        .iter()
                                        .all(|link| group.contains(&link.display_id));
                                (identity_matches && target_matches).then(|| renderer_id.clone())
                            })
                            .collect::<Vec<_>>();
                        ids.sort();
                        ids.into_iter().next()
                    })
                    .flatten();
                let reusable_terminal = (existing.len() == 1)
                    .then(|| existing.iter().next().cloned())
                    .flatten()
                    .filter(|renderer_id| {
                        inner
                            .table
                            .links_for_renderer(renderer_id)
                            .iter()
                            .all(|link| group.contains(&link.display_id))
                            && inner.renderer_slots.get(renderer_id).is_some_and(|slot| {
                                matches!(
                                    slot.state,
                                    RendererLifecycleState::Stopping { keep: true, .. }
                                        | RendererLifecycleState::Stopped { keep: true, .. }
                                        | RendererLifecycleState::Killed { keep: true, .. }
                                        | RendererLifecycleState::Failed { .. }
                                )
                            })
                    });
                let renderer_id = reusable_running
                    .or(reusable_terminal)
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                // Singleton groups get their own display's real size.
                let mut group_spawn_request = request.spawn_request.clone();
                if let [only_id] = group[..] {
                    if let Some(display) = inner.displays.get(&only_id) {
                        group_spawn_request.display_size =
                            Some((display.info.metrics.width, display.info.metrics.height));
                    }
                }
                if !inner.renderer_slots.contains_key(&renderer_id) {
                    inner.renderer_slots.insert(
                        renderer_id.clone(),
                        RendererSlot::retained(group_spawn_request.clone(), renderer_name.clone()),
                    );
                } else if !inner
                    .renderer_slots
                    .get(&renderer_id)
                    .is_some_and(|slot| slot.state.is_running())
                {
                    inner
                        .renderer_slots
                        .get_mut(&renderer_id)
                        .expect("selected renderer slot disappeared")
                        .replace_spec(
                            group_spawn_request.clone(),
                            renderer_name.clone(),
                            request.preempt_pending_start,
                        );
                }
                if request.wallpaper_layout_override.is_empty() {
                    inner.wallpaper_layout_overrides.remove(&renderer_id);
                } else {
                    inner
                        .wallpaper_layout_overrides
                        .insert(renderer_id.clone(), request.wallpaper_layout_override);
                }
                for display_id in group {
                    if !inner.displays.contains_key(&display_id) {
                        continue;
                    }
                    let enabled = !inner.manual_stopped
                        && !inner
                            .displays
                            .get(&display_id)
                            .is_some_and(|display| display.auto_replay.stop_applied);
                    inner
                        .table
                        .add_link_with_enabled(renderer_id.clone(), display_id, enabled);
                    if let Some(display) = inner.displays.get(&display_id) {
                        display.invalidate_consumption();
                    }
                    affected_displays.push(display_id);
                }
                primary_renderer_id.get_or_insert_with(|| renderer_id.clone());
                selected_renderers.push(renderer_id);
            }

            selected_renderers.sort();
            selected_renderers.dedup();
            let selected = selected_renderers.iter().cloned().collect::<HashSet<_>>();
            let mut dropped_renderers = Vec::new();
            let mut removed_renderers = Vec::new();
            let mut cancelled_starts = Vec::new();
            for renderer_id in displaced_renderers {
                if selected.contains(&renderer_id)
                    || !inner.table.links_for_renderer(&renderer_id).is_empty()
                {
                    continue;
                }
                let has_process = inner
                    .renderer_slots
                    .get(&renderer_id)
                    .is_some_and(|slot| slot.state.has_process());
                if has_process {
                    if let Some(slot) = inner.renderer_slots.get_mut(&renderer_id) {
                        slot.pending_start = None;
                        let _ =
                            slot.transition(RendererLifecycleEvent::StopRequested { keep: false });
                    }
                    inner
                        .unbind_acks_pending
                        .entry(renderer_id.clone())
                        .or_default();
                    cancelled_starts.push(renderer_id.clone());
                    dropped_renderers.push(renderer_id);
                } else if inner.renderer_slots.remove(&renderer_id).is_some() {
                    inner.wallpaper_layout_overrides.remove(&renderer_id);
                    inner.renderer_manual_paused.remove(&renderer_id);
                    cancelled_starts.push(renderer_id.clone());
                    removed_renderers.push(renderer_id);
                }
            }
            for renderer_id in &selected_renderers {
                let has_demand = inner
                    .table
                    .links_for_renderer(renderer_id)
                    .iter()
                    .any(|link| link.enabled && inner.displays.contains_key(&link.display_id));
                if !has_demand
                    && inner
                        .renderer_slots
                        .get_mut(renderer_id)
                        .is_some_and(|slot| slot.pending_start.take().is_some())
                {
                    cancelled_starts.push(renderer_id.clone());
                }
            }
            AssignmentCommit {
                primary_renderer_id: primary_renderer_id
                    .expect("assignment must select at least one renderer"),
                selected_renderers,
                affected_displays,
                dropped_renderers,
                removed_renderers,
                cancelled_starts,
            }
        };

        for renderer_id in &commit.cancelled_starts {
            self.deadlines
                .cancel(deadline::DeadlineKey::renderer_start(renderer_id));
        }
        for renderer_id in &commit.removed_renderers {
            self.emit(RouterEvent::RendererRemoved(renderer_id.clone()));
        }
        let mut affected_displays = commit.affected_displays;
        affected_displays.sort_unstable();
        affected_displays.dedup();
        for display_id in &affected_displays {
            self.sync_display(*display_id).await;
        }
        for renderer_id in &commit.dropped_renderers {
            if let Err(error) = self
                .stop_renderer_drop(renderer_id, Duration::from_secs(1))
                .await
            {
                log::warn!(
                    "renderer {renderer_id}: assignment replacement cleanup failed: {error}"
                );
            }
        }

        let cause = RendererStartCause::ExplicitApply {
            preempt_pending: request.preempt_pending_start,
        };
        let mut start_error = None;
        for renderer_id in &commit.selected_renderers {
            let failed_background = {
                let inner = self.inner.lock().await;
                !request.preempt_pending_start
                    && inner
                        .renderer_slots
                        .get(renderer_id)
                        .is_some_and(|slot| slot.state.is_failed())
            };
            if !failed_background {
                if let Err(error) = self.request_renderer_start(renderer_id, cause).await {
                    start_error.get_or_insert(error);
                }
            }
        }

        let mut active_renderers = Vec::new();
        for renderer_id in &commit.selected_renderers {
            let state = self
                .inner
                .lock()
                .await
                .renderer_slots
                .get(renderer_id)
                .map(|slot| slot.state.clone());
            match state {
                Some(RendererLifecycleState::Running { generation, .. }) => {
                    active_renderers.push(ActiveRenderer {
                        renderer_id: renderer_id.clone(),
                        process_generation: generation,
                    });
                }
                Some(RendererLifecycleState::Failed { failure })
                    if request.preempt_pending_start =>
                {
                    if start_error.is_none() {
                        start_error =
                            Some(crate::error::Error::RendererSpawnFailed(failure.reason));
                    }
                }
                _ => {}
            }
            if let Some(snapshot) = self.snapshot_renderer(renderer_id).await {
                self.emit(RouterEvent::RendererUpsert(snapshot));
            }
        }
        self.reconcile_lifecycle().await;
        self.reconcile_buffer_flags().await;
        if !affected_displays.is_empty() {
            self.emit(RouterEvent::DisplaysReplace(self.snapshot_displays().await));
        }
        if let Some(error) = start_error {
            return Err(error);
        }
        Ok(ApplyReceipt {
            renderer_id: commit.primary_renderer_id,
            activation: if active_renderers.is_empty() {
                AssignmentActivation::Deferred
            } else {
                AssignmentActivation::Active
            },
            active_renderers,
        })
    }

    pub async fn reusable_renderer_for_target(
        self: &Arc<Self>,
        request: &crate::wallframe::renderer_manager::SpawnRequest,
        target_ids: &[DisplayId],
        duplicate_renderer: bool,
    ) -> Option<RendererId> {
        let normalized_defaults = crate::catalog::properties::normalize_renderer_user_properties(
            request.default_user_properties.clone(),
        );
        let inner = self.inner.lock().await;
        let mut ids = inner
            .renderer_slots
            .iter()
            .filter_map(|(renderer_id, slot)| {
                let renderer_name_matches = request
                    .renderer_name
                    .as_deref()
                    .is_none_or(|name| name == slot.name);
                let identity_matches = slot.state.is_running()
                    && inner.table.get_renderer(renderer_id).is_some()
                    && slot.spawn_request.wp_type == request.wp_type
                    && renderer_name_matches
                    && slot.spawn_request.extras == request.extras
                    && slot.spawn_request.default_user_properties == normalized_defaults;
                let target_matches = !duplicate_renderer
                    || inner
                        .table
                        .links_for_renderer(renderer_id)
                        .iter()
                        .all(|link| target_ids.contains(&link.display_id));
                (identity_matches && target_matches).then(|| renderer_id.clone())
            })
            .collect::<Vec<_>>();
        ids.sort();
        ids.into_iter().next()
    }

    pub async fn renderer_ids_by_resource(self: &Arc<Self>, resource: &str) -> Vec<RendererId> {
        let inner = self.inner.lock().await;
        let mut ids = inner
            .renderer_slots
            .iter()
            .filter_map(|(id, slot)| {
                (slot.spawn_request.extras.get("path").map(String::as_str) == Some(resource))
                    .then(|| id.clone())
            })
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }

    pub async fn update_renderer_assignment_property(
        self: &Arc<Self>,
        renderer_id: &str,
        key: &str,
        value: Option<&str>,
    ) -> bool {
        let mut inner = self.inner.lock().await;
        let Some(slot) = inner.renderer_slots.get_mut(renderer_id) else {
            return false;
        };
        let key = crate::catalog::properties::canonical_user_property_key(key).to_string();
        match value {
            Some(value) => {
                slot.spawn_request
                    .user_property_overrides
                    .insert(key, value.to_string());
            }
            None => {
                slot.spawn_request.user_property_overrides.remove(&key);
            }
        }
        slot.spec_revision = slot.spec_revision.wrapping_add(1).max(1);
        true
    }

    pub async fn update_renderer_assignment_settings(
        self: &Arc<Self>,
        renderer_name: &str,
        settings: &[(String, String)],
    ) -> Vec<RendererId> {
        let mut inner = self.inner.lock().await;
        let mut updated = Vec::new();
        for (renderer_id, slot) in &mut inner.renderer_slots {
            if slot.name != renderer_name {
                continue;
            }
            let mut changed = false;
            for (key, value) in settings {
                if slot.spawn_request.settings.get(key) != Some(value) {
                    slot.spawn_request
                        .settings
                        .insert(key.clone(), value.clone());
                    changed = true;
                }
            }
            if changed {
                slot.spec_revision = slot.spec_revision.wrapping_add(1).max(1);
                updated.push(renderer_id.clone());
            }
        }
        updated.sort();
        updated
    }

    pub async fn update_renderer_assignment_fps(
        self: &Arc<Self>,
        renderer_id: &str,
        fps: u32,
    ) -> bool {
        let mut inner = self.inner.lock().await;
        let Some(slot) = inner.renderer_slots.get_mut(renderer_id) else {
            return false;
        };
        let value = fps.to_string();
        if slot.spawn_request.settings.get("fps") != Some(&value) {
            slot.spawn_request.settings.insert("fps".to_string(), value);
            slot.spec_revision = slot.spec_revision.wrapping_add(1).max(1);
        }
        true
    }

    pub async fn update_renderer_assignment_layout(
        self: &Arc<Self>,
        renderer_id: &str,
        layout: WallpaperLayoutOverride,
    ) -> bool {
        let exists = self
            .inner
            .lock()
            .await
            .renderer_slots
            .contains_key(renderer_id);
        if !exists {
            return false;
        }
        self.set_renderer_wallpaper_layout_override(renderer_id, layout)
            .await
    }

    // Routing policy

    /// Return the live renderers whose every display assignment is
    /// covered by `target`, meaning an imminent relink fully replaces them.
    pub async fn renderers_fully_replaced_by(
        self: &Arc<Self>,
        target: Option<&[DisplayId]>,
    ) -> Vec<RendererId> {
        let inner = self.inner.lock().await;
        inner
            .table
            .renderer_ids()
            .into_iter()
            .filter(|rid| {
                let links = inner.table.links_for_renderer(rid);
                if links.is_empty() {
                    return true;
                }
                match target {
                    None => true,
                    Some(ts) => links.iter().all(|l| ts.contains(&l.display_id)),
                }
            })
            .collect()
    }

    /// Stop and drop each logical renderer.
    pub async fn stop_renderers(self: &Arc<Self>, ids: &[RendererId]) {
        for id in ids {
            if let Err(error) = self.kill_renderer_drop(id).await {
                log::warn!("router: stop_renderers: kill {id}: {error}");
            }
        }
    }

    /// Stop the listed renderers after their displays release live bindings.
    pub async fn stop_renderers_orderly(
        self: &Arc<Self>,
        ids: &[RendererId],
        ack_timeout: Duration,
    ) {
        for id in ids {
            if let Err(error) = self.stop_renderer_drop(id, ack_timeout).await {
                log::warn!("router: stop_renderers_orderly: kill {id}: {error}");
            }
        }
    }

    /// Re-point each display assignment to `new_renderer_id`.
    pub async fn relink_displays_to(
        self: &Arc<Self>,
        display_ids: &[DisplayId],
        new_renderer_id: &str,
    ) {
        let retained_stops = self
            .renderers_inactivated_by_relink(display_ids, new_renderer_id)
            .await;
        for renderer_id in &retained_stops {
            self.begin_retained_stop(renderer_id).await;
        }
        let applied: Vec<DisplayId> = {
            let mut inner = self.inner.lock().await;
            let mut out = Vec::with_capacity(display_ids.len());
            for did in display_ids {
                if !inner.displays.contains_key(did) {
                    continue;
                }
                let existing = inner.table.links_for_display(*did);
                for link in existing {
                    inner.table.remove_link(link.id);
                }
                let enabled = !inner.manual_stopped
                    && !inner
                        .displays
                        .get(did)
                        .is_some_and(|display| display.auto_replay.stop_applied);
                inner
                    .table
                    .add_link_with_enabled(new_renderer_id.to_string(), *did, enabled);
                out.push(*did);
            }
            out
        };
        for did in &applied {
            self.sync_display(*did).await;
        }
        for renderer_id in retained_stops {
            self.finish_retained_stop(&renderer_id).await;
        }
        self.reconcile_lifecycle().await;
        // See `relink_all_displays_to` for the GC rationale. We always
        // run the mark pass so partially displaced renderers are handled.
        self.mark_orphans(Some(new_renderer_id)).await;
        self.reconcile_buffer_flags().await;
        if !applied.is_empty() {
            let all = self.snapshot_displays().await;
            self.emit(RouterEvent::DisplaysReplace(all));
        }
    }

    async fn renderers_inactivated_by_relink(
        self: &Arc<Self>,
        display_ids: &[DisplayId],
        new_renderer_id: &str,
    ) -> Vec<RendererId> {
        let inner = self.inner.lock().await;
        inner
            .table
            .renderer_ids()
            .into_iter()
            .filter(|renderer_id| renderer_id != new_renderer_id)
            .filter(|renderer_id| {
                let links = inner.table.links_for_renderer(renderer_id);
                links
                    .iter()
                    .any(|link| !display_ids.contains(&link.display_id))
                    && links
                        .iter()
                        .filter(|link| link.enabled)
                        .all(|link| display_ids.contains(&link.display_id))
            })
            .collect()
    }

    pub async fn relink_all_displays_to(self: &Arc<Self>, new_renderer_id: &str) {
        let display_ids: Vec<DisplayId> = {
            let mut inner = self.inner.lock().await;
            let ids: Vec<DisplayId> = inner.displays.keys().copied().collect();
            for did in &ids {
                let existing = inner.table.links_for_display(*did);
                for link in existing {
                    inner.table.remove_link(link.id);
                }
                let enabled = !inner.manual_stopped
                    && !inner
                        .displays
                        .get(did)
                        .is_some_and(|display| display.auto_replay.stop_applied);
                inner
                    .table
                    .add_link_with_enabled(new_renderer_id.to_string(), *did, enabled);
            }
            ids
        };
        let had_ids = !display_ids.is_empty();
        for did in display_ids {
            self.sync_display(did).await;
        }
        self.reconcile_lifecycle().await;
        // Active GC: any renderer that is no longer referenced by any
        // display gets a reap timer; the new renderer is kept.
        self.mark_orphans(Some(new_renderer_id)).await;
        self.reconcile_buffer_flags().await;
        if had_ids {
            let all = self.snapshot_displays().await;
            self.emit(RouterEvent::DisplaysReplace(all));
        }
    }

    /// Mutate a link's geometry/clear color and re-emit `SetCompositionConfig` to
    /// the affected display, without Bind or Unbind.
    pub async fn set_link_geometry(
        self: &Arc<Self>,
        link_id: LinkId,
        src: Option<LinkSrcRect>,
        dst: Option<LinkDstRect>,
        transform: Option<u32>,
        clear_rgba: Option<[f32; 4]>,
        z_order: Option<i32>,
    ) -> bool {
        let affected_display = {
            let mut inner = self.inner.lock().await;
            let changed = inner
                .table
                .update_link_geometry(link_id, src, dst, transform, clear_rgba, z_order);
            if !changed {
                return false;
            }
            let Some(link) = inner.table.get_link(link_id).cloned() else {
                return false;
            };
            inner
                .displays
                .contains_key(&link.display_id)
                .then_some(link.display_id)
        };
        if let Some(did) = affected_display {
            self.resync_display_composition(did).await;
            if let Some(snap) = self.snapshot_display(did).await {
                self.emit(RouterEvent::DisplayUpsert(snap));
            }
        } else {
            return false;
        }
        true
    }

    // ---------------------------------------------------------------
}
