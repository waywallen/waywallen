//! Typed CRUD helpers on top of the SeaORM entities.
//!
//! All helpers take `&DatabaseConnection` and return `anyhow::Result`
//! so callers can stay in one error ecosystem. Write paths prefer
//! upsert semantics over blind insert to tolerate re-scans.

use std::collections::HashSet;

use anyhow::{Context, Result};
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};

use super::entities::{item, library};

// ---------------------------------------------------------------------------
// library
// ---------------------------------------------------------------------------

/// Insert a new `library` row. Returns the created model (id assigned).
/// Fails if `path` already exists (UNIQUE constraint).
pub async fn add_library(db: &DatabaseConnection, path: &str) -> Result<library::Model> {
    let am = library::ActiveModel {
        path: Set(path.to_owned()),
        ..Default::default()
    };
    am.insert(db)
        .await
        .with_context(|| format!("insert library path={path}"))
}

/// List every library, oldest id first (insertion order).
pub async fn list_libraries(db: &DatabaseConnection) -> Result<Vec<library::Model>> {
    library::Entity::find()
        .order_by_asc(library::Column::Id)
        .all(db)
        .await
        .context("select libraries")
}

/// Look up a library by its absolute path. Returns `None` when no row
/// matches — callers typically treat that as "not yet added".
pub async fn find_library_by_path(
    db: &DatabaseConnection,
    path: &str,
) -> Result<Option<library::Model>> {
    library::Entity::find()
        .filter(library::Column::Path.eq(path))
        .one(db)
        .await
        .with_context(|| format!("select library path={path}"))
}

/// Delete a library by id. FK `ON DELETE CASCADE` sweeps every item
/// that belonged to it — `PRAGMA foreign_keys = ON` (enabled in
/// [`super::connect_url`]) is what makes SQLite honour that clause.
/// Returns the number of rows deleted (0 if the id wasn't present).
pub async fn remove_library(db: &DatabaseConnection, id: i64) -> Result<u64> {
    let res = library::Entity::delete_by_id(id)
        .exec(db)
        .await
        .with_context(|| format!("delete library id={id}"))?;
    Ok(res.rows_affected)
}

// ---------------------------------------------------------------------------
// item
// ---------------------------------------------------------------------------

/// Upsert a single item keyed by `(library_id, relative_path)`. On
/// conflict the stored `type` is overwritten so re-scans pick up
/// re-classifications.
pub async fn upsert_item(
    db: &DatabaseConnection,
    library_id: i64,
    relative_path: &str,
    ty: &str,
) -> Result<()> {
    let am = item::ActiveModel {
        library_id: Set(library_id),
        relative_path: Set(relative_path.to_owned()),
        ty: Set(ty.to_owned()),
        ..Default::default()
    };
    item::Entity::insert(am)
        .on_conflict(
            OnConflict::columns([item::Column::LibraryId, item::Column::RelativePath])
                .update_column(item::Column::Ty)
                .to_owned(),
        )
        .exec(db)
        .await
        .with_context(|| format!("upsert item lib={library_id} rel={relative_path}"))?;
    Ok(())
}

/// List every item in a library, sorted by `relative_path` ascending
/// so the output is stable across calls regardless of insert order.
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

