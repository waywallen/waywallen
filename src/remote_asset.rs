use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::plugin::source::DiscoverMetadata;

const INTERNAL: &str = ".waywallen";
const MANIFEST: &str = ".waywallen/asset.json";
const MANIFEST_LIMIT: u64 = 1024 * 1024;
static MUTATION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) async fn lock_mutation() -> tokio::sync::MutexGuard<'static, ()> {
    MUTATION.lock().await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct InstalledArtifact {
    version: u32,
    pub source_id: String,
    pub remote_id: String,
    pub resource: PathBuf,
    pub preview: Option<PathBuf>,
    pub files: Vec<crate::archive::ExtractedFile>,
    pub metadata: DiscoverMetadata,
}

#[derive(Debug, Clone)]
pub(crate) struct InstalledAsset {
    pub root: PathBuf,
    pub artifact: InstalledArtifact,
}

impl InstalledAsset {
    pub fn resource(&self) -> PathBuf {
        self.root.join(&self.artifact.resource)
    }

    pub fn preview(&self) -> Option<PathBuf> {
        self.artifact
            .preview
            .as_ref()
            .map(|path| self.root.join(path))
    }
}

fn item_key(id: &str) -> String {
    if !id.is_empty()
        && id.len() <= 80
        && id != "."
        && id != ".."
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        id.to_owned()
    } else {
        format!("asset-{:x}", Sha256::digest(id.as_bytes()))
    }
}

pub(crate) fn relative_path(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .as_os_str()
            .to_string_lossy()
            .contains(['\\', '\0', ':'])
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        bail!("artifact path must be a normal relative path");
    }
    Ok(path.to_path_buf())
}

pub(crate) fn validate_file(root: &Path, relative: &Path) -> Result<PathBuf> {
    let relative = relative_path(relative)?;
    let path = root.join(relative);
    let info = fs::symlink_metadata(&path).context("stat artifact file")?;
    if !info.file_type().is_file() {
        bail!("artifact entry must be a regular file");
    }
    let root = root
        .canonicalize()
        .context("canonicalize artifact directory")?;
    let path = path.canonicalize().context("canonicalize artifact file")?;
    if !path.starts_with(root) {
        bail!("artifact entry escaped its directory");
    }
    Ok(path)
}

fn read_asset(root: &Path) -> Result<InstalledAsset> {
    let kind = fs::symlink_metadata(root).context("stat installed artifact")?;
    if !kind.file_type().is_dir() {
        bail!("installed artifact is not a directory");
    }
    let path = validate_file(root, Path::new(MANIFEST))?;
    if fs::metadata(&path)?.len() > MANIFEST_LIMIT {
        bail!("installed artifact metadata is too large");
    }
    let artifact: InstalledArtifact =
        serde_json::from_slice(&fs::read(path)?).context("read installed artifact metadata")?;
    if artifact.version != 1 {
        bail!("unsupported installed artifact metadata version");
    }
    relative_path(&artifact.resource)?;
    for file in &artifact.files {
        relative_path(&file.path)?;
    }
    if !artifact
        .files
        .iter()
        .any(|file| file.path == artifact.resource)
    {
        bail!("artifact entry is absent from its file inventory");
    }
    if let Some(preview) = &artifact.preview {
        relative_path(preview)?;
    }
    Ok(InstalledAsset {
        root: root.to_path_buf(),
        artifact,
    })
}

pub(crate) fn load(root: &Path, source_id: &str, id: &str) -> Result<Option<InstalledAsset>> {
    let target = root.join(item_key(id));
    match fs::symlink_metadata(&target) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("stat installed artifact"),
        Ok(_) => {}
    }
    let installed = read_asset(&target)?;
    if installed.artifact.source_id != source_id || installed.artifact.remote_id != id {
        bail!("installed artifact identity does not match");
    }
    for file in &installed.artifact.files {
        let path = validate_file(&target, &file.path)
            .context("installed artifact is incomplete; remove it before downloading again")?;
        if fs::metadata(path)?.len() != file.bytes {
            bail!("installed artifact file size changed; remove it before downloading again");
        }
    }
    if let Some(preview) = &installed.artifact.preview {
        validate_file(&target, preview)
            .context("installed artifact is incomplete; remove it before downloading again")?;
    }
    Ok(Some(installed))
}

pub(crate) struct StagedArtifact {
    temporary: tempfile::TempDir,
    content: PathBuf,
}

