use sea_orm::{DatabaseConnection, TransactionTrait};

use crate::catalog::{entry::WallpaperEntry, query::WallpaperSelection};
use crate::error::{Result, ResultExt};
use crate::model::repo;

pub struct InvalidateOnDrop(pub tokio::sync::watch::Sender<u64>);

impl Drop for InvalidateOnDrop {
    fn drop(&mut self) {
        self.0.send_modify(|generation| *generation += 1);
    }
}

pub struct SelectionResult {
    pub entries: Vec<WallpaperEntry>,
    pub total: u32,
}

pub async fn query_wallpapers(
    db: &DatabaseConnection,
    selection: &WallpaperSelection,
    page: u32,
    page_size: u32,
) -> Result<SelectionResult> {
    selection.validate()?;
    let txn = db.begin().await.context("begin wallpaper selection")?;
    let mut entries = repo::select_wallpapers(&txn, selection).await?;
    let total = entries.len() as u32;
    if page_size != 0 {
        entries = entries
            .into_iter()
            .skip((page as usize).saturating_mul(page_size as usize))
            .take(page_size as usize)
            .collect();
    }
    repo::populate_entry_tags(&txn, &mut entries).await?;
    txn.commit().await.context("commit wallpaper selection")?;
    Ok(SelectionResult { entries, total })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::query::*;

    async fn seed_libraries() -> (DatabaseConnection, Vec<(i64, i64)>) {
        let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
        let mut ids = Vec::new();
        for (plugin, name) in [("a", "match"), ("b", "other")] {
            let plugin = repo::upsert_plugin(&db, plugin, "").await.unwrap();
            let lib = repo::add_library(&db, plugin.id, "/same").await.unwrap();
            let item = repo::upsert_item(
                &db,
                repo::ItemUpsertArgs {
                    plugin_id: plugin.id,
                    library_id: lib.id,
                    path: "same.png",
                    ty: "image",
                    display_name: name,
                    preview_path: None,
                    description: None,
                    external_id: None,
                    web_url: None,
                    update_available: false,
                    size: None,
                    width: None,
                    height: None,
                    content_rating: None,
                    can_remove: None,
                },
            )
            .await
            .unwrap();
            ids.push((item.id, lib.id));
        }
        (db, ids)
    }

    #[tokio::test]
    async fn wallpaper_selection_library_rules_match_ids() {
        let (db, ids) = seed_libraries().await;
        let rule = |id, exclude, group| FilterRule {
            group,
            predicate: FilterPredicate::Library { value: id, exclude },
        };
        let mut selection = WallpaperSelection::default();
        selection.filters = vec![rule(ids[0].1, false, 0)];
        let result = query_wallpapers(&db, &selection, 0, 0).await.unwrap();
        assert_eq!(result.total, 1);
        assert_eq!(result.entries[0].item_id, ids[0].0);

        selection.filters = vec![rule(ids[0].1, true, 0)];
        let result = query_wallpapers(&db, &selection, 0, 0).await.unwrap();
        assert_eq!(result.total, 1);
        assert_eq!(result.entries[0].item_id, ids[1].0);

        selection.filters = vec![rule(ids[0].1, false, 0), rule(ids[1].1, false, 1)];
        selection.logics = vec![FilterLogic {
            operator: LogicOperator::Or,
            group_a: 0,
            group_b: 1,
        }];
        assert_eq!(
            query_wallpapers(&db, &selection, 0, 0).await.unwrap().total,
            2
        );
        selection.hidden_library_ids = vec![ids[0].1];
        assert_eq!(
            query_wallpapers(&db, &selection, 0, 0).await.unwrap().total,
            1
        );

        selection.hidden_library_ids.clear();
        selection.logics.clear();
        selection.filters = vec![rule(ids[0].1, false, 0), rule(ids[0].1, true, 0)];
        assert_eq!(
            query_wallpapers(&db, &selection, 0, 0).await.unwrap().total,
            0
        );

        repo::remove_library(&db, ids[0].1).await.unwrap();
        selection.filters = vec![rule(ids[0].1, false, 0)];
        assert_eq!(
            query_wallpapers(&db, &selection, 0, 0).await.unwrap().total,
            0
        );
        selection.filters = vec![rule(ids[0].1, true, 0)];
        assert_eq!(
            query_wallpapers(&db, &selection, 0, 0).await.unwrap().total,
            1
        );

        for id in [0, -1] {
            selection.filters = vec![rule(id, false, 0)];
            assert!(query_wallpapers(&db, &selection, 0, 0).await.is_err());
        }
    }

    #[tokio::test]
    async fn wallpaper_selection_preserves_identity_and_pagination() {
        let (db, ids) = seed_libraries().await;
        let mut selection = WallpaperSelection::default();
        selection.filters.push(FilterRule {
            group: 0,
            predicate: FilterPredicate::Name {
                value: "match".into(),
                condition: StringMatch::Is,
            },
        });
        let found = query_wallpapers(&db, &selection, 0, 1).await.unwrap();
        assert_eq!(found.total, 1);
        assert_eq!(found.entries[0].item_id, ids[0].0);
        let next = query_wallpapers(&db, &selection, 1, 1).await.unwrap();
        assert_eq!(next.total, 1);
        assert!(next.entries.is_empty());
        selection.hidden_library_ids = vec![ids[0].1];
        assert_eq!(
            query_wallpapers(&db, &selection, 0, 0).await.unwrap().total,
            0
        );
        selection.filters.clear();
        assert_eq!(
            query_wallpapers(&db, &selection, 0, 0)
                .await
                .unwrap()
                .entries[0]
                .item_id,
            ids[1].0
        );
        selection.logics.push(FilterLogic {
            operator: LogicOperator::Or,
            group_a: 0,
            group_b: 1,
        });
        assert!(query_wallpapers(&db, &selection, 0, 0).await.is_err());
    }
}
