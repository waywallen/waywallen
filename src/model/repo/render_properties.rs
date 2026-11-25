use super::*;
use crate::catalog::properties::{self, WallpaperLayoutOverride};

#[derive(Debug)]
pub struct PropertyOverridesChange {
    pub before: HashMap<String, String>,
    pub after: HashMap<String, String>,
}

fn parse_user_property_overrides(item_id: i64, raw: Option<&str>) -> HashMap<String, String> {
    let Some(raw) = raw else {
        return HashMap::new();
    };
    match serde_json::from_str::<HashMap<String, String>>(raw) {
        Ok(overrides) => properties::normalize_user_property_overrides(overrides),
        Err(error) => {
            log::warn!(
                "item {item_id}: user_property_overrides JSON unparseable ({error}); treating as empty"
            );
            HashMap::new()
        }
    }
}

pub async fn get_user_property_overrides(
    db: &DatabaseConnection,
    item_id: i64,
) -> Result<HashMap<String, String>> {
    read_user_property_overrides(db, item_id).await
}

async fn read_user_property_overrides(
    db: &impl ConnectionTrait,
    item_id: i64,
) -> Result<HashMap<String, String>> {
    let row = item::Entity::find_by_id(item_id)
        .one(db)
        .await
        .with_context(|| format!("select item by id={item_id} for overrides"))?;
    let Some(item) = row else {
        return Ok(HashMap::new());
    };
    Ok(parse_user_property_overrides(
        item_id,
        item.user_property_overrides.as_deref(),
    ))
}

pub async fn get_user_property_overrides_raw(
    db: &DatabaseConnection,
    item_id: i64,
) -> Result<Option<String>> {
    let row = item::Entity::find_by_id(item_id)
        .one(db)
        .await
        .with_context(|| format!("select item by id={item_id} for raw overrides"))?;
    Ok(row
        .and_then(|item| item.user_property_overrides)
        .map(|raw| properties::normalize_user_property_overrides_json(&raw)))
}

fn parse_wallpaper_layout_override_raw(
    item_id: i64,
    raw: Option<&str>,
) -> Option<WallpaperLayoutOverride> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    let parsed = properties::wallpaper_layout_override_from_json(raw);
    if parsed.is_none() {
        log::warn!("item {item_id}: wallpaper_layout_override JSON unparseable; ignoring");
    }
    parsed
}

pub async fn get_wallpaper_render_properties(
    db: &DatabaseConnection,
    item_id: i64,
) -> Result<(HashMap<String, String>, WallpaperLayoutOverride)> {
    let row = item::Entity::find_by_id(item_id)
        .one(db)
        .await
        .with_context(|| format!("select item by id={item_id} for render properties"))?;
    let Some(item) = row else {
        return Ok((HashMap::new(), Default::default()));
    };
    let user_properties =
        parse_user_property_overrides(item_id, item.user_property_overrides.as_deref());
    let (renderer_properties, legacy_layout) =
        properties::split_renderer_properties(user_properties);
    let layout =
        parse_wallpaper_layout_override_raw(item_id, item.wallpaper_layout_override.as_deref())
            .unwrap_or(legacy_layout);
    Ok((renderer_properties, layout))
}

pub async fn get_wallpaper_layout_override_with_legacy(
    db: &DatabaseConnection,
    item_id: i64,
) -> Result<Option<WallpaperLayoutOverride>> {
    let (_, layout) = get_wallpaper_render_properties(db, item_id).await?;
    Ok((!layout.is_empty()).then_some(layout))
}

pub async fn set_wallpaper_layout_override(
    db: &DatabaseConnection,
    item_id: i64,
    layout: Option<crate::settings::ResolvedLayout>,
) -> Result<()> {
    let serialized = layout
        .map(properties::wallpaper_layout_override_to_json)
        .transpose()
        .context("serialize wallpaper_layout_override")?;
    let txn = db.begin().await.context("begin wallpaper layout update")?;
    let user_property_overrides = if serialized.is_none() {
        let mut current = read_user_property_overrides(&txn, item_id).await?;
        current.retain(|key, _| !properties::is_daemon_display_property_key(key));
        Set(serialize_user_property_overrides(&current)?)
    } else {
        NotSet
    };
    let active = item::ActiveModel {
        id: Set(item_id),
        wallpaper_layout_override: Set(serialized),
        user_property_overrides,
        ..Default::default()
    };
    item::Entity::update(active)
        .exec(&txn)
        .await
        .with_context(|| format!("update item {item_id} wallpaper_layout_override"))?;
    txn.commit().await.context("commit wallpaper layout update")
}

pub async fn set_user_property_override(
    db: &DatabaseConnection,
    item_id: i64,
    key: &str,
    value: Option<&str>,
) -> Result<()> {
    let txn = db.begin().await.context("begin user property update")?;
    let mut current = read_user_property_overrides(&txn, item_id).await?;
    let key = properties::canonical_user_property_key(key);
    if let Some(value) = value {
        current.insert(key.to_string(), value.to_string());
    } else {
        current.remove(key);
    }
    write_user_property_overrides(&txn, item_id, &current).await?;
    txn.commit().await.context("commit user property update")
}

pub(super) fn partition_user_property_overrides(
    item: &item::Model,
) -> (HashMap<String, String>, HashMap<String, String>) {
    parse_user_property_overrides(item.id, item.user_property_overrides.as_deref())
        .into_iter()
        .partition(|(key, _)| !properties::is_daemon_display_property_key(key))
}

fn serialize_user_property_overrides(
    overrides: &HashMap<String, String>,
) -> Result<Option<String>> {
    if overrides.is_empty() {
        Ok(None)
    } else {
        serde_json::to_string(overrides)
            .map(Some)
            .context("serialize user_property_overrides")
    }
}

pub(super) async fn write_user_property_overrides(
    txn: &DatabaseTransaction,
    item_id: i64,
    overrides: &HashMap<String, String>,
) -> Result<()> {
    let active = item::ActiveModel {
        id: Set(item_id),
        user_property_overrides: Set(serialize_user_property_overrides(overrides)?),
        ..Default::default()
    };
    item::Entity::update(active)
        .exec(txn)
        .await
        .with_context(|| format!("update item {item_id} user_property_overrides"))?;
    Ok(())
}
