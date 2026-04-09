//! Display endpoint — accepts external display client connections on a
//! Unix socket and streams DMA-BUF metadata + frame events to them.
//!
//! NOTE (Phase 1 rename): this file was renamed from `viewer_endpoint`
//! to `display_endpoint` to align with the new `waywallen-display-v1`
//! naming. The wire format on this socket is still the legacy
//! serde-based `ViewerMsg` / `EventMsg` until the protocol swap lands;
//! only the file name, module name, and socket path were changed.
//!
//! Wire protocol (legacy, to be replaced):
//!
//!   1. Client sends `ViewerMsg::Hello { client, version }`.
//!   2. Client sends `ViewerMsg::Subscribe { renderer_id }`.
//!   3. Server replies with one `EventMsg::BindBuffers` carrying N dup'd
//!      DMA-BUF FDs as SCM_RIGHTS ancillary data, sourced from the
//!      RendererManager's cached `BindSnapshot`. If the renderer hasn't
//!      produced a `BindBuffers` yet, the server waits up to 10 s.
//!   4. Server then forwards every `EventMsg::FrameReady` from the
//!      renderer's broadcast channel to the client until the client
//!      disconnects or the renderer dies.

use anyhow::{anyhow, Context, Result};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

use crate::ipc::proto::{EventMsg, ViewerMsg};
use crate::ipc::uds::{recv_msg, send_msg};
use crate::renderer_manager::RendererManager;

/// Default UDS path. Falls back to `/tmp/waywallen/display.sock` if
/// `XDG_RUNTIME_DIR` is unset.
pub fn default_socket_path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let dir = runtime.join("waywallen");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("display.sock")
}

/// Bind a listening UDS at `sock_path` (unlinking any stale file first)
/// and run the accept loop forever, spawning one task per client.
///
/// Returns only on bind error; the accept loop never terminates.
pub async fn serve(sock_path: &Path, mgr: Arc<RendererManager>) -> Result<()> {
    let _ = std::fs::remove_file(sock_path);
    if let Some(parent) = sock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let listener = tokio::net::UnixListener::bind(sock_path)
        .with_context(|| format!("bind display socket at {}", sock_path.display()))?;
    log::info!("display endpoint listening on {}", sock_path.display());

    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(x) => x,
            Err(e) => {
                log::warn!("display accept failed: {e}");
                continue;
            }
        };
        let std_stream = match stream
            .into_std()
            .and_then(|s| s.set_nonblocking(false).map(|_| s))
        {
            Ok(s) => s,
            Err(e) => {
                log::warn!("display into_std failed: {e}");
                continue;
            }
        };
        let mgr = Arc::clone(&mgr);
        tokio::spawn(async move {
            if let Err(e) = handle_client(std_stream, mgr).await {
                log::info!("display client closed: {e}");
            }
        });
    }
}

async fn handle_client(stream: StdUnixStream, mgr: Arc<RendererManager>) -> Result<()> {
    // Read Hello.
    let stream_for_hello = stream
        .try_clone()
        .context("clone stream for hello read")?;
    let (hello, _fds): (ViewerMsg, _) = tokio::task::spawn_blocking(move || {
        recv_msg::<ViewerMsg>(&stream_for_hello)
    })
    .await
    .context("hello read join")??;
    let _ = match hello {
        ViewerMsg::Hello { client, version } => (client, version),
        other => return Err(anyhow!("expected Hello, got {other:?}")),
    };

    // Read Subscribe.
    let stream_for_sub = stream
        .try_clone()
        .context("clone stream for subscribe read")?;
    let (sub, _fds): (ViewerMsg, _) = tokio::task::spawn_blocking(move || {
        recv_msg::<ViewerMsg>(&stream_for_sub)
    })
    .await
    .context("subscribe read join")??;
    let renderer_id = match sub {
        ViewerMsg::Subscribe { renderer_id } => renderer_id,
        other => return Err(anyhow!("expected Subscribe, got {other:?}")),
    };

    let handle = mgr
        .get(&renderer_id)
        .await
        .ok_or_else(|| anyhow!("unknown renderer id: {renderer_id}"))?;

    // Subscribe to the broadcast BEFORE we send BindBuffers, so any
    // FrameReady that races us still lands in our receiver buffer.
    let mut events: broadcast::Receiver<EventMsg> = handle.events();

    // Wait for BindSnapshot to be available. Bounded loop so a stuck
    // renderer doesn't tie up the connection forever.
    let snapshot_arc = handle.bind_snapshot();
    let mut waited = Duration::ZERO;
    let step = Duration::from_millis(50);
    let max = Duration::from_secs(10);
    let snapshot_clone: (
        crate::ipc::proto::EventMsg,
        Vec<RawFd>,
    ) = loop {
        let payload = {
            let guard = snapshot_arc
                .lock()
                .map_err(|e| anyhow!("snapshot mutex poisoned: {e}"))?;
            guard.as_ref().map(|snap| {
                let dups: Result<Vec<RawFd>, _> = snap
                    .fds
                    .iter()
                    .map(|fd| nix::unistd::dup(fd.as_raw_fd()))
                    .collect();
                dups.map(|fds| {
                    (
                        EventMsg::BindBuffers {
                            count: snap.count,
                            fourcc: snap.fourcc,
                            width: snap.width,
                            height: snap.height,
                            stride: snap.stride,
                            modifier: snap.modifier,
                            plane_offset: snap.plane_offset,
                            sizes: snap.sizes.clone(),
                        },
                        fds,
                    )
                })
            })
        };
        if let Some(res) = payload {
            break res.context("dup snapshot fds")?;
        }
        if waited >= max {
            return Err(anyhow!(
                "renderer {renderer_id} produced no BindBuffers within 10s"
            ));
        }
        tokio::time::sleep(step).await;
        waited += step;
    };

    let (bind_msg, dup_fds) = snapshot_clone;

    // Send BindBuffers + dup'd fds, then close those fds (the kernel kept
    // its own references for the message in flight).
    let stream_for_bind = stream
        .try_clone()
        .context("clone stream for BindBuffers send")?;
    let dup_fds_owned = dup_fds.clone();
    let bind_msg_clone = bind_msg.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        send_msg(&stream_for_bind, &bind_msg_clone, &dup_fds_owned)?;
        // Close the dup'd fds; the receiver got their own copies.
        for fd in dup_fds_owned {
            // SAFETY: we own each dup'd fd; closing exactly once.
            unsafe { libc::close(fd) };
        }
        Ok(())
    })
    .await
    .context("BindBuffers send join")??;

    // Forward FrameReady events. Drop other event types.
    loop {
        let event = match events.recv().await {
            Ok(ev) => ev,
            Err(broadcast::error::RecvError::Closed) => {
                return Err(anyhow!("renderer broadcast closed"));
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                log::warn!("display client lagged {n} frames; continuing");
                continue;
            }
        };
        // Only forward FrameReady; everything else is internal to the
        // daemon's bookkeeping.
        if !matches!(event, EventMsg::FrameReady { .. }) {
            continue;
        }
        let stream_for_frame = stream
            .try_clone()
            .context("clone stream for FrameReady send")?;
        let result = tokio::task::spawn_blocking(move || {
            send_msg(&stream_for_frame, &event, &[])
        })
        .await
        .context("FrameReady send join")?;
        if let Err(e) = result {
            return Err(anyhow!("FrameReady send failed: {e}"));
        }
    }
}
