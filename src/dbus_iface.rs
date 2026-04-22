//! Session-bus presence + control surface for the daemon.
//!
//! Consumers (e.g. the KDE wallpaper plugin wrapping
//! `waywallen-display`) watch `org.freedesktop.DBus.NameOwnerChanged` for
//! our well-known name to drive immediate reconnects without waiting for
//! their local backoff timer.
//!
//! The bus is **optional** — if `Connection::session()` fails we log a
//! warning and continue. Nothing in the daemon's data path depends on it.
//!
//! Methods here are thin wrappers around `crate::control`; the tray menu
//! hits those functions directly without bouncing through D-Bus.

use std::sync::Arc;

use zbus::{interface, Connection, SignalContext};

use crate::control;
use crate::tasks::TaskState;
use crate::AppState;

pub const BUS_NAME: &str = "org.waywallen.waywallen.Daemon";
pub const OBJECT_PATH: &str = "/org/waywallen/waywallen/Daemon";

pub struct Daemon1 {
    app: Arc<AppState>,
    display_socket_path: String,
}

#[interface(name = "org.waywallen.waywallen.Daemon1")]
impl Daemon1 {
    #[zbus(property)]
    fn display_socket_path(&self) -> &str {
        &self.display_socket_path
    }

    #[zbus(property)]
    fn ws_port(&self) -> u16 {
        self.app
            .ws_port
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    #[zbus(property)]
    async fn current_wallpaper_id(&self) -> String {
        self.app
            .playlist
            .lock()
            .await
            .current
            .clone()
            .unwrap_or_default()
    }

    async fn open_ui(&self) -> zbus::fdo::Result<()> {
        if !crate::spawn_ui(&self.app) {
            return Err(zbus::fdo::Error::Failed(
                "waywallen-ui not available".into(),
            ));
        }
        Ok(())
    }

    async fn next(&self) -> zbus::fdo::Result<String> {
        control::step(&self.app, 1)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    async fn previous(&self) -> zbus::fdo::Result<String> {
        control::step(&self.app, -1)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    async fn pause(&self) -> zbus::fdo::Result<()> {
        control::pause_all(&self.app)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    async fn resume(&self) -> zbus::fdo::Result<()> {
        control::resume_all(&self.app)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    async fn rescan(&self) -> zbus::fdo::Result<u32> {
        control::rescan(&self.app)
            .await
            .map(|n| n as u32)
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    async fn apply_by_id(&self, id: String) -> zbus::fdo::Result<String> {
        control::apply_wallpaper_by_id(&self.app, &id, 0, 0, 0)
            .await
            .map(|r| r.renderer_id)
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    fn quit(&self) {
        self.app.shutdown_now();
    }

    /// Snapshot of background tasks tracked by the daemon. Returns one
    /// row per task; rows are not sorted (the registry is a HashMap).
    /// Schema: `a(tssxs)` — (id, kind, name, started_at_ms, state).
    /// `state` is one of `running` / `completed` / `failed` /
    /// `cancelled`. For `failed`, the error message is appended after a
    /// colon (e.g. `failed: nope`) so callers can read it without an
    /// extra round-trip.
    /// Cancel a Running task by id. Returns `true` if a cancel token
    /// existed (the task was Running at call time); `false` if the id
    /// is unknown or the task already finished. The task may take a
    /// few ms to observe the cancellation depending on what it's
    /// awaiting.
    fn cancel_task(&self, id: u64) -> bool {
        self.app.tasks.cancel(id)
    }

    fn list_tasks(&self) -> Vec<(u64, String, String, i64, String)> {
        self.app
            .tasks
            .list()
            .into_iter()
            .map(|r| {
                let state_str = match &r.state {
                    TaskState::Failed(msg) => format!("failed: {msg}"),
                    other => other.as_str().to_string(),
                };
                (
                    r.id,
                    r.kind.as_str().to_string(),
                    r.name,
                    r.started_at_ms,
                    state_str,
                )
            })
            .collect()
    }

    #[zbus(signal)]
    async fn ready(emitter: &SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn shutting_down(emitter: &SignalContext<'_>) -> zbus::Result<()>;
}

/// Connect to the session bus, publish the interface, and claim
/// `org.waywallen.waywallen.Daemon`. Returns the `Connection` so the caller can keep
/// it alive for the process lifetime and emit signals through it.
///
/// On any failure (no session bus, name already owned, …) returns `Err`;
/// the caller should log and continue headless.
pub async fn connect(
    app: Arc<AppState>,
    display_socket_path: String,
) -> zbus::Result<Arc<Connection>> {
    let iface = Daemon1 {
        app,
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
