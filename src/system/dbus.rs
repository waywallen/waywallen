use std::path::Path;
use std::sync::Arc;

use futures_util::StreamExt;
use zbus::fdo::RequestNameFlags;
use zbus::names::WellKnownName;
use zbus::{interface, Connection, SignalContext};

use crate::application;
use crate::tasks::TaskState;
use crate::DaemonContext;

pub const BUS_NAME: &str = "org.waywallen.waywallen.Daemon";
pub const OBJECT_PATH: &str = "/org/waywallen/waywallen/Daemon";

pub struct Daemon1 {
    app: Arc<DaemonContext>,
    display_socket_path: String,
}

#[interface(name = "org.waywallen.waywallen.Daemon1")]
impl Daemon1 {
    /// Crate version (Cargo.toml). UI compares this against its own
    /// build-time `APP_VERSION` to gate the connection — mismatched
    #[zbus(property)]
    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    /// Control-plane revision plus the optional features this daemon
    /// serves. First entry is the revision (`control.v1`); a client
    /// gates on that instead of on `Version`, then checks optional
    /// features by name before using them. A daemon that predates the
    /// property answers `UnknownProperty`, which means "old daemon",
    /// not "no capabilities".
    #[zbus(property)]
    fn capabilities(&self) -> Vec<String> {
        crate::api::CONTROL_CAPABILITIES
            .iter()
            .map(|name| (*name).to_owned())
            .collect()
    }

    #[zbus(property)]
    fn display_socket_path(&self) -> &str {
        &self.display_socket_path
    }

    #[zbus(property)]
    fn ws_port(&self) -> u16 {
        self.app.ws_port.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[zbus(property)]
    async fn current_wallpaper_id(&self) -> String {
        self.app
            .queue
            .lock()
            .await
            .current
            .clone()
            .unwrap_or_default()
    }

    /// Live queue playback mode: `"sequential"`, `"shuffle"`, or
    /// `"random"`. Setter persists to settings and emits
    #[zbus(property)]
    async fn queue_mode(&self) -> String {
        self.app.queue.lock().await.mode.as_str().to_owned()
    }

    #[zbus(property)]
    async fn set_queue_mode(&self, value: String) -> zbus::Result<()> {
        let mode =
            crate::playback::Mode::from_str(&value).ok_or_else(|| zbus::Error::InvalidField)?;
        application::set_mode(&self.app, mode).await;
        Ok(())
    }

    /// Auto-rotation interval in seconds; `0` disables.
    #[zbus(property)]
    fn rotation_secs(&self) -> u32 {
        self.app.rotation.interval()
    }

    #[zbus(property)]
    async fn set_rotation_secs(&self, secs: u32) {
        application::set_rotation_interval(&self.app, secs).await;
    }

    async fn open_ui(&self) -> zbus::fdo::Result<()> {
        if !crate::open_or_raise_ui(&self.app).await {
            return Err(zbus::fdo::Error::Failed(
                "waywallen-ui not available".into(),
            ));
        }
        Ok(())
    }

    async fn next(&self) -> zbus::fdo::Result<()> {
        application::step_active(&self.app, 1)
            .await
            .map_err(zbus::fdo::Error::from)
    }

    async fn previous(&self) -> zbus::fdo::Result<()> {
        application::step_active(&self.app, -1)
            .await
            .map_err(zbus::fdo::Error::from)
    }

    async fn pause(&self) -> zbus::fdo::Result<()> {
        application::pause_all(&self.app)
            .await
            .map_err(zbus::fdo::Error::from)
    }

    async fn resume(&self) -> zbus::fdo::Result<()> {
        application::resume_all(&self.app)
            .await
            .map_err(zbus::fdo::Error::from)
    }

    async fn mute(&self) -> zbus::fdo::Result<()> {
        application::mute_all(&self.app)
            .await
            .map_err(zbus::fdo::Error::from)
    }

    async fn unmute(&self) -> zbus::fdo::Result<()> {
        application::unmute_all(&self.app)
            .await
            .map_err(zbus::fdo::Error::from)
    }

    async fn rescan(&self) -> zbus::fdo::Result<u32> {
        application::rescan(&self.app)
            .await
            .map(|n| n as u32)
            .map_err(zbus::fdo::Error::from)
    }

    async fn refresh_displays(
        &self,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
    ) -> zbus::fdo::Result<()> {
        log::info!("display refresh requested; emitting DBus Ready");
        Self::ready(&ctxt).await?;
        Ok(())
    }

    async fn apply_by_id(&self, id: String) -> zbus::fdo::Result<String> {
        application::apply_wallpaper_by_id(&self.app, &id, application::ApplySource::UserWallpaper)
            .await
            .map(|r| r.renderer_id)
            .map_err(zbus::fdo::Error::from)
    }

    /// Apply an image wallpaper via `org.freedesktop.portal.Wallpaper`.
    /// Returns the portal URI used for the request.
    async fn apply_via_portal(&self, id: String) -> zbus::fdo::Result<String> {
        application::apply_wallpaper_via_portal(
            &self.app,
            &id,
            application::ApplySource::UserWallpaper,
        )
        .await
        .map(|r| r.uri)
        .map_err(zbus::fdo::Error::from)
    }

    /// Toggle shuffle on the active playlist. Persisted to settings so
    /// it survives restart.
    async fn set_shuffle(&self, on: bool) {
        application::set_shuffle(&self.app, on).await;
    }

    /// Set the auto-rotation interval in seconds. `0` disables.
    async fn set_rotation_interval(&self, secs: u32) {
        application::set_rotation_interval(&self.app, secs).await;
    }

    /// Live status of the active queue. Tuple shape
    /// `(active_id, mode, interval_secs, current_id, position, count, is_smart)`.
    async fn queue_status(&self) -> (i64, String, u32, String, u32, u32, bool) {
        let s = application::queue_status(&self.app).await;
        (
            s.active_id.unwrap_or(0),
            s.mode,
            s.interval_secs,
            s.current.unwrap_or_default(),
            s.position.unwrap_or(0),
            s.count,
            s.is_smart,
        )
    }

    fn quit(&self) {
        self.app.shutdown_now();
    }

    /// Snapshot of background tasks tracked by the daemon. Returns one
    /// row per task; rows are not sorted (the registry is a HashMap).
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

/// Single-instance gate.
/// Claims `BUS_NAME` with `DO_NOT_QUEUE`, or execs the UI (keeps activation token).
pub async fn acquire_or_handoff(ui_path: Option<&Path>) -> Connection {
    let conn = match Connection::session().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("waywallen: dbus session bus unavailable: {e}");
            std::process::exit(1);
        }
    };

