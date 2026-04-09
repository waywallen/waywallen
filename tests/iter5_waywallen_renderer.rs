//! Iteration 5 smoke test: spawn the Rust `waywallen_renderer` binary
//! against a listening Unix-domain socket, expect
//!
//!   1. `EventMsg::Ready`,
//!   2. `EventMsg::BindBuffers` carrying 3 DMA-BUF FDs with the
//!      fourcc/stride/modifier the renderer advertised,
//!   3. clean shutdown in response to `ControlMsg::Shutdown`.
//!
//! Uses the binary cargo builds into `CARGO_BIN_EXE_waywallen_renderer`
//! so no env var wiring is required; the test is self-contained.
//!
//! This asserts the M1.3b architectural contract: the Rust renderer
//! stands in for the C++ host over the same IPC wire format. Actual
//! per-frame rendering (M1.4) is out of scope here.

use waywallen::ipc::proto::{ControlMsg, EventMsg};
use waywallen::ipc::uds::{recv_msg, send_msg};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const DRM_FORMAT_ABGR8888: u32 = 0x34324241;

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn waywallen_renderer_bind_handshake() {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_waywallen_renderer"));
    assert!(bin.exists(), "renderer binary missing: {}", bin.display());

    let sock_path = std::env::temp_dir().join(format!(
        "waywallen-iter5-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&sock_path);

    let listener = UnixListener::bind(&sock_path).expect("bind uds listener");
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _cleanup = Cleanup(sock_path.clone());

    let child = Command::new(&bin)
        .arg("--ipc")
        .arg(&sock_path)
        .arg("--width")
        .arg("256")
        .arg("--height")
        .arg("256")
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn {}: {e}", bin.display()));
    let mut guard = ChildGuard(child);

    // Bounded accept.
    let (stream, _) = {
        let (tx, rx) = std::sync::mpsc::channel();
        let lc = listener.try_clone().expect("clone listener");
        std::thread::spawn(move || {
            let _ = tx.send(lc.accept());
        });
        match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(Ok(x)) => x,
            Ok(Err(e)) => panic!("accept: {e}"),
            Err(_) => {
                let _ = guard.0.kill();
                panic!("timed out waiting for renderer connect");
            }
        }
    };

    // 1. Ready, no fds.
    let (msg, fds) = recv_msg::<EventMsg>(&stream).expect("recv Ready");
    assert!(fds.is_empty(), "Ready must not carry fds");
    assert_eq!(msg, EventMsg::Ready);

    // 2. BindBuffers with 3 fds.
    let (msg, fds) = recv_msg::<EventMsg>(&stream).expect("recv BindBuffers");
    assert_eq!(fds.len(), 3, "expected 3 DMA-BUF fds");
    match msg {
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
            assert_eq!(count, 3);
            assert_eq!(
                fourcc, DRM_FORMAT_ABGR8888,
                "renderer advertised wrong fourcc 0x{fourcc:08x}"
            );
            assert_eq!(width, 256);
            assert_eq!(height, 256);
            assert!(stride >= 256 * 4, "stride {stride} below minimum");
            assert_eq!(modifier, 0, "expected DRM_FORMAT_MOD_LINEAR");
            assert_eq!(plane_offset, 0);
            assert_eq!(sizes.len(), 3);
            for &s in &sizes {
                assert_eq!(s, u64::from(stride) * u64::from(height));
            }
        }
        other => panic!("expected BindBuffers, got {other:?}"),
    }

    // 3. Drain 6 FrameReady events (2 full cycles) and assert that the
    //    slot index cycles 0,1,2,0,1,2 — i.e. the renderer's frame loop
    //    really is picking slots deterministically.
    let mut observed_slots = Vec::<u32>::new();
    let mut last_seq: i64 = -1;
    for _ in 0..6 {
        let (ev, fds) = recv_msg::<EventMsg>(&stream).expect("recv FrameReady");
        assert!(fds.is_empty(), "FrameReady must not carry fds");
        match ev {
            EventMsg::FrameReady {
                image_index,
                seq,
                ts_ns,
                ..
            } => {
                assert!(ts_ns > 0, "ts_ns must be monotonic");
                assert!((seq as i64) > last_seq, "seq must be monotonic");
                last_seq = seq as i64;
                observed_slots.push(image_index);
            }
            other => panic!("expected FrameReady, got {other:?}"),
        }
    }
    assert_eq!(observed_slots, vec![0, 1, 2, 0, 1, 2]);

    // Pixel-level verification via mmap is deliberately skipped: AMD
    // RADV allocates the DMA-BUFs in DEVICE_LOCAL VRAM, which isn't
    // host-visible and therefore fails mmap(MAP_SHARED). The proper
    // readback path is importing into a local Vulkan instance and
    // issuing a copy — that happens in the M2 viewer milestone.

    // 4. Send Shutdown and wait for the child to exit.
    send_msg(&stream, &ControlMsg::Shutdown, &[]).expect("send Shutdown");
    let (tx, rx) = std::sync::mpsc::channel();
    // Move child out of guard so we can wait() without dropping twice.
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(2500));
        let _ = tx.send(());
    });
    // Poll wait up to 3s.
    let start = std::time::Instant::now();
    loop {
        match guard.0.try_wait() {
            Ok(Some(status)) => {
                assert!(status.success(), "renderer exit status {status:?}");
                return;
            }
            Ok(None) => {
                if start.elapsed() > Duration::from_secs(3) {
                    panic!("renderer did not exit within 3s of Shutdown");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("wait: {e}"),
        }
    }
}
