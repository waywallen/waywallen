use std::time::{SystemTime, UNIX_EPOCH};

use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
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
                    .add_column(
                        ColumnDef::new(Item::CreateAt)
                            .big_integer()
                            .not_null()
                            .default(0i64),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Item::Table)
                    .add_column(
                        ColumnDef::new(Item::UpdateAt)
                            .big_integer()
                            .not_null()
                            .default(0i64),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Item::Table)
                    .add_column(
                        ColumnDef::new(Item::SyncAt)
                            .big_integer()
                            .not_null()
                            .default(0i64),
                    )
                    .to_owned(),
            )
            .await?;

        // Backfill any pre-existing rows with the current timestamp so
        // they don't read as "epoch=0" (would falsely look ancient to
        // probe cooldown logic).
        let now = now_ms();
        manager
            .get_connection()
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "UPDATE item SET create_at = ?, update_at = ?, sync_at = ? \
                 WHERE create_at = 0 AND update_at = 0 AND sync_at = 0",
                [now.into(), now.into(), now.into()],
            ))
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Item::Table)
                    .drop_column(Item::SyncAt)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Item::Table)
                    .drop_column(Item::UpdateAt)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Item::Table)
                    .drop_column(Item::CreateAt)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(DeriveIden)]
enum Item {
    Table,
    CreateAt,
    UpdateAt,
    SyncAt,
}
