use super::*;
use std::collections::HashMap;
use std::io::{Cursor, Write};
use std::sync::Mutex;
use tokio::io::AsyncReadExt;

use crate::plugin::source::SourceManager;
use crate::plugin::source::ENTRY_VERSION_V6;
use crate::plugin::state_store::PluginStateStore;
use crate::probe::media::ProbeService;

#[derive(Clone)]
struct Reply {
    status: u16,
    bytes: Vec<u8>,
    chunked: bool,
}

struct Http {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Http {
    async fn new(routes: HashMap<String, Reply>) -> Self {
        Self::with_headers(routes, "").await
    }

    async fn with_headers(routes: HashMap<String, Reply>, headers: &'static str) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let received = requests.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0_u8; 1];
                    if stream.read_exact(&mut byte).await.is_err() {
                        break;
                    }
                    request.push(byte[0]);
                    assert!(request.len() <= 16 * 1024);
                }
                let request = String::from_utf8(request).unwrap();
                let target = request
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap();
                let path = target.split('?').next().unwrap();
                let reply = routes.get(path).cloned().unwrap_or(Reply {
                    status: 404,
                    bytes: Vec::new(),
                    chunked: false,
                });
                received.lock().unwrap().push(request);
                let mut response =
                    format!("HTTP/1.1 {} Test\r\nConnection: close\r\n", reply.status);
                response.push_str(headers);
                if reply.chunked {
                    response.push_str("Transfer-Encoding: chunked\r\n\r\n");
                    if !reply.bytes.is_empty() {
                        response.push_str(&format!("{:x}\r\n", reply.bytes.len()));
                    }
                } else {
                    response.push_str(&format!("Content-Length: {}\r\n\r\n", reply.bytes.len()));
                }
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.write_all(&reply.bytes).await;
                if reply.chunked {
                    let ending = if reply.bytes.is_empty() {
                        b"0\r\n\r\n".as_slice()
                    } else {
                        b"\r\n0\r\n\r\n".as_slice()
                    };
                    let _ = stream.write_all(ending).await;
                }
                let _ = stream.shutdown().await;
            }
        });
        Self {
            url,
            requests,
            task,
        }
    }

    fn assert_requests(&self, expected: usize) {
        let requests = self.requests.lock().unwrap();
        assert_eq!(requests.len(), expected, "{requests:?}");
        for request in requests.iter() {
            let lower = request.to_ascii_lowercase();
            assert!(!lower.contains("\r\nauthorization:"), "{request}");
            assert!(!lower.contains("\r\ncookie:"), "{request}");
        }
    }
}

impl Drop for Http {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o644);
    for (name, bytes) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn routes(zip: Vec<u8>) -> HashMap<String, Reply> {
    HashMap::from([
        (
            "/model.zip".into(),
            Reply {
                status: 200,
                bytes: zip,
                chunked: false,
            },
        ),
        (
            "/preview.png".into(),
            Reply {
                status: 200,
                bytes: b"preview bytes".to_vec(),
                chunked: false,
            },
        ),
    ])
}

fn presentation_snapshot() -> serde_json::Map<String, serde_json::Value> {
    serde_json::json!({
        "sketchfab": {
            "version": 2,
            "adapter_version": 1,
            "model_id": "model",
            "source": "https://sketchfab.com/i/models/model/options",
            "captured_at": 123,
            "profile": {
                "version": 2,
                "directional_lights": [],
                "point_lights": [],
                "environment": {"ibl_intensity": 0, "background_color": [0.18, 0.18, 0.18]},
                "display": {"tone_mapping": "linear", "exposure": 0.0},
            },
            "sections": {
                "camera": {"status": "missing"},
                "lighting": {"status": "mapped"},
                "background": {"status": "mapped"},
                "post_process": {"status": "mapped"},
                "materials": {"status": "unsupported"},
            },
        },
    })
    .as_object()
    .unwrap()
    .clone()
}

fn source(url: &str, resource: &str) -> (tempfile::TempDir, SourceManager) {
    let directory = tempfile::tempdir().unwrap();
    let presentation = serde_json::to_string(&presentation_snapshot()).unwrap();
    let script = format!(
        r#"
local downloads = 0
local M = {{}}
function M.info()
    return {{name='test',capabilities={{discover={{search=true,download=true,resolve=true}}}}}}
end
M.discover = {{}}
function M.discover.search(ctx, params) return {{items={{}},has_more=false}} end
function M.discover.download(ctx, id)
    downloads = downloads + 1
    assert(downloads == 1, 'download metadata must not be fetched again by resolve')
    return {{
        url={url:?} .. '/model.zip?Signature=temporary-signature',filename='model.zip',archive='zip',
        wp_type='gltf',title='A model',external_id=id,description='Model description',
        tags={{'3d','sample'}},web_url='https://example.invalid/models/' .. id,
        preview_url={url:?} .. '/preview.png',size=1,
        attribution={{author='Artist',author_url='https://example.invalid/artist',license='CC BY',
            license_url='https://example.invalid/license',requirements='Credit Artist'}},
        provider_data=ctx.json.strict.decode({presentation:?}),
    }}
end
function M.discover.resolve(ctx, params)
    assert(downloads == 1)
    local m = params.metadata
    assert(m and m.title == 'A model' and m.external_id == params.id)
    assert(m.url == nil and m.filename == nil)
    assert(m.attribution.author == 'Artist')
    assert(m.provider_data.sketchfab.version == 2 and m.provider_data.sketchfab.adapter_version == 1)
    assert(m.provider_data.sketchfab.profile.version == 2)
    return {{name=m.title,wp_type=m.wp_type,resource={resource:?},description=m.description,
        tags=m.tags,external_id=m.external_id}}
end
return M
"#
    );
    let entry = directory.path().join("main.lua");
    std::fs::write(&entry, script).unwrap();
    let manager = SourceManager::with_state_store(PluginStateStore::new(
        directory.path().join("state"),
        directory.path().join("legacy"),
    ))
    .unwrap();
    manager
        .load_plugin(&entry, "org.test.remote", "1", ENTRY_VERSION_V6)
        .unwrap();
    (directory, manager)
}

