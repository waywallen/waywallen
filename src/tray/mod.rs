//! Linux tray icon — hand-rolled StatusNotifierItem + DBusMenu over zbus.
//!
//! We avoid `ksni` / `libdbusmenu` / GTK: the daemon already ships a zbus
//! session connection, so we serve the two extra object paths on it and
//! call `org.kde.StatusNotifierWatcher.RegisterStatusNotifierItem`.
//!
//! Discovery contract (freedesktop SNI, object-path form):
//!   - we pass the item object path to `RegisterStatusNotifierItem`; the
//!     watcher pairs it with our unique bus name (the message sender) and
//!     exposes `:1.NN/StatusNotifierItem` in `RegisteredStatusNotifierItems`
//!   - item object path: `/StatusNotifierItem`
//!   - menu object path: `/MenuBar` (advertised via `Menu` property)
//!
//! We deliberately do NOT `request_name` `org.kde.StatusNotifierItem-<pid>-1`:
//! modern hosts (Plasma, Waybar, XFCE panel, GNOME AppIndicator, KNotifications,
//! current libappindicator / Chromium / Electron ≥ 23.3) accept the path-only
//! form, and skipping the own-name step lets us run under Flatpak without
//! `--own-name=org.kde.*`.
//!
//! Without a Watcher we record a warning and bail — the daemon keeps running.

mod dbusmenu;
mod sni;

use std::sync::Arc;

use anyhow::{anyhow, Result};
use zbus::Connection;

use crate::AppState;

const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const WATCHER_SERVICE: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const WATCHER_IFACE: &str = "org.kde.StatusNotifierWatcher";

pub async fn spawn(conn: Arc<Connection>, app: Arc<AppState>) -> Result<()> {
    let item = sni::StatusNotifierItem::new(app.clone());
    let menu = dbusmenu::DBusMenu::new(app.clone());

    conn.object_server().at(ITEM_PATH, item).await?;
    conn.object_server().at(MENU_PATH, menu).await?;

    register_with_watcher(&conn).await?;

    // Re-register whenever the watcher (re)appears.
    let conn_bg = conn.clone();
    tokio::spawn(async move {
        if let Err(e) = watch_watcher(conn_bg).await {
            log::warn!("tray watcher monitor exited: {e}");
        }
    });

    log::info!("tray: registered {ITEM_PATH}");
    Ok(())
}

async fn register_with_watcher(conn: &Connection) -> Result<()> {
    let proxy = zbus::Proxy::new(conn, WATCHER_SERVICE, WATCHER_PATH, WATCHER_IFACE).await?;
    proxy
        .call_method("RegisterStatusNotifierItem", &ITEM_PATH)
        .await
        .map_err(|e| anyhow!("RegisterStatusNotifierItem: {e}"))?;
    Ok(())
}

async fn watch_watcher(conn: Arc<Connection>) -> Result<()> {
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
