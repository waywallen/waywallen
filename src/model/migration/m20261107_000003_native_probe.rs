use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for sql in [
            "ALTER TABLE item RENAME COLUMN create_at TO added_at",
            "ALTER TABLE item RENAME COLUMN modified_at TO file_mtime_ns",
            "UPDATE item SET file_mtime_ns = CASE WHEN file_mtime_ns BETWEEN -9223372036854 AND 9223372036854 THEN file_mtime_ns * 1000000 ELSE NULL END",
            "ALTER TABLE item ADD COLUMN seen_scan_id INTEGER NOT NULL DEFAULT 0",
            "CREATE TABLE scan_sequence (id INTEGER PRIMARY KEY CHECK (id = 1), value INTEGER NOT NULL)",
            "INSERT INTO scan_sequence VALUES (1, 0)",
            "CREATE TABLE item_probe (item_id INTEGER PRIMARY KEY REFERENCES item(id) ON DELETE CASCADE, source_fields INTEGER NOT NULL DEFAULT 0, input_kind TEXT NOT NULL DEFAULT '', signature TEXT, status TEXT, attempted_at INTEGER, selected_path TEXT, width INTEGER, height INTEGER, size INTEGER, error TEXT, dependencies TEXT NOT NULL DEFAULT '[]')",
            "INSERT INTO item_probe (item_id, source_fields, attempted_at) SELECT id, (CASE WHEN size IS NOT NULL THEN 1 ELSE 0 END) | (CASE WHEN width > 0 THEN 2 ELSE 0 END) | (CASE WHEN height > 0 THEN 4 ELSE 0 END), probed_at FROM item",
            "UPDATE item SET width = NULL WHERE width <= 0",
            "UPDATE item SET height = NULL WHERE height <= 0",
            "ALTER TABLE item DROP COLUMN update_at",
            "ALTER TABLE item DROP COLUMN sync_at",
            "ALTER TABLE item DROP COLUMN stat_at",
            "ALTER TABLE item DROP COLUMN probed_at",
        ] {
            db.execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for sql in [
            "ALTER TABLE item ADD COLUMN update_at INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE item ADD COLUMN sync_at INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE item ADD COLUMN stat_at INTEGER",
            "ALTER TABLE item ADD COLUMN probed_at INTEGER",
            "UPDATE item SET probed_at = (SELECT attempted_at FROM item_probe WHERE item_id = item.id)",
            "DROP TABLE item_probe",
            "DROP TABLE scan_sequence",
            "ALTER TABLE item DROP COLUMN seen_scan_id",
            "UPDATE item SET file_mtime_ns = file_mtime_ns / 1000000",
            "ALTER TABLE item RENAME COLUMN file_mtime_ns TO modified_at",
            "ALTER TABLE item RENAME COLUMN added_at TO create_at",
        ] {
            manager.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }
}
