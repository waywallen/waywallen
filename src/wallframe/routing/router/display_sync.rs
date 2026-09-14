use super::*;

impl Router {
    /// Bring `display_id`'s sent state in line with its current link
    /// target (renderer + generation). Idempotent.
    pub(super) async fn sync_display(self: &Arc<Self>, display_id: DisplayId) {
        let mut inner = self.inner.lock().await;
        if !inner.displays.contains_key(&display_id) {
            return;
        }
        // Capture one exact publication for the complete display decision.
        let display_links = inner.table.links_for_display(display_id);
        debug_assert!(
            display_links.iter().filter(|l| l.enabled).count() <= 1,
            "display {display_id} has multiple enabled links — invariant violated"
        );
        let target: Option<(Link, Arc<RendererHandle>, Arc<PublishedPool>)> =
            display_links.into_iter().find(|l| l.enabled).and_then(|l| {
                let renderer = inner.table.get_renderer(&l.renderer_id)?;
                let pool = renderer.published_pool()?;
                Some((l, renderer, pool))
            });

        // When both producer and consumer have caps, only bind a snapshot
        // that satisfies the last negotiated scheme.
        if let Some((_, ref renderer, ref pool)) = target {
            if renderer.format_caps().is_some() && !renderer.scheme_satisfied_by(pool) {
                log::debug!(
                    "router: sync_display({display_id}) gated — renderer {} \
                     published pool does not match last-dispatched scheme",
                    renderer.id
                );
                return;
            }
        }

        // Snapshot what was last sent.
        let (last_binding, info) = {
            let s = inner.displays.get(&display_id).unwrap();
            (s.binding.clone(), s.info.clone())
        };

        let needs_update = match (&last_binding, &target) {
            (Some(old), Some((link, _, pool))) => {
                old.renderer.id != link.renderer_id || old.pool.generation != pool.generation
            }
            (None, None) => false,
            _ => true,
        };
        if !needs_update {
            return;
        }

        inner
            .displays
            .get(&display_id)
            .expect("display checked above")
            .invalidate_consumption();

        // Retire the prior pool if one was bound.
        if let Some(old) = last_binding.as_ref() {
            let s = inner.displays.get(&display_id).unwrap();
            let _ = s.tx.send(DisplayOutEvent::Unbind {
                buffer_generation: old.wire_generation,
            });
            // If the OLD renderer is currently being torn down with
            // ack tracking active, record this unbind as pending.
            if let Some(pending) = inner.unbind_acks_pending.get_mut(&old.renderer.id) {
                pending.insert((display_id, old.wire_generation));
            }
        }

        // Bind the new pool if a target renderer is ready.
        if let Some((link, renderer, pool)) = target {
            inner.next_config_generation += 1;
            let cfg_gen = inner.next_config_generation;
            let layout = self.resolved_layout_for_renderer(&info, &link.renderer_id, &inner);
            let replay = renderer
                .wp_type
                .eq_ignore_ascii_case("image")
                .then(|| renderer.latest_frame())
                .flatten()
                .filter(|frame| frame.buffer_generation == pool.generation);
            // A new pool from the same renderer spec (resize, renegotiation,
            // restart) keeps showing the same wallpaper and binds without
            // a transition.
            let content = (
                link.renderer_id.clone(),
                inner
                    .renderer_slots
                    .get(&link.renderer_id)
                    .map_or(0, |slot| slot.spec_revision),
            );
            let s = inner.displays.get_mut(&display_id).unwrap();
            let transition = s.presentation.config.transition.kind != TransitionKind::None
                && s.presented_content
                    .as_ref()
                    .is_some_and(|presented| *presented != content);
            s.presented_content = Some(content);
            s.next_wire_buffer_generation = s
                .next_wire_buffer_generation
                .checked_add(1)
                .expect("display buffer generation exhausted");
            let wire_generation = s.next_wire_buffer_generation;
            let cfg = project_link(&link, &pool, &info, cfg_gen, wire_generation, &layout);
            let _ = s.tx.send(DisplayOutEvent::Bind {
                renderer: renderer.clone(),
                pool: Arc::clone(&pool),
                buffer_generation: wire_generation,
                initial_config: cfg,
                transition,
            });
            if let Some(frame) = replay {
                let _ = s.tx.send(DisplayOutEvent::Frame {
                    renderer: renderer.clone(),
                    buffer_generation: wire_generation,
                    buffer_index: frame.buffer_index,
                    seq: frame.seq,
                    consumption: s.consumption_permit(),
                    member: None,
                });
            }
            s.binding = Some(DisplayBinding {
                renderer,
                pool,
                wire_generation,
            });
            s.failed_binding_generation = None;
        } else {
            let s = inner.displays.get_mut(&display_id).unwrap();
            s.binding = None;
        }
    }
}
