use std::sync::Arc;

use crate::error::{Error, Result};
use crate::events::GlobalEvent;
use crate::model::repo;
use crate::wallframe::routing::LibrarySnapshot;
use crate::DaemonContext;

pub(super) async fn library_directory(path: &str) -> Result<String> {
    let path = repo::expand_home_path(path);
    let metadata = tokio::fs::metadata(&path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Error::InvalidArgument(format!("Library folder does not exist: {path}"))
        } else {
            Error::InvalidArgument(format!("Cannot access library folder: {path}: {error}"))
        }
    })?;
    if !metadata.is_dir() {
        return Err(Error::InvalidArgument(format!(
            "Library path is not a folder: {path}"
        )));
    }
    Ok(path)
}

pub async fn add_library(
    app: &Arc<DaemonContext>,
    plugin_name: &str,
    path: &str,
    name: Option<&str>,
    settings: std::collections::HashMap<String, String>,
) -> Result<()> {
    let _invalidate =
        crate::application::wallpaper_query::InvalidateOnDrop(app.wallpaper_changes.clone());
    let plugin = repo::find_plugin_by_name(&app.db, plugin_name)
        .await?
        .ok_or_else(|| Error::SourcePluginNotFound(plugin_name.to_owned()))?;
    let settings = app.source_manager.library_settings(plugin_name, settings)?;
    let path = library_directory(path).await?;
    let lib = repo::add_configured_library(&app.db, plugin.id, &path, name, &settings).await?;
    app.router.upsert_library(LibrarySnapshot {
        settings,
        id: lib.id,
        name: lib.name,
        path: lib.path.clone(),
        plugin_name: plugin_name.to_owned(),
    });
    app.events.publish(GlobalEvent::LibrariesAdded {
        paths: vec![lib.path],
    });
    let rescan_app = app.clone();
    app.tasks.spawn_async_unique(
        crate::tasks::TaskKind::Generic,
        "scan/refresh",
        "scan/refresh-after-library-add",
        async move {
            super::refresh_sources(&rescan_app)
                .await
                .map(|_| ())
                .map_err(anyhow::Error::from)
        },
    );
    Ok(())
}

pub async fn rename_library(app: &Arc<DaemonContext>, id: i64, name: &str) -> Result<()> {
    let lib = repo::rename_library(&app.db, id, name).await?;
    let settings = repo::library_settings(&lib)?;
    let plugin = repo::find_plugin_by_id(&app.db, lib.plugin_id)
        .await?
        .ok_or_else(|| Error::SourcePluginNotFound(lib.plugin_id.to_string()))?;
    app.router.upsert_library(LibrarySnapshot {
        settings,
        id: lib.id,
        name: lib.name,
        path: lib.path,
        plugin_name: plugin.name,
    });
    Ok(())
}

pub async fn edit_library(
    app: &Arc<DaemonContext>,
    id: i64,
    name: &str,
    values: std::collections::HashMap<String, String>,
) -> Result<()> {
    let lib = repo::find_library_by_id(&app.db, id)
        .await?
        .ok_or(Error::LibraryNotFound(id))?;
    let plugin = repo::find_plugin_by_id(&app.db, lib.plugin_id)
        .await?
        .ok_or_else(|| Error::SourcePluginNotFound(lib.plugin_id.to_string()))?;
    let settings = app.source_manager.library_settings(&plugin.name, values)?;
    let settings_changed = app
        .source_manager
        .library_settings(&plugin.name, repo::library_settings(&lib)?)
        .map_or(true, |previous| previous != settings);
    let lib = repo::edit_library(&app.db, id, name, Some(&settings)).await?;
    app.router.upsert_library(LibrarySnapshot {
        id,
        settings,
        name: lib.name,
        path: lib.path,
        plugin_name: plugin.name.clone(),
    });
    if !settings_changed {
        return Ok(());
    }
    let source = plugin.name;
    let app = app.clone();
    app.clone().tasks.spawn_async_unique(
        crate::tasks::TaskKind::Generic,
        "scan/refresh",
        "scan/library-edit",
        async move {
            super::refresh_source(&app, &source)
                .await
                .map(|_| ())
                .map_err(anyhow::Error::from)
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn library_directory_accepts_existing_folder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        assert_eq!(library_directory(path).await.unwrap(), path);
    }

    #[tokio::test]
    async fn library_directory_rejects_missing_folder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing");
        let error = library_directory(path.to_str().unwrap()).await.unwrap_err();
        assert!(matches!(error, Error::InvalidArgument(_)));
        assert!(error.to_string().contains("does not exist"));
    }

    #[tokio::test]
    async fn library_directory_rejects_regular_file() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let error = library_directory(file.path().to_str().unwrap())
            .await
            .unwrap_err();
        assert!(matches!(error, Error::InvalidArgument(_)));
        assert!(error.to_string().contains("not a folder"));
    }
}
