use super::*;

// item

/// Payload for [`upsert_item`]. `path` / `preview_path` are both
/// relative to `library.path` — callers own the stripping.
pub struct ItemUpsertArgs<'a> {
    pub plugin_id: i64,
    pub library_id: i64,
    pub path: &'a str,
    /// Stored lowercase by [`upsert_item`] so `"Scene"` and `"scene"`
    /// don't split on reads.
    pub ty: &'a str,
    pub sub_type: &'a str,
    pub display_name: &'a str,
    pub preview_path: Option<&'a str>,
    pub description: Option<&'a str>,
    pub external_id: Option<&'a str>,
    pub web_url: Option<&'a str>,
    pub update_available: bool,
    pub size: Option<i64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub content_rating: Option<&'a str>,
    pub can_remove: Option<bool>,
}

/// Upsert an item keyed by `(library_id, path)`. Every non-key column
/// (except `added_at`) is refreshed on conflict — new scan is truth.
pub async fn upsert_item(
    db: &impl ConnectionTrait,
    args: ItemUpsertArgs<'_>,
) -> Result<item::Model> {
    let ty_norm = args.ty.to_lowercase();
    let size = args.size.filter(|n| *n >= 0);
    let width = args.width.filter(|n| *n > 0);
    let height = args.height.filter(|n| *n > 0);
    let now = now_ms();
    let am = item::ActiveModel {
        plugin_id: Set(args.plugin_id),
        library_id: Set(args.library_id),
        path: Set(args.path.to_owned()),
        ty: Set(ty_norm),
        sub_type: Set(args.sub_type.to_owned()),
        display_name: Set(args.display_name.to_owned()),
        preview_path: Set(args.preview_path.map(str::to_owned)),
        description: Set(args.description.map(str::to_owned)),
        external_id: Set(args.external_id.map(str::to_owned)),
        web_url: Set(args.web_url.map(str::to_owned)),
        update_available: Set(args.update_available),
        size: Set(size),
        width: Set(width),
        height: Set(height),
        content_rating: Set(args.content_rating.map(str::to_owned)),
        can_remove: Set(args.can_remove),
        added_at: Set(now),
        seen_scan_id: Set(0),
        ..Default::default()
    };
    item::Entity::insert(am)
        .on_conflict(
            // The first-insert timestamp survives subsequent scans.
            OnConflict::columns([item::Column::LibraryId, item::Column::Path])
                .update_columns([
                    item::Column::Ty,
                    item::Column::SubType,
                    item::Column::PluginId,
                    item::Column::DisplayName,
                    item::Column::PreviewPath,
                    item::Column::Description,
                    item::Column::ExternalId,
                    item::Column::WebUrl,
                    item::Column::UpdateAvailable,
                    item::Column::CanRemove,
                ])
                .value(
                    item::Column::Size,
                    Expr::cust("COALESCE(excluded.size, size)"),
                )
                .value(
                    item::Column::Width,
                    Expr::cust("COALESCE(excluded.width, width)"),
                )
                .value(
                    item::Column::Height,
                    Expr::cust("COALESCE(excluded.height, height)"),
                )
                .value(
                    item::Column::ContentRating,
                    Expr::cust("COALESCE(excluded.content_rating, content_rating)"),
                )
                .to_owned(),
        )
        .exec(db)
        .await
        .with_context(|| format!("upsert item lib={} path={}", args.library_id, args.path))?;
    let item = item::Entity::find()
        .filter(item::Column::LibraryId.eq(args.library_id))
        .filter(item::Column::Path.eq(args.path))
        .one(db)
        .await
        .with_context(|| format!("reload item lib={} path={}", args.library_id, args.path))?
        .ok_or_else(|| Error::Internal(anyhow!("reloaded item missing after upsert")))?;
    let fields = i32::from(size.is_some())
        | (i32::from(width.is_some()) << 1)
        | (i32::from(height.is_some()) << 2);
    set_source_media_fields(db, item.id, fields).await?;
    Ok(item)
}

