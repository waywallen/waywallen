//! C++ host handshake: spawn the `waywallen-renderer` host binary
//! against a listening Unix-domain socket and verify the handshake.
//!
//! This is an *integration* test from the Rust daemon's perspective:
//!   1. Create a UDS listener at a tempfile path.
//!   2. Spawn `$WAYWALLEN_RENDERER_BIN --ipc <path> ...`.
//!   3. Accept the host's connection.
//!   4. Read one framed message and assert it parses as `EventMsg::Ready`,
//!      which the host emits after `SceneWallpaper::initVulkan` succeeds.
//!
//! Anything past Ready (BindBuffers, FrameReady) depends on a working
//! GPU/Vulkan driver *and* a valid Wallpaper Engine `.pkg`. The
//! `--test-pattern` smoke test below covers the BindBuffers/FrameReady
//! wire without needing a real scene.
//!
//! Skipped (not failed) when `WAYWALLEN_RENDERER_BIN` is unset.

use waywallen::ipc::proto::EventMsg;
use waywallen::ipc::uds::recv_event;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};
use std::time::Duration;

#[path = "common/mod.rs"]
mod common;

#[test]
fn hello_handshake() {
    let Some(bin) = common::cpp_renderer_bin_from_env() else {
        eprintln!(
            "skipping ipc_renderer_handshake_cpp: set WAYWALLEN_RENDERER_BIN to the path \
             of the compiled waywallen-renderer binary to run this test"
        );
        return;
    };
    assert!(
        bin.exists(),
        "WAYWALLEN_RENDERER_BIN points at nonexistent path: {}",
        bin.display()
    );

    let sock_path = common::tmp_sock("cpp-host-handshake");
    let _ = std::fs::remove_file(&sock_path);
    let listener = UnixListener::bind(&sock_path).expect("bind unix listener");
    let _cleanup = common::SockCleanup(sock_path.clone());

    let child = Command::new(&bin)
        .arg("--ipc")
        .arg(&sock_path)
        .arg("--width")
        .arg("1280")
        .arg("--height")
        .arg("720")
        .arg("--fps")
        .arg("30")
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn {}: {}", bin.display(), e));
    let mut guard = common::ChildGuard(child);

    listener
        .set_nonblocking(false)
        .expect("set blocking on listener");
    let (stream, _addr) = match common::accept_with_timeout(&listener, Duration::from_secs(10)) {
        Some(Ok(x)) => x,
        Some(Err(e)) => panic!("accept failed: {e}"),
        None => {
            let _ = guard.0.kill();
            panic!("timed out waiting for waywallen-renderer to connect back");
        }
    };

    let (msg, fds): (EventMsg, _) =
        recv_event(&stream).expect("recv first frame from host");
    assert!(fds.is_empty(), "ready must not carry fds");
    match msg {
        EventMsg::Ready => { /* ok */ }
        other => panic!("expected Ready, got {other:?}"),
    }
}

/// Extended smoke against the C++ host's `--test-pattern` mode.
///
/// `SceneWallpaper::loadScene` early-returns when no assets directory is
/// configured, so without a full Wallpaper Engine install there's nothing
/// to drive `redraw_callback`. The host's `--test-pattern` CLI flag pumps
/// the offscreen ExSwapchain ring directly from a host timer thread and
/// emits BindBuffers + FrameReady without any actual pixel drawing, which
/// is enough to prove the wire end-to-end.
#[test]
fn binding_and_frames_smoke() {
    let Some(bin) = common::cpp_renderer_bin_from_env() else {
        eprintln!("skipping: WAYWALLEN_RENDERER_BIN unset");
        return;
    };

    let sock_path = common::tmp_sock("cpp-host-test-pattern");
    let _ = std::fs::remove_file(&sock_path);
    let listener = UnixListener::bind(&sock_path).expect("bind");
    let _cleanup = common::SockCleanup(sock_path.clone());

    let child = Command::new(&bin)
        .arg("--ipc")
        .arg(&sock_path)
        .arg("--width")
        .arg("1280")
        .arg("--height")
        .arg("720")
        .arg("--fps")
        .arg("30")
        .arg("--test-pattern")
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn host");
    let mut guard = common::ChildGuard(child);

    let (stream, _) = match common::accept_with_timeout(&listener, Duration::from_secs(10)) {
        Some(Ok(x)) => x,
        _ => {
            let _ = guard.0.kill();
            panic!("accept timed out");
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(8)))
        .expect("set rd timeout");

    // Drain until Ready → BindBuffers → >=5 FrameReady, or timeout.
    let mut saw_ready = false;
    let mut bind: Option<(Vec<i32>, (u32, u32, u32, u32, u64, u64))> = None;
    let mut frames = 0usize;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let (msg, fds): (EventMsg, _) = match recv_event(&stream) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("recv error (expected if hung): {e}");
                break;
            }
        };
        match msg {
            EventMsg::Ready => saw_ready = true,
            EventMsg::BindBuffers {
                count,
                fourcc,
                width,
                height,
                stride,
                modifier,
                plane_offset,
                sizes,
            } => {
                eprintln!(
                    "BindBuffers: count={} fourcc=0x{:08x} {}x{} stride={} mod=0x{:x} \
                     plane_offset={} sizes={:?} fds={}",
                    count,
                    fourcc,
                    width,
                    height,
                    stride,
                    modifier,
                    plane_offset,
                    sizes,
                    fds.len()
                );
                assert_eq!(count, 3, "expected 3 slots");
                assert_eq!(fds.len(), 3, "expected 3 FDs via SCM_RIGHTS");
                assert!(fourcc != 0, "fourcc must be non-zero");
                assert!(u64::from(stride) >= u64::from(width) * 4, "stride sanity");
                bind = Some((
                    fds.iter().map(|f| f.as_raw_fd()).collect(),
                    (count, fourcc, width, height, u64::from(stride), modifier),
                ));
                std::mem::forget(fds);
            }
            EventMsg::FrameReady { .. } => {
                frames += 1;
                if frames >= 5 && bind.is_some() {
                    break;
                }
            }
            other => eprintln!("unexpected msg: {other:?}"),
        }
    }

    assert!(saw_ready, "never saw Ready event");
    let bind = bind.expect("never saw BindBuffers under --test-pattern mode");
    assert!(
        frames >= 5,
        "expected >=5 FrameReady, got {frames}; bind={bind:?}"
    );
}
