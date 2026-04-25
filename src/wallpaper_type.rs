use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub type WallpaperType = String;

pub const WP_IMAGE: &str = "image";
pub const WP_VIDEO: &str = "video";
pub const WP_SCENE: &str = "scene";
pub const WP_GIF: &str = "gif";

/// A single wallpaper entry discovered by a source plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WallpaperEntry {
    /// Unique id within the source that produced it (e.g. file path, workshop id).
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// The wallpaper type string (e.g. "scene", "image", "video").
    pub wp_type: WallpaperType,
    /// Filesystem path or URI to the wallpaper resource.
    pub resource: String,
    /// Optional path to a preview/thumbnail image.
    pub preview: Option<String>,
    /// Type-specific metadata. For scene: {"pkg": "...", "assets": "..."}.
    pub metadata: HashMap<String, String>,
    /// Free-form description (e.g. Wallpaper Engine `project.description`).
    #[serde(default)]
    pub description: Option<String>,
    /// Source-assigned tags. Case-insensitive deduped at the DB layer.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Stable external identifier (e.g. Wallpaper Engine `workshopid`).
    #[serde(default)]
    pub external_id: Option<String>,
    /// File size in bytes.
    #[serde(default)]
    pub size: Option<i64>,
    /// Primary video stream width in pixels.
    #[serde(default)]
    pub width: Option<u32>,
    /// Primary video stream height in pixels.
    #[serde(default)]
    pub height: Option<u32>,
    /// Media format string (e.g. "matroska,webm", "image2").
    #[serde(default)]
    pub format: Option<String>,
    /// Name of the source plugin that produced this entry. Written by
    /// `SourceManager::scan_plugin` — Lua plugins do not set it
    /// themselves. Defaults to empty so deserializing older snapshots
    /// stays backwards compatible.
    #[serde(default)]
    pub plugin_name: String,
    /// Absolute path of the directory the plugin was scanning when it
    /// produced this entry. Serves two purposes:
    ///   - `library.path` = this value (absolute folder).
    ///   - `item.relative_path` = `resource` minus this prefix.
    /// Empty means "unrooted" — the sync layer drops such entries
    /// because it cannot address them relatively.
    #[serde(default)]
    pub library_root: String,
}
