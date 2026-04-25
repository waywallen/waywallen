//! `item` table.
//!
//! `plugin_id` is denormalized from `library.plugin_id` to keep
//! per-plugin filter queries cheap; the repo layer enforces they
//! stay consistent. `path` / `preview_path` are stored **relative to
//! `library.path`** so the physical root can move without a rewrite.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "item")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub plugin_id: i64,
    pub library_id: i64,
    pub path: String,
    #[sea_orm(column_name = "type")]
    pub ty: String,
    pub display_name: String,
    pub preview_path: Option<String>,
    pub description: Option<String>,
    pub external_id: Option<String>,
    pub size: Option<i64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub format: Option<String>,
    /// Milliseconds since UNIX epoch. Set on first INSERT, never
    /// updated on subsequent upserts of the same `(library_id, path)`.
    pub create_at: i64,
    /// Milliseconds since UNIX epoch. Refreshed on every upsert
    /// (sync) and on every probe pass that actually changed a field.
    pub update_at: i64,
    /// Milliseconds since UNIX epoch. Refreshed any time the daemon
    /// "sees" the item — both scan-sync and probe-task ticks.
    pub sync_at: i64,
    /// Milliseconds since UNIX epoch of the last probe attempt
    /// (success or no-op). `None` means never probed. Drives the
    /// probe-task cooldown so re-tries don't storm; `sync_at`
    /// alone can't fill this role because sync also bumps it.
    pub probed_at: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::source_plugin::Entity",
        from = "Column::PluginId",
        to = "super::source_plugin::Column::Id",
        on_delete = "Cascade"
    )]
    SourcePlugin,
    #[sea_orm(
        belongs_to = "super::library::Entity",
        from = "Column::LibraryId",
        to = "super::library::Column::Id",
        on_delete = "Cascade"
    )]
    Library,
    #[sea_orm(has_many = "super::item_tag::Entity")]
    ItemTag,
}

impl Related<super::source_plugin::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::SourcePlugin.def()
    }
}

impl Related<super::library::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Library.def()
    }
}

impl Related<super::item_tag::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ItemTag.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
