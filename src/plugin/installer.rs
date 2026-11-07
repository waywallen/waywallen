use crate::error::{Error, Result};
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone)]
pub struct PluginArchiveInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub update: Option<String>,
    pub has_source: bool,
    pub renderers: Vec<String>,
}

fn install_fail(msg: impl Into<String>) -> Error {
    Error::PluginInstallFailed(msg.into())
}

fn delete_fail(msg: impl Into<String>) -> Error {
    Error::PluginDeleteFailed(msg.into())
}

fn user_plugins_dir_path() -> Option<PathBuf> {
    crate::plugin::renderer_registry::standard_plugin_dirs("plugins")
        .into_iter()
        .next_back()
}

fn user_plugins_dir() -> Result<PathBuf> {
    user_plugins_dir_path().ok_or_else(|| install_fail("no user plugin directory"))
}

fn read_plugin_id(manifest: &Path) -> std::result::Result<String, String> {
    #[derive(serde::Deserialize)]
    struct Manifest {
        plugin: Plugin,
    }
    #[derive(serde::Deserialize)]
    struct Plugin {
        id: String,
    }
    let text = std::fs::read_to_string(manifest).map_err(|e| e.to_string())?;
    let m: Manifest = toml::from_str(&text).map_err(|e| format!("parse plugin.toml: {e}"))?;
    Ok(m.plugin.id)
}

fn read_plugin_info_from_text(text: &str) -> std::result::Result<PluginArchiveInfo, String> {
    let manifest: crate::plugin::renderer_registry::PluginManifest =
        toml::from_str(text).map_err(|e| format!("parse plugin.toml: {e}"))?;
    plugin_info(&manifest)
}

fn plugin_info(
    manifest: &crate::plugin::renderer_registry::PluginManifest,
) -> std::result::Result<PluginArchiveInfo, String> {
    match (
        manifest.plugin.entry.as_ref(),
        manifest.plugin.entry_version,
    ) {
        (Some(_), Some(version)) if crate::plugin::source::supports_entry_version(version) => {}
        (Some(_), Some(version)) => {
            return Err(format!(
                "entry_version {version} is unsupported; supported versions are {:?}",
                crate::plugin::source::SUPPORTED_ENTRY_VERSIONS
            ));
        }
        (Some(_), None) => return Err("plugin.entry requires plugin.entry_version".into()),
        (None, _) => {}
    }
    let mut renderers: Vec<_> = manifest.renderers.keys().cloned().collect();
    if let Some(probe) = &manifest.plugin.probe {
        probe.validate().map_err(|e| e.to_string())?;
    }
    renderers.sort();
    Ok(PluginArchiveInfo {
        id: manifest.plugin.id.clone(),
        name: manifest.plugin.name.clone(),
        version: manifest.plugin.version.clone(),
        update: manifest.plugin.update.clone(),
        has_source: manifest.plugin.entry.is_some(),
        renderers,
    })
}

fn read_plugin_info(manifest: &Path) -> std::result::Result<PluginArchiveInfo, String> {
    let text = std::fs::read_to_string(manifest).map_err(|e| e.to_string())?;
    read_plugin_info_from_text(&text)
}

fn validate_plugin_dir_name(id: &str) -> Result<()> {
    if id.is_empty() {
        return Err(install_fail("plugin id is empty"));
    }
    let mut components = Path::new(id).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) if !id.contains(['/', '\\']) => Ok(()),
        _ => Err(install_fail(format!("unsafe plugin id '{id}'"))),
    }
}

fn apply_zip_mode<R: std::io::Read>(entry: &zip::read::ZipFile<'_, R>, path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(mode) = entry.unix_mode() {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
                .map_err(|e| install_fail(format!("chmod {}: {e}", path.display())))?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (entry, path);
    }
    Ok(())
}

fn remove_existing(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path)
            .map_err(|e| install_fail(format!("delete {}: {e}", path.display()))),
        Ok(_) => std::fs::remove_file(path)
            .map_err(|e| install_fail(format!("delete {}: {e}", path.display()))),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(install_fail(format!("stat {}: {e}", path.display()))),
    }
}

pub fn inspect_zip(zip_path: &str) -> Result<PluginArchiveInfo> {
    let file =
        std::fs::File::open(zip_path).map_err(|e| install_fail(format!("open {zip_path}: {e}")))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| install_fail(format!("read zip: {e}")))?;
    let mut manifest = archive
        .by_name("plugin.toml")
        .map_err(|_| install_fail("archive must contain plugin.toml at root"))?;
    let mut text = String::new();
    use std::io::Read;
    manifest
        .read_to_string(&mut text)
        .map_err(|e| install_fail(format!("read plugin.toml: {e}")))?;
    let info = read_plugin_info_from_text(&text).map_err(install_fail)?;
    validate_plugin_dir_name(&info.id)?;
    Ok(info)
}

