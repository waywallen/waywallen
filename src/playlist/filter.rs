//! Smart-playlist predicate over [`WallpaperEntry`].
//!
//! `Filter` is a flat AND-of-fields predicate: every populated field
//! must match. Within a single multi-valued field (e.g. [`wp_types`])
//! values are OR-ed. A default `Filter::default()` matches every
//! entry — that's the "All" pseudo-playlist.
//!
//! Evaluation is pure and DB-free: tag/library/format predicates work
//! against whatever the source plugin attached to the in-memory entry,
//! so the same predicate drives smart-playlist materialization,
//! browse-list filtering, and ad-hoc shuffle previews without going
//! near SQL. Persistence is via `to_json` / `from_json` into the
//! `playlist.filter_json` column.
//!
//! [`wp_types`]: Filter::wp_types
//! [`WallpaperEntry`]: crate::wallpaper_type::WallpaperEntry

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::wallpaper_type::WallpaperEntry;

/// Aspect-ratio bucket. Tolerance for landscape vs square is `±5%`,
/// chosen so 16:9 / 16:10 land in `Landscape` and 1:1 photo crops with
/// pixel-rounding noise still register as `Square`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AspectClass {
    Landscape,
    Portrait,
    Square,
}

const ASPECT_TOLERANCE: f32 = 0.05;

impl AspectClass {
    fn classify(width: u32, height: u32) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        let ratio = width as f32 / height as f32;
        if ratio > 1.0 + ASPECT_TOLERANCE {
            Some(AspectClass::Landscape)
        } else if ratio < 1.0 - ASPECT_TOLERANCE {
            Some(AspectClass::Portrait)
        } else {
            Some(AspectClass::Square)
        }
    }
}

/// Smart-playlist predicate. All populated fields must match (AND);
/// multi-valued fields (`Vec<_>`) match if **any** value matches (OR).
/// `tags_all` is the one exception: it requires **every** listed tag
/// to be present on the entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Filter {
    /// e.g. `["scene", "video"]`. Empty = any wp_type. Compared
    /// case-insensitively against [`WallpaperEntry::wp_type`].
    pub wp_types: Vec<String>,

    /// Any-match tag set (case-insensitive). Empty = no constraint.
    pub tags_any: Vec<String>,

    /// All-match tag set (case-insensitive). Every name must appear in
    /// the entry's tags. Empty = no constraint.
    pub tags_all: Vec<String>,

    /// Restrict to entries whose `library_root` is in this list.
    /// Compared as exact path strings (after trailing-slash trim).
    /// Empty = any library.
    pub libraries: Vec<String>,

    /// File-extension whitelist, e.g. `["mp4", "webm"]`. Lowercased
    /// and compared against the extension of [`WallpaperEntry::resource`].
    /// Empty = any format.
    pub formats: Vec<String>,

    /// Case-insensitive substring match against the entry's `name`.
    pub name_like: Option<String>,

    pub min_width: Option<u32>,
    pub min_height: Option<u32>,
    pub min_size: Option<i64>,
    pub max_size: Option<i64>,

    /// Aspect-ratio bucket. Entries with unknown width/height fail
    /// this check rather than passing it.
    pub aspect: Option<AspectClass>,
}

impl Filter {
    /// True for every entry. Convenience for `Filter::default()`.
    pub fn match_all() -> Self {
        Self::default()
    }

