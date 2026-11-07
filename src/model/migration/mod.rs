use sea_orm_migration::prelude::*;

mod m20260503_000001_init_v2;
mod m20260523_000001_user_property_overrides;
mod m20260601_000001_playlists;
mod m20260614_000001_wallpaper_layout_override;
mod m20260816_000001_item_web_url;
mod m20260825_000001_playlist_synchronized_selection;
mod m20260929_000001_item_hidden;
mod m20261015_000001_smart_playlists;
mod m20261015_000002_library_name;
mod m20261019_000001_item_update_available;
mod m20261024_000001_item_can_remove;
mod m20261107_000001_item_sub_type;
mod m20261107_000002_library_settings;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260503_000001_init_v2::Migration),
            Box::new(m20260523_000001_user_property_overrides::Migration),
            Box::new(m20260601_000001_playlists::Migration),
            Box::new(m20260614_000001_wallpaper_layout_override::Migration),
            Box::new(m20260816_000001_item_web_url::Migration),
            Box::new(m20260825_000001_playlist_synchronized_selection::Migration),
            Box::new(m20260929_000001_item_hidden::Migration),
            Box::new(m20261015_000001_smart_playlists::Migration),
            Box::new(m20261015_000002_library_name::Migration),
            Box::new(m20261019_000001_item_update_available::Migration),
            Box::new(m20261024_000001_item_can_remove::Migration),
            Box::new(m20261107_000001_item_sub_type::Migration),
            Box::new(m20261107_000002_library_settings::Migration),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ConnectionTrait, Statement};

    async fn downgrade_before(db: &sea_orm::DatabaseConnection, migration: impl MigrationTrait) {
        let migrations = Migrator::migrations();
        let position = migrations
            .iter()
            .position(|current| current.name() == migration.name())
            .unwrap();
        Migrator::down(db, Some((migrations.len() - position) as u32))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn library_names_migrate_existing_rows() {
        let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
        downgrade_before(&db, m20261015_000002_library_name::Migration).await;
        db.execute_unprepared("INSERT INTO source_plugin (id, name) VALUES (1, 'image')")
            .await
            .unwrap();
        db.execute_unprepared("INSERT INTO library (id, plugin_id, path, metadata) VALUES (1, 1, '/a/Wallpapers/', '{\"keep\":\"value\"}'), (2, 1, '/b/Wallpapers', '{}'), (3, 1, '/c/Wallpapers', '{}')").await.unwrap();
        Migrator::up(&db, None).await.unwrap();
        let libraries = crate::model::repo::list_libraries(&db).await.unwrap();
        assert_eq!(
            libraries
                .iter()
                .map(|lib| lib.name.as_str())
                .collect::<Vec<_>>(),
            ["Wallpapers", "Wallpapers-1", "Wallpapers-2"]
        );
        assert_eq!(libraries[0].id, 1);
        assert_eq!(libraries[0].path, "/a/Wallpapers/");
        assert_eq!(libraries[0].metadata, "{\"keep\":\"value\"}");
        crate::model::repo::rename_library(&db, 2, "My wallpapers")
            .await
            .unwrap();
        Migrator::up(&db, None).await.unwrap();
        assert_eq!(
            crate::model::repo::list_libraries(&db).await.unwrap()[1].name,
            "My wallpapers"
        );
    }

    #[tokio::test]
    async fn item_can_remove_migrates_existing_items_as_unknown() {
        let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
        downgrade_before(&db, m20261024_000001_item_can_remove::Migration).await;
        db.execute_unprepared("INSERT INTO source_plugin (id, name) VALUES (1, 'image')")
            .await
            .unwrap();
        db.execute_unprepared(
            "INSERT INTO library (id, plugin_id, path) VALUES (1, 1, '/backgrounds')",
        )
        .await
        .unwrap();
        db.execute_unprepared("INSERT INTO item (id, plugin_id, library_id, path, type) VALUES (1, 1, 1, 'one.png', 'image')").await.unwrap();
        Migrator::up(&db, None).await.unwrap();
        let entry = crate::model::repo::get_entry(&db, 1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(entry.can_remove, None);
        assert_eq!(entry.resource, "/backgrounds/one.png");
    }

    #[tokio::test]
    async fn playlist_tables_exist_after_migration() {
        let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
        for table in ["playlist", "playlist_item"] {
            let stmt = Statement::from_string(
                db.get_database_backend(),
                format!("SELECT count(*) FROM {table}"),
            );
            db.execute(stmt)
                .await
                .unwrap_or_else(|e| panic!("table {table} missing: {e}"));
        }
    }

    #[tokio::test]
    async fn item_wallpaper_layout_override_column_exists_after_migration() {
        let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
        let stmt = Statement::from_string(
            db.get_database_backend(),
            "SELECT wallpaper_layout_override FROM item LIMIT 0".to_string(),
        );
        db.execute(stmt)
            .await
            .unwrap_or_else(|e| panic!("item.wallpaper_layout_override missing: {e}"));
    }

    #[tokio::test]
    async fn item_web_url_column_exists_after_migration() {
        let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
        let stmt = Statement::from_string(
            db.get_database_backend(),
            "SELECT web_url FROM item LIMIT 0".to_string(),
        );
        db.execute(stmt)
            .await
            .unwrap_or_else(|e| panic!("item.web_url missing: {e}"));
    }

    #[tokio::test]
    async fn playlist_synchronized_selection_column_exists_after_migration() {
        let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
        let stmt = Statement::from_string(
            db.get_database_backend(),
            "SELECT synchronized_selection FROM playlist LIMIT 0".to_string(),
        );
        db.execute(stmt)
            .await
            .unwrap_or_else(|e| panic!("playlist.synchronized_selection missing: {e}"));
    }

    #[tokio::test]
    async fn item_hidden_column_exists_after_migration() {
        let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
        let stmt = Statement::from_string(
            db.get_database_backend(),
            "SELECT hidden FROM item LIMIT 0".to_string(),
        );
        db.execute(stmt)
            .await
            .unwrap_or_else(|e| panic!("item.hidden missing: {e}"));
    }
}
