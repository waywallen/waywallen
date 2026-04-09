//! Iteration 9 integration test: Producer Respawn.
//!
//! DISABLED pending `RendererManager::respawn` implementation — the
//! method doesn't exist yet; this file was checked in speculatively
//! from an earlier plan. Re-enable by removing the `cfg(any())` gate
//! once respawn lands.
#![cfg(any())]
//!
//! Verifies that when a renderer is respawned:
//! 1. The RendererManager correctly kills the old process and starts a new one.
//! 2. Existing viewer connections receive a new BindBuffers event.
//! 3. The new BindBuffers carries fresh FDs.
//! 4. FrameReady events continue to flow from the new process.

use waywallen::ipc::proto::{EventMsg, ViewerMsg, PROTOCOL_VERSION};
use waywallen::ipc::uds::{recv_msg, send_msg};
use waywallen::renderer_manager::{RendererManager, SpawnRequest};
use waywallen::viewer_endpoint;

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn producer_respawn_test() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init()
        .ok();

    // Point RendererManager at the mock host.
    unsafe {
        std::env::set_var(
            "WAYWALLEN_RENDERER_BIN",
            env!("CARGO_BIN_EXE_mock_renderer_host"),
        );
    }

    let mgr = Arc::new(RendererManager::new());
    let viewer_sock: PathBuf = std::env::temp_dir().join(format!(
        "waywallen-iter9-viewer-{}-{}.sock",
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
    tokio::time::sleep(Duration::from_millis(200)).await;

    // 1. Spawn initial renderer.
    println!("[iter9] Spawning initial renderer...");
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
        .expect("spawn initial renderer");

    // 2. Connect viewer.
    println!("[iter9] Connecting viewer client...");
    let stream = UnixStream::connect(&viewer_sock).expect("connect viewer socket");
    send_msg(&stream, &ViewerMsg::Hello { client: "iter9-test".to_string(), version: PROTOCOL_VERSION }, &[]).expect("send Hello");
    send_msg(&stream, &ViewerMsg::Subscribe { renderer_id: id.clone() }, &[]).expect("send Subscribe");

    // 3. Receive first BindBuffers.
    println!("[iter9] Waiting for first BindBuffers...");
    let (msg, fds1) = recv_msg::<EventMsg>(&stream).expect("recv first BindBuffers");
    assert!(matches!(msg, EventMsg::BindBuffers { .. }));
    assert_eq!(fds1.len(), 3);

    // 4. Verify first batch of FrameReady.
    println!("[iter9] Waiting for first FrameReady...");
    let (msg, _) = recv_msg::<EventMsg>(&stream).expect("recv first FrameReady");
    assert!(matches!(msg, EventMsg::FrameReady { seq: 0, .. }));

    // 5. Trigger respawn.
    println!("[iter9] Triggering respawn...");
    mgr.respawn(&id).await.expect("respawn");

    // 6. Receive second BindBuffers on the SAME stream.
    // Crucial: mock_renderer_host at 30fps might have spammed the buffer
    // with FrameReady messages. We must drain until we see BindBuffers.
    println!("[iter9] Draining old frames and waiting for second BindBuffers...");
    let mut fds2 = vec![];
    let mut bind_found = false;
    
    for i in 0..2000 {
        let (msg, fds) = recv_msg::<EventMsg>(&stream).expect("recv event during drain");
        match msg {
            EventMsg::BindBuffers { .. } => {
                println!("[iter9] Found second BindBuffers after {} FrameReady messages!", i);
                fds2 = fds;
                bind_found = true;
                break;
            }
            EventMsg::FrameReady { .. } => {
                continue;
            }
            other => panic!("unexpected message during drain: {:?}", other),
        }
    }
    
    assert!(bind_found, "Failed to find second BindBuffers after respawn");
    assert_eq!(fds2.len(), 3);
    assert_ne!(fds1[0].as_raw_fd(), fds2[0].as_raw_fd(), "FDs must be different");

    // 7. Verify subsequent FrameReady events from the NEW process (seq reset).
    println!("[iter9] Waiting for new FrameReady...");
    let (msg, _) = recv_msg::<EventMsg>(&stream).expect("recv new FrameReady");
    assert!(matches!(msg, EventMsg::FrameReady { seq: 0, .. }));

    // Cleanup.
    println!("[iter9] Cleaning up...");
    let _ = mgr.kill(&id).await;
    endpoint.abort();
    let _ = std::fs::remove_file(&viewer_sock);
    println!("[iter9] Success: Respawn verified!");
}
