use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Library::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Library::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Library::Path)
                            .text()
                            .not_null()
                            .unique_key(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Item::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Item::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Item::LibraryId).big_integer().not_null())
                    .col(ColumnDef::new(Item::RelativePath).text().not_null())
                    .col(ColumnDef::new(Item::Type).text().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_item_library")
                            .from(Item::Table, Item::LibraryId)
                            .to(Library::Table, Library::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_item_library_path")
                    .table(Item::Table)
                    .col(Item::LibraryId)
                    .col(Item::RelativePath)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_item_library")
                    .table(Item::Table)
                    .col(Item::LibraryId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Item::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Library::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Library {
    Table,
    Id,
    Path,
}

#[derive(DeriveIden)]
enum Item {
    Table,
    Id,
    LibraryId,
    RelativePath,
    Type,
}
