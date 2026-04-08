//! Blocking Unix-domain-socket framing helpers for the waywallen IPC.
//!
//! Wire format per message:
//!   [u32 big-endian length] [JSON body]
//! File descriptors ride along as ancillary SCM_RIGHTS data on the same
//! `sendmsg(2)`. The length prefix is the length of the JSON body only,
//! and excludes itself.
//!
//! This module intentionally uses `std::os::unix::net::UnixStream` (not the
//! Tokio version) because the async path needs deeper surgery and isn't
//! needed until the daemon actually hosts connections. The Tokio adapter
//! will sit on top in a later iteration.

use anyhow::{anyhow, Context, Result};
use nix::sys::socket::{
    recvmsg, sendmsg, ControlMessage, ControlMessageOwned, MsgFlags,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::io::IoSlice;
use std::io::IoSliceMut;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

/// Maximum JSON body we'll accept in a single frame. 1 MiB is far more than
/// anything in the current protocol needs; it exists only to reject runaway
/// garbage.
const MAX_FRAME_BYTES: usize = 1 << 20;

/// Maximum number of FDs a single message may carry. Matches the triple
/// buffer + a headroom slot for future sync_file passing.
pub const MAX_FDS_PER_MSG: usize = 8;

/// Send a framed message, optionally attaching file descriptors via
/// SCM_RIGHTS. The receiver must call [`recv_msg`] to parse it.
pub fn send_msg<T: Serialize>(sock: &UnixStream, msg: &T, fds: &[RawFd]) -> Result<()> {
    if fds.len() > MAX_FDS_PER_MSG {
        return Err(anyhow!(
            "send_msg: {} fds exceeds MAX_FDS_PER_MSG={}",
            fds.len(),
            MAX_FDS_PER_MSG
        ));
    }
    let body = serde_json::to_vec(msg).context("serialize ipc message")?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(anyhow!(
            "send_msg: body {}B exceeds MAX_FRAME_BYTES={}",
            body.len(),
            MAX_FRAME_BYTES
        ));
    }

    let len_prefix = (body.len() as u32).to_be_bytes();
    let mut framed = Vec::with_capacity(4 + body.len());
    framed.extend_from_slice(&len_prefix);
    framed.extend_from_slice(&body);

    let iov = [IoSlice::new(&framed)];
    let cmsgs_storage;
    let cmsgs: &[ControlMessage] = if fds.is_empty() {
        &[]
    } else {
        cmsgs_storage = [ControlMessage::ScmRights(fds)];
        &cmsgs_storage
    };

    // Loop on EINTR; anything else is fatal.
    loop {
        match sendmsg::<()>(sock.as_raw_fd(), &iov, cmsgs, MsgFlags::empty(), None) {
            Ok(sent) if sent == framed.len() => return Ok(()),
            Ok(sent) => {
                return Err(anyhow!(
                    "send_msg: short write {}/{}",
                    sent,
                    framed.len()
                ));
            }
            Err(nix::errno::Errno::EINTR) => continue,
            Err(e) => return Err(anyhow!("send_msg: sendmsg failed: {e}")),
        }
    }
}

