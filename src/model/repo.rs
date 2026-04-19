//! Typed CRUD helpers on top of the SeaORM entities. Stage 1 keeps the
//! surface minimal — just enough to prove the wiring end-to-end; the
//! rest lands in Stage 2 per the plan.

use anyhow::{Context, Result};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};

use super::entities::{item, library};

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

/// Insert a new `item` row. Fails on `(library_id, relative_path)`
/// UNIQUE conflict — callers doing incremental sync should use a
/// proper upsert (Stage 2).
pub async fn add_item(
    db: &DatabaseConnection,
    library_id: i64,
    relative_path: &str,
    ty: &str,
) -> Result<item::Model> {
    let am = item::ActiveModel {
        library_id: Set(library_id),
        relative_path: Set(relative_path.to_owned()),
        ty: Set(ty.to_owned()),
        ..Default::default()
    };
    am.insert(db)
        .await
        .with_context(|| format!("insert item lib={library_id} rel={relative_path}"))
}

/// List every library, oldest-first.
pub async fn list_libraries(db: &DatabaseConnection) -> Result<Vec<library::Model>> {
    library::Entity::find()
        .all(db)
        .await
        .context("select libraries")
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
    async fn add_item_with_library_fk() {
        let db = mem_db().await;
        let lib = add_library(&db, "/tmp/wp2").await.unwrap();
        let it = add_item(&db, lib.id, "sub/a.png", "image").await.unwrap();
        assert_eq!(it.library_id, lib.id);
        assert_eq!(it.ty, "image");
    }

    #[tokio::test]
    async fn duplicate_library_path_rejected() {
        let db = mem_db().await;
        add_library(&db, "/tmp/dup").await.unwrap();
        let err = add_library(&db, "/tmp/dup").await;
        assert!(err.is_err(), "UNIQUE(path) should reject duplicate");
    }

    #[tokio::test]
    async fn duplicate_item_relative_path_rejected() {
        let db = mem_db().await;
        let lib = add_library(&db, "/tmp/wp3").await.unwrap();
        add_item(&db, lib.id, "a.png", "image").await.unwrap();
        let err = add_item(&db, lib.id, "a.png", "image").await;
        assert!(err.is_err(), "UNIQUE(library_id, relative_path) should reject duplicate");
    }
}
