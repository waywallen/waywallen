use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE wallpaper_property_preset (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    library_id INTEGER NOT NULL REFERENCES library(id) ON DELETE CASCADE,
                    item_path TEXT NOT NULL,
                    name TEXT NOT NULL,
                    user_property_overrides TEXT NOT NULL,
                    UNIQUE (library_id, item_path, name)
                )",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE wallpaper_property_preset")
            .await?;
        Ok(())
    }
}
