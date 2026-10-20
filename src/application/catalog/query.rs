use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;

use crate::catalog::entry::WallpaperEntry;
use crate::catalog::{FilterLogic, FilterRule, SortRule};
use crate::model::repo;
use crate::DaemonContext;

/// Resolve the user-visible ordered list of entry ids: DB entries →
/// filter → sort. Mirrors the WallpaperList pipeline so D-Bus
pub async fn ordered_entry_ids(
    app: &Arc<DaemonContext>,
    filters: &[FilterRule],
    logics: &[FilterLogic],
    sorts: &[SortRule],
) -> Result<Vec<String>> {
    let all = repo::load_entries(&app.db).await?;

    let matched_keys: Option<HashSet<(String, String)>> = if filters.is_empty() {
        None
    } else {
        Some(
            repo::list_item_keys_by_wallpaper_filters(&app.db, filters, logics)
                .await?
                .into_iter()
                .collect(),
        )
    };

    let mut filtered: Vec<&WallpaperEntry> = if let Some(keys) = matched_keys.as_ref() {
        all.iter()
            .filter(|e| {
                crate::catalog::path::relative_under_root(&e.library_root, &e.resource)
                    .map(|rel| keys.contains(&(e.library_root.clone(), rel)))
                    .unwrap_or(false)
            })
            .collect()
    } else {
        all.iter().collect()
    };

    if !sorts.is_empty() {
        crate::catalog::query::sort_entries(&mut filtered, sorts);
    }

    Ok(filtered
        .into_iter()
        .map(|e| e.item_id.to_string())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, modified_at: Option<i64>, create_at: i64) -> WallpaperEntry {
        WallpaperEntry {
            item_id: 0,
            name: name.to_string(),
            wp_type: "image".to_string(),
            resource: name.to_string(),
            preview: None,
            description: None,
            tags: Vec::new(),
            external_id: None,
            web_url: None,
            update_available: false,
            size: None,
            width: None,
            height: None,
            content_rating: None,
            modified_at,
            create_at,
            plugin_name: String::new(),
            library_root: String::new(),
            hidden: false,
        }
    }

    #[test]
    fn last_modified_sort_falls_back_to_create_at() {
        let newer_created = entry("created-newer", None, 30);
        let older_modified = entry("modified-older", Some(20), 40);
        let older_created = entry("created-older", None, 10);
        let mut entries = vec![&newer_created, &older_modified, &older_created];

        crate::catalog::query::sort_entries(
            &mut entries,
            &[SortRule {
                key: crate::catalog::query::SortKey::LastModified,
                direction: crate::catalog::query::SortDirection::Ascending,
            }],
        );

        let names: Vec<_> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["created-older", "modified-older", "created-newer"]
        );
    }
}
