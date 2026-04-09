//! Iteration 6 end-to-end test: real GPU path.
//!
//! Wires together the same daemon + display_endpoint + client rig as
//! iter4_display_endpoint, but replaces the synthetic `mock_renderer_host`
//! with the real `waywallen_renderer` ash binary. Asserts that:
//!
//!   1. `BindBuffers` arrives with 3 DMA-BUF fds, advertising the
//!      renderer's actual DRM fourcc / stride / modifier.
//!   2. The first 6 `FrameReady` events cycle image_index through
//!      `[0,1,2,0,1,2]`, matching the renderer's `seq % 3` slot
//!      selection.
//!
//! Pixel-level verification via mmap is intentionally omitted: RADV
//! allocates the DMA-BUFs in DEVICE_LOCAL VRAM, which isn't host-visible
//! and therefore fails CPU mmap. Proper pixel readback happens in M2.4
//! once the display imports the FDs back into its own Vulkan instance.

use waywallen::ipc::proto::{EventMsg, ViewerMsg, PROTOCOL_VERSION};
use waywallen::ipc::uds::{recv_msg, send_msg};
use waywallen::renderer_manager::{RendererManager, SpawnRequest};
use waywallen::display_endpoint;

use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const DRM_FORMAT_ABGR8888: u32 = 0x34324241;

#[tokio::test]
async fn real_renderer_display_handshake_and_frames() {
    // SAFETY: single-threaded test runtime.
    unsafe {
        std::env::set_var(
            "WAYWALLEN_RENDERER_BIN",
            env!("CARGO_BIN_EXE_waywallen_renderer"),
        );
    }

    let mgr = Arc::new(RendererManager::new());

    let display_sock: PathBuf = std::env::temp_dir().join(format!(
        "waywallen-iter6-display-{}-{}.sock",
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
    tokio::time::sleep(Duration::from_millis(50)).await;

    let id = mgr
        .spawn(SpawnRequest {
            scene_pkg: String::new(),
            assets: String::new(),
            width: 256,
            height: 256,
            fps: 30,
            test_pattern: false,
        })
        .await
        .expect("spawn waywallen_renderer");

    // Wait for BindSnapshot. The real renderer does Vulkan init before
    // sending it, so be generous with the timeout.
    let handle = mgr.get(&id).await.expect("renderer in map");
    let bind_arc = handle.bind_snapshot();
    let mut waited = Duration::ZERO;
    loop {
        if bind_arc.lock().unwrap().is_some() {
            break;
        }
        if waited > Duration::from_secs(10) {
            panic!("BindBuffers never cached (real renderer)");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        waited += Duration::from_millis(50);
    }

    // The client interaction is synchronous (plain UnixStream +
    // recv_msg), so it must run off the tokio runtime thread —
    // otherwise sync blocks would freeze the display_endpoint accept
    // task and the test would deadlock waiting for its own server.
    let display_sock_for_client = display_sock.clone();
    let renderer_id_for_client = id.clone();
    let (msg, fds): (EventMsg, Vec<OwnedFd>) = tokio::task::spawn_blocking(move || {
        let stream = UnixStream::connect(&display_sock_for_client).expect("connect display socket");
        send_msg(
            &stream,
            &ViewerMsg::Hello {
                client: "iter6-test".to_string(),
                version: PROTOCOL_VERSION,
            },
            &[],
        )
        .expect("send Hello");
        send_msg(
            &stream,
            &ViewerMsg::Subscribe {
                renderer_id: renderer_id_for_client,
            },
            &[],
        )
        .expect("send Subscribe");
        let (msg, fds) = recv_msg::<EventMsg>(&stream).expect("recv BindBuffers");

        // Drain 6 FrameReady events while we're on this blocking
        // thread. We can't pin the starting slot because the viewer
        // may subscribe after the renderer already emitted frame 0 —
        // instead assert that each slot equals (prev + 1) mod 3, which
        // is the invariant the renderer's `seq % 3` scheduler must
        // satisfy regardless of where we joined.
        let mut slots = Vec::<u32>::new();
        for _ in 0..6 {
            let (m, f) = recv_msg::<EventMsg>(&stream).expect("recv FrameReady");
            assert!(f.is_empty());
            match m {
                EventMsg::FrameReady { image_index, .. } => slots.push(image_index),
                other => panic!("expected FrameReady, got {other:?}"),
            }
        }
        for w in slots.windows(2) {
            assert_eq!(
                (w[0] + 1) % 3,
                w[1],
                "slot cycle broken: {slots:?}"
            );
        }
        (msg, fds)
    })
    .await
    .expect("blocking client join");
    let (count, fourcc, width, height, stride, modifier) = match msg {
        EventMsg::BindBuffers {
            count,
            fourcc,
            width,
            height,
            stride,
            modifier,
            ..
        } => (count, fourcc, width, height, stride, modifier),
        other => panic!("expected BindBuffers, got {other:?}"),
    };
    assert_eq!(count, 3);
    assert_eq!(fourcc, DRM_FORMAT_ABGR8888);
    assert_eq!(width, 256);
    assert_eq!(height, 256);
    assert!(stride >= 256 * 4, "stride {stride} below minimum");
    assert_eq!(modifier, 0, "expected DRM_FORMAT_MOD_LINEAR");
    assert_eq!(fds.len(), 3, "expected three SCM_RIGHTS fds");

    mgr.kill(&id).await.expect("kill");
    endpoint.abort();
    let _ = std::fs::remove_file(&display_sock);
}
