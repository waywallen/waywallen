use anyhow::Result;
use mlua::prelude::*;
use std::collections::HashMap;
use std::path::Path;

use crate::wallpaper_type::{WallpaperEntry, WallpaperType};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct SourcePluginInfo {
    pub name: String,
    pub types: Vec<WallpaperType>,
    pub version: String,
}

// ---------------------------------------------------------------------------
// SourceManager
// ---------------------------------------------------------------------------

pub struct SourceManager {
    lua: Lua,
    /// plugin name → registry key for the loaded module table.
    plugins: HashMap<String, LuaRegistryKey>,
    /// Flattened scan results from all plugins.
    entries: Vec<WallpaperEntry>,
    /// Index: wp_type → indices into `entries`.
    by_type: HashMap<WallpaperType, Vec<usize>>,
    /// Daemon config exposed to Lua via `ctx.config(key)`.
    config: HashMap<String, String>,
}

// mlua with the `send` feature makes Lua: Send.
// We wrap SourceManager in Arc<TokioMutex<>> so this is required.
const _: () = {
    fn assert_send<T: Send>() {}
    fn check() {
        assert_send::<SourceManager>();
    }
};

impl SourceManager {
    pub fn new(config: HashMap<String, String>) -> Result<Self> {
        let lua = Lua::new();
        Ok(Self {
            lua,
            plugins: HashMap::new(),
            entries: Vec::new(),
            by_type: HashMap::new(),
            config,
        })
    }

