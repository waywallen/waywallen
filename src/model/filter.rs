use std::collections::{BTreeMap, BTreeSet};

use sea_orm::{
    sea_query::{Expr, LikeExpr},
    Condition,
};

use crate::catalog::query::{
    FilterLogic, FilterPredicate, FilterRule, IntMatch, LogicOperator, StringMatch,
};

use super::entities::item;

pub fn build_grouped_condition<'a, F, GFn, CFn>(
    filters: impl IntoIterator<Item = &'a F>,
    group_of: GFn,
    to_condition: CFn,
    logics: &[FilterLogic],
) -> Option<Condition>
where
    F: 'a,
    GFn: Fn(&F) -> i32,
    CFn: Fn(&F) -> Option<Condition>,
{
    let mut buckets: BTreeMap<i32, Vec<Condition>> = BTreeMap::new();
    for filter in filters {
        if let Some(condition) = to_condition(filter) {
            buckets.entry(group_of(filter)).or_default().push(condition);
        }
    }
    if buckets.is_empty() {
        return None;
    }

    let group_cond = |group: i32| -> Option<Condition> {
        buckets.get(&group).map(|conditions| {
            let mut out = Condition::all();
            for condition in conditions {
                out = out.add(condition.clone());
            }
            out
        })
    };

    let mut pair_conds = Vec::new();
    let mut referenced = BTreeSet::<i32>::new();
    for logic in logics {
        let a = group_cond(logic.group_a);
        let b = group_cond(logic.group_b);
        let (a, b) = match (a, b) {
            (Some(a), Some(b)) => (a, b),
            _ => continue,
        };
        let combined = match logic.operator {
            LogicOperator::Or => Condition::any().add(a).add(b),
            LogicOperator::And => Condition::all().add(a).add(b),
        };
        pair_conds.push(combined);
        referenced.insert(logic.group_a);
        referenced.insert(logic.group_b);
    }

    let mut outer = Condition::all();
    for pair in pair_conds {
        outer = outer.add(pair);
    }
    for group in buckets.keys() {
        if !referenced.contains(group) {
            if let Some(condition) = group_cond(*group) {
                outer = outer.add(condition);
            }
        }
    }
    Some(outer)
}

pub fn wallpaper_filters_to_condition(
    filters: &[FilterRule],
    logics: &[FilterLogic],
) -> Option<Condition> {
    build_grouped_condition(
        filters,
        |filter| filter.group,
        wallpaper_filter_to_condition,
        logics,
    )
}

pub fn wallpaper_query_to_condition(
    filters: &[FilterRule],
    logics: &[FilterLogic],
    search_text: &str,
) -> Option<Condition> {
    match (
        wallpaper_filters_to_condition(filters, logics),
        wallpaper_search_to_condition(search_text),
    ) {
        (Some(filters), Some(search)) => Some(Condition::all().add(filters).add(search)),
        (Some(filters), None) => Some(filters),
        (None, Some(search)) => Some(search),
        (None, None) => None,
    }
}

fn wallpaper_search_to_condition(search_text: &str) -> Option<Condition> {
    let search_text = search_text.trim();
    if search_text.is_empty() {
        return None;
    }

    let text = text_contains_condition(search_text)?;
    let pattern = literal_contains_pattern(search_text);
    let external_id = Expr::col((item::Entity, item::Column::ExternalId))
        .like(LikeExpr::new(pattern).escape('\\'));
    Some(Condition::any().add(text).add(external_id))
}

