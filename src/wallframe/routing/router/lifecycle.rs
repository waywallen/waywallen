use super::composition::IsolatedRenderer;
use super::*;

pub(super) const AUTO_REPLAY_START_DELAY: Duration = Duration::from_secs(2);

struct RendererStartEffect {
    renderer_id: RendererId,
    process_generation: u64,
    spec_revision: u64,
    start_token: u64,
    cause: RendererStartCause,
    spawn_request: crate::wallframe::renderer_manager::SpawnRequest,
}

enum AdvanceStart {
    Start(RendererStartEffect, Vec<IsolatedRenderer>),
    Schedule(PendingRendererStart),
    Cancel,
    Wait,
}

impl Router {
    pub(super) async fn spawn_unassigned_renderer(
        self: &Arc<Self>,
        mut spawn_request: crate::wallframe::renderer_manager::SpawnRequest,
    ) -> crate::error::Result<RendererId> {
        spawn_request.default_user_properties =
            crate::catalog::properties::normalize_renderer_user_properties(
                spawn_request.default_user_properties,
            );
        let renderer_id = uuid::Uuid::new_v4().to_string();
        let effect = {
            let mut inner = self.inner.lock().await;
            inner.next_start_token = inner
                .next_start_token
                .checked_add(1)
                .expect("renderer start token exhausted");
            let start_token = inner.next_start_token;
            let process_generation = self.mgr.reserve_process_generation();
            let name = spawn_request.renderer_name.clone().unwrap_or_default();
            let mut slot = RendererSlot::retained(spawn_request.clone(), name);
            let transition = slot.transition(RendererLifecycleEvent::StartRequested {
                generation: process_generation,
                start_token,
                reactivate_failed: false,
            });
            debug_assert_eq!(transition, RendererTransition::Changed);
            inner.renderer_slots.insert(renderer_id.clone(), slot);
            RendererStartEffect {
                renderer_id: renderer_id.clone(),
                process_generation,
                spec_revision: 1,
                start_token,
                cause: RendererStartCause::ExplicitSpawn,
                spawn_request,
            }
        };
        if let Some(snapshot) = self.snapshot_renderer(&renderer_id).await {
            self.emit(RouterEvent::RendererUpsert(snapshot));
        }
        if let Err(error) = self.execute_renderer_start(effect).await {
            self.unregister_renderer(&renderer_id).await;
            return Err(error);
        }
        Ok(renderer_id)
    }

    pub async fn set_renderer_paused(self: &Arc<Self>, renderer_id: &str, paused: bool) -> bool {
        let changed = {
            let mut inner = self.inner.lock().await;
            if !inner.renderer_slots.contains_key(renderer_id) {
                return false;
            }
            if paused {
                inner.renderer_manual_paused.insert(renderer_id.to_string())
            } else {
                inner.renderer_manual_paused.remove(renderer_id)
            }
        };
        if changed {
            self.reconcile_lifecycle().await;
        }
        true
    }

    pub async fn kill_renderer_drop(
        self: &Arc<Self>,
        renderer_id: &str,
    ) -> crate::error::Result<()> {
        self.stop_renderer_drop_current(renderer_id, Duration::from_secs(1), None)
            .await
    }

    pub async fn kill_renderer_generation_drop(
        self: &Arc<Self>,
        renderer_id: &str,
        process_generation: u64,
    ) -> crate::error::Result<()> {
        self.stop_renderer_drop_current(
            renderer_id,
            Duration::from_secs(1),
            Some(process_generation),
        )
        .await
    }

    pub(super) async fn stop_renderer_drop(
        self: &Arc<Self>,
        renderer_id: &str,
        ack_timeout: Duration,
    ) -> crate::error::Result<()> {
        self.stop_renderer_drop_current(renderer_id, ack_timeout, None)
            .await
    }

