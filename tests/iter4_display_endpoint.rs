#![cfg(feature = "legacy_proto_tests")]
// Legacy viewer-protocol integration test; see Cargo.toml feature docs.

//! Iteration 4 architecture-proven test.
//!
//! Spawns:
//!   - A `RendererManager` driving a `mock_renderer_host` subprocess
//!     (the synthetic Rust binary; behaves identically to the C++ host).
//!   - The `display_endpoint::serve` task on a tempfile UDS path.
//!
//! Then connects a display client manually, runs the Hello → Subscribe
//! handshake, asserts that `BindBuffers` arrives with three real fds and
//! that subsequent `FrameReady` events flow. mmaps slot 0 and verifies
//! the synthetic "MOCK0\n" pattern the mock host wrote into it. That
//! proves DMA-BUF metadata + FDs survive every leg of the round trip:
//!
//!   mock_renderer_host  ───sendmsg+SCM_RIGHTS─→  RendererManager
//!     reader thread     ───broadcast → snapshot───→  display_endpoint
//!     handle_client     ───sendmsg+SCM_RIGHTS─→  this test (mmap+verify)

use waywallen::ipc::proto::{EventMsg, ViewerMsg, PROTOCOL_VERSION};
use waywallen::ipc::uds::{recv_msg, send_msg};
use waywallen::renderer_manager::{RendererManager, SpawnRequest};
use waywallen::display_endpoint;

use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn end_to_end_dma_buf_dump() {
    // Point RendererManager at the mock host.
    // SAFETY: this test is single-threaded; no other tests are racing
    // for the env var.
    unsafe {
        std::env::set_var(
            "WAYWALLEN_RENDERER_BIN",
            env!("CARGO_BIN_EXE_mock_renderer_host"),
        );
    }

    let mgr = Arc::new(RendererManager::new());

    // Bind a per-test display socket so concurrent test runs don't
    // collide with the production default.
    let display_sock: PathBuf = std::env::temp_dir().join(format!(
        "waywallen-iter4-display-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mgr_clone = Arc::clone(&mgr);
    let display_sock_for_task = display_sock.clone();
    let endpoint = tokio::spawn(async move {
        let _ = display_endpoint::serve(&display_sock_for_task, mgr_clone).await;
    });
    // Give the listener a tick to bind.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Spawn the mock renderer host.
    let id = mgr
        .spawn(SpawnRequest {
            scene_pkg: String::new(),
            assets: String::new(),
            width: 64,
            height: 64,
            fps: 30,
            test_pattern: false,
        })
        .await
        .expect("spawn mock renderer");

    // Wait until the BindSnapshot is populated. The mock host sends
    // BindBuffers immediately after Ready, but it races the spawn()
    // return so a tiny poll loop is honest.
    let handle = mgr.get(&id).await.expect("renderer in map");
    let bind_arc = handle.bind_snapshot();
    let mut waited = Duration::ZERO;
    loop {
        if bind_arc.lock().unwrap().is_some() {
            break;
        }
        if waited > Duration::from_secs(5) {
            panic!("BindBuffers never cached");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        waited += Duration::from_millis(20);
    }

    // Now connect a display client.
    let stream =
        UnixStream::connect(&display_sock).expect("connect display socket");
    send_msg(
        &stream,
        &ViewerMsg::Hello {
            client: "iter4-test".to_string(),
            version: PROTOCOL_VERSION,
        },
        &[],
    )
    .expect("send Hello");
    send_msg(
        &stream,
        &ViewerMsg::Subscribe {
            renderer_id: id.clone(),
        },
        &[],
    )
    .expect("send Subscribe");

    // Read BindBuffers + 3 fds.
    let (msg, fds): (EventMsg, Vec<OwnedFd>) =
        recv_msg(&stream).expect("recv BindBuffers");
    let (count, width, height, stride) = match msg {
        EventMsg::BindBuffers {
            count,
            width,
            height,
            stride,
            ..
        } => (count, width, height, stride),
        other => panic!("expected BindBuffers, got {other:?}"),
    };
    assert_eq!(count, 3);
    assert_eq!(width, 64);
    assert_eq!(height, 64);
    assert_eq!(stride, 64 * 4);
    assert_eq!(fds.len(), 3, "expected three SCM_RIGHTS fds");

    // mmap slot 0 and verify the mock's synthetic "MOCK0\n" pattern.
    let len = (stride * height) as usize;
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ,
            libc::MAP_SHARED,
            fds[0].as_raw_fd(),
            0,
        )
    };
    assert!(ptr != libc::MAP_FAILED, "mmap slot 0");
    let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
    let head = std::str::from_utf8(&bytes[..6]).unwrap_or("?");
    assert_eq!(head, "MOCK0\n", "synthetic pattern in slot 0 fd");
    unsafe { libc::munmap(ptr, len) };

    // Drain a few FrameReady events to confirm the hot path streams.
    for expected in 0..5 {
        let (msg, fds): (EventMsg, _) =
            recv_msg(&stream).expect("recv FrameReady");
        assert!(fds.is_empty(), "FrameReady must not carry fds");
        match msg {
            EventMsg::FrameReady {
                image_index,
                seq,
                ts_ns,
                ..
            } => {
                assert!(image_index < 3, "image_index in [0,3): {image_index}");
                assert_eq!(seq, expected as u64, "seq monotonic");
                assert!(ts_ns > 0);
            }
            other => panic!("expected FrameReady, got {other:?}"),
        }
    }

    // Cleanup.
    mgr.kill(&id).await.expect("kill");
    endpoint.abort();
    let _ = std::fs::remove_file(&display_sock);
}
