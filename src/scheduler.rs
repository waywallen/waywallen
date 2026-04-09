//! Display scheduler — tracks registered display clients, fans frames
//! out to them, and aggregates release signals back to the renderer.
//!
//! Phase 1 policy is intentionally minimal:
//!
//!   - Every display that registers with the daemon is automatically
//!     bound to the single currently-active renderer.
//!   - The projected `SetConfig` is always identity: `source_rect`
//!     covers the entire renderer texture, `dest_rect` covers the
//!     entire display, no transform, opaque black clear color.
//!
//! Later work will extend this with: per-display crop/stretch
//! policies, multiple active renderers, layout managers, etc.
//!
//! The scheduler is a plain struct — the endpoint code is expected to
//! wrap it in `Arc<Mutex<Scheduler>>` (plain `std::sync::Mutex`; locks
//! are never held across `.await`).

use std::collections::HashMap;

/// Unique id handed out by [`Scheduler::register_display`]. Monotonic
/// across the daemon's lifetime, never reused.
pub type DisplayId = u64;

/// Per-display bookkeeping visible to the scheduler.
#[derive(Debug, Clone)]
pub struct DisplayInfo {
    pub id: DisplayId,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
    pub properties: Vec<(String, String)>,
    /// Has the scheduler sent (or is about to send) `bind_buffers` to
    /// this display for the current `active_renderer` buffer pool?
    pub bound: bool,
}

/// Compact description of the renderer-side buffer pool this
/// scheduler fans out to clients. Mirrors the `BindBuffers` event on
/// the wire but with the texture-space metadata only; the actual
/// `dma_buf` FDs live in the renderer manager's `BindSnapshot` and
/// are dup'd per-client at broadcast time, not kept here.
#[derive(Debug, Clone)]
pub struct ActiveBinding {
    pub renderer_id: String,
    pub buffer_generation: u64,
    pub tex_width: u32,
    pub tex_height: u32,
}

/// Identity SetConfig body derived from an `ActiveBinding` + a display's
/// advertised size. Used to build the wire-level `set_config` event.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectedConfig {
    pub config_generation: u64,
    pub source_x: f32,
    pub source_y: f32,
    pub source_w: f32,
    pub source_h: f32,
    pub dest_x: f32,
    pub dest_y: f32,
    pub dest_w: f32,
    pub dest_h: f32,
    pub transform: u32,
    pub clear_rgba: [f32; 4],
}

/// Outcome of a [`Scheduler::release_frame`] call. The manager uses
/// this to decide whether it can tell the renderer a buffer slot is
/// free for reuse.
#[derive(Debug, PartialEq, Eq)]
pub enum ReleaseOutcome {
    /// Some other display still has this frame checked out. Do nothing.
    StillInFlight,
    /// Last reference just dropped. The manager should notify the
    /// renderer that `(buffer_generation, buffer_index, seq)` is now
    /// free for reuse.
    AllReleased,
    /// The frame's generation has already been retired (a new
    /// `bind_buffers` superseded it). The release is harmless — drop
    /// it silently.
    Stale,
    /// No matching outstanding frame; usually a protocol-violating
    /// duplicate release from a misbehaving client.
    Unknown,
}

/// Key that identifies an in-flight frame waiting on display releases.
/// (buffer_generation is part of the key so releases from retired
/// generations can be detected and discarded cleanly.)
type FrameKey = (u64, u32, u64); // (buffer_generation, buffer_index, seq)

