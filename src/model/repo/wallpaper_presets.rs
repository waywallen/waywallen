use sea_orm::{DbErr, Select, SqlErr};

use super::render_properties::{partition_user_property_overrides, write_user_property_overrides};
use super::*;
use crate::catalog::properties::normalize_renderer_user_properties;
use crate::model::entities::wallpaper_property_preset as preset;

pub async fn list_wallpaper_presets(
    db: &DatabaseConnection,
    item_id: i64,
) -> Result<Vec<preset::Model>> {
    let txn = db.begin().await.context("begin wallpaper preset list")?;
    let item = require_item(&txn, item_id).await?;
    let presets = presets_for_item(&item)
        .order_by_asc(preset::Column::Name)
        .all(&txn)
        .await
        .context("select wallpaper presets")?;
    txn.commit().await.context("commit wallpaper preset list")?;
    Ok(presets)
}

pub async fn create_wallpaper_preset(
    db: &DatabaseConnection,
    item_id: i64,
    name: &str,
) -> Result<()> {
    let name = validate_name(name)?;
    let txn = db.begin().await.context("begin wallpaper preset create")?;
    let item = require_item(&txn, item_id).await?;
    let snapshot = snapshot_renderer_overrides(&item)?;
    preset::ActiveModel {
        library_id: Set(item.library_id),
        item_path: Set(item.path),
        name: Set(name.to_owned()),
        user_property_overrides: Set(snapshot),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|err| preset_name_error(err, name))
    .context("insert wallpaper preset")?;
    txn.commit().await.context("commit wallpaper preset create")
}

pub async fn update_wallpaper_preset(
    db: &DatabaseConnection,
    item_id: i64,
    preset_id: i64,
) -> Result<()> {
    let txn = db.begin().await.context("begin wallpaper preset update")?;
    let item = require_item(&txn, item_id).await?;
    let mut preset: preset::ActiveModel = require_preset(&txn, &item, preset_id).await?.into();
    preset.user_property_overrides = Set(snapshot_renderer_overrides(&item)?);
    preset
        .update(&txn)
        .await
        .context("update wallpaper preset")?;
    txn.commit().await.context("commit wallpaper preset update")
}

pub async fn rename_wallpaper_preset(
    db: &DatabaseConnection,
    item_id: i64,
    preset_id: i64,
    name: &str,
) -> Result<()> {
    let name = validate_name(name)?;
    let txn = db.begin().await.context("begin wallpaper preset rename")?;
    let item = require_item(&txn, item_id).await?;
    let mut preset: preset::ActiveModel = require_preset(&txn, &item, preset_id).await?.into();
    preset.name = Set(name.to_owned());
    preset
        .update(&txn)
        .await
        .map_err(|err| preset_name_error(err, name))
        .context("rename wallpaper preset")?;
    txn.commit().await.context("commit wallpaper preset rename")
}

pub async fn delete_wallpaper_preset(
    db: &DatabaseConnection,
    item_id: i64,
    preset_id: i64,
) -> Result<()> {
    let txn = db.begin().await.context("begin wallpaper preset delete")?;
    let item = require_item(&txn, item_id).await?;
    let preset: preset::ActiveModel = require_preset(&txn, &item, preset_id).await?.into();
    preset
        .delete(&txn)
        .await
        .context("delete wallpaper preset")?;
    txn.commit().await.context("commit wallpaper preset delete")
}

/// Returns `None` when the caller must confirm replacing unsaved values.
pub async fn apply_wallpaper_preset(
    db: &DatabaseConnection,
    item_id: i64,
    preset_id: i64,
    protect_unsaved_values: bool,
) -> Result<Option<PropertyOverridesChange>> {
    let txn = db.begin().await.context("begin wallpaper preset apply")?;
    let item = require_item(&txn, item_id).await?;
    let preset = require_preset(&txn, &item, preset_id).await?;
    let after = decode_snapshot(&preset.user_property_overrides)
        .with_context(|| format!("decode wallpaper preset {preset_id} snapshot"))?;
    let (before, mut combined) = partition_user_property_overrides(&item);
    if protect_unsaved_values && !before.is_empty() && before != after {
        let other_snapshots = presets_for_item(&item)
            .filter(preset::Column::Id.ne(preset_id))
            .select_only()
            .column(preset::Column::UserPropertyOverrides)
            .into_tuple::<String>()
            .all(&txn)
            .await
            .context("select saved property overrides")?;
        if !other_snapshots
            .iter()
            .any(|snapshot| decode_snapshot(snapshot).is_ok_and(|overrides| overrides == before))
        {
            txn.rollback()
                .await
                .context("cancel unconfirmed wallpaper preset apply")?;
            return Ok(None);
        }
    }
    if before != after {
        combined.extend(after.clone());
        write_user_property_overrides(&txn, item_id, &combined).await?;
    }
    txn.commit()
        .await
        .context("commit wallpaper preset apply")?;
    Ok(Some(PropertyOverridesChange { before, after }))
}

fn decode_snapshot(raw: &str) -> serde_json::Result<HashMap<String, String>> {
    serde_json::from_str(raw).map(normalize_renderer_user_properties)
}

fn validate_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::InvalidArgument(
            "wallpaper preset name must not be empty".into(),
        ));
    }
    if name.chars().count() > 128 {
        return Err(Error::InvalidArgument(
            "wallpaper preset name must not exceed 128 characters".into(),
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(Error::InvalidArgument(
            "wallpaper preset name must not contain control characters".into(),
        ));
    }
    Ok(name)
}

fn preset_name_error(err: DbErr, name: &str) -> Error {
    if matches!(err.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) {
        return Error::InvalidArgument(format!("wallpaper preset name '{name}' already exists"));
    }
    err.into()
}

fn snapshot_renderer_overrides(item: &item::Model) -> Result<String> {
    let (renderer, _) = partition_user_property_overrides(item);
    serde_json::to_string(&renderer).context("encode wallpaper preset snapshot")
}

async fn require_item(txn: &DatabaseTransaction, item_id: i64) -> Result<item::Model> {
    item::Entity::find_by_id(item_id)
        .one(txn)
        .await
        .with_context(|| format!("select item {item_id} for wallpaper preset"))?
        .ok_or_else(|| Error::WallpaperNotFound(item_id.to_string()))
}

fn presets_for_item(item: &item::Model) -> Select<preset::Entity> {
    preset::Entity::find()
        .filter(preset::Column::LibraryId.eq(item.library_id))
        .filter(preset::Column::ItemPath.eq(item.path.as_str()))
}

async fn require_preset(
    txn: &DatabaseTransaction,
    item: &item::Model,
    preset_id: i64,
) -> Result<preset::Model> {
    presets_for_item(item)
        .filter(preset::Column::Id.eq(preset_id))
        .one(txn)
        .await
        .with_context(|| format!("select wallpaper preset {preset_id}"))?
        .ok_or_else(|| {
            Error::InvalidArgument(format!(
                "wallpaper preset {preset_id} not found for item {}",
                item.id
            ))
        })
}
