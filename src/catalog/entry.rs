use serde::{Deserialize, Serialize};

pub type WallpaperType = String;

/// A single wallpaper entry.
/// Source plugins fill the scan-time subset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WallpaperEntry {
    /// Canonical DB `item.id` for this entry's `(library_id, path)`.
    /// Filled by the daemon after scanning.
    #[serde(default)]
    pub item_id: i64,
    /// Human-readable display name.
    pub name: String,
    /// The wallpaper type string (e.g. "scene", "image", "video").
    pub wp_type: WallpaperType,
    #[serde(default)]
    pub sub_type: String,
    /// Filesystem path or URI to the wallpaper resource.
    pub resource: String,
    /// Optional path to a preview/thumbnail image.
    pub preview: Option<String>,
    /// Free-form description (e.g. Wallpaper Engine `project.description`).
    #[serde(default)]
    pub description: Option<String>,
    /// Source-assigned tags. Case-insensitive deduped at the DB layer.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Stable identifier assigned by the source plugin.
    #[serde(default)]
    pub external_id: Option<String>,
    /// Source-provided page for this wallpaper on its remote service.
    #[serde(default)]
    pub web_url: Option<String>,
    #[serde(default)]
    pub update_available: bool,
    /// File size in bytes.
    #[serde(default)]
    pub size: Option<i64>,
    /// Primary video stream width in pixels.
    #[serde(default)]
    pub width: Option<u32>,
    /// Primary video stream height in pixels.
    #[serde(default)]
    pub height: Option<u32>,
    /// Source-provided content rating (e.g. "Everyone", "Questionable",
    /// "Mature"). Free-form string; the daemon doesn't interpret it.
    #[serde(default)]
    pub content_rating: Option<String>,
    /// Scan-time deletion permission hint; never an execution-time authorization.
    #[serde(default)]
    pub can_remove: Option<bool>,
    /// File mtime in nanoseconds since epoch. Daemon-only
    /// (filled on read for sorting); plugins do not set it.
    #[serde(default)]
    pub file_mtime_ns: Option<i64>,
    /// DB insertion time in ms since epoch. Daemon-only fallback for sorting
    /// entries whose file mtime has not been captured yet.
    #[serde(default)]
    pub added_at: i64,
    /// Name of the source plugin that produced this entry.
    /// Written by `SourceManager::scan_plugin`.
    #[serde(default)]
    pub plugin_name: String,
    /// Absolute path of the directory being scanned when produced.
    /// Used to resolve relative resource paths.
    #[serde(default)]
    pub library_root: String,
    /// Library-browser hide flag. Daemon-owned; plugins do not set it.
    #[serde(default)]
    pub hidden: bool,
}
