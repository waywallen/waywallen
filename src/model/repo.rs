//! Typed CRUD helpers on top of the SeaORM entities.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction,
    EntityTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
};

use super::entities::{item, item_tag, library, source_plugin, tag};

// ---------------------------------------------------------------------------
// source_plugin
// ---------------------------------------------------------------------------

/// Insert or refresh a `source_plugin` row keyed by `name`. `version`
/// is updated on every call so a plugin bumping its version is
/// reflected without operator action.
pub async fn upsert_plugin(
    db: &DatabaseConnection,
    name: &str,
    version: &str,
) -> Result<source_plugin::Model> {
    if let Some(existing) = source_plugin::Entity::find()
        .filter(source_plugin::Column::Name.eq(name))
        .one(db)
        .await
        .with_context(|| format!("select plugin name={name}"))?
    {
        if existing.version == version {
            return Ok(existing);
        }
        let mut am: source_plugin::ActiveModel = existing.into();
        am.version = Set(version.to_owned());
        return am
            .update(db)
            .await
            .with_context(|| format!("update plugin version name={name}"));
    }
    let am = source_plugin::ActiveModel {
        name: Set(name.to_owned()),
        version: Set(version.to_owned()),
        ..Default::default()
    };
    am.insert(db)
        .await
        .with_context(|| format!("insert plugin name={name}"))
}

pub async fn list_plugins(db: &DatabaseConnection) -> Result<Vec<source_plugin::Model>> {
    source_plugin::Entity::find()
        .order_by_asc(source_plugin::Column::Id)
        .all(db)
        .await
        .context("select plugins")
}

pub async fn find_plugin_by_name(
    db: &DatabaseConnection,
    name: &str,
) -> Result<Option<source_plugin::Model>> {
    source_plugin::Entity::find()
        .filter(source_plugin::Column::Name.eq(name))
        .one(db)
        .await
        .with_context(|| format!("select plugin name={name}"))
}

pub async fn remove_plugin(db: &DatabaseConnection, id: i64) -> Result<u64> {
    let res = source_plugin::Entity::delete_by_id(id)
        .exec(db)
        .await
        .with_context(|| format!("delete plugin id={id}"))?;
    Ok(res.rows_affected)
}

// ---------------------------------------------------------------------------
// library
// ---------------------------------------------------------------------------

pub async fn add_library(
    db: &DatabaseConnection,
    plugin_id: i64,
    path: &str,
) -> Result<library::Model> {
    let am = library::ActiveModel {
        plugin_id: Set(plugin_id),
        path: Set(path.to_owned()),
        ..Default::default()
    };
    am.insert(db)
        .await
        .with_context(|| format!("insert library plugin={plugin_id} path={path}"))
}

pub async fn find_library(
    db: &DatabaseConnection,
    plugin_id: i64,
    path: &str,
) -> Result<Option<library::Model>> {
    library::Entity::find()
        .filter(library::Column::PluginId.eq(plugin_id))
        .filter(library::Column::Path.eq(path))
        .one(db)
        .await
        .with_context(|| format!("select library plugin={plugin_id} path={path}"))
}

pub async fn list_libraries_by_plugin(
    db: &DatabaseConnection,
    plugin_id: i64,
) -> Result<Vec<library::Model>> {
    library::Entity::find()
        .filter(library::Column::PluginId.eq(plugin_id))
        .order_by_asc(library::Column::Path)
        .all(db)
        .await
        .with_context(|| format!("select libraries plugin={plugin_id}"))
}

pub async fn list_libraries(db: &DatabaseConnection) -> Result<Vec<library::Model>> {
    library::Entity::find()
        .order_by_asc(library::Column::Id)
        .all(db)
        .await
        .context("select libraries")
}

pub async fn remove_library(db: &DatabaseConnection, id: i64) -> Result<u64> {
    let res = library::Entity::delete_by_id(id)
        .exec(db)
        .await
        .with_context(|| format!("delete library id={id}"))?;
    Ok(res.rows_affected)
}

pub async fn delete_libraries_missing(
    db: &DatabaseConnection,
    plugin_id: i64,
    keep: &HashSet<String>,
) -> Result<u64> {
    let mut q = library::Entity::delete_many().filter(library::Column::PluginId.eq(plugin_id));
    if !keep.is_empty() {
        q = q.filter(library::Column::Path.is_not_in(keep.iter().cloned()));
    }
    let res = q
        .exec(db)
        .await
        .with_context(|| format!("delete missing libraries plugin={plugin_id}"))?;
    Ok(res.rows_affected)
}

