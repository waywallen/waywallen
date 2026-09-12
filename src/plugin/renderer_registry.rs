use serde::Deserialize;
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use crate::catalog::entry::WallpaperType;
use crate::plugin::i18n::{LocalizedTextDef, PluginTranslationDocument, PluginTranslationMeta};

// ---------------------------------------------------------------------------
// Installable-plugin manifest (`plugins/<dir>/plugin.toml`)

/// One installable plugin: a `[plugin]` header plus optional Lua entry
/// and any number of renderers.
#[derive(Debug, Clone, Deserialize)]
pub struct PluginManifest {
    pub plugin: PluginMeta,
    /// Renderer components keyed by component name (the map key becomes
    /// `RendererDef.name`).
    #[serde(default)]
    pub renderers: HashMap<String, RendererDef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginMeta {
    /// Domain-style unique id, e.g. `org.waywallen.image`.
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    /// URL of a remote JSON update manifest for this plugin package.
    #[serde(default)]
    pub update: Option<String>,
    /// Lua entry file path, relative to the plugin directory.
    #[serde(default)]
    pub entry: Option<PathBuf>,
    /// Lua entry ABI version supported by `entry`.
    #[serde(default)]
    pub entry_version: Option<u32>,
    #[serde(default)]
    pub i18n: Option<PluginTranslationMeta>,
    /// Declarative file manifest with paths relative to the plugin dir.
    /// Loaded during scanning from the required `files.txt`.
    #[serde(skip)]
    pub files: Vec<String>,
    /// True when the plugin was discovered from a system scan root.
    #[serde(skip)]
    pub system: bool,
    #[serde(skip)]
    pub translations: Vec<PluginTranslationDocument>,
}

/// Hard-coded name of the per-plugin file manifest every plugin must
/// ship beside its `plugin.toml`. One path per line; blanks ignored.
pub const PLUGIN_FILES_MANIFEST: &str = "files.txt";

/// Lua entry discovered in a plugin manifest.
/// Carries the owning plugin metadata and entry ABI version.
#[derive(Debug, Clone)]
pub struct EntryRef {
    pub plugin_id: String,
    pub plugin_version: String,
    pub plugin_system: bool,
    pub entry: PathBuf,
    pub entry_version: u32,
}

/// Components collected from scanning one or more `plugins/` directories.
#[derive(Debug, Default)]
pub struct PluginScan {
    pub renderers: Vec<RendererDef>,
    pub entries: Vec<EntryRef>,
    /// Parsed plugin metadata (id/name/version + resolved `files`),
    /// retained for introspection.
    pub plugins: Vec<PluginMeta>,
    pub inactive_system: Vec<String>,
    pub inactive_user: Vec<String>,
    /// Plugins that are installed and selected, but declare a contract this
    /// daemon does not implement. Kept so the refusal can be reported once,
    /// with a reason, instead of surfacing as a log line and silence.
    pub incompatible: Vec<PluginIncompat>,
}

/// An installed plugin this daemon cannot serve, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginIncompat {
    pub plugin_id: String,
    /// Human-readable, daemon-authored; names what was declared and what is
    /// supported, so a client can show it without inventing wording.
    pub reason: String,
}

impl PluginScan {
    pub fn merge(&mut self, other: PluginScan) {
        self.renderers.extend(other.renderers);
        self.entries.extend(other.entries);
        self.plugins.extend(other.plugins);
        self.inactive_system.extend(other.inactive_system);
        self.inactive_user.extend(other.inactive_user);
        self.incompatible.extend(other.incompatible);
    }

    /// Installable-plugin view of the scan.
    /// One entry per `[plugin]`, with Lua entry presence included.
    pub fn packages(&self) -> Vec<PluginPackageMeta> {
        self.plugins
            .iter()
            .map(|m| PluginPackageMeta {
                id: m.id.clone(),
                name: m.name.clone(),
                version: m.version.clone(),
                update: m.update.clone(),
                has_entry: self.entries.iter().any(|s| s.plugin_id == m.id),
                system: m.system,
                incompat: self
                    .incompatible
                    .iter()
                    .find(|i| i.plugin_id == m.id)
                    .map(|i| i.reason.clone()),
            })
            .collect()
    }

    pub fn translation_documents(
        &self,
    ) -> std::result::Result<Vec<PluginTranslationDocument>, String> {
        crate::plugin::i18n::prepare_plugin_translation_documents(
            self.plugins
                .iter()
                .flat_map(|plugin| plugin.translations.iter().cloned()),
        )
    }
}

/// Installable-plugin (package) summary, retained in `DaemonContext` so the UI
/// can present a plugin-centric view independent of the component registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPackageMeta {
    pub id: String,
    pub name: String,
    pub version: String,
    pub update: Option<String>,
    pub has_entry: bool,
    /// Discovered from a system scan root.
    pub system: bool,
    /// `Some(reason)` when the plugin is installed but declares a contract
    /// this daemon does not implement; `None` when it can be served.
    pub incompat: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRoot {
    pub dir: PathBuf,
    pub system: bool,
}

