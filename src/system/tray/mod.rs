pub mod dbusmenu;
mod i18n;
mod sni;

use std::sync::Arc;

use anyhow::{anyhow, Result};
use zbus::Connection;

use crate::tasks::{TaskId, TaskKind};
use crate::DaemonContext;

const ITEM_PATH: &str = "/StatusNotifierItem";
pub const MENU_PATH: &str = "/MenuBar";
const WATCHER_SERVICE: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const WATCHER_IFACE: &str = "org.kde.StatusNotifierWatcher";

/// Live tray state. The item and menu are served on their own bus
/// connection rather than the daemon's: SNI hosts drop an icon when
/// the name that registered it vanishes, so closing this connection is
/// what makes hiding the tray work at runtime.
pub struct TrayHandle {
    conn: Connection,
    watcher: TaskId,
}

impl TrayHandle {
    /// Connection hosting `/StatusNotifierItem` and `/MenuBar`.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

/// Register the tray icon. Idempotent: no-op while a tray is up.
/// Best-effort — a missing StatusNotifierWatcher is not an error; the
/// watcher task registers once it appears.
pub async fn ensure_started(app: Arc<DaemonContext>) {
    let mut slot = app.tray.lock().await;
    if slot.is_some() {
        return;
    }
    match start(app.clone()).await {
        Ok(handle) => *slot = Some(handle),
        Err(e) => log::warn!("tray: {e} (continuing without tray)"),
    }
}

/// Remove the tray icon. Idempotent: no-op when not running.
pub async fn ensure_stopped(app: &DaemonContext) {
    // Release the slot before closing so an in-flight menu handler
    // waiting on it sees `None` instead of deadlocking against us.
    let handle = app.tray.lock().await.take();
    let Some(handle) = handle else {
        return;
    };
    app.tasks.cancel(handle.watcher);
    // Dropping the name is the SNI removal signal; hosts clean up on
    // NameOwnerChanged.
    if let Err(e) = handle.conn.close().await {
        log::debug!("tray: close: {e}");
    }
    log::info!("tray: removed {ITEM_PATH}");
}

async fn start(app: Arc<DaemonContext>) -> Result<TrayHandle> {
    let tasks = app.tasks.clone();
    // Own connection, not the daemon's — see `TrayHandle`.
    let conn = Connection::session()
        .await
        .map_err(|e| anyhow!("session bus: {e}"))?;

    // Hosts resolve `IconName` against their own `XDG_DATA_DIRS`.
    // That may miss the AppImage squashfs mount, so expose a theme path.
    let icon_theme_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent()?.parent().map(|p| p.join("share/icons")))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let item = sni::StatusNotifierItem::new(app.clone(), icon_theme_path);
    let menu = dbusmenu::DBusMenu::new(app);

    conn.object_server().at(ITEM_PATH, item).await?;
    conn.object_server().at(MENU_PATH, menu).await?;

    // Watch before the first registration attempt: a watcher that is
    // not up yet (early boot) registers us when it appears, so a
    // failure below is only informational.
    let conn_bg = conn.clone();
    let watcher = tasks.spawn_async(TaskKind::Service, "tray/watcher", async move {
        watch_watcher(conn_bg).await
    });

    match register_with_watcher(&conn).await {
        Ok(()) => log::info!("tray: registered {ITEM_PATH}"),
        Err(e) => log::info!("tray: watcher not available yet: {e}"),
    }

    Ok(TrayHandle { conn, watcher })
}

async fn register_with_watcher(conn: &Connection) -> Result<()> {
    let proxy = zbus::Proxy::new(conn, WATCHER_SERVICE, WATCHER_PATH, WATCHER_IFACE).await?;
    proxy
        .call_method("RegisterStatusNotifierItem", &ITEM_PATH)
        .await
        .map_err(|e| anyhow!("RegisterStatusNotifierItem: {e}"))?;
    Ok(())
}

async fn watch_watcher(conn: Connection) -> Result<()> {
    use futures_util::StreamExt;
    let dbus = zbus::fdo::DBusProxy::new(&conn).await?;
    let mut stream = dbus.receive_name_owner_changed().await?;
    while let Some(sig) = stream.next().await {
        let args = match sig.args() {
            Ok(a) => a,
            Err(_) => continue,
        };
        if args.name.as_str() != WATCHER_SERVICE {
            continue;
        }
        let new_owner = args.new_owner.as_ref().map(|o| o.as_str()).unwrap_or("");
        if !new_owner.is_empty() {
            log::info!("tray: watcher reappeared, re-registering");
            if let Err(e) = register_with_watcher(&conn).await {
                log::warn!("tray: re-register failed: {e}");
            }
        }
    }
    Ok(())
}
