use std::sync::Arc;

use zbus::{interface, zvariant::OwnedObjectPath, SignalContext};

use crate::application;
use crate::DaemonContext;

pub struct StatusNotifierItem {
    app: Arc<DaemonContext>,
    icon_theme_path: String,
}

impl StatusNotifierItem {
    pub fn new(app: Arc<DaemonContext>, icon_theme_path: String) -> Self {
        Self {
            app,
            icon_theme_path,
        }
    }
}

/// Tooltip tuple: `(icon_name, icon_data, title, body)`.
/// Empty icon data keeps hosts from trying to render a custom pixmap.
type ToolTip = (String, Vec<(i32, i32, Vec<u8>)>, String, String);

#[interface(name = "org.kde.StatusNotifierItem")]
impl StatusNotifierItem {
    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }

    #[zbus(property)]
    fn id(&self) -> &str {
        "waywallen"
    }

    #[zbus(property)]
    fn title(&self) -> &str {
        "waywallen"
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }

    #[zbus(property)]
    fn window_id(&self) -> u32 {
        0
    }

    #[zbus(property)]
    fn icon_name(&self) -> &str {
        "org.waywallen.waywallen"
    }

    #[zbus(property)]
    fn icon_theme_path(&self) -> &str {
        &self.icon_theme_path
    }

    #[zbus(property)]
    fn icon_pixmap(&self) -> Vec<(i32, i32, Vec<u8>)> {
        Vec::new()
    }

    #[zbus(property)]
    fn overlay_icon_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn overlay_icon_pixmap(&self) -> Vec<(i32, i32, Vec<u8>)> {
        Vec::new()
    }

    #[zbus(property)]
    fn attention_icon_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn attention_icon_pixmap(&self) -> Vec<(i32, i32, Vec<u8>)> {
        Vec::new()
    }

    #[zbus(property)]
    fn attention_movie_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn tool_tip(&self) -> ToolTip {
        (
            String::new(),
            Vec::new(),
            "waywallen".to_string(),
            super::i18n::tr("Linux wallpaper daemon").to_string(),
        )
    }

    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from("/MenuBar").unwrap()
    }

    /// Left-click. Hosts that honour `ItemIsMenu=true` will show the menu
    /// instead of calling this, but some fall back anyway.
    async fn activate(&self, _x: i32, _y: i32) {
        if let Err(e) = open_ui(&self.app).await {
            log::warn!("tray activate: {e}");
        }
    }

    async fn secondary_activate(&self, _x: i32, _y: i32) {
        if let Err(e) = application::step_active(&self.app, 1).await {
            log::warn!("tray secondary activate: {e}");
        }
    }

    async fn context_menu(&self, _x: i32, _y: i32) {
        // Host renders the menu on its own from the DBusMenu object; nothing to do.
    }

    /// Optional Wayland xdg-activation token from the StatusNotifier host
    /// (`ProvideXdgActivationToken`). Unused on X11; empty is fine there.
    async fn provide_xdg_activation_token(&self, token: String) {
        *self.app.xdg_activation_token.lock().unwrap() = Some(token);
    }

    /// Scroll wheel on the icon: ±1 through the playlist.
    async fn scroll(&self, delta: i32, orientation: String) {
        let _ = orientation;
        let step = if delta >= 0 { 1 } else { -1 };
        if let Err(e) = application::step_active(&self.app, step).await {
            log::warn!("tray scroll: {e}");
        }
    }

    #[zbus(signal)]
    pub async fn new_title(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn new_icon(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn new_attention_icon(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn new_overlay_icon(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn new_tool_tip(ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn new_status(ctxt: &SignalContext<'_>, status: &str) -> zbus::Result<()>;
}

async fn open_ui(app: &Arc<DaemonContext>) -> anyhow::Result<()> {
    if !crate::open_or_raise_ui(app).await {
        anyhow::bail!("waywallen-ui not available");
    }
    Ok(())
}
