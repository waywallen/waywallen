//! Framed wire codec for `waywallen-display-v1` over a blocking
//! `std::os::unix::net::UnixStream`.
//!
//! Wire frame:
//!
//!     [u16 LE opcode][u16 LE total_length][body...]
//!
//! where `total_length` includes the 4-byte header. Ancillary file
//! descriptors ride along as SCM_RIGHTS on the same `sendmsg(2)` /
//! `recvmsg(2)` call; their count must match the message's
//! `expected_fds()`.
//!
//! Tokio / async integration is deferred: today the only in-tree user
//! that needs this codec is `display_endpoint`, which already wraps
//! its I/O in `tokio::task::spawn_blocking` (same model as
//! `ipc::uds`). When a first-class async client appears we'll add a
//! tokio adapter on top.
//!
//! All errors are surfaced via [`CodecError`]; this module never
//! panics and never aborts.

use super::generated::{DecodeError, Event, Request};
use nix::sys::socket::{
    recvmsg, sendmsg, ControlMessage, ControlMessageOwned, MsgFlags,
};
use std::io::{IoSlice, IoSliceMut, Read};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

/// Hard cap on inline message body imposed by the u16 length field.
/// Equals `u16::MAX - 4` (header).
pub const MAX_BODY_BYTES: usize = u16::MAX as usize - 4;

/// Hard cap on SCM_RIGHTS fds per message. Matches the control-message
/// scratch buffer size. Generous for current needs (max observed: one
/// bind_buffers with ~8 planes) and bounded to keep the kernel cmsg
/// buffer stack-allocatable.
pub const MAX_FDS_PER_MSG: usize = 64;

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

