use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::RwLock as StdRwLock;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tokio::sync::Notify;

use crate::wallframe::display::layout::{Align, FillMode, Location, Rotation};
use crate::wallframe::display::placement::{CanvasRect, CanvasSize};

mod paths;
mod schema;
mod store;

pub use crate::logging::log_dir;
pub use paths::{
    data_dir, default_config_path, default_db_path, plugin_state_dir, remote_content_dir,
    sanitize_path_segment,
};
pub use schema::*;
pub use store::*;

/// Quiet period after the last `update()` before the debounced writer
/// flushes to disk.
const DEBOUNCE_WRITE: Duration = Duration::from_secs(2);
pub const DEFAULT_AUDIO_FADE_MS: u32 = 500;
pub const MAX_AUDIO_FADE_MS: u32 = 2000;
pub const DEFAULT_AUTO_REPLAY_RESUME_DELAY_MS: u32 = 250;
pub const MAX_AUTO_REPLAY_RESUME_DELAY_MS: u32 = 2000;
pub const RENDERER_ENABLE_AUDIO_KEY: &str = "enable_audio";
pub const RENDERER_VOLUME_KEY: &str = "volume";
pub const MAX_RENDERER_VOLUME: u32 = 100;
pub const MIN_BLUR_EFFECT_RADIUS: u32 = 1;
pub const MAX_BLUR_EFFECT_RADIUS: u32 = 64;
pub const DEFAULT_BLUR_EFFECT_RADIUS: u32 = 30;
pub const MIN_TRANSITION_DURATION_MS: u32 = 100;
pub const MAX_TRANSITION_DURATION_MS: u32 = 10_000;
pub const DEFAULT_TRANSITION_DURATION_MS: u32 = 500;
pub const DEFAULT_TRANSITION_ORIGIN_PERCENT: u32 = 50;

#[cfg(test)]
mod tests;
