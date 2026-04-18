//! Runtime-configurable settings store, persisted to
//! `$XDG_CONFIG_HOME/waywallen/config.toml`.
//!
//! Layout:
//!
//! ```toml
//! [global]
//! default_width  = 1920
//! default_height = 1080
//! default_fps    = 30
//!
//! [plugin.wescene]
//! # Free-form per-plugin table: keys are owned by the plugin, not the
//! # daemon. M7 forwards these into the renderer subprocess via metadata.
//! ```
//!
//! Write strategy: every mutation goes through `update()`, which takes
//! the in-memory write lock, applies the closure, then pokes a
//! `Notify`. A background task debounces those pokes by
//! `DEBOUNCE_WRITE` and then atomically `rename`s a tempfile into
//! place. Callers never block on disk.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock as StdRwLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

/// Quiet period after the last `update()` before the debounced writer
/// flushes to disk. Short enough that `Ctrl-C` shortly after a setting
/// change still persists if the user waits a beat; long enough that
/// rapid-fire UI toggles batch into a single write.
const DEBOUNCE_WRITE: Duration = Duration::from_secs(2);

/// Daemon-wide defaults consumed by `WallpaperApply` when a renderer
/// has no per-plugin override.
///
/// Note: fps is intentionally NOT here. Frame rate is a per-plugin
/// concern (different renderer engines have different sane defaults
/// and capabilities), so it lives in `[plugin.<name>]` tables only.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalSettings {
    pub default_width: u32,
    pub default_height: u32,
}

impl Default for GlobalSettings {
    fn default() -> Self {
        Self {
            default_width: 1920,
            default_height: 1080,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub global: GlobalSettings,
    /// Per-plugin string→string bag. Keyed by plugin name
    /// (`RendererDef.name`). String-only so the contents map cleanly
    /// onto `SpawnRequest.metadata` (which is also `String→String`)
    /// and the `SettingsGet/SetRequest` RPCs without per-value type
    /// gymnastics.
    #[serde(default, rename = "plugin")]
    pub plugins: HashMap<String, HashMap<String, String>>,
}

/// Resolve the on-disk location. Order:
///   1. `$XDG_CONFIG_HOME/waywallen/config.toml`
///   2. `$HOME/.config/waywallen/config.toml`
///   3. `./waywallen.toml` (last-resort fallback so tests can pass
///      `--config` without crossing a real home dir — phase 6 only
///      picks the former two).
pub fn default_config_path() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("waywallen/config.toml");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".config/waywallen/config.toml");
    }
    PathBuf::from("waywallen.toml")
}

pub struct SettingsStore {
    inner: Arc<StdRwLock<Settings>>,
    notify: Arc<Notify>,
    path: PathBuf,
}

impl SettingsStore {
    /// Load from `path` if it exists, otherwise fall back to defaults.
    /// Spawns the debounced-writer task on the current tokio runtime;
    /// callers should keep the returned `Arc` alive for the lifetime
    /// of the daemon or the writer exits.
    pub async fn load_or_default(path: PathBuf) -> Arc<Self> {
        let initial = match tokio::fs::read_to_string(&path).await {
            Ok(s) => match toml::from_str::<Settings>(&s) {
                Ok(parsed) => {
                    log::info!("settings loaded from {}", path.display());
                    parsed
                }
                Err(e) => {
                    log::warn!(
                        "settings parse {}: {e}; continuing with defaults",
                        path.display()
                    );
                    Settings::default()
                }
            },
            Err(e) => {
                log::info!(
                    "settings file {} not readable ({e}); using defaults",
                    path.display()
                );
                Settings::default()
            }
        };

        let store = Arc::new(Self {
            inner: Arc::new(StdRwLock::new(initial)),
            notify: Arc::new(Notify::new()),
            path,
        });

        // Debounced writer task.
        let writer = Arc::clone(&store);
        tokio::spawn(async move {
            writer.writer_loop().await;
        });

        store
    }

    /// Snapshot the current settings. Cheap: clones the inner struct
    /// under a read lock. Callers that only need a few fields should
    /// prefer `global()`/`plugin()` accessors instead.
    pub fn snapshot(&self) -> Settings {
        self.inner.read().expect("settings poisoned").clone()
    }

