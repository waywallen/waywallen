//! Add `item.probed_at` so the background probe scheduler can tell
//! "never been probed" apart from "just bumped by sync". Without this
//! the cooldown filter (`sync_at < now - cooldown`) excludes every
//! freshly-synced row — exactly the rows the post-refresh drain is
//! supposed to handle.
//!
//! `probed_at` is `NULL` for rows that have never been probed; the
//! probe task always stamps it (success or no-op) so re-tries respect
//! the cooldown window.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Item::Table)
                    .add_column(ColumnDef::new(Item::ProbedAt).big_integer().null())
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Item::Table)
                    .drop_column(Item::ProbedAt)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Item {
    Table,
    ProbedAt,
}
