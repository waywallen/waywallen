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
                    Ok(manifest) => {
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

/// Build a registry with backward-compatible defaults:
/// - Scans `$WAYWALLEN_RENDERER_DIR` or `$XDG_DATA_HOME/waywallen/renderers/`
/// - If `WAYWALLEN_RENDERER_BIN` is set, inserts a legacy "scene" renderer at
///   priority 50 so it doesn't override explicit manifests.
pub fn build_default_registry() -> Result<RendererRegistry> {
    let scan_dir = std::env::var_os("WAYWALLEN_RENDERER_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let base = std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    let home = std::env::var_os("HOME").unwrap_or_default();
                    PathBuf::from(home).join(".local/share")
                });
            base.join("waywallen/renderers")
        });

    let mut registry = if scan_dir.is_dir() {
        RendererRegistry::scan(&scan_dir)?
    } else {
        log::info!(
            "renderer manifest dir does not exist: {}; starting with empty registry",
            scan_dir.display()
        );
        RendererRegistry::new()
    };

    // Backward compat: WAYWALLEN_RENDERER_BIN → legacy scene renderer
    if let Some(bin) = std::env::var_os("WAYWALLEN_RENDERER_BIN") {
        let def = RendererDef {
            name: "legacy".to_string(),
            bin: PathBuf::from(bin),
            types: vec!["scene".to_string()],
            extra_args: vec![],
            priority: 50,
        };
        log::info!("registered legacy renderer from WAYWALLEN_RENDERER_BIN");
        registry.register(def);
    }

    Ok(registry)
}
