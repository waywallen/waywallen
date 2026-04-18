//! Display endpoint — accepts external display client connections on a
//! Unix socket and speaks the `waywallen-display-v1` protocol with them.
//!
//! Phase 1 split: this file is now a thin wire layer. All scheduling
//! decisions (which renderer feeds which display, when to bind/unbind,
//! how to project SetConfig) live in `crate::routing::Router`. The
//! endpoint just:
//!
//!   1. Performs the protocol handshake.
//!   2. Calls `router.register_display(...)` and gets back a
//!      `DisplayHandle { id, rx }`.
//!   3. Translates `DisplayOutEvent`s from `rx` into wire `Event`s.
//!   4. Forwards client requests (BufferRelease/UpdateDisplay/Bye)
//!      back into the router.
//!
//! No `RendererHandle` references remain in the per-client state
//! machine — the router owns all renderer subscriptions.

use anyhow::{anyhow, Context, Result};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::display_proto::generated::Rect;
use crate::display_proto::{codec, opcode, Event, Request, PROTOCOL_NAME};
use crate::dummy_fence;
use crate::renderer_manager::{BindSnapshot, RendererHandle};
use crate::routing::{DisplayHandle, DisplayOutEvent, DisplayRegistration, Router};
use crate::scheduler::ProjectedConfig;

/// Server version string advertised in `welcome.server_version`.
pub const SERVER_VERSION: &str = concat!("waywallen ", env!("CARGO_PKG_VERSION"));

/// v1 mandatory feature flags.
const MANDATORY_FEATURES: &[&str] = &["explicit_sync_fd"];

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn default_socket_path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let dir = runtime.join("waywallen");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("display.sock")
}

/// Back-compat 2-arg entry point used by integration tests that
/// don't care about daemon-level shutdown. Internally forwards to
/// [`serve_with_shutdown`] with a never-firing channel so the fast
/// path in production (D-Bus `Quit` → kick every blocking `recvmsg`)
/// goes through the same code.
pub async fn serve(sock_path: &Path, router: Arc<Router>) -> Result<()> {
    // Holding `_never_tx` in scope keeps `wait_for` parked on `Pending`
    // — if we dropped it, every subscriber would see `RecvError::Closed`
    // and the shutdown branch would fire immediately.
    let (_never_tx, rx) = tokio::sync::watch::channel(false);
    let res = serve_with_shutdown(sock_path, router, rx).await;
    drop(_never_tx);
    res
}