pub fn wallpaper_filter_to_condition(filter: &FilterRule) -> Option<Condition> {
    match &filter.predicate {
        FilterPredicate::Name { value, condition } => {
            name_condition_to_condition(value, *condition)
        }
        FilterPredicate::WallpaperType { value, condition } => string_condition_to_condition(
            || Expr::col((item::Entity, item::Column::Ty)),
            *condition,
            &value.to_ascii_lowercase(),
            false,
        ),
        FilterPredicate::Library { value, exclude } => Some(Condition::all().add(if *exclude {
            Expr::col((item::Entity, item::Column::LibraryId)).ne(*value)
        } else {
            Expr::col((item::Entity, item::Column::LibraryId)).eq(*value)
        })),
        FilterPredicate::Width { value, condition } => int_condition_to_condition(
            || Expr::col((item::Entity, item::Column::Width)),
            *condition,
            *value,
        ),
        FilterPredicate::Height { value, condition } => int_condition_to_condition(
            || Expr::col((item::Entity, item::Column::Height)),
            *condition,
            *value,
        ),
        FilterPredicate::Size { value, condition } => int_condition_to_condition(
            || Expr::col((item::Entity, item::Column::Size)),
            *condition,
            *value,
        ),
        FilterPredicate::ContentRating { value, condition } => string_condition_to_condition(
            || Expr::col((item::Entity, item::Column::ContentRating)),
            *condition,
            value,
            true,
        ),
        FilterPredicate::Tags { values, condition } => {
            tag_list_condition_to_condition(values, *condition)
        }
    }
}

/// Tag-set membership. IS → has any of `values`; IS_NOT → has none of
/// them. Empty list imposes no constraint.
fn tag_list_condition_to_condition(values: &[String], cond: StringMatch) -> Option<Condition> {
    let names: Vec<&String> = values.iter().filter(|v| !v.is_empty()).collect();
    if names.is_empty() {
        return None;
    }
    let exists = |tag: &str| {
        format!(
            "EXISTS (SELECT 1 FROM item_tag JOIN tag ON tag.id = item_tag.tag_id \
             WHERE item_tag.item_id = item.id AND tag.name = {} COLLATE NOCASE)",
            sqlite_quote(tag)
        )
    };
    match cond {
        StringMatch::Is | StringMatch::Contains => {
            let mut any = Condition::any();
            for tag in names {
                any = any.add(Expr::cust(exists(tag)));
            }
            Some(any)
        }
        StringMatch::IsNot | StringMatch::ContainsNot => {
            let mut all = Condition::all();
            for tag in names {
                all = all.add(Expr::cust(format!("NOT {}", exists(tag))));
            }
            Some(all)
        }
    }
}

fn name_condition_to_condition(value: &str, cond: StringMatch) -> Option<Condition> {
    match cond {
        StringMatch::Contains => text_contains_condition(value),
        StringMatch::ContainsNot => {
            let fts_query = build_fts_match_query(value)?;
            let quoted = sqlite_quote(&fts_query);
            let esc = value.replace('\'', "''");
            let tag_exists = format!(
                "EXISTS (SELECT 1 FROM item_tag JOIN tag ON tag.id = item_tag.tag_id \
                 WHERE item_tag.item_id = item.id AND tag.name LIKE '%{esc}%' COLLATE NOCASE)"
            );
            let fts = format!(
                "item.id NOT IN (SELECT rowid FROM item_fts WHERE item_fts MATCH {quoted})"
            );
            Some(
                Condition::all()
                    .add(Expr::cust(fts))
                    .add(Expr::cust(format!("NOT {tag_exists}"))),
            )
        }
        _ => string_condition_to_condition(
            || Expr::col((item::Entity, item::Column::DisplayName)),
            cond,
            value,
            false,
        ),
    }
}

fn text_contains_condition(value: &str) -> Option<Condition> {
    let fts_query = build_fts_match_query(value)?;
    let quoted = sqlite_quote(&fts_query);
    let esc = value.replace('\'', "''");
    let fts = format!("item.id IN (SELECT rowid FROM item_fts WHERE item_fts MATCH {quoted})");
    let tag_exists = format!(
        "EXISTS (SELECT 1 FROM item_tag JOIN tag ON tag.id = item_tag.tag_id \
         WHERE item_tag.item_id = item.id AND tag.name LIKE '%{esc}%' COLLATE NOCASE)"
    );
    Some(
        Condition::any()
            .add(Expr::cust(fts))
            .add(Expr::cust(tag_exists)),
    )
}

