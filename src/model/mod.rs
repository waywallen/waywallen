//! SQLite persistence layer.
//!
//! Two tables seed the schema:
//!
//! - `library` — user-added wallpaper root folders.
//! - `item`    — individual wallpapers discovered inside a library,
//!               addressed by `(library_id, relative_path)`.
//!
//! The daemon opens a single pooled connection at startup via
//! [`connect`] and stashes it on `AppState.db`. Migrations run
//! transactionally on every boot and are idempotent.

use std::path::Path;

use anyhow::{Context, Result};
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
};
use sea_orm_migration::MigratorTrait;

pub mod entities;
pub mod migration;
pub mod repo;
pub mod sync;

/// Open (or create) the SQLite DB at `db_path`, run pending migrations,
/// and hand back a pooled [`DatabaseConnection`]. The parent directory
/// is created on demand so a fresh `$XDG_DATA_HOME` works on first run.
pub async fn connect(db_path: &Path) -> Result<DatabaseConnection> {
    if let Some(parent) = db_path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create db parent {}", parent.display()))?;
        }
    }
    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    connect_url(&url).await
}

/// Open a connection to an arbitrary SQLite URL. Exists so tests can
/// target `sqlite::memory:` without touching the filesystem.
pub async fn connect_url(url: &str) -> Result<DatabaseConnection> {
    let mut opt = ConnectOptions::new(url.to_owned());
    opt.sqlx_logging(false).max_connections(4);
    let db = Database::connect(opt)
        .await
        .with_context(|| format!("connect {url}"))?;
    // SQLite ignores FK declarations unless FK enforcement is opted in
    // per-connection. Do it before migrating so CREATE TABLE's FK
    // clauses start enforcing cascade on the very first use.
    db.execute(Statement::from_string(
        DatabaseBackend::Sqlite,
        "PRAGMA foreign_keys = ON",
    ))
    .await
    .context("enable sqlite foreign_keys")?;
    migration::Migrator::up(&db, None)
        .await
        .context("run migrations")?;
    Ok(db)
}
