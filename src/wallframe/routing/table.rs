use std::collections::HashMap;
use std::sync::Arc;

use crate::wallframe::renderer_manager::{RendererHandle, RendererId};
use crate::wallframe::scheduler::DisplayId;

pub type LinkId = u64;

/// Source rectangle in renderer-texture pixel space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinkSrcRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Destination rectangle in display pixel space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinkDstRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Sentinel for "use the renderer's full texture / display's full
/// surface". The router resolves this at sync_display time.
pub const FULL_SRC: LinkSrcRect = LinkSrcRect {
    x: 0.0,
    y: 0.0,
    w: f32::INFINITY,
    h: f32::INFINITY,
};
pub const FULL_DST: LinkDstRect = LinkDstRect {
    x: 0.0,
    y: 0.0,
    w: f32::INFINITY,
    h: f32::INFINITY,
};

#[derive(Debug, Clone, PartialEq)]
pub enum LinkProjection {
    Independent,
    Canvas {
        canvas_id: String,
        extent: crate::wallframe::display::placement::CanvasRect,
        member: crate::wallframe::display::placement::CanvasRect,
        layout: crate::settings::ResolvedLayout,
    },
}

/// A single renderer-to-display routing edge.
/// Geometry and ordering are stored per link.
#[derive(Debug, Clone)]
pub struct Link {
    pub id: LinkId,
    pub renderer_id: RendererId,
    pub display_id: DisplayId,
    pub enabled: bool,
    pub projection: LinkProjection,
    /// Source rect in renderer texture space (use `FULL_SRC` for identity).
    pub src_rect: LinkSrcRect,
    /// Destination rect in display surface space (use `FULL_DST` for identity).
    pub dst_rect: LinkDstRect,
    /// `wl_output.transform` value: 0=normal, 1=90, 2=180, 3=270, 4..=flipped.
    pub transform: u32,
    /// Background clear color (RGBA, 0..=1).
    pub clear_rgba: [f32; 4],
    /// Z-order for composition; higher values are on top.
    pub z_order: i32,
}

#[derive(Default)]
pub struct RoutingTable {
    next_link_id: LinkId,
    renderers: HashMap<RendererId, Arc<RendererHandle>>,
    links: HashMap<LinkId, Link>,
    by_display: HashMap<DisplayId, Vec<LinkId>>,
    by_renderer: HashMap<RendererId, Vec<LinkId>>,
}

impl RoutingTable {
    pub fn new() -> Self {
        Self::default()
    }

    // ---------------------------------------------------------------
    // Renderers

    pub fn add_renderer(&mut self, handle: Arc<RendererHandle>) {
        let id = handle.id.clone();
        self.renderers.insert(id, handle);
    }

    pub fn detach_renderer(&mut self, id: &str) -> Option<Arc<RendererHandle>> {
        self.renderers.remove(id)
    }

    /// Remove a renderer and the links pointing at it. Returns the
    /// (link_id, display_id) pairs that were removed so the caller can
    pub fn remove_renderer(&mut self, id: &str) -> Vec<(LinkId, DisplayId)> {
        self.renderers.remove(id);
        let link_ids = self.by_renderer.remove(id).unwrap_or_default();
        let mut out = Vec::with_capacity(link_ids.len());
        for lid in link_ids {
            if let Some(link) = self.links.remove(&lid) {
                if let Some(v) = self.by_display.get_mut(&link.display_id) {
                    v.retain(|x| *x != lid);
                }
                out.push((lid, link.display_id));
            }
        }
        out
    }

    pub fn get_renderer(&self, id: &str) -> Option<Arc<RendererHandle>> {
        self.renderers.get(id).cloned()
    }

    pub fn renderer_ids(&self) -> Vec<RendererId> {
        self.renderers.keys().cloned().collect()
    }

    /// Pick a deterministic renderer to seed a new display's initial link.
    pub fn first_renderer(&self) -> Option<RendererId> {
        let mut ids: Vec<&RendererId> = self.renderers.keys().collect();
        ids.sort();
        ids.into_iter().next().cloned()
    }

    // ---------------------------------------------------------------
    // Links

    /// Add a `(renderer → display)` link and return its id. Enforces
    /// the single-wallpaper invariant by *first* deleting any prior
    pub fn add_link(&mut self, renderer_id: RendererId, display_id: DisplayId) -> LinkId {
        self.add_link_with_enabled(renderer_id, display_id, true)
    }

