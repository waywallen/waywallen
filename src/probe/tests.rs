use super::{media::ProbeService, task};
use crate::model::{repo, sync};
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

pub(super) fn rewrite_preserving_stat(path: &std::path::Path, contents: &[u8]) {
    use std::io::Write;
    let before = super::stat::snapshot(path.to_str().unwrap());
    assert_eq!(before.size, Some(contents.len() as i64));
    let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    let modified = file.metadata().unwrap().modified().unwrap();
    file.write_all(contents).unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert_eq!(super::stat::snapshot(path.to_str().unwrap()), before);
}

fn fake_tool(root: &std::path::Path) -> std::path::PathBuf {
    let tool = root.join("probe.py");
    std::fs::write(&tool, r#"#!/usr/bin/env python3
import sys,json,os,time
print(json.dumps({'event':'ready','protocol_version':2}),flush=True)
def stamp(path):
    try:
        s=os.stat(path)
        return dict(path=path,kind='directory' if os.path.isdir(path) else 'file',size=s.st_size,mtime_ns=s.st_mtime_ns,device=s.st_dev,inode=s.st_ino)
    except OSError:
        return dict(path=path,kind='missing',size=None)
for line in sys.stdin:
    if json.loads(line).get('event') == 'configure':
        print(json.dumps({'event':'configured'}),flush=True)
        continue
    j=json.loads(line); path=j['path']; deps=[stamp(path)]
    with open(os.path.join(os.path.dirname(__file__),'calls'),'a') as log: log.write(str(os.getpid())+'\n')
    print(json.dumps(dict(event='resolved',request_id=j['request_id'],dependencies=deps,selected_path=path)),flush=True)
    data=open(path,'rb').read()
    if data.startswith(b'hang'): time.sleep(60)
    if data.startswith(b'crash'): os._exit(139)
    if data == b'oversized':
        print('x'*(2*1024*1024+1),flush=True)
        continue
    if data.startswith(b'protocol'):
        print('invalid protocol output',flush=True)
        continue
    if data == b'changing':
        with open(path,'wb') as output: output.write(b'image')
    time.sleep(0.05)
    result=dict(event='result',request_id=j['request_id'],dependencies=deps,selected_path=path)
    if data.startswith(b'bad'):
        print('test decoder failed',file=sys.stderr,flush=True)
        result.update(status='failed',error='test decoder failed')
    else: result.update(status='ok',width=2,height=1,size=len(data))
    if data == b'wrong-id': result['request_id']='unknown'
    if data == b'bad-dimensions': result['width']=0
    if data == b'wrong-event': result['event']='unknown'
    if data == b'duplicate': print(json.dumps(result),flush=True)
    print(json.dumps(result),flush=True)
"#).unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    tool
}

async fn seed(
    db: &sea_orm::DatabaseConnection,
    root: &std::path::Path,
    name: &str,
    sub_type: &str,
) -> i64 {
    seed_typed(db, root, name, sub_type, "image").await
}

async fn seed_typed(
    db: &sea_orm::DatabaseConnection,
    root: &std::path::Path,
    name: &str,
    sub_type: &str,
    ty: &str,
) -> i64 {
    let entry = serde_json::from_value(serde_json::json!({
        "name": name, "wp_type":ty, "sub_type":sub_type, "resource":root.join(name),
        "library_root":root, "preview":null, "plugin_name":"image"
    }))
    .unwrap();
    sync::sync_plugin_entries(
        db,
        sync::PluginRef {
            name: "image",
            version: "1",
        },
        &[entry],
        &[],
    )
    .await
    .unwrap();
    repo::load_entries(db)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.resource.ends_with(name))
        .unwrap()
        .item_id
}

#[path = "lua_tests.rs"]
mod lua_tests;

