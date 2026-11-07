use std::collections::{HashMap, HashSet};

use anyhow::anyhow;

use crate::error::{Error, Result, ResultExt};
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction,
    EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set, TransactionTrait,
};

use super::entities::{item, item_tag, library, source_plugin, tag};
use super::filter;
use crate::tasks::now_ms;
use sea_orm::ActiveValue::NotSet;

pub const LIBRARY_METADATA_MANAGED_KEY: &str = "waywallen.managed";
pub const LIBRARY_METADATA_MANAGED_REMOTE: &str = "remote";

mod items;
mod libraries;
pub mod playlists;
mod plugins;
mod probe;
mod render_properties;
mod tags;

pub use items::*;
pub use libraries::*;
pub use plugins::*;
pub use probe::*;
pub use render_properties::{
    get_user_property_overrides, get_user_property_overrides_raw,
    get_wallpaper_layout_override_with_legacy, get_wallpaper_render_properties,
    set_user_property_override, set_wallpaper_layout_override,
};
pub use tags::*;

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
