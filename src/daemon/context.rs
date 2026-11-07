use std::path::PathBuf;
use std::sync::Arc;

use crate::probe::media::ProbeService;
use crate::wallframe::{display, renderer_manager, routing};
use crate::{events, playback, plugin, settings, system, tasks};

pub(crate) struct DaemonContext {
    pub(crate) renderer_manager: Arc<renderer_manager::RendererManager>,
    pub(super) _audio: system::audio::AudioService,
    pub(crate) source_manager: Arc<plugin::source::SourceManager>,
    pub(crate) qr_login: Arc<plugin::qr_login::QrLoginManager>,
    pub(crate) plugins: Arc<tokio::sync::RwLock<Vec<plugin::renderer_registry::PluginPackageMeta>>>,
    pub(crate) inactive_system: Arc<tokio::sync::RwLock<Vec<String>>>,
    pub(crate) inactive_user: Arc<tokio::sync::RwLock<Vec<String>>>,
    pub(crate) plugin_updates: plugin::update::PluginUpdateStore,
    pub(crate) plugin_update_check: tokio::sync::Mutex<()>,
    pub(crate) plugin_roots: Arc<Vec<plugin::renderer_registry::PluginRoot>>,
    pub(crate) source_plugins: Arc<tokio::sync::RwLock<Vec<plugin::source::SourcePluginInfo>>>,
    pub(crate) plugin_translations:
        Arc<tokio::sync::RwLock<plugin::i18n::PluginTranslationSnapshot>>,
    pub(crate) plugin_mutation: tokio::sync::Mutex<()>,
    pub(crate) autostart: system::autostart::AutostartService,
    pub(crate) router: Arc<routing::Router>,
    pub(crate) display_backend_status: std::sync::RwLock<display::spawner::DisplayBackendStatus>,
    pub(crate) settings: Arc<settings::SettingsStore>,
    pub(crate) system_info: Arc<system::SystemInfo>,
    pub(crate) db: sea_orm::DatabaseConnection,
    pub(crate) queue: tokio::sync::Mutex<playback::QueueState>,
    pub(crate) rotation: playback::RotationHandle,
    pub(crate) events: events::EventBus,
    pub(crate) ws_port: std::sync::atomic::AtomicU16,
    pub(crate) scan_in_progress: std::sync::atomic::AtomicBool,
    pub(crate) scans: Arc<crate::application::ScanCoordinator>,
    pub(crate) ui_path: std::sync::Mutex<Option<PathBuf>>,
    /// Latest tray/host xdg-activation token (SNI `ProvideXdgActivationToken`).
    pub(crate) xdg_activation_token: std::sync::Mutex<Option<String>>,
    pub(crate) dbus_conn: std::sync::Mutex<Option<Arc<zbus::Connection>>>,
    pub(crate) shutdown: tokio::sync::watch::Sender<bool>,
    pub(crate) tasks: Arc<tasks::TaskManager>,
    pub(crate) probe: Arc<ProbeService>,
    pub(crate) playlists: playback::playlist::Engine,
    pub(crate) wallpaper_changes: tokio::sync::watch::Sender<u64>,
    pub(crate) playlist_mutation: tokio::sync::Mutex<()>,
    pub(crate) no_tray: bool,
    pub(crate) tray: tokio::sync::Mutex<Option<system::tray::TrayHandle>>,
}

impl DaemonContext {
    pub fn shutdown_now(&self) {
        let _ = self.shutdown.send(true);
    }

    pub fn shutdown_subscribe(&self) -> tokio::sync::watch::Receiver<bool> {
        self.shutdown.subscribe()
    }
}
