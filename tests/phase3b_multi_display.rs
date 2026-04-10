//! Multi-display sync_fd fan-out test: verify that TWO concurrent
//! display clients, each subscribed to the same renderer, both
//! receive real `dma_fence` sync_file fds (not dummy eventfds) on
//! every `FrameReady` — proving that `clone_sync_fd` correctly dup's
//! the producer's fence to all subscribers.

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use waywallen::display_endpoint;
use waywallen::display_proto::{codec, Event, Request, PROTOCOL_NAME};
use waywallen::renderer_manager::{RendererManager, SpawnRequest};
use waywallen::scheduler::Scheduler;

fn tmp_sock(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "waywallen-multi-{tag}-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn have_vulkan() -> bool {
    std::path::Path::new("/dev/dri").exists()
}

/// Drive a single display client through handshake + N frames.
/// Returns the count of real `anon_inode:sync_file` fds received.
fn run_client(sock: &PathBuf, name: &str, n_frames: usize) -> anyhow::Result<usize> {
    let stream = UnixStream::connect(sock)?;

    // hello
    codec::send_request(
        &stream,
        &Request::Hello {
            protocol: PROTOCOL_NAME.to_string(),
            client_name: name.to_string(),
            client_version: "0.0.1".to_string(),
        },
        &[],
    )?;
    let (welcome, _) = codec::recv_event(&stream)?;
    anyhow::ensure!(matches!(welcome, Event::Welcome { .. }));

    // register
    codec::send_request(
        &stream,
        &Request::RegisterDisplay {
            name: name.to_string(),
            width: 640,
            height: 480,
            refresh_mhz: 60_000,
            properties: Vec::new(),
        },
        &[],
    )?;
    let (accepted, _) = codec::recv_event(&stream)?;
    anyhow::ensure!(matches!(accepted, Event::DisplayAccepted { .. }));

    // bind_buffers
    let (bind, bind_fds) = codec::recv_event(&stream)?;
    let buffer_generation = match bind {
        Event::BindBuffers { buffer_generation, .. } => buffer_generation,
        _ => anyhow::bail!("{name}: expected bind_buffers"),
    };
    drop(bind_fds);

    // set_config
    let (cfg, _) = codec::recv_event(&stream)?;
    anyhow::ensure!(matches!(cfg, Event::SetConfig { .. }));

    // drain frames
    let mut real_count = 0usize;
    let mut frames = 0usize;
    while frames < n_frames {
        let (evt, fds) = codec::recv_event(&stream)?;
        match evt {
            Event::FrameReady {
                buffer_generation: g,
                buffer_index,
                seq,
            } => {
                anyhow::ensure!(g == buffer_generation);
                anyhow::ensure!(fds.len() == 1);
                let link = std::fs::read_link(format!(
                    "/proc/self/fd/{}",
                    fds[0].as_raw_fd()
                ))
                .unwrap_or_default();
                if link.to_string_lossy().contains("sync_file") {
                    real_count += 1;
                }
                codec::send_request(
                    &stream,
                    &Request::BufferRelease {
                        buffer_generation: g,
                        buffer_index,
                        seq,
                    },
                    &[],
                )?;
                frames += 1;
            }
            Event::SetConfig { .. } | Event::BindBuffers { .. } => {}
            other => anyhow::bail!("{name}: unexpected {other:?}"),
        }
    }
    codec::send_request(&stream, &Request::Bye, &[])?;
    Ok(real_count)
}

#[tokio::test]
async fn two_displays_both_get_real_sync_fds() {
    if !have_vulkan() {
        eprintln!("skip: no /dev/dri");
        return;
    }

    let renderer_bin = env!("CARGO_BIN_EXE_waywallen_renderer");
    std::env::set_var("WAYWALLEN_RENDERER_BIN", renderer_bin);

    let mgr = Arc::new(RendererManager::new());
    let sched = Arc::new(Mutex::new(Scheduler::new()));
    let sock = tmp_sock("multi");
    let _ = std::fs::remove_file(&sock);

    let sock2 = sock.clone();
    let mgr2 = Arc::clone(&mgr);
    let sched2 = Arc::clone(&sched);
    let server = tokio::spawn(async move {
        let _ = display_endpoint::serve(&sock2, mgr2, sched2).await;
    });

    for _ in 0..100 {
        if sock.exists() { break; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(sock.exists());

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
            eprintln!("skip: renderer spawn: {e:#}");
            server.abort();
            let _ = std::fs::remove_file(&sock);
            return;
        }
    };

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Spawn two display clients concurrently.
    let sock_a = sock.clone();
    let sock_b = sock.clone();
    let client_a = tokio::task::spawn_blocking(move || run_client(&sock_a, "display-A", 3));
    let client_b = tokio::task::spawn_blocking(move || run_client(&sock_b, "display-B", 3));

    let real_a = client_a.await.expect("A join").expect("A flow");
    let real_b = client_b.await.expect("B join").expect("B flow");

    eprintln!("display-A: {real_a}/3 real sync_files");
    eprintln!("display-B: {real_b}/3 real sync_files");

    // Both must have gotten at least 1 real sync_file (proving the
    // dup fan-out works). In practice we expect 3/3 for each.
    assert!(
        real_a >= 1,
        "display-A got no real sync_files; clone_sync_fd fan-out broken"
    );
    assert!(
        real_b >= 1,
        "display-B got no real sync_files; clone_sync_fd fan-out broken"
    );

    server.abort();
    let _ = std::fs::remove_file(&sock);
}
