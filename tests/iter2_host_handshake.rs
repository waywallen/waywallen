//! Iteration 2 smoke test: spawn the `waywallen-renderer` host binary
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

use kwallpaper_backend::ipc::proto::EventMsg;
use kwallpaper_backend::ipc::uds::recv_msg;
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
            "skipping iter2_host_handshake: set WAYWALLEN_RENDERER_BIN to the path \
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
        "waywallen-iter2-{}-{}.sock",
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
        .arg("640")
        .arg("--height")
        .arg("360")
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
