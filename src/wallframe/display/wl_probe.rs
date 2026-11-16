//! One-shot `wl_registry` probe.
//!
//! Connects to the compositor, asks for the registry, waits for the
//! `wl_display.sync` callback and maps the advertised globals onto the
//! capability tokens used by display manifests (`requires = [...]`).
//!
//! The daemon only needs the global list, so this speaks the two requests and
//! three events it needs straight on the socket instead of pulling in a full
//! Wayland client stack.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const CAP_WLR_LAYER_SHELL: &str = "wlr-layer-shell";
pub const CAP_LINUX_DMABUF_V4: &str = "linux-dmabuf-v4";

const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_REPLY_BYTES: usize = 1 << 20;

const WL_DISPLAY_ID: u32 = 1;
const REGISTRY_ID: u32 = 2;
const CALLBACK_ID: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Global {
    pub interface: String,
    pub version: u32,
}

/// Resolve the compositor socket the way libwayland does for
/// `wl_display_connect(NULL)`. `None` when `WAYLAND_DISPLAY` is unset.
pub fn socket_path(wayland_display: Option<&str>, runtime_dir: Option<&Path>) -> Option<PathBuf> {
    let display = wayland_display.filter(|s| !s.is_empty())?;
    let display = Path::new(display);
    if display.is_absolute() {
        return Some(display.to_path_buf());
    }
    Some(runtime_dir?.join(display))
}

/// Probe the compositor and return capability tokens. Any failure yields an
/// empty list, which callers treat as "unknown".
pub fn probe_capabilities(wayland_display: Option<&str>) -> Vec<String> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let Some(path) = socket_path(wayland_display, runtime_dir.as_deref()) else {
        return Vec::new();
    };
    match list_globals(&path, PROBE_TIMEOUT) {
        Ok(globals) => {
            let caps = capabilities(&globals);
            log::debug!(
                "wl_registry probe {}: {} globals, caps={caps:?}",
                path.display(),
                globals.len()
            );
            caps
        }
        Err(e) => {
            log::debug!("wl_registry probe {} failed: {e}", path.display());
            Vec::new()
        }
    }
}

pub fn capabilities(globals: &[Global]) -> Vec<String> {
    let mut caps = Vec::new();
    if globals.iter().any(|g| g.interface == "zwlr_layer_shell_v1") {
        caps.push(CAP_WLR_LAYER_SHELL.to_string());
    }
    if globals
        .iter()
        .any(|g| g.interface == "zwp_linux_dmabuf_v1" && g.version >= 4)
    {
        caps.push(CAP_LINUX_DMABUF_V4.to_string());
    }
    caps
}

pub fn list_globals(path: &Path, timeout: Duration) -> std::io::Result<Vec<Global>> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_write_timeout(Some(timeout))?;

    let mut request = Vec::with_capacity(24);
    // wl_display.get_registry(new_id) is opcode 1, wl_display.sync(new_id) is 0.
    push_message(&mut request, WL_DISPLAY_ID, 1, &[REGISTRY_ID]);
    push_message(&mut request, WL_DISPLAY_ID, 0, &[CALLBACK_ID]);
    stream.write_all(&request)?;

    let deadline = Instant::now() + timeout;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(globals) = parse_events(&buf)? {
            return Ok(globals);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        stream.set_read_timeout(Some(left))?;
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > MAX_REPLY_BYTES {
            return Err(invalid("registry reply too large"));
        }
    }
}

fn push_message(out: &mut Vec<u8>, object: u32, opcode: u16, args: &[u32]) {
    let size = 8 + 4 * args.len() as u32;
    out.extend_from_slice(&object.to_ne_bytes());
    out.extend_from_slice(&((size << 16) | opcode as u32).to_ne_bytes());
    for arg in args {
        out.extend_from_slice(&arg.to_ne_bytes());
    }
}

/// Walk the complete messages in `buf`. Returns the globals once the sync
/// callback fired, `None` while more bytes are needed.
fn parse_events(buf: &[u8]) -> std::io::Result<Option<Vec<Global>>> {
    let mut globals = Vec::new();
    let mut off = 0;
    while buf.len() - off >= 8 {
        let object = read_u32(buf, off);
        let word = read_u32(buf, off + 4);
        let size = (word >> 16) as usize;
        let opcode = word & 0xffff;
        if size < 8 || size % 4 != 0 {
            return Err(invalid("malformed wayland message"));
        }
        if buf.len() - off < size {
            break;
        }
        let body = &buf[off + 8..off + size];
        match (object, opcode) {
            (WL_DISPLAY_ID, 0) => return Err(invalid("compositor sent wl_display.error")),
            (REGISTRY_ID, 0) => globals.push(parse_global(body)?),
            (CALLBACK_ID, 0) => return Ok(Some(globals)),
            _ => {}
        }
        off += size;
    }
    Ok(None)
}

fn parse_global(body: &[u8]) -> std::io::Result<Global> {
    // global(name: uint, interface: string, version: uint)
    if body.len() < 12 {
        return Err(invalid("short wl_registry.global"));
    }
    let len = read_u32(body, 4) as usize;
    let padded = (len + 3) & !3;
    if len == 0 || body.len() < 8 + padded + 4 {
        return Err(invalid("bad wl_registry.global string"));
    }
    let raw = &body[8..8 + len - 1];
    let interface = String::from_utf8_lossy(raw).into_owned();
    let version = read_u32(body, 8 + padded);
    Ok(Global { interface, version })
}