impl StagedArtifact {
    pub fn create(root: &Path) -> Result<Self> {
        fs::create_dir_all(root).context("create remote library")?;
        let temporary = tempfile::Builder::new()
            .prefix(".download-")
            .tempdir_in(root)?;
        let content = temporary.path().join("content");
        fs::create_dir(&content)?;
        Ok(Self { temporary, content })
    }

    pub fn content(&self) -> &Path {
        &self.content
    }

    pub fn download_path(&self) -> PathBuf {
        self.temporary.path().join("download.zip")
    }

    pub fn preview_path(&self) -> PathBuf {
        self.temporary.path().join("preview")
    }

    pub fn prepare(
        self,
        source_id: &str,
        id: &str,
        resource: &Path,
        preview: Option<&Path>,
        metadata: DiscoverMetadata,
        files: Vec<crate::archive::ExtractedFile>,
    ) -> Result<PreparedArtifact> {
        relative_path(resource)?;
        if resource.components().next() == Some(Component::Normal(INTERNAL.as_ref())) {
            bail!("artifact entry uses a reserved directory");
        }
        validate_file(&self.content, resource)?;
        if !files.iter().any(|file| file.path == resource) {
            bail!("artifact entry is absent from its file inventory");
        }
        for file in &files {
            let path = validate_file(&self.content, &file.path)?;
            if fs::metadata(path)?.len() != file.bytes {
                bail!("artifact file changed after extraction");
            }
        }
        fs::create_dir(self.content.join(INTERNAL))
            .context("create artifact metadata directory")?;
        let preview = match preview {
            Some(path) => {
                if !fs::symlink_metadata(path)?.file_type().is_file() {
                    bail!("artifact preview is not a regular file");
                }
                let relative = PathBuf::from(INTERNAL).join("preview");
                let mut source = fs::File::open(path)?;
                let mut destination = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(self.content.join(&relative))?;
                std::io::copy(&mut source, &mut destination)?;
                Some(relative)
            }
            None => None,
        };
        let artifact = InstalledArtifact {
            version: 1,
            source_id: source_id.to_owned(),
            remote_id: id.to_owned(),
            resource: resource.to_path_buf(),
            preview,
            files,
            metadata,
        };
        let data = serde_json::to_vec_pretty(&artifact)?;
        if data.len() as u64 > MANIFEST_LIMIT {
            bail!("artifact metadata is too large");
        }
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(self.content.join(MANIFEST))?;
        file.write_all(&data)?;
        file.sync_all()?;
        Ok(PreparedArtifact {
            staged: self,
            artifact,
        })
    }
}

pub(crate) struct PreparedArtifact {
    staged: StagedArtifact,
    artifact: InstalledArtifact,
}

impl PreparedArtifact {
    pub fn publish(self, root: &Path) -> Result<Publication> {
        if let Some(installed) = load(root, &self.artifact.source_id, &self.artifact.remote_id)? {
            return Ok(Publication {
                installed,
                created: false,
                committed: true,
            });
        }
        let target = root.join(item_key(&self.artifact.remote_id));
        // Every published directory contains its descriptor, so a racing rename
        // cannot replace an already published nonempty directory.
        fs::rename(self.staged.content(), &target).context("publish remote artifact")?;
        Ok(Publication {
            installed: InstalledAsset {
                root: target,
                artifact: self.artifact,
            },
            created: true,
            committed: false,
        })
    }
}

pub(crate) struct Publication {
    pub installed: InstalledAsset,
    created: bool,
    committed: bool,
}

impl Publication {
    pub fn commit(mut self) -> InstalledAsset {
        self.committed = true;
        self.installed.clone()
    }
}

impl Drop for Publication {
    fn drop(&mut self) {
        if self.created && !self.committed {
            if let Err(error) = fs::remove_dir_all(&self.installed.root) {
                log::warn!("remote artifact rollback failed: {error}");
            }
        }
    }
}

fn owned(root: &Path, source_id: &str, resource: &Path) -> Result<InstalledAsset> {
    let relative = resource
        .strip_prefix(root)
        .map_err(|_| anyhow!("artifact resource is outside the remote library"))?;
    let relative = relative_path(relative)?;
    let key = relative
        .components()
        .next()
        .ok_or_else(|| anyhow!("missing artifact directory"))?;
    let package = root.join(key.as_os_str());
    let installed = read_asset(&package)?;
    if installed.artifact.source_id != source_id
        || std::ffi::OsStr::new(&item_key(&installed.artifact.remote_id)) != key.as_os_str()
        || installed.resource() != resource
    {
        bail!("resource does not identify this installed artifact");
    }
    Ok(installed)
}

