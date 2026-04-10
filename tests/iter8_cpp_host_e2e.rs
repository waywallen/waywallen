#![cfg(feature = "legacy_proto_tests")]
// Legacy viewer-protocol integration test; see Cargo.toml feature docs.

//! I4 — end-to-end daemon + display_endpoint with the C++ `waywallen-
//! renderer` host as the producer, running in `--test-pattern` mode so
//! no Wallpaper Engine assets directory is required.
//!
//! This is the first test that proves the *C++* producer can be driven
//! through `RendererManager` and its events rebroadcast to a
//! `display_endpoint` client — the same pipeline `waywallen_display_demo` will
//! hit when we wire in a real scene (I5).
//!
//! Skipped when `WAYWALLEN_RENDERER_BIN` is not set. Set it to the path
//! of the built C++ host, e.g.
//!   `open-wallpaper-engine/build/clang-release/host/waywallen-renderer`.

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
async fn cpp_host_test_pattern_end_to_end() {
    if std::env::var_os("WAYWALLEN_RENDERER_BIN").is_none() {
        eprintln!(
            "skipping iter8_cpp_host_e2e: set WAYWALLEN_RENDERER_BIN to the \
             built C++ waywallen-renderer binary to run this test"
        );
        return;
    }

    let mgr = Arc::new(RendererManager::new_default());
    let display_sock: PathBuf = std::env::temp_dir().join(format!(
        "waywallen-iter8-display-{}-{}.sock",
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

    // The C++ host refuses to create a swapchain smaller than 500x500
    // (VulkanRender::Impl::init logs an error below that threshold). Use
    // a realistic size.
    let id = mgr
        .spawn(SpawnRequest {
            wp_type: "scene".into(),
            metadata: std::collections::HashMap::new(),
            width: 1280,
            height: 720,
            fps: 30,
            test_pattern: true,
        })
        .await
        .expect("spawn C++ waywallen-renderer");

    let handle = mgr.get(&id).await.expect("renderer in map");
    let bind_arc = handle.bind_snapshot();
    let mut waited = Duration::ZERO;
    loop {
        if bind_arc.lock().unwrap().is_some() {
            break;
        }
        if waited > Duration::from_secs(10) {
            panic!("BindBuffers never cached in daemon snapshot");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        waited += Duration::from_millis(50);
    }

    // Connect as a display client and assert BindBuffers + >=6 FrameReady
    // with the `(prev+1) % 3` slot cycle that the test-pattern pump
    // produces. Do this on the blocking pool — current_thread runtime
    // starvation fix (iter6).
    let display_sock_for_client = display_sock.clone();
    let renderer_id_for_client = id.clone();
    let summary = tokio::task::spawn_blocking(move || -> String {
        let stream = UnixStream::connect(&display_sock_for_client)
            .expect("connect display endpoint");
        stream
            .set_read_timeout(Some(Duration::from_secs(8)))
            .expect("rd timeout");
        send_msg(
            &stream,
            &ViewerMsg::Hello {
                client: "iter8-cpp-host-e2e".to_string(),
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

        let (msg, fds): (EventMsg, Vec<OwnedFd>) =
            recv_msg(&stream).expect("recv BindBuffers");
        let (fourcc, width, height, stride, modifier) = match msg {
            EventMsg::BindBuffers {
                count,
                fourcc,
                width,
                height,
                stride,
                modifier,
                ..
            } => {
                assert_eq!(count, 3, "expected 3 slots from C++ host");
                (fourcc, width, height, stride, modifier)
            }
            other => panic!("expected BindBuffers, got {other:?}"),
        };
        assert_eq!(fds.len(), 3, "expected 3 FDs");
        assert_eq!(fourcc, DRM_FORMAT_ABGR8888, "C++ host must output ABGR8888");
        assert_eq!(modifier, 0, "C++ host must use DRM_FORMAT_MOD_LINEAR");
        assert_eq!(width, 1280);
        assert_eq!(height, 720);
        assert!(
            u64::from(stride) >= u64::from(width) * 4,
            "stride too small: {stride}"
        );

        // Drain >=6 FrameReady and check the slot cycle invariant.
        let mut frames: Vec<u32> = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(6);
        while frames.len() < 6 && std::time::Instant::now() < deadline {
            let (msg, _): (EventMsg, _) =
                recv_msg(&stream).expect("recv FrameReady");
            if let EventMsg::FrameReady { image_index, .. } = msg {
                frames.push(image_index);
            }
        }
        assert!(
            frames.len() >= 6,
            "expected >=6 FrameReady, got {}",
            frames.len()
        );
        // TripleSwapchain cycles the 3 slots in a stable order, but the
        // direction depends on the initial atomic assignment inside
        // VulkanExSwapchain — observed order from the C++ host is the
        // reverse of the Rust producer's. Assert the weaker, direction-
        // independent invariant: consecutive frames differ, and every
        // 3-frame sliding window covers all three slots.
        for w in frames.windows(2) {
            assert_ne!(w[0], w[1], "consecutive frames same slot: {frames:?}");
            assert!(w[0] < 3 && w[1] < 3, "slot id out of range: {frames:?}");
        }
        for w in frames.windows(3) {
            let mut seen = [false; 3];
            for &s in w {
                seen[s as usize] = true;
            }
            assert!(
                seen.iter().all(|&b| b),
                "3-frame window missed a slot: {w:?} in {frames:?}"
            );
        }
        format!(
            "C++ host OK: {width}x{height} fourcc=0x{fourcc:08x} \
             stride={stride} mod={modifier} frames={frames:?}"
        )
    })
    .await
    .expect("blocking join");
    eprintln!("[iter8] {summary}");

    mgr.kill(&id).await.expect("kill");
    endpoint.abort();
    let _ = std::fs::remove_file(&display_sock);
}
