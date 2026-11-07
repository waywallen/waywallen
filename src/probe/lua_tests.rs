use super::{seed_typed, task, ProbeService};
use crate::model::repo;
use crate::probe::program::{Declaration, Program};
use std::{collections::HashMap, path::Path, sync::Arc};

#[path = "lua_input_tests.rs"]
mod input_tests;

fn service(root: &Path, script: &str, modules: &[(&str, &str)]) -> Option<Arc<ProbeService>> {
    let tool = std::env::var_os("WAYWALLEN_TEST_PROBE")?;
    let plugin = root.join("plugin");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::write(plugin.join("probe.lua"), script).unwrap();
    let mut files = vec!["probe.lua".to_owned()];
    for (name, code) in modules {
        std::fs::write(plugin.join(name), code).unwrap();
        files.push(name.to_string());
    }
    let program = Program::open(
        "test.probe",
        &plugin,
        &Declaration {
            entry: "probe.lua".into(),
            version: 1,
            types: vec!["scene".into()],
        },
        &files,
    )
    .unwrap();
    let service = Arc::new(ProbeService::with_tool(Some(tool.into())));
    service.replace_programs(HashMap::from([("image".into(), Arc::new(program))]));
    Some(service)
}

#[tokio::test]
async fn script_probe_and_builtin_media_share_the_batch() {
    let root = tempfile::tempdir().unwrap();
    let Some(probe) = service(root.path(), "local helper=require('./helper'); return {probe=function(item) return helper.dimensions() end}", &[("helper.lua","return {dimensions=function() return {width=1920,height=1080} end}")]) else { return; };
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    std::fs::write(root.path().join("scene.json"), b"scene").unwrap();
    std::fs::write(root.path().join("image.ppm"), b"P6\n1 1\n255\n\xff\x00\x00").unwrap();
    let scene = seed_typed(&db, root.path(), "scene.json", "", "scene").await;
    let mut supplied = repo::get_entry(&db, scene).await.unwrap().unwrap();
    supplied.size = Some(123456);
    crate::model::sync::sync_plugin_entries(
        &db,
        crate::model::sync::PluginRef {
            name: "image",
            version: "1",
        },
        &[supplied],
        &[],
    )
    .await
    .unwrap();
    let image = seed_typed(&db, root.path(), "image.ppm", "", "image").await;
    assert_eq!(
        task::run_pending(&db, probe.clone())
            .await
            .unwrap()
            .media_probed,
        2
    );
    assert_eq!(
        repo::get_entry(&db, scene).await.unwrap().unwrap().width,
        Some(1920)
    );
    assert_eq!(
        repo::get_entry(&db, scene).await.unwrap().unwrap().size,
        Some(123456)
    );
    assert_eq!(
        repo::get_entry(&db, image).await.unwrap().unwrap().width,
        Some(1)
    );
    assert_eq!(
        task::run_pending(&db, probe.clone())
            .await
            .unwrap()
            .media_probed,
        0
    );
    super::rewrite_preserving_stat(
        &root.path().join("plugin/helper.lua"),
        b"return {dimensions=function() return {width=2560,height=1440} end}",
    );
    assert!(crate::probe::task::inputs_changed(&db, &probe)
        .await
        .unwrap());
    assert_eq!(
        task::run_pending(&db, probe.clone())
            .await
            .unwrap()
            .media_probed,
        1
    );
    assert_eq!(
        repo::get_entry(&db, scene).await.unwrap().unwrap().width,
        Some(2560)
    );
    std::fs::write(
        root.path().join("plugin/other.lua"),
        "return {probe=function() return {width=3840,height=2160} end}",
    )
    .unwrap();
    let replacement = Program::open(
        "test.probe",
        &root.path().join("plugin"),
        &Declaration {
            entry: "other.lua".into(),
            version: 1,
            types: vec!["scene".into()],
        },
        &["other.lua".into()],
    )
    .unwrap();
    probe.replace_programs(HashMap::from([("image".into(), Arc::new(replacement))]));
    assert!(crate::probe::task::inputs_changed(&db, &probe)
        .await
        .unwrap());
    assert_eq!(
        task::run_pending(&db, probe.clone())
            .await
            .unwrap()
            .media_probed,
        1
    );
    assert_eq!(
        repo::get_entry(&db, scene).await.unwrap().unwrap().width,
        Some(3840)
    );
    probe.replace_programs(HashMap::new());
    assert!(probe
        .prepare_programs()
        .await
        .unwrap()
        .input_kind("image", "scene", "")
        .is_none());
}

