use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use sea_orm::{DatabaseConnection, TransactionTrait};
use tokio::io::AsyncWriteExt;

use crate::catalog::entry::WallpaperEntry;
use crate::error::Error;
use crate::events::{GlobalEvent, RemoteDownloadState, RemoteDownloadTransfer};
use crate::model::repo;
use crate::plugin::source::{
    DiscoverDownload, DiscoverMetadata, DownloadFormat, RemoteCapability, SourceManager,
};
use crate::remote_asset::{self, PreparedArtifact, StagedArtifact};
use crate::settings::{remote_content_dir, sanitize_path_segment};
use crate::DaemonContext;

use super::notify_wallpaper_db_changed;

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;

const DOWNLOAD_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
const PREVIEW_LIMIT: u64 = 8 * 1024 * 1024;
const DOWNLOAD_PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
const ZIP_LIMITS: crate::archive::Limits = crate::archive::Limits {
    max_entries: 10_000,
    max_file_bytes: 2 * 1024 * 1024 * 1024,
    max_total_bytes: 4 * 1024 * 1024 * 1024,
    max_depth: 64,
};

#[derive(Default)]
struct DownloadProgressThrottle {
    last_reported: Option<Instant>,
}

impl DownloadProgressThrottle {
    fn should_report(&mut self, now: Instant, force: bool) -> bool {
        if force
            || self.last_reported.is_none_or(|previous| {
                now.saturating_duration_since(previous) >= DOWNLOAD_PROGRESS_INTERVAL
            })
        {
            self.last_reported = Some(now);
            true
        } else {
            false
        }
    }
}

fn safe_remote_filename(filename: &str, id: &str) -> String {
    Path::new(filename)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .map(sanitize_path_segment)
        .unwrap_or_else(|| format!("{}.bin", sanitize_path_segment(id)))
}

fn sidecar_path(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(".json");
    PathBuf::from(value)
}

pub fn publish_remote_download_progress(
    state: &Arc<DaemonContext>,
    source_id: &str,
    id: &str,
    download_state: RemoteDownloadState,
    error: impl Into<String>,
) {
    state.events.publish(GlobalEvent::RemoteDownloadProgress {
        source_id: source_id.to_string(),
        id: id.to_string(),
        state: download_state,
        error: error.into(),
        transfer: None,
    });
}

async fn default_remote_source_id(state: &Arc<DaemonContext>) -> Result<String> {
    state
        .source_manager
        .discover_sources()?
        .into_iter()
        .next()
        .map(|source| source.plugin_id)
        .ok_or_else(|| anyhow!("no discover source plugin"))
}

pub async fn resolve_remote_source_id(
    state: &Arc<DaemonContext>,
    source_id: &str,
) -> Result<String> {
    if !source_id.trim().is_empty() {
        return Ok(source_id.to_string());
    }
    default_remote_source_id(state).await
}

pub fn remote_capability(
    state: &Arc<DaemonContext>,
    source_id: &str,
) -> Result<Option<RemoteCapability>> {
    state
        .source_manager
        .discover_sources()?
        .into_iter()
        .find(|source| source.plugin_id == source_id)
        .map(|source| source.remote_capability)
        .ok_or_else(|| anyhow!("remote source '{source_id}' not found"))
}

async fn ensure_remote_library(
    db: &DatabaseConnection,
    source_id: &str,
    version: &str,
    dir: &Path,
) -> Result<crate::model::entities::library::Model> {
    let plugin = repo::upsert_plugin(db, source_id, version).await?;
    let dir = dir.to_string_lossy().to_string();
    let library = match repo::find_library(db, plugin.id, &dir).await? {
        Some(library) => library,
        None => match repo::add_library(db, plugin.id, &dir).await {
            Ok(library) => library,
            Err(error) => repo::find_library(db, plugin.id, &dir)
                .await?
                .ok_or(error)?,
        },
    };
    repo::set_library_metadata_value(
        db,
        library.id,
        repo::LIBRARY_METADATA_MANAGED_KEY,
        Some(repo::LIBRARY_METADATA_MANAGED_REMOTE),
    )
    .await?;
    Ok(library)
}

