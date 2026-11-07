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
                    .add_column(ColumnDef::new(Item::SubType).text().not_null().default(""))
                    .to_owned(),
            )
            .await
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Item::Table)
                    .drop_column(Item::SubType)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Item {
    Table,
    SubType,
}
