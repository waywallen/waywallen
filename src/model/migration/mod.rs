use sea_orm_migration::prelude::*;

mod m20260419_000001_init;
mod m20260425_000001_item_media_meta;
mod m20260425_000002_item_timestamps;
mod m20260425_000003_item_probed_at;
mod m20260425_000004_playlist;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260419_000001_init::Migration),
            Box::new(m20260425_000001_item_media_meta::Migration),
            Box::new(m20260425_000002_item_timestamps::Migration),
            Box::new(m20260425_000003_item_probed_at::Migration),
            Box::new(m20260425_000004_playlist::Migration),
        ]
    }
}
