use anyhow::anyhow;
use mlua::prelude::*;
use sea_orm::DatabaseConnection;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use crate::catalog::entry::{WallpaperEntry, WallpaperType};
use crate::error::{Error, Result};
use crate::model::repo;
use crate::probe::media::{AvFormatProbe, MediaProbe};

mod filesystem;
mod manager;
mod parsing;
mod runtime;
mod types;

pub use manager::*;
pub use runtime::{LuaPluginRuntime, WallpaperApply};
pub use types::*;

#[cfg(test)]
mod image_tests;
#[cfg(test)]
mod tests;
