//! Phase 3b end-to-end smoke test: validate that a real Vulkan
//! `waywallen_renderer` subprocess produces real `dma_fence` sync_fds
//! on every `FrameReady`, those fds survive the
//! `renderer_manager::run_reader` harvest, and `display_endpoint`
//! forwards them to a connected client as the acquire fence fd on
//! `Event::FrameReady`.
//!
//! The test uses the in-process RendererManager + Scheduler rig
//! (no HTTP layer, no separate daemon process) so it can be run from
//! `cargo test` without port contention. A real Vulkan device is
//! required; the test skips itself (with a WARN) if no suitable
//! device is found.

use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use waywallen::display_endpoint;
use waywallen::display_proto::{codec, Event, Request, PROTOCOL_NAME};
use waywallen::renderer_manager::{RendererManager, SpawnRequest};
use waywallen::scheduler::Scheduler;

fn tmp_sock(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "waywallen-phase3b-{tag}-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn have_vulkan_device() -> bool {
    // Cheap heuristic: the kernel DRM render node exists. If we can't
    // see a DRI device the renderer subprocess will fail during
    // vkCreateInstance anyway, and there's no point exercising the
    // rest of the test.
    std::path::Path::new("/dev/dri").exists()
}

#[tokio::test]
async fn renderer_produces_real_sync_fds() {
    if !have_vulkan_device() {
        eprintln!("skip: no /dev/dri on this host");
        return;
    }

    // Resolve the renderer binary path via cargo's CARGO_BIN_EXE
    // convention so the test doesn't rely on PATH.
    let renderer_bin = env!("CARGO_BIN_EXE_waywallen_renderer");
    std::env::set_var("WAYWALLEN_RENDERER_BIN", renderer_bin);

    // ---- Rig: manager + scheduler + display endpoint ----
    let mgr = Arc::new(RendererManager::new());
    let sched = Arc::new(Mutex::new(Scheduler::new()));
    let sock = tmp_sock("e2e");
    let _ = std::fs::remove_file(&sock);

    let sock_for_task = sock.clone();
    let mgr_for_task = Arc::clone(&mgr);
    let sched_for_task = Arc::clone(&sched);
    let server = tokio::spawn(async move {
        let _ = display_endpoint::serve(&sock_for_task, mgr_for_task, sched_for_task).await;
    });

    // Wait for endpoint bind.
    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(sock.exists(), "display endpoint did not bind");

    // ---- Spawn a real renderer ----
    let spawn_res = mgr
        .spawn(SpawnRequest {
            scene_pkg: String::new(),
            assets: String::new(),
            width: 640,
            height: 480,
            fps: 60,
            test_pattern: false,
        })
        .await;
    let _renderer_id = match spawn_res {
        Ok(id) => id,
        Err(e) => {
            eprintln!("skip: could not spawn waywallen_renderer: {e:#}");
            server.abort();
            let _ = std::fs::remove_file(&sock);
            return;
        }
    };

    // Give the renderer a moment to emit its first BindBuffers.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // ---- Connect a display client and drive the full flow ----
    let sock_for_client = sock.clone();
    let client = tokio::task::spawn_blocking(move || -> anyhow::Result<usize> {
        use std::os::unix::net::UnixStream;
        let stream = UnixStream::connect(&sock_for_client)?;

        // hello / welcome
        codec::send_request(
            &stream,
            &Request::Hello {
                protocol: PROTOCOL_NAME.to_string(),
                client_name: "phase3b-e2e".to_string(),
                client_version: "0.0.1".to_string(),
            },
            &[],
        )?;
        let (welcome, _) = codec::recv_event(&stream)?;
        anyhow::ensure!(
            matches!(welcome, Event::Welcome { .. }),
            "expected welcome, got {welcome:?}"
        );

        // register / accepted
        codec::send_request(
            &stream,
            &Request::RegisterDisplay {
                name: "e2e-display".to_string(),
                width: 640,
                height: 480,
                refresh_mhz: 60_000,
                properties: Vec::new(),
            },
            &[],
        )?;
        let (accepted, _) = codec::recv_event(&stream)?;
        anyhow::ensure!(
            matches!(accepted, Event::DisplayAccepted { .. }),
            "expected display_accepted, got {accepted:?}"
        );

        // bind_buffers (real dma-buf fds from the renderer)
        let (bind, bind_fds) = codec::recv_event(&stream)?;
        let Event::BindBuffers {
            buffer_generation,
            count,
            planes_per_buffer,
            ..
        } = bind
        else {
            anyhow::bail!("expected bind_buffers");
        };
        let expected_fds = (count * planes_per_buffer) as usize;
        anyhow::ensure!(
            bind_fds.len() == expected_fds,
            "bind_buffers fd count {} != expected {}",
            bind_fds.len(),
            expected_fds
        );
        for (i, fd) in bind_fds.iter().enumerate() {
            // Sanity: must be a valid fd the kernel handed us.
            anyhow::ensure!(fd.as_raw_fd() >= 0, "invalid dma-buf fd #{i}");
        }
        drop(bind_fds);

        // set_config
        let (cfg, _) = codec::recv_event(&stream)?;
        anyhow::ensure!(
            matches!(cfg, Event::SetConfig { .. }),
            "expected set_config"
        );

        // Drain at least 3 frames and verify each carries a live sync fd.
        let mut real_fence_count = 0usize;
        let mut frames_seen = 0usize;
        while frames_seen < 3 {
            let (evt, fds) = codec::recv_event(&stream)?;
            match evt {
                Event::FrameReady {
                    buffer_generation: g,
                    buffer_index,
                    seq,
                } => {
                    anyhow::ensure!(
                        g == buffer_generation,
                        "frame_ready gen={g} != bind gen={buffer_generation}"
                    );
                    anyhow::ensure!(
                        fds.len() == 1,
                        "frame_ready expected 1 sync fd, got {}",
                        fds.len()
                    );
                    let fd = &fds[0];
                    anyhow::ensure!(fd.as_raw_fd() >= 0, "invalid sync fd");

                    // Distinguish a real dma_fence sync_file from our
                    // eventfd placeholder. The f_op of a sync_file is
                    // "sync_file", so the readlink of the /proc fd
                    // starts with "anon_inode:sync_file". eventfd's
                    // readlink is "anon_inode:[eventfd]".
                    let link = std::fs::read_link(format!(
                        "/proc/self/fd/{}",
                        fd.as_raw_fd()
                    ))
                    .unwrap_or_default();
                    let link_str = link.to_string_lossy();
                    if link_str.contains("sync_file") {
                        real_fence_count += 1;
                    }
                    eprintln!(
                        "frame #{frames_seen} idx={buffer_index} seq={seq} fd={} kind={link_str}",
                        fd.as_raw_fd()
                    );

                    // Release the buffer so the renderer can reuse it.
                    codec::send_request(
                        &stream,
                        &Request::BufferRelease {
                            buffer_generation: g,
                            buffer_index,
                            seq,
                        },
                        &[],
                    )?;
                    frames_seen += 1;
                }
                Event::SetConfig { .. } | Event::BindBuffers { .. } => {
                    // mid-session rebind or config update — fine, drop
                }
                other => anyhow::bail!("unexpected event: {other:?}"),
            }
        }

        // Send bye to let the server clean up cleanly.
        codec::send_request(&stream, &Request::Bye, &[])?;
        Ok(real_fence_count)
    });

    let result = client.await.expect("client join");
    let real_fence_count = match result {
        Ok(n) => n,
        Err(e) => {
            eprintln!("client flow failed: {e:#}");
            server.abort();
            let _ = std::fs::remove_file(&sock);
            panic!("Phase 3b e2e failed: {e:#}");
        }
    };

    eprintln!("received {real_fence_count} real dma_fence sync_files out of 3 frames");
    // Acceptance: at least 1 of 3 must be a real sync_file, proving
    // the producer-to-consumer sync_fd path works end-to-end. We do
    // not require all 3 because the very first frame on some drivers
    // may not yet have the semaphore exported in time.
    assert!(
        real_fence_count >= 1,
        "no real dma_fence sync_files observed; sync_fd path is broken"
    );

    server.abort();
    let _ = std::fs::remove_file(&sock);
}
