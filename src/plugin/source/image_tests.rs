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
end
M.source.scan = function(ctx) configure(ctx); return scan(ctx) end
M.source.auto_detect = function(ctx) configure(ctx); return detect(ctx) end
return M
"#
        ),
    );
    let probe = Arc::new(ImageProbe::default());
    let manager = SourceManager::with_probe(probe.clone()).unwrap();
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
async fn image_system_auto_detect_honors_xdg_and_preserves_pictures() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let alias = root.path().join("alias");
    for dir in ["data/backgrounds", "data/wallpapers", "Pictures/Wallpapers"] {
        std::fs::create_dir_all(root.path().join(dir)).unwrap();
    }
    symlink(&data, &alias).unwrap();
    let (_plugin, manager, _) = manager(serde_json::json!({
        "HOME": root.path(),
        "XDG_DATA_DIRS": format!("relative:{}:{}:{}", data.display(), alias.display(), data.display()),
    }));
    let detected = manager.auto_detect_all().await.unwrap();
    let paths = &detected["image"];
    assert_eq!(
        paths,
        &[
            root.path().join("Pictures/Wallpapers").to_str().unwrap(),
            data.join("backgrounds").to_str().unwrap(),
            data.join("wallpapers").to_str().unwrap(),
        ]
    );
    assert_eq!(manager.auto_detect_all().await.unwrap()["image"], *paths);
}

#[tokio::test]
async fn image_system_requires_light_images_and_protects_package_removal() {
    let root = tempfile::tempdir().unwrap();
    let wallpapers = root.path().join("wallpapers");
    let image = wallpapers.join("Night/contents/images_dark/1920x1080.png");
    file(&image, b"image");
    file(
        &wallpapers.join("Night/metadata.json"),
        r#"{"KPlugin":{"Name":"Night"}}"#,
    );
    let (_plugin, manager, _) = manager(serde_json::json!({
        "XDG_DATA_DIRS": root.path().join("missing"),
    }));
    assert!(scan(&manager, &[wallpapers.clone()]).await.is_empty());
    let light = wallpapers.join("Night/contents/images/1920x1080.png");
    file(&light, b"image");
    let entries = scan(&manager, &[wallpapers]).await;
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].resource,
        root.path()
            .join("wallpapers/Night/contents")
            .to_str()
            .unwrap()
    );
    assert_eq!(entries[0].preview.as_deref(), light.to_str());
    assert_eq!(entries[0].name, "Night");
    assert_eq!(entries[0].can_remove, Some(false));
    assert!(!manager.supports_entry_remove(&entries[0]));
    assert!(manager
        .remove_item("image", &entries[0], &[])
        .await
        .is_err());
    assert!(image.exists());
    assert!(manager
        .call_properties("image", &entries[0])
        .await
        .unwrap()
        .unwrap()
        .contains("waywallen.scheme_color"));
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
async fn image_system_package_library_keeps_one_persistable_item() {
    let root = tempfile::tempdir().unwrap();
    file(&root.path().join("contents/images/1920x1080.png"), b"image");
    file(&root.path().join("contents/images/3840x2160.png"), b"image");
    file(
        &root.path().join("contents/images_dark/1920x1080.png"),
        b"image",
    );
    let (_plugin, manager, _) = manager(serde_json::json!({}));
    let entries = scan(&manager, &[root.path().to_path_buf()]).await;
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].resource,
        root.path().join("contents").to_str().unwrap()
    );
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let (summary, _) = crate::model::sync::sync_plugin_entries(
        &db,
        crate::model::sync::PluginRef {
            name: "image",
            version: "0.5.0",
        },
        &entries,
        &[root.path().to_str().unwrap().into()],
    )
    .await
    .unwrap();
    assert_eq!(summary.dropped, 0);
    let loaded = crate::model::repo::load_entries(&db).await.unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].preview, entries[0].preview);
    assert_eq!(
        (loaded[0].width, loaded[0].height),
        (Some(1920), Some(1080))
    );
    assert_eq!(loaded[0].can_remove, Some(false));
}

#[tokio::test]
async fn image_system_preview_prefers_smallest_closest_widescreen_image() {
    let root = tempfile::tempdir().unwrap();
    for name in [
        "100x100.png",
        "1920x1080.png",
        "3840x2160.png",
        "2560x1600.png",
        "light.png",
        "0x100.png",
        "999999999999x1.png",
    ] {
        file(&root.path().join("contents/images").join(name), b"image");
    }
    file(
        &root.path().join("contents/images_dark/320x180.png"),
        b"image",
    );
    let (_plugin, manager, probe) = manager(serde_json::json!({}));
    let entries = scan(&manager, &[root.path().to_path_buf()]).await;
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].preview.as_deref(),
        root.path().join("contents/images/1920x1080.png").to_str()
    );
    assert_eq!(
        (entries[0].width, entries[0].height),
        (Some(1920), Some(1080))
    );
    assert_eq!(probe.0.load(Ordering::Relaxed), 0);
    std::fs::remove_file(root.path().join("contents/images/1920x1080.png")).unwrap();
    std::fs::remove_file(root.path().join("contents/images/3840x2160.png")).unwrap();
    let entries = scan(&manager, &[root.path().to_path_buf()]).await;
    assert_eq!(
        (entries[0].width, entries[0].height),
        (Some(2560), Some(1600))
    );
}

