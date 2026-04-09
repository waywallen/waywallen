//! mock_renderer_host — drop-in replacement for the C++ `waywallen-renderer`
//! used by integration tests that need a deterministic host without a real
//! GPU or scene `.pkg`.
//!
//! Mimics the host's protocol exactly:
//!   1. Connects to the UDS path passed via `--ipc`.
//!   2. Sends `EventMsg::Ready`.
//!   3. Creates three `memfd_create` buffers, fills each with a distinct
//!      synthetic pattern ("MOCK0\n", "MOCK1\n", "MOCK2\n" repeated to fill
//!      the first 1 KiB; the rest is left zero so mmap reads still work).
//!   4. Sends a single `BindBuffers` event carrying all three FDs via
//!      SCM_RIGHTS.
//!   5. Loops sending `FrameReady { image_index = (seq % 3) }` events at
//!      `--fps` until it receives a `Shutdown` control message or the
//!      socket is closed.
//!   6. Terminates cleanly.
//!
//! Wire format and message schema match the production host exactly so
//! the daemon can't tell them apart.

use waywallen::ipc::proto::{ControlMsg, EventMsg};
use waywallen::ipc::uds::{recv_msg, send_msg};

use nix::sys::memfd::{memfd_create, MemFdCreateFlag};
use std::ffi::CString;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 64;
const STRIDE: u32 = WIDTH * 4; // RGBA8
const SIZE: u64 = (STRIDE as u64) * (HEIGHT as u64);
const DRM_FORMAT_ABGR8888: u32 = 0x34324241;

fn parse_args() -> (String, u32) {
    let mut ipc_path = None;
    let mut fps: u32 = 10;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--ipc" => ipc_path = args.next(),
            "--fps" => fps = args.next().and_then(|s| s.parse().ok()).unwrap_or(10),
            // Accept and ignore the same flags the real host accepts so a
            // shared spawner contract works for both.
            "--width" | "--height" | "--scene" | "--assets" => {
                let _ = args.next();
            }
            _ => {}
        }
    }
    let ipc_path = ipc_path.unwrap_or_else(|| {
        eprintln!("mock_renderer_host: --ipc <path> is required");
        std::process::exit(1);
    });
    (ipc_path, fps)
}

fn make_memfd(slot: u8) -> std::fs::File {
    let name = CString::new(format!("waywallen-mock-slot-{slot}")).unwrap();
    let fd = memfd_create(&name, MemFdCreateFlag::MFD_CLOEXEC).expect("memfd_create");
    let mut f = unsafe { std::fs::File::from_raw_fd(fd.into_raw_fd()) };
    f.set_len(SIZE).expect("set_len");
    let pattern = format!("MOCK{slot}\n");
    let mut buf = Vec::with_capacity(1024);
    while buf.len() + pattern.len() <= 1024 {
        buf.extend_from_slice(pattern.as_bytes());
    }
    f.write_all(&buf).expect("write pattern");
    f
}

fn main() {
    let (ipc_path, fps) = parse_args();
    let stream = StdUnixStream::connect(&ipc_path)
        .unwrap_or_else(|e| panic!("connect {ipc_path}: {e}"));
    let stream = Arc::new(Mutex::new(stream));

    // Send Ready first (matches the real host).
    {
        let s = stream.lock().unwrap();
        send_msg(&*s, &EventMsg::Ready, &[]).expect("send Ready");
    }

    // Create the 3 memfd slots, then send a single BindBuffers carrying
    // all three fds.
    let slots = [make_memfd(0), make_memfd(1), make_memfd(2)];
    let bind = EventMsg::BindBuffers {
        count: 3,
        fourcc: DRM_FORMAT_ABGR8888,
        width: WIDTH,
        height: HEIGHT,
        stride: STRIDE,
        modifier: 0, // DRM_FORMAT_MOD_LINEAR
        plane_offset: 0,
        sizes: vec![SIZE, SIZE, SIZE],
    };
    let fds: Vec<RawFd> = slots.iter().map(|f| f.as_raw_fd()).collect();
    {
        let s = stream.lock().unwrap();
        send_msg(&*s, &bind, &fds).expect("send BindBuffers");
    }

    // Spawn a control reader so Shutdown wakes us out of the frame loop.
    let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let stream = stream.clone();
        let shutdown = shutdown.clone();
        let read_stream = stream.lock().unwrap().try_clone().expect("clone for reader");
        thread::spawn(move || loop {
            match recv_msg::<ControlMsg>(&read_stream) {
                Ok((ControlMsg::Shutdown, _)) => {
                    shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
                Ok(_) => continue,
                Err(_) => {
                    shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            }
        });
    }

    // Frame loop. Use steady_clock to keep cadence honest.
    let frame_period = Duration::from_secs_f64(1.0 / fps.max(1) as f64);
    let start = Instant::now();
    let mut seq: u64 = 0;
    while !shutdown.load(std::sync::atomic::Ordering::SeqCst) {
        let next = start + frame_period * (seq as u32 + 1);
        let frame = EventMsg::FrameReady {
            image_index: (seq % 3) as u32,
            seq,
            ts_ns: now_ns(),
            has_sync_fd: false,
        };
        {
            let s = stream.lock().unwrap();
            if send_msg(&*s, &frame, &[]).is_err() {
                break;
            }
        }
        seq += 1;
        let now = Instant::now();
        if next > now {
            thread::sleep(next - now);
        }
    }
}

fn now_ns() -> u64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}
