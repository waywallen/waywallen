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
    pub sub_type: String,
    pub display_name: String,
    pub preview_path: Option<String>,
    pub description: Option<String>,
    pub external_id: Option<String>,
    pub web_url: Option<String>,
    pub update_available: bool,
    pub size: Option<i64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub content_rating: Option<String>,
    pub can_remove: Option<bool>,
    /// First insertion time, Unix milliseconds.
    pub added_at: i64,
    pub seen_scan_id: i64,
    /// Resource file mtime, Unix nanoseconds.
    pub file_mtime_ns: Option<i64>,
    /// JSON map of per-item user-property overrides. Each key is the
    /// shader's `u_*` uniform name (matches keys in the renderer's
    pub user_property_overrides: Option<String>,
    /// JSON map of daemon-owned per-wallpaper display layout override.
    /// This is intentionally separate from renderer user properties.
    pub wallpaper_layout_override: Option<String>,
    /// When true, the library browser hides this item unless the
    /// show-hidden filter is enabled.
    pub hidden: bool,
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
