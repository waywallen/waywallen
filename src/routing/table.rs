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

/// A single (renderer → display) routing edge. Phase 1 only uses
/// `enabled` and the identity fields; geometry/transform fields land
/// in Phase 3 once `SetConfig` per-link composition is wired in.
#[derive(Debug, Clone)]
pub struct Link {
    pub id: LinkId,
    pub renderer_id: RendererId,
    pub display_id: DisplayId,
    pub enabled: bool,
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
        };
        self.links.insert(id, link);
        self.by_display.entry(display_id).or_default().push(id);
        self.by_renderer.entry(renderer_id).or_default().push(id);
        id
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
