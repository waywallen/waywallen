//! `library` table — a per-plugin logical group of items. `path` is a
//! plugin-scoped key: a filesystem path for folder-style plugins, or a
//! synthetic label for snapshot-style plugins.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "library")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub plugin_id: i64,
    pub path: String,
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
    #[sea_orm(has_many = "super::item::Entity")]
    Item,
}

impl Related<super::source_plugin::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::SourcePlugin.def()
    }
}

impl Related<super::item::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Item.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
