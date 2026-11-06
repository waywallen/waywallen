use mlua::prelude::*;
use std::ffi::CString;
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::path::Path;

fn sticky_allows(mode: u32, directory_uid: u32, file_uid: u32, uid: u32) -> bool {
    mode & libc::S_ISVTX == 0 || uid == 0 || uid == directory_uid || uid == file_uid
}

pub(super) fn can_remove(path: &Path) -> Option<bool> {
    let file = std::fs::symlink_metadata(path).ok()?;
    if file.is_dir() {
        return Some(false);
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let directory = std::fs::metadata(parent).ok()?;
    let name = CString::new(parent.as_os_str().as_bytes()).ok()?;
    // A permission snapshot is only a UI hint; unlink remains authoritative.
    let allowed = unsafe {
        libc::faccessat(
            libc::AT_FDCWD,
            name.as_ptr(),
            libc::W_OK | libc::X_OK,
            libc::AT_EACCESS,
        )
    };
    if allowed != 0 {
        return match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::EACCES | libc::EPERM | libc::EROFS) => Some(false),
            _ => None,
        };
    }
    let uid = unsafe { libc::geteuid() };
    if !sticky_allows(directory.mode(), directory.uid(), file.uid(), uid) {
        return Some(false);
    }
    Some(true)
}

fn directory_size(path: &Path) -> Option<i64> {
    let root = std::fs::canonicalize(path).ok()?;
    if !root.is_dir() {
        return None;
    }
    let mut pending = vec![root];
    let mut total = 0_i64;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).ok()? {
            let entry = entry.ok()?;
            let metadata = std::fs::symlink_metadata(entry.path()).ok()?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                total = total.checked_add(i64::try_from(metadata.len()).ok()?)?;
            }
        }
    }
    Some(total)
}

pub(super) fn install(lua: &Lua, fs: &LuaTable) -> LuaResult<()> {
    fs.set(
        "directory_size",
        lua.create_async_function(|_, path: String| async move {
            tokio::task::spawn_blocking(move || directory_size(Path::new(&path)))
                .await
                .map_err(LuaError::external)
        })?,
    )?;
    fs.set(
        "list_files",
        lua.create_function(|_, path: String| {
            let mut files = Vec::new();
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    if entry.path().is_file() {
                        if let Ok(path) = entry.path().into_os_string().into_string() {
                            files.push(path);
                        }
                    }
                }
            }
            files.sort();
            Ok(files)
        })?,
    )?;
    fs.set(
        "is_file",
        lua.create_function(|_, path: String| Ok(Path::new(&path).is_file()))?,
    )?;
    fs.set(
        "is_dir",
        lua.create_function(|_, path: String| Ok(Path::new(&path).is_dir()))?,
    )?;
    fs.set(
        "readable_dir",
        lua.create_function(|_, path: String| Ok(std::fs::read_dir(path).is_ok()))?,
    )?;
    fs.set(
        "is_symlink",
        lua.create_function(|_, path: String| Ok(Path::new(&path).is_symlink()))?,
    )?;
    fs.set(
        "canonicalize",
        lua.create_function(|_, path: String| {
            Ok(std::fs::canonicalize(path)
                .ok()
                .and_then(|p| p.into_os_string().into_string().ok()))
        })?,
    )?;
    fs.set(
        "parent",
        lua.create_function(|_, path: String| {
            Ok(Path::new(&path)
                .parent()
                .and_then(|p| p.to_str())
                .map(str::to_owned))
        })?,
    )?;
    fs.set(
        "can_remove",
        lua.create_function(|_, path: String| Ok(can_remove(Path::new(&path))))?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn directory_size_counts_nested_files_without_following_child_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let model = root.path().join("model");
        std::fs::create_dir_all(model.join("textures/nested")).unwrap();
        std::fs::write(model.join("scene.gltf"), b"model").unwrap();
        std::fs::write(model.join("scene.bin"), b"buffer").unwrap();
        std::fs::write(model.join("textures/nested/image.png"), b"texture").unwrap();
        std::fs::write(model.join(".hidden"), b"extra").unwrap();
        let outside = root.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("unrelated"), b"outside content").unwrap();
        symlink(&outside, model.join("linked-directory")).unwrap();
        symlink(&model, model.join("textures/loop")).unwrap();
        symlink(model.join("scene.bin"), model.join("alias.bin")).unwrap();
        symlink(root.path().join("missing"), model.join("broken")).unwrap();
        assert_eq!(directory_size(&model), Some(23));
        let alias = root.path().join("alias");
        symlink(&model, &alias).unwrap();
        assert_eq!(directory_size(&alias), Some(23));
        assert_eq!(directory_size(&model.join("scene.gltf")), None);
        assert_eq!(directory_size(&root.path().join("missing")), None);
    }

    #[test]
    fn removal_uses_parent_permissions_not_file_mode_or_symlink_target() {
        assert_ne!(
            unsafe { libc::geteuid() },
            0,
            "permission test must run unprivileged"
        );
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("read-only");
        std::fs::create_dir(&dir).unwrap();
        let file = dir.join("image.png");
        std::fs::write(&file, b"image").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert_eq!(can_remove(&file), Some(true));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        assert_eq!(can_remove(&file), Some(false));
        let link = root.path().join("link.png");
        symlink(&file, &link).unwrap();
        assert_eq!(can_remove(&link), Some(true));
        assert_eq!(can_remove(&root.path().join("missing")), None);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn sticky_directory_checks_ownership() {
        let root = tempfile::tempdir().unwrap();
        let link = root.path().join("owned-link");
        symlink("/missing", &link).unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o1777)).unwrap();
        assert_eq!(can_remove(&link), Some(true));
        assert!(!sticky_allows(0o1777, 100, 200, 300));
        assert!(sticky_allows(0o1777, 100, 200, 100));
        assert!(sticky_allows(0o1777, 100, 200, 200));
        assert!(sticky_allows(0o1777, 100, 200, 0));
        assert!(sticky_allows(0o777, 100, 200, 300));
    }
}