async fn write_remote_sidecar(path: &Path, info: &DiscoverMetadata) -> Result<()> {
    let sidecar = sidecar_path(path);
    let tmp = tempfile::Builder::new()
        .prefix(".metadata-")
        .tempfile_in(
            sidecar
                .parent()
                .ok_or_else(|| anyhow!("sidecar path has no parent"))?,
        )?
        .into_temp_path();
    let data = serde_json::to_vec_pretty(info)?;
    tokio::fs::write(&tmp, data).await?;
    tokio::fs::rename(&tmp, &sidecar).await?;
    Ok(())
}

async fn download_remote_file(
    url: &str,
    path: &Path,
    limit: u64,
    mut progress: impl FnMut(RemoteDownloadTransfer),
) -> Result<()> {
    let url = reqwest::Url::parse(url).context("invalid download URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
    {
        return Err(anyhow!("download URL is not an HTTP resource"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("download path has no parent"))?;
    let temporary = tempfile::Builder::new()
        .prefix(".transfer-")
        .tempfile_in(parent)?
        .into_temp_path();
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) waywallen")
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(30 * 60))
        .build()?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(reqwest::Error::without_url)
        .context("download request failed")?
        .error_for_status()
        .map_err(reqwest::Error::without_url)
        .context("download response failed")?;
    let total_bytes = response.content_length();
    if total_bytes.is_some_and(|length| length > limit) {
        return Err(anyhow!("download exceeds the size limit"));
    }
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&temporary)
        .await?;
    let mut received = 0_u64;
    let mut throttle = DownloadProgressThrottle::default();
    if throttle.should_report(Instant::now(), true) {
        progress(RemoteDownloadTransfer {
            received_bytes: 0,
            total_bytes,
        });
    }
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(reqwest::Error::without_url)
            .context("download chunk failed")?;
        received = received
            .checked_add(chunk.len() as u64)
            .filter(|value| *value <= limit)
            .ok_or_else(|| anyhow!("download exceeds the size limit"))?;
        file.write_all(&chunk).await?;
        if throttle.should_report(Instant::now(), false) {
            progress(RemoteDownloadTransfer {
                received_bytes: received,
                total_bytes,
            });
        }
    }
    file.flush().await?;
    drop(file);
    tokio::fs::rename(&temporary, path).await?;
    if throttle.should_report(Instant::now(), true) {
        progress(RemoteDownloadTransfer {
            received_bytes: received,
            total_bytes,
        });
    }
    Ok(())
}

async fn upsert_remote_download(
    db: &DatabaseConnection,
    source_id: &str,
    version: &str,
    dir: &Path,
    path: &Path,
    preview: Option<&Path>,
    info: &DiscoverMetadata,
) -> Result<()> {
    if info.wp_type.trim().is_empty() {
        return Err(anyhow!("download wp_type is empty"));
    }
    let library = ensure_remote_library(db, source_id, version, dir).await?;
    let relative_path = path
        .strip_prefix(dir)
        .ok()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("download target is not under remote library"))?;
    let title = if info.title.trim().is_empty() {
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("Remote wallpaper")
    } else {
        info.title.as_str()
    };
    let preview = preview
        .map(|path| {
            path.strip_prefix(dir)
                .ok()
                .and_then(|value| value.to_str())
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("preview target is not under remote library"))
        })
        .transpose()?;
    let transaction = db.begin().await?;
    let item = repo::upsert_item(
        &transaction,
        repo::ItemUpsertArgs {
            plugin_id: library.plugin_id,
            library_id: library.id,
            path: relative_path,
            ty: &info.wp_type,
            display_name: title,
            preview_path: preview.as_deref(),
            description: (!info.description.trim().is_empty()).then_some(info.description.as_str()),
            external_id: (!info.external_id.trim().is_empty()).then_some(info.external_id.as_str()),
            web_url: (!info.web_url.trim().is_empty()).then_some(info.web_url.as_str()),
            update_available: false,
            can_remove: None,
            size: info.size,
            width: info.width.and_then(|value| i32::try_from(value).ok()),
            height: info.height.and_then(|value| i32::try_from(value).ok()),
            content_rating: info
                .content_rating
                .as_deref()
                .filter(|value| !value.trim().is_empty()),
        },
    )
    .await?;
    let tags = repo::upsert_tags(&transaction, &info.tags).await?;
    let tag_ids: Vec<i64> = tags.into_iter().map(|tag| tag.id).collect();
    repo::replace_item_tags(&transaction, item.id, &tag_ids).await?;
    transaction.commit().await?;
    Ok(())
}

