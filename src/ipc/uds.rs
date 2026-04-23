//! Blocking Unix-domain-socket framing helpers for the waywallen IPC.
//!
//! Wire frame (same as `waywallen-display-v1`):
//!
//!     [u16 LE opcode][u16 LE total_length][body...]
//!
//! where `total_length` includes the 4-byte header. Ancillary file
//! descriptors ride along as SCM_RIGHTS on the same `sendmsg(2)` /
//! `recvmsg(2)` call; their count must match the message's
//! `expected_fds()`.
//!
//! This module intentionally uses `std::os::unix::net::UnixStream`
//! (not Tokio). Async callers are expected to wrap calls in
//! `spawn_blocking`, the same model `display_endpoint` uses.

use crate::ipc::generated::{DecodeError, Event, Request};
use nix::sys::socket::{
    recvmsg, sendmsg, ControlMessage, ControlMessageOwned, MsgFlags,
};
use std::io::{IoSlice, IoSliceMut, Read};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

/// Hard cap on the inline message body imposed by the u16 length field.
/// Equals `u16::MAX - 4` (header).
pub const MAX_BODY_BYTES: usize = u16::MAX as usize - 4;

/// Hard cap on SCM_RIGHTS fds per message. Generous for current needs
/// (max observed: one bind_buffers with ~8 planes).
pub const MAX_FDS_PER_MSG: usize = 64;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum CodecError {
    Io(std::io::Error),
    Nix(nix::errno::Errno),
    PeerClosed,
    FrameTooLarge(usize),
    BadFrameLen(u16),
    TooManyFds(usize),
    FdCountMismatch { expected: u32, actual: usize },
    Decode(DecodeError),
    ShortWrite { sent: usize, total: usize },
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Nix(e) => write!(f, "syscall: {e}"),
            Self::PeerClosed => write!(f, "peer closed connection"),
            Self::FrameTooLarge(n) => write!(f, "frame too large: {n}B"),
            Self::BadFrameLen(n) => write!(f, "bad frame length: {n}"),
            Self::TooManyFds(n) => write!(f, "too many fds: {n}"),
            Self::FdCountMismatch { expected, actual } => {
                write!(f, "fd count mismatch: expected {expected}, got {actual}")
            }
            Self::Decode(e) => write!(f, "decode: {e}"),
            Self::ShortWrite { sent, total } => {
                write!(f, "short write {sent}/{total}")
            }
        }
    }
}

impl std::error::Error for CodecError {}

impl From<DecodeError> for CodecError {
    fn from(e: DecodeError) -> Self {
        Self::Decode(e)
    }
}
impl From<std::io::Error> for CodecError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<nix::errno::Errno> for CodecError {
    fn from(e: nix::errno::Errno) -> Self {
        Self::Nix(e)
    }
}

pub type CodecResult<T> = Result<T, CodecError>;

// ---------------------------------------------------------------------------
// Send path
// ---------------------------------------------------------------------------

/// Send a control message (daemon → subprocess).
pub fn send_control(sock: &UnixStream, req: &Request, fds: &[RawFd]) -> CodecResult<()> {
    if fds.len() > MAX_FDS_PER_MSG {
        return Err(CodecError::TooManyFds(fds.len()));
    }
    let expected = req.expected_fds();
    if fds.len() != expected as usize {
        return Err(CodecError::FdCountMismatch {
            expected,
            actual: fds.len(),
        });
    }
    let mut body = Vec::new();
    req.encode(&mut body);
    write_framed(sock, req.opcode(), &body, fds)
}

/// Send an event (subprocess → daemon).
pub fn send_event(sock: &UnixStream, evt: &Event, fds: &[RawFd]) -> CodecResult<()> {
    if fds.len() > MAX_FDS_PER_MSG {
        return Err(CodecError::TooManyFds(fds.len()));
    }
    let expected = evt.expected_fds();
    if fds.len() != expected as usize {
        return Err(CodecError::FdCountMismatch {
            expected,
            actual: fds.len(),
        });
    }
    let mut body = Vec::new();
    evt.encode(&mut body);
    write_framed(sock, evt.opcode(), &body, fds)
}

/// Historical alias for `send_control`. Kept so renderer_manager and
/// other call sites that previously used the generic `send_msg` with a
/// `ControlMsg` don't need rewriting.
#[inline]
pub fn send_msg_control(sock: &UnixStream, msg: &Request, fds: &[RawFd]) -> CodecResult<()> {
    send_control(sock, msg, fds)
}