#[tokio::test]
async fn prepared_program_keys_are_stable_and_provider_replacements_are_detected() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("probe.lua"), "return 1").unwrap();
    let program = Arc::new(
        super::program::Program::open(
            "test",
            root.path(),
            &super::program::Declaration {
                entry: "probe.lua".into(),
                version: 1,
                types: vec!["scene".into()],
            },
            &["probe.lua".into()],
        )
        .unwrap(),
    );
    let service = ProbeService::with_tool(None);
    service.replace_programs([("test".into(), program.clone())].into());
    let first = service.prepare_programs().await.unwrap();
    let original = first.input_kind("test", "scene", "").unwrap();
    let same = service.prepare_programs().await.unwrap();
    assert_eq!(
        same.input_kind("test", "scene", "").as_deref(),
        Some(original.as_str())
    );
    rewrite_preserving_stat(&root.path().join("probe.lua"), b"return 2");
    let changed = service.prepare_programs().await.unwrap();
    assert_ne!(
        changed.input_kind("test", "scene", "").as_deref(),
        Some(original.as_str())
    );
    assert_eq!(
        first.input_kind("test", "scene", "").as_deref(),
        Some(original.as_str())
    );
    assert_eq!(
        first.input_kind("test", "video", "").as_deref(),
        Some("media")
    );
    assert!(service.program_is_current(&first, "test"));
    service.replace_programs([("test".into(), Arc::new(program.as_ref().clone()))].into());
    assert!(!service.program_is_current(&first, "test"));
    assert!(service.program_is_current(&service.prepare_programs().await.unwrap(), "test"));
}

#[tokio::test]
async fn input_versions_cache_success_and_failure_across_rescans() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("image.png");
    std::fs::write(&path, b"good").unwrap();
    let service = Arc::new(ProbeService::with_tool(Some(fake_tool(root.path()))));
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let id = seed(&db, root.path(), "image.png", "").await;
    let added = repo::get_entry(&db, id).await.unwrap().unwrap().added_at;
    assert_eq!(
        task::run_pending(&db, service.clone())
            .await
            .unwrap()
            .media_probed,
        1
    );
    assert_eq!(
        repo::get_entry(&db, id).await.unwrap().unwrap().width,
        Some(2)
    );
    seed(&db, root.path(), "image.png", "").await;
    assert_eq!(
        task::run_pending(&db, service.clone())
            .await
            .unwrap()
            .media_probed,
        0
    );
    std::fs::write(&path, b"bad decoder input").unwrap();
    assert_eq!(
        task::run_pending(&db, service.clone())
            .await
            .unwrap()
            .media_probed,
        1
    );
    assert!(repo::get_entry(&db, id)
        .await
        .unwrap()
        .unwrap()
        .width
        .is_none());
    let fresh = Arc::new(ProbeService::with_tool(Some(root.path().join("probe.py"))));
    assert_eq!(
        task::run_pending(&db, fresh.clone())
            .await
            .unwrap()
            .media_probed,
        0
    );
    std::fs::write(&path, b"good changed input").unwrap();
    assert_eq!(task::run_pending(&db, fresh).await.unwrap().media_probed, 1);
    assert_eq!(
        repo::get_entry(&db, id).await.unwrap().unwrap().added_at,
        added
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("calls"))
            .unwrap()
            .lines()
            .count(),
        3
    );
}

#[tokio::test]
async fn missing_tool_does_not_record_failed_attempts() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("image.png"), b"image").unwrap();
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let id = seed(&db, root.path(), "image.png", "").await;
    assert!(!ProbeService::with_tool(None).available());
    task::run_pending(&db, Arc::new(ProbeService::with_tool(None)))
        .await
        .unwrap();
    let state = repo::probe_state(&db, id).await.unwrap().unwrap();
    assert!(state.signature.is_none() && state.status.is_none() && state.attempted_at.is_none());
    assert_eq!(
        task::run_pending(
            &db,
            Arc::new(ProbeService::with_tool(Some(fake_tool(root.path()))))
        )
        .await
        .unwrap()
        .media_probed,
        1
    );
}

#[tokio::test]
async fn protocol_errors_do_not_poison_input_versions() {
    for input in [
        "protocol",
        "oversized",
        "wrong-id",
        "bad-dimensions",
        "wrong-event",
    ] {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("image.png"), input).unwrap();
        let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
        let id = seed(&db, root.path(), "image.png", "").await;
        let service = Arc::new(ProbeService::with_tool(Some(fake_tool(root.path()))));
        assert!(task::run_pending(&db, service).await.is_err(), "{input}");
        let state = repo::probe_state(&db, id).await.unwrap().unwrap();
        assert!(
            state.signature.is_none() && state.status.is_none(),
            "{input}"
        );
    }
}