#[tokio::test]
async fn missing_declared_module_is_tracked_before_loading() {
    let root = tempfile::tempdir().unwrap();
    let Some(probe) = service(
        root.path(),
        "local helper=require('./helper'); return {probe=helper.probe}",
        &[(
            "helper.lua",
            "return {probe=function() return {width=4,height=3} end}",
        )],
    ) else {
        return;
    };
    std::fs::remove_file(root.path().join("plugin/helper.lua")).unwrap();
    std::fs::write(root.path().join("scene.json"), b"scene").unwrap();
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let id = seed_typed(&db, root.path(), "scene.json", "", "scene").await;
    task::run_pending(&db, probe.clone()).await.unwrap();
    assert_eq!(
        repo::probe_state(&db, id)
            .await
            .unwrap()
            .unwrap()
            .status
            .as_deref(),
        Some("failed")
    );
    std::fs::write(
        root.path().join("plugin/helper.lua"),
        "return {probe=function() return {width=4,height=3} end}",
    )
    .unwrap();
    assert_eq!(task::run_pending(&db, probe).await.unwrap().media_probed, 1);
    assert_eq!(
        repo::get_entry(&db, id).await.unwrap().unwrap().width,
        Some(4)
    );
}

#[tokio::test]
async fn same_stat_script_updates_invalidate_all_cached_result_states() {
    for (old, status) in [
        (
            "return {probe=function() return {width=1,height=1} end}",
            "ok",
        ),
        (
            "return {probe=function() error('old failure') end}",
            "failed",
        ),
        ("return {probe=function() end}", "unsupported"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let Some(probe) = service(root.path(), &format!("{old:<256}"), &[]) else {
            return;
        };
        std::fs::write(root.path().join("scene.json"), b"scene").unwrap();
        let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
        let id = seed_typed(&db, root.path(), "scene.json", "", "scene").await;
        assert_eq!(
            task::run_pending(&db, probe.clone())
                .await
                .unwrap()
                .media_probed,
            1
        );
        let saved = repo::probe_state(&db, id).await.unwrap().unwrap();
        assert_eq!(saved.status.as_deref(), Some(status));
        assert_eq!(
            task::run_pending(&db, probe.clone())
                .await
                .unwrap()
                .media_probed,
            0
        );
        let updated = format!(
            "{:<256}",
            "return {probe=function() return {width=2,height=2} end}"
        );
        super::rewrite_preserving_stat(&root.path().join("plugin/probe.lua"), updated.as_bytes());
        assert!(crate::probe::task::inputs_changed(&db, &probe)
            .await
            .unwrap());
        assert_eq!(
            repo::probe_state(&db, id)
                .await
                .unwrap()
                .unwrap()
                .input_kind,
            saved.input_kind
        );
        assert_eq!(
            task::run_pending(&db, probe.clone())
                .await
                .unwrap()
                .media_probed,
            1
        );
        let state = repo::probe_state(&db, id).await.unwrap().unwrap();
        assert_eq!(state.status.as_deref(), Some("ok"));
        assert_ne!(state.input_kind, saved.input_kind);
        let entry = repo::get_entry(&db, id).await.unwrap().unwrap();
        assert_eq!((entry.width, entry.height), (Some(2), Some(2)));
        assert_eq!(task::run_pending(&db, probe).await.unwrap().media_probed, 0);
    }
}

#[tokio::test]
async fn declared_but_unloaded_modules_participate_in_the_cache_key() {
    let root = tempfile::tempdir().unwrap();
    let Some(probe) = service(
        root.path(),
        "return {probe=function() return {width=1,height=1} end}",
        &[("optional.lua", "return 1")],
    ) else {
        return;
    };
    std::fs::write(root.path().join("scene.json"), b"scene").unwrap();
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let id = seed_typed(&db, root.path(), "scene.json", "", "scene").await;
    assert_eq!(
        task::run_pending(&db, probe.clone())
            .await
            .unwrap()
            .media_probed,
        1
    );
    let saved = repo::probe_state(&db, id).await.unwrap().unwrap();
    assert!(!saved.dependencies.contains("optional.lua"));
    super::rewrite_preserving_stat(&root.path().join("plugin/optional.lua"), b"return 2");
    assert!(crate::probe::task::inputs_changed(&db, &probe)
        .await
        .unwrap());
    assert_eq!(
        task::run_pending(&db, probe.clone())
            .await
            .unwrap()
            .media_probed,
        1
    );
    assert_ne!(
        repo::probe_state(&db, id)
            .await
            .unwrap()
            .unwrap()
            .input_kind,
        saved.input_kind
    );
    assert_eq!(task::run_pending(&db, probe).await.unwrap().media_probed, 0);
}

#[tokio::test]
async fn script_failures_track_missing_inputs_and_recover_on_change() {
    let root = tempfile::tempdir().unwrap();
    let Some(probe) = service(root.path(), "local fs=require('waywallen.fs'); return {probe=function(item) local path=fs.join(fs.parent(item.path),'later'); if not fs.exists(path) then error('missing input') end; return {width=3,height=2} end}", &[]) else {return;};
    std::fs::write(root.path().join("scene.json"), b"scene").unwrap();
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let id = seed_typed(&db, root.path(), "scene.json", "", "scene").await;
    task::run_pending(&db, probe.clone()).await.unwrap();
    let state = repo::probe_state(&db, id).await.unwrap().unwrap();
    assert_eq!(state.status.as_deref(), Some("failed"));
    assert!(state.error.unwrap().contains("missing input"));
    assert_eq!(
        task::run_pending(&db, probe.clone())
            .await
            .unwrap()
            .media_probed,
        0
    );
    std::fs::write(root.path().join("later"), b"present").unwrap();
    assert_eq!(task::run_pending(&db, probe).await.unwrap().media_probed, 1);
    assert_eq!(
        repo::get_entry(&db, id).await.unwrap().unwrap().width,
        Some(3)
    );
}

#[tokio::test]
async fn script_budget_and_module_allowlist_are_enforced() {
    for script in [
        "return {probe=function() while true do end end}",
        "return {probe=function() require('../escape'); return {width=1,height=1} end}",
        "return {probe=function() return {width=0,height=1} end}",
    ] {
        let root = tempfile::tempdir().unwrap();
        let Some(probe) = service(root.path(), script, &[]) else {
            return;
        };
        std::fs::write(root.path().join("scene.json"), b"scene").unwrap();
        let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
        let id = seed_typed(&db, root.path(), "scene.json", "", "scene").await;
        task::run_pending(&db, probe.clone()).await.unwrap();
        assert_eq!(
            repo::probe_state(&db, id)
                .await
                .unwrap()
                .unwrap()
                .status
                .as_deref(),
            Some("failed")
        );
        assert_eq!(task::run_pending(&db, probe).await.unwrap().media_probed, 0);
    }
}

fn pkg(version: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend((version.len() as u32).to_le_bytes());
    bytes.extend(version.as_bytes());
    bytes.extend((files.len() as u32).to_le_bytes());
    let mut offset = 0_u32;
    for (name, data) in files {
        bytes.extend((name.len() as u32).to_le_bytes());
        bytes.extend(name.as_bytes());
        bytes.extend(offset.to_le_bytes());
        bytes.extend((data.len() as u32).to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in files {
        bytes.extend(*data);
    }
    bytes
}

#[tokio::test]
async fn owe_script_reads_explicit_auto_and_pkg_dimensions() {
    let Some(path) = std::env::var_os("WAYWALLEN_TEST_OWE_PROBE") else {
        return;
    };
    let script = std::fs::read_to_string(path).unwrap();
    let root = tempfile::tempdir().unwrap();
    let Some(probe) = service(root.path(), &script, &[]) else {
        return;
    };
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let explicit = br#"{"general":{"orthogonalprojection":{"width":2560,"height":1440}}}"#;
    let auto = br#"{"general":{"orthogonalprojection":{"auto":true}},"objects":[{"image":"a","origin":"960 540 0"}]}"#;
    let array = br#"{"general":{"orthogonalprojection":{"auto":true}},"objects":[{"image":"a","origin":[1280.0,720.0,0]}]}"#;
    let mut nested = serde_json::json!(true);
    for _ in 0..40 {
        nested = serde_json::json!({"value":nested});
    }
    let deep = serde_json::to_vec(&serde_json::json!({
        "general":{"orthogonalprojection":{"width":2560,"height":1440}},
        "unrelated":nested
    }))
    .unwrap();
    for (name, bytes, expected) in [
        ("explicit.json", explicit.as_slice(), Some(2560)),
        ("deep.json", deep.as_slice(), Some(2560)),
        ("auto.json", auto.as_slice(), Some(1920)),
        ("array.json", array.as_slice(), Some(2560)),
        (
            "legacy.pkg",
            pkg("PKGV0023", &[("legacy.json", auto)]).as_slice(),
            Some(1920),
        ),
        (
            "scene.pkg",
            pkg("PKGV0024", &[("scene.json", auto)]).as_slice(),
            Some(1920),
        ),
        (
            "newer.pkg",
            pkg("PKGV0025", &[("newer.json", auto)]).as_slice(),
            Some(1920),
        ),
        (
            "gifscene.pkg",
            pkg(
                "PKGV0001",
                &[("scene.json", explicit), ("/GiFsCeNe.JsOn", auto)],
            )
            .as_slice(),
            Some(1920),
        ),
        (
            "missing.pkg",
            pkg("PKGV0024", &[("scene.json", auto)]).as_slice(),
            None,
        ),
        (
            "perspective.json",
            br#"{"general":{"orthogonalprojection":null}}"#.as_slice(),
            None,
        ),
    ] {
        std::fs::write(root.path().join(name), bytes).unwrap();
        let id = seed_typed(&db, root.path(), name, "", "scene").await;
        task::run_pending(&db, probe.clone()).await.unwrap();
        assert_eq!(
            repo::get_entry(&db, id).await.unwrap().unwrap().width,
            expected,
            "{name}: {:?}",
            repo::probe_state(&db, id).await.unwrap()
        );
        if expected.is_none() {
            assert_eq!(
                repo::probe_state(&db, id)
                    .await
                    .unwrap()
                    .unwrap()
                    .status
                    .as_deref(),
                Some("unsupported")
            );
        }
    }
    for (name, bytes) in [
        ("broken.pkg", b"truncated".as_slice()),
        (
            "invalid.pkg",
            pkg("NOTAPKG", &[("invalid.json", auto)]).as_slice(),
        ),
    ] {
        std::fs::write(root.path().join(name), bytes).unwrap();
        let id = seed_typed(&db, root.path(), name, "", "scene").await;
        task::run_pending(&db, probe.clone()).await.unwrap();
        assert_eq!(
            repo::probe_state(&db, id)
                .await
                .unwrap()
                .unwrap()
                .status
                .as_deref(),
            Some("failed"),
            "{name}"
        );
    }
    assert_eq!(task::run_pending(&db, probe).await.unwrap().media_probed, 0);
}

#[tokio::test]
async fn owe_script_update_retries_failed_and_unsupported_inputs() {
    let Some(path) = std::env::var_os("WAYWALLEN_TEST_OWE_PROBE") else {
        return;
    };
    let script = std::fs::read_to_string(path).unwrap();
    let root = tempfile::tempdir().unwrap();
    let Some(probe) = service(root.path(), "return {probe=function(item) if item.path:match('/scene%.pkg$') then error('Unsupported PKG version') end end}", &[]) else { return; };
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let scene = br#"{"general":{"orthogonalprojection":{"width":1920,"height":1080}}}"#;
    let mut ids = Vec::new();
    for (name, version, entry) in [
        ("scene.pkg", "PKGV0024", "scene.json"),
        ("gifscene.pkg", "PKGV0001", "gifscene.json"),
    ] {
        std::fs::write(root.path().join(name), pkg(version, &[(entry, scene)])).unwrap();
        ids.push(seed_typed(&db, root.path(), name, "", "scene").await);
    }
    assert_eq!(
        task::run_pending(&db, probe.clone())
            .await
            .unwrap()
            .media_probed,
        2
    );
    for (id, status) in ids.iter().zip(["failed", "unsupported"]) {
        assert_eq!(
            repo::probe_state(&db, *id)
                .await
                .unwrap()
                .unwrap()
                .status
                .as_deref(),
            Some(status)
        );
    }
    assert_eq!(
        task::run_pending(&db, probe.clone())
            .await
            .unwrap()
            .media_probed,
        0
    );
    std::fs::write(root.path().join("plugin/probe.lua"), script).unwrap();
    assert!(crate::probe::task::inputs_changed(&db, &probe)
        .await
        .unwrap());
    assert_eq!(task::run_pending(&db, probe).await.unwrap().media_probed, 2);
    for id in ids {
        let entry = repo::get_entry(&db, id).await.unwrap().unwrap();
        assert_eq!((entry.width, entry.height), (Some(1920), Some(1080)));
    }
}
