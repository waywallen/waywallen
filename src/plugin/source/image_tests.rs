use super::*;
use crate::probe::media::MediaMeta;
use std::os::unix::fs::{symlink, PermissionsExt};

use std::sync::atomic::AtomicUsize;

#[derive(Default)]
struct ImageProbe(AtomicUsize);

impl MediaProbe for ImageProbe {
    fn probe_media(&self, _: &str) -> MediaMeta {
        self.0.fetch_add(1, Ordering::Relaxed);
        MediaMeta {
            width: Some(1920),
            height: Some(1080),
        }
    }
}

fn file(path: &Path, content: impl AsRef<[u8]>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn manager(env: serde_json::Value) -> (tempfile::TempDir, SourceManager, Arc<ImageProbe>) {
    manager_with_db(env, None)
}

fn manager_with_db(
    env: serde_json::Value,
    db: Option<DatabaseConnection>,
) -> (tempfile::TempDir, SourceManager, Arc<ImageProbe>) {
    let dir = tempfile::tempdir().unwrap();
    for (path, content) in [
        (
            "original.lua",
            include_str!("../../../plugins/org.waywallen.image/main.lua"),
        ),
        (
            "image/source.lua",
            include_str!("../../../plugins/org.waywallen.image/image/source.lua"),
        ),
        (
            "image/system.lua",
            include_str!("../../../plugins/org.waywallen.image/image/system.lua"),
        ),
        (
            "image/wallpaper.lua",
            include_str!("../../../plugins/org.waywallen.image/image/wallpaper.lua"),
        ),
    ] {
        file(&dir.path().join(path), content);
    }
    file(
        &dir.path().join("main.lua"),
        format!(
            r#"
local M = import("original")
local scan, detect = M.source.scan, M.source.auto_detect
local function configure(ctx)
    local values = ctx.json.parse([==[{env}]==])
    ctx.env = function(key) return values[key] end
    local library_settings = ctx.library_settings
    ctx.library_settings = function(path)
        local result = library_settings(path)
        result.scan_mode = values.SCAN_MODE or result.scan_mode
        return result
    end
end
M.source.scan = function(ctx) configure(ctx); return scan(ctx) end
M.source.auto_detect = function(ctx) configure(ctx); return detect(ctx) end
return M
"#
        ),
    );
    let probe = Arc::new(ImageProbe::default());
    let manager = SourceManager::with_probe(probe.clone()).unwrap();
    if let Some(db) = db {
        manager.attach_db(db);
    }
    manager
        .load_plugin(
            &dir.path().join("main.lua"),
            "org.waywallen.image",
            "0.5.0",
            ENTRY_VERSION,
        )
        .unwrap();
    (dir, manager, probe)
}

async fn scan(manager: &SourceManager, roots: &[PathBuf]) -> Vec<WallpaperEntry> {
    manager
        .scan_all(&HashMap::from([(
            "image".into(),
            roots
                .iter()
                .map(|p| p.to_str().unwrap().to_owned())
                .collect(),
        )]))
        .await
        .unwrap();
    manager.list()
}

#[tokio::test]
async fn image_system_auto_detect_keeps_modes_and_ignores_data_home() {
    let root = tempfile::tempdir().unwrap();
    for path in [
        "data/wallpapers",
        "data/gnome-background-properties",
        "data/backgrounds",
        "Pictures/Wallpapers",
        "home-data/wallpapers",
    ] {
        std::fs::create_dir_all(root.path().join(path)).unwrap();
    }
    let (_plugin, manager, _) = manager(serde_json::json!({
        "HOME": root.path(), "XDG_DATA_DIRS": root.path().join("data"), "XDG_DATA_HOME":root.path().join("home-data")
    }));
    let libraries = manager
        .auto_detect_all()
        .await
        .unwrap()
        .remove("image")
        .unwrap();
    assert_eq!(libraries.len(), 3);
    assert_eq!(libraries[0].settings["scan_mode"], "wallpapers");
    assert_eq!(libraries[1].settings["scan_mode"], "wallpapers");
    assert_eq!(libraries[2].settings["scan_mode"], "files");
    assert!(libraries
        .iter()
        .all(|library| !library.path.contains("home-data")));
}

#[tokio::test]
async fn image_system_descriptors_are_one_entry_each() {
    let root = tempfile::tempdir().unwrap();
    file(
        &root.path().join("KDE/metadata.json"),
        r#"{"KPlugin":{"Name":"Pair"}}"#,
    );
    file(
        &root.path().join("KDE/contents/images/1920x1080.png"),
        "image",
    );
    file(
        &root.path().join("KDE/contents/images/3840x2160.png"),
        "image",
    );
    file(
        &root.path().join("KDE/contents/images_dark/1920x1080.png"),
        "image",
    );
    file(
        &root.path().join("gnome/pair.xml"),
        r#"<wallpapers><wallpaper><name>Pair</name><filename>day.png</filename><filename-dark>night.png</filename-dark></wallpaper><wallpaper><filename>other.png</filename></wallpaper></wallpapers>"#,
    );
    file(
        &root.path().join("gnome/timed.xml"),
        r#"<background><static><duration>10</duration><file>day.png</file></static><transition><duration>10</duration><from>day.png</from><to>night.png</to></transition></background>"#,
    );
    file(&root.path().join("gnome/day.png"), "image");
    let (_plugin, manager, probe) = manager(serde_json::json!({"SCAN_MODE":"wallpapers"}));
    let entries = scan(&manager, &[root.path().to_owned()]).await;
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().all(|entry| entry.sub_type == "timeline"
        && entry.preview.is_none()
        && entry.can_remove == Some(false)));
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.resource.ends_with(".xml"))
            .count(),
        2
    );
    let kde = entries
        .iter()
        .find(|entry| entry.resource.ends_with("metadata.json"))
        .unwrap();
    assert_eq!((kde.width, kde.height), (Some(1920), Some(1080)));
    assert_eq!(probe.0.load(Ordering::Relaxed), 0);
    let apply = manager.call_apply("image", kde).await.unwrap();
    assert_eq!(apply.extras.len(), 1);
    assert_eq!(apply.extras["path"], kde.resource);
}

