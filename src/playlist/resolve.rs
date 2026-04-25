//! Bridge between persisted playlists (`playlist` / `playlist_item`
//! tables) and the in-memory wallpaper snapshot owned by the source
//! manager.
//!
//! Two layers:
//!   - [`WallpaperIndex`] — pure, builds a `(library_root, relative
//!     path) → wallpaper id` lookup table from a `&[WallpaperEntry]`.
//!     Re-built on every `refresh_sources` so it tracks library churn
//!     without a DB round-trip per lookup.
//!   - [`activate`] / [`resolve_active`] — async, query the DB for
//!     either the curated member list or the smart filter blob, walk
//!     the index to produce wallpaper ids in playlist-defined order,
//!     and stash the result into the live [`PlaylistState`].
//!
//! Resolution drops members that no longer exist on disk (item row
//! cascade-deleted, library removed, file moved out from under us).
//! That keeps the state cursor from pointing at a phantom id; the
//! caller does not need to reconcile.

use std::collections::HashMap;

use anyhow::{anyhow, Context, Result};
use sea_orm::{DatabaseConnection, EntityTrait};

use super::filter::Filter;
use super::state::{Mode, PlaylistState};
use crate::model::entities::{item, library, playlist};
use crate::model::repo;
use crate::model::sync::relative_under_root;
use crate::wallpaper_type::WallpaperEntry;

/// `(library_root, relative_path) → wallpaper id` lookup. Both keys
/// are normalized to strip a single trailing slash so paths emitted
/// with and without a separator collide on the same row.
pub struct WallpaperIndex {
    by_path: HashMap<(String, String), String>,
}

impl WallpaperIndex {
    pub fn build(snapshot: &[WallpaperEntry]) -> Self {
        let mut by_path = HashMap::with_capacity(snapshot.len());
        for e in snapshot {
            if e.library_root.is_empty() {
                continue;
            }
            let rel = match relative_under_root(&e.library_root, &e.resource) {
                Some(r) => r,
                None => continue,
            };
            by_path.insert((normalize(&e.library_root), rel), e.id.clone());
        }
        Self { by_path }
    }

    pub fn lookup(&self, library_root: &str, relative_path: &str) -> Option<&str> {
        self.by_path
            .get(&(normalize(library_root), relative_path.to_string()))
            .map(String::as_str)
    }
}

fn normalize(p: &str) -> String {
    p.strip_suffix('/').unwrap_or(p).to_string()
}

/// Snapshot the `(item.id → wallpaper id)` bridge in one shot. Issues
/// a single `SELECT items JOIN library` and indexes the result. Used
/// by curated-playlist resolution; smart playlists never need this.
async fn build_item_id_to_entry_id(
    db: &DatabaseConnection,
    idx: &WallpaperIndex,
) -> Result<HashMap<i64, String>> {
    let pairs = item::Entity::find()
        .find_also_related(library::Entity)
        .all(db)
        .await
        .context("select items join library for playlist resolve")?;

    let mut out = HashMap::with_capacity(pairs.len());
    for (it, lib) in pairs {
        let lib = match lib {
            Some(l) => l,
            None => continue,
        };
        if let Some(eid) = idx.lookup(&lib.path, &it.path) {
            out.insert(it.id, eid.to_string());
        }
    }
    Ok(out)
}

/// Re-resolve `state.ids` against the latest snapshot using whatever
/// source the active playlist defines. Cheap for All (clones snapshot
/// ids), one DB query for curated, zero DB queries for smart (the
/// filter is already cached on the state).
pub async fn resolve_active(
    db: &DatabaseConnection,
    snapshot: &[WallpaperEntry],
    state: &PlaylistState,
) -> Result<Vec<String>> {
    // 1) "All" pseudo-playlist.
    if state.active_id.is_none() {
        return Ok(snapshot.iter().map(|e| e.id.clone()).collect());
    }

    // 2) Smart: cached filter on the state evaluates against snapshot.
    if let Some(filter) = state.filter.as_ref() {
        return Ok(filter.apply(snapshot));
    }

    // 3) Curated: pull ordered item ids and bridge through the index.
    let id = state.active_id.unwrap();
    let item_ids = repo::list_playlist_item_ids(db, id).await?;
    if item_ids.is_empty() {
        return Ok(Vec::new());
    }
    let idx = WallpaperIndex::build(snapshot);
    let bridge = build_item_id_to_entry_id(db, &idx).await?;
    Ok(item_ids
        .into_iter()
        .filter_map(|iid| bridge.get(&iid).cloned())
        .collect())
}