    pub fn add_link_with_enabled(
        &mut self,
        renderer_id: RendererId,
        display_id: DisplayId,
        enabled: bool,
    ) -> LinkId {
        self.add_link_with_projection(
            renderer_id,
            display_id,
            enabled,
            LinkProjection::Independent,
        )
    }

    pub fn add_link_with_projection(
        &mut self,
        renderer_id: RendererId,
        display_id: DisplayId,
        enabled: bool,
        projection: LinkProjection,
    ) -> LinkId {
        // Delete pre-existing links for this display so it has exactly
        // one assigned renderer in the current routing model.
        let existing: Vec<LinkId> = self
            .by_display
            .get(&display_id)
            .map(|v| v.clone())
            .unwrap_or_default();
        for old in existing {
            let _ = self.remove_link(old);
        }

        let clear_rgba = self
            .renderers
            .get(&renderer_id)
            .map(|renderer| renderer.clear_rgba())
            .unwrap_or([0.0, 0.0, 0.0, 1.0]);
        self.next_link_id += 1;
        let id = self.next_link_id;
        let link = Link {
            id,
            renderer_id: renderer_id.clone(),
            display_id,
            enabled,
            projection,
            src_rect: FULL_SRC,
            dst_rect: FULL_DST,
            transform: 0,
            clear_rgba,
            z_order: 0,
        };
        self.links.insert(id, link);
        self.by_display.entry(display_id).or_default().push(id);
        self.by_renderer.entry(renderer_id).or_default().push(id);
        id
    }

    /// Mutate a link's geometry/clear color in place. Returns `true`
    /// iff the link existed and any field changed.
    pub fn update_link_geometry(
        &mut self,
        link_id: LinkId,
        src: Option<LinkSrcRect>,
        dst: Option<LinkDstRect>,
        transform: Option<u32>,
        clear_rgba: Option<[f32; 4]>,
        z_order: Option<i32>,
    ) -> bool {
        let Some(link) = self.links.get_mut(&link_id) else {
            return false;
        };
        let mut changed = false;
        if src.is_some() || dst.is_some() || transform.is_some() {
            if link.projection != LinkProjection::Independent {
                link.projection = LinkProjection::Independent;
                changed = true;
            }
        }
        if let Some(v) = src {
            if link.src_rect != v {
                link.src_rect = v;
                changed = true;
            }
        }
        if let Some(v) = dst {
            if link.dst_rect != v {
                link.dst_rect = v;
                changed = true;
            }
        }
        if let Some(v) = transform {
            if link.transform != v {
                link.transform = v;
                changed = true;
            }
        }
        if let Some(v) = clear_rgba {
            if link.clear_rgba != v {
                link.clear_rgba = v;
                changed = true;
            }
        }
        if let Some(v) = z_order {
            if link.z_order != v {
                link.z_order = v;
                changed = true;
            }
        }
        changed
    }

    pub fn update_canvas_projection(
        &mut self,
        link_id: LinkId,
        canvas_id: String,
        extent: crate::wallframe::display::placement::CanvasRect,
        member: crate::wallframe::display::placement::CanvasRect,
        layout: crate::settings::ResolvedLayout,
    ) -> bool {
        let Some(link) = self.links.get_mut(&link_id) else {
            return false;
        };
        let projection = LinkProjection::Canvas {
            canvas_id,
            extent,
            member,
            layout,
        };
        if link.projection == projection {
            return false;
        }
        link.projection = projection;
        true
    }

    pub fn set_projection_independent(&mut self, link_id: LinkId) -> bool {
        let Some(link) = self.links.get_mut(&link_id) else {
            return false;
        };
        if link.projection == LinkProjection::Independent {
            return false;
        }
        link.projection = LinkProjection::Independent;
        link.src_rect = FULL_SRC;
        link.dst_rect = FULL_DST;
        true
    }

    pub fn all_links(&self) -> Vec<Link> {
        self.links.values().cloned().collect()
    }

    pub fn get_link(&self, link_id: LinkId) -> Option<&Link> {
        self.links.get(&link_id)
    }

    pub fn set_link_enabled(&mut self, link_id: LinkId, enabled: bool) -> bool {
        let Some(link) = self.links.get_mut(&link_id) else {
            return false;
        };
        if link.enabled == enabled {
            return false;
        }
        link.enabled = enabled;
        true
    }