fn assert_empty(path: &Path) {
    assert_eq!(
        std::fs::read_dir(path).unwrap().count(),
        0,
        "{} is not empty",
        path.display()
    );
}

#[test]
fn remote_download_progress_throttles_but_always_reports_boundaries() {
    let now = Instant::now();
    let mut throttle = DownloadProgressThrottle::default();
    assert!(throttle.should_report(now, false));
    assert!(!throttle.should_report(now + Duration::from_millis(1), false));
    assert!(!throttle.should_report(now + Duration::from_millis(249), false));
    assert!(throttle.should_report(now + Duration::from_millis(250), false));
    assert!(!throttle.should_report(now + Duration::from_millis(251), false));
    assert!(throttle.should_report(now + Duration::from_millis(251), true));
    assert!(!throttle.should_report(now + Duration::from_millis(500), false));
    assert!(throttle.should_report(now + Duration::from_millis(501), false));
}

#[tokio::test]
async fn remote_download_progress_uses_received_bytes_and_optional_response_length() {
    for chunked in [false, true] {
        for bytes in [Vec::new(), vec![1; 17]] {
            let http = Http::new(HashMap::from([(
                "/model".into(),
                Reply {
                    status: 200,
                    bytes: bytes.clone(),
                    chunked,
                },
            )]))
            .await;
            let root = tempfile::tempdir().unwrap();
            let target = root.path().join("model");
            let mut progress = Vec::new();
            download_remote_file(
                &format!("{}/model", http.url),
                &target,
                DOWNLOAD_LIMIT,
                |value| progress.push(value),
            )
            .await
            .unwrap();
            assert_eq!(std::fs::read(&target).unwrap(), bytes);
            let total_bytes = (!chunked).then_some(bytes.len() as u64);
            assert_eq!(
                progress.first(),
                Some(&RemoteDownloadTransfer {
                    received_bytes: 0,
                    total_bytes,
                })
            );
            assert_eq!(
                progress.last(),
                Some(&RemoteDownloadTransfer {
                    received_bytes: bytes.len() as u64,
                    total_bytes,
                })
            );
            assert!(progress.len() >= 2);
            assert!(progress
                .iter()
                .all(|value| value.total_bytes == total_bytes));
            assert!(progress
                .windows(2)
                .all(|values| values[0].received_bytes <= values[1].received_bytes));
            http.assert_requests(1);
        }
    }
}

#[tokio::test]
async fn remote_download_keeps_single_file_layout_and_nonsecret_sidecar() {
    let bytes = b"image bytes";
    let http = Http::new(HashMap::from([(
        "/image.png".into(),
        Reply {
            status: 200,
            bytes: bytes.to_vec(),
            chunked: false,
        },
    )]))
    .await;
    let root = tempfile::tempdir().unwrap();
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let metadata = DiscoverMetadata {
        wp_type: "image".into(),
        title: "An image".into(),
        external_id: "image".into(),
        tags: vec!["sample".into()],
        web_url: "https://example.invalid/image".into(),
        ..Default::default()
    };
    let target = root.path().join(safe_remote_filename("image.png", "image"));
    download_remote_file(
        &format!("{}/image.png?signature=temporary-signature", http.url),
        &target,
        DOWNLOAD_LIMIT,
        |_| {},
    )
    .await
    .unwrap();
    write_remote_sidecar(&target, &metadata).await.unwrap();
    upsert_remote_download(
        &db,
        "file-provider",
        "1",
        root.path(),
        &target,
        None,
        &metadata,
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), bytes);
    let sidecar = std::fs::read_to_string(sidecar_path(&target)).unwrap();
    assert!(!sidecar.contains("temporary-signature"));
    assert!(!sidecar.contains("Authorization"));
    let stored: DiscoverMetadata = serde_json::from_str(&sidecar).unwrap();
    assert_eq!(stored.title, "An image");
    let entries = repo::load_entries(&db).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].wp_type, "image");
    assert_eq!(entries[0].resource, target.to_str().unwrap());
    assert_eq!(entries[0].preview, None);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
    http.assert_requests(1);
}

