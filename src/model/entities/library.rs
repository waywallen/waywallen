use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "library")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub plugin_id: i64,
    pub path: String,
    pub name: String,
    pub settings: String,
    /// JSON-encoded `{string -> string}` map. Free-form per-plugin
    /// scratch space exposed to source plugins via `ctx.library_meta_*`.
    pub metadata: String,
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