#[tokio::test]
async fn invalid_handshakes_and_trailing_results_are_reported() {
    for input in ["handshake", "duplicate"] {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("image.png"), input).unwrap();
        let tool = fake_tool(root.path());
        if input == "handshake" {
            let script = std::fs::read_to_string(&tool)
                .unwrap()
                .replace("'protocol_version':2", "'protocol_version':3");
            std::fs::write(&tool, script).unwrap();
        }
        let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
        let id = seed(&db, root.path(), "image.png", "").await;
        assert!(
            task::run_pending(&db, Arc::new(ProbeService::with_tool(Some(tool))))
                .await
                .is_err()
        );
        if input == "handshake" {
            assert!(repo::probe_state(&db, id)
                .await
                .unwrap()
                .unwrap()
                .status
                .is_none());
        }
    }
}

#[tokio::test]
async fn blocked_configuration_is_bounded_and_not_cached_as_input_failure() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("scene.pkg"), b"image").unwrap();
    let tool = fake_tool(root.path());
    let script = std::fs::read_to_string(&tool).unwrap().replace(
        "for line in sys.stdin:",
        "time.sleep(60)\nfor line in sys.stdin:",
    );
    std::fs::write(&tool, script).unwrap();
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let id = seed_typed(&db, root.path(), "scene.pkg", "", "scene").await;
    let service = Arc::new(ProbeService::with_tool(Some(tool)));
    let program = super::program::Program {
        key: "test".into(),
        root: root.path().into(),
        entry: "probe.lua".into(),
        files: vec!["a".repeat(16384); 16],
        version: 1,
        types: vec!["scene".into()],
    };
    service.replace_programs([("image".into(), Arc::new(program))].into());
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(6),
        task::run_pending(&db, service),
    )
    .await
    .expect("configuration write must be bounded");
    assert!(result.is_err());
    let state = repo::probe_state(&db, id).await.unwrap().unwrap();
    assert!(state.signature.is_none() && state.attempted_at.is_none());
}

#[tokio::test]
async fn changed_input_results_are_discarded_and_writes_are_transactional() {
    use sea_orm::ConnectionTrait;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("image.png"), b"changing").unwrap();
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let id = seed(&db, root.path(), "image.png", "").await;
    let service = Arc::new(ProbeService::with_tool(Some(fake_tool(root.path()))));
    assert_eq!(
        task::run_pending(&db, service.clone())
            .await
            .unwrap()
            .media_probed,
        0
    );
    assert!(repo::probe_state(&db, id)
        .await
        .unwrap()
        .unwrap()
        .status
        .is_none());
    db.execute_unprepared("CREATE TRIGGER fail_probe BEFORE UPDATE OF width ON item WHEN NEW.width IS NOT NULL BEGIN SELECT RAISE(ABORT, 'test rollback'); END").await.unwrap();
    assert!(task::run_pending(&db, service.clone()).await.is_err());
    assert!(repo::probe_state(&db, id)
        .await
        .unwrap()
        .unwrap()
        .status
        .is_none());
    db.execute_unprepared("DROP TRIGGER fail_probe")
        .await
        .unwrap();
    assert_eq!(
        task::run_pending(&db, service).await.unwrap().media_probed,
        1
    );
}

#[tokio::test]
async fn cancelling_the_batch_reaps_workers_without_recording_failure() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("image.png"), b"hang").unwrap();
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let id = seed(&db, root.path(), "image.png", "").await;
    let service = Arc::new(ProbeService::with_tool(Some(fake_tool(root.path()))));
    let worker_db = db.clone();
    let task = tokio::spawn(async move { task::run_pending(&worker_db, service).await });
    let calls = root.path().join("calls");
    let pid = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if let Ok(value) = std::fs::read_to_string(&calls) {
                if let Ok(pid) = value.trim().parse::<u32>() {
                    break pid;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while std::path::Path::new(&format!("/proc/{pid}")).exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(repo::probe_state(&db, id)
        .await
        .unwrap()
        .unwrap()
        .status
        .is_none());
}

#[tokio::test]
async fn timed_out_and_crashed_inputs_are_cached_without_blocking_others() {
    for input in [b"hang".as_slice(), b"crash".as_slice()] {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("bad.png"), input).unwrap();
        std::fs::write(root.path().join("good.png"), b"image").unwrap();
        let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
        let id = seed(&db, root.path(), "bad.png", "").await;
        seed(&db, root.path(), "good.png", "").await;
        let service = Arc::new(ProbeService::with_timeout(
            fake_tool(root.path()),
            std::time::Duration::from_secs(1),
        ));
        assert_eq!(
            task::run_pending(&db, service.clone())
                .await
                .unwrap()
                .media_probed,
            2
        );
        assert_eq!(
            repo::probe_state(&db, id)
                .await
                .unwrap()
                .unwrap()
                .status
                .as_deref(),
            Some("failed")
        );
        assert_eq!(
            task::run_pending(&db, service).await.unwrap().media_probed,
            0
        );
    }
}

