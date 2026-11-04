use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Seek, SeekFrom};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

use crate::archive::ExtractedFile;
use crate::plugin::renderer_registry::PluginPrepare;
use crate::plugin::source::DiscoverResource;

pub(crate) const MAX_RESOURCES: usize = 16;
pub(crate) const RESOURCE_LIMIT: u64 = 64 * 1024 * 1024;
pub(crate) const INPUT_LIMIT: u64 = 128 * 1024 * 1024;
const JSON_LIMIT: u64 = 64 * 1024;
const OUTPUT_LIMIT: u64 = 128 * 1024 * 1024;
const OUTPUT_FILES: usize = 64;
const HELPER_TIMEOUT: Duration = Duration::from_secs(60);
static HELPER_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

pub(crate) struct Program {
    version: u32,
    file: Mutex<std::fs::File>,
}

impl Program {
    pub fn open(definition: &PluginPrepare) -> Result<Self> {
        let file = std::fs::File::open(&definition.bin)
            .context("open declared artifact prepare helper")?;
        ensure!(
            file.metadata()?.is_file(),
            "artifact prepare helper is not a regular file"
        );
        Ok(Self {
            version: definition.version,
            file: Mutex::new(file),
        })
    }

    fn copy_to(&self, path: &Path) -> Result<()> {
        let mut source = self
            .file
            .lock()
            .map_err(|_| anyhow!("prepare helper source poisoned"))?;
        source.seek(SeekFrom::Start(0))?;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        std::io::copy(&mut *source, &mut output)?;
        output.set_permissions(std::fs::Permissions::from_mode(0o700))?;
        Ok(())
    }
}

pub(crate) fn validate_url(value: &str) -> Result<reqwest::Url> {
    ensure!(value.len() <= 16 * 1024, "download URL is too long");
    let url = reqwest::Url::parse(value).context("invalid download URL")?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && url.host_str().is_some(),
        "download URL is not an HTTP resource"
    );
    Ok(url)
}

pub(crate) fn validate_resources(resources: &[DiscoverResource]) -> Result<()> {
    ensure!(
        resources.len() <= MAX_RESOURCES,
        "too many auxiliary resources"
    );
    let mut ids = HashSet::new();
    let mut urls = HashMap::<String, Option<u64>>::new();
    for resource in resources {
        ensure!(
            !resource.id.is_empty()
                && resource.id.len() <= 64
                && resource
                    .id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
            "auxiliary resource id must be 1-64 ASCII letters, digits, hyphens or underscores"
        );
        ensure!(ids.insert(&resource.id), "duplicate auxiliary resource id");
        let url = validate_url(&resource.url)?;
        ensure!(
            resource.size.is_none_or(|size| size <= RESOURCE_LIMIT),
            "auxiliary resource exceeds size limit"
        );
        let previous = urls.entry(url.to_string()).or_default();
        if let (Some(previous), Some(size)) = (*previous, resource.size) {
            ensure!(
                previous == size,
                "duplicate resource URL has inconsistent sizes"
            );
        }
        *previous = previous.or(resource.size);
    }
    let declared = urls.values().flatten().try_fold(0_u64, |total, size| {
        total
            .checked_add(*size)
            .context("auxiliary resource size overflow")
    })?;
    ensure!(
        declared <= INPUT_LIMIT,
        "auxiliary resources exceed total size limit"
    );
    Ok(())
}

#[derive(Serialize)]
struct Request<'a> {
    version: u32,
    parameters: &'a serde_json::Map<String, serde_json::Value>,
    resources: &'a BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    version: u32,
    files: Vec<ExtractedFile>,
    data: serde_json::Map<String, serde_json::Value>,
}

struct ChildGuard(Option<tokio::process::Child>, i32);

impl ChildGuard {
    async fn terminate(&mut self) {
        if let Some(mut child) = self.0.take() {
            unsafe {
                libc::kill(-self.1, libc::SIGKILL);
            }
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            unsafe {
                libc::kill(-self.1, libc::SIGKILL);
            }
            let _ = child.start_kill();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = child.wait().await;
                });
            }
        }
    }
}