    /// Load a single `.lua` source plugin. Returns the plugin name.
    pub fn load_plugin(&mut self, path: &Path) -> Result<String> {
        let source = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))?;
        let module: LuaTable = self
            .lua
            .load(&source)
            .set_name(path.to_string_lossy())
            .eval()
            .map_err(|e| anyhow::anyhow!("eval {}: {e}", path.display()))?;

        // Call info() to get plugin metadata.
        let info_fn: LuaFunction = module
            .get("info")
            .map_err(|e| anyhow::anyhow!("plugin must export info(): {e}"))?;
        let info_table: LuaTable = info_fn
            .call(())
            .map_err(|e| anyhow::anyhow!("info() failed: {e}"))?;
        let name: String = info_table
            .get("name")
            .map_err(|e| anyhow::anyhow!("info().name required: {e}"))?;

        let key = self.lua.create_registry_value(module)?;
        self.plugins.insert(name.clone(), key);
        log::info!("loaded source plugin: {name} from {}", path.display());
        Ok(name)
    }

    /// Scan a directory for `*.lua` plugin files and load all of them.
    pub fn load_all(&mut self, dir: &Path) -> Result<Vec<String>> {
        let pattern = dir.join("*.lua");
        let pattern_str = pattern
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("source dir path not valid UTF-8"))?;
        let mut names = Vec::new();
        for entry in glob::glob(pattern_str).map_err(|e| anyhow::anyhow!("glob: {e}"))? {
            match entry {
                Ok(path) => match self.load_plugin(&path) {
                    Ok(name) => names.push(name),
                    Err(e) => log::warn!("skip {}: {e}", path.display()),
                },
                Err(e) => log::warn!("source plugin glob error: {e}"),
            }
        }
        Ok(names)
    }

    /// Run `scan(ctx)` on all loaded plugins and merge results.
    pub fn scan_all(&mut self) -> Result<()> {
        self.entries.clear();
        self.by_type.clear();

        let plugin_names: Vec<String> = self.plugins.keys().cloned().collect();
        for name in &plugin_names {
            if let Err(e) = self.scan_plugin(name) {
                log::warn!("scan plugin {name} failed: {e}");
            }
        }
        Ok(())
    }

    /// Run `scan(ctx)` on a single plugin by name.
    fn scan_plugin(&mut self, name: &str) -> Result<()> {
        let key = self
            .plugins
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("unknown plugin"))?;
        let module: LuaTable = self.lua.registry_value(key)?;
        let scan_fn: LuaFunction = module
            .get("scan")
            .map_err(|e| anyhow::anyhow!("plugin must export scan(ctx): {e}"))?;

        let ctx = self.build_ctx()?;
        let results: LuaTable = scan_fn.call(ctx)?;

        for pair in results.sequence_values::<LuaTable>() {
            let tbl = pair?;
            let entry = WallpaperEntry {
                id: tbl.get("id").unwrap_or_default(),
                name: tbl.get("name").unwrap_or_default(),
                wp_type: tbl.get("wp_type").unwrap_or_default(),
                resource: tbl.get("resource").unwrap_or_default(),
                preview: tbl.get::<String>("preview").ok(),
                metadata: parse_lua_string_map(&tbl, "metadata"),
                plugin_name: name.to_owned(),
                library_root: tbl.get("library_root").unwrap_or_default(),
                description: tbl.get::<String>("description").ok(),
                tags: tbl.get::<Vec<String>>("tags").unwrap_or_default(),
                external_id: tbl.get::<String>("external_id").ok(),
            };
            let idx = self.entries.len();
            self.by_type
                .entry(entry.wp_type.clone())
                .or_default()
                .push(idx);
            self.entries.push(entry);
        }
        Ok(())
    }

    /// Build the `ctx` table passed to Lua `scan(ctx)`.
    fn build_ctx(&self) -> Result<LuaTable> {
        let ctx = self.lua.create_table()?;

        // ctx.glob(pattern) -> list of file paths
        let glob_fn = self.lua.create_function(|lua, pattern: String| {
            let paths = lua.create_table()?;
            let mut i = 1;
            if let Ok(entries) = glob::glob(&pattern) {
                for entry in entries.flatten() {
                    if let Some(s) = entry.to_str() {
                        paths.set(i, s.to_string())?;
                        i += 1;
                    }
                }
            }
            Ok(paths)
        })?;
        ctx.set("glob", glob_fn)?;

        // ctx.list_dirs(path) -> list of subdirectory paths
        let list_dirs_fn = self.lua.create_function(|lua, path: String| {
            let dirs = lua.create_table()?;
            let mut i = 1;
            if let Ok(entries) = std::fs::read_dir(&path) {
                for entry in entries.flatten() {
                    if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        if let Some(s) = entry.path().to_str() {
                            dirs.set(i, s.to_string())?;
                            i += 1;
                        }
                    }
                }
            }
            Ok(dirs)
        })?;
        ctx.set("list_dirs", list_dirs_fn)?;

        // ctx.file_exists(path) -> bool
        let file_exists_fn = self.lua.create_function(|_, path: String| {
            Ok(std::path::Path::new(&path).exists())
        })?;
        ctx.set("file_exists", file_exists_fn)?;

        // ctx.read_file(path) -> string|nil (capped at 1MB)
        let read_file_fn = self.lua.create_function(|lua, path: String| {
            match std::fs::metadata(&path) {
                Ok(meta) if meta.len() > 1_048_576 => Ok(mlua::Value::Nil),
                Ok(_) => match std::fs::read_to_string(&path) {
                    Ok(s) => Ok(mlua::Value::String(lua.create_string(&s)?)),
                    Err(_) => Ok(mlua::Value::Nil),
                },
                Err(_) => Ok(mlua::Value::Nil),
            }
        })?;
        ctx.set("read_file", read_file_fn)?;

        // ctx.extension(path) -> string|nil
        let extension_fn = self.lua.create_function(|_, path: String| {
            Ok(std::path::Path::new(&path)
                .extension()
                .and_then(|e| e.to_str())
                .map(String::from))
        })?;
        ctx.set("extension", extension_fn)?;

        // ctx.filename(path) -> string|nil
        let filename_fn = self.lua.create_function(|_, path: String| {
            Ok(std::path::Path::new(&path)
                .file_name()
                .and_then(|e| e.to_str())
                .map(String::from))
        })?;
        ctx.set("filename", filename_fn.clone())?;

        // ctx.basename(path) -> string|nil (same as filename on dirs)
        ctx.set("basename", filename_fn)?;

        // ctx.env(name) -> string|nil
        let env_fn = self.lua.create_function(|_, name: String| {
            Ok(std::env::var(&name).ok())
        })?;
        ctx.set("env", env_fn)?;

        // ctx.config(key) -> string|nil
        let config = self.config.clone();
        let config_fn = self.lua.create_function(move |_, key: String| {
            Ok(config.get(&key).cloned())
        })?;
        ctx.set("config", config_fn)?;

        // ctx.json_parse(str) -> table|nil
        let json_parse_fn = self.lua.create_function(|lua, s: String| {
            match serde_json::from_str::<serde_json::Value>(&s) {
                Ok(val) => json_to_lua(lua, &val),
                Err(_) => Ok(mlua::Value::Nil),
            }
        })?;
        ctx.set("json_parse", json_parse_fn)?;

        // ctx.log(msg)
        let log_fn = self.lua.create_function(|_, msg: String| {
            log::info!("[lua] {msg}");
            Ok(())
        })?;
        ctx.set("log", log_fn)?;

        Ok(ctx)
    }

    // -----------------------------------------------------------------------
    // Query API
    // -----------------------------------------------------------------------

    pub fn list(&self) -> &[WallpaperEntry] {
        &self.entries
    }

    pub fn list_by_type(&self, wp_type: &str) -> Vec<&WallpaperEntry> {
        self.by_type
            .get(wp_type)
            .map(|indices| indices.iter().map(|&i| &self.entries[i]).collect())
            .unwrap_or_default()
    }

    pub fn get(&self, id: &str) -> Option<&WallpaperEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn plugins(&self) -> Result<Vec<SourcePluginInfo>> {
        let mut out = Vec::new();
        for (name, key) in &self.plugins {
            let module: LuaTable = self.lua.registry_value(key)?;
            let info_fn: LuaFunction = module.get("info")?;
            let info: LuaTable = info_fn.call(())?;
            let types: Vec<String> = info
                .get::<LuaTable>("types")
                .map(|t| {
                    t.sequence_values::<String>()
                        .filter_map(|v| v.ok())
                        .collect()
                })
                .unwrap_or_default();
            let version: String = info.get("version").unwrap_or_else(|_| "0.0.0".into());
            out.push(SourcePluginInfo {
                name: name.clone(),
                types,
                version,
            });
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_lua_string_map(tbl: &LuaTable, key: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Ok(meta) = tbl.get::<LuaTable>(key) {
        for pair in meta.pairs::<String, String>() {
            if let Ok((k, v)) = pair {
                map.insert(k, v);
            }
        }
    }
    map
}

fn json_to_lua(lua: &Lua, val: &serde_json::Value) -> LuaResult<LuaValue> {
    match val {
        serde_json::Value::Null => Ok(LuaValue::Nil),
        serde_json::Value::Bool(b) => Ok(LuaValue::Boolean(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(LuaValue::Integer(i))
            } else {
                Ok(LuaValue::Number(n.as_f64().unwrap_or(0.0)))
            }
        }
        serde_json::Value::String(s) => Ok(LuaValue::String(lua.create_string(s)?)),
        serde_json::Value::Array(arr) => {
            let t = lua.create_table()?;
            for (i, v) in arr.iter().enumerate() {
                t.set(i + 1, json_to_lua(lua, v)?)?;
            }
            Ok(LuaValue::Table(t))
        }
        serde_json::Value::Object(obj) => {
            let t = lua.create_table()?;
            for (k, v) in obj {
                t.set(k.as_str(), json_to_lua(lua, v)?)?;
            }
            Ok(LuaValue::Table(t))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_load_and_scan_plugin() {
        let dir = tempfile::tempdir().unwrap();

        // Write a minimal source plugin
        let plugin_path = dir.path().join("test_source.lua");
        let mut f = std::fs::File::create(&plugin_path).unwrap();
        write!(
            f,
            r#"
local M = {{}}
function M.info()
    return {{ name = "test", types = {{"image"}}, version = "1.0" }}
end
function M.scan(ctx)
    return {{
        {{ id = "w1", name = "Test Wallpaper", wp_type = "image",
           resource = "/tmp/test.png", metadata = {{}} }},
    }}
end
return M
"#
        )
        .unwrap();

        let mut mgr = SourceManager::new(HashMap::new()).unwrap();
        let name = mgr.load_plugin(&plugin_path).unwrap();
        assert_eq!(name, "test");

        mgr.scan_all().unwrap();
        assert_eq!(mgr.list().len(), 1);
        assert_eq!(mgr.list()[0].id, "w1");
        assert_eq!(mgr.list()[0].wp_type, "image");
        assert_eq!(mgr.list()[0].plugin_name, "test");

        let by_type = mgr.list_by_type("image");
        assert_eq!(by_type.len(), 1);

        let by_type_empty = mgr.list_by_type("video");
        assert!(by_type_empty.is_empty());

        let found = mgr.get("w1");
        assert!(found.is_some());

        let plugins = mgr.plugins().unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "test");
    }
}
