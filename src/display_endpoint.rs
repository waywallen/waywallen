//! Display endpoint — accepts external display client connections on a
//! Unix socket and speaks the `waywallen-display-v1` protocol with them.
//!
//! Wire roles:
//!   * **Client** = display (libwaywallen_display, the `waywallen_display_demo`
//!     bin, or any other conforming consumer).
//!   * **Server** = this module, acting on behalf of the daemon's
//!     `RendererManager` + `Scheduler`.
//!
//! Phase 1 state machine per client:
//!
//!     INIT              →hello             →HELLO_SENT
//!     HELLO_SENT        ←welcome           →READY
//!     READY             →register_display  →REGISTERING
//!     REGISTERING       ←display_accepted  →IDLE
//!     IDLE              ←bind_buffers      →PENDING_CONFIG
//!     PENDING_CONFIG    ←set_config        →BOUND
//!     BOUND             ←frame_ready / →buffer_release
//!     BOUND             ←unbind            →IDLE
//!     *                 EOF/err            →DEAD
//!
//! The frame loop is split into a reader half (a blocking
//! `spawn_blocking` task that drains client requests into an mpsc
//! channel) and a writer half (a tokio `select!` loop that forwards
//! renderer broadcast events + replies to client requests).
//!
//! Phase 1 simplifications — explicitly deferred:
//!   * Producer side does not yet export a real `dma_fence` sync_fd,
//!     so every forwarded `frame_ready` carries a
//!     [`dummy_fence::make_signaled_dummy_fence`] placeholder instead.
//!   * `buffer_release` from the client is bookkeeping-only — it is
//!     **not** aggregated back to the renderer yet (the legacy
//!     renderer protocol has no release channel). Full K-way refcount
//!     via `Scheduler::begin_frame`/`release_frame` is unit-tested
//!     separately and will be wired up when the renderer side grows a
//!     release channel.
//!   * Only the first `RendererManager::list()` entry is ever bound;
//!     multi-renderer scheduling lands later.
//!   * `update_display` is taken (scheduler state updated) but does
//!     not trigger a fresh `set_config` until a future milestone.

use anyhow::{anyhow, Context, Result};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::broadcast;

use crate::display_proto::generated::Rect;
use crate::display_proto::{codec, opcode, Event, Request, PROTOCOL_NAME};
use crate::dummy_fence;
use crate::ipc::proto::EventMsg;
use crate::renderer_manager::{BindSnapshot, RendererHandle, RendererManager};
use crate::scheduler::{ActiveBinding, DisplayId, Scheduler};

/// Server version string advertised in `welcome.server_version`.
pub const SERVER_VERSION: &str = concat!("waywallen ", env!("CARGO_PKG_VERSION"));

/// v1 mandatory feature flags.
const MANDATORY_FEATURES: &[&str] = &["explicit_sync_fd"];

/// How long to wait for the renderer's first `BindBuffers` to arrive
/// before giving up on a freshly-connected client. Matches the legacy
/// viewer_endpoint timeout.
const BIND_WAIT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

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