    /// True if `self` is the empty/identity predicate.
    pub fn is_match_all(&self) -> bool {
        self.wp_types.is_empty()
            && self.tags_any.is_empty()
            && self.tags_all.is_empty()
            && self.libraries.is_empty()
            && self.formats.is_empty()
            && self.name_like.as_deref().map(str::is_empty).unwrap_or(true)
            && self.min_width.is_none()
            && self.min_height.is_none()
            && self.min_size.is_none()
            && self.max_size.is_none()
            && self.aspect.is_none()
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Evaluate against a single entry.
    pub fn matches(&self, e: &WallpaperEntry) -> bool {
        if !self.wp_types.is_empty()
            && !self.wp_types.iter().any(|t| eq_ci(t, &e.wp_type))
        {
            return false;
        }

        if !self.libraries.is_empty() {
            let lib = trim_trailing_slash(&e.library_root);
            if !self
                .libraries
                .iter()
                .any(|l| trim_trailing_slash(l) == lib)
            {
                return false;
            }
        }

        if !self.formats.is_empty() {
            let ext = resource_extension(&e.resource);
            let matched = ext
                .as_deref()
                .map(|ext| self.formats.iter().any(|f| eq_ci(f, ext)))
                .unwrap_or(false);
            if !matched {
                return false;
            }
        }

        if !self.tags_any.is_empty() {
            let any = self
                .tags_any
                .iter()
                .any(|t| e.tags.iter().any(|et| eq_ci(t, et)));
            if !any {
                return false;
            }
        }

        if !self.tags_all.is_empty() {
            let all = self
                .tags_all
                .iter()
                .all(|t| e.tags.iter().any(|et| eq_ci(t, et)));
            if !all {
                return false;
            }
        }

        if let Some(needle) = self.name_like.as_deref() {
            if !needle.is_empty() && !contains_ci(&e.name, needle) {
                return false;
            }
        }

        if let Some(min) = self.min_width {
            if e.width.map(|w| w < min).unwrap_or(true) {
                return false;
            }
        }
        if let Some(min) = self.min_height {
            if e.height.map(|h| h < min).unwrap_or(true) {
                return false;
            }
        }
        if let Some(min) = self.min_size {
            if e.size.map(|s| s < min).unwrap_or(true) {
                return false;
            }
        }
        if let Some(max) = self.max_size {
            if e.size.map(|s| s > max).unwrap_or(true) {
                return false;
            }
        }

        if let Some(want) = self.aspect {
            let got = match (e.width, e.height) {
                (Some(w), Some(h)) => AspectClass::classify(w, h),
                _ => None,
            };
            if got != Some(want) {
                return false;
            }
        }

        true
    }

    /// Apply over an iterator of entries, returning the matched ids in
    /// the input order. Stable across re-evaluations as long as the
    /// upstream iteration order is stable (the source manager sorts by
    /// id, so it is).
    pub fn apply<'a, I>(&self, entries: I) -> Vec<String>
    where
        I: IntoIterator<Item = &'a WallpaperEntry>,
    {
        entries
            .into_iter()
            .filter(|e| self.matches(e))
            .map(|e| e.id.clone())
            .collect()
    }
}

fn eq_ci(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.eq_ignore_ascii_case(b)
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    if needle.len() > haystack.len() {
        return false;
    }
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn trim_trailing_slash(s: &str) -> &str {
    s.strip_suffix('/').unwrap_or(s)
}

fn resource_extension(resource: &str) -> Option<String> {
    Path::new(resource)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn entry(id: &str) -> WallpaperEntry {
        WallpaperEntry {
            id: id.into(),
            name: id.into(),
            wp_type: "image".into(),
            resource: format!("/lib/{id}.png"),
            preview: None,
            metadata: HashMap::new(),
            description: None,
            tags: Vec::new(),
            external_id: None,
            size: None,
            width: None,
            height: None,
            format: None,
            plugin_name: "p".into(),
            library_root: "/lib".into(),
        }
    }

    #[test]
    fn default_filter_matches_everything() {
        let f = Filter::default();
        assert!(f.is_match_all());
        assert!(f.matches(&entry("a")));

        let mut img = entry("b");
        img.wp_type = "video".into();
        img.size = Some(1);
        assert!(f.matches(&img));
    }

    #[test]
    fn wp_types_or_semantics_case_insensitive() {
        let mut f = Filter::default();
        f.wp_types = vec!["scene".into(), "VIDEO".into()];

        let mut a = entry("a");
        a.wp_type = "image".into();
        assert!(!f.matches(&a));

        let mut b = entry("b");
        b.wp_type = "Scene".into();
        assert!(f.matches(&b));

        let mut c = entry("c");
        c.wp_type = "video".into();
        assert!(f.matches(&c));
    }

    #[test]
    fn tags_any_vs_tags_all() {
        let mut e = entry("e");
        e.tags = vec!["Anime".into(), "Landscape".into()];

        let mut any = Filter::default();
        any.tags_any = vec!["nature".into(), "anime".into()];
        assert!(any.matches(&e));

        let mut all = Filter::default();
        all.tags_all = vec!["anime".into(), "landscape".into()];
        assert!(all.matches(&e));

        let mut all_miss = Filter::default();
        all_miss.tags_all = vec!["anime".into(), "game".into()];
        assert!(!all_miss.matches(&e));
    }

    #[test]
    fn formats_match_extension_case_insensitive() {
        let mut a = entry("a");
        a.resource = "/x/y.MP4".into();

        let mut f = Filter::default();
        f.formats = vec!["mp4".into()];
        assert!(f.matches(&a));

        let mut b = entry("b");
        b.resource = "/x/no_ext".into();
        assert!(!f.matches(&b));

        let mut c = entry("c");
        c.resource = "/x/y.webm".into();
        assert!(!f.matches(&c));
    }

    #[test]
    fn name_like_substring_case_insensitive() {
        let mut e = entry("e");
        e.name = "Cherry Blossom".into();

        let mut f = Filter::default();
        f.name_like = Some("blossom".into());
        assert!(f.matches(&e));

        f.name_like = Some("BLOSSOM".into());
        assert!(f.matches(&e));

        f.name_like = Some("absent".into());
        assert!(!f.matches(&e));

        // Empty needle is treated as no constraint.
        f.name_like = Some(String::new());
        assert!(f.matches(&e));
    }

    #[test]
    fn min_dimensions_and_size_bounds() {
        let mut e = entry("e");
        e.width = Some(1920);
        e.height = Some(1080);
        e.size = Some(10_000);

        let mut f = Filter::default();
        f.min_width = Some(1920);
        f.min_height = Some(1080);
        f.min_size = Some(1);
        f.max_size = Some(10_000);
        assert!(f.matches(&e));

        f.min_width = Some(2560);
        assert!(!f.matches(&e));
    }

    #[test]
    fn missing_dimensions_fail_min_filters() {
        let e = entry("e"); // width/height/size all None
        let mut f = Filter::default();
        f.min_width = Some(1);
        assert!(!f.matches(&e));

        let mut g = Filter::default();
        g.min_size = Some(1);
        assert!(!g.matches(&e));
    }

    #[test]
    fn aspect_classifies_with_tolerance() {
        let mut e = entry("e");
        e.width = Some(1920);
        e.height = Some(1080);

        let mut f = Filter::default();
        f.aspect = Some(AspectClass::Landscape);
        assert!(f.matches(&e));

        f.aspect = Some(AspectClass::Portrait);
        assert!(!f.matches(&e));

        // 1:1 with rounding noise is still square.
        let mut sq = entry("sq");
        sq.width = Some(1024);
        sq.height = Some(1000);
        let mut g = Filter::default();
        g.aspect = Some(AspectClass::Square);
        assert!(g.matches(&sq));

        // Unknown dims always fail aspect filter.
        let unknown = entry("u");
        assert!(!g.matches(&unknown));
    }

    #[test]
    fn libraries_match_with_trailing_slash_normalization() {
        let mut a = entry("a");
        a.library_root = "/home/me/wp".into();

        let mut f = Filter::default();
        f.libraries = vec!["/home/me/wp/".into()];
        assert!(f.matches(&a));

        f.libraries = vec!["/home/other".into()];
        assert!(!f.matches(&a));
    }

    #[test]
    fn json_roundtrip_preserves_fields() {
        let mut f = Filter::default();
        f.wp_types = vec!["scene".into()];
        f.tags_any = vec!["Anime".into()];
        f.min_width = Some(1920);
        f.aspect = Some(AspectClass::Portrait);

        let s = f.to_json().unwrap();
        let g = Filter::from_json(&s).unwrap();
        assert_eq!(f, g);
    }

    #[test]
    fn from_json_accepts_partial_objects() {
        // Older config snapshots / hand-edited JSON may omit fields.
        let f: Filter = Filter::from_json(r#"{"wp_types":["video"]}"#).unwrap();
        assert_eq!(f.wp_types, vec!["video".to_string()]);
        assert!(f.tags_any.is_empty());
        assert!(f.aspect.is_none());
    }

    #[test]
    fn apply_returns_ids_in_input_order() {
        let mut a = entry("a");
        a.wp_type = "scene".into();
        let b = entry("b"); // image
        let mut c = entry("c");
        c.wp_type = "scene".into();

        let mut f = Filter::default();
        f.wp_types = vec!["scene".into()];
        let ids = f.apply(&[a, b, c]);
        assert_eq!(ids, vec!["a".to_string(), "c".to_string()]);
    }
}
