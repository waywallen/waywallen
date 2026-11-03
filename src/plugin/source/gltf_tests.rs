use super::*;
use crate::probe::media::MediaMeta;
use std::os::unix::fs::symlink;

struct NoMediaProbe;

impl MediaProbe for NoMediaProbe {
    fn probe_media(&self, _: &str) -> MediaMeta {
        panic!("glTF discovery must not probe a model as image or video")
    }
}

fn manager() -> (tempfile::TempDir, SourceManager) {
    let dir = tempfile::tempdir().unwrap();
    for (path, contents) in [
        (
            "main.lua",
            include_str!("../../../plugins/org.waywallen.gltf/main.lua"),
        ),
        (
            "gltf/source.lua",
            include_str!("../../../plugins/org.waywallen.gltf/gltf/source.lua"),
        ),
        (
            "gltf/wallpaper.lua",
            include_str!("../../../plugins/org.waywallen.gltf/gltf/wallpaper.lua"),
        ),
    ] {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    let manager = SourceManager::with_probe(Arc::new(NoMediaProbe)).unwrap();
    manager
        .load_plugin(
            &dir.path().join("main.lua"),
            "org.waywallen.gltf",
            "0.5.0",
            ENTRY_VERSION,
        )
        .unwrap();
    (dir, manager)
}

fn model(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"model content belongs to vecgltf").unwrap();
}

#[tokio::test]
async fn gltf_source_scans_two_levels_and_deduplicates_canonical_paths() {
    let root = tempfile::tempdir().unwrap();
    let models = root.path().join("models");
    for path in [
        "main.glb",
        "one/MODEL.GLTF",
        "one/two/animated.glb",
        "one/two/three/deep.glb",
        "buffer.bin",
        "texture.png",
        "not-a-model.glb.txt",
    ] {
        model(&models.join(path));
    }
    std::fs::create_dir(models.join("directory.glb")).unwrap();
    symlink(models.join("main.glb"), models.join("alias.glb")).unwrap();
    symlink("/missing/gltf-model", models.join("broken.glb")).unwrap();
    let alias = root.path().join("alias");
    symlink(&models, &alias).unwrap();
    let (_plugin, manager) = manager();
    manager
        .scan_all(&HashMap::from([(
            "gltf".into(),
            vec![
                models.to_str().unwrap().into(),
                alias.to_str().unwrap().into(),
                models.join("one").to_str().unwrap().into(),
            ],
        )]))
        .await
        .unwrap();
    let entries = manager.list();
    // The overlapping root makes the otherwise-too-deep file visible.
    assert_eq!(entries.len(), 4, "{entries:?}");
    let canonical: HashSet<_> = entries
        .iter()
        .map(|entry| std::fs::canonicalize(&entry.resource).unwrap())
        .collect();
    assert_eq!(canonical.len(), entries.len());
    assert!(entries.iter().all(|entry| entry.wp_type == "gltf"
        && entry.plugin_name == "gltf"
        && entry.can_remove == Some(false)
        && entry.preview.is_none()
        && entry.size.is_some()
        && entry.width.is_none()
        && entry.height.is_none()));
    assert!(entries.iter().any(|entry| entry.name == "MODEL"));
}

#[tokio::test]
async fn gltf_source_counts_the_model_directory_and_preserves_size_after_probe() {
    let root = tempfile::tempdir().unwrap();
    let package = root.path().join("model");
    std::fs::create_dir_all(package.join("textures/nested")).unwrap();
    std::fs::write(package.join("scene.gltf"), b"model").unwrap();
    std::fs::write(package.join("variant.glb"), b"variant").unwrap();
    std::fs::write(package.join("scene.bin"), b"buffer").unwrap();
    std::fs::write(package.join("textures/nested/image.png"), b"texture").unwrap();
    std::fs::write(root.path().join("unrelated"), b"outside").unwrap();
    symlink(&package, package.join("textures/loop")).unwrap();
    symlink(root.path().join("unrelated"), package.join("external.bin")).unwrap();
    let (_plugin, manager) = manager();
    let library = root.path().to_str().unwrap().to_owned();
    let libraries = HashMap::from([("gltf".into(), vec![library.clone()])]);
    manager.scan_all(&libraries).await.unwrap();
    let entries = manager.list();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|entry| entry.size == Some(25)));

    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    crate::model::sync::sync_plugin_entries(
        &db,
        crate::model::sync::PluginRef {
            name: "gltf",
            version: "0.5.0",
        },
        &entries,
        std::slice::from_ref(&library),
    )
    .await
    .unwrap();
    let stats = crate::probe::task::run_pending(&db, Arc::new(NoMediaProbe))
        .await
        .unwrap();
    assert_eq!(stats.stat_done, 2);
    let stored = crate::model::repo::load_entries(&db).await.unwrap();
    assert!(stored
        .iter()
        .all(|entry| entry.size == Some(25) && entry.modified_at.is_some()));

    std::fs::write(package.join("textures/extra.png"), b"extra").unwrap();
    manager.scan_all(&libraries).await.unwrap();
    assert!(manager.list().iter().all(|entry| entry.size == Some(30)));
}

#[tokio::test]
async fn gltf_source_leaves_deeper_models_unscanned() {
    let root = tempfile::tempdir().unwrap();
    model(&root.path().join("one/two/model.glb"));
    model(&root.path().join("one/two/three/deeper.gltf"));
    let (_plugin, manager) = manager();
    manager
        .scan_all(&HashMap::from([(
            "gltf".into(),
            vec![root.path().to_str().unwrap().into()],
        )]))
        .await
        .unwrap();
    let entries = manager.list();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "model");
}

#[tokio::test]
async fn gltf_wallpaper_apply_forwards_the_model_path_without_properties() {
    let root = tempfile::tempdir().unwrap();
    model(&root.path().join("a model.glb"));
    let (_plugin, manager) = manager();
    manager
        .scan_all(&HashMap::from([(
            "gltf".into(),
            vec![root.path().to_str().unwrap().into()],
        )]))
        .await
        .unwrap();
    let entry = manager.list().pop().unwrap();
    let applied = manager.call_apply("gltf", &entry).await.unwrap();
    assert_eq!(
        applied.extras,
        HashMap::from([("path".into(), entry.resource.clone())])
    );
    assert!(applied.default_user_properties.is_empty());
    assert!(manager
        .call_properties("gltf", &entry)
        .await
        .unwrap()
        .is_none());
}