async fn prepare_artifact(
    source_manager: &SourceManager,
    source_id: &str,
    id: &str,
    dir: &Path,
    info: DiscoverDownload,
    mut progress: impl FnMut(Option<RemoteDownloadTransfer>),
) -> Result<PreparedArtifact> {
    let staged = StagedArtifact::create(dir)?;
    let (staged, bytes, entries) = match info.transfer.format {
        DownloadFormat::File => {
            let filename = safe_remote_filename(&info.transfer.filename, id);
            let path = staged.content().join(&filename);
            download_remote_file(&info.transfer.url, &path, DOWNLOAD_LIMIT, |transfer| {
                progress(Some(transfer));
            })
            .await?;
            progress(None);
            let bytes = tokio::fs::metadata(&path).await?.len();
            let entries = vec![crate::archive::ExtractedFile {
                path: PathBuf::from(filename),
                bytes,
            }];
            (staged, bytes, entries)
        }
        DownloadFormat::Zip => {
            download_remote_file(
                &info.transfer.url,
                &staged.download_path(),
                DOWNLOAD_LIMIT,
                |transfer| progress(Some(transfer)),
            )
            .await?;
            progress(None);
            tokio::task::spawn_blocking(move || -> Result<_> {
                let extracted = crate::archive::extract_zip(
                    &staged.download_path(),
                    staged.content(),
                    ZIP_LIMITS,
                )?;
                log::debug!(
                    "remote archive extracted: {} files, {} bytes",
                    extracted.files,
                    extracted.bytes
                );
                Ok((staged, extracted.bytes, extracted.entries))
            })
            .await
            .context("archive extraction task failed")??
        }
    };
    let content = staged
        .content()
        .to_str()
        .ok_or_else(|| anyhow!("artifact directory is not UTF-8"))?;
    let resolved = source_manager
        .call_resolve(source_id, id, content, Some(&info.metadata))
        .await?;
    remote_asset::validate_file(staged.content(), Path::new(&resolved.resource))?;
    if resolved.external_id != id || resolved.wp_type.trim().is_empty() {
        return Err(anyhow!("resolved artifact identity or type is invalid"));
    }
    let mut metadata = info.metadata;
    metadata.title = resolved.name;
    metadata.wp_type = resolved.wp_type;
    metadata.description = resolved.description;
    metadata.tags = resolved.tags;
    metadata.external_id = resolved.external_id;
    metadata.size = Some(i64::try_from(bytes).context("artifact size is too large")?);
    metadata.content_rating = resolved.content_rating.or(metadata.content_rating);
    let preview = if let Some(preview) = resolved.preview {
        Some(remote_asset::validate_file(
            staged.content(),
            Path::new(&preview),
        )?)
    } else if !metadata.preview_url.is_empty() {
        let path = staged.preview_path();
        match download_remote_file(&metadata.preview_url, &path, PREVIEW_LIMIT, |_| {}).await {
            Ok(()) => Some(path),
            Err(error) => {
                log::warn!("remote preview was not downloaded: {error:#}");
                None
            }
        }
    } else {
        None
    };
    let resource = PathBuf::from(resolved.resource);
    let source_id = source_id.to_owned();
    let id = id.to_owned();
    tokio::task::spawn_blocking(move || {
        staged.prepare(
            &source_id,
            &id,
            &resource,
            preview.as_deref(),
            metadata,
            entries,
        )
    })
    .await
    .context("artifact metadata task failed")?
}