#[tokio::test]
async fn remote_download_imports_archive_preview_metadata_and_reuses_publication() {
    let scene = br#"{"asset":{"version":"2.0"},"buffers":[{"uri":"scene.bin","byteLength":4}]}"#;
    let mesh = b"mesh";
    let texture = b"texture bytes";
    let zip = archive(&[
        ("scene.gltf", scene),
        ("scene.bin", mesh),
        ("textures/base.png", texture),
    ]);
    let transfer_bytes = zip.len() as u64;
    let http = Http::new(routes(zip)).await;
    let (_plugin, manager) = source(&http.url, "scene.gltf");
    let root = tempfile::tempdir().unwrap();
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let info = manager.call_download("test", "model").await.unwrap();
    let mut progress = Vec::new();
    let prepared = prepare_artifact(
        &manager.download_session("test").unwrap(),
        "test",
        "model",
        root.path(),
        info.clone(),
        |value| progress.push(value),
    )
    .await
    .unwrap();
    assert_eq!(
        progress.first(),
        Some(&Some(RemoteDownloadTransfer {
            received_bytes: 0,
            total_bytes: Some(transfer_bytes),
        }))
    );
    assert_eq!(progress.last(), Some(&None));
    assert_eq!(progress.iter().filter(|value| value.is_none()).count(), 1);
    let transfers: Vec<_> = progress.iter().flatten().collect();
    assert!(transfers
        .iter()
        .all(|value| value.total_bytes == Some(transfer_bytes)));
    assert_eq!(transfers.last().unwrap().received_bytes, transfer_bytes);
    let publication = prepared.publish(root.path()).unwrap();
    let installed = &publication.installed;
    assert_eq!(std::fs::read(installed.resource()).unwrap(), scene);
    assert_eq!(
        std::fs::read(installed.root.join("scene.bin")).unwrap(),
        mesh
    );
    assert_eq!(
        std::fs::read(installed.root.join("textures/base.png")).unwrap(),
        texture
    );
    assert_eq!(
        std::fs::read(installed.preview().unwrap()).unwrap(),
        b"preview bytes"
    );
    assert_eq!(installed.artifact.source_id, "test");
    assert_eq!(installed.artifact.remote_id, "model");
    assert_eq!(
        installed.artifact.metadata.provider_data,
        Some(presentation_snapshot())
    );
    assert_eq!(
        installed.artifact.metadata.size,
        Some((scene.len() + mesh.len() + texture.len()) as i64)
    );
    let credits = installed.artifact.metadata.attribution.as_ref().unwrap();
    assert_eq!(credits.author, "Artist");
    assert_eq!(credits.author_url, "https://example.invalid/artist");
    assert_eq!(credits.license, "CC BY");
    assert_eq!(credits.license_url, "https://example.invalid/license");
    assert_eq!(credits.requirements, "Credit Artist");
    let persisted = std::fs::read_to_string(installed.root.join(".waywallen/asset.json")).unwrap();
    assert!(!persisted.contains("temporary-signature"));
    assert!(!persisted.contains("model.zip"));
    assert!(!persisted.contains("Authorization"));
    assert!(!persisted.contains("Token "));
    upsert_remote_download(
        &db,
        "test",
        "1",
        root.path(),
        &installed.resource(),
        installed.preview().as_deref(),
        &installed.artifact.metadata,
    )
    .await
    .unwrap();
    let installed = publication.commit();
    let stats = crate::probe::task::run_pending(&db, Arc::new(ProbeService::with_tool(None)))
        .await
        .unwrap();
    assert_eq!(stats.stat_done, 1);
    let entries = repo::load_entries(&db).await.unwrap();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.plugin_name, "test");
    assert_eq!(entry.wp_type, "gltf");
    assert_eq!(entry.size, installed.artifact.metadata.size);
    assert!(entry.file_mtime_ns.is_some());
    assert_eq!(entry.resource, installed.resource().to_str().unwrap());
    assert_eq!(
        entry.preview.as_deref(),
        installed.preview().as_deref().and_then(Path::to_str)
    );
    assert_eq!(entry.external_id.as_deref(), Some("model"));
    assert_eq!(entry.name, "A model");
    assert_eq!(
        entry.web_url.as_deref(),
        Some("https://example.invalid/models/model")
    );
    assert_eq!(
        repo::list_tags_of_item(&db, entry.item_id)
            .await
            .unwrap()
            .into_iter()
            .map(|tag| tag.name)
            .collect::<Vec<_>>(),
        ["3d", "sample"]
    );
    let plugin = repo::find_plugin_by_name(&db, "test")
        .await
        .unwrap()
        .unwrap();
    let library = repo::find_library(&db, plugin.id, root.path().to_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        repo::get_library_metadata_value(&db, library.id, repo::LIBRARY_METADATA_MANAGED_KEY)
            .await
            .unwrap()
            .as_deref(),
        Some(repo::LIBRARY_METADATA_MANAGED_REMOTE)
    );
    let reloaded = remote_asset::load(root.path(), "test", "model")
        .unwrap()
        .unwrap();
    assert_eq!(
        reloaded.artifact.metadata.provider_data,
        Some(presentation_snapshot())
    );
    assert_eq!(
        reloaded.artifact.metadata.attribution.unwrap().requirements,
        "Credit Artist"
    );
    use std::os::unix::fs::MetadataExt;
    let original_inode = std::fs::metadata(installed.resource()).unwrap().ino();
    let replacement = prepare_artifact(
        &manager.download_session("test").unwrap(),
        "test",
        "model",
        root.path(),
        info,
        |_| {},
    )
    .await
    .unwrap();
    let reused = replacement.publish(root.path()).unwrap().commit();
    assert_eq!(reused.resource(), installed.resource());
    assert_eq!(
        reused.artifact.metadata.provider_data,
        Some(presentation_snapshot())
    );
    assert_eq!(std::fs::read(reused.resource()).unwrap(), scene);
    assert_eq!(
        std::fs::metadata(reused.resource()).unwrap().ino(),
        original_inode
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    http.assert_requests(4);

    let bytes = b"single-file model";
    let http = Http::new(routes(bytes.to_vec())).await;
    let (_plugin, manager) = source(&http.url, "scene.glb");
    let root = tempfile::tempdir().unwrap();
    let mut info = manager.call_download("test", "model").await.unwrap();
    assert!(info.transfer.resolve);
    info.transfer.format = DownloadFormat::File;
    info.transfer.filename = "../scene.glb".into();
    let provider_data = presentation_snapshot();
    info.metadata.provider_data = Some(provider_data.clone());
    let publication = prepare_artifact(
        &manager.download_session("test").unwrap(),
        "test",
        "model",
        root.path(),
        info.clone(),
        |_| {},
    )
    .await
    .unwrap()
    .publish(root.path())
    .unwrap();
    assert!(publication.installed.resource().is_file());
    assert_eq!(
        publication.installed.artifact.metadata.provider_data,
        Some(provider_data.clone())
    );
    drop(publication);
    assert_empty(root.path());

    let installed = prepare_artifact(
        &manager.download_session("test").unwrap(),
        "test",
        "model",
        root.path(),
        info.clone(),
        |_| {},
    )
    .await
    .unwrap()
    .publish(root.path())
    .unwrap()
    .commit();
    assert_eq!(installed.artifact.resource, Path::new("scene.glb"));
    assert_eq!(std::fs::read(installed.resource()).unwrap(), bytes);
    assert_eq!(installed.artifact.metadata.size, Some(bytes.len() as i64));
    assert_eq!(installed.artifact.files.len(), 1);
    assert_eq!(installed.artifact.files[0].path, Path::new("scene.glb"));
    assert_eq!(installed.artifact.files[0].bytes, bytes.len() as u64);
    let reloaded = remote_asset::load(root.path(), "test", "model")
        .unwrap()
        .unwrap();
    assert_eq!(
        reloaded.artifact.metadata.provider_data,
        Some(provider_data.clone())
    );
    let persisted = std::fs::read_to_string(installed.root.join(".waywallen/asset.json")).unwrap();
    assert!(!persisted.contains("temporary-signature"));
    let original_inode = std::fs::metadata(installed.resource()).unwrap().ino();
    let mut newer = serde_json::Value::Object(provider_data.clone());
    newer["sketchfab"]["captured_at"] = 456.into();
    newer["sketchfab"]["profile"]["display"]["exposure"] = 2.into();
    info.metadata.provider_data = Some(newer.as_object().unwrap().clone());
    let reused = prepare_artifact(
        &manager.download_session("test").unwrap(),
        "test",
        "model",
        root.path(),
        info,
        |_| {},
    )
    .await
    .unwrap()
    .publish(root.path())
    .unwrap()
    .commit();
    assert_eq!(reused.artifact.metadata.provider_data, Some(provider_data));
    assert_eq!(
        std::fs::metadata(reused.resource()).unwrap().ino(),
        original_inode
    );
    let removal = remote_asset::remove(root.path(), "test", &reused.resource())
        .await
        .unwrap();
    assert!(removal.removed);
    assert_empty(root.path());
    http.assert_requests(6);
}

