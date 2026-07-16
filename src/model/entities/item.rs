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
    pub content_rating: Option<String>,
    /// Milliseconds since UNIX epoch. Set on first INSERT, never
    /// updated on subsequent upserts of the same `(library_id, path)`.
    pub create_at: i64,
    /// Milliseconds since UNIX epoch. Refreshed on every upsert
    /// (sync) and on every probe pass that actually changed a field.
    pub update_at: i64,
    /// Milliseconds since UNIX epoch. Refreshed any time the daemon
    /// "sees" the item — both scan-sync and probe-task ticks.
    pub sync_at: i64,
    /// Milliseconds since UNIX epoch of the last media-probe attempt.
    /// Cooldown anchor for libavformat-backed probing.
    pub probed_at: Option<i64>,
    /// File mtime in milliseconds since UNIX epoch, captured by the
    /// stat tier of the probe scheduler. Distinct from `update_at`
    pub modified_at: Option<i64>,
    /// Cooldown anchor for the stat tier. `None` means never stat'd.
    pub stat_at: Option<i64>,
    /// JSON map of per-item user-property overrides. Each key is the
    /// shader's `u_*` uniform name (matches keys in the renderer's
    pub user_property_overrides: Option<String>,
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
    #[sea_orm(has_many = "super::playlist_item::Entity")]
    PlaylistItem,
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

impl Related<super::playlist_item::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::PlaylistItem.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
