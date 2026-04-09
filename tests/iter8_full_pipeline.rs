#![cfg(feature = "legacy_proto_tests")]
// Legacy viewer-protocol integration test; see Cargo.toml feature docs.

//! Iteration 8: Full E2E smoke test.
//! Renderer (Subprocess) -> Daemon (RendererManager + DisplayEndpoint) -> Display (Subprocess).
//!
//! This test launches every part of the pipeline as a subprocess/task and
//! verifies that the Display observes FrameReady events from the Renderer.

use waywallen::renderer_manager::{RendererManager, SpawnRequest};
use waywallen::display_endpoint;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio::io::{BufReader, AsyncBufReadExt};

#[tokio::test]
async fn full_pipeline_smoke_test() {
    // Set up renderer binary path.
    unsafe {
        std::env::set_var(
            "WAYWALLEN_RENDERER_BIN",
            env!("CARGO_BIN_EXE_waywallen_renderer"),
        );
    }

    let mgr = Arc::new(RendererManager::new());
    let display_sock: PathBuf = std::env::temp_dir().join(format!(
        "waywallen-iter8-display-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    
    // 1. Start Display Endpoint (Daemon part).
    let mgr_clone = Arc::clone(&mgr);
    let display_sock_for_task = display_sock.clone();
    let endpoint = tokio::spawn(async move {
        let _ = display_endpoint::serve(&display_sock_for_task, mgr_clone).await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 2. Spawn Renderer via Manager (Daemon part).
    let renderer_id = mgr
        .spawn(SpawnRequest {
            scene_pkg: String::new(),
            assets: String::new(),
            width: 256,
            height: 256,
            fps: 10,
            test_pattern: false,
        })
        .await
        .expect("spawn waywallen_renderer");

    // 3. Spawn waywallen_display_demo binary as a subprocess.
    // We use --renderer-id to subscribe.
    // We'll capture its stdout/stderr to check for "observed X frames" logs.
    let display_bin = env!("CARGO_BIN_EXE_waywallen_display_demo");
    let mut display_child = Command::new(display_bin)
        .arg("--display-sock")
        .arg(&display_sock)
        .arg("--renderer-id")
        .arg(&renderer_id)
        .env("RUST_LOG", "info")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn waywallen_display_demo");

    let stderr = display_child.stderr.take().unwrap();
    let mut reader = BufReader::new(stderr).lines();

    // 4. Wait for the display to log that it observed frames.
    let mut observed = false;
    let timeout = tokio::time::sleep(Duration::from_secs(10));
    tokio::pin!(timeout);

    println!("[iter8] Waiting for display to report frames...");
    
    loop {
        tokio::select! {
            line = reader.next_line() => {
                match line {
                    Ok(Some(l)) => {
                        println!("[display stderr] {}", l);
                        if l.contains("display observed") {
                            observed = true;
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            _ = &mut timeout => {
                break;
            }
        }
    }

    // Cleanup.
    let _ = display_child.kill().await;
    let _ = mgr.kill(&renderer_id).await;
    endpoint.abort();
    let _ = std::fs::remove_file(&display_sock);

    assert!(observed, "Display failed to observe FrameReady events within 10s");
    println!("[iter8] Success: Full pipeline end-to-end verified!");
}
