use std::time::Duration;

use crate::catalog::entry::WallpaperEntry;
use crate::wallframe::scheduler::DisplayId;

mod catalog;
pub mod playback;
mod plugins;
pub mod wallpaper_query;

pub use catalog::*;
pub use playback::*;
pub use plugins::*;

/// Default time the daemon waits for a renderer to produce its first frame.
pub const DEFAULT_APPLY_FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(15);

/// Time the daemon waits for a renderer to produce its first frame.
///
/// Heavy scenes can take longer than the default to load on older hardware.
/// Set `WAYWALLEN_FIRST_FRAME_TIMEOUT_SECS` to override it (whole seconds,
/// greater than zero). Invalid values fall back to the default.
pub fn apply_first_frame_timeout() -> Duration {
    static TIMEOUT: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
    *TIMEOUT.get_or_init(
        || match std::env::var("WAYWALLEN_FIRST_FRAME_TIMEOUT_SECS") {
            Ok(v) => match v.trim().parse::<u64>() {
                Ok(secs) if secs > 0 => Duration::from_secs(secs),
                _ => {
                    tracing::warn!(
                        "ignoring invalid WAYWALLEN_FIRST_FRAME_TIMEOUT_SECS={v:?}; using default"
                    );
                    DEFAULT_APPLY_FIRST_FRAME_TIMEOUT
                }
            },
            Err(_) => DEFAULT_APPLY_FIRST_FRAME_TIMEOUT,
        },
    )
}
const PLUGIN_UPDATE_NOTIFICATION_ID: &str = "org.waywallen.waywallen.plugin-updates";

pub struct ApplyResult {
    pub renderer_id: String,
    pub entry: WallpaperEntry,
    pub activation: ApplyActivation,
    pub stopped_playlists: Vec<StoppedPlaylist>,
    pub targets: Vec<ApplyTarget>,
    pub display_ids: Vec<DisplayId>,
}

pub struct StoppedPlaylist {
    pub id: i64,
    pub name: String,
    pub display_ids: Vec<DisplayId>,
    pub all_displays: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyActivation {
    Active,
    Deferred,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplySource {
    UserWallpaper,
    UserQueueStep,
    UserPlaylistActivation,
    UserPlaylistJump,
    UserPlaylistStep,
    QueueRotation,
    PlaylistRotation,
    PlaylistRebuild,
    StartupRestore,
    DisplayRecall,
    PlaylistAttach,
}

impl ApplySource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserWallpaper => "user-wallpaper",
            Self::UserQueueStep => "user-queue-step",
            Self::UserPlaylistActivation => "user-playlist-activation",
            Self::UserPlaylistJump => "user-playlist-jump",
            Self::UserPlaylistStep => "user-playlist-step",
            Self::QueueRotation => "queue-rotation",
            Self::PlaylistRotation => "playlist-rotation",
            Self::PlaylistRebuild => "playlist-rebuild",
            Self::StartupRestore => "startup-restore",
            Self::DisplayRecall => "display-recall",
            Self::PlaylistAttach => "playlist-attach",
        }
    }

    pub fn preempts_pending_start(self) -> bool {
        !matches!(
            self,
            Self::QueueRotation | Self::PlaylistRotation | Self::PlaylistRebuild
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RendererSharingPolicy {
    #[default]
    UseSettings,
    Shared,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ApplyTarget {
    Display(DisplayId),
    Canvas(String),
}

#[derive(Clone, Debug)]
pub struct ApplyRequest {
    pub source: ApplySource,
    pub targets: Option<Vec<ApplyTarget>>,
    pub renderer_name: Option<String>,
    pub first_frame_timeout: Option<Duration>,
    pub require_display: bool,
    pub sharing: RendererSharingPolicy,
}

fn should_duplicate_renderers(
    setting_enabled: bool,
    has_targets: bool,
    sharing: RendererSharingPolicy,
) -> bool {
    setting_enabled && has_targets && sharing == RendererSharingPolicy::UseSettings
}

mod scan;
#[cfg(test)]
mod tests;
use catalog::run_scan_pass;
pub(crate) use scan::{run_media_input_watcher, ScanCoordinator};
