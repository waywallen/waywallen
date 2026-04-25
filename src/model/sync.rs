//! Plugin-scoped persistence of scan snapshots.
//!
//! Groups incoming `WallpaperEntry` by `(plugin_name, library_root)`;
//! each distinct root becomes a `library` row whose `path` is the
//! absolute scanned directory. Both `item.path` and `item.preview_path`
//! are **relative to `library.path`** — the sync layer strips the
//! prefix; Lua plugins continue emitting absolute paths.
//!
//! Every sync is a full snapshot: libraries the plugin stopped
//! reporting are deleted; within each surviving library items absent
//! from the snapshot are deleted. Tags live in a shared `tag` table
//! and are linked via `item_tag` after each item upsert.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use sea_orm::DatabaseConnection;

use super::repo::{self, ItemUpsertArgs};
use crate::wallpaper_type::WallpaperEntry;

#[derive(Debug, Clone)]
pub struct PluginRef<'a> {
    pub name: &'a str,
    pub version: &'a str,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SyncSummary {
    pub items_upserted: usize,
    pub items_deleted: u64,
    pub libraries_deleted: u64,
    /// Entries the caller passed that we couldn't place in any
    /// library (empty `library_root` or `resource` not under it).
    pub dropped: usize,
}

/// Persist the full state of one plugin. Idempotent; reports counts.
///
/// `protected_libraries` lists library paths that must not be deleted
/// even if the scan returned no entries for them. Use this to feed in
/// the user-managed library set from the DB so an empty (or
/// momentarily-failing) scan does not nuke the user's configured
/// folders. Pass `&[]` for the legacy "scan owns the library set"
/// behaviour, e.g. in tests.
pub async fn sync_plugin_entries(
    db: &DatabaseConnection,
    plugin: PluginRef<'_>,
    entries: &[WallpaperEntry],
    protected_libraries: &[String],
) -> Result<(SyncSummary, super::entities::source_plugin::Model)> {
    let plugin_model = repo::upsert_plugin(db, plugin.name, plugin.version)
        .await
        .with_context(|| format!("upsert plugin={}", plugin.name))?;

    // (library_root -> Vec<(item.path, &entry)>). Keeping a reference
    // to the original entry lets us copy rich columns off without
    // reconstructing them.
    let mut grouped: HashMap<String, Vec<(String, &WallpaperEntry)>> = HashMap::new();
    let mut dropped = 0usize;
    for entry in entries {
        if entry.library_root.is_empty() {
            dropped += 1;
            log::warn!(
                "sync plugin={} drop entry id={} resource={}: empty library_root",
                plugin.name,
                entry.id,
                entry.resource,
            );
            continue;
        }
        match relative_under_root(&entry.library_root, &entry.resource) {
            Some(rel) if !rel.is_empty() => {
                grouped
                    .entry(entry.library_root.clone())
                    .or_default()
                    .push((rel, entry));
            }
            _ => {
                dropped += 1;
                log::warn!(
                    "sync plugin={} drop entry resource={} not under library_root={}",
                    plugin.name,
                    entry.resource,
                    entry.library_root,
                );
            }
        }
    }

    // Upsert every tag once up front and build a lower→id map.
    let mut all_tag_names: Vec<String> = Vec::new();
    for entry in entries {
        for t in &entry.tags {
            all_tag_names.push(t.clone());
        }
    }
    let tag_models = repo::upsert_tags(db, &all_tag_names).await?;
    let tag_id_by_lower: HashMap<String, i64> = tag_models
        .into_iter()
        .map(|t| (t.name.to_lowercase(), t.id))
        .collect();

    let mut summary = SyncSummary {
        dropped,
        ..Default::default()
    };
    let mut keep_lib_paths: HashSet<String> = HashSet::with_capacity(grouped.len());

    for (lib_path, items) in &grouped {
        let lib_model = match repo::find_library(db, plugin_model.id, lib_path).await? {
            Some(existing) => existing,
            None => repo::add_library(db, plugin_model.id, lib_path).await?,
        };
        keep_lib_paths.insert(lib_path.clone());

        let mut keep_items: HashSet<String> = HashSet::with_capacity(items.len());
        for (rel, entry) in items {
            let preview_rel = entry
                .preview
                .as_deref()
                .and_then(|abs| relative_under_root(lib_path, abs))
                .filter(|s| !s.is_empty());

            // Sync only persists what the plugin emitted. Missing
            // media metadata stays NULL — the daemon's scheduled
            // probe task is responsible for filling those columns
            // out-of-band.
            let size = entry.size;
            let width = entry.width.and_then(|v| i32::try_from(v).ok());
            let height = entry.height.and_then(|v| i32::try_from(v).ok());
            let format_str = entry.format.clone();

            let persisted = repo::upsert_item(
                db,
                ItemUpsertArgs {
                    plugin_id: plugin_model.id,
                    library_id: lib_model.id,
                    path: rel,
                    ty: &entry.wp_type,
                    display_name: &entry.name,
                    preview_path: preview_rel.as_deref(),
                    description: entry.description.as_deref(),
                    external_id: entry.external_id.as_deref(),
                    size,
                    width,
                    height,
                    format: format_str.as_deref(),
                },
            )
            .await?;
            let tag_ids: Vec<i64> = entry
                .tags
                .iter()
                .filter_map(|n| tag_id_by_lower.get(&n.trim().to_lowercase()).copied())
                .collect();
            repo::replace_item_tags(db, persisted.id, &tag_ids).await?;
            keep_items.insert(rel.clone());
        }
        summary.items_upserted += keep_items.len();
        summary.items_deleted += repo::delete_items_missing(db, lib_model.id, &keep_items).await?;
    }

    // Protect user-managed libraries from being swept up by the
    // missing-library deletion: even if the scan emitted no entries
    // for them this round, they still belong to the user.
    for path in protected_libraries {
        keep_lib_paths.insert(path.clone());
    }
    summary.libraries_deleted +=
        repo::delete_libraries_missing(db, plugin_model.id, &keep_lib_paths).await?;

    Ok((summary, plugin_model))
}