impl PluginRoot {
    pub fn system(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: normalize_path(&dir.into()),
            system: true,
        }
    }

    pub fn user(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: normalize_path(&dir.into()),
            system: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RendererActivityMode {
    Continuous,
    #[default]
    OnDemand,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RendererDef {
    /// Component name. Empty in a manifest map value — filled from the
    /// `[renderers.<name>]` map key during scanning.
    #[serde(default)]
    pub name: String,
    /// Domain id of the owning installable plugin. Filled during
    /// scanning; reported to UIs so they can show a renderer's source.
    #[serde(default)]
    pub plugin_id: String,
    #[serde(skip)]
    pub plugin_version: String,
    #[serde(skip)]
    pub plugin_system: bool,
    pub bin: PathBuf,
    pub types: Vec<WallpaperType>,
    #[serde(default = "default_priority")]
    pub priority: u32,
    /// Declares whether lack of frame progress is meaningful. Third-party
    /// manifests default to on-demand for backward compatibility.
    #[serde(default)]
    pub activity: RendererActivityMode,
    /// Renderer spawn contract version declared by the plugin.
    /// `None` means the plugin accepts the daemon compile-time version.
    #[serde(default)]
    pub spawn_version: Option<u32>,
    /// Allow-listed metadata keys forwarded as `Init.resource_extras`.
    /// The canonical `path` is always handled separately.
    #[serde(default)]
    pub extras: Vec<String>,
    /// Optional schema for plugin-level settings.
    /// Each entry declares a type and validation envelope.
    #[serde(default)]
    pub settings: HashMap<String, SettingDef>,
    /// Retained only so manifests using the removed static subscription
    /// field can be rejected instead of silently changing behaviour.
    #[serde(default, rename = "events")]
    pub legacy_events: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SettingDef {
    #[serde(rename = "type")]
    pub ty: SettingType,
    pub default: toml::Value,
    /// When `true` (the default), the setting participates in the
    /// renderer's identity hash, so changes respawn the renderer.
    #[serde(default = "default_true")]
    pub identity: bool,
    /// i18n key the UI binds to for the field label.
    /// Optional for older manifests.
    #[serde(default)]
    pub label_key: Option<String>,
    /// Optional i18n key for a short helper / tooltip line.
    #[serde(default)]
    pub description_key: Option<String>,
    #[serde(default)]
    pub label: Option<LocalizedTextDef>,
    #[serde(default)]
    pub description: Option<LocalizedTextDef>,
    #[serde(default)]
    pub group_label: Option<LocalizedTextDef>,
    /// Numeric lower bound (inclusive) for `U32`/`I32`/`F32` settings.
    /// Ignored on string/bool; `SettingsSet` rejects out-of-range values.
    #[serde(default)]
    pub min: Option<toml::Value>,
    /// Numeric upper bound (inclusive). Same semantics as `min`.
    #[serde(default)]
    pub max: Option<toml::Value>,
    /// Optional UI hint for slider/spinner increments.
    /// The daemon validates range, not step alignment.
    #[serde(default)]
    pub step: Option<toml::Value>,
    /// Allowed string values.
    /// Only valid for `String`; `SettingsSet` rejects other values.
    #[serde(default)]
    pub choices: Option<Vec<String>>,
    /// Logical group key. UI groups settings sharing this name into
    /// the same panel section. `None` = ungrouped.
    #[serde(default)]
    pub group: Option<String>,
    /// Sort order within a group. Lower goes first. `0` for unspecified.
    #[serde(default)]
    pub order: Option<i32>,
}

impl SettingDef {
    /// Bare-minimum constructor for tests and generated defaults.
    /// Optional schema metadata is filled with `None`.
    pub fn new(ty: SettingType, default: toml::Value, identity: bool) -> Self {
        Self {
            ty,
            default,
            identity,
            label_key: None,
            description_key: None,
            label: None,
            description: None,
            group_label: None,
            min: None,
            max: None,
            step: None,
            choices: None,
            group: None,
            order: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SettingType {
    U32,
    I32,
    F32,
    String,
    Bool,
}

fn default_priority() -> u32 {
    100
}

fn default_version() -> String {
    "v0.0.0".into()
}

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Setting validation

#[derive(Debug, Clone, PartialEq)]
pub enum ValidationError {
    /// Setting value couldn't be coerced into the schema's declared
    /// type.
    BadSettingType {
        key: String,
        expected: SettingType,
        got: String,
    },
    /// Numeric setting value fell outside the manifest's `[min, max]`
    /// envelope.
    OutOfRange {
        key: String,
        got: String,
        min: Option<String>,
        max: Option<String>,
    },
    /// String setting value didn't match any entry in the manifest's
    /// `choices` allowlist.
    BadChoice {
        key: String,
        got: String,
        choices: Vec<String>,
    },
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::BadSettingType { key, expected, got } => write!(
                f,
                "plugin setting '{key}' expected type {expected:?}, got {got:?}"
            ),
            ValidationError::OutOfRange { key, got, min, max } => write!(
                f,
                "plugin setting '{key}' value {got:?} out of range (min={min:?}, max={max:?})"
            ),
            ValidationError::BadChoice { key, got, choices } => write!(
                f,
                "plugin setting '{key}' value {got:?} not in allowed choices {choices:?}"
            ),
        }
    }
}

impl std::error::Error for ValidationError {}

/// Validate an already-typecast setting against the manifest envelope.
/// Returns `Ok(())` when the value is accepted.
pub fn check_setting_bounds(
    key: &str,
    coerced: &str,
    schema: &SettingDef,
) -> std::result::Result<(), ValidationError> {
    match schema.ty {
        SettingType::U32 => {
            let v: u32 = coerced
                .parse()
                .map_err(|_| ValidationError::BadSettingType {
                    key: key.to_string(),
                    expected: SettingType::U32,
                    got: coerced.to_string(),
                })?;
            if let Some(min_v) = schema.min.as_ref().and_then(toml_to_u32) {
                if v < min_v {
                    return Err(out_of_range(key, coerced, schema));
                }
            }
            if let Some(max_v) = schema.max.as_ref().and_then(toml_to_u32) {
                if v > max_v {
                    return Err(out_of_range(key, coerced, schema));
                }
            }
            Ok(())
        }
        SettingType::I32 => {
            let v: i32 = coerced
                .parse()
                .map_err(|_| ValidationError::BadSettingType {
                    key: key.to_string(),
                    expected: SettingType::I32,
                    got: coerced.to_string(),
                })?;
            if let Some(min_v) = schema.min.as_ref().and_then(toml_to_i32) {
                if v < min_v {
                    return Err(out_of_range(key, coerced, schema));
                }
            }
            if let Some(max_v) = schema.max.as_ref().and_then(toml_to_i32) {
                if v > max_v {
                    return Err(out_of_range(key, coerced, schema));
                }
            }
            Ok(())
        }
        SettingType::F32 => {
            let v: f32 = coerced
                .parse()
                .map_err(|_| ValidationError::BadSettingType {
                    key: key.to_string(),
                    expected: SettingType::F32,
                    got: coerced.to_string(),
                })?;
            if let Some(min_v) = schema.min.as_ref().and_then(toml_to_f32) {
                if v < min_v {
                    return Err(out_of_range(key, coerced, schema));
                }
            }
            if let Some(max_v) = schema.max.as_ref().and_then(toml_to_f32) {
                if v > max_v {
                    return Err(out_of_range(key, coerced, schema));
                }
            }
            Ok(())
        }
        SettingType::String => {
            if let Some(choices) = schema.choices.as_ref() {
                if !choices.iter().any(|c| c == coerced) {
                    return Err(ValidationError::BadChoice {
                        key: key.to_string(),
                        got: coerced.to_string(),
                        choices: choices.clone(),
                    });
                }
            }
            Ok(())
        }
        SettingType::Bool => Ok(()),
    }
}

/// Top-level entry for `SettingsSet`.
/// Typechecks a raw user value and returns canonical string form.
pub fn coerce_and_validate(
    key: &str,
    raw: &str,
    schema: &SettingDef,
) -> std::result::Result<String, ValidationError> {
    let coerced =
        coerce_setting(raw, schema.ty).ok_or_else(|| ValidationError::BadSettingType {
            key: key.to_string(),
            expected: schema.ty,
            got: raw.to_string(),
        })?;
    check_setting_bounds(key, &coerced, schema)?;
    Ok(coerced)
}

fn out_of_range(key: &str, coerced: &str, schema: &SettingDef) -> ValidationError {
    ValidationError::OutOfRange {
        key: key.to_string(),
        got: coerced.to_string(),
        min: schema.min.as_ref().map(toml_value_to_display),
        max: schema.max.as_ref().map(toml_value_to_display),
    }
}

fn toml_value_to_display(v: &toml::Value) -> String {
    match v {
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn toml_to_u32(v: &toml::Value) -> Option<u32> {
    match v {
        toml::Value::Integer(i) if *i >= 0 => u32::try_from(*i).ok(),
        toml::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn toml_to_i32(v: &toml::Value) -> Option<i32> {
    match v {
        toml::Value::Integer(i) => i32::try_from(*i).ok(),
        toml::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn toml_to_f32(v: &toml::Value) -> Option<f32> {
    match v {
        toml::Value::Integer(i) => Some(*i as f32),
        toml::Value::Float(f) => Some(*f as f32),
        toml::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// Try to interpret a raw `String` as a value of `ty`.
/// Returns the canonical string form on success.
fn coerce_setting(raw: &str, ty: SettingType) -> Option<String> {
    match ty {
        SettingType::U32 => raw.parse::<u32>().ok().map(|v| v.to_string()),
        SettingType::I32 => raw.parse::<i32>().ok().map(|v| v.to_string()),
        SettingType::F32 => raw.parse::<f32>().ok().map(|v| v.to_string()),
        SettingType::Bool => match raw {
            "true" | "false" => Some(raw.to_string()),
            _ => None,
        },
        SettingType::String => Some(raw.to_string()),
    }
}

/// Return a setting's manifest default in its canonical wire form.
pub fn setting_default_value(setting: &SettingDef) -> String {
    match setting.ty {
        SettingType::U32 => match &setting.default {
            toml::Value::Integer(i) if *i >= 0 => i.to_string(),
            toml::Value::String(s) => s.clone(),
            other => other.to_string(),
        },
        SettingType::I32 => match &setting.default {
            toml::Value::Integer(i) => i.to_string(),
            toml::Value::String(s) => s.clone(),
            other => other.to_string(),
        },
        SettingType::F32 => match &setting.default {
            toml::Value::Float(f) => f.to_string(),
            toml::Value::Integer(i) => (*i as f32).to_string(),
            toml::Value::String(s) => s.clone(),
            other => other.to_string(),
        },
        SettingType::Bool => match &setting.default {
            toml::Value::Boolean(b) => b.to_string(),
            toml::Value::String(s) => s.clone(),
            other => other.to_string(),
        },
        SettingType::String => match &setting.default {
            toml::Value::String(s) => s.clone(),
            other => other.to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// Registry

#[derive(Clone, Default)]
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

    /// Find a renderer by its manifest `name`, regardless of type.
    /// Returns the first occurrence — `register` keeps duplicates
    pub fn resolve_by_name(&self, name: &str) -> Option<&RendererDef> {
        self.by_type
            .values()
            .flat_map(|v| v.iter())
            .find(|d| d.name == name)
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

/// Scan a `plugins/` directory.
/// Each immediate subdirectory with `plugin.toml` is one plugin.
pub fn scan_plugins(dir: &Path, system: bool) -> PluginScan {
    let mut out = PluginScan::default();
    let dir = normalize_path(dir);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            log::warn!("read plugins dir {}: {e}", dir.display());
            return out;
        }
    };
    for entry in entries.flatten() {
        let plugin_dir = normalize_path(&entry.path());
        if !plugin_dir.is_dir() {
            continue;
        }
        let manifest_path = plugin_dir.join("plugin.toml");
        if !manifest_path.is_file() {
            continue;
        }
        let manifest: PluginManifest = match std::fs::read_to_string(&manifest_path)
            .map_err(|e| e.to_string())
            .and_then(|c| toml::from_str(&c).map_err(|e| e.to_string()))
        {
            Ok(m) => m,
            Err(e) => {
                log::warn!("skip {}: {e}", manifest_path.display());
                continue;
            }
        };

        let mut meta = manifest.plugin;
        meta.system = system;

        // Every plugin must ship a newline-separated `files.txt` manifest.
        let files_path = plugin_dir.join(PLUGIN_FILES_MANIFEST);
        match std::fs::read_to_string(&files_path) {
            Ok(text) => meta.files.extend(
                text.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned),
            ),
            Err(e) => log::warn!(
                "plugin {}: required {} missing or unreadable ({e})",
                meta.id,
                files_path.display()
            ),
        }

        if let Some(i18n) = &meta.i18n {
            match crate::plugin::i18n::load_plugin_translation_documents(
                &plugin_dir,
                &meta.id,
                i18n,
                &meta.files,
            ) {
                Ok(documents) => meta.translations = documents,
                Err(error) => {
                    log::warn!("skip plugin {}: {error}", meta.id);
                    continue;
                }
            }
        }

        if let Some(entry) = meta.entry.take() {
            match meta.entry_version {
                // Decide here whether the declared entry ABI is one this
                // daemon implements. Queueing an entry that is certain to be
                // refused at load time is what made the refusal invisible:
                // `packages()` counts queued entries, so the plugin looked
                // exactly like a working one while its Lua never ran.
                Some(entry_version)
                    if crate::plugin::source::supports_entry_version(entry_version) =>
                {
                    out.entries.push(EntryRef {
                        plugin_id: meta.id.clone(),
                        plugin_version: meta.version.clone(),
                        plugin_system: meta.system,
                        entry: resolve_rel(&plugin_dir, entry),
                        entry_version,
                    });
                }
                Some(entry_version) => {
                    let reason = format!(
                        "plugin entry_version {entry_version} is unsupported; \
                         this daemon supports {:?}",
                        crate::plugin::source::SUPPORTED_ENTRY_VERSIONS
                    );
                    log::warn!("plugin {}: {reason}", meta.id);
                    out.incompatible.push(PluginIncompat {
                        plugin_id: meta.id.clone(),
                        reason,
                    });
                }
                None => {
                    let reason = "plugin.entry requires plugin.entry_version".to_string();
                    log::warn!("plugin {}: {reason}", meta.id);
                    out.incompatible.push(PluginIncompat {
                        plugin_id: meta.id.clone(),
                        reason,
                    });
                }
            }
        } else if meta.entry_version.is_some() {
            log::warn!(
                "plugin {}: plugin.entry_version ignored without plugin.entry",
                meta.id
            );
        }

        for (name, mut def) in manifest.renderers {
            def.name = name;
            def.plugin_id = meta.id.clone();
            def.plugin_version = meta.version.clone();
            def.plugin_system = meta.system;
            def.bin = resolve_rel(&plugin_dir, def.bin);
            if def.legacy_events.is_some() {
                log::error!(
                    "renderer {}: manifest field `events` was removed; register optional events at runtime",
                    def.name
                );
                continue;
            }
            let invalid_text = def.settings.iter().find_map(|(key, setting)| {
                for (field, value) in [
                    ("label", setting.label.as_ref()),
                    ("description", setting.description.as_ref()),
                    ("group_label", setting.group_label.as_ref()),
                ] {
                    if value.is_some_and(|value| !value.is_valid()) {
                        return Some(format!(
                            "renderer {} setting {key}.{field} requires a non-empty msgid",
                            def.name
                        ));
                    }
                }
                None
            });
            if let Some(error) = invalid_text {
                log::error!("{error}");
                continue;
            }
            for (key, setting) in &def.settings {
                if setting.label.is_none() && setting.label_key.is_some() {
                    log::warn!(
                        "renderer {} setting {key}: label_key is deprecated; use label = {{ msgid = \"...\" }}",
                        def.name
                    );
                }
                if setting.description.is_none() && setting.description_key.is_some() {
                    log::warn!(
                        "renderer {} setting {key}: description_key is deprecated; use description = {{ msgid = \"...\" }}",
                        def.name
                    );
                }
            }
            log::info!(
                "loaded renderer component: {} (plugin {}, types: {:?})",
                def.name,
                meta.id,
                def.types,
            );
            out.renderers.push(def);
        }

        log::info!(
            "loaded plugin: {} ({}) v{} ({} files)",
            meta.name,
            meta.id,
            meta.version,
            meta.files.len()
        );
        out.plugins.push(meta);
    }
    out
}

/// Join `p` against `base` when relative; pass through when absolute.
fn resolve_rel(base: &Path, p: PathBuf) -> PathBuf {
    normalize_path(&if p.is_relative() { base.join(p) } else { p })
}

pub fn scan_plugin_roots(roots: &[PluginRoot]) -> PluginScan {
    let mut scan = PluginScan::default();
    for system in [true, false] {
        for root in roots.iter().filter(|root| root.system == system) {
            if root.dir.is_dir() {
                scan.merge(scan_plugins(&root.dir, root.system));
            }
        }
    }
    select_active_plugins(scan)
}

fn select_active_plugins(mut scan: PluginScan) -> PluginScan {
    let mut selected_by_id: HashMap<String, usize> = HashMap::new();
    let mut inactive_system = BTreeSet::new();
    let mut inactive_user = BTreeSet::new();

    for (idx, meta) in scan.plugins.iter().enumerate() {
        let Some(&current_idx) = selected_by_id.get(&meta.id) else {
            selected_by_id.insert(meta.id.clone(), idx);
            continue;
        };
        let current = &scan.plugins[current_idx];
        if plugin_priority(meta, current).is_gt() {
            record_inactive(current, &mut inactive_system, &mut inactive_user);
            selected_by_id.insert(meta.id.clone(), idx);
        } else {
            record_inactive(meta, &mut inactive_system, &mut inactive_user);
        }
    }

    let active_keys: HashSet<(String, String, bool)> = selected_by_id
        .values()
        .map(|idx| {
            let meta = &scan.plugins[*idx];
            (meta.id.clone(), meta.version.clone(), meta.system)
        })
        .collect();

    scan.plugins
        .retain(|m| active_keys.contains(&(m.id.clone(), m.version.clone(), m.system)));
    scan.entries.retain(|e| {
        active_keys.contains(&(
            e.plugin_id.clone(),
            e.plugin_version.clone(),
            e.plugin_system,
        ))
    });
    scan.renderers.retain(|r| {
        active_keys.contains(&(
            r.plugin_id.clone(),
            r.plugin_version.clone(),
            r.plugin_system,
        ))
    });
    // Only report a refusal for a plugin that actually won the id: a shadowed
    // copy is already accounted for as inactive.
    let retained: HashSet<String> = scan.plugins.iter().map(|m| m.id.clone()).collect();
    scan.incompatible
        .retain(|i| retained.contains(&i.plugin_id));

    scan.inactive_system = inactive_system.into_iter().collect();
    scan.inactive_user = inactive_user.into_iter().collect();
    scan
}

fn record_inactive(
    meta: &PluginMeta,
    inactive_system: &mut BTreeSet<String>,
    inactive_user: &mut BTreeSet<String>,
) {
    if meta.system {
        inactive_system.insert(meta.id.clone());
    } else {
        inactive_user.insert(meta.id.clone());
    }
}

fn plugin_priority(a: &PluginMeta, b: &PluginMeta) -> Ordering {
    let version = version_cmp(&a.version, &b.version);
    if !version.is_eq() {
        return version;
    }
    match (a.system, b.system) {
        (false, true) => Ordering::Greater,
        (true, false) => Ordering::Less,
        _ => Ordering::Equal,
    }
}

fn version_cmp(a: &str, b: &str) -> Ordering {
    match (parse_version(a), parse_version(b)) {
        (Some(a), Some(b)) => a.cmp(&b),
        _ => a.cmp(b),
    }
}

fn parse_version(v: &str) -> Option<semver::Version> {
    semver::Version::parse(v.trim().trim_start_matches(['v', 'V'])).ok()
}

/// Return the standard scan roots (bundled + XDG) for a given subdirectory
/// name (e.g. `"plugins"` or `"displays"`).
pub fn standard_plugin_roots(subdir: &str) -> Vec<PluginRoot> {
    let mut roots = Vec::new();

    // Bundled: <exec>/../share/waywallen/<subdir>/
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            if let Some(prefix) = parent.parent() {
                roots.push(PluginRoot::system(
                    prefix.join("share/waywallen").join(subdir),
                ));
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
    roots.push(PluginRoot::user(xdg.join("waywallen").join(subdir)));

    roots
}

pub fn standard_plugin_dirs(subdir: &str) -> Vec<PathBuf> {
    standard_plugin_roots(subdir)
        .into_iter()
        .map(|root| root.dir)
        .collect()
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            Component::Normal(part) => out.push(part),
            Component::ParentDir => {
                let last = out.components().next_back();
                if matches!(last, Some(Component::Normal(_))) {
                    out.pop();
                } else if !matches!(last, Some(Component::RootDir | Component::Prefix(_))) {
                    out.push("..");
                }
            }
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod schema_tests {
    use super::*;

    fn write_test_plugin(plugins_dir: &Path, id: &str) {
        let plugin_dir = plugins_dir.join(id);
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("plugin.toml"),
            format!(
                r#"[plugin]
id = "{id}"
name = "{id}"
version = "1.0.0"

[renderers.test]
bin = "../bin/renderer"
types = ["image"]
"#
            ),
        )
        .unwrap();
        std::fs::write(plugin_dir.join("files.txt"), "plugin.toml\nfiles.txt\n").unwrap();
    }

    fn plugin_meta(id: &str, version: &str, system: bool) -> PluginMeta {
        PluginMeta {
            id: id.to_string(),
            name: id.to_string(),
            version: version.to_string(),
            update: None,
            entry: None,
            entry_version: None,
            i18n: None,
            files: Vec::new(),
            system,
            translations: Vec::new(),
        }
    }

    fn renderer(id: &str, version: &str, system: bool, name: &str) -> RendererDef {
        RendererDef {
            name: name.to_string(),
            plugin_id: id.to_string(),
            plugin_version: version.to_string(),
            plugin_system: system,
            bin: PathBuf::from("/dev/null"),
            types: vec!["image".to_string()],
            priority: 100,
            activity: RendererActivityMode::OnDemand,
            spawn_version: None,
            extras: Vec::new(),
            settings: Default::default(),
            legacy_events: None,
        }
    }

    fn test_setting(ty: SettingType, default: toml::Value, identity: bool) -> SettingDef {
        SettingDef {
            ty,
            default,
            identity,
            label_key: None,
            description_key: None,
            label: None,
            description: None,
            group_label: None,
            min: None,
            max: None,
            step: None,
            choices: None,
            group: None,
            order: None,
        }
    }

    #[test]
    fn manifest_preserves_legacy_events_for_rejection() {
        let src = r#"
            [plugin]
            id = "org.waywallen.wallpaper-engine"
            name = "Wallpaper Engine"
            update = "https://example.org/owe/update.json"

            [renderers.wescene-renderer]
            bin = "bin/waywallen-wescene-renderer"
            types = ["scene"]
            events = ["pointer", "mpris"]
        "#;
        let m: PluginManifest = toml::from_str(src).expect("parses");
        assert_eq!(
            m.plugin.update.as_deref(),
            Some("https://example.org/owe/update.json")
        );
        let r = &m.renderers["wescene-renderer"];
        assert_eq!(
            r.legacy_events,
            Some(vec!["pointer".to_string(), "mpris".to_string()])
        );
    }

    #[test]
    fn scan_rejects_renderer_with_legacy_events() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");
        write_test_plugin(&plugins, "org.test.legacy");
        std::fs::write(
            plugins.join("org.test.legacy/plugin.toml"),
            r#"[plugin]
id = "org.test.legacy"
name = "Legacy"

[renderers.test]
bin = "bin/renderer"
types = ["image"]
events = ["pointer"]
"#,
        )
        .unwrap();

        let scan = scan_plugin_roots(&[PluginRoot::system(plugins)]);
        assert_eq!(scan.plugins.len(), 1);
        assert!(scan.renderers.is_empty());
    }

    /// Writes a plugin whose manifest declares a Lua entry at `entry_version`.
    fn write_entry_plugin(plugins_dir: &Path, id: &str, entry_version: Option<u32>) {
        write_test_plugin(plugins_dir, id);
        let declared = match entry_version {
            Some(v) => format!("entry_version = {v}\n"),
            None => String::new(),
        };
        std::fs::write(
            plugins_dir.join(id).join("plugin.toml"),
            format!(
                r#"[plugin]
id = "{id}"
name = "{id}"
version = "1.0.0"
entry = "main.lua"
{declared}"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn scan_reports_unsupported_entry_version_instead_of_queueing_it() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");
        // 2 predates SUPPORTED_ENTRY_VERSIONS; loading it could only ever fail.
        write_entry_plugin(&plugins, "org.test.oldabi", Some(2));

        let scan = scan_plugin_roots(&[PluginRoot::system(plugins)]);

        assert_eq!(scan.plugins.len(), 1);
        assert!(
            scan.entries.is_empty(),
            "an entry that cannot be served must not be queued as if it could"
        );
        assert_eq!(scan.incompatible.len(), 1);
        let reason = &scan.incompatible[0].reason;
        assert!(
            reason.contains('2'),
            "reason should name what was declared: {reason}"
        );
        assert!(
            reason.contains(&format!(
                "{:?}",
                crate::plugin::source::SUPPORTED_ENTRY_VERSIONS
            )),
            "reason should name what the daemon supports: {reason}"
        );

        // The package view is what a client reads; it must not look healthy.
        let pkg = &scan.packages()[0];
        assert!(!pkg.has_entry);
        assert_eq!(pkg.incompat.as_deref(), Some(reason.as_str()));
    }

    #[test]
    fn scan_reports_entry_declared_without_a_version() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");
        write_entry_plugin(&plugins, "org.test.noabi", None);

        let scan = scan_plugin_roots(&[PluginRoot::system(plugins)]);

        assert!(scan.entries.is_empty());
        assert_eq!(scan.incompatible.len(), 1);
        assert_eq!(
            scan.packages()[0].incompat.as_deref(),
            Some("plugin.entry requires plugin.entry_version")
        );
    }

    #[test]
    fn scan_keeps_a_supported_entry_version_compatible() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");
        write_entry_plugin(
            &plugins,
            "org.test.currentabi",
            Some(crate::plugin::source::ENTRY_VERSION),
        );

        let scan = scan_plugin_roots(&[PluginRoot::system(plugins)]);

        assert_eq!(scan.entries.len(), 1);
        assert!(scan.incompatible.is_empty());
        let pkg = &scan.packages()[0];
        assert!(pkg.has_entry);
        assert!(pkg.incompat.is_none());
    }

    #[test]
    fn scan_plugin_roots_scans_system_before_user() {
        let tmp = tempfile::tempdir().unwrap();
        let system_plugins = tmp.path().join("system/plugins");
        let user_plugins = tmp.path().join("user/plugins");
        write_test_plugin(&system_plugins, "org.test.system");
        write_test_plugin(&user_plugins, "org.test.user");

        let scan = scan_plugin_roots(&[
            PluginRoot::user(user_plugins),
            PluginRoot::system(system_plugins),
        ]);

        let ids: Vec<_> = scan.plugins.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["org.test.system", "org.test.user"]);
    }

    #[test]
    fn scan_loads_only_declared_plugin_translations() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");
        let plugin_dir = plugins.join("org.test.localized");
        std::fs::create_dir_all(plugin_dir.join("i18n")).unwrap();
        std::fs::write(
            plugin_dir.join("plugin.toml"),
            r#"[plugin]
id = "org.test.localized"
name = "Localized"

[plugin.i18n]
directory = "i18n"
"#,
        )
        .unwrap();
        std::fs::write(
            plugin_dir.join("files.txt"),
            "plugin.toml\nfiles.txt\ni18n/zh-CN.po\ni18n/ru.po\n",
        )
        .unwrap();
        std::fs::write(plugin_dir.join("i18n/zh-CN.po"), b"zh").unwrap();
        std::fs::write(plugin_dir.join("i18n/ru.po"), b"ru").unwrap();
        std::fs::write(plugin_dir.join("i18n/de.po"), b"not declared").unwrap();

        let scan = scan_plugin_roots(&[PluginRoot::system(plugins)]);
        let documents = scan.translation_documents().unwrap();
        assert_eq!(scan.plugins.len(), 1);
        assert_eq!(documents.len(), 2);
        assert_eq!(documents[0].plugin_id, "org.test.localized");
        assert_eq!(documents[0].locale, "ru");
        assert_eq!(documents[0].po, b"ru");
        assert_eq!(documents[1].locale, "zh-CN");
        assert_eq!(documents[1].po, b"zh");
    }

    #[test]
    fn scan_rejects_translation_directory_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");
        let plugin_dir = plugins.join("org.test.localized");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("plugin.toml"),
            r#"[plugin]
id = "org.test.localized"
name = "Localized"

[plugin.i18n]
directory = "../i18n"
"#,
        )
        .unwrap();
        std::fs::write(plugin_dir.join("files.txt"), "plugin.toml\nfiles.txt\n").unwrap();

        let scan = scan_plugin_roots(&[PluginRoot::system(plugins)]);
        assert!(scan.plugins.is_empty());
        assert!(scan.translation_documents().unwrap().is_empty());
    }

    #[test]
    fn plugin_root_owner_and_paths_are_lexical() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("external/plugins");
        write_test_plugin(&plugins, "org.test.plugin");

        let root = PluginRoot::user(tmp.path().join("external/../external/plugins"));
        assert_eq!(root.dir, plugins);

        let scan = scan_plugin_roots(&[root]);
        assert_eq!(scan.plugins.len(), 1);
        assert!(!scan.plugins[0].system);
        assert_eq!(scan.renderers.len(), 1);
        assert!(!scan.renderers[0].plugin_system);
        assert_eq!(
            scan.renderers[0].bin,
            tmp.path().join("external/plugins/bin/renderer")
        );
    }

    #[test]
    fn user_plugin_wins_equal_version() {
        let scan = select_active_plugins(PluginScan {
            plugins: vec![
                plugin_meta("org.test.plugin", "1.0.0", true),
                plugin_meta("org.test.plugin", "1.0.0", false),
            ],
            renderers: vec![
                renderer("org.test.plugin", "1.0.0", true, "system-renderer"),
                renderer("org.test.plugin", "1.0.0", false, "user-renderer"),
            ],
            ..Default::default()
        });

        assert_eq!(scan.plugins.len(), 1);
        assert!(!scan.plugins[0].system);
        assert_eq!(scan.renderers.len(), 1);
        assert_eq!(scan.renderers[0].name, "user-renderer");
        assert_eq!(scan.inactive_system, vec!["org.test.plugin".to_string()]);
        assert!(scan.inactive_user.is_empty());
    }

    #[test]
    fn higher_system_version_wins_over_user() {
        let scan = select_active_plugins(PluginScan {
            plugins: vec![
                plugin_meta("org.test.plugin", "2.0.0", true),
                plugin_meta("org.test.plugin", "1.0.0", false),
            ],
            renderers: vec![
                renderer("org.test.plugin", "2.0.0", true, "system-renderer"),
                renderer("org.test.plugin", "1.0.0", false, "user-renderer"),
            ],
            ..Default::default()
        });

        assert_eq!(scan.plugins.len(), 1);
        assert!(scan.plugins[0].system);
        assert_eq!(scan.renderers.len(), 1);
        assert_eq!(scan.renderers[0].name, "system-renderer");
        assert_eq!(scan.inactive_user, vec!["org.test.plugin".to_string()]);
        assert!(scan.inactive_system.is_empty());
    }

    #[test]
    fn manifest_legacy_events_default_none() {
        let src = r#"
            [plugin]
            id = "org.waywallen.image"
            name = "Image"

            [renderers.waywallen-image]
            bin = "bin/waywallen-image-renderer"
            types = ["image"]
        "#;
        let m: PluginManifest = toml::from_str(src).expect("parses");
        assert!(m.renderers["waywallen-image"].legacy_events.is_none());
    }

    #[test]
    fn manifest_parses_entry_extras_and_settings() {
        // End-to-end: plugin.toml to Lua entry metadata and RendererDef.
        // Wire-level details are asserted by focused tests elsewhere.
        let src = r#"
            [plugin]
            id = "org.waywallen.mpv"
            name = "mpv"
            entry = "mpv.lua"
            entry_version = 4

            [renderers.waywallen-mpv]
            bin = "bin/waywallen-mpv-renderer"
            types = ["video"]
            priority = 100
            spawn_version = 1
            extras = ["subtitle"]

            [renderers.waywallen-mpv.settings]
            loop_file = { type = "string", default = "inf",  identity = false }
            hwdec     = { type = "string", default = "auto", identity = false }
        "#;
        let m: PluginManifest = toml::from_str(src).expect("manifest parses");
        assert_eq!(m.plugin.entry.as_ref().unwrap(), &PathBuf::from("mpv.lua"));
        assert_eq!(m.plugin.entry_version, Some(4));
        let r = &m.renderers["waywallen-mpv"];
        assert_eq!(r.spawn_version, Some(1));
        assert_eq!(r.extras, vec!["subtitle".to_string()]);
        assert_eq!(r.settings.len(), 2);
    }

    #[test]
    fn coerce_and_validate_u32_in_range() {
        let s = SettingDef {
            min: Some(toml::Value::Integer(0)),
            max: Some(toml::Value::Integer(100)),
            ..test_setting(SettingType::U32, toml::Value::Integer(50), false)
        };
        assert_eq!(coerce_and_validate("volume", "75", &s).unwrap(), "75");
        // boundaries inclusive
        assert_eq!(coerce_and_validate("volume", "0", &s).unwrap(), "0");
        assert_eq!(coerce_and_validate("volume", "100", &s).unwrap(), "100");
    }

    #[test]
    fn coerce_and_validate_u32_out_of_range_errors() {
        let s = SettingDef {
            min: Some(toml::Value::Integer(0)),
            max: Some(toml::Value::Integer(100)),
            ..test_setting(SettingType::U32, toml::Value::Integer(50), false)
        };
        let err = coerce_and_validate("volume", "500", &s).expect_err("must error");
        assert!(matches!(err, ValidationError::OutOfRange { ref key, .. } if key == "volume"));
    }

    #[test]
    fn coerce_and_validate_i32_in_range() {
        let s = SettingDef {
            min: Some(toml::Value::Integer(-1)),
            max: Some(toml::Value::Integer(4)),
            ..test_setting(SettingType::I32, toml::Value::Integer(2), false)
        };
        assert_eq!(coerce_and_validate("resolution", "-1", &s).unwrap(), "-1");
        assert_eq!(coerce_and_validate("resolution", "4", &s).unwrap(), "4");
        assert!(matches!(
            coerce_and_validate("resolution", "-2", &s),
            Err(ValidationError::OutOfRange { .. })
        ));
    }

    #[test]
    fn coerce_and_validate_f32_bounds() {
        let s = SettingDef {
            min: Some(toml::Value::Float(0.0)),
            max: Some(toml::Value::Float(1.5)),
            ..test_setting(SettingType::F32, toml::Value::Float(1.0), false)
        };
        assert!(coerce_and_validate("ratio", "0.75", &s).is_ok());
        assert!(matches!(
            coerce_and_validate("ratio", "2.0", &s),
            Err(ValidationError::OutOfRange { .. })
        ));
        assert!(matches!(
            coerce_and_validate("ratio", "-0.1", &s),
            Err(ValidationError::OutOfRange { .. })
        ));
    }

    #[test]
    fn coerce_and_validate_choices_hit_and_miss() {
        let s = SettingDef {
            choices: Some(vec!["auto".into(), "vaapi".into(), "nvdec".into()]),
            ..test_setting(
                SettingType::String,
                toml::Value::String("auto".into()),
                false,
            )
        };
        assert_eq!(coerce_and_validate("hwdec", "vaapi", &s).unwrap(), "vaapi");
        let err = coerce_and_validate("hwdec", "ssh", &s).expect_err("must error");
        assert!(matches!(
            err,
            ValidationError::BadChoice { ref key, .. } if key == "hwdec"
        ));
    }

    #[test]
    fn coerce_and_validate_bad_type_errors() {
        let s = test_setting(SettingType::U32, toml::Value::Integer(0), false);
        let err = coerce_and_validate("fps", "lots", &s).expect_err("must error");
        assert!(matches!(err, ValidationError::BadSettingType { .. }));
    }
}
