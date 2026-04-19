use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(SourcePlugin::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SourcePlugin::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(SourcePlugin::Name)
                            .text()
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(SourcePlugin::Version)
                            .text()
                            .not_null()
                            .default(""),
                    )
                    .to_owned(),
            )
            .await?;

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
                    .col(ColumnDef::new(Library::PluginId).big_integer().not_null())
                    .col(ColumnDef::new(Library::Path).text().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_library_plugin")
                            .from(Library::Table, Library::PluginId)
                            .to(SourcePlugin::Table, SourcePlugin::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_library_plugin_path")
                    .table(Library::Table)
                    .col(Library::PluginId)
                    .col(Library::Path)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_library_plugin")
                    .table(Library::Table)
                    .col(Library::PluginId)
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
                    .col(ColumnDef::new(Item::PluginId).big_integer().not_null())
                    .col(ColumnDef::new(Item::LibraryId).big_integer().not_null())
                    .col(ColumnDef::new(Item::Path).text().not_null())
                    .col(ColumnDef::new(Item::Type).text().not_null())
                    .col(
                        ColumnDef::new(Item::DisplayName)
                            .text()
                            .not_null()
                            .default(""),
                    )
                    .col(ColumnDef::new(Item::PreviewPath).text().null())
                    .col(ColumnDef::new(Item::Description).text().null())
                    .col(ColumnDef::new(Item::ExternalId).text().null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_item_plugin")
                            .from(Item::Table, Item::PluginId)
                            .to(SourcePlugin::Table, SourcePlugin::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
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
                    .col(Item::Path)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_item_plugin")
                    .table(Item::Table)
                    .col(Item::PluginId)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_item_external_id")
                    .table(Item::Table)
                    .col(Item::ExternalId)
                    .to_owned(),
            )
            .await?;

        // `tag.name` needs case-insensitive uniqueness so "Anime" and
        // "anime" collapse. SeaORM's ColumnDef has no collation knob
        // across all backends, so we emit the SQLite DDL verbatim.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE IF NOT EXISTS tag (\
                   id INTEGER PRIMARY KEY AUTOINCREMENT,\
                   name TEXT NOT NULL UNIQUE COLLATE NOCASE\
                 )",
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ItemTag::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(ItemTag::ItemId).big_integer().not_null())
                    .col(ColumnDef::new(ItemTag::TagId).big_integer().not_null())
                    .primary_key(
                        Index::create()
                            .col(ItemTag::ItemId)
                            .col(ItemTag::TagId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_item_tag_item")
                            .from(ItemTag::Table, ItemTag::ItemId)
                            .to(Item::Table, Item::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_item_tag_tag")
                            .from(ItemTag::Table, ItemTag::TagId)
                            .to(Tag::Table, Tag::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_item_tag_tag")
                    .table(ItemTag::Table)
                    .col(ItemTag::TagId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ItemTag::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Tag::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Item::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Library::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(SourcePlugin::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum SourcePlugin {
    Table,
    Id,
    Name,
    Version,
}

#[derive(DeriveIden)]
enum Library {
    Table,
    Id,
    PluginId,
    Path,
}

#[derive(DeriveIden)]
enum Item {
    Table,
    Id,
    PluginId,
    LibraryId,
    Path,
    Type,
    DisplayName,
    PreviewPath,
    Description,
    ExternalId,
}

#[derive(DeriveIden)]
enum Tag {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum ItemTag {
    Table,
    ItemId,
    TagId,
}
