//! `playlist` + `playlist_item` tables.
//!
//! Two membership modes share one table:
//! - `source_kind = 'curated'`: members are explicit rows in
//!   `playlist_item`. `filter_json` is NULL.
//! - `source_kind = 'smart'`: members are derived from `filter_json`
//!   at runtime; `playlist_item` is empty for these rows.
//!
//! `mode` is the rotation/cursor mode: sequential, shuffle, random.
//! `interval_secs = 0` disables auto-rotation.
//!
//! Raw SQL is used for `playlist` so we can emit `COLLATE NOCASE` on
//! `name` (matches the `tag.name` convention) and `CHECK` constraints
//! on the two enum-like text columns in one shot.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE IF NOT EXISTS playlist (\
                   id INTEGER PRIMARY KEY AUTOINCREMENT,\
                   name TEXT NOT NULL UNIQUE COLLATE NOCASE,\
                   source_kind TEXT NOT NULL DEFAULT 'curated' \
                     CHECK(source_kind IN ('curated','smart')),\
                   filter_json TEXT NULL,\
                   mode TEXT NOT NULL DEFAULT 'sequential' \
                     CHECK(mode IN ('sequential','shuffle','random')),\
                   interval_secs INTEGER NOT NULL DEFAULT 0,\
                   shuffle_seed BIGINT NOT NULL DEFAULT 0,\
                   create_at BIGINT NOT NULL,\
                   update_at BIGINT NOT NULL\
                 )",
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(PlaylistItem::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PlaylistItem::PlaylistId)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PlaylistItem::ItemId)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PlaylistItem::Position)
                            .integer()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(PlaylistItem::PlaylistId)
                            .col(PlaylistItem::Position),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_playlist_item_playlist")
                            .from(PlaylistItem::Table, PlaylistItem::PlaylistId)
                            .to(Playlist::Table, Playlist::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_playlist_item_item")
                            .from(PlaylistItem::Table, PlaylistItem::ItemId)
                            .to(Item::Table, Item::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_playlist_item_unique")
                    .table(PlaylistItem::Table)
                    .col(PlaylistItem::PlaylistId)
                    .col(PlaylistItem::ItemId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_playlist_item_item")
                    .table(PlaylistItem::Table)
                    .col(PlaylistItem::ItemId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PlaylistItem::Table).to_owned())
            .await?;
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS playlist")
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Playlist {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum PlaylistItem {
    Table,
    PlaylistId,
    ItemId,
    Position,
}

#[derive(DeriveIden)]
enum Item {
    Table,
    Id,
}