pub(crate) fn relative_under_root(root: &str, resource: &str) -> Option<String> {
    let root = root.trim_end_matches('/');
    Path::new(resource)
        .strip_prefix(root)
        .ok()
        .and_then(|p| p.to_str().map(|s| s.trim_start_matches('/').to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::connect_url;

    fn entry(
        plugin_name: &str,
        library_root: &str,
        resource: &str,
        wp_type: &str,
    ) -> WallpaperEntry {
        WallpaperEntry {
            id: resource.to_owned(),
            name: resource.to_owned(),
            wp_type: wp_type.to_owned(),
            resource: resource.to_owned(),
            preview: None,
            metadata: HashMap::new(),
            plugin_name: plugin_name.to_owned(),
            library_root: library_root.to_owned(),
            description: None,
            tags: Vec::new(),
            external_id: None,
            size: None,
            width: None,
            height: None,
            format: None,
        }
    }

    async fn mem_db() -> DatabaseConnection {
        connect_url("sqlite::memory:").await.unwrap()
    }

    #[tokio::test]
    async fn first_sync_groups_by_root_and_strips_prefix() {
        let db = mem_db().await;
        let entries = [
            entry("image", "/home/u/Pictures", "/home/u/Pictures/a.png", "image"),
            entry(
                "image",
                "/home/u/Pictures",
                "/home/u/Pictures/sub/b.png",
                "image",
            ),
            entry("image", "/other/root", "/other/root/z.png", "image"),
        ];
        let (summary, _) = sync_plugin_entries(
            &db,
            PluginRef { name: "image", version: "0.1" },
            &entries,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(summary.items_upserted, 3);
        assert_eq!(summary.dropped, 0);

        let plugin = repo::find_plugin_by_name(&db, "image").await.unwrap().unwrap();
        let libs = repo::list_libraries_by_plugin(&db, plugin.id).await.unwrap();
        let home_lib = libs.iter().find(|l| l.path == "/home/u/Pictures").unwrap();
        let items = repo::list_items_by_library(&db, home_lib.id).await.unwrap();
        let paths: Vec<_> = items.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(paths, ["a.png", "sub/b.png"]);
    }

    #[tokio::test]
    async fn preview_path_stored_relative_to_library() {
        let db = mem_db().await;
        let mut e = entry("p", "/ws", "/ws/12345/scene.pkg", "scene");
        e.preview = Some("/ws/12345/preview.gif".into());
        let _ = sync_plugin_entries(&db, PluginRef { name: "p", version: "" }, &[e], &[])
            .await
            .unwrap();
        let plugin = repo::find_plugin_by_name(&db, "p").await.unwrap().unwrap();
        let items = repo::list_items_by_plugin(&db, plugin.id).await.unwrap();
        assert_eq!(items[0].path, "12345/scene.pkg");
        assert_eq!(items[0].preview_path.as_deref(), Some("12345/preview.gif"));
    }

    #[tokio::test]
    async fn preview_outside_library_becomes_none() {
        let db = mem_db().await;
        let mut e = entry("p", "/root", "/root/a.png", "image");
        e.preview = Some("/elsewhere/thumb.png".into());
        let _ = sync_plugin_entries(&db, PluginRef { name: "p", version: "" }, &[e], &[])
            .await
            .unwrap();
        let plugin = repo::find_plugin_by_name(&db, "p").await.unwrap().unwrap();
        let items = repo::list_items_by_plugin(&db, plugin.id).await.unwrap();
        assert!(items[0].preview_path.is_none());
    }

    #[tokio::test]
    async fn entry_outside_root_is_dropped() {
        let db = mem_db().await;
        let entries = [
            entry("p", "/root", "/root/ok.png", "image"),
            entry("p", "/root", "/elsewhere/bad.png", "image"),
        ];
        let (summary, _) = sync_plugin_entries(&db, PluginRef { name: "p", version: "" }, &entries, &[])
                .await
                .unwrap();
        assert_eq!(summary.items_upserted, 1);
        assert_eq!(summary.dropped, 1);
    }

    #[tokio::test]
    async fn type_is_normalized_lowercase() {
        let db = mem_db().await;
        let entries = [entry("p", "/r", "/r/a.png", "Scene")];
        let _ = sync_plugin_entries(&db, PluginRef { name: "p", version: "" }, &entries, &[])
            .await
            .unwrap();
        let plugin = repo::find_plugin_by_name(&db, "p").await.unwrap().unwrap();
        let libs = repo::list_libraries_by_plugin(&db, plugin.id).await.unwrap();
        let items = repo::list_items_by_library(&db, libs[0].id).await.unwrap();
        assert_eq!(items[0].ty, "scene");
    }

    #[tokio::test]
    async fn rich_columns_and_tags_persist() {
        let db = mem_db().await;
        let we = WallpaperEntry {
            id: "12345".to_owned(),
            name: "Forest River".to_owned(),
            wp_type: "scene".to_owned(),
            resource: "/ws/12345/scene.pkg".to_owned(),
            preview: Some("/ws/12345/preview.gif".to_owned()),
            metadata: HashMap::new(),
            plugin_name: "wallpaper_engine".to_owned(),
            library_root: "/ws".to_owned(),
            description: Some("rain and music".to_owned()),
            tags: vec!["Nature".to_owned(), "relaxing".to_owned()],
            external_id: Some("12345".to_owned()),
            size: None,
            width: None,
            height: None,
            format: None,
        };
        let _ = sync_plugin_entries(
            &db,
            PluginRef { name: "wallpaper_engine", version: "0.2.0" },
            &[we],
            &[],
        )
        .await
        .unwrap();

        let plugin =
            repo::find_plugin_by_name(&db, "wallpaper_engine").await.unwrap().unwrap();
        let items = repo::list_items_by_plugin(&db, plugin.id).await.unwrap();
        let it = &items[0];
        assert_eq!(it.path, "12345/scene.pkg");
        assert_eq!(it.display_name, "Forest River");
        assert_eq!(it.preview_path.as_deref(), Some("12345/preview.gif"));
        assert_eq!(it.description.as_deref(), Some("rain and music"));
        assert_eq!(it.external_id.as_deref(), Some("12345"));
        let tags = repo::list_tags_of_item(&db, it.id).await.unwrap();
        assert_eq!(tags.len(), 2);
    }

    #[tokio::test]
    async fn tag_casing_collapses_across_entries() {
        let db = mem_db().await;
        let mk = |rel: &str, tag: &str| {
            let mut e = entry("p", "/r", &format!("/r/{rel}"), "image");
            e.tags = vec![tag.to_owned()];
            e
        };
        let entries = [mk("a.png", "Anime"), mk("b.png", "anime"), mk("c.png", "ANIME")];
        let _ = sync_plugin_entries(&db, PluginRef { name: "p", version: "" }, &entries, &[])
            .await
            .unwrap();
        assert_eq!(repo::list_tags(&db).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn second_sync_refreshes_tag_set() {
        let db = mem_db().await;
        let mut first = entry("p", "/r", "/r/a.png", "image");
        first.tags = vec!["Anime".into(), "Nature".into()];
        let _ = sync_plugin_entries(&db, PluginRef { name: "p", version: "" }, &[first], &[])
            .await
            .unwrap();

        let mut second = entry("p", "/r", "/r/a.png", "image");
        second.tags = vec!["Game".into()];
        let _ = sync_plugin_entries(&db, PluginRef { name: "p", version: "" }, &[second], &[])
            .await
            .unwrap();

        let plugin = repo::find_plugin_by_name(&db, "p").await.unwrap().unwrap();
        let items = repo::list_items_by_plugin(&db, plugin.id).await.unwrap();
        let tags = repo::list_tags_of_item(&db, items[0].id).await.unwrap();
        let names: Vec<_> = tags.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Game"]);
    }

    #[tokio::test]
    async fn second_sync_prunes_items_and_libraries() {
        let db = mem_db().await;
        let first = [
            entry("p", "/a", "/a/x.png", "image"),
            entry("p", "/a", "/a/y.png", "image"),
            entry("p", "/b", "/b/z.png", "image"),
        ];
        let _ = sync_plugin_entries(&db, PluginRef { name: "p", version: "1" }, &first, &[])
            .await
            .unwrap();

        let second = [entry("p", "/a", "/a/x.png", "image")];
        let (summary, _) = sync_plugin_entries(&db, PluginRef { name: "p", version: "1" }, &second, &[])
                .await
                .unwrap();
        assert_eq!(summary.items_upserted, 1);
        assert_eq!(summary.items_deleted, 1);
        assert_eq!(summary.libraries_deleted, 1);
    }

    #[tokio::test]
    async fn empty_snapshot_prunes_all_libraries() {
        let db = mem_db().await;
        let _ = sync_plugin_entries(
            &db,
            PluginRef { name: "p", version: "" },
            &[entry("p", "/one", "/one/x.png", "image")],
            &[],
        )
        .await
        .unwrap();
        let (summary, _) = sync_plugin_entries(&db, PluginRef { name: "p", version: "" }, &[], &[])
                .await
                .unwrap();
        assert_eq!(summary.libraries_deleted, 1);
    }

    #[tokio::test]
    async fn media_meta_remains_null_when_entry_lacks_it() {
        let db = mem_db().await;
        let e = entry("p", "/r", "/r/a.mp4", "video");
        let _ = sync_plugin_entries(&db, PluginRef { name: "p", version: "" }, &[e], &[])
            .await
            .unwrap();
        let plugin = repo::find_plugin_by_name(&db, "p").await.unwrap().unwrap();
        let items = repo::list_items_by_plugin(&db, plugin.id).await.unwrap();
        let it = &items[0];
        // Sync no longer probes — missing fields stay None for the
        // background probe task to fill in.
        assert_eq!(it.size, None);
        assert_eq!(it.width, None);
        assert_eq!(it.height, None);
        assert_eq!(it.format, None);
    }

    #[tokio::test]
    async fn plugin_provided_media_meta_persisted() {
        let db = mem_db().await;
        let mut e = entry("p", "/r", "/r/b.mp4", "video");
        e.size = Some(42);
        e.width = Some(1920);
        e.height = Some(1080);
        e.format = Some("matroska,webm".to_owned());
        let _ = sync_plugin_entries(&db, PluginRef { name: "p", version: "" }, &[e], &[])
            .await
            .unwrap();
        let plugin = repo::find_plugin_by_name(&db, "p").await.unwrap().unwrap();
        let items = repo::list_items_by_plugin(&db, plugin.id).await.unwrap();
        let it = &items[0];
        assert_eq!(it.size, Some(42));
        assert_eq!(it.width, Some(1920));
        assert_eq!(it.height, Some(1080));
        assert_eq!(it.format.as_deref(), Some("matroska,webm"));
    }

    #[tokio::test]
    async fn first_insert_stamps_create_at_and_preserves_it_on_conflict() {
        let db = mem_db().await;
        let e = entry("p", "/r", "/r/a.png", "image");
        let _ = sync_plugin_entries(&db, PluginRef { name: "p", version: "" }, &[e.clone()], &[])
            .await
            .unwrap();
        let plugin = repo::find_plugin_by_name(&db, "p").await.unwrap().unwrap();
        let items = repo::list_items_by_plugin(&db, plugin.id).await.unwrap();
        let first_create = items[0].create_at;
        let first_update = items[0].update_at;
        assert!(first_create > 0);
        assert_eq!(first_update, first_create);
        // Force the wall clock to advance so the second upsert sees a
        // strictly newer now_ms() than the first.
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        let _ = sync_plugin_entries(&db, PluginRef { name: "p", version: "" }, &[e], &[])
            .await
            .unwrap();
        let items2 = repo::list_items_by_plugin(&db, plugin.id).await.unwrap();
        assert_eq!(items2[0].create_at, first_create, "create_at must be sticky");
        assert!(items2[0].update_at >= first_update);
        assert!(items2[0].sync_at >= first_update);
    }
}
