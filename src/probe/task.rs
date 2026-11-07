use std::collections::HashMap;
use std::sync::Arc;

pub use super::input_check::inputs_changed;
use super::media::{PreparedPrograms, ProbeJob, ProbeService};
use super::stat::{self, Dependency};
use crate::error::{Error, Result};
use crate::model::{entities::item, repo};
use sea_orm::{ActiveModelTrait, EntityTrait, Set, TransactionTrait};

#[derive(Debug, Default)]
pub struct ProbeStats {
    pub stat_done: usize,
    pub stat_changed: usize,
    pub media_probed: usize,
    pub gained_dimensions: usize,
}

pub async fn run_pending(
    db: &sea_orm::DatabaseConnection,
    probe: Arc<ProbeService>,
) -> Result<ProbeStats> {
    run_sources(db, probe, None).await
}

pub async fn run_sources(
    db: &sea_orm::DatabaseConnection,
    probe: Arc<ProbeService>,
    sources: Option<&std::collections::HashSet<String>>,
) -> Result<ProbeStats> {
    let mut stats = ProbeStats::default();
    let programs = probe.prepare_programs().await?;
    let entries: Vec<_> = repo::load_entries(db)
        .await?
        .into_iter()
        .filter(|e| sources.is_none_or(|s| s.contains(&e.plugin_name)))
        .collect();
    let mut states = repo::probe_states(db).await?;
    let paths: Vec<_> = entries
        .iter()
        .map(|e| (e.resource.clone(), e.library_root.clone()))
        .collect();
    let roots = tokio::task::spawn_blocking(move || {
        paths
            .iter()
            .map(|(p, library)| (stat::snapshot(p), std::path::Path::new(library).is_dir()))
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| Error::Internal(e.into()))?;
    let mut jobs = Vec::new();
    let mut expected = HashMap::new();
    let mut cached = Vec::new();
    for (entry, (root, present)) in entries.into_iter().zip(roots) {
        if !present {
            continue;
        }
        let state = states.remove(&entry.item_id);
        let kind = programs.input_kind(&entry.plugin_name, &entry.wp_type, &entry.sub_type);
        let media = kind.is_some();
        let stat_size = root.kind == "file"
            && entry.sub_type != "timeline"
            && state.as_ref().is_none_or(|s| s.source_fields & 1 == 0)
            && (!media || state.as_ref().is_none_or(|s| s.status.is_none()));
        let size_changed = stat_size && entry.size != root.size;
        if entry.file_mtime_ns != root.mtime_ns || size_changed {
            if let Some(row) = item::Entity::find_by_id(entry.item_id).one(db).await? {
                let mut model: item::ActiveModel = row.into();
                model.file_mtime_ns = Set(root.mtime_ns);
                if size_changed {
                    model.size = Set(root.size);
                }
                model.update(db).await?;
                stats.stat_done += 1;
                stats.stat_changed += 1;
            }
        }
        if !media {
            continue;
        }
        let kind = kind.unwrap();
        let Some(state) = state else {
            continue;
        };
        let mut dependencies: Vec<Dependency> =
            serde_json::from_str(&state.dependencies).unwrap_or_default();
        if dependencies.is_empty() {
            dependencies.push(root);
        }
        let current = tokio::task::spawn_blocking(move || stat::current(&dependencies))
            .await
            .map_err(|e| Error::Internal(e.into()))?;
        let signature = stat::signature(&kind, &current);
        if state.signature.as_deref() == Some(signature.as_str()) && state.input_kind == kind {
            cached.push((entry.item_id, entry.plugin_name, state));
            continue;
        }
        expected.insert(entry.item_id, entry.clone());
        jobs.push(ProbeJob {
            id: entry.item_id,
            source: entry.plugin_name,
            wp_type: entry.wp_type,
            sub_type: entry.sub_type,
            path: entry.resource,
            kind: kind.into(),
        });
    }
    for (id, source, state) in cached {
        if probe.program_is_current(&programs, &source) {
            apply_state(db, id, &state).await?;
        }
    }
    let (sender, mut receiver) = tokio::sync::mpsc::channel(32);
    let execution = probe.execute(&programs, jobs, sender);
    tokio::pin!(execution);
    let mut completion = None;
    let mut receiving = true;
    let mut pending = Vec::with_capacity(32);
    while receiving || completion.is_none() {
        tokio::select! {
            result = receiver.recv(), if receiving => {
                if let Some(result) = result {
                    pending.push(result);
                    if pending.len() == 32 {
                        commit_results(db, &probe, &programs, &expected, &pending, &mut stats).await?;
                        pending.clear();
                    }
                } else { receiving = false; }
            }
            result = &mut execution, if completion.is_none() => { completion = Some(result); }
        }
    }
    commit_results(db, &probe, &programs, &expected, &pending, &mut stats).await?;
    let batch = completion
        .expect("probe completed")
        .map_err(Error::Internal)?
        .unwrap_or_default();
    for error in &batch.errors {
        log::error!("{error}");
    }
    if !batch.errors.is_empty() {
        return Err(Error::Internal(anyhow::anyhow!(batch.errors.join("; "))));
    }
    log::info!(
        "media probe finished: changed={} attempted={} dimensions={}",
        stats.stat_changed,
        stats.media_probed,
        stats.gained_dimensions
    );
    Ok(stats)
}

async fn commit_results(
    db: &sea_orm::DatabaseConnection,
    probe: &ProbeService,
    programs: &PreparedPrograms,
    expected: &HashMap<i64, crate::catalog::entry::WallpaperEntry>,
    results: &[(ProbeJob, super::media::ProbeResult)],
    stats: &mut ProbeStats,
) -> Result<()> {
    let tx = db.begin().await?;
    for (job, result) in results {
        if result.status == "input_changed" {
            continue;
        }
        if !probe.program_is_current(programs, &job.source)
            || programs
                .input_kind(&job.source, &job.wp_type, &job.sub_type)
                .as_deref()
                != Some(job.kind.as_str())
        {
            continue;
        }
        let Some(before) = expected.get(&job.id) else {
            continue;
        };
        let Some(now) = repo::get_entry(&tx, job.id).await? else {
            continue;
        };
        if now.resource != before.resource
            || now.wp_type != before.wp_type
            || now.sub_type != before.sub_type
        {
            continue;
        }
        let dependencies = result.dependencies.clone();
        let check = tokio::task::spawn_blocking(move || stat::current(&dependencies))
            .await
            .map_err(|e| Error::Internal(e.into()))?;
        if check != result.dependencies {
            continue;
        }
        let Some(mut state) = repo::probe_state(&tx, job.id).await? else {
            continue;
        };
        state.input_kind = job.kind.clone();
        state.signature = Some(stat::signature(&job.kind, &result.dependencies));
        state.status = Some(result.status.clone());
        state.attempted_at = Some(crate::tasks::now_ms());
        state.selected_path = result.selected_path.clone();
        state.width = result.width;
        state.height = result.height;
        state.size = result.size;
        state.error = result.error.clone();
        state.dependencies =
            serde_json::to_string(&result.dependencies).map_err(|e| Error::Internal(e.into()))?;
        repo::save_probe(&tx, &state).await?;
        apply_state(&tx, job.id, &state).await?;
        stats.media_probed += 1;
        if result.status == "ok" && result.width.is_some() && result.height.is_some() {
            stats.gained_dimensions += 1;
        } else if result.status != "ok" && result.status != "unsupported" {
            log::error!(
                "media probe failed for {}: {}",
                job.path,
                result.error.as_deref().unwrap_or(&result.status)
            );
        }
    }
    tx.commit().await?;
    Ok(())
}

async fn apply_state(
    db: &impl sea_orm::ConnectionTrait,
    id: i64,
    state: &repo::ProbeState,
) -> Result<()> {
    if state.status.is_none() {
        return Ok(());
    }
    let Some(row) = item::Entity::find_by_id(id).one(db).await? else {
        return Ok(());
    };
    let ok = state.status.as_deref() == Some("ok");
    let width = if state.source_fields & 2 != 0 {
        row.width
    } else {
        state.width.filter(|_| ok)
    };
    let height = if state.source_fields & 4 != 0 {
        row.height
    } else {
        state.height.filter(|_| ok)
    };
    let size = if state.source_fields & 1 != 0 {
        row.size
    } else {
        state.size.filter(|_| ok)
    };
    if (width, height, size) == (row.width, row.height, row.size) {
        return Ok(());
    }
    let mut model: item::ActiveModel = row.into();
    model.width = Set(width);
    model.height = Set(height);
    model.size = Set(size);
    model.update(db).await?;
    Ok(())
}