#[derive(Debug, Default)]
pub struct Scheduler {
    next_display_id: u64,
    next_config_generation: u64,
    displays: HashMap<DisplayId, DisplayInfo>,
    active: Option<ActiveBinding>,
    /// Outstanding frames keyed by (gen, idx, seq). For each, records
    /// the set of display ids that still owe a `buffer_release`.
    pending: HashMap<FrameKey, std::collections::HashSet<DisplayId>>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self::default()
    }

    // ---------------------------------------------------------------
    // Display registration
    // ---------------------------------------------------------------

    /// Register a display client and return its assigned opaque id.
    pub fn register_display(
        &mut self,
        name: String,
        width: u32,
        height: u32,
        refresh_mhz: u32,
        properties: Vec<(String, String)>,
    ) -> DisplayId {
        self.next_display_id += 1;
        let id = self.next_display_id;
        self.displays.insert(
            id,
            DisplayInfo {
                id,
                name,
                width,
                height,
                refresh_mhz,
                properties,
                // Phase 1 policy: bound implicitly as soon as an active
                // renderer exists. If we register before any renderer
                // is active, `bound` is false and we'll flip it when
                // `set_active_binding` is called.
                bound: self.active.is_some(),
            },
        );
        id
    }

    /// Remove a display from the scheduler and drop any pending frame
    /// references it held. Returns a vector of `(buffer_gen, idx, seq)`
    /// frames that just became releasable as a result — the caller
    /// should notify the renderer for each.
    pub fn unregister_display(&mut self, id: DisplayId) -> Vec<FrameKey> {
        self.displays.remove(&id);
        let mut freed = Vec::new();
        self.pending.retain(|key, owed| {
            owed.remove(&id);
            if owed.is_empty() {
                freed.push(*key);
                false
            } else {
                true
            }
        });
        freed
    }

    pub fn get_display(&self, id: DisplayId) -> Option<&DisplayInfo> {
        self.displays.get(&id)
    }

    /// Let a display tell the scheduler its size changed. Phase 1 does
    /// not re-compute SetConfig on its own — that happens when the
    /// next frame is scheduled — so this is a pure write.
    pub fn update_display_size(&mut self, id: DisplayId, width: u32, height: u32) {
        if let Some(d) = self.displays.get_mut(&id) {
            d.width = width;
            d.height = height;
        }
    }

    // ---------------------------------------------------------------
    // Renderer binding
    // ---------------------------------------------------------------

    /// Record that a renderer has published a new buffer pool. Returns
    /// the list of display ids that the manager should now send a
    /// `bind_buffers` + `set_config` to. (Caller is responsible for
    /// actually constructing and dispatching the wire events.)
    pub fn set_active_binding(&mut self, binding: ActiveBinding) -> Vec<DisplayId> {
        // Advancing to a new buffer_generation retires every pending
        // frame for the previous generation — those frames no longer
        // need release aggregation (the old fds are gone).
        if let Some(old) = &self.active {
            if old.buffer_generation != binding.buffer_generation {
                self.pending.retain(|(gen, _, _), _| *gen == binding.buffer_generation);
            }
        }
        self.active = Some(binding);
        let mut ids: Vec<DisplayId> = self.displays.keys().copied().collect();
        ids.sort_unstable();
        for d in self.displays.values_mut() {
            d.bound = true;
        }
        ids
    }

    /// Clear the active binding (renderer disappeared). The caller
    /// should emit `unbind` events to the listed displays.
    pub fn clear_active_binding(&mut self) -> Vec<DisplayId> {
        self.active = None;
        self.pending.clear();
        let mut ids: Vec<DisplayId> = self.displays.keys().copied().collect();
        ids.sort_unstable();
        for d in self.displays.values_mut() {
            d.bound = false;
        }
        ids
    }

    pub fn active_binding(&self) -> Option<&ActiveBinding> {
        self.active.as_ref()
    }

    /// Compute the identity SetConfig for a given display under the
    /// current active binding. Returns `None` if no renderer is bound
    /// or the display is unknown. Each call bumps the scheduler's
    /// `config_generation` counter so distinct SetConfig events can be
    /// told apart on the wire.
    pub fn project_config(&mut self, display_id: DisplayId) -> Option<ProjectedConfig> {
        let active = self.active.as_ref()?;
        let disp = self.displays.get(&display_id)?;
        self.next_config_generation += 1;
        Some(ProjectedConfig {
            config_generation: self.next_config_generation,
            source_x: 0.0,
            source_y: 0.0,
            source_w: active.tex_width as f32,
            source_h: active.tex_height as f32,
            dest_x: 0.0,
            dest_y: 0.0,
            dest_w: disp.width as f32,
            dest_h: disp.height as f32,
            transform: 0,
            clear_rgba: [0.0, 0.0, 0.0, 1.0],
        })
    }

    // ---------------------------------------------------------------
    // Frame fan-out & release aggregation
    // ---------------------------------------------------------------

    /// Called when the renderer produces a frame for `(gen, idx, seq)`.
    /// Installs a pending refcount entry covering every currently bound
    /// display and returns their ids so the caller can fan out the
    /// `frame_ready` event. Returns an empty vec if the frame is for a
    /// generation that does not match the current active binding
    /// (stale / racy: the caller should drop it).
    pub fn begin_frame(
        &mut self,
        buffer_generation: u64,
        buffer_index: u32,
        seq: u64,
    ) -> Vec<DisplayId> {
        let Some(active) = &self.active else {
            return Vec::new();
        };
        if active.buffer_generation != buffer_generation {
            return Vec::new();
        }
        let bound: std::collections::HashSet<DisplayId> = self
            .displays
            .iter()
            .filter(|(_, d)| d.bound)
            .map(|(id, _)| *id)
            .collect();
        if bound.is_empty() {
            return Vec::new();
        }
        let key: FrameKey = (buffer_generation, buffer_index, seq);
        self.pending.insert(key, bound.clone());
        let mut out: Vec<DisplayId> = bound.into_iter().collect();
        out.sort_unstable();
        out
    }

    /// Called when a display sends `buffer_release`. Returns whether
    /// the frame is now fully released, still in flight, already
    /// retired, or never existed.
    pub fn release_frame(
        &mut self,
        display_id: DisplayId,
        buffer_generation: u64,
        buffer_index: u32,
        seq: u64,
    ) -> ReleaseOutcome {
        let key: FrameKey = (buffer_generation, buffer_index, seq);

        // Detect "already retired by a newer bind_buffers".
        if let Some(active) = &self.active {
            if active.buffer_generation != buffer_generation {
                return ReleaseOutcome::Stale;
            }
        } else {
            return ReleaseOutcome::Stale;
        }

        let Some(owed) = self.pending.get_mut(&key) else {
            return ReleaseOutcome::Unknown;
        };
        if !owed.remove(&display_id) {
            return ReleaseOutcome::Unknown;
        }
        if owed.is_empty() {
            self.pending.remove(&key);
            ReleaseOutcome::AllReleased
        } else {
            ReleaseOutcome::StillInFlight
        }
    }

    #[cfg(test)]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Scheduler {
        Scheduler::new()
    }

    fn mk_binding(gen: u64) -> ActiveBinding {
        ActiveBinding {
            renderer_id: "r".into(),
            buffer_generation: gen,
            tex_width: 1920,
            tex_height: 1080,
        }
    }

    #[test]
    fn register_assigns_monotonic_ids() {
        let mut s = fresh();
        let a = s.register_display("a".into(), 800, 600, 60000, vec![]);
        let b = s.register_display("b".into(), 800, 600, 60000, vec![]);
        assert_eq!(a, 1);
        assert_eq!(b, 2);
        assert_ne!(a, b);
    }

    #[test]
    fn set_binding_marks_all_displays_bound() {
        let mut s = fresh();
        let d1 = s.register_display("d1".into(), 800, 600, 0, vec![]);
        let d2 = s.register_display("d2".into(), 800, 600, 0, vec![]);
        let ids = s.set_active_binding(mk_binding(1));
        assert_eq!(ids, vec![d1, d2]);
        assert!(s.get_display(d1).unwrap().bound);
        assert!(s.get_display(d2).unwrap().bound);
    }

    #[test]
    fn project_config_identity() {
        let mut s = fresh();
        let id = s.register_display("d".into(), 1280, 720, 0, vec![]);
        s.set_active_binding(mk_binding(1));
        let cfg = s.project_config(id).unwrap();
        assert_eq!(cfg.source_w, 1920.0);
        assert_eq!(cfg.source_h, 1080.0);
        assert_eq!(cfg.dest_w, 1280.0);
        assert_eq!(cfg.dest_h, 720.0);
        assert_eq!(cfg.transform, 0);
        assert_eq!(cfg.clear_rgba, [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn config_generation_is_monotonic() {
        let mut s = fresh();
        let id = s.register_display("d".into(), 100, 100, 0, vec![]);
        s.set_active_binding(mk_binding(1));
        let a = s.project_config(id).unwrap().config_generation;
        let b = s.project_config(id).unwrap().config_generation;
        assert!(b > a);
    }

    #[test]
    fn frame_fanout_k1_release_once() {
        let mut s = fresh();
        let d = s.register_display("d".into(), 100, 100, 0, vec![]);
        s.set_active_binding(mk_binding(1));

        let fanout = s.begin_frame(1, 0, 10);
        assert_eq!(fanout, vec![d]);
        assert_eq!(s.pending_count(), 1);

        let out = s.release_frame(d, 1, 0, 10);
        assert_eq!(out, ReleaseOutcome::AllReleased);
        assert_eq!(s.pending_count(), 0);
    }

    #[test]
    fn frame_fanout_k2_requires_both_releases() {
        let mut s = fresh();
        let a = s.register_display("a".into(), 100, 100, 0, vec![]);
        let b = s.register_display("b".into(), 100, 100, 0, vec![]);
        s.set_active_binding(mk_binding(1));

        let fanout = s.begin_frame(1, 0, 10);
        assert_eq!(fanout.len(), 2);
        assert!(fanout.contains(&a) && fanout.contains(&b));

        assert_eq!(s.release_frame(a, 1, 0, 10), ReleaseOutcome::StillInFlight);
        assert_eq!(s.pending_count(), 1);
        assert_eq!(s.release_frame(b, 1, 0, 10), ReleaseOutcome::AllReleased);
        assert_eq!(s.pending_count(), 0);
    }

    #[test]
    fn frame_fanout_k3_requires_all_three_releases() {
        let mut s = fresh();
        let a = s.register_display("a".into(), 100, 100, 0, vec![]);
        let b = s.register_display("b".into(), 100, 100, 0, vec![]);
        let c = s.register_display("c".into(), 100, 100, 0, vec![]);
        s.set_active_binding(mk_binding(1));

        let fanout = s.begin_frame(1, 0, 10);
        assert_eq!(fanout.len(), 3);

        assert_eq!(s.release_frame(a, 1, 0, 10), ReleaseOutcome::StillInFlight);
        assert_eq!(s.release_frame(c, 1, 0, 10), ReleaseOutcome::StillInFlight);
        assert_eq!(s.release_frame(b, 1, 0, 10), ReleaseOutcome::AllReleased);
    }

    #[test]
    fn release_from_unknown_display_is_unknown() {
        let mut s = fresh();
        let a = s.register_display("a".into(), 100, 100, 0, vec![]);
        s.set_active_binding(mk_binding(1));
        let _ = s.begin_frame(1, 0, 10);

        // Using an ID that never registered.
        assert_eq!(s.release_frame(999, 1, 0, 10), ReleaseOutcome::Unknown);
        // Real release still completes the frame.
        assert_eq!(s.release_frame(a, 1, 0, 10), ReleaseOutcome::AllReleased);
    }

    #[test]
    fn release_after_rebind_is_stale() {
        let mut s = fresh();
        let a = s.register_display("a".into(), 100, 100, 0, vec![]);
        s.set_active_binding(mk_binding(1));
        let _ = s.begin_frame(1, 0, 10);

        // New generation retires the old frame implicitly.
        s.set_active_binding(mk_binding(2));
        assert_eq!(s.release_frame(a, 1, 0, 10), ReleaseOutcome::Stale);
    }

    #[test]
    fn double_release_is_unknown() {
        let mut s = fresh();
        let a = s.register_display("a".into(), 100, 100, 0, vec![]);
        s.set_active_binding(mk_binding(1));
        let _ = s.begin_frame(1, 0, 10);
        assert_eq!(s.release_frame(a, 1, 0, 10), ReleaseOutcome::AllReleased);
        assert_eq!(s.release_frame(a, 1, 0, 10), ReleaseOutcome::Unknown);
    }

    #[test]
    fn unregister_releases_held_refcounts() {
        let mut s = fresh();
        let a = s.register_display("a".into(), 100, 100, 0, vec![]);
        let b = s.register_display("b".into(), 100, 100, 0, vec![]);
        s.set_active_binding(mk_binding(1));
        let _ = s.begin_frame(1, 0, 10);
        let _ = s.begin_frame(1, 1, 11);

        // a goes away — both frames should become complete (b already
        // released nothing, so a's removal drops refcount to 1, not 0).
        // Wait, that's wrong: a and b both owe each frame. Removing a
        // alone leaves b still owing. unregister returns the frames
        // that went to refcount 0, which here is none.
        let freed = s.unregister_display(a);
        assert!(freed.is_empty(), "b still owes");
        // Now b releases both — those should complete.
        assert_eq!(s.release_frame(b, 1, 0, 10), ReleaseOutcome::AllReleased);
        assert_eq!(s.release_frame(b, 1, 1, 11), ReleaseOutcome::AllReleased);
    }

    #[test]
    fn unregister_last_holder_frees_frame() {
        let mut s = fresh();
        let a = s.register_display("a".into(), 100, 100, 0, vec![]);
        s.set_active_binding(mk_binding(1));
        let _ = s.begin_frame(1, 0, 10);

        let freed = s.unregister_display(a);
        assert_eq!(freed, vec![(1, 0, 10)]);
        assert_eq!(s.pending_count(), 0);
    }

    #[test]
    fn begin_frame_with_no_bound_displays_is_noop() {
        let mut s = fresh();
        s.set_active_binding(mk_binding(1));
        assert!(s.begin_frame(1, 0, 10).is_empty());
        assert_eq!(s.pending_count(), 0);
    }

    #[test]
    fn begin_frame_wrong_generation_is_noop() {
        let mut s = fresh();
        let _ = s.register_display("a".into(), 100, 100, 0, vec![]);
        s.set_active_binding(mk_binding(2));
        // Renderer raced and emitted a frame for the old generation.
        assert!(s.begin_frame(1, 0, 10).is_empty());
    }
}
