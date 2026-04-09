//! Iteration 3 integration test: RendererManager spawn → control → kill.
//!
//! Skipped (not failed) when `WAYWALLEN_RENDERER_BIN` is unset, mirroring
//! the iter2 test's contract.

use kwallpaper_backend::ipc::proto::ControlMsg;
use kwallpaper_backend::renderer_manager::{RendererManager, SpawnRequest};
use std::time::Duration;

fn skip_if_no_bin() -> bool {
    if std::env::var_os("WAYWALLEN_RENDERER_BIN").is_none() {
        eprintln!(
            "skipping iter3_renderer_manager: set WAYWALLEN_RENDERER_BIN to the path \
             of the compiled waywallen-renderer binary to run this test"
        );
        return true;
    }
    false
}

#[tokio::test]
async fn spawn_control_kill_roundtrip() {
    if skip_if_no_bin() {
        return;
    }

    let mgr = RendererManager::new();

    // Spawn a renderer with bogus scene/assets — the host will start its
    // looper threads and emit Ready before noticing the scene is missing,
    // which is fine for this test (we only care about IPC liveness).
    let req = SpawnRequest {
        scene_pkg: String::new(),
        assets: String::new(),
        width: 320,
        height: 240,
        fps: 15,
        test_pattern: false,
    };
    let id = mgr.spawn(req).await.expect("spawn");
    assert!(!id.is_empty());

    // The renderer should be discoverable via list().
    let listed = mgr.list().await;
    assert!(listed.contains(&id), "list() should contain {id}: {listed:?}");

    // Push a few control messages. Each one is a fire-and-forget round
    // trip on the unix socket; success means the host's reader thread
    // accepted the JSON without disconnecting.
    mgr.send_control(&id, ControlMsg::Play)
        .await
        .expect("Play");
    mgr.send_control(&id, ControlMsg::Pause)
        .await
        .expect("Pause");
    mgr.send_control(&id, ControlMsg::Mouse { x: 0.5, y: 0.25 })
        .await
        .expect("Mouse");
    mgr.send_control(&id, ControlMsg::SetFps { fps: 24 })
        .await
        .expect("SetFps");

    // Tiny delay to let the host process the messages before we tear it
    // down — without this we sometimes race the kill ahead of the host's
    // reader thread observing the messages.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Kill cleans up. After kill the id should no longer list.
    mgr.kill(&id).await.expect("kill");
    let listed = mgr.list().await;
    assert!(!listed.contains(&id), "list() should not contain {id} after kill: {listed:?}");

    // send_control on a killed renderer must error.
    let err = mgr
        .send_control(&id, ControlMsg::Play)
        .await
        .expect_err("send to dead renderer should error");
    assert!(err.to_string().contains("unknown renderer"));
}