fn read_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_ne_bytes(buf[off..off + 4].try_into().unwrap())
}

fn invalid(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    fn global_event(name: u32, interface: &str, version: u32) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&name.to_ne_bytes());
        let len = interface.len() as u32 + 1;
        body.extend_from_slice(&len.to_ne_bytes());
        body.extend_from_slice(interface.as_bytes());
        body.push(0);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        body.extend_from_slice(&version.to_ne_bytes());
        let mut msg = Vec::new();
        let size = 8 + body.len() as u32;
        msg.extend_from_slice(&REGISTRY_ID.to_ne_bytes());
        msg.extend_from_slice(&(size << 16).to_ne_bytes());
        msg.extend_from_slice(&body);
        msg
    }

    fn done_event() -> Vec<u8> {
        let mut msg = Vec::new();
        push_message(&mut msg, CALLBACK_ID, 0, &[0]);
        msg
    }

    fn reply(globals: &[(&str, u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, (interface, version)) in globals.iter().enumerate() {
            out.extend(global_event(i as u32 + 1, interface, *version));
        }
        out.extend(done_event());
        out
    }

    #[test]
    fn socket_path_follows_libwayland_rules() {
        let rt = Path::new("/run/user/1000");
        assert_eq!(
            socket_path(Some("wayland-1"), Some(rt)),
            Some(PathBuf::from("/run/user/1000/wayland-1"))
        );
        assert_eq!(
            socket_path(Some("/tmp/wl.sock"), None),
            Some(PathBuf::from("/tmp/wl.sock"))
        );
        assert_eq!(socket_path(None, Some(rt)), None);
        assert_eq!(socket_path(Some(""), Some(rt)), None);
        assert_eq!(socket_path(Some("wayland-0"), None), None);
    }

    #[test]
    fn parses_globals_until_sync_done() {
        let bytes = reply(&[("wl_compositor", 6), ("zwlr_layer_shell_v1", 4)]);
        let globals = parse_events(&bytes).unwrap().unwrap();
        assert_eq!(
            globals,
            vec![
                Global {
                    interface: "wl_compositor".into(),
                    version: 6
                },
                Global {
                    interface: "zwlr_layer_shell_v1".into(),
                    version: 4
                },
            ]
        );
        // Truncated input waits for more bytes instead of failing.
        assert!(parse_events(&bytes[..bytes.len() - 4]).unwrap().is_none());
    }

    #[test]
    fn capabilities_need_layer_shell_and_dmabuf_v4() {
        let g = |i: &str, v| Global {
            interface: i.into(),
            version: v,
        };
        assert_eq!(
            capabilities(&[g("zwlr_layer_shell_v1", 4), g("zwp_linux_dmabuf_v1", 5)]),
            [CAP_WLR_LAYER_SHELL, CAP_LINUX_DMABUF_V4]
        );
        assert_eq!(
            capabilities(&[g("zwlr_layer_shell_v1", 1), g("zwp_linux_dmabuf_v1", 3)]),
            [CAP_WLR_LAYER_SHELL]
        );
        assert!(capabilities(&[g("zwp_linux_dmabuf_v1", 4)]) == [CAP_LINUX_DMABUF_V4]);
        assert!(capabilities(&[g("wl_shm", 1)]).is_empty());
    }

    #[test]
    fn rejects_wl_display_error() {
        let mut bytes = Vec::new();
        push_message(&mut bytes, WL_DISPLAY_ID, 0, &[1, 0, 0]);
        assert!(parse_events(&bytes).is_err());
    }

    #[test]
    fn probes_a_socket_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wayland-test");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut request = [0u8; 24];
            conn.read_exact(&mut request).unwrap();
            assert_eq!(read_u32(&request, 0), WL_DISPLAY_ID);
            assert_eq!(read_u32(&request, 4), (12 << 16) | 1);
            assert_eq!(read_u32(&request, 8), REGISTRY_ID);
            assert_eq!(read_u32(&request, 16), (12 << 16) | 0);
            assert_eq!(read_u32(&request, 20), CALLBACK_ID);
            // Split the reply to exercise partial reads.
            let bytes = reply(&[("zwlr_layer_shell_v1", 4), ("zwp_linux_dmabuf_v1", 4)]);
            let (a, b) = bytes.split_at(10);
            conn.write_all(a).unwrap();
            conn.flush().unwrap();
            std::thread::sleep(Duration::from_millis(20));
            conn.write_all(b).unwrap();
        });

        let globals = list_globals(&path, Duration::from_secs(2)).unwrap();
        server.join().unwrap();
        assert_eq!(
            capabilities(&globals),
            [CAP_WLR_LAYER_SHELL, CAP_LINUX_DMABUF_V4]
        );
    }

    #[test]
    fn silent_compositor_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wayland-silent");
        let _listener = UnixListener::bind(&path).unwrap();
        let err = list_globals(&path, Duration::from_millis(50)).unwrap_err();
        assert!(matches!(
            err.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ));
    }
}