/// Bind a listening UDS at `sock_path` (unlinking any stale file
/// first) and run the accept loop forever, spawning one task per
/// client. Returns only on bind error.
pub async fn serve(
    sock_path: &Path,
    mgr: Arc<RendererManager>,
    scheduler: Arc<StdMutex<Scheduler>>,
) -> Result<()> {
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
        let std_stream = match stream.into_std().and_then(|s| {
            s.set_nonblocking(false).map(|_| s)
        }) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("display into_std failed: {e}");
                continue;
            }
        };
        let mgr = Arc::clone(&mgr);
        let scheduler = Arc::clone(&scheduler);
        tokio::spawn(async move {
            if let Err(e) = handle_client(std_stream, mgr, scheduler).await {
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
    mgr: Arc<RendererManager>,
    scheduler: Arc<StdMutex<Scheduler>>,
) -> Result<()> {
    // ---- Handshake (INIT → HELLO_SENT → READY → REGISTERING → IDLE) ----
    let (display_id, renderer) =
        do_handshake(&stream, &mgr, &scheduler).await?;
    log::info!(
        "display {display_id} registered; bound to renderer {}",
        renderer.id
    );

    // ---- Bind the initial buffer pool + send SetConfig (IDLE → BOUND) ----
    let buffer_generation: u64 = 1;
    send_initial_bind_and_config(
        &stream,
        &renderer,
        &scheduler,
        display_id,
        buffer_generation,
    )
    .await
    .context("initial bind + config")?;

    // ---- Frame loop ----
    let loop_result = run_frame_loop(
        stream,
        renderer,
        scheduler.clone(),
        display_id,
        buffer_generation,
    )
    .await;

    // Always unregister on exit, regardless of success/failure.
    {
        if let Ok(mut s) = scheduler.lock() {
            let _ = s.unregister_display(display_id);
        }
    }

    loop_result
}

// ---------------------------------------------------------------------------
// Handshake helpers
// ---------------------------------------------------------------------------

async fn do_handshake(
    stream: &StdUnixStream,
    mgr: &Arc<RendererManager>,
    scheduler: &Arc<StdMutex<Scheduler>>,
) -> Result<(DisplayId, Arc<RendererHandle>)> {
    // 1. Read Hello.
    let hello_stream = stream.try_clone().context("clone for hello")?;
    let (hello, _fds): (Request, _) =
        tokio::task::spawn_blocking(move || codec::recv_request(&hello_stream))
            .await
            .context("hello join")?
            .map_err(|e| anyhow!("recv hello: {e}"))?;
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

    // 2. Send Welcome.
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

    // 3. Read RegisterDisplay.
    let reg_stream = stream.try_clone().context("clone for register")?;
    let (reg, _fds): (Request, _) =
        tokio::task::spawn_blocking(move || codec::recv_request(&reg_stream))
            .await
            .context("register join")?
            .map_err(|e| anyhow!("recv register_display: {e}"))?;
    let Request::RegisterDisplay {
        name,
        width,
        height,
        refresh_mhz,
        properties,
    } = reg
    else {
        return Err(anyhow!("expected register_display, got opcode {}", reg.opcode()));
    };

    // 4. Scheduler assigns display_id, bind to first available renderer.
    let display_id = {
        let mut s = scheduler
            .lock()
            .map_err(|e| anyhow!("scheduler poisoned: {e}"))?;
        s.register_display(name.clone(), width, height, refresh_mhz, properties)
    };
    log::info!(
        "display {display_id}: name={name} size={width}x{height}@{refresh_mhz}mHz"
    );

    // 5. Send DisplayAccepted.
    let ack_stream = stream.try_clone().context("clone for accepted")?;
    tokio::task::spawn_blocking(move || {
        codec::send_event(
            &ack_stream,
            &Event::DisplayAccepted { display_id },
            &[],
        )
    })
    .await
    .context("accepted join")?
    .map_err(|e| anyhow!("send display_accepted: {e}"))?;

    // 6. Pick a renderer. Phase 1: first in manager.list().
    let renderer = first_renderer(mgr)
        .await
        .ok_or_else(|| anyhow!("no renderer is spawned yet"))?;

    Ok((display_id, renderer))
}

async fn first_renderer(mgr: &Arc<RendererManager>) -> Option<Arc<RendererHandle>> {
    let ids = mgr.list().await;
    let first = ids.into_iter().next()?;
    mgr.get(&first).await
}

// ---------------------------------------------------------------------------
// Initial bind + config
// ---------------------------------------------------------------------------

async fn send_initial_bind_and_config(
    stream: &StdUnixStream,
    renderer: &Arc<RendererHandle>,
    scheduler: &Arc<StdMutex<Scheduler>>,
    display_id: DisplayId,
    buffer_generation: u64,
) -> Result<()> {
    // Wait for the renderer's first BindBuffers to be cached.
    let snapshot_arc = renderer.bind_snapshot();
    let mut waited = Duration::ZERO;
    let step = Duration::from_millis(50);
    loop {
        {
            let guard = snapshot_arc
                .lock()
                .map_err(|e| anyhow!("snapshot mutex poisoned: {e}"))?;
            if guard.is_some() {
                break;
            }
        }
        if waited >= BIND_WAIT {
            return Err(anyhow!(
                "renderer {} produced no BindBuffers within {}s",
                renderer.id,
                BIND_WAIT.as_secs()
            ));
        }
        tokio::time::sleep(step).await;
        waited += step;
    }

    // Snapshot → new-protocol BindBuffers event + fd dup set.
    let (bind_event, dup_fds) = {
        let guard = snapshot_arc
            .lock()
            .map_err(|e| anyhow!("snapshot mutex poisoned: {e}"))?;
        let snap = guard.as_ref().expect("just checked Some");
        build_bind_event(snap, buffer_generation)?
    };

    // Register the binding with the scheduler (so future fan-out knows
    // about this renderer). We do this after we know the snapshot is
    // valid so the scheduler's `active_binding` is never ahead of
    // reality.
    {
        let mut s = scheduler
            .lock()
            .map_err(|e| anyhow!("scheduler poisoned: {e}"))?;
        let _ = s.set_active_binding(ActiveBinding {
            renderer_id: renderer.id.clone(),
            buffer_generation,
            tex_width: renderer.width,
            tex_height: renderer.height,
        });
    }

    // Send bind_buffers with dup'd fds; close our copies after send.
    let bind_stream = stream.try_clone().context("clone for bind")?;
    let dup_for_send = dup_fds.clone();
    let bind_event_for_send = bind_event.clone();
    tokio::task::spawn_blocking(move || {
        let result = codec::send_event(&bind_stream, &bind_event_for_send, &dup_for_send);
        // Close the dup'd fds; kernel held refs during the in-flight
        // sendmsg, so it's safe (and required) to close after send.
        for fd in dup_for_send {
            unsafe { libc::close(fd) };
        }
        result
    })
    .await
    .context("bind send join")?
    .map_err(|e| anyhow!("send bind_buffers: {e}"))?;

    // Project + send SetConfig.
    let cfg = {
        let mut s = scheduler
            .lock()
            .map_err(|e| anyhow!("scheduler poisoned: {e}"))?;
        s.project_config(display_id)
            .ok_or_else(|| anyhow!("project_config returned None"))?
    };
    let set_cfg_event = Event::SetConfig {
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
    let cfg_stream = stream.try_clone().context("clone for set_config")?;
    tokio::task::spawn_blocking(move || {
        codec::send_event(&cfg_stream, &set_cfg_event, &[])
    })
    .await
    .context("set_config join")?
    .map_err(|e| anyhow!("send set_config: {e}"))?;

    Ok(())
}

/// Translate the legacy `BindSnapshot` (single-plane) into the new
/// wire event, including a fresh `dup(2)` of every dma-buf fd. The
/// returned fds are raw integers owned by the caller, which must
/// `close(2)` them after the `sendmsg` completes.
fn build_bind_event(
    snap: &BindSnapshot,
    buffer_generation: u64,
) -> Result<(Event, Vec<RawFd>)> {
    let count = snap.count;
    let planes_per_buffer = 1u32;
    let n = count as usize;

    // Phase 1 assumption: every buffer has the same stride /
    // plane_offset (the legacy protocol only carries a single value).
    // When multi-plane support lands this fan-out goes away.
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
// Frame loop
// ---------------------------------------------------------------------------

async fn run_frame_loop(
    stream: StdUnixStream,
    renderer: Arc<RendererHandle>,
    scheduler: Arc<StdMutex<Scheduler>>,
    display_id: DisplayId,
    buffer_generation: u64,
) -> Result<()> {
    let mut events = renderer.events();

    // Spawn reader half: blocking recv_request into an mpsc channel.
    let read_stream = stream.try_clone().context("clone for reader")?;
    let (req_tx, mut req_rx) =
        tokio::sync::mpsc::unbounded_channel::<codec::CodecResult<Request>>();
    let reader_handle = tokio::task::spawn_blocking(move || {
        loop {
            let res = codec::recv_request(&read_stream);
            let is_err = res.is_err();
            let _ = req_tx.send(res.map(|(r, _fds)| r));
            if is_err {
                return;
            }
        }
    });

    let result = loop {
        tokio::select! {
            // ---- Events from the renderer broadcast ----
            evt = events.recv() => {
                match evt {
                    Ok(EventMsg::FrameReady { image_index, seq, .. }) => {
                        if let Err(e) = forward_frame_ready(
                            &stream,
                            &renderer,
                            buffer_generation,
                            image_index,
                            seq,
                        ).await {
                            break Err(e);
                        }
                    }
                    Ok(EventMsg::BindBuffers { .. }) => {
                        // Producer rebind mid-session is not supported
                        // in Phase 1 — the original snapshot fds are
                        // still live for this client. Log and drop.
                        log::debug!(
                            "display {display_id}: ignoring mid-session renderer rebind"
                        );
                    }
                    Ok(_) => { /* Ready/etc — nothing to forward. */ }
                    Err(broadcast::error::RecvError::Closed) => {
                        log::info!("display {display_id}: renderer broadcast closed");
                        break Ok(());
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        log::warn!("display {display_id}: lagged {n} frames; continuing");
                    }
                }
            }
            // ---- Requests from the client ----
            maybe_req = req_rx.recv() => {
                match maybe_req {
                    Some(Ok(Request::BufferRelease { buffer_generation: g, buffer_index, seq })) => {
                        // Phase 1: accept + log only. Scheduler/renderer
                        // release aggregation lands with the producer
                        // sync-fence work.
                        log::debug!(
                            "display {display_id}: release gen={g} idx={buffer_index} seq={seq}"
                        );
                    }
                    Some(Ok(Request::UpdateDisplay { width, height, properties: _ })) => {
                        if let Ok(mut s) = scheduler.lock() {
                            s.update_display_size(display_id, width, height);
                        }
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
                }
            }
        }
    };

    reader_handle.abort();
    result
}

/// Produce the sync_fd to attach to a forwarded FrameReady event.
///
/// Phase 3b preference order:
///   1. Real `dma_fence` sync_file exported by the producer side —
///      retrieved from `RendererHandle::take_sync_fd(seq)`.
///   2. Already-signalled `eventfd` placeholder from `dummy_fence`,
///      used when the producer didn't export one or another display
///      already consumed this (gen, seq).
///
/// Path 1 feeds a real GPU-side fence into the display's EGL
/// import path; path 2 keeps the protocol satisfied for stub-backend
/// consumers (the headless Rust demo, multi-display non-primary
/// subscribers).
///
/// Returns an `OwnedFd` that the caller must pass to SCM_RIGHTS and
/// then drop.
fn acquire_sync_fd(
    renderer: &Arc<RendererHandle>,
    seq: u64,
) -> Result<OwnedFd> {
    if let Some(fd) = renderer.take_sync_fd(seq) {
        log::debug!("forwarding real acquire sync_fd for seq={seq}");
        return Ok(fd);
    }
    log::debug!("falling back to dummy fence for seq={seq}");
    dummy_fence::make_signaled_dummy_fence()
        .map_err(|e| anyhow!("dummy fence: {e}"))
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
    drop(fence); // close our copy; kernel still has refs in the in-flight cmsg
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
        use std::os::fd::FromRawFd;
        // Make two memfds so the builder has real fds to dup.
        use nix::sys::memfd::{memfd_create, MemFdCreateFlag};
        use std::ffi::CString;
        let name = CString::new("waywallen-display-endpoint-test").unwrap();
        let fd1 = memfd_create(&name, MemFdCreateFlag::MFD_CLOEXEC).unwrap();
        let fd2 = memfd_create(&name, MemFdCreateFlag::MFD_CLOEXEC).unwrap();

        let snap = BindSnapshot {
            count: 2,
            fourcc: 0x34325258, // 'XR24'
            width: 800,
            height: 600,
            stride: 3200,
            modifier: 0,
            plane_offset: 0,
            sizes: vec![1_920_000, 1_920_000],
            fds: vec![fd1, fd2],
        };

        let (event, dup_fds) = build_bind_event(&snap, 7).unwrap();
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
        // Close dup'd fds so we don't leak.
        for raw in dup_fds {
            let _ = unsafe { std::fs::File::from_raw_fd(raw) };
        }
    }
}

// Silence a rustc warning about `opcode` being re-exported but only
// used through the derived `Request::opcode()` / `Event::opcode()`
// methods inside this file.
#[allow(dead_code)]
const _OPCODE_MOD_KEEPALIVE: fn() = || {
    let _ = opcode::request::HELLO;
};
