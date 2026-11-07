use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Library::Table)
                    .add_column(
                        ColumnDef::new(Library::Settings)
                            .text()
                            .not_null()
                            .default("{}"),
                    )
                    .to_owned(),
            )
            .await
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Library::Table)
                    .drop_column(Library::Settings)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Library {
    Table,
    Settings,
}
