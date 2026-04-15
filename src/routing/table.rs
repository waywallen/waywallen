//! Plain CRUD on the routing table. No I/O, no awaits.
//!
//! `RoutingTable` is the source of truth for which displays exist,
//! which renderers exist, and how they are linked. The router wraps
//! it in a `tokio::Mutex` and layers the dispatcher logic on top.

use std::collections::HashMap;
use std::sync::Arc;

use crate::renderer_manager::{RendererHandle, RendererId};
use crate::scheduler::DisplayId;

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
pub const FULL_SRC: LinkSrcRect = LinkSrcRect { x: 0.0, y: 0.0, w: f32::INFINITY, h: f32::INFINITY };
pub const FULL_DST: LinkDstRect = LinkDstRect { x: 0.0, y: 0.0, w: f32::INFINITY, h: f32::INFINITY };

/// A single (renderer → display) routing edge. Phase 3 grows
/// geometry; Phase 4 adds per-link multiplexing on the wire.
#[derive(Debug, Clone)]
pub struct Link {
    pub id: LinkId,
    pub renderer_id: RendererId,
    pub display_id: DisplayId,
    pub enabled: bool,
    /// Source rect in renderer texture space (use `FULL_SRC` for identity).
    pub src_rect: LinkSrcRect,
    /// Destination rect in display surface space (use `FULL_DST` for identity).
    pub dst_rect: LinkDstRect,
    /// `wl_output.transform` value: 0=normal, 1=90, 2=180, 3=270, 4..=flipped.
    pub transform: u32,
    /// Background clear color (RGBA, 0..=1).
    pub clear_rgba: [f32; 4],
    /// Z-order (higher = on top). Phase 4 multi-link composition.
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
    // ---------------------------------------------------------------

    pub fn add_renderer(&mut self, handle: Arc<RendererHandle>) {
        let id = handle.id.clone();
        self.renderers.insert(id, handle);
    }

    /// Remove a renderer and the links pointing at it. Returns the
    /// (link_id, display_id) pairs that were removed so the caller can
    /// notify those displays (e.g. emit Unbind).
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

    /// Phase 1 helper: pick "any" renderer to seed a new display's
    /// initial link with. Replaced by per-display config in Phase 2.
    pub fn first_renderer(&self) -> Option<RendererId> {
        let mut ids: Vec<&RendererId> = self.renderers.keys().collect();
        ids.sort();
        ids.into_iter().next().cloned()
    }

    // ---------------------------------------------------------------
    // Links
    // ---------------------------------------------------------------

    pub fn add_link(&mut self, renderer_id: RendererId, display_id: DisplayId) -> LinkId {
        self.next_link_id += 1;
        let id = self.next_link_id;
        let link = Link {
            id,
            renderer_id: renderer_id.clone(),
            display_id,
            enabled: true,
            src_rect: FULL_SRC,
            dst_rect: FULL_DST,
            transform: 0,
            clear_rgba: [0.0, 0.0, 0.0, 1.0],
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

    pub fn get_link(&self, link_id: LinkId) -> Option<&Link> {
        self.links.get(&link_id)
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
    // ---------------------------------------------------------------

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
// ---------------------------------------------------------------------------

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
    fn remove_display_drops_its_links() {
        let mut t = RoutingTable::new();
        let _ = t.add_link("r1".into(), 1);
        let _ = t.add_link("r2".into(), 1);
        let removed = t.remove_display(1);
        assert_eq!(removed.len(), 2);
        assert!(t.links_for_renderer("r1").is_empty());
        assert!(t.links_for_renderer("r2").is_empty());
    }
}