    /// Copy the `GlobalSettings` subset.
    pub fn global(&self) -> GlobalSettings {
        self.inner.read().expect("settings poisoned").global.clone()
    }

    /// Clone the value map for a single plugin, or `None` if the
    /// plugin has no recorded settings.
    pub fn plugin(&self, plugin_name: &str) -> Option<HashMap<String, String>> {
        self.inner
            .read()
            .expect("settings poisoned")
            .plugins
            .get(plugin_name)
            .cloned()
    }

    /// Apply an in-memory mutation and schedule an eventual disk
    /// flush. Returns as soon as the write lock drops.
    pub fn update<F>(&self, f: F)
    where
        F: FnOnce(&mut Settings),
    {
        {
            let mut g = self.inner.write().expect("settings poisoned");
            f(&mut g);
        }
        self.notify.notify_one();
    }

    async fn writer_loop(self: Arc<Self>) {
        loop {
            // Block until something needs to be written.
            self.notify.notified().await;
            // Debounce: keep resetting the timer until DEBOUNCE_WRITE
            // elapses without another update.
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(DEBOUNCE_WRITE) => break,
                    _ = self.notify.notified() => {}
                }
            }
            self.flush().await;
        }
    }

    async fn flush(&self) {
        let snapshot = self.snapshot();
        let serialized = match toml::to_string_pretty(&snapshot) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("settings serialize failed: {e}");
                return;
            }
        };

        if let Some(parent) = self.path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                log::warn!(
                    "settings create_dir_all {}: {e}",
                    parent.display()
                );
                return;
            }
        }

        // Atomic replace: write to `<name>.tmp` then rename. If the
        // write fails we leave the existing file unchanged.
        let tmp = {
            let mut p = self.path.clone();
            let new_name = match p.file_name() {
                Some(n) => {
                    let mut s = n.to_os_string();
                    s.push(".tmp");
                    s
                }
                None => return,
            };
            p.set_file_name(new_name);
            p
        };
        if let Err(e) = tokio::fs::write(&tmp, serialized).await {
            log::warn!("settings write {}: {e}", tmp.display());
            return;
        }
        if let Err(e) = tokio::fs::rename(&tmp, &self.path).await {
            log::warn!(
                "settings rename {} → {}: {e}",
                tmp.display(),
                self.path.display()
            );
            return;
        }
        log::debug!("settings flushed to {}", self.path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_roundtrip() {
        let s: Settings = toml::from_str("").unwrap();
        assert_eq!(s.global.default_width, 1920);
        assert_eq!(s.global.default_height, 1080);
        assert!(s.plugins.is_empty());
    }

    #[test]
    fn global_override_parses() {
        let src = "[global]\ndefault_width = 2560\n";
        let s: Settings = toml::from_str(src).unwrap();
        assert_eq!(s.global.default_width, 2560);
        // Unspecified fields keep their defaults.
        assert_eq!(s.global.default_height, 1080);
    }

    #[test]
    fn plugin_section_preserved() {
        let src = r#"
[plugin.wescene]
foo = "bar"
baz = "7"
"#;
        let s: Settings = toml::from_str(src).unwrap();
        let wescene = s.plugins.get("wescene").expect("wescene section");
        assert_eq!(wescene.get("foo").map(String::as_str), Some("bar"));
        assert_eq!(wescene.get("baz").map(String::as_str), Some("7"));
    }

    #[tokio::test]
    async fn debounced_write_roundtrips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let store = SettingsStore::load_or_default(path.clone()).await;
        assert_eq!(store.global().default_width, 1920);

        store.update(|s| s.global.default_width = 2560);
        // Wait past the debounce window.
        tokio::time::sleep(DEBOUNCE_WRITE + Duration::from_millis(500)).await;

        let written = tokio::fs::read_to_string(&path).await.unwrap();
        let parsed: Settings = toml::from_str(&written).unwrap();
        assert_eq!(parsed.global.default_width, 2560);
    }
}