fn literal_contains_pattern(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('%');
    for ch in value.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped.push('%');
    escaped
}

fn build_fts_match_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split_whitespace()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" AND "))
    }
}

fn sqlite_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', "''"))
}

fn string_condition_to_condition<E>(
    col: E,
    cond: StringMatch,
    value: &str,
    null_matches_negative: bool,
) -> Option<Condition>
where
    E: Fn() -> sea_orm::sea_query::Expr,
{
    match cond {
        StringMatch::Contains => Some(Condition::all().add(col().like(format!("%{value}%")))),
        StringMatch::ContainsNot => {
            let not_like = col().not_like(format!("%{value}%"));
            if null_matches_negative {
                Some(Condition::any().add(col().is_null()).add(not_like))
            } else {
                Some(Condition::all().add(not_like))
            }
        }
        StringMatch::Is => Some(Condition::all().add(col().eq(value))),
        StringMatch::IsNot => {
            let ne = col().ne(value);
            if null_matches_negative {
                Some(Condition::any().add(col().is_null()).add(ne))
            } else {
                Some(Condition::all().add(ne))
            }
        }
    }
}

fn int_condition_to_condition<E>(col: E, cond: IntMatch, value: i64) -> Option<Condition>
where
    E: Fn() -> sea_orm::sea_query::Expr,
{
    let expr = match cond {
        IntMatch::Equal => col().eq(value),
        IntMatch::NotEqual => col().ne(value),
        IntMatch::Less => col().lt(value),
        IntMatch::LessEqual => col().lte(value),
        IntMatch::Greater => col().gt(value),
        IntMatch::GreaterEqual => col().gte(value),
    };
    Some(Condition::all().add(expr))
}

#[cfg(test)]
mod tests {
    use sea_orm::{EntityTrait, QueryFilter};

    use super::*;
    use crate::model::{
        connect_url,
        entities::{item, library},
        repo::{self, ItemUpsertArgs},
    };

    async fn seed() -> sea_orm::DatabaseConnection {
        let db = connect_url("sqlite::memory:").await.unwrap();
        let plugin = repo::upsert_plugin(&db, "p", "1").await.unwrap();
        let lib_a = repo::add_library(&db, plugin.id, "/lib/a").await.unwrap();
        let lib_b = repo::add_library(&db, plugin.id, "/lib/b").await.unwrap();

        repo::upsert_item(
            &db,
            ItemUpsertArgs {
                plugin_id: plugin.id,
                library_id: lib_a.id,
                path: "city.png",
                ty: "image",
                display_name: "City",
                preview_path: None,
                description: Some("sunset skyline"),
                external_id: Some("wallhaven-Ab_C%42"),
                web_url: None,
                update_available: false,
                size: Some(2048),
                width: Some(1920),
                height: Some(1080),
                content_rating: Some("Everyone"),
            },
        )
        .await
        .unwrap();
        repo::upsert_item(
            &db,
            ItemUpsertArgs {
                plugin_id: plugin.id,
                library_id: lib_b.id,
                path: "portrait.webm",
                ty: "video",
                display_name: "Portrait",
                preview_path: None,
                description: None,
                external_id: Some("wallhaven-AbXCDY42"),
                web_url: None,
                update_available: false,
                size: Some(4096),
                width: Some(900),
                height: Some(1600),
                content_rating: Some("Mature"),
            },
        )
        .await
        .unwrap();
        repo::upsert_item(
            &db,
            ItemUpsertArgs {
                plugin_id: plugin.id,
                library_id: lib_a.id,
                path: "unrated.png",
                ty: "image",
                display_name: "Unrated",
                preview_path: None,
                description: None,
                external_id: None,
                web_url: None,
                update_available: false,
                size: Some(1024),
                width: Some(640),
                height: Some(480),
                content_rating: None,
            },
        )
        .await
        .unwrap();
        db
    }

