//! Placeholder acquire-fence helpers.
//!
//! The long-term plan: the producer (`wescene-renderer` / the Vulkan
//! DMA-BUF exporter on the daemon side) exports a real `dma_fence`
//! sync_file via `vkGetSemaphoreFdKHR(SYNC_FD_BIT)` and sends it in
//! the `frame_ready` event's SCM_RIGHTS slot. The display library
//! then imports it via `vkImportSemaphoreFdKHR` / `eglCreateSyncKHR`
//! and waits on it before sampling the texture.
//!
//! Producer-side sync export is not wired up yet, so for Phase 1 we
//! stand in with an **`eventfd` initialised to a non-zero counter**.
//! That fd has the following useful property: a `poll(2)` with
//! `POLLIN` returns immediately, so any client that treats the fd as
//! "already signalled, read-to-sample" behaves correctly without
//! blocking.
//!
//! ⚠️ Limitations:
//!
//!   - An `eventfd` is **not** a `dma_fence sync_file`. Drivers that
//!     call `vkImportSemaphoreFdKHR(SYNC_FD)` or
//!     `eglCreateSyncKHR(NATIVE_FENCE_ANDROID, fd)` will reject it.
//!     That's fine for the current headless `waywallen_display_demo`
//!     (which closes the fd without importing), but Phase 3's real
//!     EGL backend **must** replace this with a proper sync_file once
//!     the producer side exports one.
//!   - TODO(sync): replace with producer-exported `dma_fence` sync_file
//!     when `vulkan_dma_buf.rs` learns to export `VkSemaphore ->
//!     sync_fd`. Tracking: waywallen-display-v1 Phase 1 → Phase 3.

use std::io;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};

/// Create a fresh "already-signalled" placeholder fence fd, owned by
/// the caller. The returned fd is safe to `dup(2)` and pass across
/// SCM_RIGHTS.
pub fn make_signaled_dummy_fence() -> io::Result<OwnedFd> {
    // SAFETY: eventfd(2) is a simple syscall with no invariants we
    // could violate from Rust. The returned fd is a fresh kernel
    // resource we immediately wrap in OwnedFd to track its lifetime.
    let raw: RawFd = unsafe { libc::eventfd(1, libc::EFD_CLOEXEC) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: we just created this fd and no one else has a reference.
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    #[test]
    fn create_and_close() {
        let fd = make_signaled_dummy_fence().expect("eventfd");
        assert!(fd.as_raw_fd() >= 0);
        // Drop closes the fd; nothing else to assert.
    }

    #[test]
    fn poll_reports_readable_immediately() {
        let fd = make_signaled_dummy_fence().expect("eventfd");
        let mut pfd = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: pfd is a stack-local with a single valid fd.
        let rc = unsafe { libc::poll(&mut pfd as *mut _, 1, 0) };
        assert_eq!(rc, 1, "poll should return 1 (readable)");
        assert!(pfd.revents & libc::POLLIN != 0, "POLLIN not set");
    }

    #[test]
    fn two_fences_have_distinct_fds() {
        let a = make_signaled_dummy_fence().unwrap();
        let b = make_signaled_dummy_fence().unwrap();
        assert_ne!(a.as_raw_fd(), b.as_raw_fd());
    }
}