#[tokio::test]
async fn remote_download_rejects_unsafe_archive_and_invalid_resolve_without_residue() {
    for (entries, resource) in [
        (
            vec![
                ("../escape.txt", b"escape".as_slice()),
                ("scene.gltf", b"model".as_slice()),
            ],
            "scene.gltf",
        ),
        (vec![("scene.gltf", b"model".as_slice())], "../outside.gltf"),
        (
            vec![("textures/base.png", b"texture".as_slice())],
            "scene.gltf",
        ),
    ] {
        let http = Http::new(routes(archive(&entries))).await;
        let (_plugin, manager) = source(&http.url, resource);
        let root = tempfile::tempdir().unwrap();
        let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
        let info = manager.call_download("test", "model").await.unwrap();
        assert!(prepare_artifact(
            &manager.download_session("test").unwrap(),
            "test",
            "model",
            root.path(),
            info,
            |_| {}
        )
        .await
        .is_err());
        assert_empty(root.path());
        assert!(repo::list_items_all(&db).await.unwrap().is_empty());
        assert!(!root.path().join("escape.txt").exists());
        http.assert_requests(1);
    }
}

#[tokio::test]
async fn remote_download_error_redacts_signed_query_and_cleans_temporary_files() {
    let http = Http::new(HashMap::from([(
        "/denied".into(),
        Reply {
            status: 403,
            bytes: b"denied".to_vec(),
            chunked: false,
        },
    )]))
    .await;
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("download.zip");
    let url = format!(
        "{}/denied?Signature=temporary-signature&token=temporary-token",
        http.url
    );
    let mut progress = Vec::new();
    let error = download_remote_file(&url, &target, 1024, |value| progress.push(value))
        .await
        .unwrap_err();
    assert!(progress.is_empty());
    let error = format!("{error:#}");
    assert!(error.contains("403"));
    assert!(!error.contains("temporary-signature"));
    assert!(!error.contains("temporary-token"));
    assert!(!error.contains("Signature="));
    assert_empty(root.path());
    http.assert_requests(1);
}