#[tokio::test]
async fn workers_reuse_processes_and_bound_parallelism() {
    let root = tempfile::tempdir().unwrap();
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    for i in 0..8 {
        let name = format!("{i}.png");
        std::fs::write(root.path().join(&name), b"image").unwrap();
        seed(&db, root.path(), &name, "").await;
    }
    let service = Arc::new(ProbeService::with_tool(Some(fake_tool(root.path()))));
    assert_eq!(
        task::run_pending(&db, service).await.unwrap().media_probed,
        8
    );
    let calls = std::fs::read_to_string(root.path().join("calls")).unwrap();
    let workers: std::collections::HashSet<_> = calls.lines().collect();
    assert!(workers.len() <= 4);
    if std::thread::available_parallelism().unwrap().get() > 1 {
        assert!(workers.len() > 1);
    }
    assert_eq!(calls.lines().count(), 8);
}

#[tokio::test]
async fn plugin_media_fields_take_precedence_and_cached_results_can_be_reused() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("image.png"), b"image").unwrap();
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let mut entry: crate::catalog::entry::WallpaperEntry = serde_json::from_value(serde_json::json!({
        "name":"image", "wp_type":"image", "resource":root.path().join("image.png"),
        "library_root":root.path(), "plugin_name":"image", "size":123456, "width":1920,"height":1080
    })).unwrap();
    let plugin = sync::PluginRef {
        name: "image",
        version: "1",
    };
    sync::sync_plugin_entries(&db, plugin.clone(), &[entry.clone()], &[])
        .await
        .unwrap();
    let service = Arc::new(ProbeService::with_tool(Some(fake_tool(root.path()))));
    task::run_pending(&db, service.clone()).await.unwrap();
    let before = repo::load_entries(&db).await.unwrap().remove(0);
    assert_eq!(
        (before.size, before.width, before.height),
        (Some(123456), Some(1920), Some(1080))
    );
    entry.size = None;
    entry.width = None;
    entry.height = None;
    sync::sync_plugin_entries(&db, plugin, &[entry], &[])
        .await
        .unwrap();
    assert_eq!(
        task::run_pending(&db, service).await.unwrap().media_probed,
        0
    );
    let after = repo::load_entries(&db).await.unwrap().remove(0);
    assert_eq!(
        (after.size, after.width, after.height),
        (Some(5), Some(2), Some(1))
    );
}