// ---------------------------------------------------------------------------
// item
// ---------------------------------------------------------------------------

/// Payload for [`upsert_item`]. All fields are ephemeral `&str`
/// borrows so the caller keeps ownership of the underlying entry.
pub struct ItemUpsertArgs<'a> {
    pub plugin_id: i64,
    pub library_id: i64,
    pub relative_path: &'a str,
    /// Stored lowercase by [`upsert_item`] so `"Scene"` and `"scene"`
    /// don't split on reads.
    pub ty: &'a str,
    pub display_name: &'a str,
    pub preview_path: Option<&'a str>,
    pub description: Option<&'a str>,
    pub external_id: Option<&'a str>,
    pub metadata_json: &'a str,
}

/// Upsert an item keyed by `(library_id, relative_path)`. Every
/// non-key column is refreshed on conflict (new scan is truth).
/// Returns the stored [`item::Model`] — the caller can use
/// `model.id` for tag linkage without an extra round-trip.
pub async fn upsert_item(
    db: &DatabaseConnection,
    args: ItemUpsertArgs<'_>,
) -> Result<item::Model> {
    let ty_norm = args.ty.to_lowercase();
    let am = item::ActiveModel {
        plugin_id: Set(args.plugin_id),
        library_id: Set(args.library_id),
        relative_path: Set(args.relative_path.to_owned()),
        ty: Set(ty_norm.clone()),
        display_name: Set(args.display_name.to_owned()),
        preview_path: Set(args.preview_path.map(str::to_owned)),
        description: Set(args.description.map(str::to_owned)),
        external_id: Set(args.external_id.map(str::to_owned)),
        metadata_json: Set(args.metadata_json.to_owned()),
        ..Default::default()
    };
    item::Entity::insert(am)
        .on_conflict(
            OnConflict::columns([item::Column::LibraryId, item::Column::RelativePath])
                .update_columns([
                    item::Column::Ty,
                    item::Column::PluginId,
                    item::Column::DisplayName,
                    item::Column::PreviewPath,
                    item::Column::Description,
                    item::Column::ExternalId,
                    item::Column::MetadataJson,
                ])
                .to_owned(),
        )
        .exec(db)
        .await
        .with_context(|| {
            format!(
                "upsert item lib={} rel={}",
                args.library_id, args.relative_path
            )
        })?;
    item::Entity::find()
        .filter(item::Column::LibraryId.eq(args.library_id))
        .filter(item::Column::RelativePath.eq(args.relative_path))
        .one(db)
        .await
        .with_context(|| {
            format!(
                "reload item lib={} rel={}",
                args.library_id, args.relative_path
            )
        })?
        .ok_or_else(|| anyhow::anyhow!("reloaded item missing after upsert"))
}

pub async fn list_items_by_library(
    db: &DatabaseConnection,
    library_id: i64,
) -> Result<Vec<item::Model>> {
    item::Entity::find()
        .filter(item::Column::LibraryId.eq(library_id))
        .order_by_asc(item::Column::RelativePath)
        .all(db)
        .await
        .with_context(|| format!("select items lib={library_id}"))
}

pub async fn list_items_by_plugin(
    db: &DatabaseConnection,
    plugin_id: i64,
) -> Result<Vec<item::Model>> {
    item::Entity::find()
        .filter(item::Column::PluginId.eq(plugin_id))
        .order_by_asc(item::Column::LibraryId)
        .order_by_asc(item::Column::RelativePath)
        .all(db)
        .await
        .with_context(|| format!("select items plugin={plugin_id}"))
}

pub async fn delete_items_missing(
    db: &DatabaseConnection,
    library_id: i64,
    keep: &HashSet<String>,
) -> Result<u64> {
    let mut q = item::Entity::delete_many().filter(item::Column::LibraryId.eq(library_id));
    if !keep.is_empty() {
        q = q.filter(item::Column::RelativePath.is_not_in(keep.iter().cloned()));
    }
    let res = q
        .exec(db)
        .await
        .with_context(|| format!("delete missing items lib={library_id}"))?;
    Ok(res.rows_affected)
}

// ---------------------------------------------------------------------------
// tag / item_tag
// ---------------------------------------------------------------------------