pub fn send_request(
    sock: &UnixStream,
    req: &Request,
    fds: &[RawFd],
) -> CodecResult<()> {
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

pub fn send_event(
    sock: &UnixStream,
    evt: &Event,
    fds: &[RawFd],
) -> CodecResult<()> {
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

fn write_framed(
    sock: &UnixStream,
    opcode: u16,
    body: &[u8],
    fds: &[RawFd],
) -> CodecResult<()> {
    if fds.len() > MAX_FDS_PER_MSG {
        return Err(CodecError::TooManyFds(fds.len()));
    }
    let total = 4usize.checked_add(body.len()).ok_or(CodecError::FrameTooLarge(usize::MAX))?;
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

pub fn recv_request(sock: &UnixStream) -> CodecResult<(Request, Vec<OwnedFd>)> {
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

/// Read the 4-byte header (harvesting any ancillary fds that ride with
/// it) then read exactly `total_length - 4` body bytes via `read_exact`.
/// Back-to-back frames cannot leak fds because SCM_RIGHTS only ever
/// attaches to the first byte of a frame and we always call recvmsg
/// with a buffer ≤ the outstanding header bytes.
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
    use crate::display_proto::generated::Rect;
    use std::os::fd::IntoRawFd;

    fn pair() -> (UnixStream, UnixStream) {
        UnixStream::pair().expect("socketpair")
    }

    #[test]
    fn request_bye_roundtrip() {
        let (a, b) = pair();
        send_request(&a, &Request::Bye, &[]).unwrap();
        let (req, fds) = recv_request(&b).unwrap();
        assert_eq!(req, Request::Bye);
        assert!(fds.is_empty());
    }

    #[test]
    fn request_hello_roundtrip() {
        let (a, b) = pair();
        let sent = Request::Hello {
            protocol: "waywallen-display-v1".to_string(),
            client_name: "client".to_string(),
            client_version: "0.1.0".to_string(),
        };
        send_request(&a, &sent, &[]).unwrap();
        let (got, _) = recv_request(&b).unwrap();
        assert_eq!(got, sent);
    }

    #[test]
    fn event_welcome_with_features() {
        let (a, b) = pair();
        let sent = Event::Welcome {
            server_version: "waywallen 0.1.0".to_string(),
            features: vec!["explicit_sync_fd".to_string(), "hdr".to_string()],
        };
        send_event(&a, &sent, &[]).unwrap();
        let (got, _) = recv_event(&b).unwrap();
        assert_eq!(got, sent);
    }

    #[test]
    fn event_set_config_roundtrip() {
        let (a, b) = pair();
        let sent = Event::SetConfig {
            config_generation: 3,
            source_rect: Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 },
            dest_rect: Rect { x: 10.0, y: 20.0, w: 1900.0, h: 1060.0 },
            transform: 0,
            clear_r: 0.0,
            clear_g: 0.0,
            clear_b: 0.0,
            clear_a: 1.0,
        };
        send_event(&a, &sent, &[]).unwrap();
        let (got, _) = recv_event(&b).unwrap();
        assert_eq!(got, sent);
    }

    fn make_memfd() -> OwnedFd {
        use nix::sys::memfd::{memfd_create, MemFdCreateFlag};
        use std::ffi::CString;
        let name = CString::new("waywallen-codec-test").unwrap();
        memfd_create(&name, MemFdCreateFlag::MFD_CLOEXEC).expect("memfd_create")
    }

    #[test]
    fn event_bind_buffers_with_fds() {
        let (a, b) = pair();
        // 3 buffers × 1 plane = 3 fds expected.
        let fd1 = make_memfd();
        let fd2 = make_memfd();
        let fd3 = make_memfd();
        let raw_fds: Vec<RawFd> = vec![fd1.as_raw_fd(), fd2.as_raw_fd(), fd3.as_raw_fd()];

        let sent = Event::BindBuffers {
            buffer_generation: 1,
            count: 3,
            width: 1920,
            height: 1080,
            fourcc: 0x34325258, // 'XR24'
            modifier: 0,
            planes_per_buffer: 1,
            stride: vec![7680, 7680, 7680],
            plane_offset: vec![0, 0, 0],
            size: vec![8_294_400; 3],
        };
        assert_eq!(sent.expected_fds(), 3);
        send_event(&a, &sent, &raw_fds).unwrap();

        // Keep originals alive until after send.
        drop((fd1, fd2, fd3));

        let (got, got_fds) = recv_event(&b).unwrap();
        assert_eq!(got, sent);
        assert_eq!(got_fds.len(), 3);
    }

    #[test]
    fn event_frame_ready_with_fd() {
        let (a, b) = pair();
        let fd = make_memfd();
        let raw = fd.as_raw_fd();

        send_event(
            &a,
            &Event::FrameReady {
                buffer_generation: 1,
                buffer_index: 0,
                seq: 42,
            },
            &[raw],
        )
        .unwrap();
        drop(fd);

        let (got, fds) = recv_event(&b).unwrap();
        assert_eq!(
            got,
            Event::FrameReady {
                buffer_generation: 1,
                buffer_index: 0,
                seq: 42
            }
        );
        assert_eq!(fds.len(), 1);
    }

    #[test]
    fn missing_fd_is_rejected_on_send() {
        let (a, _b) = pair();
        let err = send_event(
            &a,
            &Event::FrameReady {
                buffer_generation: 1,
                buffer_index: 0,
                seq: 1,
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
    fn extra_fd_is_rejected_on_send() {
        let (a, _b) = pair();
        let fd = make_memfd();
        let raw = fd.into_raw_fd();
        let err = send_request(&a, &Request::Bye, &[raw]).unwrap_err();
        assert!(matches!(
            err,
            CodecError::FdCountMismatch { expected: 0, actual: 1 }
        ));
        // close the raw fd we passed in
        unsafe { libc::close(raw) };
    }

    #[test]
    fn back_to_back_frames_parse_independently() {
        let (a, b) = pair();
        send_request(&a, &Request::Bye, &[]).unwrap();
        send_request(
            &a,
            &Request::BufferRelease {
                buffer_generation: 1,
                buffer_index: 2,
                seq: 3,
            },
            &[],
        )
        .unwrap();
        let (r1, _) = recv_request(&b).unwrap();
        let (r2, _) = recv_request(&b).unwrap();
        assert_eq!(r1, Request::Bye);
        assert_eq!(
            r2,
            Request::BufferRelease {
                buffer_generation: 1,
                buffer_index: 2,
                seq: 3
            }
        );
    }

    #[test]
    fn peer_close_reported() {
        let (a, b) = pair();
        drop(a);
        let err = recv_request(&b).unwrap_err();
        assert!(matches!(err, CodecError::PeerClosed));
    }
}