pub fn inspect_user_plugin(plugin_id: &str) -> Result<Option<PluginArchiveInfo>> {
    validate_plugin_dir_name(plugin_id)?;
    let Some(dest_root) = user_plugins_dir_path() else {
        return Ok(None);
    };
    let manifest = dest_root.join(plugin_id).join("plugin.toml");
    if !manifest.is_file() {
        return Ok(None);
    }
    let info = read_plugin_info(&manifest).map_err(install_fail)?;
    Ok(Some(info))
}

/// Extract a plugin `.zip` into the user plugin directory and return the
/// installed plugin id.
pub fn install_zip(zip_path: &str) -> Result<String> {
    let file =
        std::fs::File::open(zip_path).map_err(|e| install_fail(format!("open {zip_path}: {e}")))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| install_fail(format!("read zip: {e}")))?;

    let dest_root = user_plugins_dir()?;
    std::fs::create_dir_all(&dest_root)
        .map_err(|e| install_fail(format!("mkdir {}: {e}", dest_root.display())))?;

    let staging_root = dest_root.join(".install-tmp");
    std::fs::create_dir_all(&staging_root)
        .map_err(|e| install_fail(format!("mkdir {}: {e}", staging_root.display())))?;
    let staging = staging_root.join(uuid::Uuid::new_v4().to_string());
    let extract_root = staging.join("archive");
    std::fs::create_dir_all(&extract_root)
        .map_err(|e| install_fail(format!("mkdir {}: {e}", extract_root.display())))?;

    let result = (|| {
        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|e| install_fail(e.to_string()))?;
            // `enclosed_name` rejects absolute paths and `..` traversal.
            let rel = match entry.enclosed_name() {
                Some(p) => p.to_path_buf(),
                None => {
                    return Err(install_fail(format!(
                        "unsafe path in archive: {}",
                        entry.name()
                    )))
                }
            };
            if rel.as_os_str().is_empty() {
                continue;
            }
            let out = extract_root.join(&rel);
            if entry.is_dir() {
                std::fs::create_dir_all(&out).map_err(|e| install_fail(e.to_string()))?;
                apply_zip_mode(&entry, &out)?;
            } else {
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| install_fail(e.to_string()))?;
                }
                let mut f = std::fs::File::create(&out).map_err(|e| install_fail(e.to_string()))?;
                std::io::copy(&mut entry, &mut f).map_err(|e| install_fail(e.to_string()))?;
                apply_zip_mode(&entry, &out)?;
            }
        }

        let manifest = extract_root.join("plugin.toml");
        if !manifest.is_file() {
            return Err(install_fail("archive must contain plugin.toml at root"));
        }
        let definition: crate::plugin::renderer_registry::PluginManifest = toml::from_str(
            &std::fs::read_to_string(&manifest).map_err(|e| install_fail(e.to_string()))?,
        )
        .map_err(|e| install_fail(e.to_string()))?;
        let info = plugin_info(&definition).map_err(install_fail)?;
        if let Some(probe) = &definition.plugin.probe {
            let files = crate::plugin::renderer_registry::read_plugin_files(&extract_root)
                .map_err(|e| install_fail(e.to_string()))?;
            crate::probe::program::Program::open(&info.id, &extract_root, probe, &files)
                .map_err(|e| install_fail(format!("invalid plugin.probe: {e:#}")))?;
        }
        let id = info.id;
        validate_plugin_dir_name(&id)?;
        let target = dest_root.join(&id);
        remove_existing(&target)?;
        std::fs::rename(&extract_root, &target).map_err(|e| {
            install_fail(format!(
                "install {} to {}: {e}",
                extract_root.display(),
                target.display()
            ))
        })?;
        log::info!("installed plugin '{id}' into {}", target.display());
        Ok(id)
    })();

    if let Err(e) = std::fs::remove_dir_all(&staging) {
        if e.kind() != ErrorKind::NotFound {
            log::warn!("cleanup {} failed: {e}", staging.display());
        }
    }

    result
}