    pub async fn restart_renderers_orderly(
        self: &Arc<Self>,
        renderer_ids: &[RendererId],
        ack_timeout: Duration,
        first_frame_timeout: Duration,
    ) -> crate::error::Result<()> {
        let mut renderer_ids = renderer_ids.to_vec();
        renderer_ids.sort();
        renderer_ids.dedup();

        let mut active = Vec::new();
        let mut first_error = None;
        for renderer_id in renderer_ids {
            let result = self
                .restart_renderer_orderly(&renderer_id, ack_timeout)
                .await;
            match result {
                Ok(Some(renderer)) => active.push(renderer),
                Ok(None) => {}
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }
        for renderer in &active {
            if let Err(error) = self
                .wait_for_first_frame(renderer, first_frame_timeout)
                .await
            {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    async fn restart_renderer_orderly(
        self: &Arc<Self>,
        renderer_id: &str,
        ack_timeout: Duration,
    ) -> crate::error::Result<Option<ActiveRenderer>> {
        let (process_generation, has_assignment, has_demand) = {
            let inner = self.inner.lock().await;
            let Some(handle) = inner.table.get_renderer(renderer_id) else {
                return Ok(None);
            };
            let links = inner.table.links_for_renderer(renderer_id);
            let has_assignment = !links.is_empty();
            let has_demand = links
                .iter()
                .any(|link| link.enabled && inner.displays.contains_key(&link.display_id));
            (handle.process_generation, has_assignment, has_demand)
        };

        if !has_assignment {
            self.stop_renderer_drop(renderer_id, ack_timeout).await?;
            return Ok(None);
        }

        self.begin_retained_stop(renderer_id).await;
        if has_demand {
            self.request_renderer_start(renderer_id, RendererStartCause::ExplicitRestart)
                .await?;
        }

        let affected_displays = {
            let mut inner = self.inner.lock().await;
            let current = inner
                .table
                .get_renderer(renderer_id)
                .is_some_and(|handle| handle.process_generation == process_generation);
            if !current {
                return Err(crate::error::Error::Internal(anyhow::anyhow!(
                    "renderer {renderer_id} changed generation while preparing restart"
                )));
            }
            inner.table.detach_renderer(renderer_id);
            let display_ids = inner
                .table
                .links_for_renderer(renderer_id)
                .into_iter()
                .map(|link| link.display_id)
                .collect::<Vec<_>>();
            for display_id in &display_ids {
                if let Some(display) = inner.displays.get(display_id) {
                    display.invalidate_consumption();
                }
            }
            display_ids
        };
        for display_id in affected_displays {
            self.sync_display(display_id).await;
        }

        self.finish_retained_stop(renderer_id).await;

        let (state, still_has_demand) = {
            let inner = self.inner.lock().await;
            let state = inner
                .renderer_slots
                .get(renderer_id)
                .map(|slot| slot.state.clone())
                .ok_or_else(|| crate::error::Error::RendererNotFound(renderer_id.to_string()))?;
            let still_has_demand = inner
                .table
                .links_for_renderer(renderer_id)
                .iter()
                .any(|link| link.enabled && inner.displays.contains_key(&link.display_id));
            (state, still_has_demand)
        };
        match state {
            RendererLifecycleState::Running { generation, .. }
                if generation != process_generation =>
            {
                Ok(Some(ActiveRenderer {
                    renderer_id: renderer_id.to_string(),
                    process_generation: generation,
                }))
            }
            RendererLifecycleState::Stopped { keep: true, .. }
            | RendererLifecycleState::Killed { keep: true, .. }
                if !still_has_demand =>
            {
                Ok(None)
            }
            RendererLifecycleState::Failed { failure } => {
                Err(crate::error::Error::RendererSpawnFailed(failure.reason))
            }
            state => Err(crate::error::Error::Internal(anyhow::anyhow!(
                "renderer {renderer_id} restart ended in state {}",
                state.as_str()
            ))),
        }
    }

    async fn stop_renderer_drop_current(
        self: &Arc<Self>,
        renderer_id: &str,
        ack_timeout: Duration,
        expected_generation: Option<u64>,
    ) -> crate::error::Result<()> {
        let process_generation = {
            let mut inner = self.inner.lock().await;
            let Some(slot) = inner.renderer_slots.get_mut(renderer_id) else {
                return Err(crate::error::Error::RendererNotFound(
                    renderer_id.to_string(),
                ));
            };
            if expected_generation.is_some() && slot.state.generation() != expected_generation {
                return Ok(());
            }
            slot.pending_start = None;
            let transition = slot.transition(RendererLifecycleEvent::StopRequested { keep: false });
            let process_generation = match (&slot.state, transition) {
                (RendererLifecycleState::Stopping { generation, .. }, _) => Some(*generation),
                (_, RendererTransition::Remove) => None,
                _ => None,
            };
            for link in inner.table.links_for_renderer(renderer_id) {
                inner.table.set_link_enabled(link.id, false);
                if let Some(display) = inner.displays.get(&link.display_id) {
                    display.invalidate_consumption();
                }
            }
            process_generation
        };
        self.deadlines
            .cancel(deadline::DeadlineKey::renderer_start(renderer_id));
        let Some(process_generation) = process_generation else {
            self.unregister_renderer(renderer_id).await;
            return Ok(());
        };
        self.begin_unbind_ack_tracking(renderer_id).await;
        let displays = {
            let inner = self.inner.lock().await;
            inner
                .table
                .links_for_renderer(renderer_id)
                .into_iter()
                .map(|link| link.display_id)
                .collect::<Vec<_>>()
        };
        for display_id in displays {
            self.sync_display(display_id).await;
        }
        if let Some(snapshot) = self.snapshot_renderer(renderer_id).await {
            self.emit(RouterEvent::RendererUpsert(snapshot));
        }
        if self
            .await_unbind_acks_for(renderer_id, ack_timeout)
            .await
            .is_err()
        {
            log::warn!("renderer {renderer_id}: kill unbind acknowledgement timed out");
        }
        if let Some(exit) = self
            .mgr
            .stop_generation(renderer_id, process_generation)
            .await?
        {
            self.on_renderer_process_exit(exit).await;
        }
        Ok(())
    }

    pub(super) async fn apply_auto_stop_links(self: &Arc<Self>) {
        {
            let mut inner = self.inner.lock().await;
            inner.auto_stopped_renderers = inner
                .renderer_slots
                .keys()
                .filter(|id| inner.auto_stops_renderer(id))
                .cloned()
                .collect();
            let plans: Vec<(DisplayId, bool)> = inner
                .displays
                .iter()
                .filter_map(|(display_id, state)| {
                    let should_stop =
                        inner.auto_effects.stop || state.auto_replay.effects().stop_local;
                    (state.auto_replay.stop_applied != should_stop)
                        .then_some((*display_id, should_stop))
                })
                .collect();
            for (display_id, should_stop) in plans {
                if let Some(state) = inner.displays.get_mut(&display_id) {
                    state.auto_replay.stop_applied = should_stop;
                }
            }
        }
        self.reconcile_assignment_activation(RendererStartCause::AutoReplayResume)
            .await;
    }

    async fn reconcile_assignment_activation(self: &Arc<Self>, resume_cause: RendererStartCause) {
        let (mut changed_displays, mut reenabled_renderers, mut stopped_renderers) = {
            let mut inner = self.inner.lock().await;
            let display_ids = inner.displays.keys().copied().collect::<Vec<_>>();
            let mut changed_displays = Vec::new();
            let mut reenabled_renderers = Vec::new();
            for display_id in display_ids {
                let enabled = !inner.manual_stopped
                    && !inner.auto_effects.stop
                    && !inner
                        .displays
                        .get(&display_id)
                        .is_some_and(|display| display.auto_replay.stop_applied);
                let mut changed = false;
                for link in inner.table.links_for_display(display_id) {
                    let enabled = enabled && !inner.auto_stops_renderer(&link.renderer_id);
                    if inner.table.set_link_enabled(link.id, enabled) {
                        changed = true;
                        if enabled {
                            reenabled_renderers.push(link.renderer_id);
                        }
                    }
                }
                if changed {
                    if let Some(display) = inner.displays.get(&display_id) {
                        display.invalidate_consumption();
                    }
                    changed_displays.push(display_id);
                }
            }
            let stopped_renderers = inner
                .renderer_slots
                .keys()
                .filter(|renderer_id| {
                    let links = inner.table.links_for_renderer(renderer_id);
                    inner.manual_stopped
                        || inner.auto_effects.stop
                        || (!links.is_empty() && links.iter().all(|link| !link.enabled))
                })
                .cloned()
                .collect::<Vec<_>>();
            (changed_displays, reenabled_renderers, stopped_renderers)
        };
        changed_displays.sort_unstable();
        changed_displays.dedup();
        reenabled_renderers.sort();
        reenabled_renderers.dedup();
        stopped_renderers.sort();
        stopped_renderers.dedup();
        for renderer_id in &stopped_renderers {
            self.begin_retained_stop(renderer_id).await;
        }
        for display_id in &changed_displays {
            self.sync_display(*display_id).await;
        }
        futures_util::future::join_all(
            stopped_renderers
                .iter()
                .map(|renderer_id| self.finish_retained_stop(renderer_id)),
        )
        .await;
        futures_util::future::join_all(
            reenabled_renderers
                .iter()
                .map(|renderer_id| self.request_renderer_start(renderer_id, resume_cause)),
        )
        .await;
        if !changed_displays.is_empty() {
            self.reconcile_buffer_flags().await;
            let all = self.snapshot_displays().await;
            self.emit(RouterEvent::DisplaysReplace(all));
        }
    }

    pub async fn set_manual_stop(self: &Arc<Self>, stopped: bool) {
        let changed = {
            let mut inner = self.inner.lock().await;
            if inner.manual_stopped == stopped {
                false
            } else {
                inner.manual_stopped = stopped;
                true
            }
        };
        if changed {
            self.reconcile_assignment_activation(RendererStartCause::ManualStopResume)
                .await;
            self.reconcile_lifecycle().await;
        }
    }

    pub(super) async fn begin_retained_stop(self: &Arc<Self>, renderer_id: &str) {
        let (generation, cancelled_start) = {
            let mut inner = self.inner.lock().await;
            let Some(slot) = inner.renderer_slots.get_mut(renderer_id) else {
                return;
            };
            let cancelled_start = slot.pending_start.take().is_some();
            let generation = (slot
                .transition(RendererLifecycleEvent::StopRequested { keep: true })
                == RendererTransition::Changed)
                .then(|| slot.state.generation())
                .flatten();
            (generation, cancelled_start)
        };
        if cancelled_start {
            log::debug!("renderer {renderer_id}: cancel pending start; activation stopped");
        }
        self.deadlines
            .cancel(deadline::DeadlineKey::renderer_start(renderer_id));
        if generation.is_none() {
            return;
        }
        self.begin_unbind_ack_tracking(renderer_id).await;
        if let Some(snapshot) = self.snapshot_renderer(renderer_id).await {
            self.emit(RouterEvent::RendererUpsert(snapshot));
        }
    }

    pub(super) async fn finish_retained_stop(self: &Arc<Self>, renderer_id: &str) {
        let generation = {
            let inner = self.inner.lock().await;
            inner
                .renderer_slots
                .get(renderer_id)
                .and_then(|slot| match slot.state {
                    RendererLifecycleState::Stopping { generation, .. } => Some(generation),
                    _ => None,
                })
        };
        let Some(generation) = generation else { return };
        if self
            .await_unbind_acks_for(renderer_id, Duration::from_secs(1))
            .await
            .is_err()
        {
            log::warn!("renderer {renderer_id}: retained stop unbind acknowledgement timed out");
        }
        match self.mgr.stop_generation(renderer_id, generation).await {
            Ok(Some(exit)) => {
                self.on_renderer_process_exit(exit).await;
            }
            Ok(None) => {
                log::debug!("renderer {renderer_id}: skip stop for stale generation={generation}");
            }
            Err(crate::error::Error::RendererNotFound(_)) => {
                log::debug!(
                    "renderer {renderer_id}: generation={generation} is not registered while stopping"
                );
            }
            Err(error) => {
                log::warn!("renderer {renderer_id}: retained stop failed: {error}");
            }
        }
    }

    pub(super) async fn request_renderer_start(
        self: &Arc<Self>,
        renderer_id: &str,
        cause: RendererStartCause,
    ) -> crate::error::Result<()> {
        let now = tokio::time::Instant::now();
        let pending = {
            let mut inner = self.inner.lock().await;
            let has_demand = inner
                .table
                .links_for_renderer(renderer_id)
                .iter()
                .any(|link| link.enabled && inner.displays.contains_key(&link.display_id));
            if !has_demand {
                if let Some(slot) = inner.renderer_slots.get_mut(renderer_id) {
                    slot.pending_start = None;
                }
                None
            } else {
                let Some(slot_snapshot) = inner.renderer_slots.get(renderer_id) else {
                    return Ok(());
                };
                let existing = slot_snapshot.pending_start;
                {
                    let next_cause = cause;
                    let not_before = match cause {
                        RendererStartCause::AutoReplayResume => now + AUTO_REPLAY_START_DELAY,
                        _ => now,
                    };
                    if let Some(current) = existing {
                        if !cause.preempts_pending() {
                            match (current.cause, cause) {
                                (
                                    RendererStartCause::AutoReplayResume,
                                    RendererStartCause::AutoReplayResume,
                                ) => {
                                    log::debug!(
                                        "renderer {renderer_id}: keep pending start cause={} token={}",
                                        current.cause.as_str(),
                                        current.token
                                    );
                                    return Ok(());
                                }
                                _ => {
                                    log::debug!(
                                        "renderer {renderer_id}: coalesce {} into pending {} token={}",
                                        cause.as_str(),
                                        current.cause.as_str(),
                                        current.token
                                    );
                                    return Ok(());
                                }
                            }
                        }
                    }
                    inner.next_start_token = inner
                        .next_start_token
                        .checked_add(1)
                        .expect("renderer start token exhausted");
                    let pending = PendingRendererStart {
                        cause: next_cause,
                        not_before,
                        token: inner.next_start_token,
                    };
                    if let Some(slot) = inner.renderer_slots.get_mut(renderer_id) {
                        slot.pending_start = Some(pending);
                    }
                    Some(pending)
                }
            }
        };
        let key = deadline::DeadlineKey::renderer_start(renderer_id);
        let Some(pending) = pending else {
            self.deadlines.cancel(key);
            return Ok(());
        };
        log::debug!(
            "renderer {renderer_id}: pending start cause={} token={} wait={:?}",
            pending.cause.as_str(),
            pending.token,
            pending.not_before.saturating_duration_since(now)
        );
        self.advance_renderer_start(renderer_id, pending.token)
            .await
    }

    async fn advance_renderer_start(
        self: &Arc<Self>,
        renderer_id: &str,
        token: u64,
    ) -> crate::error::Result<()> {
        let action = {
            let mut inner = self.inner.lock().await;
            let has_demand = inner
                .table
                .links_for_renderer(renderer_id)
                .iter()
                .any(|link| link.enabled && inner.displays.contains_key(&link.display_id));
            let has_live_handle = inner.table.get_renderer(renderer_id).is_some();
            let Some(slot) = inner.renderer_slots.get(renderer_id) else {
                return Ok(());
            };
            let Some(pending) = slot.pending_start.filter(|pending| pending.token == token) else {
                return Ok(());
            };
            if !has_demand {
                if let Some(slot) = inner.renderer_slots.get_mut(renderer_id) {
                    slot.pending_start = None;
                }
                AdvanceStart::Cancel
            } else if pending.not_before > tokio::time::Instant::now() {
                AdvanceStart::Schedule(pending)
            } else if slot.state.is_running() {
                if let Some(slot) = inner.renderer_slots.get_mut(renderer_id) {
                    slot.pending_start = None;
                }
                AdvanceStart::Cancel
            } else if has_live_handle
                || matches!(
                    slot.state,
                    RendererLifecycleState::Starting { .. }
                        | RendererLifecycleState::Stopping { .. }
                )
            {
                AdvanceStart::Wait
            } else {
                let process_generation = self.mgr.reserve_process_generation();
                let slot = inner
                    .renderer_slots
                    .get_mut(renderer_id)
                    .expect("renderer slot disappeared while locked");
                if slot.transition(RendererLifecycleEvent::StartRequested {
                    generation: process_generation,
                    start_token: pending.token,
                    reactivate_failed: pending.cause.allows_failed(),
                }) != RendererTransition::Changed
                {
                    slot.pending_start = None;
                    AdvanceStart::Cancel
                } else {
                    slot.pending_start = None;
                    let effect = RendererStartEffect {
                        renderer_id: renderer_id.to_owned(),
                        process_generation,
                        spec_revision: slot.spec_revision,
                        start_token: pending.token,
                        cause: pending.cause,
                        spawn_request: slot.spawn_request.clone(),
                    };
                    let isolated = self.isolate_renderer_targets_locked(&mut inner, renderer_id);
                    AdvanceStart::Start(effect, isolated)
                }
            }
        };
        match action {
            AdvanceStart::Start(effect, isolated) => {
                self.deadlines
                    .cancel(deadline::DeadlineKey::renderer_start(renderer_id));
                for target in &isolated {
                    for display_id in &target.display_ids {
                        self.sync_display(*display_id).await;
                    }
                    if let Some(snapshot) = self.snapshot_renderer(&target.renderer_id).await {
                        self.emit(RouterEvent::RendererUpsert(snapshot));
                    }
                }
                if !isolated.is_empty() {
                    self.emit(RouterEvent::DisplaysReplace(self.snapshot_displays().await));
                }
                if let Some(snapshot) = self.snapshot_renderer(renderer_id).await {
                    self.emit(RouterEvent::RendererUpsert(snapshot));
                }
                let mut error = self.execute_renderer_start(effect).await.err();
                // Each isolated slot now owns exactly one target. Use the
                // regular start path so stopped displays and lifecycle races
                // receive the same checks as the original slot. The original
                // slot has already waited out any auto-resume deadline.
                for target in isolated {
                    if let Err(failure) = Box::pin(self.request_renderer_start(
                        &target.renderer_id,
                        RendererStartCause::DisplayReconnect,
                    ))
                    .await
                    {
                        error.get_or_insert(failure);
                    }
                }
                error.map_or(Ok(()), Err)
            }
            AdvanceStart::Schedule(pending) => {
                self.deadlines.schedule(
                    deadline::DeadlineKey::renderer_start(renderer_id),
                    pending.token,
                    pending.not_before,
                );
                Ok(())
            }
            AdvanceStart::Cancel => {
                self.deadlines
                    .cancel(deadline::DeadlineKey::renderer_start(renderer_id));
                Ok(())
            }
            AdvanceStart::Wait => Ok(()),
        }
    }

    async fn execute_renderer_start(
        self: &Arc<Self>,
        effect: RendererStartEffect,
    ) -> crate::error::Result<()> {
        let renderer_id = effect.renderer_id.clone();
        log::info!(
            "renderer {renderer_id}: start cause={} generation={}",
            effect.cause.as_str(),
            effect.process_generation
        );
        match self
            .mgr
            .spawn_for_generation(
                renderer_id.clone(),
                effect.process_generation,
                effect.spawn_request,
            )
            .await
        {
            Ok(()) => {
                let Some(handle) = self.mgr.get(&renderer_id).await else {
                    return Ok(());
                };
                if !self
                    .register_renderer_current(
                        handle,
                        Some((
                            effect.spec_revision,
                            effect.process_generation,
                            effect.start_token,
                        )),
                    )
                    .await
                {
                    let tracked = {
                        let mut inner = self.inner.lock().await;
                        let no_live_handle = inner.table.get_renderer(&renderer_id).is_none();
                        inner
                            .renderer_slots
                            .get_mut(&renderer_id)
                            .filter(|_| no_live_handle)
                            .is_some_and(|slot| {
                                if matches!(
                                    slot.state,
                                    RendererLifecycleState::Starting { generation }
                                        if generation == effect.process_generation
                                ) {
                                    let _ =
                                        slot.transition(RendererLifecycleEvent::StopRequested {
                                            keep: true,
                                        });
                                }
                                matches!(
                                    slot.state,
                                    RendererLifecycleState::Stopping { generation, .. }
                                        if generation == effect.process_generation
                                )
                            })
                    };
                    match self
                        .mgr
                        .stop_generation(&renderer_id, effect.process_generation)
                        .await
                    {
                        Ok(Some(exit)) if tracked => {
                            self.settle_renderer_process_exit(exit).await;
                            Box::pin(self.resume_renderer_after_exit(&renderer_id)).await;
                        }
                        Ok(Some(exit)) => log::debug!(
                            "renderer {renderer_id}: discarded stale spawned generation {}",
                            exit.process_generation
                        ),
                        Ok(None) | Err(crate::error::Error::RendererNotFound(_)) => {}
                        Err(error) => log::warn!(
                            "renderer {renderer_id}: stale generation cleanup failed: {error}"
                        ),
                    }
                    return Ok(());
                }
                let displays = {
                    let inner = self.inner.lock().await;
                    inner
                        .table
                        .links_for_renderer(&renderer_id)
                        .into_iter()
                        .filter(|link| link.enabled)
                        .map(|link| link.display_id)
                        .collect::<Vec<_>>()
                };
                for display_id in displays {
                    self.sync_display(display_id).await;
                }
                self.reconcile_buffer_flags().await;
                if let Some(snapshot) = self.snapshot_renderer(&renderer_id).await {
                    self.emit(RouterEvent::RendererUpsert(snapshot));
                }
                Ok(())
            }
            Err(error) => {
                log::warn!("renderer {renderer_id}: retained start failed: {error}");
                let remove = {
                    let mut inner = self.inner.lock().await;
                    if let Some(slot) = inner.renderer_slots.get_mut(&renderer_id) {
                        if slot.active_start_token != Some(effect.start_token) {
                            false
                        } else {
                            slot.transition(RendererLifecycleEvent::SpawnFailed {
                                generation: effect.process_generation,
                                failure: RendererExitSnapshot {
                                    code: None,
                                    signal: None,
                                    reason: error.to_string(),
                                },
                            }) == RendererTransition::Remove
                        }
                    } else {
                        false
                    }
                };
                if remove {
                    self.unregister_renderer(&renderer_id).await;
                } else if let Some(snapshot) = self.snapshot_renderer(&renderer_id).await {
                    self.emit(RouterEvent::RendererUpsert(snapshot));
                }
                Err(error)
            }
        }
    }

    pub(super) async fn resume_renderer_after_exit(self: &Arc<Self>, renderer_id: &str) {
        let pending = {
            let inner = self.inner.lock().await;
            let Some(slot) = inner.renderer_slots.get(renderer_id) else {
                return;
            };
            slot.pending_start
        };
        if let Some(pending) = pending {
            let _ = self
                .advance_renderer_start(renderer_id, pending.token)
                .await;
        }
    }

    pub(super) async fn on_deadline_reached(self: &Arc<Self>, event: deadline::DeadlineReached) {
        match event.key {
            deadline::DeadlineKey::RendererStart(renderer_id) => {
                let _ = self.advance_renderer_start(&renderer_id, event.token).await;
            }
            deadline::DeadlineKey::AutoReplayResume(display_id) => {
                self.finish_auto_replay_resume(display_id, event.token)
                    .await;
            }
        }
    }

    pub async fn set_manual_pause(self: &Arc<Self>, paused: bool) {
        let changed = {
            let mut inner = self.inner.lock().await;
            if inner.manual_paused == paused {
                false
            } else {
                inner.manual_paused = paused;
                true
            }
        };
        if changed {
            self.reconcile_lifecycle().await;
        }
    }

    pub async fn toggle_manual_pause(self: &Arc<Self>) -> bool {
        let paused = {
            let mut inner = self.inner.lock().await;
            inner.manual_paused = !inner.manual_paused;
            inner.manual_paused
        };
        self.reconcile_lifecycle().await;
        paused
    }

    pub async fn set_manual_mute(self: &Arc<Self>, muted: bool) {
        let changed = {
            let mut inner = self.inner.lock().await;
            if inner.manual_muted == muted {
                false
            } else {
                inner.manual_muted = muted;
                true
            }
        };
        if changed {
            self.reconcile_lifecycle().await;
        }
    }

    pub async fn toggle_manual_mute(self: &Arc<Self>) -> bool {
        let muted = {
            let mut inner = self.inner.lock().await;
            inner.manual_muted = !inner.manual_muted;
            inner.manual_muted
        };
        self.reconcile_lifecycle().await;
        muted
    }

    pub async fn set_other_playback_active(self: &Arc<Self>, active: bool) {
        let changed = {
            let mut inner = self.inner.lock().await;
            if inner.other_playback_active == active {
                false
            } else {
                inner.other_playback_active = active;
                true
            }
        };
        if changed {
            self.reconcile_lifecycle().await;
        }
    }

    pub async fn manual_lifecycle_state(self: &Arc<Self>) -> ManualLifecycleState {
        let inner = self.inner.lock().await;
        ManualLifecycleState {
            paused: inner.manual_paused,
            muted: inner.manual_muted,
            stopped: inner.manual_stopped,
        }
    }

    /// Whether this renderer's effective commanded activity is paused.
    /// Returns `false` for unknown ids.
    pub async fn is_paused(self: &Arc<Self>, renderer_id: &str) -> bool {
        self.inner
            .lock()
            .await
            .renderer_slots
            .get(renderer_id)
            .is_some_and(|slot| slot.state.activity() == Some(RendererActivity::Paused))
    }

    pub async fn is_muted(self: &Arc<Self>, renderer_id: &str) -> bool {
        self.inner
            .lock()
            .await
            .renderer_slots
            .get(renderer_id)
            .is_some_and(|slot| slot.state.activity() == Some(RendererActivity::Muted))
    }
}