#[tokio::test]
async fn image_system_files_mode_does_not_infer_packages() {
    let root = tempfile::tempdir().unwrap();
    file(&root.path().join("metadata.json"), r#"{"KPlugin":{}}"#);
    file(&root.path().join("contents/images/1920x1080.png"), "image");
    file(&root.path().join("ignored.jxl"), "image");
    file(&root.path().join("ignored.xml"), "<wallpapers/>");
    let (_plugin, manager, _) = manager(serde_json::json!({}));
    let entries = scan(&manager, &[root.path().to_owned()]).await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].sub_type, "");
    assert!(entries[0].resource.ends_with(".png"));
    assert!(entries[0].preview.is_none());
}

#[tokio::test]
async fn image_system_kde_size_matches_the_selected_image_after_scan_sync_and_probe() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().to_str().unwrap().to_owned();
    let selected = root.path().join("contents/images/1920x1080.png");
    file(
        &root.path().join("metadata.json"),
        r#"{"KPlugin":{"Name":"Size"}}"#,
    );
    file(&selected, "selected image");
    file(
        &root.path().join("contents/images/3840x2160.png"),
        "larger selected image",
    );
    file(&root.path().join("contents/images/800x600.png"), "x");
    file(
        &root.path().join("contents/images_dark/1920x1080.png"),
        "dark image is not added to the size",
    );
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let (_plugin, manager, probe) = manager_with_db(
        serde_json::json!({"SCAN_MODE": "wallpapers"}),
        Some(db.clone()),
    );
    for (remove_selected, expected_size, expected_width, expected_height) in
        [(false, 14, 1920, 1080), (true, 21, 3840, 2160)]
    {
        if remove_selected {
            std::fs::remove_file(&selected).unwrap();
        }
        let entries = scan(&manager, &[root.path().to_owned()]).await;
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].resource,
            root.path().join("metadata.json").to_str().unwrap()
        );
        assert_eq!(entries[0].size, Some(expected_size));
        assert_eq!(
            (entries[0].width, entries[0].height),
            (Some(expected_width), Some(expected_height))
        );
        crate::model::sync::sync_plugin_entries(
            &db,
            crate::model::sync::PluginRef {
                name: "image",
                version: "1",
            },
            &entries,
            std::slice::from_ref(&path),
        )
        .await
        .unwrap();
        let stats = crate::probe::task::run_pending(&db, probe.clone())
            .await
            .unwrap();
        assert_eq!(stats.media_probed, 0);
        let stored = repo::load_entries(&db).await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].size, Some(expected_size));
        assert_eq!(
            (stored[0].width, stored[0].height),
            (Some(expected_width), Some(expected_height))
        );
    }
    assert_eq!(probe.0.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn image_system_reads_saved_library_settings_on_each_scan() {
    let root = tempfile::tempdir().unwrap();
    file(
        &root.path().join("metadata.json"),
        r#"{"KPlugin":{"Name":"Test"}}"#,
    );
    file(&root.path().join("contents/images/1920x1080.png"), "image");
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let (_plugin, manager, _) = manager_with_db(serde_json::json!({}), Some(db.clone()));
    let plugin = repo::upsert_plugin(&db, "image", "1").await.unwrap();
    let settings = HashMap::from([("scan_mode".into(), "wallpapers".into())]);
    let path = root.path().to_str().unwrap().to_owned();
    let library = repo::add_configured_library(&db, plugin.id, &path, None, &settings)
        .await
        .unwrap();
    let entries = scan(&manager, &[root.path().to_owned()]).await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].sub_type, "timeline");
    crate::model::sync::sync_plugin_entries(
        &db,
        crate::model::sync::PluginRef {
            name: "image",
            version: "1",
        },
        &entries,
        std::slice::from_ref(&path),
    )
    .await
    .unwrap();
    let stored = repo::load_entries(&db).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].sub_type, "timeline");
    assert_eq!(stored[0].resource, entries[0].resource);
    assert!(stored[0].preview.is_none());
    repo::set_library_settings(
        &db,
        library.id,
        &HashMap::from([("scan_mode".into(), "files".into())]),
    )
    .await
    .unwrap();
    manager.scan_source("image", &[path]).await.unwrap();
    let entries = manager.list();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].sub_type, "");
    assert!(entries[0].resource.ends_with(".png"));
}