pub async fn list_items_by_library(
    db: &DatabaseConnection,
    library_id: i64,
) -> Result<Vec<item::Model>> {
    item::Entity::find()
        .filter(item::Column::LibraryId.eq(library_id))
        .order_by_asc(item::Column::Path)
        .all(db)
        .await
        .with_context(|| format!("select items lib={library_id}"))
}

pub async fn list_items_all(db: &DatabaseConnection) -> Result<Vec<item::Model>> {
    item::Entity::find()
        .order_by_asc(item::Column::LibraryId)
        .order_by_asc(item::Column::Path)
        .all(db)
        .await
        .context("select all items")
}

/// Reconstruct a `WallpaperEntry` from a DB `item` row + its owning
/// library path and plugin name. `item.path`/`preview_path` are stored
fn entry_from_item(
    it: item::Model,
    library_path: &str,
    plugin_name: &str,
) -> crate::catalog::entry::WallpaperEntry {
    use std::path::Path;
    let resource = Path::new(library_path)
        .join(&it.path)
        .to_string_lossy()
        .into_owned();
    let preview = it.preview_path.as_deref().map(|path| {
        Path::new(library_path)
            .join(path)
            .to_string_lossy()
            .into_owned()
    });
    crate::catalog::entry::WallpaperEntry {
        item_id: it.id,
        name: it.display_name,
        wp_type: it.ty,
        sub_type: it.sub_type,
        resource,
        preview,
        description: it.description,
        tags: Vec::new(),
        external_id: it.external_id,
        web_url: it.web_url,
        update_available: it.update_available,
        size: it.size,
        width: it.width.map(|v| v as u32),
        height: it.height.map(|v| v as u32),
        content_rating: it.content_rating,
        can_remove: it.can_remove,
        file_mtime_ns: it.file_mtime_ns,
        added_at: it.added_at,
        plugin_name: plugin_name.to_string(),
        library_root: library_path.to_string(),
        hidden: it.hidden,
    }
}

/// All items as fully-populated `WallpaperEntry` values, rebuilt from
/// the DB (the read source of truth). Stable `(library_id, path)` order.
pub async fn load_entries(
    db: &DatabaseConnection,
) -> Result<Vec<crate::catalog::entry::WallpaperEntry>> {
    load_entries_excluding_libraries(db, &[]).await
}

/// Empty IDs hide nothing; unknown positive IDs do not affect existing libraries.
pub async fn load_entries_excluding_libraries(
    db: &DatabaseConnection,
    hidden_library_ids: &[i64],
) -> Result<Vec<crate::catalog::entry::WallpaperEntry>> {
    let hidden_library_ids = normalize_library_ids(hidden_library_ids)?;
    let lib_path: HashMap<i64, String> = list_libraries(db)
        .await?
        .into_iter()
        .map(|l| (l.id, l.path))
        .collect();
    let plugin_name: HashMap<i64, String> = list_plugins(db)
        .await?
        .into_iter()
        .map(|p| (p.id, p.name))
        .collect();
    let mut query = item::Entity::find();
    if !hidden_library_ids.is_empty() {
        query = query.filter(item::Column::LibraryId.is_not_in(hidden_library_ids));
    }
    let items = query
        .order_by_asc(item::Column::LibraryId)
        .order_by_asc(item::Column::Path)
        .all(db)
        .await
        .context("select library items")?;
    Ok(items
        .into_iter()
        .filter_map(|it| {
            let lib = lib_path.get(&it.library_id)?;
            let plugin = plugin_name.get(&it.plugin_id).cloned().unwrap_or_default();
            Some(entry_from_item(it, lib, &plugin))
        })
        .collect())
}

