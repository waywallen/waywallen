use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

use crate::catalog::entry::WallpaperEntry;
use crate::error::Error;
use crate::events::{GlobalEvent, RemoteDownloadState};
use crate::model::repo;
use crate::plugin::source::{DiscoverDownload, RemoteCapability};
use crate::settings::{remote_content_dir, sanitize_path_segment};
use crate::DaemonContext;

use super::notify_wallpaper_db_changed;

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
    state: &Arc<DaemonContext>,
    source_id: &str,
    dir: &Path,
) -> Result<crate::model::entities::library::Model> {
    let version = state
        .source_manager
        .plugin_version(source_id)
        .unwrap_or_else(|| "0.0.0".to_string());
    let plugin = repo::upsert_plugin(&state.db, source_id, &version).await?;
    let dir = dir.to_string_lossy().to_string();
    let library = match repo::find_library(&state.db, plugin.id, &dir).await? {
        Some(library) => library,
        None => repo::add_library(&state.db, plugin.id, &dir).await?,
    };
    repo::set_library_metadata_value(
        &state.db,
        library.id,
        repo::LIBRARY_METADATA_MANAGED_KEY,
        Some(repo::LIBRARY_METADATA_MANAGED_REMOTE),
    )
    .await?;
    Ok(library)
}

async fn write_remote_sidecar(path: &Path, info: &DiscoverDownload) -> Result<()> {
    let sidecar = sidecar_path(path);
    let tmp = sidecar.with_extension(format!(
        "{}.tmp-{}",
        sidecar
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("json"),
        uuid::Uuid::new_v4()
    ));
    let data = serde_json::to_vec_pretty(info)?;
    tokio::fs::write(&tmp, data).await?;
    tokio::fs::rename(&tmp, &sidecar).await?;
    Ok(())
}

async fn download_remote_file(url: &str, path: &Path) -> Result<()> {
    let tmp = path.with_extension(format!(
        "{}.part-{}",
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or("download"),
        uuid::Uuid::new_v4()
    ));
    let result: Result<()> = async {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (X11; Linux x86_64) waywallen")
            .build()?;
        let response = client
            .get(url)
            .send()
            .await
            .with_context(|| format!("download request {url}"))?
            .error_for_status()
            .with_context(|| format!("download response {url}"))?;
        let mut stream = response.bytes_stream();
        let mut file = tokio::fs::File::create(&tmp).await?;
        while let Some(chunk) = stream.next().await {
            file.write_all(&chunk.context("download chunk")?).await?;
        }
        file.flush().await?;
        drop(file);
        tokio::fs::rename(&tmp, path).await?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    result
}

async fn upsert_remote_download(
    state: &Arc<DaemonContext>,
    source_id: &str,
    dir: &Path,
    path: &Path,
    info: &DiscoverDownload,
) -> Result<()> {
    let _invalidate =
        crate::application::wallpaper_query::InvalidateOnDrop(state.wallpaper_changes.clone());
    if info.wp_type.trim().is_empty() {
        return Err(anyhow!("download wp_type is empty"));
    }
    let library = ensure_remote_library(state, source_id, dir).await?;
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
    let item = repo::upsert_item(
        &state.db,
        repo::ItemUpsertArgs {
            plugin_id: library.plugin_id,
            library_id: library.id,
            path: relative_path,
            ty: &info.wp_type,
            display_name: title,
            preview_path: None,
            description: (!info.description.trim().is_empty()).then_some(info.description.as_str()),
            external_id: (!info.external_id.trim().is_empty()).then_some(info.external_id.as_str()),
            web_url: (!info.web_url.trim().is_empty()).then_some(info.web_url.as_str()),
            update_available: false,
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
    let tags = repo::upsert_tags(&state.db, &info.tags).await?;
    let tag_ids: Vec<i64> = tags.into_iter().map(|tag| tag.id).collect();
    repo::replace_item_tags(&state.db, item.id, &tag_ids).await?;
    Ok(())
}

pub async fn download_remote(
    state: Arc<DaemonContext>,
    source_id: String,
    id: String,
) -> Result<()> {
    publish_remote_download_progress(&state, &source_id, &id, RemoteDownloadState::Pending, "");

    let info = state.source_manager.call_download(&source_id, &id).await?;
    let dir = remote_content_dir(&source_id);
    tokio::fs::create_dir_all(&dir).await?;
    publish_remote_download_progress(
        &state,
        &source_id,
        &id,
        RemoteDownloadState::Downloading,
        "",
    );

    if info.url.trim().is_empty() {
        return Err(anyhow!("download url is empty"));
    }
    let target = dir.join(safe_remote_filename(&info.filename, &id));
    download_remote_file(&info.url, &target).await?;
    write_remote_sidecar(&target, &info).await?;
    upsert_remote_download(&state, &source_id, &dir, &target, &info).await?;

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
    state
        .source_manager
        .remove_item(&entry.plugin_name, entry, &libraries)
        .await?;
    repo::delete_item(&state.db, entry.item_id).await?;
    Ok(())
}
