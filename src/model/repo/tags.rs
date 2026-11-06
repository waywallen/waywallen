use super::*;

// tag / item_tag

/// Upsert tags by name. SQLite `COLLATE NOCASE` makes the unique
/// index case-insensitive, so differently cased duplicates collapse.
pub async fn upsert_tags(db: &impl ConnectionTrait, names: &[String]) -> Result<Vec<tag::Model>> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut unique_inputs: Vec<&str> = Vec::new();
    for n in names {
        let trimmed = n.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = trimmed.to_lowercase();
        if seen.insert(key) {
            unique_inputs.push(trimmed);
        }
    }
    let mut out = Vec::with_capacity(unique_inputs.len());
    for name in unique_inputs {
        let existing = tag::Entity::find()
            .filter(tag::Column::Name.eq(name))
            .one(db)
            .await
            .with_context(|| format!("select tag name={name}"))?;
        let model = match existing {
            Some(m) => m,
            None => tag::ActiveModel {
                name: Set(name.to_owned()),
                ..Default::default()
            }
            .insert(db)
            .await
            .with_context(|| format!("insert tag name={name}"))?,
        };
        out.push(model);
    }
    Ok(out)
}

/// Replace the complete tag set of an item. DELETE + INSERT in one
/// transaction.
pub async fn replace_item_tags(
    db: &(impl ConnectionTrait + TransactionTrait),
    item_id: i64,
    tag_ids: &[i64],
) -> Result<()> {
    let tx: DatabaseTransaction = db.begin().await.context("begin tx")?;
    item_tag::Entity::delete_many()
        .filter(item_tag::Column::ItemId.eq(item_id))
        .exec(&tx)
        .await
        .with_context(|| format!("clear item_tag item={item_id}"))?;
    let unique: HashSet<i64> = tag_ids.iter().copied().collect();
    if !unique.is_empty() {
        let rows: Vec<item_tag::ActiveModel> = unique
            .into_iter()
            .map(|tid| item_tag::ActiveModel {
                item_id: Set(item_id),
                tag_id: Set(tid),
            })
            .collect();
        item_tag::Entity::insert_many(rows)
            .exec(&tx)
            .await
            .with_context(|| format!("insert item_tag item={item_id}"))?;
    }
    tx.commit().await.context("commit tx")?;
    Ok(())
}

pub async fn list_tags(db: &DatabaseConnection) -> Result<Vec<tag::Model>> {
    tag::Entity::find()
        .order_by_asc(tag::Column::Name)
        .all(db)
        .await
        .context("select tags")
}

/// Distinct non-null `content_rating` values across all items, ascending.
pub async fn list_content_ratings(db: &DatabaseConnection) -> Result<Vec<String>> {
    let rows: Vec<Option<String>> = item::Entity::find()
        .select_only()
        .column(item::Column::ContentRating)
        .distinct()
        .filter(item::Column::ContentRating.is_not_null())
        .order_by_asc(item::Column::ContentRating)
        .into_tuple()
        .all(db)
        .await
        .context("select distinct content_rating")?;
    Ok(rows.into_iter().flatten().collect())
}

pub async fn list_items_by_tag(db: &DatabaseConnection, tag_id: i64) -> Result<Vec<item::Model>> {
    item::Entity::find()
        .inner_join(item_tag::Entity)
        .filter(item_tag::Column::TagId.eq(tag_id))
        .order_by_asc(item::Column::Id)
        .all(db)
        .await
        .with_context(|| format!("select items by tag={tag_id}"))
}

pub async fn list_tags_of_item(db: &DatabaseConnection, item_id: i64) -> Result<Vec<tag::Model>> {
    tag::Entity::find()
        .inner_join(item_tag::Entity)
        .filter(item_tag::Column::ItemId.eq(item_id))
        .order_by_asc(tag::Column::Name)
        .all(db)
        .await
        .with_context(|| format!("select tags of item={item_id}"))
}

/// Batch variant of `list_tags_of_item`: one round-trip resolving the
/// tag set for every requested item, grouped by item id.
pub async fn list_tags_for_items<C: ConnectionTrait>(
    db: &C,
    item_ids: &[i64],
) -> Result<HashMap<i64, Vec<String>>> {
    if item_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(item_tag::Model, Option<tag::Model>)> = item_tag::Entity::find()
        .find_also_related(tag::Entity)
        .filter(item_tag::Column::ItemId.is_in(item_ids.iter().copied()))
        .order_by_asc(tag::Column::Name)
        .all(db)
        .await
        .context("select tags for items")?;
    let mut out: HashMap<i64, Vec<String>> = HashMap::new();
    for (it, t) in rows {
        if let Some(t) = t {
            out.entry(it.item_id).or_default().push(t.name);
        }
    }
    Ok(out)
}