/// Switch the live state to a different playlist. Loads the row,
/// installs `mode` / `filter` / `shuffle_seed`, resolves member ids
/// against the snapshot, and pins the cursor on whatever was current
/// if it still appears in the new id list.
pub async fn activate(
    db: &DatabaseConnection,
    snapshot: &[WallpaperEntry],
    state: &mut PlaylistState,
    id: i64,
) -> Result<()> {
    let row = playlist::Entity::find_by_id(id)
        .one(db)
        .await
        .with_context(|| format!("select playlist id={id}"))?
        .ok_or_else(|| anyhow!("playlist id={id} not found"))?;

    let filter = if row.source_kind == repo::PLAYLIST_KIND_SMART {
        let blob = row
            .filter_json
            .as_deref()
            .ok_or_else(|| anyhow!("smart playlist id={id} missing filter_json"))?;
        Some(
            Filter::from_json(blob)
                .with_context(|| format!("decode filter_json id={id}"))?,
        )
    } else {
        None
    };

    let mode = Mode::from_str(&row.mode).unwrap_or_default();

    state.set_active(Some(id), filter);
    state.set_mode(mode);
    state.shuffle_seed = row.shuffle_seed as u64;

    let ids = resolve_active(db, snapshot, state).await?;
    state.refresh(ids);
    Ok(())
}

/// Switch back to the All pseudo-playlist (any wallpaper the source
/// manager currently knows about). The caller is expected to follow
/// this with a `state.refresh(snapshot ids)` once it has the snapshot;
/// kept separate so we don't need both DB and source-manager handles
/// here.
pub fn deactivate(state: &mut PlaylistState) {
    state.set_active(None, None);
    // Mode/seed kept as-is — the user's preferred sequence/shuffle on
    // All is independent of any specific playlist.
}

/// Re-resolve and write `state.ids` against the latest snapshot. Used
/// by `refresh_sources` to keep curated lists pruned of vanished items
/// and smart lists current with the freshly-rescanned library.
pub async fn rebind_after_refresh(
    db: &DatabaseConnection,
    snapshot: &[WallpaperEntry],
    state: &mut PlaylistState,
) -> Result<()> {
    let ids = resolve_active(db, snapshot, state).await?;
    state.refresh(ids);
    Ok(())
}

