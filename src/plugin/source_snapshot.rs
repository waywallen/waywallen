//! Read-only mirror of the latest `SourceManager` scan results.
//!
//! `SourceManager` owns a Lua VM and must run scans serially behind a
//! `tokio::Mutex`. While a scan is in flight, every reader that takes
//! that mutex (`WallpaperList`, `WallpaperApply`, `SourceList`) parks
//! behind it — a cold scan over a large library wedges the entire
//! control plane.
//!
//! This snapshot lives next to the manager in `AppState`, guarded by a
//! `tokio::RwLock` so reads never contend with each other and writers
//! only need the lock for a brief swap. `refresh_sources` runs the Lua
//! scan under the manager mutex, then briefly takes the snapshot's
//! write guard to install the new vec; readers see either the old or
//! the new snapshot atomically.
//!
//! Indices on `entries` (`by_type`, `by_id`) are precomputed at install
//! time so reads stay O(1) — the original `SourceManager` recomputed
//! them on every `scan_all`, which we replicate here.

use std::collections::HashMap;

use crate::plugin::source_manager::SourcePluginInfo;
use crate::wallpaper_type::{WallpaperEntry, WallpaperType};

#[derive(Default)]
pub struct SourceSnapshot {
    entries: Vec<WallpaperEntry>,
    by_type: HashMap<WallpaperType, Vec<usize>>,
    by_id: HashMap<String, usize>,
    plugins: Vec<SourcePluginInfo>,
}

impl SourceSnapshot {
    /// Replace the snapshot with a freshly-scanned vec + plugin
    /// metadata. `entries` is consumed; callers do not retain a copy.
    pub fn install(&mut self, entries: Vec<WallpaperEntry>, plugins: Vec<SourcePluginInfo>) {
        let mut by_type: HashMap<WallpaperType, Vec<usize>> = HashMap::new();
        let mut by_id: HashMap<String, usize> = HashMap::with_capacity(entries.len());
        for (idx, e) in entries.iter().enumerate() {
            by_type.entry(e.wp_type.clone()).or_default().push(idx);
            by_id.insert(e.id.clone(), idx);
        }
        self.entries = entries;
        self.by_type = by_type;
        self.by_id = by_id;
        self.plugins = plugins;
    }

    pub fn list(&self) -> &[WallpaperEntry] {
        &self.entries
    }

    pub fn list_by_type(&self, wp_type: &str) -> Vec<&WallpaperEntry> {
        self.by_type
            .get(wp_type)
            .map(|idxs| idxs.iter().map(|i| &self.entries[*i]).collect())
            .unwrap_or_default()
    }

    pub fn get(&self, id: &str) -> Option<&WallpaperEntry> {
        self.by_id.get(id).map(|i| &self.entries[*i])
    }

    pub fn plugins(&self) -> &[SourcePluginInfo] {
        &self.plugins
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, ty: &str) -> WallpaperEntry {
        WallpaperEntry {
            id: id.to_string(),
            name: id.to_string(),
            wp_type: ty.to_string(),
            resource: format!("/tmp/{id}"),
            preview: None,
            metadata: Default::default(),
            description: None,
            tags: vec![],
            external_id: None,
            size: None,
            width: None,
            height: None,
            format: None,
            plugin_name: "test".to_string(),
            library_root: "/tmp".to_string(),
        }
    }

    #[test]
    fn install_indexes_entries() {
        let mut snap = SourceSnapshot::default();
        snap.install(
            vec![entry("a", "image"), entry("b", "image"), entry("c", "scene")],
            vec![],
        );
        assert_eq!(snap.len(), 3);
        assert_eq!(snap.list_by_type("image").len(), 2);
        assert_eq!(snap.list_by_type("scene").len(), 1);
        assert!(snap.get("b").is_some());
        assert!(snap.get("missing").is_none());
    }

    #[test]
    fn install_replaces_indexes() {
        let mut snap = SourceSnapshot::default();
        snap.install(vec![entry("a", "image")], vec![]);
        snap.install(vec![entry("z", "video")], vec![]);
        assert!(snap.get("a").is_none());
        assert!(snap.get("z").is_some());
        assert!(snap.list_by_type("image").is_empty());
    }
}