async fn read_bounded(input: impl tokio::io::AsyncRead + Unpin) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    input.take(JSON_LIMIT + 1).read_to_end(&mut bytes).await?;
    ensure!(
        bytes.len() as u64 <= JSON_LIMIT,
        "prepare helper output exceeds 64 KiB"
    );
    Ok(bytes)
}

fn scan_output(root: &Path) -> Result<BTreeMap<PathBuf, u64>> {
    ensure!(
        std::fs::symlink_metadata(root)?.file_type().is_dir(),
        "prepare helper output root is not a regular directory"
    );
    let mut pending = vec![(PathBuf::new(), 0_usize)];
    let mut result = BTreeMap::new();
    let mut total = 0_u64;
    let mut directories = 0;
    while let Some((relative, depth)) = pending.pop() {
        ensure!(
            depth <= 16,
            "prepare helper output exceeds directory depth limit"
        );
        for entry in std::fs::read_dir(root.join(&relative))? {
            let entry = entry?;
            let path = relative.join(entry.file_name());
            super::relative_path(&path)?;
            let metadata = std::fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_dir() {
                directories += 1;
                ensure!(
                    directories <= 128,
                    "prepare helper output has too many directories"
                );
                pending.push((path, depth + 1));
            } else {
                ensure!(
                    metadata.file_type().is_file(),
                    "prepare helper output must contain only regular files"
                );
                ensure!(
                    result.len() < OUTPUT_FILES,
                    "prepare helper output has too many files"
                );
                total = total
                    .checked_add(metadata.len())
                    .context("prepare helper output size overflow")?;
                ensure!(
                    total <= OUTPUT_LIMIT,
                    "prepare helper output exceeds total size limit"
                );
                result.insert(path, metadata.len());
            }
        }
    }
    Ok(result)
}

fn response(bytes: &[u8], root: &Path) -> Result<Response> {
    let lua = mlua::Lua::new();
    let value =
        mlua_extra::json::strict::decode(&lua, bytes).context("invalid prepare helper JSON")?;
    let bytes = mlua_extra::json::strict::encode(&value)?;
    let response: Response =
        serde_json::from_slice(&bytes).context("invalid prepare helper response")?;
    ensure!(
        response.version == 1,
        "unsupported prepare helper response version"
    );
    ensure!(
        response.files.len() <= OUTPUT_FILES,
        "prepare helper listed too many files"
    );
    ensure!(
        serde_json::to_vec(&response.data)?.len() as u64 <= JSON_LIMIT,
        "prepare helper data exceeds 64 KiB"
    );
    let mut listed = BTreeMap::new();
    for file in &response.files {
        let path = super::relative_path(&file.path)?;
        ensure!(
            listed.insert(path, file.bytes).is_none(),
            "prepare helper listed duplicate files"
        );
    }
    ensure!(
        listed == scan_output(root)?,
        "prepare helper file inventory does not match output"
    );
    Ok(response)
}

pub(crate) async fn run(
    helper: &Program,
    staged: &mut super::StagedArtifact,
    parameters: &serde_json::Map<String, serde_json::Value>,
    resources: &BTreeMap<String, String>,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    run_with_timeout(helper, staged, parameters, resources, HELPER_TIMEOUT).await
}