/// Receive a framed message. Returns the decoded body plus any file
/// descriptors that arrived as SCM_RIGHTS ancillary data.
///
/// Implementation note — back-to-back frame safety:
/// The naive "one big recvmsg" approach is broken: if the sender does
/// two consecutive sendmsg calls, the kernel may deliver both frames in
/// a single recvmsg buffer, and a parser that only consumes the first
/// silently drops the second along with its ancillary data. Instead we
/// always read **exactly** four bytes for the length prefix via recvmsg
/// (which is also where SCM_RIGHTS lives since the sender attaches the
/// cmsg to the first byte of the frame), then read **exactly** body_len
/// bytes via plain `read_exact`. This guarantees no over-read and no
/// lost FDs.
///
/// FDs are returned as [`OwnedFd`] so the caller is responsible for their
/// lifetime — dropping them closes the FD.
pub fn recv_msg<T: DeserializeOwned>(sock: &UnixStream) -> Result<(T, Vec<OwnedFd>)> {
    use std::io::Read;

    // ---------- Phase 1: read 4-byte prefix + harvest SCM_RIGHTS ----------
    let mut prefix = [0u8; 4];
    let mut owned_fds: Vec<OwnedFd> = Vec::new();
    let mut filled = 0usize;
    while filled < 4 {
        let mut cmsg_space = nix::cmsg_space!([RawFd; MAX_FDS_PER_MSG]);
        let mut iov = [IoSliceMut::new(&mut prefix[filled..])];
        let msg = loop {
            match recvmsg::<()>(
                sock.as_raw_fd(),
                &mut iov,
                Some(&mut cmsg_space),
                MsgFlags::empty(),
            ) {
                Ok(m) => break m,
                Err(nix::errno::Errno::EINTR) => continue,
                Err(e) => return Err(anyhow!("recv_msg: recvmsg(prefix) failed: {e}")),
            }
        };
        // Harvest any cmsgs *before* checking for EOF — even on a 0-byte
        // recvmsg the cmsg can still be attached on Linux in some edge
        // cases, but more importantly we want to drain them once.
        for c in msg.cmsgs().context("decode cmsgs")? {
            if let ControlMessageOwned::ScmRights(rfds) = c {
                for fd in rfds {
                    // SAFETY: kernel just handed us a fresh fd; we own it.
                    owned_fds.push(unsafe { std::os::fd::FromRawFd::from_raw_fd(fd) });
                }
            }
        }
        if msg.bytes == 0 {
            return Err(anyhow!("recv_msg: peer closed connection"));
        }
        filled += msg.bytes;
    }
    let body_len = u32::from_be_bytes(prefix) as usize;
    if body_len > MAX_FRAME_BYTES {
        return Err(anyhow!(
            "recv_msg: body {body_len}B exceeds MAX_FRAME_BYTES={MAX_FRAME_BYTES}"
        ));
    }

    // ---------- Phase 2: read exactly body_len bytes ----------
    let mut body = vec![0u8; body_len];
    let mut reader = sock;
    reader
        .read_exact(&mut body)
        .map_err(|e| anyhow!("recv_msg: read_exact(body): {e}"))?;

    let decoded: T = serde_json::from_slice(&body).context("deserialize ipc message")?;
    Ok((decoded, owned_fds))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::proto::{ControlMsg, EventMsg, PROTOCOL_VERSION};
    use nix::sys::memfd::{memfd_create, MemFdCreateFlag};
    use std::ffi::CString;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};

    fn pair() -> (UnixStream, UnixStream) {
        UnixStream::pair().expect("socketpair")
    }

    #[test]
    fn roundtrip_control_no_fds() {
        let (a, b) = pair();
        let sent = ControlMsg::LoadScene {
            pkg: "scene.pkg".into(),
            assets: "/a".into(),
            fps: 30,
            width: 1280,
            height: 720,
        };
        send_msg(&a, &sent, &[]).unwrap();
        let (got, fds): (ControlMsg, _) = recv_msg(&b).unwrap();
        assert_eq!(sent, got);
        assert!(fds.is_empty());
    }

    #[test]
    fn roundtrip_hello() {
        let (a, b) = pair();
        let sent = ControlMsg::Hello {
            client: "test".into(),
            version: PROTOCOL_VERSION,
        };
        send_msg(&a, &sent, &[]).unwrap();
        let (got, _): (ControlMsg, _) = recv_msg(&b).unwrap();
        assert_eq!(sent, got);
    }

    /// Core iteration-0 test: send a BindBuffers event carrying a real
    /// memfd FD, verify the receiver can read the memfd contents through
    /// the fd it got back.
    #[test]
    fn roundtrip_bindbuffers_with_memfd() {
        let (a, b) = pair();

        // Make a memfd, write a known payload into it.
        let name = CString::new("waywallen-ipc-test").unwrap();
        let memfd =
            memfd_create(&name, MemFdCreateFlag::MFD_CLOEXEC).expect("memfd_create");
        // Convert nix OwnedFd → std File for writes.
        let mut f = unsafe {
            std::fs::File::from_raw_fd(memfd.into_raw_fd())
        };
        let payload = b"hello waywallen ipc";
        f.write_all(payload).unwrap();
        let fd_to_send: RawFd = f.as_raw_fd();

        let sent = EventMsg::BindBuffers {
            count: 1,
            fourcc: 0x34324152, // 'AR24' = DRM_FORMAT_ARGB8888
            width: 1280,
            height: 720,
            stride: 1280 * 4,
            modifier: 0, // DRM_FORMAT_MOD_LINEAR
            plane_offset: 0,
            sizes: vec![payload.len() as u64],
        };
        send_msg(&a, &sent, &[fd_to_send]).unwrap();

        let (got, fds): (EventMsg, _) = recv_msg(&b).unwrap();
        assert_eq!(sent, got);
        assert_eq!(fds.len(), 1);

        // The received fd must be a *different* fd number than the sender's,
        // proving the kernel really dup'd the description.
        let recv_raw = fds[0].as_raw_fd();
        assert_ne!(recv_raw, fd_to_send);

        // Read through the received fd and confirm the bytes match.
        let mut reader = unsafe { std::fs::File::from_raw_fd(fds[0].as_raw_fd()) };
        // Prevent the OwnedFd drop from double-closing.
        std::mem::forget(fds);
        reader.seek(SeekFrom::Start(0)).unwrap();
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).unwrap();
        assert_eq!(buf, payload);

        // Keep `f` alive so its fd isn't reclaimed mid-test.
        drop(f);
    }

    #[test]
    fn fd_limit_enforced() {
        let (a, _b) = pair();
        let fds: Vec<RawFd> = (0..(MAX_FDS_PER_MSG + 1) as i32).collect();
        let err = send_msg(
            &a,
            &ControlMsg::Play,
            &fds,
        )
        .unwrap_err();
        assert!(err.to_string().contains("MAX_FDS_PER_MSG"));
    }
}