#[tokio::test]
async fn remote_download_size_limits_cover_headers_and_streaming_cleanup() {
    for chunked in [false, true] {
        let http = Http::new(HashMap::from([(
            "/large".into(),
            Reply {
                status: 200,
                bytes: vec![1; 17],
                chunked,
            },
        )]))
        .await;
        let root = tempfile::tempdir().unwrap();
        let mut progress = Vec::new();
        let error = download_remote_file(
            &format!("{}/large", http.url),
            &root.path().join("large"),
            8,
            |value| progress.push(value),
        )
        .await
        .unwrap_err();
        assert!(format!("{error:#}").contains("size limit"));
        assert!(progress.iter().all(|value| value.received_bytes <= 8));
        if !chunked {
            assert!(progress.is_empty());
        }
        assert_empty(root.path());
        http.assert_requests(1);
    }
}

#[tokio::test]
async fn auxiliary_downloads_preserve_encoded_bytes_and_check_exact_sizes() {
    let gzip = vec![
        31, 139, 8, 0, 0, 0, 0, 0, 0, 3, 203, 72, 205, 201, 201, 7, 0, 134, 166, 16, 54, 5, 0, 0, 0,
    ];
    for chunked in [false, true] {
        let http = Http::with_headers(
            HashMap::from([(
                "/environment".into(),
                Reply {
                    status: 200,
                    bytes: gzip.clone(),
                    chunked,
                },
            )]),
            "Content-Encoding: gzip\r\n",
        )
        .await;
        for expected in [gzip.len() as u64, gzip.len() as u64 + 1] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("environment");
            let result = download_remote_file_checked(
                &format!("{}/environment", http.url),
                &path,
                1024,
                Some(expected),
                true,
                None,
                |_| Ok(()),
            )
            .await;
            if expected == gzip.len() as u64 {
                result.unwrap();
                assert_eq!(std::fs::read(path).unwrap(), gzip);
            } else {
                assert!(format!("{:#}", result.unwrap_err()).contains("match descriptor"));
                assert_empty(root.path());
            }
        }
        http.assert_requests(2);
    }
}

#[tokio::test]
async fn auxiliary_downloads_enforce_the_shared_byte_budget_before_writing() {
    let http = Http::new(HashMap::from([(
        "/environment".into(),
        Reply {
            status: 200,
            bytes: vec![1; 17],
            chunked: true,
        },
    )]))
    .await;
    let root = tempfile::tempdir().unwrap();
    let budget = AtomicU64::new(crate::remote_asset::prepare::INPUT_LIMIT - 8);
    let error = download_remote_file_checked(
        &format!("{}/environment", http.url),
        &root.path().join("environment"),
        1024,
        None,
        true,
        Some(&budget),
        |_| Ok(()),
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("total size limit"));
    assert_empty(root.path());
}

