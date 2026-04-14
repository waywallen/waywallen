//! Session-bus presence for the daemon.
//!
//! Consumers (e.g. the KDE wallpaper plugin wrapping
//! `waywallen-display`) watch `org.freedesktop.DBus.NameOwnerChanged` for
//! our well-known name to drive immediate reconnects without waiting for
//! their local backoff timer.
//!
//! The bus is **optional** — if `Connection::session()` fails we log a
//! warning and continue. Nothing in the daemon's data path depends on it.
use std::sync::Arc;

use zbus::{interface, Connection, SignalContext};

pub const BUS_NAME: &str = "org.waywallen.Daemon";
pub const OBJECT_PATH: &str = "/org/waywallen/Daemon";

pub struct Daemon1 {
    display_socket_path: String,
}

#[interface(name = "org.waywallen.Daemon1")]
impl Daemon1 {
    #[zbus(property)]
    fn display_socket_path(&self) -> &str {
        &self.display_socket_path
    }

    #[zbus(signal)]
    async fn ready(emitter: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn shutting_down(emitter: &SignalContext<'_>) -> zbus::Result<()>;
}

/// Connect to the session bus, publish the interface, and claim
/// `org.waywallen.Daemon`. Returns the `Connection` so the caller can keep
/// it alive for the process lifetime and emit signals through it.
///
/// On any failure (no session bus, name already owned, …) returns `Err`;
/// the caller should log and continue headless.
pub async fn connect(display_socket_path: String) -> zbus::Result<Arc<Connection>> {
    let iface = Daemon1 {
        display_socket_path,
    };
    let conn = zbus::connection::Builder::session()?
        .name(BUS_NAME)?
        .serve_at(OBJECT_PATH, iface)?
        .build()
        .await?;
    Ok(Arc::new(conn))
}

/// Emit the `Ready` signal on the published interface. Safe to call once
/// startup is complete.
pub async fn emit_ready(conn: &Connection) -> zbus::Result<()> {
    let iface_ref = conn
        .object_server()
        .interface::<_, Daemon1>(OBJECT_PATH)
        .await?;
    Daemon1::ready(iface_ref.signal_context()).await
}

/// Emit `ShuttingDown`. Callers should await this before dropping the
/// connection so clients see the signal before the name is released.
pub async fn emit_shutting_down(conn: &Connection) -> zbus::Result<()> {
    let iface_ref = conn
        .object_server()
        .interface::<_, Daemon1>(OBJECT_PATH)
        .await?;
    Daemon1::shutting_down(iface_ref.signal_context()).await
}
