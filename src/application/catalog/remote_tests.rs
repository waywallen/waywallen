use super::*;
use std::collections::HashMap;
use std::io::{Cursor, Write};
use std::sync::Mutex;
use tokio::io::AsyncReadExt;

use crate::plugin::source::ENTRY_VERSION_V6;
use crate::plugin::state_store::PluginStateStore;
use crate::probe::media::AvFormatProbe;

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
                if reply.chunked {
                    response.push_str("Transfer-Encoding: chunked\r\n\r\n");
                    response.push_str(&format!("{:x}\r\n", reply.bytes.len()));
                } else {
                    response.push_str(&format!("Content-Length: {}\r\n\r\n", reply.bytes.len()));
                }
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.write_all(&reply.bytes).await;
                if reply.chunked {
                    let _ = stream.write_all(b"\r\n0\r\n\r\n").await;
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

fn source(url: &str, resource: &str) -> (tempfile::TempDir, SourceManager) {
    let directory = tempfile::tempdir().unwrap();
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
    }}
end
function M.discover.resolve(ctx, params)
    assert(downloads == 1)
    local m = params.metadata
    assert(m and m.title == 'A model' and m.external_id == params.id)
    assert(m.url == nil and m.filename == nil)
    assert(m.attribution.author == 'Artist')
    return {{name=m.title,wp_type=m.wp_type,resource={resource:?},description=m.description,
        tags=m.tags,external_id=m.external_id}}
end
return M
"#
    );
    let entry = directory.path().join("main.lua");
    std::fs::write(&entry, script).unwrap();
    let manager = SourceManager::with_probe_and_state_store(
        Arc::new(AvFormatProbe::new()),
        PluginStateStore::new(
            directory.path().join("state"),
            directory.path().join("legacy"),
        ),
    )
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
    let http = Http::new(routes(archive(&[
        ("scene.gltf", scene),
        ("scene.bin", mesh),
        ("textures/base.png", texture),
    ])))
    .await;
    let (_plugin, manager) = source(&http.url, "scene.gltf");
    let root = tempfile::tempdir().unwrap();
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    let info = manager.call_download("test", "model").await.unwrap();
    let prepared = prepare_artifact(&manager, "test", "model", root.path(), info.clone())
        .await
        .unwrap();
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
    let entries = repo::load_entries(&db).await.unwrap();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.plugin_name, "test");
    assert_eq!(entry.wp_type, "gltf");
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
        reloaded.artifact.metadata.attribution.unwrap().requirements,
        "Credit Artist"
    );
    use std::os::unix::fs::MetadataExt;
    let original_inode = std::fs::metadata(installed.resource()).unwrap().ino();
    let replacement = prepare_artifact(&manager, "test", "model", root.path(), info)
        .await
        .unwrap();
    let reused = replacement.publish(root.path()).unwrap().commit();
    assert_eq!(reused.resource(), installed.resource());
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
    let provider_data = serde_json::json!({
        "camera": {"position": [1, 2, 3], "target": [0, 0, 0]},
    })
    .as_object()
    .unwrap()
    .clone();
    info.metadata.provider_data = Some(provider_data.clone());
    let publication = prepare_artifact(&manager, "test", "model", root.path(), info.clone())
        .await
        .unwrap()
        .publish(root.path())
        .unwrap();
    assert!(publication.installed.resource().is_file());
    drop(publication);
    assert_empty(root.path());

    let installed = prepare_artifact(&manager, "test", "model", root.path(), info.clone())
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
    info.metadata.provider_data = Some(
        serde_json::json!({"camera": {"position": [4, 5, 6]}})
            .as_object()
            .unwrap()
            .clone(),
    );
    let reused = prepare_artifact(&manager, "test", "model", root.path(), info)
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
        assert!(
            prepare_artifact(&manager, "test", "model", root.path(), info)
                .await
                .is_err()
        );
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
    let error = download_remote_file(&url, &target, 1024).await.unwrap_err();
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
        let error = download_remote_file(
            &format!("{}/large", http.url),
            &root.path().join("large"),
            8,
        )
        .await
        .unwrap_err();
        assert!(format!("{error:#}").contains("size limit"));
        assert_empty(root.path());
        http.assert_requests(1);
    }
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
    let prepared = prepare_artifact(&manager, "test", "model", root.path(), info)
        .await
        .unwrap();
    let publication = prepared.publish(root.path()).unwrap();
    let installed = &publication.installed;
    assert!(installed.resource().is_file());
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