#[tokio::test]
async fn artifact_transfers_deduplicate_urls_and_report_aggregate_progress() {
    use crate::plugin::source::{DiscoverResource, DiscoverTransfer};

    for chunked in [false, true] {
        let http = Http::new(HashMap::from([
            (
                "/model".into(),
                Reply {
                    status: 200,
                    bytes: b"model".to_vec(),
                    chunked: false,
                },
            ),
            (
                "/environment".into(),
                Reply {
                    status: 200,
                    bytes: b"environment".to_vec(),
                    chunked,
                },
            ),
        ]))
        .await;
        let root = tempfile::tempdir().unwrap();
        let staged = StagedArtifact::create(root.path()).unwrap();
        let info = DiscoverTransfer {
            url: format!("{}/model", http.url),
            resources: ["diffuse", "specular"]
                .into_iter()
                .map(|id| DiscoverResource {
                    id: id.into(),
                    url: format!("{}/environment", http.url),
                    size: Some(11),
                })
                .chain(std::iter::once(DiscoverResource {
                    id: "model_alias".into(),
                    url: format!("{}/model", http.url),
                    size: Some(5),
                }))
                .collect(),
            ..Default::default()
        };
        let mut progress = Vec::new();
        let (primary, resources) =
            transfer::download(&info, &staged, &mut |value| progress.push(value))
                .await
                .unwrap();
        assert_eq!(std::fs::read(primary).unwrap(), b"model");
        assert_eq!(resources["diffuse"], resources["specular"]);
        assert_eq!(resources["model_alias"], "input-0");
        assert_eq!(std::fs::read_dir(staged.input_root()).unwrap().count(), 2);
        assert!(progress.last().unwrap().is_none());
        assert_eq!(progress.iter().filter(|value| value.is_none()).count(), 1);
        let transfers = progress.iter().flatten().collect::<Vec<_>>();
        assert!(transfers
            .windows(2)
            .all(|pair| pair[0].received_bytes <= pair[1].received_bytes));
        assert_eq!(transfers.last().unwrap().received_bytes, 16);
        assert_eq!(
            transfers.last().unwrap().total_bytes,
            (!chunked).then_some(16)
        );
        assert!(transfers
            .iter()
            .all(|value| value.total_bytes.is_none_or(|total| total == 16)));
        http.assert_requests(2);
        drop(staged);
        assert_empty(root.path());
    }
}