#[test]
fn image_system_library_settings_validate_independently() {
    let (_plugin, manager, _) = manager(serde_json::json!({}));
    assert_eq!(
        manager.library_settings("image", HashMap::new()).unwrap()["scan_mode"],
        "files"
    );
    assert!(manager
        .library_settings(
            "image",
            HashMap::from([("scan_mode".into(), "unknown".into())])
        )
        .is_err());
    assert!(manager
        .library_settings("image", HashMap::from([("unknown".into(), "files".into())]))
        .is_err());
}

#[test]
fn image_system_data_dirs_defaults_and_rejects_relative_paths() {
    let lua = Lua::new();
    let module: LuaTable = lua
        .load(include_str!(
            "../../../plugins/org.waywallen.image/image/system.lua"
        ))
        .eval()
        .unwrap();
    let data_dirs = module.get::<LuaFunction>("data_dirs").unwrap();
    for value in [None, Some(String::new())] {
        let ctx = lua.create_table().unwrap();
        ctx.set(
            "env",
            lua.create_function(move |_, _: String| Ok(value.clone()))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            data_dirs.call::<Vec<String>>(ctx).unwrap(),
            ["/usr/local/share", "/usr/share"]
        );
    }
    let ctx = lua.create_table().unwrap();
    ctx.set(
        "env",
        lua.create_function(|_, _: String| Ok("relative:/first/:/second:/first::"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        data_dirs.call::<Vec<String>>(ctx).unwrap(),
        ["/first", "/second"]
    );
}

#[tokio::test]
async fn image_system_preserves_user_depth_and_refreshes_removal_permissions() {
    assert_ne!(
        unsafe { libc::geteuid() },
        0,
        "permission test must run unprivileged"
    );
    let root = tempfile::tempdir().unwrap();
    let library = root.path().join("wallpapers");
    let path = library.join("album/one.png");
    for relative in [
        "a.png",
        "album/one.png",
        "album/nested/two.JPG",
        "album/nested/deeper/hidden.png",
    ] {
        file(&library.join(relative), b"image");
    }
    let (_plugin, manager, probe) = manager(serde_json::json!({}));
    let entries = scan(&manager, &[library.clone()]).await;
    assert_eq!(entries.len(), 3);
    assert_eq!(probe.0.load(Ordering::Relaxed), 0);
    let mut entry = entries
        .iter()
        .find(|entry| entry.resource == path.to_str().unwrap())
        .unwrap()
        .clone();
    assert!(manager.supports_entry_remove(&entry));
    let parent = path.parent().unwrap();
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o555)).unwrap();
    let result = manager.remove_item("image", &entry, &[]).await;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.is_err());
    assert!(path.exists());
    assert_eq!(manager.list().len(), 3);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    entry.can_remove = Some(false);
    assert!(!manager.supports_entry_remove(&entry));
    manager.remove_item("image", &entry, &[]).await.unwrap();
    assert!(!path.exists());

    let remaining = library.join("a.png");
    std::fs::set_permissions(&library, std::fs::Permissions::from_mode(0o555)).unwrap();
    let entries = scan(&manager, &[library.clone()]).await;
    std::fs::set_permissions(&library, std::fs::Permissions::from_mode(0o755)).unwrap();
    let entry = entries
        .iter()
        .find(|e| e.resource == remaining.to_str().unwrap())
        .unwrap();
    assert_eq!(entry.can_remove, Some(false));
    assert!(!manager.supports_entry_remove(entry));
    let entries = scan(&manager, &[library]).await;
    let entry = entries
        .iter()
        .find(|e| e.resource == remaining.to_str().unwrap())
        .unwrap();
    assert_eq!(entry.can_remove, Some(true));
    assert!(manager.supports_entry_remove(entry));
}

#[tokio::test]
async fn image_system_removes_symlink_not_canonical_target() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("system/image.png");
    file(&target, b"image");
    let library = root.path().join("personal");
    std::fs::create_dir(&library).unwrap();
    let link = library.join("link.png");
    symlink(&target, &link).unwrap();
    let (_plugin, manager, _) = manager(serde_json::json!({}));
    let entries = scan(&manager, &[library]).await;
    manager
        .remove_item("image", &entries[0], &[])
        .await
        .unwrap();
    assert!(!link.exists());
    assert!(target.exists());
}