/// Upsert tags by name. SQLite `COLLATE NOCASE` makes the unique
/// index case-insensitive so "Anime" / "anime" collapse to one row
/// (first-seen casing wins). Returns models for every input name in
/// arbitrary order, deduped.
pub async fn upsert_tags(
    db: &DatabaseConnection,
    names: &[String],
) -> Result<Vec<tag::Model>> {
    // Deduplicate case-insensitively while preserving first-seen casing.
    let mut seen: HashSet<String> = HashSet::new();
    let mut unique_inputs: Vec<&str> = Vec::new();
    for n in names {
        let trimmed = n.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = trimmed.to_lowercase();
        if seen.insert(key) {
            unique_inputs.push(trimmed);
        }
    }
    let mut out = Vec::with_capacity(unique_inputs.len());
    for name in unique_inputs {
        let existing = tag::Entity::find()
            .filter(tag::Column::Name.eq(name))
            .one(db)
            .await
            .with_context(|| format!("select tag name={name}"))?;
        let model = match existing {
            Some(m) => m,
            None => tag::ActiveModel {
                name: Set(name.to_owned()),
                ..Default::default()
            }
            .insert(db)
            .await
            .with_context(|| format!("insert tag name={name}"))?,
        };
        out.push(model);
    }
    Ok(out)
}

/// Replace the complete tag set of an item. Runs DELETE + INSERT in
/// a single transaction so partial updates never leak out.
pub async fn replace_item_tags(
    db: &DatabaseConnection,
    item_id: i64,
    tag_ids: &[i64],
) -> Result<()> {
    let tx: DatabaseTransaction = db.begin().await.context("begin tx")?;
    item_tag::Entity::delete_many()
        .filter(item_tag::Column::ItemId.eq(item_id))
        .exec(&tx)
        .await
        .with_context(|| format!("clear item_tag item={item_id}"))?;
    // Dedupe tag_ids to keep the junction clean; composite PK would
    // reject duplicates anyway but we'd rather not roundtrip twice.
    let unique: HashSet<i64> = tag_ids.iter().copied().collect();
    if !unique.is_empty() {
        let rows: Vec<item_tag::ActiveModel> = unique
            .into_iter()
            .map(|tid| item_tag::ActiveModel {
                item_id: Set(item_id),
                tag_id: Set(tid),
            })
            .collect();
        item_tag::Entity::insert_many(rows)
            .exec(&tx)
            .await
            .with_context(|| format!("insert item_tag item={item_id}"))?;
    }
    tx.commit().await.context("commit tx")?;
    Ok(())
}

pub async fn list_tags(db: &DatabaseConnection) -> Result<Vec<tag::Model>> {
    tag::Entity::find()
        .order_by_asc(tag::Column::Name)
        .all(db)
        .await
        .context("select tags")
}

pub async fn list_items_by_tag(
    db: &DatabaseConnection,
    tag_id: i64,
) -> Result<Vec<item::Model>> {
    item::Entity::find()
        .inner_join(item_tag::Entity)
        .filter(item_tag::Column::TagId.eq(tag_id))
        .order_by_asc(item::Column::Id)
        .all(db)
        .await
        .with_context(|| format!("select items by tag={tag_id}"))
}