    /// Move every link owned by `old_renderer_id` to `new_renderer_id`
    /// without changing link identity, geometry, enabled state, or ordering.
    pub fn retarget_renderer_links(
        &mut self,
        old_renderer_id: &str,
        new_renderer_id: &str,
    ) -> Vec<DisplayId> {
        if old_renderer_id == new_renderer_id {
            return self
                .links_for_renderer(old_renderer_id)
                .into_iter()
                .map(|link| link.display_id)
                .collect();
        }
        let link_ids = self.by_renderer.remove(old_renderer_id).unwrap_or_default();
        let mut displays = Vec::with_capacity(link_ids.len());
        for link_id in &link_ids {
            let Some(link) = self.links.get_mut(link_id) else {
                continue;
            };
            link.renderer_id = new_renderer_id.to_owned();
            displays.push(link.display_id);
        }
        self.by_renderer
            .entry(new_renderer_id.to_owned())
            .or_default()
            .extend(link_ids);
        displays.sort_unstable();
        displays.dedup();
        displays
    }

    /// Change one link's owner while preserving its identity and projection.
    pub fn retarget_link(&mut self, link_id: LinkId, renderer_id: &str) -> bool {
        let Some(link) = self.links.get_mut(&link_id) else {
            return false;
        };
        if link.renderer_id == renderer_id {
            return false;
        }
        if let Some(ids) = self.by_renderer.get_mut(&link.renderer_id) {
            ids.retain(|id| *id != link_id);
        }
        link.renderer_id = renderer_id.to_string();
        self.by_renderer
            .entry(renderer_id.to_string())
            .or_default()
            .push(link_id);
        true
    }

    pub fn remove_link(&mut self, link_id: LinkId) -> Option<Link> {
        let link = self.links.remove(&link_id)?;
        if let Some(v) = self.by_display.get_mut(&link.display_id) {
            v.retain(|x| *x != link_id);
        }
        if let Some(v) = self.by_renderer.get_mut(&link.renderer_id) {
            v.retain(|x| *x != link_id);
        }
        Some(link)
    }