fn write_framed(
    sock: &UnixStream,
    opcode: u16,
    body: &[u8],
    fds: &[RawFd],
) -> CodecResult<()> {
    if fds.len() > MAX_FDS_PER_MSG {
        return Err(CodecError::TooManyFds(fds.len()));
    }
    let total = 4usize
        .checked_add(body.len())
        .ok_or(CodecError::FrameTooLarge(usize::MAX))?;
    if total > u16::MAX as usize {
        return Err(CodecError::FrameTooLarge(total));
    }

    let mut framed = Vec::with_capacity(total);
    framed.extend_from_slice(&opcode.to_le_bytes());
    framed.extend_from_slice(&(total as u16).to_le_bytes());
    framed.extend_from_slice(body);

    let iov = [IoSlice::new(&framed)];
    let cmsgs_storage;
    let cmsgs: &[ControlMessage] = if fds.is_empty() {
        &[]
    } else {
        cmsgs_storage = [ControlMessage::ScmRights(fds)];
        &cmsgs_storage
    };

    loop {
        match sendmsg::<()>(sock.as_raw_fd(), &iov, cmsgs, MsgFlags::MSG_NOSIGNAL, None) {
            Ok(n) if n == framed.len() => return Ok(()),
            Ok(n) => {
                return Err(CodecError::ShortWrite {
                    sent: n,
                    total: framed.len(),
                })
            }
            Err(nix::errno::Errno::EINTR) => continue,
            Err(e) => return Err(CodecError::Nix(e)),
        }
    }
}

// ---------------------------------------------------------------------------
// Receive path
// ---------------------------------------------------------------------------

/// Receive a control message (daemon → subprocess). Used by renderer
/// subprocesses.
pub fn recv_control(sock: &UnixStream) -> CodecResult<(Request, Vec<OwnedFd>)> {
    let (opcode, body, fds) = read_framed(sock)?;
    let req = Request::decode(opcode, &body)?;
    let expected = req.expected_fds();
    if fds.len() != expected as usize {
        return Err(CodecError::FdCountMismatch {
            expected,
            actual: fds.len(),
        });
    }
    Ok((req, fds))
}

/// Receive an event (subprocess → daemon). Used by the daemon.
pub fn recv_event(sock: &UnixStream) -> CodecResult<(Event, Vec<OwnedFd>)> {
    let (opcode, body, fds) = read_framed(sock)?;
    let evt = Event::decode(opcode, &body)?;
    let expected = evt.expected_fds();
    if fds.len() != expected as usize {
        return Err(CodecError::FdCountMismatch {
            expected,
            actual: fds.len(),
        });
    }
    Ok((evt, fds))
}

