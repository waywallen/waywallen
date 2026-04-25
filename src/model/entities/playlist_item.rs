//! `playlist_item` — ordered M-to-M between curated playlists and items.
//!
//! Composite primary key `(playlist_id, position)` keeps the order
//! authoritative. A separate `(playlist_id, item_id)` unique index
//! prevents the same wallpaper from appearing twice in one playlist.
//! Smart playlists do not write rows here.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "playlist_item")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub playlist_id: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    pub position: i32,
    pub item_id: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::playlist::Entity",
        from = "Column::PlaylistId",
        to = "super::playlist::Column::Id",
        on_delete = "Cascade"
    )]
    Playlist,
    #[sea_orm(
        belongs_to = "super::item::Entity",
        from = "Column::ItemId",
        to = "super::item::Column::Id",
        on_delete = "Cascade"
    )]
    Item,
}

impl Related<super::playlist::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Playlist.def()
    }
}

impl Related<super::item::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Item.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