/// Prune every item in `library_id` whose `relative_path` is NOT in
/// `keep`. Intended to follow a scan where `keep` is the full set of
/// paths still present on disk. Returns the number of rows deleted.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::connect_url;

    async fn mem_db() -> DatabaseConnection {
        connect_url("sqlite::memory:")
            .await
            .expect("open in-memory db")
    }

    #[tokio::test]
    async fn add_library_roundtrips() {
        let db = mem_db().await;
        let lib = add_library(&db, "/tmp/wallpapers").await.unwrap();
        assert!(lib.id > 0);
        assert_eq!(lib.path, "/tmp/wallpapers");

        let all = list_libraries(&db).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, lib.id);
    }

    #[tokio::test]
    async fn find_library_by_path_roundtrip() {
        let db = mem_db().await;
        assert!(find_library_by_path(&db, "/tmp/nope").await.unwrap().is_none());
        let lib = add_library(&db, "/tmp/foo").await.unwrap();
        let hit = find_library_by_path(&db, "/tmp/foo").await.unwrap();
        assert_eq!(hit.map(|m| m.id), Some(lib.id));
    }

    #[tokio::test]
    async fn duplicate_library_path_rejected() {
        let db = mem_db().await;
        add_library(&db, "/tmp/dup").await.unwrap();
        assert!(add_library(&db, "/tmp/dup").await.is_err());
    }

    #[tokio::test]
    async fn upsert_item_inserts_and_updates() {
        let db = mem_db().await;
        let lib = add_library(&db, "/tmp/up").await.unwrap();
        upsert_item(&db, lib.id, "a.png", "image").await.unwrap();
        upsert_item(&db, lib.id, "a.png", "gif").await.unwrap(); // update path

        let items = list_items_by_library(&db, lib.id).await.unwrap();
        assert_eq!(items.len(), 1, "second upsert must not duplicate");
        assert_eq!(items[0].ty, "gif", "second upsert must overwrite type");
    }

    #[tokio::test]
    async fn list_items_is_sorted_by_relative_path() {
        let db = mem_db().await;
        let lib = add_library(&db, "/tmp/sort").await.unwrap();
        // Insert out-of-order.
        upsert_item(&db, lib.id, "z.png", "image").await.unwrap();
        upsert_item(&db, lib.id, "a.png", "image").await.unwrap();
        upsert_item(&db, lib.id, "m.png", "image").await.unwrap();
        let items = list_items_by_library(&db, lib.id).await.unwrap();
        let paths: Vec<_> = items.iter().map(|i| i.relative_path.as_str()).collect();
        assert_eq!(paths, ["a.png", "m.png", "z.png"]);
    }

    #[tokio::test]
    async fn remove_library_cascades_items() {
        let db = mem_db().await;
        let lib = add_library(&db, "/tmp/casc").await.unwrap();
        upsert_item(&db, lib.id, "a.png", "image").await.unwrap();
        upsert_item(&db, lib.id, "b.png", "image").await.unwrap();

        let removed = remove_library(&db, lib.id).await.unwrap();
        assert_eq!(removed, 1);

        // Items must be gone even though we didn't delete them explicitly.
        let leftover = list_items_by_library(&db, lib.id).await.unwrap();
        assert!(leftover.is_empty(), "items must cascade on library delete");
    }

    #[tokio::test]
    async fn remove_library_returns_zero_for_missing_id() {
        let db = mem_db().await;
        let removed = remove_library(&db, 9999).await.unwrap();
        assert_eq!(removed, 0);
    }

    #[tokio::test]
    async fn delete_items_missing_prunes_absent_paths() {
        let db = mem_db().await;
        let lib = add_library(&db, "/tmp/miss").await.unwrap();
        for rel in ["a.png", "b.png", "c.png", "d.png"] {
            upsert_item(&db, lib.id, rel, "image").await.unwrap();
        }
        let keep: HashSet<String> =
            ["a.png", "c.png"].into_iter().map(String::from).collect();
        let deleted = delete_items_missing(&db, lib.id, &keep).await.unwrap();
        assert_eq!(deleted, 2);

        let remaining: Vec<_> = list_items_by_library(&db, lib.id)
            .await
            .unwrap()
            .into_iter()
            .map(|i| i.relative_path)
            .collect();
        assert_eq!(remaining, ["a.png", "c.png"]);
    }

    #[tokio::test]
    async fn delete_items_missing_empty_keep_wipes_library() {
        let db = mem_db().await;
        let lib = add_library(&db, "/tmp/wipe").await.unwrap();
        upsert_item(&db, lib.id, "a.png", "image").await.unwrap();
        upsert_item(&db, lib.id, "b.png", "image").await.unwrap();

        let deleted = delete_items_missing(&db, lib.id, &HashSet::new())
            .await
            .unwrap();
        assert_eq!(deleted, 2);
        assert!(list_items_by_library(&db, lib.id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn delete_items_missing_does_not_touch_other_libraries() {
        let db = mem_db().await;
        let a = add_library(&db, "/tmp/a").await.unwrap();
        let b = add_library(&db, "/tmp/b").await.unwrap();
        upsert_item(&db, a.id, "x.png", "image").await.unwrap();
        upsert_item(&db, b.id, "x.png", "image").await.unwrap();

        // Wipe library A.
        delete_items_missing(&db, a.id, &HashSet::new()).await.unwrap();

        assert!(list_items_by_library(&db, a.id).await.unwrap().is_empty());
        assert_eq!(list_items_by_library(&db, b.id).await.unwrap().len(), 1);
    }
}
