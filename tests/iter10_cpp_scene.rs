//! I5 — End-to-end test with a real Wallpaper Engine scene.
//!
//! Unlike iter8, this does NOT use `--test-pattern`. The C++ host is
//! driven through `SceneWallpaper::loadScene`, which means:
//!   - `PROPERTY_ASSETS` must point at a real Wallpaper Engine install
//!     `assets/` directory (shaders/materials/effects library).
//!   - `PROPERTY_SOURCE` must point at a workshop `scene.pkg`.
//!
//! Both come from env vars so the test skips cleanly on machines
//! without them:
//!   - `WAYWALLEN_RENDERER_BIN` — path to built C++ host binary
//!   - `WAYWALLEN_WE_ASSETS`    — path to WE `assets/` dir
//!   - `WAYWALLEN_WE_SCENE`     — path to a scene `.pkg` file
//!
//! Passes if: BindBuffers with 3 DMA-BUF FDs arrives within 10s of
//! spawn + ≥10 FrameReady events cycle the slots within 5s afterwards.
//! Some scenes log glslang / missing-texture warnings during load; the
//! test intentionally doesn't assert stderr is clean.

use waywallen::ipc::proto::{EventMsg, ViewerMsg, PROTOCOL_VERSION};
use waywallen::ipc::uds::{recv_msg, send_msg};
use waywallen::renderer_manager::{RendererManager, SpawnRequest};
use waywallen::viewer_endpoint;

use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const DRM_FORMAT_ABGR8888: u32 = 0x34324241;

fn env_path(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

#[tokio::test]
async fn real_scene_end_to_end() {
    let Some(_bin) = env_path("WAYWALLEN_RENDERER_BIN") else {
        eprintln!("skipping iter10_cpp_scene: WAYWALLEN_RENDERER_BIN unset");
        return;
    };
    let Some(assets) = env_path("WAYWALLEN_WE_ASSETS") else {
        eprintln!("skipping iter10_cpp_scene: WAYWALLEN_WE_ASSETS unset");
        return;
    };
    let Some(scene) = env_path("WAYWALLEN_WE_SCENE") else {
        eprintln!("skipping iter10_cpp_scene: WAYWALLEN_WE_SCENE unset");
        return;
    };
    assert!(
        std::path::Path::new(&assets).is_dir(),
        "WAYWALLEN_WE_ASSETS is not a directory: {assets}"
    );
    assert!(
        std::path::Path::new(&scene).is_file(),
        "WAYWALLEN_WE_SCENE is not a file: {scene}"
    );

    let mgr = Arc::new(RendererManager::new());
    let viewer_sock: PathBuf = std::env::temp_dir().join(format!(
        "waywallen-iter10-viewer-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mgr_clone = Arc::clone(&mgr);
    let viewer_sock_for_task = viewer_sock.clone();
    let endpoint = tokio::spawn(async move {
        let _ = viewer_endpoint::serve(&viewer_sock_for_task, mgr_clone).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let id = mgr
        .spawn(SpawnRequest {
            scene_pkg: scene.clone(),
            assets: assets.clone(),
            width: 1280,
            height: 720,
            fps: 30,
            test_pattern: false,
        })
        .await
        .expect("spawn C++ renderer with real scene");

    // Wait up to 15s for the daemon to cache the first BindBuffers. The
    // C++ host only emits BindBuffers from its redraw_callback, which
    // fires only after SceneWallpaper has successfully loaded the pkg
    // and rendered a first frame. Real scenes take longer than the
    // test-pattern path.
    let handle = mgr.get(&id).await.expect("renderer in map");
    let bind_arc = handle.bind_snapshot();
    let mut waited = Duration::ZERO;
    loop {
        if bind_arc.lock().unwrap().is_some() {
            break;
        }
        if waited > Duration::from_secs(15) {
            panic!("BindBuffers never cached within 15s for real scene");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        waited += Duration::from_millis(100);
    }

    // Subscribe as a viewer and collect frames.
    let viewer_sock_for_client = viewer_sock.clone();
    let renderer_id_for_client = id.clone();
    let summary = tokio::task::spawn_blocking(move || -> String {
        let stream = UnixStream::connect(&viewer_sock_for_client)
            .expect("connect viewer endpoint");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("rd timeout");
        send_msg(
            &stream,
            &ViewerMsg::Hello {
                client: "iter10-real-scene".to_string(),
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
                assert_eq!(count, 3);
                (fourcc, width, height, stride, modifier)
            }
            other => panic!("expected BindBuffers, got {other:?}"),
        };
        assert_eq!(fds.len(), 3);
        assert_eq!(fourcc, DRM_FORMAT_ABGR8888);
        assert_eq!(modifier, 0);

        // Drain ≥10 FrameReady events.
        let mut frames: Vec<u32> = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        while frames.len() < 10 && std::time::Instant::now() < deadline {
            let (msg, _): (EventMsg, _) =
                recv_msg(&stream).expect("recv FrameReady");
            if let EventMsg::FrameReady { image_index, .. } = msg {
                frames.push(image_index);
            }
        }
        assert!(
            frames.len() >= 10,
            "expected ≥10 FrameReady, got {}",
            frames.len()
        );
        for w in frames.windows(2) {
            assert_ne!(w[0], w[1], "consecutive frames same slot: {frames:?}");
            assert!(w[0] < 3 && w[1] < 3);
        }
        format!(
            "real-scene OK: {width}x{height} fourcc=0x{fourcc:08x} \
             stride={stride} mod={modifier} frame_slots={frames:?}"
        )
    })
    .await
    .expect("blocking join");
    eprintln!("[iter10] {summary}");

    mgr.kill(&id).await.expect("kill");
    endpoint.abort();
    let _ = std::fs::remove_file(&viewer_sock);
}
