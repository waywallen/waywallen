//! `library` table — a user-added wallpaper root directory.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "library")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    #[sea_orm(unique)]
    pub path: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::item::Entity")]
    Item,
}

impl Related<super::item::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Item.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