pub fn delete_user_plugin(plugin_id: &str) -> Result<String> {
    if plugin_id.is_empty() {
        return Err(delete_fail("plugin id is empty"));
    }

    let dest_root =
        user_plugins_dir_path().ok_or_else(|| delete_fail("no user plugin directory"))?;
    let entries = match std::fs::read_dir(&dest_root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Err(delete_fail(format!("user plugin '{plugin_id}' not found")));
        }
        Err(e) => return Err(delete_fail(format!("read {}: {e}", dest_root.display()))),
    };
    let dest_root_canon = dest_root
        .canonicalize()
        .map_err(|e| delete_fail(format!("canonicalize {}: {e}", dest_root.display())))?;

    for entry in entries.flatten() {
        let plugin_dir = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }

        let manifest = plugin_dir.join("plugin.toml");
        if !manifest.is_file() {
            continue;
        }
        let id = match read_plugin_id(&manifest) {
            Ok(id) => id,
            Err(e) => {
                log::warn!("skip {} while deleting plugin: {e}", manifest.display());
                continue;
            }
        };
        if id != plugin_id {
            continue;
        }

        let plugin_dir_canon = plugin_dir
            .canonicalize()
            .map_err(|e| delete_fail(format!("canonicalize {}: {e}", plugin_dir.display())))?;
        if !plugin_dir_canon.starts_with(&dest_root_canon) {
            return Err(delete_fail(format!(
                "plugin '{plugin_id}' is outside the user plugin directory"
            )));
        }

        std::fs::remove_dir_all(&plugin_dir)
            .map_err(|e| delete_fail(format!("delete {}: {e}", plugin_dir.display())))?;
        log::info!(
            "deleted user plugin '{plugin_id}' from {}",
            plugin_dir.display()
        );
        return Ok(id);
    }

    Err(delete_fail(format!("user plugin '{plugin_id}' not found")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::io::Write;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct XdgDataHomeGuard {
        old: Option<OsString>,
    }

    impl XdgDataHomeGuard {
        fn set(path: &Path) -> Self {
            let old = std::env::var_os("XDG_DATA_HOME");
            std::env::set_var("XDG_DATA_HOME", path);
            Self { old }
        }
    }

    impl Drop for XdgDataHomeGuard {
        fn drop(&mut self) {
            if let Some(old) = &self.old {
                std::env::set_var("XDG_DATA_HOME", old);
            } else {
                std::env::remove_var("XDG_DATA_HOME");
            }
        }
    }

    fn with_xdg_data_home<T>(path: &Path, f: impl FnOnce() -> T) -> T {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = XdgDataHomeGuard::set(path);
        f()
    }

    fn write_zip(path: &Path, files: &[(&str, &str)]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .unix_permissions(0o644);
        for (name, body) in files {
            zip.start_file(name, options).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    fn manifest(id: &str) -> String {
        format!(
            r#"[plugin]
id = "{id}"
name = "Test"
version = "1.0.0"
"#
        )
    }

    #[test]
    fn install_zip_accepts_plugin_root_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("plugin.zip");
        write_zip(
            &zip_path,
            &[
                ("plugin.toml", &manifest("org.example.root")),
                ("files.txt", "plugin.toml\nfiles.txt\nmain.lua\n"),
                ("main.lua", "return {}\n"),
            ],
        );

        with_xdg_data_home(tmp.path(), || {
            let id = install_zip(zip_path.to_str().unwrap()).unwrap();
            assert_eq!(id, "org.example.root");
            let plugin_dir = tmp.path().join("waywallen/plugins/org.example.root");
            assert!(plugin_dir.join("plugin.toml").is_file());
            assert!(plugin_dir.join("files.txt").is_file());
            assert!(plugin_dir.join("main.lua").is_file());
            assert!(!tmp.path().join("waywallen/plugins/plugin.toml").exists());
        });
    }

    #[test]
    fn invalid_probe_does_not_overwrite_an_installed_plugin() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("plugin.zip");
        let id = "org.example.probe";
        let valid = manifest(id);
        write_zip(
            &archive,
            &[
                ("plugin.toml", &valid),
                ("files.txt", "plugin.toml\nfiles.txt\nmain.lua\n"),
                ("main.lua", "return {} -- original"),
            ],
        );
        with_xdg_data_home(root.path(), || {
            install_zip(archive.to_str().unwrap()).unwrap();
            let invalid =
                format!("{valid}\n[plugin.probe]\nentry='probe.lua'\nversion=1\ntypes=['scene']\n");
            write_zip(
                &archive,
                &[
                    ("plugin.toml", &invalid),
                    ("files.txt", "plugin.toml\nfiles.txt\nmain.lua\nprobe.lua\n"),
                    ("main.lua", "return {} -- replacement"),
                ],
            );
            assert!(install_zip(archive.to_str().unwrap()).is_err());
            assert_eq!(
                std::fs::read_to_string(
                    root.path()
                        .join("waywallen/plugins")
                        .join(id)
                        .join("main.lua")
                )
                .unwrap(),
                "return {} -- original"
            );
        });
    }

    #[test]
    fn inspect_zip_reads_plugin_root_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("plugin.zip");
        write_zip(
            &zip_path,
            &[
                (
                    "plugin.toml",
                    r#"[plugin]
id = "org.example.inspect"
name = "Inspect"
version = "2.1.0"
update = "https://example.org/inspect/update.json"
entry = "main.lua"
entry_version = 4

[renderers.scene]
bin = "bin/renderer"
types = ["scene"]
"#,
                ),
                ("files.txt", "plugin.toml\nfiles.txt\nmain.lua\n"),
                ("main.lua", "return {}\n"),
            ],
        );

        let info = inspect_zip(zip_path.to_str().unwrap()).unwrap();
        assert_eq!(info.id, "org.example.inspect");
        assert_eq!(info.name, "Inspect");
        assert_eq!(info.version, "2.1.0");
        assert_eq!(
            info.update.as_deref(),
            Some("https://example.org/inspect/update.json")
        );
        assert!(info.has_source);
        assert_eq!(info.renderers, vec!["scene".to_string()]);
    }

    #[test]
    fn install_zip_rejects_unsupported_entry_version_before_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let current_zip = tmp.path().join("current.zip");
        let unsupported_zip = tmp.path().join("unsupported.zip");
        write_zip(
            &current_zip,
            &[
                (
                    "plugin.toml",
                    r#"[plugin]
id = "org.example.version"
name = "Version"
entry = "main.lua"
entry_version = 4
"#,
                ),
                ("files.txt", "plugin.toml\nfiles.txt\nmain.lua\n"),
                ("main.lua", "return 'current'\n"),
            ],
        );
        write_zip(
            &unsupported_zip,
            &[
                (
                    "plugin.toml",
                    r#"[plugin]
id = "org.example.version"
name = "Version"
entry = "main.lua"
entry_version = 2
"#,
                ),
                ("files.txt", "plugin.toml\nfiles.txt\nmain.lua\n"),
                ("main.lua", "return 'unsupported'\n"),
            ],
        );

        with_xdg_data_home(tmp.path(), || {
            install_zip(current_zip.to_str().unwrap()).unwrap();
            let error = install_zip(unsupported_zip.to_str().unwrap()).unwrap_err();
            assert!(error.to_string().contains("entry_version 2 is unsupported"));
            let entry = tmp
                .path()
                .join("waywallen/plugins/org.example.version/main.lua");
            assert_eq!(
                std::fs::read_to_string(entry).unwrap(),
                "return 'current'\n"
            );
        });
    }

    #[test]
    fn inspect_zip_rejects_missing_entry_version() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("missing-version.zip");
        write_zip(
            &zip_path,
            &[
                (
                    "plugin.toml",
                    r#"[plugin]
id = "org.example.missing-version"
name = "Missing version"
entry = "main.lua"
"#,
                ),
                ("files.txt", "plugin.toml\nfiles.txt\nmain.lua\n"),
                ("main.lua", "return {}\n"),
            ],
        );

        let error = inspect_zip(zip_path.to_str().unwrap()).unwrap_err();
        assert!(error
            .to_string()
            .contains("plugin.entry requires plugin.entry_version"));
    }

    #[test]
    fn install_zip_overwrites_existing_plugin_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let first_zip = tmp.path().join("first.zip");
        let second_zip = tmp.path().join("second.zip");
        write_zip(
            &first_zip,
            &[
                ("plugin.toml", &manifest("org.example.overwrite")),
                ("files.txt", "plugin.toml\nfiles.txt\nold.lua\n"),
                ("old.lua", "return 'old'\n"),
            ],
        );
        write_zip(
            &second_zip,
            &[
                ("plugin.toml", &manifest("org.example.overwrite")),
                ("files.txt", "plugin.toml\nfiles.txt\nnew.lua\n"),
                ("new.lua", "return 'new'\n"),
            ],
        );

        with_xdg_data_home(tmp.path(), || {
            let id = install_zip(first_zip.to_str().unwrap()).unwrap();
            assert_eq!(id, "org.example.overwrite");
            let id = install_zip(second_zip.to_str().unwrap()).unwrap();
            assert_eq!(id, "org.example.overwrite");

            let plugin_dir = tmp.path().join("waywallen/plugins/org.example.overwrite");
            assert!(plugin_dir.join("new.lua").is_file());
            assert!(!plugin_dir.join("old.lua").exists());
            assert_eq!(
                std::fs::read_to_string(plugin_dir.join("new.lua")).unwrap(),
                "return 'new'\n"
            );
        });
    }

    #[test]
    fn install_zip_rejects_top_level_plugin_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("plugin.zip");
        write_zip(
            &zip_path,
            &[
                ("bundle/plugin.toml", &manifest("org.example.top")),
                ("bundle/files.txt", "plugin.toml\nfiles.txt\nmain.lua\n"),
                ("bundle/main.lua", "return {}\n"),
            ],
        );

        with_xdg_data_home(tmp.path(), || {
            let err = install_zip(zip_path.to_str().unwrap()).unwrap_err();
            assert_eq!(
                err.to_string(),
                "plugin install failed: archive must contain plugin.toml at root"
            );
            assert!(!tmp
                .path()
                .join("waywallen/plugins/org.example.top")
                .exists());
        });
    }
}
