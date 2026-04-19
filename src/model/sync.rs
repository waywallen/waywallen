//! Glue between `SourceManager`'s in-memory scan results and the
//! persistent `library`/`item` tables.
//!
//! Mapping choice for Stage 3 (deliberately pragmatic):
//!
//! - A whole `SourceManager` snapshot is persisted under a single
//!   library row keyed by `source_name` (e.g. `"source_manager"`).
//!   Per-plugin partitioning will follow once `SourceManager` tags
//!   entries with their originating plugin; the schema is forward
//!   compatible since per-plugin would just mean "more library rows".
//! - `WallpaperEntry.id` is reused as `item.relative_path`. It's the
//!   only string each source guarantees unique within itself, which
//!   is exactly the uniqueness contract the schema needs. The column
//!   name stays `relative_path` because that's what a real folder
//!   library will put there — we're sharing one column across both
//!   shapes of "unique-within-library key".
//! - `WallpaperEntry.wp_type` → `item.type` verbatim.
//!
//! The flow is an upsert + prune: every scan is a full snapshot, so
//! entries not present in the new snapshot are deleted.

use std::collections::HashSet;

use anyhow::{Context, Result};
use sea_orm::DatabaseConnection;

use super::repo;
use crate::wallpaper_type::WallpaperEntry;

/// Counts from a single [`sync_source_entries`] call. Handy for log
/// lines so operators can see scan deltas at a glance.
#[derive(Debug, Default, Clone, Copy)]
pub struct SyncSummary {
    /// Rows the upsert loop touched (inserts + updates, not
    /// distinguishable at the SQL layer without extra work).
    pub upserted: usize,
    /// Rows deleted because they disappeared from the new snapshot.
    pub deleted: u64,
}

/// Persist `entries` as the complete current state of `source_name`.
/// Missing entries are pruned; returning entries are upserted
/// (type-column refreshed on every call).
pub async fn sync_source_entries(
    db: &DatabaseConnection,
    source_name: &str,
    entries: &[WallpaperEntry],
) -> Result<SyncSummary> {
    let lib_path = library_path_for_source(source_name);
    let library = match repo::find_library_by_path(db, &lib_path).await? {
        Some(existing) => existing,
        None => repo::add_library(db, &lib_path)
            .await
            .with_context(|| format!("create library for source={source_name}"))?,
    };

    let mut keep: HashSet<String> = HashSet::with_capacity(entries.len());
    for entry in entries {
        // Defensive: skip entries with empty id — they'd collide on
        // UNIQUE(library_id, relative_path) and aren't addressable
        // anyway.
        if entry.id.is_empty() {
            continue;
        }
        repo::upsert_item(db, library.id, &entry.id, &entry.wp_type).await?;
        keep.insert(entry.id.clone());
    }

    let deleted = repo::delete_items_missing(db, library.id, &keep).await?;
    Ok(SyncSummary {
        upserted: keep.len(),
        deleted,
    })
}

/// Canonical `library.path` value for a source-manager snapshot.
/// Prefixed so a real folder-library and a synthetic source-library
/// can never collide in the UNIQUE(path) index.
fn library_path_for_source(source_name: &str) -> String {
    format!("source:{source_name}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::connect_url;
    use std::collections::HashMap;

    fn entry(id: &str, wp_type: &str) -> WallpaperEntry {
        WallpaperEntry {
            id: id.to_owned(),
            name: id.to_owned(),
            wp_type: wp_type.to_owned(),
            resource: format!("/fake/{id}"),
            preview: None,
            metadata: HashMap::new(),
        }
    }

    async fn mem_db() -> DatabaseConnection {
        connect_url("sqlite::memory:").await.unwrap()
    }

    #[tokio::test]
    async fn first_sync_inserts_all_and_creates_library() {
        let db = mem_db().await;
        let entries = vec![
            entry("a", "image"),
            entry("b", "video"),
            entry("c", "image"),
        ];
        let summary = sync_source_entries(&db, "unit", &entries).await.unwrap();
        assert_eq!(summary.upserted, 3);
        assert_eq!(summary.deleted, 0);

        let libs = repo::list_libraries(&db).await.unwrap();
        assert_eq!(libs.len(), 1);
        assert_eq!(libs[0].path, "source:unit");

        let items = repo::list_items_by_library(&db, libs[0].id).await.unwrap();
        let by_id: HashMap<_, _> = items
            .iter()
            .map(|i| (i.relative_path.as_str(), i.ty.as_str()))
            .collect();
        assert_eq!(by_id.get("a"), Some(&"image"));
        assert_eq!(by_id.get("b"), Some(&"video"));
    }

    #[tokio::test]
    async fn second_sync_prunes_disappeared_entries() {
        let db = mem_db().await;
        sync_source_entries(
            &db,
            "unit",
            &[entry("a", "image"), entry("b", "image"), entry("c", "image")],
        )
        .await
        .unwrap();

        // Second snapshot drops b and c, adds d.
        let summary = sync_source_entries(&db, "unit", &[entry("a", "image"), entry("d", "image")])
            .await
            .unwrap();
        assert_eq!(summary.upserted, 2);
        assert_eq!(summary.deleted, 2);

        let libs = repo::list_libraries(&db).await.unwrap();
        let items = repo::list_items_by_library(&db, libs[0].id).await.unwrap();
        let ids: Vec<_> = items.iter().map(|i| i.relative_path.as_str()).collect();
        assert_eq!(ids, ["a", "d"]);
    }

    #[tokio::test]
    async fn resync_refreshes_type_column() {
        let db = mem_db().await;
        sync_source_entries(&db, "unit", &[entry("a", "image")])
            .await
            .unwrap();
        // Same id, different type classification.
        sync_source_entries(&db, "unit", &[entry("a", "gif")])
            .await
            .unwrap();

        let libs = repo::list_libraries(&db).await.unwrap();
        let items = repo::list_items_by_library(&db, libs[0].id).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].ty, "gif");
    }

    #[tokio::test]
    async fn empty_snapshot_clears_library() {
        let db = mem_db().await;
        sync_source_entries(&db, "unit", &[entry("a", "image"), entry("b", "image")])
            .await
            .unwrap();
        let summary = sync_source_entries(&db, "unit", &[]).await.unwrap();
        assert_eq!(summary.upserted, 0);
        assert_eq!(summary.deleted, 2);
    }

    #[tokio::test]
    async fn empty_id_entries_are_skipped() {
        let db = mem_db().await;
        let entries = vec![
            entry("", "image"), // should be ignored
            entry("real", "image"),
        ];
        let summary = sync_source_entries(&db, "unit", &entries).await.unwrap();
        assert_eq!(summary.upserted, 1);

        let libs = repo::list_libraries(&db).await.unwrap();
        let items = repo::list_items_by_library(&db, libs[0].id).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].relative_path, "real");
    }

    #[tokio::test]
    async fn distinct_source_names_get_distinct_libraries() {
        let db = mem_db().await;
        sync_source_entries(&db, "plugin_a", &[entry("x", "image")])
            .await
            .unwrap();
        sync_source_entries(&db, "plugin_b", &[entry("x", "image")])
            .await
            .unwrap();
        let libs = repo::list_libraries(&db).await.unwrap();
        assert_eq!(libs.len(), 2);
        let paths: Vec<_> = libs.iter().map(|l| l.path.as_str()).collect();
        assert!(paths.contains(&"source:plugin_a"));
        assert!(paths.contains(&"source:plugin_b"));
    }
}
