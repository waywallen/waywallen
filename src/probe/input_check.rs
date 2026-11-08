use super::stat::{self, Dependency};
use crate::error::{Error, Result};
use crate::model::repo;

pub async fn inputs_changed(
    db: &sea_orm::DatabaseConnection,
    probe: &super::media::ProbeService,
) -> Result<bool> {
    let programs = probe.prepare_programs().await?;
    let mut states = repo::probe_states(db).await?;
    for entry in repo::load_entries(db).await? {
        let root = entry.resource.clone();
        let library = entry.library_root.clone();
        let (root, present) = tokio::task::spawn_blocking(move || {
            (
                stat::snapshot(&root),
                std::path::Path::new(&library).is_dir(),
            )
        })
        .await
        .map_err(|e| Error::Internal(e.into()))?;
        if !present {
            continue;
        }
        if entry.file_mtime_ns != root.mtime_ns {
            return Ok(true);
        }
        let Some(kind) = programs.input_kind(&entry.plugin_name, &entry.wp_type, &entry.sub_type)
        else {
            continue;
        };
        let Some(state) = states.remove(&entry.item_id) else {
            return Ok(true);
        };
        let dependencies: Vec<Dependency> =
            serde_json::from_str(&state.dependencies).unwrap_or_default();
        if dependencies.is_empty() || state.signature.is_none() || state.input_kind != kind {
            return Ok(true);
        }
        let current = tokio::task::spawn_blocking(move || stat::current(&dependencies))
            .await
            .map_err(|e| Error::Internal(e.into()))?;
        if state.signature.as_deref() != Some(stat::signature(&kind, &current).as_str()) {
            return Ok(true);
        }
    }
    Ok(false)
}