/// Convenience: same as [`rebind_after_refresh`] but takes the
/// shared mutex directly so callers in `control.rs` don't have to
/// manage the lock themselves. Returns the resolved id count for
/// logging.
pub async fn rebind_locked(
    db: &DatabaseConnection,
    snapshot: &[WallpaperEntry],
    locked: &tokio::sync::Mutex<PlaylistState>,
) -> Result<usize> {
    let mut g = locked.lock().await;
    rebind_after_refresh(db, snapshot, &mut g).await?;
    Ok(g.count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn entry(id: &str, library_root: &str, rel: &str) -> WallpaperEntry {
        WallpaperEntry {
            id: id.into(),
            name: id.into(),
            wp_type: "image".into(),
            resource: format!("{library_root}/{rel}"),
            preview: None,
            metadata: HashMap::new(),
            description: None,
            tags: Vec::new(),
            external_id: None,
            size: None,
            width: None,
            height: None,
            format: None,
            plugin_name: "p".into(),
            library_root: library_root.into(),
        }
    }

    #[test]
    fn index_lookup_normalizes_trailing_slash() {
        let snap = vec![entry("e1", "/lib", "a.png")];
        let idx = WallpaperIndex::build(&snap);
        assert_eq!(idx.lookup("/lib", "a.png"), Some("e1"));
        assert_eq!(idx.lookup("/lib/", "a.png"), Some("e1"));
        assert_eq!(idx.lookup("/lib", "missing.png"), None);
    }

    #[test]
    fn index_skips_entries_outside_their_root() {
        let mut e = entry("e1", "/lib", "a.png");
        e.resource = "/somewhere/else/a.png".into();
        let snap = vec![e];
        let idx = WallpaperIndex::build(&snap);
        assert_eq!(idx.lookup("/lib", "a.png"), None);
    }

    #[test]
    fn index_skips_entries_with_empty_library_root() {
        let mut e = entry("e1", "/lib", "a.png");
        e.library_root = "".into();
        let snap = vec![e];
        let idx = WallpaperIndex::build(&snap);
        assert!(idx.by_path.is_empty());
    }

    // ---- DB-backed integration tests ----

    use crate::model::connect_url;
    use crate::model::repo::{
        add_library, create_playlist, set_playlist_items, upsert_item, upsert_plugin,
        ItemUpsertArgs, PlaylistCreateArgs,
    };

    async fn mem_db() -> DatabaseConnection {
        connect_url("sqlite::memory:").await.unwrap()
    }

    fn min_args<'a>(plugin_id: i64, library_id: i64, path: &'a str) -> ItemUpsertArgs<'a> {
        ItemUpsertArgs {
            plugin_id,
            library_id,
            path,
            ty: "image",
            display_name: "",
            preview_path: None,
            description: None,
            external_id: None,
            size: None,
            width: None,
            height: None,
            format: None,
        }
    }

    #[tokio::test]
    async fn resolve_all_returns_snapshot_ids_in_order() {
        let db = mem_db().await;
        let snapshot = vec![
            entry("e1", "/lib", "a.png"),
            entry("e2", "/lib", "b.png"),
        ];
        let state = PlaylistState::default();
        let ids = resolve_active(&db, &snapshot, &state).await.unwrap();
        assert_eq!(ids, vec!["e1".to_string(), "e2".to_string()]);
    }

    #[tokio::test]
    async fn resolve_smart_runs_cached_filter() {
        let db = mem_db().await;
        let mut e_video = entry("v1", "/lib", "v.mp4");
        e_video.wp_type = "video".into();
        let snapshot = vec![entry("i1", "/lib", "i.png"), e_video];

        let mut filter = Filter::default();
        filter.wp_types = vec!["video".into()];
        let mut state = PlaylistState::default();
        state.set_active(Some(99), Some(filter));
        let ids = resolve_active(&db, &snapshot, &state).await.unwrap();
        assert_eq!(ids, vec!["v1".to_string()]);
    }

    #[tokio::test]
    async fn resolve_curated_bridges_item_ids_to_entry_ids_in_position_order() {
        let db = mem_db().await;
        let plug = upsert_plugin(&db, "p", "").await.unwrap();
        let lib = add_library(&db, plug.id, "/lib").await.unwrap();
        let item_a = upsert_item(&db, min_args(plug.id, lib.id, "a.png"))
            .await
            .unwrap();
        let item_b = upsert_item(&db, min_args(plug.id, lib.id, "b.png"))
            .await
            .unwrap();
        let item_c = upsert_item(&db, min_args(plug.id, lib.id, "c.png"))
            .await
            .unwrap();

        let pl = create_playlist(&db, PlaylistCreateArgs::curated("Pl"))
            .await
            .unwrap();
        // Position [c, a, b].
        set_playlist_items(&db, pl.id, &[item_c.id, item_a.id, item_b.id])
            .await
            .unwrap();

        let snapshot = vec![
            entry("e_a", "/lib", "a.png"),
            entry("e_b", "/lib", "b.png"),
            entry("e_c", "/lib", "c.png"),
        ];
        let mut state = PlaylistState::default();
        state.set_active(Some(pl.id), None);
        let ids = resolve_active(&db, &snapshot, &state).await.unwrap();
        assert_eq!(ids, vec!["e_c".to_string(), "e_a".to_string(), "e_b".to_string()]);
    }

    #[tokio::test]
    async fn resolve_curated_drops_members_missing_from_snapshot() {
        let db = mem_db().await;
        let plug = upsert_plugin(&db, "p", "").await.unwrap();
        let lib = add_library(&db, plug.id, "/lib").await.unwrap();
        let item_a = upsert_item(&db, min_args(plug.id, lib.id, "a.png"))
            .await
            .unwrap();
        let item_gone = upsert_item(&db, min_args(plug.id, lib.id, "gone.png"))
            .await
            .unwrap();

        let pl = create_playlist(&db, PlaylistCreateArgs::curated("Pl"))
            .await
            .unwrap();
        set_playlist_items(&db, pl.id, &[item_a.id, item_gone.id])
            .await
            .unwrap();

        // Snapshot only has `a.png` — `gone.png` was deleted from disk.
        let snapshot = vec![entry("e_a", "/lib", "a.png")];
        let mut state = PlaylistState::default();
        state.set_active(Some(pl.id), None);
        let ids = resolve_active(&db, &snapshot, &state).await.unwrap();
        assert_eq!(ids, vec!["e_a".to_string()]);
    }

    #[tokio::test]
    async fn activate_loads_mode_filter_and_seed_then_pins_state() {
        let db = mem_db().await;
        let f = r#"{"wp_types":["video"]}"#;
        let pl = create_playlist(&db, PlaylistCreateArgs::smart("Vids", f))
            .await
            .unwrap();
        crate::model::repo::set_playlist_mode(&db, pl.id, "shuffle")
            .await
            .unwrap();
        crate::model::repo::set_playlist_shuffle_seed(&db, pl.id, 7)
            .await
            .unwrap();

        let mut e_video = entry("v1", "/lib", "v.mp4");
        e_video.wp_type = "video".into();
        let snapshot = vec![entry("i1", "/lib", "i.png"), e_video];

        let mut state = PlaylistState::default();
        activate(&db, &snapshot, &mut state, pl.id).await.unwrap();

        assert_eq!(state.active_id, Some(pl.id));
        assert_eq!(state.mode, Mode::Shuffle);
        assert_eq!(state.shuffle_seed, 7);
        assert_eq!(state.ids, vec!["v1".to_string()]);
    }

    #[tokio::test]
    async fn rebind_after_refresh_keeps_smart_in_sync_with_growing_snapshot() {
        let db = mem_db().await;
        let mut state = PlaylistState::default();
        let mut filter = Filter::default();
        filter.wp_types = vec!["video".into()];
        state.set_active(Some(1), Some(filter));

        // Initial snapshot has zero videos.
        let snap0 = vec![entry("img1", "/lib", "a.png")];
        rebind_after_refresh(&db, &snap0, &mut state).await.unwrap();
        assert!(state.ids.is_empty());

        // Snapshot grows to include a video — re-bind catches it.
        let mut e_video = entry("vid1", "/lib", "v.mp4");
        e_video.wp_type = "video".into();
        let snap1 = vec![entry("img1", "/lib", "a.png"), e_video];
        rebind_after_refresh(&db, &snap1, &mut state).await.unwrap();
        assert_eq!(state.ids, vec!["vid1".to_string()]);
    }
}