    #[tokio::test]
    async fn content_rating_exclusion_keeps_unrated_items() {
        let db = seed().await;
        let filter = FilterRule {
            group: 0,
            predicate: FilterPredicate::ContentRating {
                value: "Mature".into(),
                condition: StringMatch::IsNot,
            },
        };

        let condition = wallpaper_filters_to_condition(&[filter], &[]).unwrap();
        let rows = item::Entity::find()
            .filter(condition)
            .all(&db)
            .await
            .unwrap();
        let names: BTreeSet<_> = rows.into_iter().map(|row| row.display_name).collect();

        assert_eq!(
            names,
            BTreeSet::from(["City".to_owned(), "Unrated".to_owned()])
        );
    }

    #[tokio::test]
    async fn wallpaper_filters_to_condition_matches_grouped_rules() {
        let db = seed().await;

        let name = FilterRule {
            group: 0,
            predicate: FilterPredicate::Name {
                value: "City".into(),
                condition: StringMatch::Contains,
            },
        };

        let width = FilterRule {
            group: 0,
            predicate: FilterPredicate::Width {
                value: 1000,
                condition: IntMatch::GreaterEqual,
            },
        };

        let condition = wallpaper_filters_to_condition(&[name, width], &[]).unwrap();
        let rows = item::Entity::find()
            .find_also_related(library::Entity)
            .filter(condition)
            .all(&db)
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0.display_name, "City");
    }

    #[tokio::test]
    async fn wallpaper_filters_to_condition_honors_group_or_logic() {
        let db = seed().await;

        let ty = FilterRule {
            group: 0,
            predicate: FilterPredicate::WallpaperType {
                value: "video".into(),
                condition: StringMatch::Is,
            },
        };

        let wide = FilterRule {
            group: 1,
            predicate: FilterPredicate::Width {
                value: 1500,
                condition: IntMatch::GreaterEqual,
            },
        };

        let logic = FilterLogic {
            operator: LogicOperator::Or,
            group_a: 0,
            group_b: 1,
        };

        let condition = wallpaper_filters_to_condition(&[ty, wide], &[logic]).unwrap();
        let rows = item::Entity::find()
            .find_also_related(library::Entity)
            .filter(condition)
            .all(&db)
            .await
            .unwrap();

        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn wallpaper_name_contains_matches_description_via_fts() {
        let db = seed().await;

        let name = FilterRule {
            group: 0,
            predicate: FilterPredicate::Name {
                value: "skyline".into(),
                condition: StringMatch::Contains,
            },
        };

        let condition = wallpaper_filters_to_condition(&[name], &[]).unwrap();
        let rows = item::Entity::find()
            .find_also_related(library::Entity)
            .filter(condition)
            .all(&db)
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0.display_name, "City");
    }

    #[tokio::test]
    async fn wallpaper_search_matches_external_id_literal_substring() {
        let db = seed().await;

        let condition = wallpaper_query_to_condition(&[], &[], "B_C%4").unwrap();
        let rows = item::Entity::find()
            .find_also_related(library::Entity)
            .filter(condition)
            .all(&db)
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0.display_name, "City");
    }

    #[tokio::test]
    async fn wallpaper_search_ands_with_structured_filters() {
        let db = seed().await;
        let video = FilterRule {
            group: 0,
            predicate: FilterPredicate::WallpaperType {
                value: "video".into(),
                condition: StringMatch::Is,
            },
        };

        let condition = wallpaper_query_to_condition(&[video], &[], "B_C%4").unwrap();
        let rows = item::Entity::find()
            .find_also_related(library::Entity)
            .filter(condition)
            .all(&db)
            .await
            .unwrap();

        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn wallpaper_search_keeps_existing_text_matches() {
        let db = seed().await;

        let condition = wallpaper_query_to_condition(&[], &[], "skyline").unwrap();
        let rows = item::Entity::find()
            .find_also_related(library::Entity)
            .filter(condition)
            .all(&db)
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0.display_name, "City");
    }
}
