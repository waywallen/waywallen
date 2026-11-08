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

pub const APPLY_FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(15);
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
