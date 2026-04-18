#![allow(dead_code)]
//! Shared helpers for integration tests.
//!
//! Each file under `tests/*.rs` is compiled as its own crate, so shared
//! code must be pulled in with `#[path = "common/mod.rs"] mod common;`.
//! `#![allow(dead_code)]` silences warnings in files that only use a
//! subset of the helpers.

use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Allocate a unique tempfile path for a Unix-domain socket. The pid and
/// a nanosecond timestamp keep parallel tests from colliding.
pub fn tmp_sock(tag: &str) -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "waywallen-{tag}-{}-{ts}.sock",
        std::process::id()
    ))
}

/// RAII guard that unlinks the socket file on drop. Safe to hold for the
/// full test body — the listener fd stays valid after unlink.
pub struct SockCleanup(pub PathBuf);
impl Drop for SockCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// RAII wrapper around a `Child` that SIGKILLs + waits on drop so a
/// failing test never leaks a renderer process.
pub struct ChildGuard(pub Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `UnixListener::accept` with a bounded wall-clock timeout. Returns
/// `None` on timeout (caller decides whether to panic or skip).
pub fn accept_with_timeout(
    listener: &UnixListener,
    timeout: Duration,
) -> Option<std::io::Result<(UnixStream, SocketAddr)>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let l_clone = listener.try_clone().expect("clone listener");
    std::thread::spawn(move || {
        let _ = tx.send(l_clone.accept());
    });
    rx.recv_timeout(timeout).ok()
}

/// Poll `path.exists()` until true or `timeout` elapses. Used to wait
/// for the display endpoint to finish `UnixListener::bind` before
/// clients attempt to connect.
pub async fn wait_for_sock_bind(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

/// Cheap DRM render-node presence check. Vulkan-only tests should
/// `return` with a `skip:` eprintln when this returns `false`.
pub fn have_vulkan_device() -> bool {
    Path::new("/dev/dri").exists()
}

/// Resolve the external C++ host binary from `WAYWALLEN_RENDERER_BIN`.
/// Tests that need the C++ host should early-return with a skip line
/// when this yields `None`.
pub fn cpp_renderer_bin_from_env() -> Option<PathBuf> {
    std::env::var_os("WAYWALLEN_RENDERER_BIN").map(PathBuf::from)
}
