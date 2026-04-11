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
//! GPU/Vulkan driver *and* a valid Wallpaper Engine `.pkg`, neither of
//! which this test attempts to provide — those land in the Iteration 4
//! end-to-end milestone.
//!
//! The test is skipped (not failed) when `WAYWALLEN_RENDERER_BIN` is not
//! set, so `cargo test` stays green on machines that haven't built the
//! C++ host.

use waywallen::ipc::proto::EventMsg;
use waywallen::ipc::uds::recv_msg;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// RAII wrapper that SIGTERMs the child on drop so a failing test doesn't
/// leave the renderer process running in the background.
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn renderer_bin() -> Option<PathBuf> {
    std::env::var_os("WAYWALLEN_RENDERER_BIN").map(PathBuf::from)
}

#[test]
fn hello_handshake() {
    let Some(bin) = renderer_bin() else {
        eprintln!(
            "skipping cpp_host_handshake: set WAYWALLEN_RENDERER_BIN to the path \
             of the compiled waywallen-renderer binary to run this test"
        );
        return;
    };
    assert!(
        bin.exists(),
        "WAYWALLEN_RENDERER_BIN points at nonexistent path: {}",
        bin.display()
    );

    // Tempfile path for the socket. We can't just use TempPath because
    // we need the path removed *before* we bind — a leftover file would
    // make bind() fail with EADDRINUSE.
    let sock_path = std::env::temp_dir().join(format!(
        "waywallen-cpp-host-handshake-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&sock_path);

    let listener = UnixListener::bind(&sock_path).expect("bind unix listener");
    // Delete the socket file as soon as the child has connected so test
    // cleanup is automatic; the socket fd stays valid after unlink.
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _cleanup = Cleanup(sock_path.clone());

    let mut cmd = Command::new(&bin);
    cmd.arg("--ipc")
        .arg(&sock_path)
        .arg("--width")
        .arg("1280")
        .arg("--height")
        .arg("720")
        .arg("--fps")
        .arg("30")
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let child = cmd.spawn().unwrap_or_else(|e| {
        panic!("failed to spawn {}: {}", bin.display(), e);
    });
    let mut guard = ChildGuard(child);

    // Bound the wait so a hung child can't wedge the test runner.
    listener
        .set_nonblocking(false)
        .expect("set blocking on listener");
    let (stream, _addr) = {
        // Use a background thread to enforce a timeout.
        let (tx, rx) = std::sync::mpsc::channel();
        let l_clone = listener.try_clone().expect("clone listener");
        std::thread::spawn(move || {
            let res = l_clone.accept();
            let _ = tx.send(res);
        });
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(x)) => x,
            Ok(Err(e)) => panic!("accept failed: {e}"),
            Err(_) => {
                let _ = guard.0.kill();
                panic!("timed out waiting for waywallen-renderer to connect back");
            }
        }
    };

    let (msg, fds): (EventMsg, _) =
        recv_msg(&stream).expect("recv first frame from host");
    assert!(fds.is_empty(), "ready must not carry fds");
    match msg {
        EventMsg::Ready => { /* ok */ }
        other => panic!("expected Ready, got {other:?}"),
    }

    // Done. ChildGuard::drop will SIGTERM the host; PR_SET_PDEATHSIG
    // plus our kill is belt-and-suspenders.
}

/// I2 extended smoke against the C++ host's `--test-pattern` mode.
///
/// Background: `SceneWallpaper::loadScene` early-returns when no assets
/// directory is configured, so without a full Wallpaper Engine install
/// there's nothing to drive `redraw_callback`. To unblock the Rust-side
/// bring-up (I4) before we wire up a real scene, the C++ host gained a
/// `--test-pattern` CLI flag that pumps the offscreen ExSwapchain ring
/// directly from a host timer thread and emits BindBuffers + FrameReady
/// events without any actual pixel drawing.
///
/// This test uses that mode to prove the wire end-to-end: Ready +
/// BindBuffers(3 FDs, DMA-BUF metadata populated per I3 audit) + a few
/// FrameReady events with distinct image indices.
#[test]
fn binding_and_frames_smoke() {
    let Some(bin) = renderer_bin() else {
        eprintln!("skipping: WAYWALLEN_RENDERER_BIN unset");
        return;
    };

    let sock_path = std::env::temp_dir().join(format!(
        "waywallen-cpp-host-test-pattern-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&sock_path);
    let listener = UnixListener::bind(&sock_path).expect("bind");
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _cleanup = Cleanup(sock_path.clone());

    let mut cmd = Command::new(&bin);
    cmd.arg("--ipc")
        .arg(&sock_path)
        .arg("--width")
        .arg("1280")
        .arg("--height")
        .arg("720")
        .arg("--fps")
        .arg("30")
        .arg("--test-pattern")
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut guard = ChildGuard(cmd.spawn().expect("spawn host"));

    let (stream, _) = {
        let (tx, rx) = std::sync::mpsc::channel();
        let l_clone = listener.try_clone().unwrap();
        std::thread::spawn(move || {
            let _ = tx.send(l_clone.accept());
        });
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(x)) => x,
            _ => {
                let _ = guard.0.kill();
                panic!("accept timed out");
            }
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
        let (msg, fds): (EventMsg, _) = match recv_msg(&stream) {
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
                    "I2 BindBuffers: count={} fourcc=0x{:08x} {}x{} stride={} mod=0x{:x} \
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
