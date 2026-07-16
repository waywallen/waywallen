use sea_orm_migration::prelude::*;

mod m20260503_000001_init_v2;
mod m20260523_000001_user_property_overrides;
mod m20260601_000001_playlists;
mod m20260614_000001_wallpaper_layout_override;
mod m20260715_000001_drop_wallpaper_layout_override;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260503_000001_init_v2::Migration),
            Box::new(m20260523_000001_user_property_overrides::Migration),
            Box::new(m20260601_000001_playlists::Migration),
            Box::new(m20260614_000001_wallpaper_layout_override::Migration),
            Box::new(m20260715_000001_drop_wallpaper_layout_override::Migration),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ConnectionTrait, Statement};

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
    async fn item_wallpaper_layout_override_column_dropped_after_migration() {
        let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
        let stmt = Statement::from_string(
            db.get_database_backend(),
            "SELECT wallpaper_layout_override FROM item LIMIT 0".to_string(),
        );
        assert!(
            db.execute(stmt).await.is_err(),
            "item.wallpaper_layout_override should be dropped by the migration"
        );
    }
}