    let name: WellKnownName<'_> = WellKnownName::try_from(BUS_NAME).expect("valid bus name");
    // zbus 4 maps `Exists` / `InQueue` to error variants; only the
    // primary-owner reply means this process owns the name.
    match conn
        .request_name_with_flags(name, RequestNameFlags::DoNotQueue.into())
        .await
    {
        Ok(_) => conn,
        Err(zbus::Error::NameTaken) => {
            let Some(ui) = ui_path else {
                eprintln!("waywallen: already running, no UI to launch");
                std::process::exit(0);
            };
            log::info!("waywallen already running; exec into {}", ui.display());
            use std::os::unix::process::CommandExt;
            let err = std::process::Command::new(ui).exec();
            eprintln!("waywallen: exec {} failed: {err}", ui.display());
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("waywallen: dbus RequestName failed: {e}");
            std::process::exit(1);
        }
    }
}

/// Publish the `Daemon1` interface on a connection that already owns
/// `BUS_NAME` (returned by [`acquire_or_handoff`]).
pub async fn serve(
    conn: Connection,
    app: Arc<DaemonContext>,
    display_socket_path: String,
) -> zbus::Result<Arc<Connection>> {
    let iface = Daemon1 {
        app,
        display_socket_path,
    };
    conn.object_server().at(OBJECT_PATH, iface).await?;
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

pub async fn wait_for_disconnect(conn: &Connection) -> zbus::Result<()> {
    let mut messages = zbus::MessageStream::from(conn);
    while let Some(message) = messages.next().await {
        message?;
    }
    Ok(())
}

/// Snapshot of the dbus connection from `DaemonContext`. Callers can take
/// it across an await without holding the std::sync::Mutex.
fn live_conn(app: &DaemonContext) -> Option<Arc<Connection>> {
    app.dbus_conn.lock().unwrap().clone()
}

pub async fn notify_queue_mode_changed(app: &DaemonContext) {
    let conn = match live_conn(app) {
        Some(c) => c,
        None => return,
    };
    let iface = match conn
        .object_server()
        .interface::<_, Daemon1>(OBJECT_PATH)
        .await
    {
        Ok(i) => i,
        Err(_) => return,
    };
    let inner = iface.get().await;
    let _ = inner.queue_mode_changed(iface.signal_context()).await;
}

pub async fn notify_rotation_secs_changed(app: &DaemonContext) {
    let conn = match live_conn(app) {
        Some(c) => c,
        None => return,
    };
    let iface = match conn
        .object_server()
        .interface::<_, Daemon1>(OBJECT_PATH)
        .await
    {
        Ok(i) => i,
        Err(_) => return,
    };
    let inner = iface.get().await;
    let _ = inner.rotation_secs_changed(iface.signal_context()).await;
}

pub async fn notify_current_wallpaper_id_changed(app: &DaemonContext) {
    let conn = match live_conn(app) {
        Some(c) => c,
        None => return,
    };
    let iface = match conn
        .object_server()
        .interface::<_, Daemon1>(OBJECT_PATH)
        .await
    {
        Ok(i) => i,
        Err(_) => return,
    };
    let inner = iface.get().await;
    let _ = inner
        .current_wallpaper_id_changed(iface.signal_context())
        .await;
}
