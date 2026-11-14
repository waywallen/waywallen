use std::future::Future;
use std::sync::Arc;

use super::DaemonContext;

/// Spawn the `waywallen-ui` subprocess and reap it asynchronously.
/// The UI reads the WS port from the Daemon1 DBus interface.
pub(crate) fn spawn_ui(state: &DaemonContext) -> bool {
    spawn_ui_with_token(state, "")
}

/// Raise an existing UI if present, otherwise spawn one.
/// Uses a pending SNI xdg-activation token when the tray host provided one
/// (Wayland). On X11 the token is empty and Raise still restores via Qt.
pub(crate) async fn open_or_raise_ui(state: &DaemonContext) -> bool {
    let token = state
        .xdg_activation_token
        .lock()
        .unwrap()
        .take()
        .unwrap_or_default();
    if try_raise_ui(state, &token).await {
        return true;
    }
    spawn_ui_with_token(state, &token)
}

async fn try_raise_ui(state: &DaemonContext, token: &str) -> bool {
    let Some(conn) = state.dbus_conn.lock().unwrap().clone() else {
        return false;
    };
    let proxy = match zbus::Proxy::new(
        conn.as_ref(),
        "org.waywallen.waywallen.UI",
        "/org/waywallen/waywallen/UI",
        "org.waywallen.waywallen.UI1",
    )
    .await
    {
        Ok(proxy) => proxy,
        Err(_) => return false,
    };
    proxy.call_method("Raise", &(token,)).await.is_ok()
}

fn spawn_ui_with_token(state: &DaemonContext, token: &str) -> bool {
    let ui_bin = match state.ui_path.lock().unwrap().clone() {
        Some(path) => path,
        None => return false,
    };
    log::info!("launching ui: {}", ui_bin.display());
    let mut cmd = tokio::process::Command::new(&ui_bin);
    if !token.is_empty() {
        cmd.env("XDG_ACTIVATION_TOKEN", token);
    }
    match cmd.spawn() {
        Ok(child) => {
            tokio::spawn(wait_for_ui(child));
            true
        }
        Err(error) => {
            log::warn!("failed to launch ui {}: {error}", ui_bin.display());
            false
        }
    }
}

async fn wait_for_ui(mut child: tokio::process::Child) {
    let pid = child.id().expect("newly spawned UI has a PID");
    log::info!("ui pid: {pid}");
    match child.wait().await {
        Ok(status) if status.success() => log::info!("ui {pid} exited: {status}"),
        Ok(status) => log::warn!("ui {pid} exited: {status}"),
        Err(error) => log::warn!("failed to wait for ui {pid}: {error}"),
    }
}

pub(super) async fn run_until_shutdown<F>(
    state: Arc<DaemonContext>,
    websocket: F,
    dbus: Arc<zbus::Connection>,
) -> anyhow::Result<()>
where
    F: Future<Output = anyhow::Result<()>>,
{
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::pin!(websocket);

    let websocket_exited = tokio::select! {
        result = &mut websocket => {
            if let Err(error) = result {
                log::error!("ws server exited with error: {error}");
            }
            true
        }
        _ = tokio::signal::ctrl_c() => {
            log::info!("SIGINT received, shutting down");
            false
        }
        Some(()) = sigterm.recv() => {
            log::info!("SIGTERM received, shutting down");
            false
        }
        result = crate::system::dbus::wait_for_disconnect(&dbus) => {
            match result {
                Ok(()) => log::info!("D-Bus user bus connection closed, shutting down"),
                Err(error) => {
                    log::info!("D-Bus user bus connection closed: {error}; shutting down")
                }
            }
            false
        }
        Ok(_) = async {
            let mut receiver = state.shutdown_subscribe();
            receiver.wait_for(|requested| *requested).await
        } => {
            log::info!("shutdown requested");
            false
        }
    };

    state.shutdown_now();
    if !websocket_exited {
        if let Err(error) = websocket.await {
            log::warn!("ws server shutdown failed: {error}");
        }
    }
    crate::system::tray::ensure_stopped(&state).await;
    if let Err(error) = state.qr_login.cancel_all_and_wait().await {
        log::warn!("QR login shutdown cleanup failed: {error:#}");
    }
    state.playlists.shutdown().await;
    state.tasks.wait_stopped().await;
    let renderer_ids = state
        .router
        .snapshot_renderers()
        .await
        .into_iter()
        .map(|renderer| renderer.id)
        .collect::<Vec<_>>();
    state
        .router
        .stop_renderers_orderly(&renderer_ids, std::time::Duration::from_secs(1))
        .await;
    state.renderer_manager.shutdown().await;
    state.settings.stop_writer().await;
    state.settings.flush_now().await;

    if let Err(error) = crate::system::dbus::emit_shutting_down(&dbus).await {
        log::warn!("DBus ShuttingDown emit failed: {error}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::wait_for_ui;
    use std::process::Stdio;
    use std::time::Duration;
    use tokio::process::Command;

    async fn assert_reaped(child: tokio::process::Child) {
        let pid = child.id().unwrap() as libc::pid_t;
        tokio::time::timeout(Duration::from_secs(5), tokio::spawn(wait_for_ui(child)))
            .await
            .expect("UI wait task timed out")
            .expect("UI wait task panicked");
        let result = unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) };
        let error = std::io::Error::last_os_error();
        assert_eq!(result, -1, "UI child was not reaped");
        assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
    }

    #[tokio::test]
    async fn ui_children_are_reaped_after_exit() {
        for _ in 0..4 {
            for script in ["exit 0", "exit 7", "kill -TERM $$"] {
                let child = Command::new("sh").args(["-c", script]).spawn().unwrap();
                assert_reaped(child).await;
            }
        }
    }

    #[tokio::test]
    async fn waiting_for_ui_does_not_block_other_children() {
        let mut child = Command::new("sh")
            .args(["-c", "read line; exit 0"])
            .stdin(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let waiting = tokio::spawn(assert_reaped(child));
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());

        let child = Command::new("sh").args(["-c", "exit 0"]).spawn().unwrap();
        assert_reaped(child).await;
        assert!(!waiting.is_finished());
        drop(stdin);
        waiting.await.unwrap();
    }
}