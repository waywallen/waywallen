use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::wallpaper_type::WallpaperType;

// ---------------------------------------------------------------------------
// Manifest (TOML on disk)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct RendererManifest {
    pub renderer: RendererDef,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RendererDef {
    pub name: String,
    pub bin: PathBuf,
    pub types: Vec<WallpaperType>,
    #[serde(default)]
    pub extra_args: Vec<String>,
    #[serde(default = "default_priority")]
    pub priority: u32,
}

fn default_priority() -> u32 {
    100
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

pub struct RendererRegistry {
    /// type → list of RendererDef sorted by descending priority.
    by_type: HashMap<WallpaperType, Vec<RendererDef>>,
}

impl RendererRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            by_type: HashMap::new(),
        }
    }

    /// Scan a directory for `*.toml` renderer manifest files and populate
    /// the registry. Non-parseable files are logged and skipped.
    pub fn scan(dir: &Path) -> Result<Self> {
        let mut reg = Self::new();
        let pattern = dir.join("*.toml");
        let pattern_str = pattern
            .to_str()
            .context("manifest dir path not valid UTF-8")?;
        for entry in glob::glob(pattern_str).context("glob pattern")? {
            let path = match entry {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("renderer manifest glob error: {e}");
                    continue;
                }
            };
            match std::fs::read_to_string(&path) {
                Ok(contents) => match toml::from_str::<RendererManifest>(&contents) {
                    Ok(mut manifest) => {
                        // Resolve relative bin paths against the manifest's directory.
                        if manifest.renderer.bin.is_relative() {
                            if let Some(manifest_dir) = path.parent() {
                                manifest.renderer.bin = manifest_dir.join(&manifest.renderer.bin);
                            }
                        }
                        log::info!(
                            "loaded renderer manifest: {} (types: {:?})",
                            manifest.renderer.name,
                            manifest.renderer.types
                        );
                        reg.register(manifest.renderer);
                    }
                    Err(e) => log::warn!("skip {}: {e}", path.display()),
                },
                Err(e) => log::warn!("skip {}: {e}", path.display()),
            }
        }
        Ok(reg)
    }

    /// Register a renderer definition programmatically.
    pub fn register(&mut self, def: RendererDef) {
        for wp_type in &def.types {
            let list = self.by_type.entry(wp_type.clone()).or_default();
            list.push(def.clone());
            list.sort_by(|a, b| b.priority.cmp(&a.priority));
        }
    }

    /// Find the highest-priority renderer for a wallpaper type.
    pub fn resolve(&self, wp_type: &str) -> Option<&RendererDef> {
        self.by_type.get(wp_type)?.first()
    }

    /// List all wallpaper types that have at least one renderer.
    pub fn supported_types(&self) -> Vec<&WallpaperType> {
        self.by_type.keys().collect()
    }

    /// List all registered renderer definitions (deduplicated by name).
    pub fn all_renderers(&self) -> Vec<&RendererDef> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for defs in self.by_type.values() {
            for def in defs {
                if seen.insert(&def.name) {
                    out.push(def);
                }
            }
        }
        out
    }
}

/// Build a registry by scanning the two canonical plugin paths:
/// 1. `<exec>/../share/waywallen/renderers/`  (bundled / system install)
/// 2. `$XDG_DATA_HOME/waywallen/renderers/`   (user overrides)
///
/// User-local manifests (XDG) are loaded last so they can shadow bundled
/// ones by name. Non-existent directories are silently skipped.
pub fn build_default_registry() -> Result<RendererRegistry> {
    let mut registry = RendererRegistry::new();

    for dir in standard_plugin_dirs("renderers") {
        if dir.is_dir() {
            match RendererRegistry::scan(&dir) {
                Ok(scanned) => {
                    for def in scanned.all_renderers() {
                        registry.register(def.clone());
                    }
                }
                Err(e) => log::warn!("scan {}: {e}", dir.display()),
            }
        }
    }

    Ok(registry)
}

/// Return the two canonical plugin directories (bundled + XDG) for a
/// given subdirectory name (e.g. `"renderers"` or `"sources"`). Returned
/// in load order: bundled first, user-local second.
pub fn standard_plugin_dirs(subdir: &str) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // Bundled: <exec>/../share/waywallen/<subdir>/
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            if let Some(prefix) = parent.parent() {
                dirs.push(prefix.join("share/waywallen").join(subdir));
            }
        }
    }

    // User-local: $XDG_DATA_HOME/waywallen/<subdir>/
    let xdg = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").unwrap_or_default();
            PathBuf::from(home).join(".local/share")
        });
    dirs.push(xdg.join("waywallen").join(subdir));

    dirs
}