#[tokio::test]
async fn native_media_probe_handles_video_and_invalid_inputs() {
    let Some(tool) = std::env::var_os("WAYWALLEN_TEST_PROBE") else {
        eprintln!("native probe not supplied; skipping runtime check");
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let video = "video \n片.y4m";
    let bytes = b"YUV4MPEG2 W2 H2 F25:1 Ip A1:1 C420jpeg\nFRAME\n\x10\x10\x10\x10\x80\x80";
    std::fs::write(root.path().join(video), bytes).unwrap();
    std::fs::write(root.path().join("broken.png"), b"invalid image").unwrap();
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let video_id = seed(&db, root.path(), video, "").await;
    let broken_id = seed(&db, root.path(), "broken.png", "").await;
    let service = Arc::new(ProbeService::with_tool(Some(tool.into())));
    assert_eq!(
        task::run_pending(&db, service.clone())
            .await
            .unwrap()
            .media_probed,
        2
    );
    let entry = repo::get_entry(&db, video_id).await.unwrap().unwrap();
    assert_eq!(
        (entry.width, entry.height, entry.size),
        (Some(2), Some(2), Some(bytes.len() as i64))
    );
    assert_eq!(
        repo::probe_state(&db, broken_id)
            .await
            .unwrap()
            .unwrap()
            .status
            .as_deref(),
        Some("failed")
    );
    assert_eq!(
        task::run_pending(&db, service).await.unwrap().media_probed,
        0
    );
}

#[tokio::test]
async fn native_timeline_metadata_uses_representative_image_and_dependencies() {
    let Some(tool) = std::env::var_os("WAYWALLEN_TEST_PROBE") else {
        eprintln!("native probe not supplied; skipping runtime check");
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let image = root.path().join("day.ppm");
    std::fs::write(&image, b"P6\n2 1\n255\n\xff\x00\x00\x00\xff\x00").unwrap();
    std::fs::write(root.path().join("pair.xml"),b"<wallpapers><wallpaper><filename>day.ppm</filename><filename-dark>missing.ppm</filename-dark></wallpaper></wallpapers>").unwrap();
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let id = seed(&db, root.path(), "pair.xml", "timeline").await;
    let service = Arc::new(ProbeService::with_tool(Some(tool.into())));
    assert_eq!(
        task::run_pending(&db, service.clone())
            .await
            .unwrap()
            .media_probed,
        1
    );
    let entry = repo::get_entry(&db, id).await.unwrap().unwrap();
    assert_eq!((entry.width, entry.height), (Some(2), Some(1)));
    assert_eq!(
        entry.size,
        Some(std::fs::metadata(&image).unwrap().len() as i64)
    );
    assert!(entry.resource.ends_with("pair.xml") && entry.preview.is_none());
    assert_eq!(
        task::run_pending(&db, service.clone())
            .await
            .unwrap()
            .media_probed,
        0
    );
    std::fs::write(&image, b"P6\n1 1\n255\n\xff\x00\x00").unwrap();
    assert_eq!(
        task::run_pending(&db, service).await.unwrap().media_probed,
        1
    );
    assert_eq!(
        repo::get_entry(&db, id).await.unwrap().unwrap().width,
        Some(1)
    );
}

#[tokio::test]
async fn native_kde_candidate_addition_invalidates_the_saved_result() {
    let Some(tool) = std::env::var_os("WAYWALLEN_TEST_PROBE") else {
        eprintln!("native probe not supplied; skipping runtime check");
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let images = root.path().join("contents/images");
    std::fs::create_dir_all(&images).unwrap();
    std::fs::write(
        root.path().join("metadata.json"),
        br#"{"KPlugin":{"Name":"Test"}}"#,
    )
    .unwrap();
    std::fs::write(
        images.join("1920x1080.png"),
        b"P6\n2 1\n255\n\xff\x00\x00\x00\xff\x00",
    )
    .unwrap();
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let id = seed(&db, root.path(), "metadata.json", "timeline").await;
    let service = Arc::new(ProbeService::with_tool(Some(tool.into())));
    task::run_pending(&db, service.clone()).await.unwrap();
    assert_eq!(
        repo::get_entry(&db, id).await.unwrap().unwrap().width,
        Some(2)
    );
    std::fs::write(images.join("1280x720.png"), b"P6\n1 1\n255\n\xff\x00\x00").unwrap();
    assert_eq!(
        task::run_pending(&db, service).await.unwrap().media_probed,
        1
    );
    assert_eq!(
        repo::get_entry(&db, id).await.unwrap().unwrap().width,
        Some(1)
    );
}

#[tokio::test]
async fn native_timeline_missing_image_and_symlink_target_changes_are_detected() {
    let Some(tool) = std::env::var_os("WAYWALLEN_TEST_PROBE") else {
        eprintln!("native probe not supplied; skipping runtime check");
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let link = root.path().join("selected.ppm");
    std::os::unix::fs::symlink("day.ppm", &link).unwrap();
    std::fs::write(
        root.path().join("pair.xml"),
        b"<wallpapers><wallpaper><filename>selected.ppm</filename></wallpaper></wallpapers>",
    )
    .unwrap();
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let id = seed(&db, root.path(), "pair.xml", "timeline").await;
    let service = Arc::new(ProbeService::with_tool(Some(tool.into())));
    task::run_pending(&db, service.clone()).await.unwrap();
    assert_eq!(
        repo::probe_state(&db, id)
            .await
            .unwrap()
            .unwrap()
            .status
            .as_deref(),
        Some("failed")
    );
    assert_eq!(
        task::run_pending(&db, service.clone())
            .await
            .unwrap()
            .media_probed,
        0
    );
    std::fs::write(
        root.path().join("day.ppm"),
        b"P6\n2 1\n255\n\xff\x00\x00\x00\xff\x00",
    )
    .unwrap();
    assert_eq!(
        task::run_pending(&db, service.clone())
            .await
            .unwrap()
            .media_probed,
        1
    );
    std::fs::write(root.path().join("night.ppm"), b"P6\n1 1\n255\n\xff\x00\x00").unwrap();
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink("night.ppm", &link).unwrap();
    assert_eq!(
        task::run_pending(&db, service).await.unwrap().media_probed,
        1
    );
    assert_eq!(
        repo::get_entry(&db, id).await.unwrap().unwrap().width,
        Some(1)
    );
}