/// Read the 4-byte header (harvesting any ancillary fds that ride
/// with it) then `read_exact` the body bytes. SCM_RIGHTS only ever
/// attaches to the first byte of a frame, so reading a body via plain
/// `read_exact` never discards fds from subsequent frames.
fn read_framed(sock: &UnixStream) -> CodecResult<(u16, Vec<u8>, Vec<OwnedFd>)> {
    let mut hdr = [0u8; 4];
    let mut fds: Vec<OwnedFd> = Vec::new();
    let mut filled = 0usize;
    while filled < 4 {
        let mut cmsg_space = nix::cmsg_space!([RawFd; MAX_FDS_PER_MSG]);
        let mut iov = [IoSliceMut::new(&mut hdr[filled..])];
        let msg = loop {
            match recvmsg::<()>(
                sock.as_raw_fd(),
                &mut iov,
                Some(&mut cmsg_space),
                MsgFlags::empty(),
            ) {
                Ok(m) => break m,
                Err(nix::errno::Errno::EINTR) => continue,
                Err(e) => return Err(CodecError::Nix(e)),
            }
        };
        for c in msg.cmsgs().map_err(CodecError::Nix)? {
            if let ControlMessageOwned::ScmRights(rfds) = c {
                for fd in rfds {
                    // SAFETY: kernel just handed us a fresh fd; we own it.
                    fds.push(unsafe { std::os::fd::FromRawFd::from_raw_fd(fd) });
                }
            }
        }
        if msg.bytes == 0 {
            return Err(CodecError::PeerClosed);
        }
        filled += msg.bytes;
    }
    let opcode = u16::from_le_bytes([hdr[0], hdr[1]]);
    let total = u16::from_le_bytes([hdr[2], hdr[3]]);
    if total < 4 {
        return Err(CodecError::BadFrameLen(total));
    }
    let body_len = (total - 4) as usize;

    let mut body = vec![0u8; body_len];
    let mut s = sock;
    s.read_exact(&mut body)?;
    Ok((opcode, body, fds))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::generated::{Event, Request, PROTOCOL_VERSION};
    use nix::sys::memfd::{memfd_create, MemFdCreateFlag};
    use std::ffi::CString;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::fd::{FromRawFd, IntoRawFd};

    fn pair() -> (UnixStream, UnixStream) {
        UnixStream::pair().expect("socketpair")
    }

    #[test]
    fn roundtrip_control_no_fds() {
        let (a, b) = pair();
        let sent = Request::LoadScene {
            pkg: "scene.pkg".into(),
            assets: "/a".into(),
            fps: 30,
            width: 1280,
            height: 720,
        };
        send_control(&a, &sent, &[]).unwrap();
        let (got, fds) = recv_control(&b).unwrap();
        assert_eq!(sent, got);
        assert!(fds.is_empty());
    }

    #[test]
    fn roundtrip_hello() {
        let (a, b) = pair();
        let sent = Request::Hello {
            client: "test".into(),
            version: PROTOCOL_VERSION,
        };
        send_control(&a, &sent, &[]).unwrap();
        let (got, _) = recv_control(&b).unwrap();
        assert_eq!(sent, got);
    }

    #[test]
    fn roundtrip_play_pause_shutdown() {
        let (a, b) = pair();
        for msg in [Request::Play, Request::Pause, Request::Shutdown] {
            send_control(&a, &msg, &[]).unwrap();
            let (got, _) = recv_control(&b).unwrap();
            assert_eq!(msg, got);
        }
    }

    fn make_memfd() -> OwnedFd {
        let name = CString::new("waywallen-ipc-test").unwrap();
        memfd_create(&name, MemFdCreateFlag::MFD_CLOEXEC).expect("memfd_create")
    }

    /// Core iteration-0 test: send a BindBuffers event carrying a real
    /// memfd FD, verify the receiver can read the memfd contents through
    /// the fd it got back.
    #[test]
    fn roundtrip_bindbuffers_with_memfd() {
        let (a, b) = pair();

        let memfd = make_memfd();
        let mut f = unsafe { std::fs::File::from_raw_fd(memfd.into_raw_fd()) };
        let payload = b"hello waywallen ipc";
        f.write_all(payload).unwrap();
        let fd_to_send: RawFd = f.as_raw_fd();

        let sent = Event::BindBuffers {
            count: 1,
            fourcc: 0x34324152, // 'AR24'
            width: 1280,
            height: 720,
            stride: 1280 * 4,
            modifier: 0,
            plane_offset: 0,
            sizes: vec![payload.len() as u64],
        };
        send_event(&a, &sent, &[fd_to_send]).unwrap();

        let (got, fds) = recv_event(&b).unwrap();
        assert_eq!(sent, got);
        assert_eq!(fds.len(), 1);

        let recv_raw = fds[0].as_raw_fd();
        assert_ne!(recv_raw, fd_to_send);

        let mut reader = unsafe { std::fs::File::from_raw_fd(fds[0].as_raw_fd()) };
        std::mem::forget(fds);
        reader.seek(SeekFrom::Start(0)).unwrap();
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).unwrap();
        assert_eq!(buf, payload);

        drop(f);
    }

    #[test]
    fn frame_ready_requires_sync_fd() {
        let (a, _b) = pair();
        let err = send_event(
            &a,
            &Event::FrameReady {
                image_index: 0,
                seq: 1,
                ts_ns: 0,
            },
            &[],
        )
        .unwrap_err();
        assert!(matches!(
            err,
            CodecError::FdCountMismatch { expected: 1, actual: 0 }
        ));
    }

    #[test]
    fn fd_limit_enforced() {
        let (a, _b) = pair();
        let fds: Vec<RawFd> = (0..(MAX_FDS_PER_MSG + 1) as i32).collect();
        let err = send_control(&a, &Request::Play, &fds).unwrap_err();
        assert!(matches!(err, CodecError::TooManyFds(_)));
    }

    #[test]
    fn back_to_back_frames() {
        let (a, b) = pair();
        send_control(&a, &Request::Play, &[]).unwrap();
        send_control(
            &a,
            &Request::SetFps { fps: 60 },
            &[],
        )
        .unwrap();
        let (r1, _) = recv_control(&b).unwrap();
        let (r2, _) = recv_control(&b).unwrap();
        assert_eq!(r1, Request::Play);
        assert_eq!(r2, Request::SetFps { fps: 60 });
    }

    #[test]
    fn peer_close_reported() {
        let (a, b) = pair();
        drop(a);
        let err = recv_control(&b).unwrap_err();
        assert!(matches!(err, CodecError::PeerClosed));
    }
}
