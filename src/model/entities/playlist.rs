//! `playlist` table — curated lists and smart filters share one row
//! type. `source_kind` discriminates: `'curated'` rows have explicit
//! members in `playlist_item`; `'smart'` rows store the predicate in
//! `filter_json` and derive their members at runtime.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "playlist")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    #[sea_orm(unique)]
    pub name: String,
    /// `'curated'` or `'smart'`. CHECK constraint enforced in SQL.
    pub source_kind: String,
    /// JSON-encoded `playlist::Filter`; non-null iff `source_kind = 'smart'`.
    pub filter_json: Option<String>,
    /// `'sequential'` | `'shuffle'` | `'random'`. CHECK enforced in SQL.
    pub mode: String,
    /// Auto-rotation period; `0` disables rotation.
    pub interval_secs: i32,
    /// Stable RNG seed for shuffle reshuffles. Persisted so a restart
    /// resumes the same shuffle sequence rather than re-randomizing.
    pub shuffle_seed: i64,
    pub create_at: i64,
    pub update_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::playlist_item::Entity")]
    PlaylistItem,
}

impl Related<super::playlist_item::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::PlaylistItem.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
