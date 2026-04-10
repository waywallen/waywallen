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
}