pub(crate) fn load_resource(
    root: &Path,
    source_id: &str,
    resource: &Path,
) -> Result<InstalledAsset> {
    let installed = owned(root, source_id, resource)?;
    validate_file(&installed.root, &installed.artifact.resource)?;
    Ok(installed)
}

pub(crate) fn artifact_root(root: &Path, source_id: &str, resource: &Path) -> Result<PathBuf> {
    Ok(load_resource(root, source_id, resource)?.root)
}

#[derive(Debug)]
pub(crate) struct Removal {
    pub removed: bool,
    _mutation: tokio::sync::MutexGuard<'static, ()>,
}

pub(crate) async fn remove(root: &Path, source_id: &str, resource: &Path) -> Result<Removal> {
    let mutation = lock_mutation().await;
    let root = root.to_path_buf();
    let source_id = source_id.to_owned();
    let resource = resource.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let relative = relative_path(
            resource
                .strip_prefix(&root)
                .map_err(|_| anyhow!("artifact resource is outside the remote library"))?,
        )?;
        let key = relative
            .components()
            .next()
            .ok_or_else(|| anyhow!("missing artifact directory"))?;
        if relative.components().count() < 2 {
            bail!("resource does not identify an artifact entry");
        }
        match fs::symlink_metadata(root.join(key.as_os_str())) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Removal {
                    removed: false,
                    _mutation: mutation,
                });
            }
            Err(error) => return Err(error).context("stat remote artifact"),
            Ok(_) => {}
        }
        let installed = owned(&root, &source_id, &resource)?;
        fs::remove_dir_all(installed.root).context("remove remote artifact")?;
        Ok(Removal {
            removed: true,
            _mutation: mutation,
        })
    })
    .await
    .context("artifact removal task failed")?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn prepared(root: &Path, id: &str, data: &[u8]) -> PreparedArtifact {
        let staged = StagedArtifact::create(root).unwrap();
        fs::write(staged.content().join("scene.gltf"), data).unwrap();
        staged
            .prepare(
                "provider",
                id,
                Path::new("scene.gltf"),
                None,
                DiscoverMetadata {
                    title: "A model".into(),
                    wp_type: "gltf".into(),
                    external_id: id.into(),
                    ..Default::default()
                },
                vec![crate::archive::ExtractedFile {
                    path: "scene.gltf".into(),
                    bytes: data.len() as u64,
                }],
            )
            .unwrap()
    }

    #[test]
    fn remote_asset_publication_rolls_back_uncommitted_and_keeps_existing() {
        let root = tempfile::tempdir().unwrap();
        let publication = prepared(root.path(), "first", b"model")
            .publish(root.path())
            .unwrap();
        assert!(publication.installed.resource().is_file());
        drop(publication);
        assert!(load(root.path(), "provider", "first").unwrap().is_none());
        let original = prepared(root.path(), "first", b"original")
            .publish(root.path())
            .unwrap()
            .commit();
        let reused = prepared(root.path(), "first", b"replacement")
            .publish(root.path())
            .unwrap()
            .commit();
        assert_eq!(original.resource(), reused.resource());
        assert_eq!(fs::read(reused.resource()).unwrap(), b"original");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
        let loaded = load_resource(root.path(), "provider", &reused.resource()).unwrap();
        assert_eq!(loaded.root, reused.root);
        assert!(loaded.artifact.metadata.provider_data.is_none());
        let mut stored = loaded.artifact;
        stored.metadata.provider_data = Some(
            serde_json::json!({"camera": {"position": [1, 2, 3]}})
                .as_object()
                .unwrap()
                .clone(),
        );
        fs::write(
            reused.root.join(MANIFEST),
            serde_json::to_vec(&stored).unwrap(),
        )
        .unwrap();
        let loaded = load_resource(root.path(), "provider", &reused.resource()).unwrap();
        assert_eq!(
            loaded.artifact.metadata.provider_data.unwrap()["camera"]["position"],
            serde_json::json!([1, 2, 3])
        );
    }

    #[tokio::test]
    async fn remote_asset_delete_requires_exact_owned_entry() {
        let root = tempfile::tempdir().unwrap();
        let installed = prepared(root.path(), "item", b"model")
            .publish(root.path())
            .unwrap()
            .commit();
        assert!(load_resource(root.path(), "other", &installed.resource()).is_err());
        assert!(load_resource(root.path(), "provider", root.path()).is_err());
        assert!(load_resource(root.path(), "provider", &installed.root.join(MANIFEST)).is_err());
        assert!(remove(root.path(), "other", &installed.resource())
            .await
            .is_err());
        assert!(remove(root.path(), "provider", root.path()).await.is_err());
        assert!(
            remove(root.path(), "provider", &installed.root.join(MANIFEST))
                .await
                .is_err()
        );
        assert!(
            remove(root.path(), "provider", &installed.resource())
                .await
                .unwrap()
                .removed
        );
        assert!(
            !remove(root.path(), "provider", &installed.resource())
                .await
                .unwrap()
                .removed
        );
        assert!(!installed.root.exists());
    }

    #[tokio::test]
    async fn remote_asset_can_remove_owned_package_with_missing_content() {
        let root = tempfile::tempdir().unwrap();
        let installed = prepared(root.path(), "missing", b"model")
            .publish(root.path())
            .unwrap()
            .commit();
        assert_eq!(
            artifact_root(root.path(), "provider", &installed.resource()).unwrap(),
            installed.root
        );
        fs::remove_file(installed.resource()).unwrap();
        assert!(load(root.path(), "provider", "missing").is_err());
        assert!(artifact_root(root.path(), "provider", &installed.resource()).is_err());
        assert!(load_resource(root.path(), "provider", &installed.resource()).is_err());
        assert!(
            remove(root.path(), "provider", &installed.resource())
                .await
                .unwrap()
                .removed
        );
        assert!(!installed.root.exists());
    }

    #[tokio::test]
    async fn remote_asset_inventory_detects_missing_companion_files() {
        let root = tempfile::tempdir().unwrap();
        let staged = StagedArtifact::create(root.path()).unwrap();
        fs::write(staged.content().join("scene.gltf"), b"model").unwrap();
        fs::write(staged.content().join("scene.bin"), b"mesh").unwrap();
        let files = vec![
            crate::archive::ExtractedFile {
                path: "scene.gltf".into(),
                bytes: 5,
            },
            crate::archive::ExtractedFile {
                path: "scene.bin".into(),
                bytes: 4,
            },
        ];
        let installed = staged
            .prepare(
                "provider",
                "complete",
                Path::new("scene.gltf"),
                None,
                DiscoverMetadata::default(),
                files,
            )
            .unwrap()
            .publish(root.path())
            .unwrap()
            .commit();
        assert!(load(root.path(), "provider", "complete").unwrap().is_some());
        fs::remove_file(installed.root.join("scene.bin")).unwrap();
        assert!(load(root.path(), "provider", "complete").is_err());
        assert!(
            remove(root.path(), "provider", &installed.resource())
                .await
                .unwrap()
                .removed
        );
    }

    #[tokio::test]
    async fn remote_asset_removal_retains_ownership_until_db_completion() {
        let root = tempfile::tempdir().unwrap();
        let installed = prepared(root.path(), "held", b"model")
            .publish(root.path())
            .unwrap()
            .commit();
        let removal = remove(root.path(), "provider", &installed.resource())
            .await
            .unwrap();
        let pending = tokio::spawn(async {
            let _guard = lock_mutation().await;
        });
        tokio::task::yield_now().await;
        assert!(!pending.is_finished());
        drop(removal);
        tokio::time::timeout(std::time::Duration::from_secs(1), pending)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn remote_asset_cancelled_worker_still_cleans_owned_staging() {
        let root = tempfile::tempdir().unwrap();
        let staged = StagedArtifact::create(root.path()).unwrap();
        let (started, ready) = tokio::sync::oneshot::channel();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let worker_barrier = barrier.clone();
        let task = tokio::spawn(async move {
            tokio::task::spawn_blocking(move || {
                let _ = started.send(());
                worker_barrier.wait();
                staged
            })
            .await
        });
        ready.await.unwrap();
        task.abort();
        let _ = task.await;
        barrier.wait();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while fs::read_dir(root.path()).unwrap().count() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[test]
    fn remote_asset_resolve_and_manifest_paths_cannot_escape() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("model.gltf"), b"model").unwrap();
        let staged = StagedArtifact::create(root.path()).unwrap();
        symlink(
            outside.path().join("model.gltf"),
            staged.content().join("scene.gltf"),
        )
        .unwrap();
        assert!(validate_file(staged.content(), Path::new("scene.gltf")).is_err());
        for path in [
            "../outside.gltf",
            "/tmp/model.gltf",
            "a/../scene.gltf",
            "C:\\model.gltf",
            "",
        ] {
            assert!(relative_path(Path::new(path)).is_err());
        }
        assert!(load(root.path(), "provider", "../outside")
            .unwrap()
            .is_none());
        assert!(staged
            .prepare(
                "provider",
                "item",
                Path::new("../outside.gltf"),
                None,
                DiscoverMetadata::default(),
                Vec::new()
            )
            .is_err());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