pub async fn list_tags_of_item(
    db: &DatabaseConnection,
    item_id: i64,
) -> Result<Vec<tag::Model>> {
    tag::Entity::find()
        .inner_join(item_tag::Entity)
        .filter(item_tag::Column::ItemId.eq(item_id))
        .order_by_asc(tag::Column::Name)
        .all(db)
        .await
        .with_context(|| format!("select tags of item={item_id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::connect_url;

    async fn mem_db() -> DatabaseConnection {
        connect_url("sqlite::memory:").await.unwrap()
    }

    fn minimal_args<'a>(plugin_id: i64, library_id: i64, rel: &'a str, ty: &'a str) -> ItemUpsertArgs<'a> {
        ItemUpsertArgs {
            plugin_id,
            library_id,
            relative_path: rel,
            ty,
            display_name: "",
            preview_path: None,
            description: None,
            external_id: None,
            metadata_json: "{}",
        }
    }

    #[tokio::test]
    async fn upsert_plugin_inserts_then_updates_version() {
        let db = mem_db().await;
        let p1 = upsert_plugin(&db, "wescene", "1.0").await.unwrap();
        assert_eq!(p1.version, "1.0");
        let p2 = upsert_plugin(&db, "wescene", "1.1").await.unwrap();
        assert_eq!(p2.id, p1.id);
        assert_eq!(p2.version, "1.1");
    }

    #[tokio::test]
    async fn library_path_scoped_per_plugin() {
        let db = mem_db().await;
        let a = upsert_plugin(&db, "a", "").await.unwrap();
        let b = upsert_plugin(&db, "b", "").await.unwrap();
        add_library(&db, a.id, "/shared").await.unwrap();
        add_library(&db, b.id, "/shared").await.unwrap();
        assert!(add_library(&db, a.id, "/shared").await.is_err());
    }

    #[tokio::test]
    async fn upsert_item_lowercases_type_and_returns_model() {
        let db = mem_db().await;
        let p = upsert_plugin(&db, "p", "").await.unwrap();
        let lib = add_library(&db, p.id, "/root").await.unwrap();
        let m = upsert_item(
            &db,
            ItemUpsertArgs {
                plugin_id: p.id,
                library_id: lib.id,
                relative_path: "a.png",
                ty: "Scene",
                display_name: "Hello",
                preview_path: Some("/p/thumb.jpg"),
                description: Some("desc"),
                external_id: Some("wk-1"),
                metadata_json: r#"{"scene":"/x"}"#,
            },
        )
        .await
        .unwrap();
        assert!(m.id > 0);
        assert_eq!(m.ty, "scene"); // lowercased
        assert_eq!(m.display_name, "Hello");
        assert_eq!(m.preview_path.as_deref(), Some("/p/thumb.jpg"));
        assert_eq!(m.external_id.as_deref(), Some("wk-1"));
        assert_eq!(m.metadata_json, r#"{"scene":"/x"}"#);
    }

    #[tokio::test]
    async fn upsert_item_refreshes_every_column_on_conflict() {
        let db = mem_db().await;
        let p = upsert_plugin(&db, "p", "").await.unwrap();
        let lib = add_library(&db, p.id, "/root").await.unwrap();
        upsert_item(
            &db,
            ItemUpsertArgs {
                plugin_id: p.id,
                library_id: lib.id,
                relative_path: "a.png",
                ty: "image",
                display_name: "Old",
                preview_path: None,
                description: None,
                external_id: None,
                metadata_json: "{}",
            },
        )
        .await
        .unwrap();
        let updated = upsert_item(
            &db,
            ItemUpsertArgs {
                plugin_id: p.id,
                library_id: lib.id,
                relative_path: "a.png",
                ty: "GIF",
                display_name: "New",
                preview_path: Some("/new/preview.png"),
                description: Some("now animated"),
                external_id: Some("ext-42"),
                metadata_json: r#"{"k":"v"}"#,
            },
        )
        .await
        .unwrap();
        assert_eq!(updated.ty, "gif");
        assert_eq!(updated.display_name, "New");
        assert_eq!(updated.description.as_deref(), Some("now animated"));
        assert_eq!(updated.external_id.as_deref(), Some("ext-42"));
        assert_eq!(updated.metadata_json, r#"{"k":"v"}"#);
    }

    #[tokio::test]
    async fn upsert_tags_dedupes_case_insensitively() {
        let db = mem_db().await;
        let tags = upsert_tags(
            &db,
            &["Anime".into(), "anime".into(), "Landscape".into(), "ANIME".into()],
        )
        .await
        .unwrap();
        assert_eq!(tags.len(), 2);
        let all = list_tags(&db).await.unwrap();
        let names: Vec<_> = all.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Anime", "Landscape"]);
    }

    #[tokio::test]
    async fn upsert_tags_skips_whitespace_entries() {
        let db = mem_db().await;
        let tags = upsert_tags(&db, &["  ".into(), "".into(), " Anime ".into()])
            .await
            .unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "Anime");
    }

    #[tokio::test]
    async fn replace_item_tags_idempotent_and_atomic() {
        let db = mem_db().await;
        let p = upsert_plugin(&db, "p", "").await.unwrap();
        let lib = add_library(&db, p.id, "/r").await.unwrap();
        let item = upsert_item(&db, minimal_args(p.id, lib.id, "a.png", "image"))
            .await
            .unwrap();
        let tags = upsert_tags(&db, &["Anime".into(), "Nature".into(), "Game".into()])
            .await
            .unwrap();
        let ids: HashMap<&str, i64> = tags.iter().map(|t| (t.name.as_str(), t.id)).collect();

        replace_item_tags(&db, item.id, &[ids["Anime"], ids["Nature"]])
            .await
            .unwrap();
        let after = list_tags_of_item(&db, item.id).await.unwrap();
        assert_eq!(after.len(), 2);

        // Replace wipes the previous set.
        replace_item_tags(&db, item.id, &[ids["Game"]]).await.unwrap();
        let after = list_tags_of_item(&db, item.id).await.unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].name, "Game");
    }

    #[tokio::test]
    async fn list_items_by_tag_crosses_libraries() {
        let db = mem_db().await;
        let p = upsert_plugin(&db, "p", "").await.unwrap();
        let l1 = add_library(&db, p.id, "/one").await.unwrap();
        let l2 = add_library(&db, p.id, "/two").await.unwrap();
        let i1 = upsert_item(&db, minimal_args(p.id, l1.id, "a", "image"))
            .await
            .unwrap();
        let i2 = upsert_item(&db, minimal_args(p.id, l2.id, "b", "image"))
            .await
            .unwrap();
        let tags = upsert_tags(&db, &["Shared".into()]).await.unwrap();
        replace_item_tags(&db, i1.id, &[tags[0].id]).await.unwrap();
        replace_item_tags(&db, i2.id, &[tags[0].id]).await.unwrap();

        let hits = list_items_by_tag(&db, tags[0].id).await.unwrap();
        assert_eq!(hits.len(), 2);
    }

    #[tokio::test]
    async fn item_delete_cascades_item_tag() {
        let db = mem_db().await;
        let p = upsert_plugin(&db, "p", "").await.unwrap();
        let lib = add_library(&db, p.id, "/r").await.unwrap();
        let item = upsert_item(&db, minimal_args(p.id, lib.id, "a", "image"))
            .await
            .unwrap();
        let tags = upsert_tags(&db, &["Anime".into()]).await.unwrap();
        replace_item_tags(&db, item.id, &[tags[0].id]).await.unwrap();

        // Dropping the library cascades to item, which must cascade
        // to item_tag — the tag row itself survives.
        remove_library(&db, lib.id).await.unwrap();
        assert!(list_items_by_tag(&db, tags[0].id).await.unwrap().is_empty());
        assert_eq!(list_tags(&db).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn remove_plugin_cascades_everything_including_item_tag() {
        let db = mem_db().await;
        let p = upsert_plugin(&db, "doomed", "").await.unwrap();
        let lib = add_library(&db, p.id, "/x").await.unwrap();
        let it = upsert_item(&db, minimal_args(p.id, lib.id, "a", "image"))
            .await
            .unwrap();
        let tags = upsert_tags(&db, &["T".into()]).await.unwrap();
        replace_item_tags(&db, it.id, &[tags[0].id]).await.unwrap();

        remove_plugin(&db, p.id).await.unwrap();
        assert!(list_plugins(&db).await.unwrap().is_empty());
        assert!(list_items_by_plugin(&db, p.id).await.unwrap().is_empty());
        assert!(list_items_by_tag(&db, tags[0].id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn delete_items_missing_prunes_and_respects_library_scope() {
        let db = mem_db().await;
        let p = upsert_plugin(&db, "p", "").await.unwrap();
        let l1 = add_library(&db, p.id, "/one").await.unwrap();
        let l2 = add_library(&db, p.id, "/two").await.unwrap();
        for rel in ["a", "b", "c"] {
            upsert_item(&db, minimal_args(p.id, l1.id, rel, "image"))
                .await
                .unwrap();
        }
        upsert_item(&db, minimal_args(p.id, l2.id, "z", "image"))
            .await
            .unwrap();

        let keep: HashSet<String> = ["a".to_owned()].into_iter().collect();
        let deleted = delete_items_missing(&db, l1.id, &keep).await.unwrap();
        assert_eq!(deleted, 2);
        assert_eq!(list_items_by_library(&db, l1.id).await.unwrap().len(), 1);
        assert_eq!(list_items_by_library(&db, l2.id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn delete_libraries_missing_drops_absent_and_cascades_items() {
        let db = mem_db().await;
        let p = upsert_plugin(&db, "p", "").await.unwrap();
        let keep_lib = add_library(&db, p.id, "/keep").await.unwrap();
        let drop_lib = add_library(&db, p.id, "/drop").await.unwrap();
        upsert_item(&db, minimal_args(p.id, drop_lib.id, "x", "image"))
            .await
            .unwrap();

        let keep_set: HashSet<String> = ["/keep".to_owned()].into_iter().collect();
        let deleted = delete_libraries_missing(&db, p.id, &keep_set).await.unwrap();
        assert_eq!(deleted, 1);

        let remaining = list_libraries_by_plugin(&db, p.id).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, keep_lib.id);
        assert_eq!(list_items_by_plugin(&db, p.id).await.unwrap().len(), 0);
    }
}
