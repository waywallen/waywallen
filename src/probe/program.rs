use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_MODULE_SIZE: u64 = 1024 * 1024;
// Bump when native Lua probe semantics change without changing plugin files.
const LUA_CACHE_VERSION: u32 = 2;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Declaration {
    pub entry: PathBuf,
    pub version: u32,
    pub types: Vec<String>,
}

impl Declaration {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            bail!("plugin.probe.version must be 1");
        }
        relative(&self.entry)?;
        if self.entry.extension().is_none_or(|s| s != "lua") {
            bail!("plugin.probe.entry must be a Lua file");
        }
        if self.types.is_empty()
            || self.types.len() > 32
            || self
                .types
                .iter()
                .any(|t| t.is_empty() || t.len() > 64 || t.trim() != t)
            || self
                .types
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != self.types.len()
        {
            bail!("plugin.probe.types must contain supported wallpaper type names");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Program {
    pub key: String,
    pub root: PathBuf,
    pub entry: PathBuf,
    pub files: Vec<String>,
    pub version: u32,
    pub types: Vec<String>,
}

pub fn relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || !path.components().all(|c| matches!(c, Component::Normal(_)))
        || path
            .to_str()
            .is_none_or(|p| p.contains(['\\', '\0']) || p.len() > 16384)
    {
        bail!("probe path must be a relative UTF-8 path inside the plugin");
    }
    Ok(())
}

fn module_path(root: &Path, script: &str) -> Result<PathBuf> {
    relative(Path::new(script))?;
    let mut path = root.to_owned();
    for component in Path::new(script).components() {
        path.push(component);
        if std::fs::symlink_metadata(&path)?.file_type().is_symlink() {
            bail!("probe modules must not contain symlinks");
        }
    }
    let metadata = std::fs::metadata(&path)?;
    if !metadata.is_file()
        || metadata.len() > MAX_MODULE_SIZE
        || !path.canonicalize()?.starts_with(root)
    {
        bail!("probe module must be a file inside the plugin");
    }
    Ok(path)
}

fn module_bytes(root: &Path, script: &str) -> Result<Vec<u8>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(module_path(root, script)?)?;
    if !file.metadata()?.is_file() {
        bail!("probe module is not a regular file");
    }
    let mut bytes = Vec::new();
    file.take(MAX_MODULE_SIZE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_MODULE_SIZE {
        bail!("probe module is too large");
    }
    Ok(bytes)
}

impl Program {
    pub fn cache_key(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(b"waywallen-probe-lua-cache\0");
        hash.update(LUA_CACHE_VERSION.to_le_bytes());
        hash.update(self.key.as_bytes());
        for script in &self.files {
            hash.update((script.len() as u64).to_le_bytes());
            hash.update(script.as_bytes());
            match module_bytes(&self.root, script) {
                Ok(bytes) => {
                    hash.update([1]);
                    hash.update((bytes.len() as u64).to_le_bytes());
                    hash.update(bytes);
                }
                Err(_) => hash.update([0]),
            }
        }
        format!("{:x}", hash.finalize())
    }

    pub fn open(
        id: &str,
        root: &Path,
        declaration: &Declaration,
        files: &[String],
    ) -> Result<Self> {
        declaration.validate()?;
        let entry = declaration
            .entry
            .to_str()
            .context("probe entry is not UTF-8")?;
        if !files.iter().any(|f| f == entry) {
            bail!("plugin.probe.entry must be listed in files.txt");
        }
        let root = root.canonicalize()?;
        let mut scripts: Vec<_> = files
            .iter()
            .filter(|f| f.ends_with(".lua"))
            .cloned()
            .collect();
        scripts.sort();
        scripts.dedup();
        if scripts.len() > 256 || scripts.iter().map(String::len).sum::<usize>() > 262144 {
            bail!("too many probe modules");
        }
        for script in &scripts {
            module_path(&root, script)?;
        }
        if !scripts.iter().any(|s| s == entry) {
            bail!("probe entry must be a Lua file");
        }
        let identity = serde_json::to_vec(&(id, &root, declaration, &scripts))?;
        Ok(Self {
            key: format!("{:x}", Sha256::digest(identity)),
            root,
            entry: declaration.entry.clone(),
            files: scripts,
            version: declaration.version,
            types: declaration.types.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declaration(entry: &str) -> Declaration {
        Declaration {
            entry: entry.into(),
            version: 1,
            types: vec!["scene".into()],
        }
    }

    #[test]
    fn declarations_reject_invalid_paths_versions_and_types() {
        for entry in [
            "",
            "../probe.lua",
            "/tmp/probe.lua",
            "a\\probe.lua",
            "probe.so",
        ] {
            assert!(declaration(entry).validate().is_err(), "{entry}");
        }
        let mut definition = declaration("probe.lua");
        definition.version = 2;
        assert!(definition.validate().is_err());
        definition.version = 1;
        definition.types.clear();
        assert!(definition.validate().is_err());
        definition.types = vec!["scene".into(), "scene".into()];
        assert!(definition.validate().is_err());
    }

    #[test]
    fn program_files_are_declared_local_and_have_stable_identity() {
        let root = tempfile::tempdir().unwrap();
        let declaration = declaration("probe.lua");
        std::fs::write(root.path().join("probe.lua"), "return {}").unwrap();
        assert!(Program::open("test", root.path(), &declaration, &[]).is_err());
        let files = vec!["probe.lua".to_string()];
        let first = Program::open("test", root.path(), &declaration, &files).unwrap();
        std::fs::write(
            root.path().join("probe.lua"),
            "return {probe=function() end}",
        )
        .unwrap();
        assert_eq!(
            first.key,
            Program::open("test", root.path(), &declaration, &files)
                .unwrap()
                .key
        );
        std::fs::remove_file(root.path().join("probe.lua")).unwrap();
        std::os::unix::fs::symlink("/tmp/outside.lua", root.path().join("probe.lua")).unwrap();
        assert!(Program::open("test", root.path(), &declaration, &files).is_err());
    }

    #[test]
    fn cache_keys_include_module_contents_without_file_metadata() {
        let root = tempfile::tempdir().unwrap();
        for name in ["probe.lua", "helper.lua"] {
            std::fs::write(root.path().join(name), "return 1").unwrap();
        }
        let program = Program::open(
            "test",
            root.path(),
            &declaration("probe.lua"),
            &["probe.lua".into(), "helper.lua".into()],
        )
        .unwrap();
        let initial = program.cache_key();
        assert_eq!(program.cache_key(), initial);
        let reordered = Program::open(
            "test",
            root.path(),
            &declaration("probe.lua"),
            &["helper.lua".into(), "probe.lua".into()],
        )
        .unwrap();
        assert_eq!(reordered.cache_key(), initial);
        for name in ["probe.lua", "helper.lua"] {
            super::super::tests::rewrite_preserving_stat(&root.path().join(name), b"return 2");
            assert_ne!(program.cache_key(), initial);
            super::super::tests::rewrite_preserving_stat(&root.path().join(name), b"return 1");
            assert_eq!(program.cache_key(), initial);
        }
        let file = std::fs::File::options()
            .write(true)
            .open(root.path().join("probe.lua"))
            .unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::UNIX_EPOCH))
            .unwrap();
        assert_eq!(program.cache_key(), initial);
        std::fs::remove_file(root.path().join("helper.lua")).unwrap();
        let missing = program.cache_key();
        assert_ne!(missing, initial);
        assert_eq!(program.cache_key(), missing);
        std::fs::write(root.path().join("helper.lua"), "return 1").unwrap();
        assert_eq!(program.cache_key(), initial);
    }
}