    pub fn links_for_display(&self, display_id: DisplayId) -> Vec<Link> {
        self.by_display
            .get(&display_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.links.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn links_for_renderer(&self, renderer_id: &str) -> Vec<Link> {
        self.by_renderer
            .get(renderer_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.links.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    // ---------------------------------------------------------------
    // Display registry (just the ids — full metadata stays in scheduler)

    pub fn remove_display(&mut self, display_id: DisplayId) -> Vec<Link> {
        let link_ids = self.by_display.remove(&display_id).unwrap_or_default();
        let mut removed = Vec::with_capacity(link_ids.len());
        for lid in link_ids {
            if let Some(link) = self.links.remove(&lid) {
                if let Some(v) = self.by_renderer.get_mut(&link.renderer_id) {
                    v.retain(|x| *x != lid);
                }
                removed.push(link);
            }
        }
        removed
    }
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_remove_link_updates_indexes() {
        let mut t = RoutingTable::new();
        let l1 = t.add_link("r1".into(), 1);
        let l2 = t.add_link("r1".into(), 2);
        assert_eq!(t.links_for_display(1).len(), 1);
        assert_eq!(t.links_for_display(2).len(), 1);
        assert_eq!(t.links_for_renderer("r1").len(), 2);

        t.remove_link(l1).unwrap();
        assert!(t.links_for_display(1).is_empty());
        assert_eq!(t.links_for_renderer("r1").len(), 1);
        // l2 still around
        assert_eq!(t.links_for_display(2)[0].id, l2);
    }

    #[test]
    fn retarget_one_link_preserves_metadata_and_both_indexes() {
        let mut table = RoutingTable::new();
        let first = table.add_link("shared".into(), 1);
        let second = table.add_link_with_enabled("shared".into(), 2, false);
        let src = LinkSrcRect {
            x: 10.0,
            y: 20.0,
            w: 100.0,
            h: 200.0,
        };
        table.update_link_geometry(second, Some(src), None, Some(3), Some([0.2; 4]), Some(7));
        assert!(table.retarget_link(second, "independent"));
        let link = table.get_link(second).unwrap();
        assert_eq!(link.renderer_id, "independent");
        assert!(!link.enabled);
        assert_eq!(link.src_rect, src);
        assert_eq!(link.transform, 3);
        assert_eq!(link.clear_rgba, [0.2; 4]);
        assert_eq!(link.z_order, 7);
        assert_eq!(table.links_for_renderer("shared").len(), 1);
        assert_eq!(table.links_for_renderer("shared")[0].id, first);
        assert_eq!(table.links_for_renderer("independent")[0].id, second);
        assert_eq!(table.links_for_display(2)[0].id, second);
        assert!(!table.retarget_link(second, "independent"));
    }

    #[test]
    fn add_link_inherits_renderer_clear_color() {
        let mut t = RoutingTable::new();
        let renderer = RendererHandle::test_stub("r1", "image");
        renderer.test_set_clear_rgba([0.1, 0.2, 0.3, 1.0]);
        t.add_renderer(renderer);

        let link_id = t.add_link("r1".into(), 1);

        assert_eq!(
            t.get_link(link_id).unwrap().clear_rgba,
            [0.1, 0.2, 0.3, 1.0]
        );
    }

    #[test]
    fn remove_renderer_drops_its_links() {
        let mut t = RoutingTable::new();
        let _ = t.add_link("r1".into(), 1);
        let _ = t.add_link("r1".into(), 2);
        let _ = t.add_link("r2".into(), 3);
        let removed = t.remove_renderer("r1");
        assert_eq!(removed.len(), 2);
        assert!(t.links_for_renderer("r1").is_empty());
        assert!(t.links_for_display(1).is_empty());
        assert!(t.links_for_display(2).is_empty());
        assert_eq!(t.links_for_display(3).len(), 1);
    }

    #[test]
    fn retarget_renderer_preserves_link_configuration() {
        let mut t = RoutingTable::new();
        let link_id = t.add_link("old".into(), 7);
        assert!(t.update_link_geometry(
            link_id,
            Some(LinkSrcRect {
                x: 1.0,
                y: 2.0,
                w: 3.0,
                h: 4.0,
            }),
            Some(LinkDstRect {
                x: 5.0,
                y: 6.0,
                w: 7.0,
                h: 8.0,
            }),
            Some(2),
            Some([0.1, 0.2, 0.3, 1.0]),
            Some(9),
        ));
        assert!(t.set_link_enabled(link_id, false));

        assert_eq!(t.retarget_renderer_links("old", "new"), vec![7]);
        assert!(t.links_for_renderer("old").is_empty());
        let link = t.links_for_renderer("new").pop().unwrap();
        assert_eq!(link.id, link_id);
        assert_eq!(link.display_id, 7);
        assert!(!link.enabled);
        assert_eq!(link.transform, 2);
        assert_eq!(link.clear_rgba, [0.1, 0.2, 0.3, 1.0]);
        assert_eq!(link.z_order, 9);
        assert_eq!(link.src_rect.x, 1.0);
        assert_eq!(link.dst_rect.x, 5.0);
    }

    #[test]
    fn add_link_evicts_prior_link_on_same_display() {
        // Single-display invariant: adding rB → d1 must drop the
        // existing rA → d1 link automatically.
        let mut t = RoutingTable::new();
        let l_a = t.add_link("rA".into(), 1);
        let l_b = t.add_link("rB".into(), 1);

        assert_ne!(l_a, l_b, "second add returns a fresh id");
        let links = t.links_for_display(1);
        assert_eq!(links.len(), 1, "exactly one link survives");
        assert_eq!(links[0].id, l_b);
        assert_eq!(links[0].renderer_id, "rB");
        assert!(t.links_for_renderer("rA").is_empty(), "rA fully unlinked");
        assert_eq!(t.links_for_renderer("rB").len(), 1);
    }

    #[test]
    fn remove_display_drops_its_links() {
        // Under the single-link-per-display invariant, only the most
        // recently-added link survives in `by_display`. `remove_display`
        let mut t = RoutingTable::new();
        let _ = t.add_link("r1".into(), 1);
        // r2's add evicts r1's link as part of the invariant.
        let _ = t.add_link("r2".into(), 1);
        let removed = t.remove_display(1);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].renderer_id, "r2");
        assert!(t.links_for_renderer("r1").is_empty());
        assert!(t.links_for_renderer("r2").is_empty());
    }
}
