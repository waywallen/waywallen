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

pub(super) fn install(lua: &Lua, fs: &LuaTable) -> LuaResult<()> {
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