pub async fn download_remote(
    state: Arc<DaemonContext>,
    source_id: String,
    id: String,
) -> Result<()> {
    publish_remote_download_progress(&state, &source_id, &id, RemoteDownloadState::Pending, "");
    let dir = remote_content_dir(&source_id);
    let version = state
        .source_manager
        .plugin_version(&source_id)
        .unwrap_or_else(|| "0.0.0".to_owned());
    let info = state.source_manager.call_download(&source_id, &id).await?;
    tokio::fs::create_dir_all(&dir).await?;
    publish_remote_download_progress(
        &state,
        &source_id,
        &id,
        RemoteDownloadState::Downloading,
        "",
    );

    if info.transfer.url.trim().is_empty() {
        return Err(anyhow!("download url is empty"));
    }
    let mut transfer_progress = |transfer| {
        state.events.publish(GlobalEvent::RemoteDownloadProgress {
            source_id: source_id.clone(),
            id: id.clone(),
            state: RemoteDownloadState::Downloading,
            error: String::new(),
            transfer,
        });
    };
    match info.transfer.format {
        DownloadFormat::File if !info.transfer.resolve => {
            let target = dir.join(safe_remote_filename(&info.transfer.filename, &id));
            download_remote_file(&info.transfer.url, &target, DOWNLOAD_LIMIT, |transfer| {
                transfer_progress(Some(transfer));
            })
            .await?;
            transfer_progress(None);
            write_remote_sidecar(&target, &info.metadata).await?;
            upsert_remote_download(
                &state.db,
                &source_id,
                &version,
                &dir,
                &target,
                None,
                &info.metadata,
            )
            .await?;
        }
        DownloadFormat::File | DownloadFormat::Zip => {
            let mutation = remote_asset::lock_mutation().await;
            if let Some(installed) = remote_asset::load(&dir, &source_id, &id)? {
                upsert_remote_download(
                    &state.db,
                    &source_id,
                    &version,
                    &dir,
                    &installed.resource(),
                    installed.preview().as_deref(),
                    &installed.artifact.metadata,
                )
                .await?;
                drop(mutation);
                notify_wallpaper_db_changed(&state, 1).await;
                publish_remote_download_progress(
                    &state,
                    &source_id,
                    &id,
                    RemoteDownloadState::Done,
                    "",
                );
                return Ok(());
            }
            drop(mutation);
            let prepared = prepare_artifact(
                &state.source_manager,
                &source_id,
                &id,
                &dir,
                info,
                &mut transfer_progress,
            )
            .await?;
            // Once publication starts, its owner completes the DB transaction
            // even when the requesting task is cancelled.
            return tokio::spawn(async move {
                let mutation = remote_asset::lock_mutation().await;
                let publication = prepared.publish(&dir)?;
                let installed = &publication.installed;
                upsert_remote_download(
                    &state.db,
                    &source_id,
                    &version,
                    &dir,
                    &installed.resource(),
                    installed.preview().as_deref(),
                    &installed.artifact.metadata,
                )
                .await?;
                publication.commit();
                drop(mutation);
                notify_wallpaper_db_changed(&state, 1).await;
                publish_remote_download_progress(
                    &state,
                    &source_id,
                    &id,
                    RemoteDownloadState::Done,
                    "",
                );
                Ok(())
            })
            .await
            .context("artifact publication task failed")?;
        }
    }

    notify_wallpaper_db_changed(&state, 1).await;
    publish_remote_download_progress(&state, &source_id, &id, RemoteDownloadState::Done, "");
    Ok(())
}

async fn source_libraries_for_plugin(
    state: &Arc<DaemonContext>,
    plugin_name: &str,
) -> Result<Vec<String>> {
    let plugin = repo::find_plugin_by_name(&state.db, plugin_name)
        .await?
        .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_string()))?;
    Ok(repo::list_libraries_by_plugin(&state.db, plugin.id)
        .await?
        .into_iter()
        .map(|library| library.path)
        .collect())
}

pub async fn remove_wallpaper_entry_files_and_db(
    state: &Arc<DaemonContext>,
    entry: &WallpaperEntry,
) -> Result<()> {
    let _invalidate =
        crate::application::wallpaper_query::InvalidateOnDrop(state.wallpaper_changes.clone());
    let libraries = source_libraries_for_plugin(state, &entry.plugin_name).await?;
    let _removal = state
        .source_manager
        .remove_item(&entry.plugin_name, entry, &libraries)
        .await?;
    repo::delete_item(&state.db, entry.item_id).await?;
    Ok(())
}