#[tokio::test]
async fn image_system_scans_kde_and_gnome_and_deduplicates_fedora_links() {
    let root = tempfile::tempdir().unwrap();
    let backgrounds = root.path().join("backgrounds");
    let wallpapers = root.path().join("wallpapers");
    for path in [
        "backgrounds/gnome/light.png",
        "backgrounds/gnome/dark.jpg",
        "backgrounds/gnome/thumbnail.png",
        "backgrounds/f43/default/day.png",
        "backgrounds/f43/default/night.png",
        "backgrounds/unsupported.jxl",
        "backgrounds/unsupported.svg",
    ] {
        file(&root.path().join(path), b"image");
    }
    file(
        &root.path().join("gnome-background-properties/gnome.xml"),
        r#"<!DOCTYPE wallpapers SYSTEM "gnome-wp-list.dtd">
<wallpapers><wallpaper deleted="false"><name>GNOME</name><name xml:lang="zh_CN">桌面</name>
<filename>../backgrounds/gnome/light.png</filename><filename-dark>../backgrounds/gnome/dark.jpg</filename-dark>
<thumbnail>../backgrounds/gnome/thumbnail.png</thumbnail></wallpaper>
<wallpaper><name>Fedora</name><filename>../backgrounds/f43/default/background.xml</filename></wallpaper></wallpapers>"#,
    );
    file(&backgrounds.join("f43/default/background.xml"), "<background><static><file>day.png</file></static><static><file><size width=\"100\" height=\"100\">night.png</size></file></static><transition><from>day.png</from><to>night.png</to></transition></background>");
    file(
        &root.path().join("gnome-background-properties/bad.xml"),
        "<broken",
    );
    file(
        &wallpapers.join("Mountain/metadata.json"),
        r#"{"KPlugin":{"Name":"Mountain","Name[zh_CN]":"山峰","Id":"Next"}}"#,
    );
    for path in [
        "Mountain/contents/images/100x100.png",
        "Mountain/contents/images/200x100.png",
        "Mountain/contents/images_dark/100x100.png",
        "Mountain/contents/screenshot.png",
    ] {
        file(&wallpapers.join(path), b"image");
    }
    file(&wallpapers.join("Broken/metadata.json"), "not json");
    file(
        &wallpapers.join("Broken/contents/images/320x200.png"),
        b"image",
    );
    file(&wallpapers.join("album/nested/photo.png"), b"image");
    file(
        &wallpapers.join("F43/metadata.json"),
        r#"{"KPlugin":{"Name":"Fedora","Id":"Next"}}"#,
    );
    for (variant, target) in [("images", "day.png"), ("images_dark", "night.png")] {
        let dir = wallpapers.join("F43/contents").join(variant);
        std::fs::create_dir_all(&dir).unwrap();
        for resolution in ["1024x768.png", "3840x2160.png"] {
            symlink(
                backgrounds.join("f43/default").join(target),
                dir.join(resolution),
            )
            .unwrap();
        }
        symlink("/missing", dir.join("broken.png")).unwrap();
    }
    symlink(&backgrounds, backgrounds.join("cycle")).unwrap();
    let (_plugin, manager, probe) =
        manager(serde_json::json!({"XDG_DATA_DIRS": root.path(), "LANG": "zh_CN.UTF-8"}));
    let entries = scan(&manager, &[wallpapers.clone(), backgrounds.clone()]).await;
    assert_eq!(entries.len(), 8, "{entries:#?}");
    assert_eq!(probe.0.load(Ordering::Relaxed), 5);
    assert!(entries
        .iter()
        .all(|e| e.width.is_some() && e.height.is_some() && e.size.is_some()));
    assert!(entries
        .iter()
        .all(|e| e.can_remove == Some(e.preview.is_none())));
    assert!(entries.iter().any(|e| e.name == "桌面 · Dark · 1920×1080"));
    assert!(entries
        .iter()
        .any(|e| e.name == "山峰" && e.preview.is_some()));
    let package = entries.iter().find(|e| e.name == "山峰").unwrap();
    assert_eq!((package.width, package.height), (Some(200), Some(100)));
    let schema = manager
        .call_properties("image", package)
        .await
        .unwrap()
        .unwrap();
    let schema: serde_json::Value = serde_json::from_str(&schema).unwrap();
    assert_eq!(schema["waywallen.image_variant"]["type"], "combo");
    assert_eq!(schema["waywallen.image_variant"]["value"], "day-night");
    assert!(entries.iter().any(|e| e.name == "Broken"));
    assert!(!entries.iter().any(|e| e.resource.contains("screenshot")
        || e.resource.contains("thumbnail")
        || e.resource.contains("/F43/contents/images")));
    let resources: Vec<_> = entries.iter().map(|e| &e.resource).collect();
    let repeated = scan(&manager, &[backgrounds, wallpapers]).await;
    assert_eq!(
        resources,
        repeated.iter().map(|e| &e.resource).collect::<Vec<_>>()
    );
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let mut identities = None;
    for snapshot in [&entries, &repeated] {
        crate::model::sync::sync_plugin_entries(
            &db,
            crate::model::sync::PluginRef {
                name: "image",
                version: "0.5.0",
            },
            snapshot,
            &[],
        )
        .await
        .unwrap();
        let loaded = crate::model::repo::load_entries(&db).await.unwrap();
        assert_eq!(loaded.len(), 8);
        assert!(loaded.iter().all(
            |entry| entry.can_remove == Some(entry.preview.is_none()) && entry.width.is_some()
        ));
        let ids: Vec<_> = loaded
            .into_iter()
            .map(|entry| (entry.item_id, entry.resource))
            .collect();
        assert_eq!(identities.get_or_insert_with(|| ids.clone()), &ids);
    }
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