async fn run_with_timeout(
    helper: &Program,
    staged: &mut super::StagedArtifact,
    parameters: &serde_json::Map<String, serde_json::Value>,
    resources: &BTreeMap<String, String>,
    timeout: Duration,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    let _permit = HELPER_SLOTS
        .acquire()
        .await
        .context("prepare helper limiter closed")?;
    ensure!(helper.version == 1, "unsupported prepare helper version");
    let request = serde_json::to_vec(&Request {
        version: 1,
        parameters,
        resources,
    })?;
    ensure!(
        request.len() as u64 <= JSON_LIMIT,
        "prepare helper request exceeds 64 KiB"
    );
    tokio::fs::write(staged.request_path(), request).await?;
    let executable = staged.request_path().with_file_name("prepare-helper");
    helper.copy_to(&executable)?;
    let child = tokio::process::Command::new(executable)
        .arg("--request")
        .arg(staged.request_path())
        .arg("--input-root")
        .arg(staged.input_root())
        .arg("--output-root")
        .arg(staged.output_root())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .context("start artifact prepare helper")?;
    let pid = child.id().context("prepare helper has no process id")? as i32;
    let mut guard = ChildGuard(Some(child), pid);
    let child = guard.0.as_mut().unwrap();
    let stdout = child
        .stdout
        .take()
        .context("prepare helper stdout unavailable")?;
    let stderr = child
        .stderr
        .take()
        .context("prepare helper stderr unavailable")?;
    let result = tokio::time::timeout(timeout, async {
        tokio::try_join!(read_bounded(stdout), read_bounded(stderr), async {
            child.wait().await.map_err(anyhow::Error::from)
        },)
    })
    .await;
    let (stdout, stderr, status) = match result {
        Ok(Ok(result)) => result,
        error => {
            guard.terminate().await;
            return match error {
                Ok(Err(error)) => Err(error),
                Err(_) => Err(anyhow!("artifact prepare helper timed out")),
                _ => unreachable!(),
            };
        }
    };
    guard.0.take();
    if !status.success() {
        bail!(
            "artifact prepare helper failed ({status}): {}",
            crate::plugin::source::redact_secrets(&String::from_utf8_lossy(&stderr))
        );
    }
    let response = response(&stdout, &staged.output_root())?;
    staged.adopt_prepared_files(response.files)?;
    Ok(response.data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn helper(body: &str) -> (tempfile::TempDir, Program) {
        let directory = tempfile::tempdir().unwrap();
        let bin = directory.path().join("prepare");
        std::fs::write(
            &bin,
            format!(
                r#"#!/bin/sh
set -eu
while [ "$#" -gt 0 ]; do
    case "$1" in
        --request) prep_request="$2";;
        --input-root) prep_input="$2";;
        --output-root) prep_output="$2";;
    esac
    shift 2
done
{body}
"#
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let program = Program::open(&PluginPrepare { bin, version: 1 }).unwrap();
        (directory, program)
    }

    #[test]
    fn auxiliary_resource_descriptors_are_bounded_and_unambiguous() {
        let good = DiscoverResource {
            id: "background".into(),
            url: "https://example.invalid/env.gz".into(),
            size: Some(42),
        };
        validate_resources(&[good.clone()]).unwrap();
        for value in [
            DiscoverResource {
                id: "../escape".into(),
                ..good.clone()
            },
            DiscoverResource {
                id: "".into(),
                ..good.clone()
            },
            DiscoverResource {
                url: "file:///etc/passwd".into(),
                ..good.clone()
            },
            DiscoverResource {
                url: "https://user:pass@example.invalid/env.gz".into(),
                ..good.clone()
            },
            DiscoverResource {
                url: "https://example.invalid/env.gz#fragment".into(),
                ..good.clone()
            },
            DiscoverResource {
                size: Some(RESOURCE_LIMIT + 1),
                ..good.clone()
            },
        ] {
            assert!(validate_resources(&[value]).is_err());
        }
        assert!(validate_resources(&[good.clone(), good.clone()]).is_err());
        assert!(validate_resources(&[
            good.clone(),
            DiscoverResource {
                id: "another".into(),
                size: Some(43),
                ..good.clone()
            }
        ])
        .is_err());
        assert!(validate_resources(&[
            DiscoverResource {
                id: "unknown".into(),
                size: None,
                ..good.clone()
            },
            good.clone(),
            DiscoverResource {
                id: "mismatch".into(),
                size: Some(43),
                ..good.clone()
            },
        ])
        .is_err());
        let resources = (0..17)
            .map(|index| DiscoverResource {
                id: format!("id{index}"),
                ..good.clone()
            })
            .collect::<Vec<_>>();
        assert!(validate_resources(&resources).is_err());
        let resources = (0..3)
            .map(|index| DiscoverResource {
                id: format!("id{index}"),
                url: format!("https://example.invalid/{index}"),
                size: Some(RESOURCE_LIMIT),
            })
            .collect::<Vec<_>>();
        assert!(validate_resources(&resources).is_err());
    }

    #[tokio::test]
    async fn helper_results_are_adopted_before_resolve_and_published_in_inventory() {
        let (_helper, definition) = helper(
            r#"mkdir -p "$prep_output/env"
printf '%s' 'hdr' > "$prep_output/env/light.hdr"
printf '%s' '{"version":1,"files":[{"path":"env/light.hdr","bytes":3}],"data":{"preprocessed":{"diffuse":"env/light.hdr"}}}'"#,
        );
        let root = tempfile::tempdir().unwrap();
        let mut staged = super::super::StagedArtifact::create(root.path()).unwrap();
        std::fs::write(staged.content().join("scene.gltf"), b"scene").unwrap();
        let data = run(
            &definition,
            &mut staged,
            &serde_json::Map::new(),
            &BTreeMap::new(),
        )
        .await
        .unwrap();
        assert_eq!(data["preprocessed"]["diffuse"], "env/light.hdr");
        let relative = super::super::resource_path(Path::new("env/light.hdr")).unwrap();
        assert_eq!(
            std::fs::read(staged.content().join(&relative)).unwrap(),
            b"hdr"
        );
        assert_eq!(staged.prepared_bytes(), 3);
        let installed = staged
            .prepare(
                "source",
                "model",
                Path::new("scene.gltf"),
                None,
                Default::default(),
                vec![ExtractedFile {
                    path: "scene.gltf".into(),
                    bytes: 5,
                }],
            )
            .unwrap()
            .publish(root.path())
            .unwrap()
            .commit();
        assert_eq!(installed.artifact.files.len(), 2);
        assert!(installed
            .artifact
            .files
            .iter()
            .any(|file| file.path == relative && file.bytes == 3));
    }

    #[tokio::test]
    async fn helper_output_must_match_regular_bounded_files_exactly() {
        let cases = [
            "printf '%s' '{'",
            "printf '%s' '{\"version\":2,\"files\":[],\"data\":{}}'",
            "printf '%s' '{\"version\":1,\"files\":[],\"data\":null}'",
            "printf x > \"$prep_output/extra\"; printf '%s' '{\"version\":1,\"files\":[],\"data\":{}}'",
            "printf '%s' '{\"version\":1,\"files\":[{\"path\":\"missing\",\"bytes\":1}],\"data\":{}}'",
            "printf x > \"$prep_output/item\"; printf '%s' '{\"version\":1,\"files\":[{\"path\":\"item\",\"bytes\":2}],\"data\":{}}'",
            "ln -s /etc/passwd \"$prep_output/link\"; printf '%s' '{\"version\":1,\"files\":[],\"data\":{}}'",
            "prep_work=$(dirname \"$prep_output\"); mkdir -p \"$prep_work/content/textures\"; printf x > \"$prep_work/content/textures/item\"; rmdir \"$prep_output\"; ln -s \"$prep_work/content/textures\" \"$prep_output\"; printf '%s' '{\"version\":1,\"files\":[{\"path\":\"item\",\"bytes\":1}],\"data\":{}}'",
            "printf x > \"$prep_output/item\"; printf '%s' '{\"version\":1,\"files\":[{\"path\":\"item\",\"bytes\":1},{\"path\":\"item\",\"bytes\":1}],\"data\":{}}'",
            "printf '%s' '{\"version\":1,\"files\":[{\"path\":\"../escape\",\"bytes\":1}],\"data\":{}}'",
            "truncate -s 134217729 \"$prep_output/huge\"; printf '%s' '{\"version\":1,\"files\":[],\"data\":{}}'",
            "for n in $(seq 1 65); do touch \"$prep_output/$n\"; done; printf '%s' '{\"version\":1,\"files\":[],\"data\":{}}'",
            "head -c 65537 /dev/zero",
            "head -c 65537 /dev/zero >&2; printf '%s' '{\"version\":1,\"files\":[],\"data\":{}}'",
            "exit 3",
            "printf '%s' '{\"version\":1,\"version\":1,\"files\":[],\"data\":{}}'",
        ];
        for body in cases {
            let (_helper, definition) = helper(body);
            let root = tempfile::tempdir().unwrap();
            let mut staged = super::super::StagedArtifact::create(root.path()).unwrap();
            let error = run(
                &definition,
                &mut staged,
                &serde_json::Map::new(),
                &BTreeMap::new(),
            )
            .await;
            assert!(error.is_err(), "accepted {body}");
            drop(staged);
            assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        }
    }

    #[tokio::test]
    async fn helper_timeout_and_namespace_collision_do_not_publish() {
        let (_helper, definition) = helper("exec sleep 10");
        let root = tempfile::tempdir().unwrap();
        let mut staged = super::super::StagedArtifact::create(root.path()).unwrap();
        let error = run_with_timeout(
            &definition,
            &mut staged,
            &serde_json::Map::new(),
            &BTreeMap::new(),
            Duration::from_millis(20),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        drop(staged);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        let (_helper, definition) =
            helper("printf '%s' '{\"version\":1,\"files\":[],\"data\":{}}'");
        let mut staged = super::super::StagedArtifact::create(root.path()).unwrap();
        std::fs::create_dir(staged.content().join(".waywallen")).unwrap();
        assert!(run(
            &definition,
            &mut staged,
            &serde_json::Map::new(),
            &BTreeMap::new()
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn cancelling_prepare_kills_and_reaps_the_helper() {
        let (_helper, definition) =
            helper("printf '%s' \"$$\" > \"$prep_output/pid\"; exec sleep 10");
        let root = tempfile::tempdir().unwrap();
        let mut staged = super::super::StagedArtifact::create(root.path()).unwrap();
        let pid_path = staged.output_root().join("pid");
        let task = tokio::spawn(async move {
            run(
                &definition,
                &mut staged,
                &serde_json::Map::new(),
                &BTreeMap::new(),
            )
            .await
        });
        let pid = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(value) = std::fs::read_to_string(&pid_path) {
                    if let Ok(pid) = value.parse::<i32>() {
                        break pid;
                    }
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        task.abort();
        let _ = task.await;
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if unsafe { libc::kill(pid, 0) } == -1
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn waiting_for_a_prepare_slot_is_cancellable() {
        let permits = HELPER_SLOTS.acquire_many(2).await.unwrap();
        let (_helper, definition) = helper("exit 0");
        let root = tempfile::tempdir().unwrap();
        let mut staged = super::super::StagedArtifact::create(root.path()).unwrap();
        assert!(tokio::time::timeout(
            Duration::from_millis(20),
            run(
                &definition,
                &mut staged,
                &serde_json::Map::new(),
                &BTreeMap::new()
            )
        )
        .await
        .is_err());
        assert!(!staged.request_path().exists());
        drop(permits);
    }

    #[tokio::test]
    async fn helper_program_retains_the_opened_file_after_path_replacement() {
        let (directory, program) =
            helper("printf '%s' '{\"version\":1,\"files\":[],\"data\":{\"generation\":\"old\"}}'");
        let bin = directory.path().join("prepare");
        std::fs::rename(&bin, directory.path().join("old-helper")).unwrap();
        std::fs::write(&bin, "#!/bin/sh\nexit 9\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut staged = super::super::StagedArtifact::create(root.path()).unwrap();
        let data = run(
            &program,
            &mut staged,
            &serde_json::Map::new(),
            &BTreeMap::new(),
        )
        .await
        .unwrap();
        assert_eq!(data["generation"], "old");
        let replacement = Program::open(&PluginPrepare { bin, version: 1 }).unwrap();
        let mut staged = super::super::StagedArtifact::create(root.path()).unwrap();
        assert!(run(
            &replacement,
            &mut staged,
            &serde_json::Map::new(),
            &BTreeMap::new()
        )
        .await
        .is_err());
    }
}
