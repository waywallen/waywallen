use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Manifest (TOML on disk)

#[derive(Debug, Clone, Deserialize)]
pub struct DisplayManifest {
    pub display: DisplayDef,
}

/// How the daemon handles the backend's lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpawnMode {
    /// Daemon starts the `bin` as a subprocess (e.g. wlr-layer-shell bin).
    Daemon,
    /// DE starts it on its own (e.g. Plasma kpackage loaded by plasmashell).
    External,
}

impl Default for SpawnMode {
    fn default() -> Self {
        SpawnMode::Daemon
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DisplayDef {
    pub name: String,
    /// Executable path. Only meaningful when `spawn == Daemon`.
    #[serde(default)]
    pub bin: PathBuf,
    /// DE tokens (lower-case) this backend targets. `"*"` = any DE.
    /// Matched against the first token of `XDG_CURRENT_DESKTOP` (split on `:`).
    #[serde(default)]
    pub de: Vec<String>,
    #[serde(default = "default_priority")]
    pub priority: i32,
    /// Optional capability tokens this backend needs.
    /// `display::spawner` intersects them with probed Wayland globals.
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub extra_args: Vec<String>,
    #[serde(default)]
    pub spawn: SpawnMode,
}

fn default_priority() -> i32 {
    100
}

/// `name` next to the daemon executable if it is there, else `name` for PATH.
fn sibling_bin(name: &str) -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let candidate = parent.join(name);
            if candidate.exists() {
                return candidate;
            }
        }
    }
    PathBuf::from(name)
}

/// Display backends available without an installed manifest.
pub fn builtin_display_defs() -> Vec<DisplayDef> {
    vec![
        DisplayDef {
            name: "kde-plasma".to_string(),
            bin: PathBuf::new(),
            de: vec!["kde".to_string()],
            priority: 100,
            requires: Vec::new(),
            extra_args: Vec::new(),
            spawn: SpawnMode::External,
        },
        DisplayDef {
            name: "gnome-shell".to_string(),
            bin: PathBuf::new(),
            de: vec!["gnome".to_string()],
            priority: 100,
            requires: Vec::new(),
            extra_args: Vec::new(),
            spawn: SpawnMode::External,
        },
        DisplayDef {
            name: "layer-shell".to_string(),
            bin: sibling_bin("waywallen-layer-shell"),
            de: vec![
                "hyprland".to_string(),
                "sway".to_string(),
                "niri".to_string(),
                "river".to_string(),
                "cosmic".to_string(),
                "wayfire".to_string(),
            ],
            priority: 50,
            requires: vec!["wlr-layer-shell".to_string(), "linux-dmabuf-v4".to_string()],
            extra_args: Vec::new(),
            spawn: SpawnMode::Daemon,
        },
        DisplayDef {
            name: "x11".to_string(),
            bin: sibling_bin("waywallen-x11"),
            de: vec![
                "mate".to_string(),
                "x-cinnamon".to_string(),
                "xfce".to_string(),
            ],
            priority: 50,
            requires: vec!["x11".to_string()],
            extra_args: Vec::new(),
            spawn: SpawnMode::Daemon,
        },
    ]
}

// ---------------------------------------------------------------------------
// Registry

#[derive(Default)]
pub struct DisplayRegistry {
    /// Sorted descending by priority; ties broken by registration order.
    defs: Vec<DisplayDef>,
}

impl DisplayRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        for def in builtin_display_defs() {
            registry.register(def);
        }
        registry
    }

    /// Scan a directory for `*.toml` display manifests and collect them.
    /// Non-parseable files are logged and skipped. Relative `bin` paths
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
                    log::warn!("display manifest glob error: {e}");
                    continue;
                }
            };
            match std::fs::read_to_string(&path) {
                Ok(contents) => match toml::from_str::<DisplayManifest>(&contents) {
                    Ok(mut manifest) => {
                        if manifest.display.bin.is_relative()
                            && !manifest.display.bin.as_os_str().is_empty()
                        {
                            if let Some(manifest_dir) = path.parent() {
                                manifest.display.bin = manifest_dir.join(&manifest.display.bin);
                            }
                        }
                        log::info!(
                            "loaded display manifest: {} (de={:?} spawn={:?} priority={})",
                            manifest.display.name,
                            manifest.display.de,
                            manifest.display.spawn,
                            manifest.display.priority
                        );
                        reg.register(manifest.display);
                    }
                    Err(e) => log::warn!("skip {}: {e}", path.display()),
                },
                Err(e) => log::warn!("skip {}: {e}", path.display()),
            }
        }
        Ok(reg)
    }

    pub fn register(&mut self, def: DisplayDef) {
        // Replace any existing entry with the same name (XDG overrides bundled).
        self.defs.retain(|d| d.name != def.name);
        self.defs.push(def);
        self.defs.sort_by(|a, b| b.priority.cmp(&a.priority));
    }

    pub fn all(&self) -> &[DisplayDef] {
        &self.defs
    }

    pub fn find(&self, name: &str) -> Option<&DisplayDef> {
        self.defs.iter().find(|d| d.name == name)
    }
}

/// Build the registry from built-ins, bundled manifests, then user manifests.
pub fn build_default_registry() -> DisplayRegistry {
    let mut registry = DisplayRegistry::with_builtins();
    for dir in crate::plugin::renderer_registry::standard_plugin_dirs("displays") {
        if dir.is_dir() {
            match DisplayRegistry::scan(&dir) {
                Ok(scanned) => {
                    for def in scanned.all() {
                        registry.register(def.clone());
                    }
                }
                Err(e) => log::warn!("scan {}: {e}", dir.display()),
            }
        }
    }
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_registered_by_name() {
        let registry = DisplayRegistry::with_builtins();

        assert_eq!(
            registry.find("kde-plasma").map(|def| def.spawn),
            Some(SpawnMode::External)
        );
        assert_eq!(
            registry.find("layer-shell").map(|def| def.spawn),
            Some(SpawnMode::Daemon)
        );
        assert_eq!(
            registry.find("gnome-shell").map(|def| def.spawn),
            Some(SpawnMode::External)
        );
        assert_eq!(
            registry.find("x11").map(|def| def.spawn),
            Some(SpawnMode::Daemon)
        );
    }

    #[test]
    fn later_registration_overrides_builtin_by_name() {
        let mut registry = DisplayRegistry::with_builtins();
        let replacement = DisplayDef {
            name: "layer-shell".to_string(),
            bin: PathBuf::from("/custom/layer-shell"),
            de: vec!["kde".to_string()],
            priority: 200,
            requires: Vec::new(),
            extra_args: vec!["--custom".to_string()],
            spawn: SpawnMode::Daemon,
        };

        registry.register(replacement);

        let resolved = registry.find("layer-shell").expect("replacement");
        assert_eq!(resolved.bin, PathBuf::from("/custom/layer-shell"));
        assert_eq!(resolved.de, ["kde"]);
        assert_eq!(resolved.extra_args, ["--custom"]);
    }
}