pub async fn serve_with_shutdown(
    sock_path: &Path,
    router: Arc<Router>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let _ = std::fs::remove_file(sock_path);
    if let Some(parent) = sock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let listener = tokio::net::UnixListener::bind(sock_path)
        .with_context(|| format!("bind display socket at {}", sock_path.display()))?;
    log::info!("display endpoint listening on {}", sock_path.display());

    loop {
        let accepted = tokio::select! {
            biased;
            _ = wait_shutdown(&mut shutdown_rx) => {
                log::info!("display endpoint: shutdown received, ceasing accept");
                return Ok(());
            }
            res = listener.accept() => res,
        };
        let (stream, _addr) = match accepted {
            Ok(x) => x,
            Err(e) => {
                log::warn!("display accept failed: {e}");
                continue;
            }
        };
        let std_stream = match stream.into_std().and_then(|s| {
            s.set_nonblocking(false).map(|_| s)
        }) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("display into_std failed: {e}");
                continue;
            }
        };
        let router = Arc::clone(&router);
        let client_shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(std_stream, router, client_shutdown_rx).await {
                log::info!("display client closed: {e}");
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Per-client state machine
// ---------------------------------------------------------------------------

async fn handle_client(
    stream: StdUnixStream,
    router: Arc<Router>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let registration = do_handshake(&stream, &mut shutdown_rx).await?;
    let DisplayHandle { id: display_id, rx } = router.register_display(registration).await;
    log::info!("display {display_id} registered with router");

    let send_ack_stream = stream.try_clone().context("clone for accepted")?;
    tokio::task::spawn_blocking(move || {
        codec::send_event(&send_ack_stream, &Event::DisplayAccepted { display_id }, &[])
    })
    .await
    .context("accepted join")?
    .map_err(|e| anyhow!("send display_accepted: {e}"))?;

    let result = run_frame_loop(stream, router.clone(), display_id, rx, shutdown_rx).await;
    router.unregister_display(display_id).await;
    result
}

// ---------------------------------------------------------------------------
// Handshake
// ---------------------------------------------------------------------------

async fn do_handshake(
    stream: &StdUnixStream,
    shutdown_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<DisplayRegistration> {
    let (hello, _fds): (Request, _) = recv_request_cancellable(stream, shutdown_rx)
        .await
        .context("recv hello")?;
    let Request::Hello {
        protocol,
        client_name,
        client_version,
    } = hello
    else {
        return Err(anyhow!("expected hello, got opcode {}", hello.opcode()));
    };
    if protocol != PROTOCOL_NAME {
        let s = stream.try_clone().context("clone for error")?;
        let msg = format!("unsupported protocol: {protocol}");
        let err_msg = msg.clone();
        let _ = tokio::task::spawn_blocking(move || {
            codec::send_event(
                &s,
                &Event::Error {
                    code: 1,
                    message: err_msg,
                },
                &[],
            )
        })
        .await;
        return Err(anyhow!("bad protocol string: {msg}"));
    }
    log::info!("display hello: {client_name} v{client_version}");

    let welcome_stream = stream.try_clone().context("clone for welcome")?;
    tokio::task::spawn_blocking(move || {
        codec::send_event(
            &welcome_stream,
            &Event::Welcome {
                server_version: SERVER_VERSION.to_string(),
                features: MANDATORY_FEATURES.iter().map(|s| s.to_string()).collect(),
            },
            &[],
        )
    })
    .await
    .context("welcome join")?
    .map_err(|e| anyhow!("send welcome: {e}"))?;

    let (reg, _fds): (Request, _) = recv_request_cancellable(stream, shutdown_rx)
        .await
        .context("recv register_display")?;
    let Request::RegisterDisplay {
        name,
        width,
        height,
        refresh_mhz,
        properties,
    } = reg
    else {
        return Err(anyhow!(
            "expected register_display, got opcode {}",
            reg.opcode()
        ));
    };
    Ok(DisplayRegistration {
        name,
        width,
        height,
        refresh_mhz,
        properties,
    })
}

// ---------------------------------------------------------------------------
// Frame loop — translate DisplayOutEvent → wire Event
// ---------------------------------------------------------------------------

async fn run_frame_loop(
    stream: StdUnixStream,
    router: Arc<Router>,
    display_id: crate::scheduler::DisplayId,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<DisplayOutEvent>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    // Spawn the blocking reader half (client→server requests).
    let read_stream = stream.try_clone().context("clone for reader")?;
    let (req_tx, mut req_rx) =
        tokio::sync::mpsc::unbounded_channel::<codec::CodecResult<Request>>();
    let reader_handle = tokio::task::spawn_blocking(move || loop {
        let res = codec::recv_request(&read_stream);
        let is_err = res.is_err();
        let _ = req_tx.send(res.map(|(r, _fds)| r));
        if is_err {
            return;
        }
    });

    let result = loop {
        tokio::select! {
            _ = wait_shutdown(&mut shutdown_rx) => {
                log::info!("display {display_id}: shutdown signalled");
                break Ok(());
            }
            evt = rx.recv() => match evt {
                None => {
                    log::info!("display {display_id}: router rx closed");
                    break Ok(());
                }
                Some(DisplayOutEvent::Bind { renderer }) => {
                    if let Err(e) = send_bind_from_renderer(&stream, &renderer).await {
                        break Err(e);
                    }
                }
                Some(DisplayOutEvent::Unbind { buffer_generation }) => {
                    if let Err(e) = send_unbind(&stream, buffer_generation).await {
                        break Err(e);
                    }
                }
                Some(DisplayOutEvent::SetConfig(cfg)) => {
                    if let Err(e) = send_set_config(&stream, &cfg).await {
                        break Err(e);
                    }
                }
                Some(DisplayOutEvent::Frame { renderer, buffer_generation, buffer_index, seq }) => {
                    if let Err(e) = forward_frame_ready(
                        &stream, &renderer, buffer_generation, buffer_index, seq,
                    ).await {
                        break Err(e);
                    }
                }
            },
            maybe_req = req_rx.recv() => match maybe_req {
                Some(Ok(Request::BufferRelease { buffer_generation: g, buffer_index, seq })) => {
                    log::debug!(
                        "display {display_id}: release gen={g} idx={buffer_index} seq={seq}"
                    );
                    // Phase 1: bookkeeping only — release aggregation
                    // back to the renderer lands once the renderer
                    // protocol grows a release channel.
                }
                Some(Ok(Request::UpdateDisplay { width, height, properties: _ })) => {
                    router.update_display_size(display_id, width, height).await;
                    log::info!("display {display_id}: resized to {width}x{height}");
                }
                Some(Ok(Request::Bye)) => {
                    log::info!("display {display_id}: bye");
                    break Ok(());
                }
                Some(Ok(other)) => {
                    log::warn!(
                        "display {display_id}: unexpected request opcode {}",
                        other.opcode()
                    );
                }
                Some(Err(e)) => {
                    log::info!("display {display_id}: client recv error: {e}");
                    break Ok(());
                }
                None => {
                    log::info!("display {display_id}: reader task ended");
                    break Ok(());
                }
            },
        }
    };
    // Force the blocking reader out of its parked `recvmsg`. `shutdown`
    // operates on the underlying socket object, so it propagates to
    // every `try_clone`d fd — including the one the reader holds. The
    // reader's next `recvmsg` returns 0 bytes → `CodecError::PeerClosed`
    // → reader thread returns, and the blocking pool worker is
    // reclaimable instead of hanging `BlockingPool::shutdown` during
    // runtime teardown.
    let _ = stream.shutdown(std::net::Shutdown::Both);
    let _ = reader_handle.await;
    result
}

/// Run `codec::recv_request` on the blocking pool but tear down the
/// wait if `shutdown_rx` flips to `true`. On shutdown we force
/// `recvmsg` to return by calling `shutdown(SHUT_RDWR)` on a cloned
/// fd referring to the same socket object, so the blocking task is
/// always joined — never leaked.
async fn recv_request_cancellable(
    stream: &StdUnixStream,
    shutdown_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<(Request, Vec<OwnedFd>)> {
    let blocking_stream = stream.try_clone().context("clone for recv")?;
    let shutdown_stream = stream.try_clone().context("clone for shutdown-kick")?;
    let mut handle =
        tokio::task::spawn_blocking(move || codec::recv_request(&blocking_stream));
    tokio::select! {
        biased;
        res = &mut handle => match res {
            Ok(r) => r.map_err(|e| anyhow!("recv: {e}")),
            Err(e) => Err(anyhow!("recv join: {e}")),
        },
        _ = wait_shutdown(shutdown_rx) => {
            let _ = shutdown_stream.shutdown(std::net::Shutdown::Both);
            let _ = handle.await;
            Err(anyhow!("shutdown during recv"))
        }
    }
}

/// Resolve to `()` once the daemon flips the shutdown flag.
///
/// Wrapped in a helper because `watch::Receiver::wait_for` yields a
/// `Ref<'_, T>` holding an internal `RwLockReadGuard`, which is `!Send`.
/// Hiding the `Ref` inside a plain `async fn -> ()` keeps the
/// surrounding `tokio::select!` futures `Send` so they can run on the
/// multi-thread runtime.
async fn wait_shutdown(rx: &mut tokio::sync::watch::Receiver<bool>) {
    let _ = rx.wait_for(|v| *v).await;
}

// ---------------------------------------------------------------------------
// Wire-event senders
// ---------------------------------------------------------------------------

async fn send_unbind(stream: &StdUnixStream, buffer_generation: u64) -> Result<()> {
    let evt = Event::Unbind { buffer_generation };
    let s = stream.try_clone().context("clone for unbind")?;
    tokio::task::spawn_blocking(move || codec::send_event(&s, &evt, &[]))
        .await
        .context("unbind join")?
        .map_err(|e| anyhow!("send unbind: {e}"))?;
    Ok(())
}

async fn send_set_config(stream: &StdUnixStream, cfg: &ProjectedConfig) -> Result<()> {
    let evt = Event::SetConfig {
        config_generation: cfg.config_generation,
        source_rect: Rect {
            x: cfg.source_x,
            y: cfg.source_y,
            w: cfg.source_w,
            h: cfg.source_h,
        },
        dest_rect: Rect {
            x: cfg.dest_x,
            y: cfg.dest_y,
            w: cfg.dest_w,
            h: cfg.dest_h,
        },
        transform: cfg.transform,
        clear_r: cfg.clear_rgba[0],
        clear_g: cfg.clear_rgba[1],
        clear_b: cfg.clear_rgba[2],
        clear_a: cfg.clear_rgba[3],
    };
    let s = stream.try_clone().context("clone for set_config")?;
    tokio::task::spawn_blocking(move || codec::send_event(&s, &evt, &[]))
        .await
        .context("set_config join")?
        .map_err(|e| anyhow!("send set_config: {e}"))?;
    Ok(())
}

async fn send_bind_from_renderer(
    stream: &StdUnixStream,
    renderer: &Arc<RendererHandle>,
) -> Result<()> {
    let snapshot_arc = renderer.bind_snapshot();
    let (event, dup_fds) = {
        let guard = snapshot_arc
            .lock()
            .map_err(|e| anyhow!("snapshot mutex poisoned: {e}"))?;
        let snap = guard
            .as_ref()
            .ok_or_else(|| anyhow!("renderer {} has no snapshot", renderer.id))?;
        build_bind_event(snap)?
    };
    let s = stream.try_clone().context("clone for bind")?;
    let event_for_send = event.clone();
    let dup_for_send = dup_fds.clone();
    tokio::task::spawn_blocking(move || {
        let result = codec::send_event(&s, &event_for_send, &dup_for_send);
        for fd in dup_for_send {
            unsafe { libc::close(fd) };
        }
        result
    })
    .await
    .context("bind send join")?
    .map_err(|e| anyhow!("send bind_buffers: {e}"))?;
    Ok(())
}

/// Translate the legacy `BindSnapshot` (single-plane) into the new
/// wire event, including a fresh `dup(2)` of every dma-buf fd. The
/// returned fds are raw integers owned by the caller, which must
/// `close(2)` them after the `sendmsg` completes.
fn build_bind_event(snap: &BindSnapshot) -> Result<(Event, Vec<RawFd>)> {
    let buffer_generation = snap.generation;
    let count = snap.count;
    let planes_per_buffer = 1u32;
    let n = count as usize;

    let stride: Vec<u32> = vec![snap.stride; n];
    let plane_offset: Vec<u32> = vec![snap.plane_offset as u32; n];
    let size: Vec<u64> = snap.sizes.clone();
    if size.len() != n {
        return Err(anyhow!(
            "BindSnapshot sizes length {} != count {}",
            size.len(),
            n
        ));
    }
    if snap.fds.len() != n {
        return Err(anyhow!(
            "BindSnapshot fds length {} != count {}",
            snap.fds.len(),
            n
        ));
    }

    let mut dup_fds: Vec<RawFd> = Vec::with_capacity(n);
    for fd in &snap.fds {
        let raw = nix::unistd::dup(fd.as_raw_fd())
            .map_err(|e| anyhow!("dup dma-buf fd: {e}"))?;
        dup_fds.push(raw);
    }

    let event = Event::BindBuffers {
        buffer_generation,
        count,
        width: snap.width,
        height: snap.height,
        fourcc: snap.fourcc,
        modifier: snap.modifier,
        planes_per_buffer,
        stride,
        plane_offset,
        size,
    };
    Ok((event, dup_fds))
}

// ---------------------------------------------------------------------------
// Frame forwarding (with sync fence)
// ---------------------------------------------------------------------------

fn acquire_sync_fd(renderer: &Arc<RendererHandle>, seq: u64) -> Result<OwnedFd> {
    if let Some(fd) = renderer.clone_sync_fd(seq) {
        log::debug!("forwarding real acquire sync_fd for seq={seq}");
        return Ok(fd);
    }
    log::debug!("falling back to dummy fence for seq={seq}");
    dummy_fence::make_signaled_dummy_fence().map_err(|e| anyhow!("dummy fence: {e}"))
}

async fn forward_frame_ready(
    stream: &StdUnixStream,
    renderer: &Arc<RendererHandle>,
    buffer_generation: u64,
    buffer_index: u32,
    seq: u64,
) -> Result<()> {
    let fence = acquire_sync_fd(renderer, seq)?;
    let fence_raw = fence.as_raw_fd();
    let send_stream = stream.try_clone().context("clone for frame_ready")?;
    let evt = Event::FrameReady {
        buffer_generation,
        buffer_index,
        seq,
    };
    let send_result = tokio::task::spawn_blocking(move || {
        codec::send_event(&send_stream, &evt, &[fence_raw])
    })
    .await
    .context("frame_ready send join")?;
    drop(fence);
    send_result.map_err(|e| anyhow!("send frame_ready: {e}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_bind_event_identity() {
        use nix::sys::memfd::{memfd_create, MemFdCreateFlag};
        use std::ffi::CString;
        use std::os::fd::FromRawFd;
        let name = CString::new("waywallen-display-endpoint-test").unwrap();
        let fd1 = memfd_create(&name, MemFdCreateFlag::MFD_CLOEXEC).unwrap();
        let fd2 = memfd_create(&name, MemFdCreateFlag::MFD_CLOEXEC).unwrap();

        let snap = BindSnapshot {
            generation: 7,
            count: 2,
            fourcc: 0x34325258,
            width: 800,
            height: 600,
            stride: 3200,
            modifier: 0,
            plane_offset: 0,
            sizes: vec![1_920_000, 1_920_000],
            fds: vec![fd1, fd2],
        };

        let (event, dup_fds) = build_bind_event(&snap).unwrap();
        assert_eq!(dup_fds.len(), 2);
        match event {
            Event::BindBuffers {
                buffer_generation,
                count,
                width,
                height,
                fourcc,
                modifier,
                planes_per_buffer,
                stride,
                plane_offset,
                size,
            } => {
                assert_eq!(buffer_generation, 7);
                assert_eq!(count, 2);
                assert_eq!(width, 800);
                assert_eq!(height, 600);
                assert_eq!(fourcc, 0x34325258);
                assert_eq!(modifier, 0);
                assert_eq!(planes_per_buffer, 1);
                assert_eq!(stride, vec![3200, 3200]);
                assert_eq!(plane_offset, vec![0, 0]);
                assert_eq!(size, vec![1_920_000, 1_920_000]);
            }
            _ => panic!("expected BindBuffers"),
        }
        for raw in dup_fds {
            let _ = unsafe { std::fs::File::from_raw_fd(raw) };
        }
    }
}

#[allow(dead_code)]
const _OPCODE_MOD_KEEPALIVE: fn() = || {
    let _ = opcode::request::HELLO;
};