#[tokio::test]
async fn download_slots_are_global_and_cancellation_releases_temporary_inputs() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (accepted, mut requests) = tokio::sync::mpsc::unbounded_channel();
    let responses = Arc::new(tokio::sync::Semaphore::new(0));
    let allowed = responses.clone();
    let server = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let accepted = accepted.clone();
            let allowed = allowed.clone();
            connections.spawn(async move {
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(stream.read_u8().await.unwrap());
                }
                let request = String::from_utf8(request).unwrap();
                let index = request
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .trim_start_matches('/')
                    .parse::<usize>()
                    .unwrap();
                accepted.send(index).unwrap();
                let permit = allowed.acquire().await.unwrap();
                permit.forget();
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 1\r\n\r\nx",
                    )
                    .await;
            });
        }
    });
    let directories = (0..3)
        .map(|_| tempfile::tempdir().unwrap())
        .collect::<Vec<_>>();
    let mut downloads = directories
        .iter()
        .enumerate()
        .map(|(index, root)| {
            let path = root.path().join("input");
            Some(tokio::spawn(async move {
                download_remote_file(&format!("http://{address}/{index}"), &path, 1024, |_| {})
                    .await
            }))
        })
        .collect::<Vec<_>>();
    let first = tokio::time::timeout(Duration::from_secs(5), requests.recv())
        .await
        .unwrap()
        .unwrap();
    let second = tokio::time::timeout(Duration::from_secs(5), requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert_ne!(first, second);
    assert!(
        tokio::time::timeout(Duration::from_millis(25), requests.recv())
            .await
            .is_err()
    );
    let cancelled = downloads[first].take().unwrap();
    cancelled.abort();
    assert!(cancelled.await.unwrap_err().is_cancelled());
    assert_empty(directories[first].path());
    let third = tokio::time::timeout(Duration::from_secs(5), requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert_ne!(third, first);
    assert_ne!(third, second);
    responses.add_permits(3);
    for download in downloads.into_iter().flatten() {
        download.await.unwrap().unwrap();
    }
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn artifact_download_sessions_reject_reload_without_using_the_replacement() {
    let http = Http::new(routes(archive(&[("scene.gltf", b"model")]))).await;
    let (_old_plugin, manager) = source(&http.url, "scene.gltf");
    let (_new_plugin, replacement) = source(&http.url, "replacement.gltf");
    let session = manager.download_session("test").unwrap();
    let info = session.download("model").await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut replacement = Some(replacement);
    let result = prepare_artifact(&session, "test", "model", root.path(), info, |_| {
        if let Some(replacement) = replacement.take() {
            manager.replace_plugins(replacement).unwrap();
        }
    })
    .await;
    assert!(replacement.is_none());
    assert!(format!("{:#}", result.err().unwrap()).contains("changed while downloading"));
    assert_empty(root.path());
    assert!(remote_asset::load(root.path(), "test", "model")
        .unwrap()
        .is_none());
    assert!(session.ensure_active().is_err());
    manager
        .download_session("test")
        .unwrap()
        .download("model")
        .await
        .unwrap();
    http.assert_requests(1);
}

#[tokio::test]
async fn remote_download_database_failure_rolls_back_item_tags_and_publication() {
    use sea_orm::{ConnectionTrait, DbBackend, Statement};

    let http = Http::new(routes(archive(&[("scene.gltf", b"model")]))).await;
    let (_plugin, manager) = source(&http.url, "scene.gltf");
    let root = tempfile::tempdir().unwrap();
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    db.execute(Statement::from_string(DbBackend::Sqlite,
        "CREATE TRIGGER reject_remote_tags BEFORE INSERT ON tag BEGIN SELECT RAISE(FAIL, 'reject remote tags'); END".to_owned()
    )).await.unwrap();
    let info = manager.call_download("test", "model").await.unwrap();
    let prepared = prepare_artifact(
        &manager.download_session("test").unwrap(),
        "test",
        "model",
        root.path(),
        info,
        |_| {},
    )
    .await
    .unwrap();
    let publication = prepared.publish(root.path()).unwrap();
    let installed = &publication.installed;
    assert!(installed.resource().is_file());
    assert_eq!(
        remote_asset::load(root.path(), "test", "model")
            .unwrap()
            .unwrap()
            .artifact
            .metadata
            .provider_data,
        Some(presentation_snapshot())
    );
    let result = upsert_remote_download(
        &db,
        "test",
        "1",
        root.path(),
        &installed.resource(),
        installed.preview().as_deref(),
        &installed.artifact.metadata,
    )
    .await;
    assert!(format!("{:#}", result.unwrap_err()).contains("reject remote tags"));
    assert!(repo::list_items_all(&db).await.unwrap().is_empty());
    assert!(repo::list_tags(&db).await.unwrap().is_empty());
    drop(publication);
    assert_empty(root.path());
    assert!(remote_asset::load(root.path(), "test", "model")
        .unwrap()
        .is_none());
    http.assert_requests(2);
}

#[tokio::test]
#[ignore = "copies an installed model and downloads explicitly selected public environment resources"]
async fn sketchfab_environment_preparation_publishes_and_applies_real_artifact() {
    let configured = |key| PathBuf::from(std::env::var_os(key).expect(key));
    let output = configured("WAYWALLEN_SKETCHFAB_OUTPUT_DIR")
        .canonicalize()
        .unwrap();
    assert_eq!(
        configured("XDG_DATA_HOME"),
        output.join("data"),
        "run in an isolated process with XDG_DATA_HOME under output"
    );
    let id = std::env::var("WAYWALLEN_SKETCHFAB_MODEL_ID").unwrap();
    let original = remote_asset::load(
        &configured("WAYWALLEN_SKETCHFAB_SOURCE_ROOT"),
        "sketchfab",
        &id,
    )
    .unwrap()
    .expect("model must already be installed");
    assert!(!output.starts_with(&original.root));
    let helper = configured("WAYWALLEN_SKETCHFAB_PREPARE_BIN");
    let options = std::fs::read_to_string(configured("WAYWALLEN_SKETCHFAB_OPTIONS_PATH")).unwrap();
    let environments =
        std::fs::read_to_string(configured("WAYWALLEN_SKETCHFAB_ENVIRONMENTS_PATH")).unwrap();
    assert!(options.len() <= 4 * 1024 * 1024 && environments.len() <= 4 * 1024 * 1024);
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let zip_options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for file in &original.artifact.files {
        let path = Path::new("nested").join(&file.path);
        zip.start_file(path.to_str().unwrap(), zip_options).unwrap();
        std::io::copy(
            &mut std::fs::File::open(
                remote_asset::validate_file(&original.root, &file.path).unwrap(),
            )
            .unwrap(),
            &mut zip,
        )
        .unwrap();
    }
    let archive = zip.finish().unwrap().into_inner();
    let archive_size = archive.len();
    let http = Http::new(HashMap::from([(
        "/model.zip".into(),
        Reply {
            status: 200,
            bytes: archive,
            chunked: false,
        },
    )]))
    .await;
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/org.waywallen.sketchfab");
    let plugin_root = output.join("plugins/org.waywallen.sketchfab");
    std::fs::create_dir_all(&plugin_root).unwrap();
    let manifest_text = std::fs::read_to_string(source_root.join("plugin.toml")).unwrap();
    let manifest: crate::plugin::renderer_registry::PluginManifest =
        toml::from_str(&manifest_text).unwrap();
    let helper_relative = &manifest
        .plugin
        .prepare
        .as_ref()
        .expect("source must declare its converter")
        .bin;
    let files = std::fs::read_to_string(source_root.join("files.txt")).unwrap();
    for relative in files.lines().filter(|line| !line.trim().is_empty()) {
        let path = remote_asset::relative_path(Path::new(relative)).unwrap();
        let target = plugin_root.join(&path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(
            if &path == helper_relative {
                helper.clone()
            } else {
                source_root.join(&path)
            },
            &target,
        )
        .unwrap();
    }
    let entry = manifest.plugin.entry.as_ref().unwrap();
    std::fs::copy(plugin_root.join(entry), plugin_root.join("original.lua")).unwrap();
    std::fs::write(
        plugin_root.join("files.txt"),
        format!("{files}\noriginal.lua\n"),
    )
    .unwrap();
    let model = serde_json::json!({
        "uid": id, "name": original.artifact.metadata.title,
        "isDownloadable": true, "isAgeRestricted": false,
        "description": original.artifact.metadata.description,
        "viewerUrl": original.artifact.metadata.web_url,
        "user": {"username":"fixture-author","displayName":"Fixture author","profileUrl":"https://example.invalid/fixture-author"},
        "license": {"slug":"by","label":"CC Attribution","url":"https://creativecommons.org/licenses/by/4.0/"},
        "tags":[],
    }).to_string();
    let wrapper = format!(
        r#"
local api = import('sketchfab.api')
local session = import('sketchfab.session')
session.authenticated = function() return true end
session.headers = function() return {{}} end
local model_calls, options_calls, environment_calls, download_calls = 0,0,0,0
api.model = function(ctx, uid)
    assert(uid == {id:?}); model_calls=model_calls+1; assert(model_calls == 1)
    return ctx.json.strict.decode({model:?})
end
api.viewer_options = function(ctx, uid)
    assert(uid == {id:?}); options_calls=options_calls+1; assert(options_calls == 1)
    return ctx.json.strict.decode({options:?})
end
api.environments = function(ctx, uid)
    assert(uid == {id:?}); environment_calls=environment_calls+1; assert(environment_calls == 1)
    return ctx.json.strict.decode({environments:?})
end
api.download = function(ctx, uid)
    assert(uid == {id:?}); download_calls=download_calls+1; assert(download_calls == 1)
    return {{url={url:?} .. '/model.zip',filename='model.zip',archive='zip',size={archive_size}}}
end
return import('original')
"#,
        url = http.url
    );
    std::fs::write(plugin_root.join(entry), wrapper).unwrap();
    let scan = crate::plugin::renderer_registry::scan_plugin_roots(&[
        crate::plugin::renderer_registry::PluginRoot::user(output.join("plugins")),
    ]);
    assert!(scan.incompatible.is_empty(), "{:?}", scan.incompatible);
    assert_eq!(scan.entries.len(), 1);
    let manager = SourceManager::with_state_store(PluginStateStore::new(
        output.join("plugin-state"),
        output.join("legacy-state"),
    ))
    .unwrap();
    manager.load_entry(&scan.entries[0]).unwrap();
    let session = manager.download_session("sketchfab").unwrap();
    let info = session.download(&id).await.unwrap();
    let uses_environment = info.transfer.prepare.is_some();
    let inputs = info
        .transfer
        .resources
        .iter()
        .map(|resource| serde_json::json!({"id":resource.id,"size":resource.size}))
        .collect::<Vec<_>>();
    std::fs::write(
        output.join("inputs.json"),
        serde_json::to_vec_pretty(&inputs).unwrap(),
    )
    .unwrap();
    let root = remote_content_dir("sketchfab");
    let mut progress = Vec::new();
    let prepared = prepare_artifact(&session, "sketchfab", &id, &root, info, |value| {
        progress.push(value)
    })
    .await
    .unwrap();
    let publication = prepared.publish(&root).unwrap();
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    upsert_remote_download(
        &db,
        "sketchfab",
        "fixture",
        &root,
        &publication.installed.resource(),
        publication.installed.preview().as_deref(),
        &publication.installed.artifact.metadata,
    )
    .await
    .unwrap();
    let installed = publication.commit();
    let entry = repo::load_entries(&db).await.unwrap().remove(0);
    let applied = manager.call_apply("sketchfab", &entry).await.unwrap();
    assert!(installed
        .resource()
        .strip_prefix(&installed.root)
        .unwrap()
        .starts_with("nested"));
    if uses_environment {
        assert!(installed.artifact.files.len() > original.artifact.files.len());
        assert_eq!(
            installed.artifact.metadata.provider_data.as_ref().unwrap()["sketchfab"]["sections"]
                ["environment"]["status"],
            "mapped"
        );
        assert!(installed
            .artifact
            .files
            .iter()
            .any(|file| file.path.starts_with(".waywallen/resources")));
    }
    for file in &installed.artifact.files {
        remote_asset::validate_file(&installed.root, &file.path).unwrap();
    }
    assert_eq!(
        installed.artifact.metadata.size,
        Some(
            installed
                .artifact
                .files
                .iter()
                .map(|file| file.bytes)
                .sum::<u64>() as i64
        )
    );
    std::fs::write(
        output.join("snapshot.json"),
        serde_json::to_vec_pretty(&installed.artifact.metadata).unwrap(),
    )
    .unwrap();
    std::fs::write(
        output.join("entry.json"),
        serde_json::to_vec_pretty(&entry).unwrap(),
    )
    .unwrap();
    std::fs::write(
        output.join("extras.json"),
        serde_json::to_vec_pretty(&applied.extras).unwrap(),
    )
    .unwrap();
    let profile = applied
        .extras
        .get("render-profile")
        .expect("prepared source must export profile");
    std::fs::write(output.join("render-profile.json"), profile).unwrap();
    let progress = progress.into_iter().map(|value| match value {
        Some(value) => serde_json::json!({"received_bytes": value.received_bytes, "total_bytes": value.total_bytes}),
        None => serde_json::Value::Null,
    }).collect::<Vec<_>>();
    std::fs::write(
        output.join("progress.json"),
        serde_json::to_vec_pretty(&progress).unwrap(),
    )
    .unwrap();
    http.assert_requests(1);
    eprintln!("prepared Sketchfab artifact: {}", installed.root.display());
}