/// A single item as a `WallpaperEntry` by DB id, with its tags filled.
pub async fn get_entry(
    db: &impl ConnectionTrait,
    item_id: i64,
) -> Result<Option<crate::catalog::entry::WallpaperEntry>> {
    Ok(get_entries_ordered(db, &[item_id]).await?.pop())
}

pub async fn get_entries_ordered<C: ConnectionTrait>(
    conn: &C,
    item_ids: &[i64],
) -> Result<Vec<crate::catalog::entry::WallpaperEntry>> {
    if item_ids.is_empty() {
        return Ok(Vec::new());
    }

    let rows: Vec<(item::Model, Option<library::Model>)> = item::Entity::find()
        .filter(item::Column::Id.is_in(item_ids.iter().copied()))
        .find_also_related(library::Entity)
        .all(conn)
        .await
        .context("select ordered items")?;
    let plugin_ids: HashSet<i64> = rows.iter().map(|(item, _)| item.plugin_id).collect();
    let plugin_names: HashMap<i64, String> = source_plugin::Entity::find()
        .filter(source_plugin::Column::Id.is_in(plugin_ids))
        .all(conn)
        .await
        .context("select ordered item plugins")?
        .into_iter()
        .map(|plugin| (plugin.id, plugin.name))
        .collect();
    let tag_rows: Vec<(item_tag::Model, Option<tag::Model>)> = item_tag::Entity::find()
        .find_also_related(tag::Entity)
        .filter(item_tag::Column::ItemId.is_in(item_ids.iter().copied()))
        .order_by_asc(tag::Column::Name)
        .all(conn)
        .await
        .context("select ordered item tags")?;
    let mut tags_by_item: HashMap<i64, Vec<String>> = HashMap::new();
    for (item_tag, tag) in tag_rows {
        if let Some(tag) = tag {
            tags_by_item
                .entry(item_tag.item_id)
                .or_default()
                .push(tag.name);
        }
    }

    let mut by_id = HashMap::with_capacity(rows.len());
    for (item, library) in rows {
        let Some(library) = library else {
            continue;
        };
        let plugin_name = plugin_names
            .get(&item.plugin_id)
            .cloned()
            .unwrap_or_default();
        let item_id = item.id;
        let mut entry = entry_from_item(item, &library.path, &plugin_name);
        entry.tags = tags_by_item.remove(&item_id).unwrap_or_default();
        by_id.insert(item_id, entry);
    }

    Ok(item_ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

pub async fn select_wallpapers<C: ConnectionTrait>(
    conn: &C,
    selection: &crate::catalog::query::WallpaperSelection,
) -> Result<Vec<crate::catalog::entry::WallpaperEntry>> {
    use crate::catalog::query::{FilterPredicate, FilterRule, HiddenFilter, StringMatch};
    selection.validate()?;
    let mut query = item::Entity::find().find_also_related(library::Entity);
    if let Some(condition) = filter::wallpaper_query_to_condition(
        &selection.filters,
        &selection.logics,
        &selection.search_text,
    ) {
        query = query.filter(condition);
    }
    if !selection.hidden_library_ids.is_empty() {
        query =
            query.filter(item::Column::LibraryId.is_not_in(selection.hidden_library_ids.clone()));
    }
    if !selection.wp_type.is_empty() {
        query = query.filter(item::Column::Ty.eq(&selection.wp_type));
    }
    if !selection.skip_types.is_empty() {
        query = query.filter(item::Column::Ty.is_not_in(selection.skip_types.clone()));
    }
    match selection.hidden {
        HiddenFilter::Exclude => query = query.filter(item::Column::Hidden.eq(false)),
        HiddenFilter::Only => query = query.filter(item::Column::Hidden.eq(true)),
        HiddenFilter::Include => {}
    }
    let mut quick = Vec::new();
    if !selection.filter_tags.is_empty() {
        quick.push(FilterRule {
            group: 0,
            predicate: FilterPredicate::Tags {
                values: selection.filter_tags.clone(),
                condition: StringMatch::Is,
            },
        });
    }
    for rating in &selection.skip_content_ratings {
        quick.push(FilterRule {
            group: 0,
            predicate: FilterPredicate::ContentRating {
                value: rating.clone(),
                condition: StringMatch::IsNot,
            },
        });
    }
    if let Some(condition) = filter::wallpaper_filters_to_condition(&quick, &[]) {
        query = query.filter(condition);
    }
    let rows = query
        .order_by_asc(item::Column::LibraryId)
        .order_by_asc(item::Column::Path)
        .order_by_asc(item::Column::Id)
        .all(conn)
        .await
        .context("select wallpaper IDs")?;
    let plugins = source_plugin::Entity::find()
        .all(conn)
        .await
        .context("select wallpaper plugins")?
        .into_iter()
        .map(|plugin| (plugin.id, plugin.name))
        .collect::<HashMap<_, _>>();
    let entries = rows
        .into_iter()
        .filter_map(|(item, library)| {
            library.map(|library| {
                let plugin = plugins
                    .get(&item.plugin_id)
                    .map(String::as_str)
                    .unwrap_or_default();
                entry_from_item(item, &library.path, plugin)
            })
        })
        .collect::<Vec<_>>();
    let mut ordered: Vec<_> = entries.iter().collect();
    crate::catalog::query::sort_entries(&mut ordered, &selection.sorts);
    Ok(ordered.into_iter().cloned().collect())
}

pub async fn populate_entry_tags<C: ConnectionTrait>(
    conn: &C,
    entries: &mut [crate::catalog::entry::WallpaperEntry],
) -> Result<()> {
    for entries in entries.chunks_mut(500) {
        let ids = entries.iter().map(|e| e.item_id).collect::<Vec<_>>();
        let mut tags = list_tags_for_items(conn, &ids).await?;
        for entry in entries {
            entry.tags = tags.remove(&entry.item_id).unwrap_or_default();
        }
    }
    Ok(())
}

pub async fn list_item_keys_by_wallpaper_filters(
    db: &DatabaseConnection,
    filters: &[crate::catalog::FilterRule],
    logics: &[crate::catalog::FilterLogic],
) -> Result<Vec<(String, String)>> {
    list_item_keys_by_wallpaper_query(
        db,
        &crate::catalog::CatalogQuery {
            filters: filters.to_vec(),
            logics: logics.to_vec(),
            ..Default::default()
        },
    )
    .await
}

pub async fn list_item_keys_by_wallpaper_query(
    db: &DatabaseConnection,
    catalog_query: &crate::catalog::CatalogQuery,
) -> Result<Vec<(String, String)>> {
    let mut query = item::Entity::find().find_also_related(library::Entity);
    if let Some(condition) = filter::wallpaper_query_to_condition(
        &catalog_query.filters,
        &catalog_query.logics,
        &catalog_query.search_text,
    ) {
        query = query.filter(condition);
    }
    let rows = query
        .order_by_asc(item::Column::LibraryId)
        .order_by_asc(item::Column::Path)
        .all(db)
        .await
        .context("select wallpaper query item keys")?;
    Ok(rows
        .into_iter()
        .filter_map(|(it, lib)| lib.map(|lib| (lib.path, it.path)))
        .collect())
}

/// Queue iteration row: enough for the caller to bridge to a
/// `WallpaperEntry` via library_root + relative path, and to anchor
#[derive(Debug, Clone)]
pub struct QueueRow {
    pub item_id: i64,
    pub library_path: String,
    pub item_path: String,
}

/// Total count of items matching the filter.
pub async fn count_items_by_filter(
    db: &DatabaseConnection,
    filters: &[crate::catalog::FilterRule],
    logics: &[crate::catalog::FilterLogic],
) -> Result<u64> {
    let mut query = item::Entity::find().find_also_related(library::Entity);
    if let Some(condition) = filter::wallpaper_filters_to_condition(filters, logics) {
        query = query.filter(condition);
    }
    query.count(db).await.context("count filtered items")
}

/// Every DB id matching the filter, in stable (library_id, path) order.
/// Used to materialize a shuffle round.
pub async fn list_item_ids_by_filter(
    db: &DatabaseConnection,
    filters: &[crate::catalog::FilterRule],
    logics: &[crate::catalog::FilterLogic],
) -> Result<Vec<i64>> {
    let mut query = item::Entity::find();
    if let Some(condition) = filter::wallpaper_filters_to_condition(filters, logics) {
        query = query.filter(condition);
    }
    let rows = query
        .order_by_asc(item::Column::LibraryId)
        .order_by_asc(item::Column::Path)
        .all(db)
        .await
        .context("select filtered item ids")?;
    Ok(rows.into_iter().map(|it| it.id).collect())
}

/// Random sample. `exclude_id` is the current cursor, omitted from the
/// pool when more than one item matches the filter.
pub async fn random_item_by_filter(
    db: &DatabaseConnection,
    filters: &[crate::catalog::FilterRule],
    logics: &[crate::catalog::FilterLogic],
    exclude_id: Option<i64>,
) -> Result<Option<QueueRow>> {
    use sea_orm::sea_query::Expr;
    use sea_orm::Condition;

    let cond = filter::wallpaper_filters_to_condition(filters, logics);

    // Decide whether the exclusion would empty the candidate set.
    let total = count_items_by_filter(db, filters, logics).await?;
    let apply_exclude = matches!(exclude_id, Some(_)) && total > 1;

    let combined = match (cond, exclude_id) {
        (Some(c), Some(eid)) if apply_exclude => Some(c.add(item::Column::Id.ne(eid))),
        (Some(c), _) => Some(c),
        (None, Some(eid)) if apply_exclude => Some(Condition::all().add(item::Column::Id.ne(eid))),
        (None, _) => None,
    };

    let mut query = item::Entity::find().find_also_related(library::Entity);
    if let Some(c) = combined {
        query = query.filter(c);
    }
    let row = query
        .order_by_asc(Expr::cust("RANDOM()"))
        .one(db)
        .await
        .context("random_item_by_filter")?;
    Ok(row.and_then(|(it, lib)| {
        lib.map(|lib| QueueRow {
            item_id: it.id,
            library_path: lib.path,
            item_path: it.path,
        })
    }))
}

/// Resolve an item by `(library.path, item.path)`. Used to bridge
/// snapshot entries to DB rows after `WallpaperApply` (so the queue's
pub async fn find_item_by_library_path(
    db: &DatabaseConnection,
    library_path: &str,
    relative_path: &str,
) -> Result<Option<item::Model>> {
    let lib = library::Entity::find()
        .filter(library::Column::Path.eq(library_path))
        .one(db)
        .await
        .with_context(|| format!("select library by path={library_path}"))?;
    let lib = match lib {
        Some(l) => l,
        None => return Ok(None),
    };
    item::Entity::find()
        .filter(item::Column::LibraryId.eq(lib.id))
        .filter(item::Column::Path.eq(relative_path))
        .one(db)
        .await
        .with_context(|| format!("select item by lib={library_path} path={relative_path}"))
}

/// Resolve a single item by DB id (with its library row).
pub async fn get_item_with_library(db: &DatabaseConnection, id: i64) -> Result<Option<QueueRow>> {
    let row = item::Entity::find_by_id(id)
        .find_also_related(library::Entity)
        .one(db)
        .await
        .with_context(|| format!("select item id={id}"))?;
    Ok(row.and_then(|(it, lib)| {
        lib.map(|lib| QueueRow {
            item_id: it.id,
            library_path: lib.path,
            item_path: it.path,
        })
    }))
}

pub async fn list_items_by_plugin(
    db: &DatabaseConnection,
    plugin_id: i64,
) -> Result<Vec<item::Model>> {
    item::Entity::find()
        .filter(item::Column::PluginId.eq(plugin_id))
        .order_by_asc(item::Column::LibraryId)
        .order_by_asc(item::Column::Path)
        .all(db)
        .await
        .with_context(|| format!("select items plugin={plugin_id}"))
}

pub async fn list_items_by_plugin_external_id(
    db: &DatabaseConnection,
    plugin_name: &str,
    external_id: &str,
) -> Result<Vec<(item::Model, library::Model)>> {
    if external_id.is_empty() {
        return Ok(Vec::new());
    }
    let Some(plugin) = find_plugin_by_name(db, plugin_name).await? else {
        return Ok(Vec::new());
    };
    let rows = item::Entity::find()
        .filter(item::Column::PluginId.eq(plugin.id))
        .filter(item::Column::ExternalId.eq(external_id))
        .find_also_related(library::Entity)
        .order_by_asc(item::Column::LibraryId)
        .order_by_asc(item::Column::Path)
        .all(db)
        .await
        .with_context(|| format!("select items plugin={plugin_name} external_id={external_id}"))?;
    Ok(rows
        .into_iter()
        .filter_map(|(it, lib)| lib.map(|lib| (it, lib)))
        .collect())
}

pub async fn has_item_by_plugin_external_id(
    db: &DatabaseConnection,
    plugin_name: &str,
    external_id: &str,
) -> Result<bool> {
    Ok(
        !list_items_by_plugin_external_id(db, plugin_name, external_id)
            .await?
            .is_empty(),
    )
}

/// Sweep stale items in `library_ids`.
/// Only the caller's successfully scanned libraries may be swept.
pub async fn delete_items_not_seen(
    db: &impl ConnectionTrait,
    library_ids: &[i64],
    scan_id: i64,
) -> Result<u64> {
    if library_ids.is_empty() {
        return Ok(0);
    }
    let res = item::Entity::delete_many()
        .filter(item::Column::LibraryId.is_in(library_ids.iter().copied()))
        .filter(item::Column::SeenScanId.ne(scan_id))
        .exec(db)
        .await
        .context("sweep items not seen in scan")?;
    Ok(res.rows_affected)
}

pub async fn delete_item(db: &DatabaseConnection, item_id: i64) -> Result<u64> {
    let res = item::Entity::delete_by_id(item_id)
        .exec(db)
        .await
        .with_context(|| format!("delete item id={item_id}"))?;
    Ok(res.rows_affected)
}

/// Validates all ids and returns the ids whose hidden flag changed after commit.
pub async fn set_items_hidden(
    db: &DatabaseConnection,
    item_ids: &[i64],
    hidden: bool,
) -> Result<Vec<i64>> {
    if item_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut ids = item_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let txn = db.begin().await.context("begin item hidden update")?;
    let mut changed = Vec::new();
    // Bound SQL parameters for SQLite builds with a low variable limit.
    for chunk in ids.chunks(500) {
        let rows: HashMap<i64, bool> = item::Entity::find()
            .select_only()
            .columns([item::Column::Id, item::Column::Hidden])
            .filter(item::Column::Id.is_in(chunk.iter().copied()))
            .into_tuple::<(i64, bool)>()
            .all(&txn)
            .await
            .context("select item hidden flags")?
            .into_iter()
            .collect();
        for id in chunk {
            let current = rows
                .get(id)
                .ok_or_else(|| Error::WallpaperNotFound(id.to_string()))?;
            if *current != hidden {
                changed.push(*id);
            }
        }
    }
    for chunk in changed.chunks(500) {
        item::Entity::update_many()
            .col_expr(item::Column::Hidden, Expr::value(hidden))
            .filter(item::Column::Id.is_in(chunk.iter().copied()))
            .exec(&txn)
            .await
            .context("update item hidden flags")?;
    }
    txn.commit().await.context("commit item hidden update")?;
    Ok(changed)
}
