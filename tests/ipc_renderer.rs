#[path = "common/mod.rs"]
mod common;

mod handshake_cpp {
    #[allow(unused_imports)]
    use super::common;
    // C++ host handshake: spawn the `waywallen-renderer` host binary
    // against a listening Unix-domain socket and verify the handshake.

    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixListener;
    use std::process::{Command, Stdio};
    use std::time::Duration;
    use waywallen::wallframe::ipc::proto::EventMsg;
    use waywallen::wallframe::ipc::uds::recv_event;

    #[test]
    fn hello_handshake() {
        let Some(bin) = common::cpp_renderer_bin_from_env() else {
            eprintln!(
                "skipping ipc_renderer_handshake_cpp: set WAYWALLEN_RENDERER_BIN to the path \
             of the compiled waywallen-renderer binary to run this test"
            );
            return;
        };
        assert!(
            bin.exists(),
            "WAYWALLEN_RENDERER_BIN points at nonexistent path: {}",
            bin.display()
        );

        let sock_path = common::tmp_sock("cpp-host-handshake");
        let _ = std::fs::remove_file(&sock_path);
        let listener = UnixListener::bind(&sock_path).expect("bind unix listener");
        let _cleanup = common::SockCleanup(sock_path.clone());

        let child = Command::new(&bin)
            .arg("--ipc")
            .arg(&sock_path)
            .arg("--width")
            .arg("1280")
            .arg("--height")
            .arg("720")
            .arg("--fps")
            .arg("30")
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to spawn {}: {}", bin.display(), e));
        let mut guard = common::ChildGuard(child);

        listener
            .set_nonblocking(false)
            .expect("set blocking on listener");
        let (stream, _addr) = match common::accept_with_timeout(&listener, Duration::from_secs(10))
        {
            Some(Ok(x)) => x,
            Some(Err(e)) => panic!("accept failed: {e}"),
            None => {
                let _ = guard.0.kill();
                panic!("timed out waiting for waywallen-renderer to connect back");
            }
        };

        let (msg, fds): (EventMsg, _) = recv_event(&stream).expect("recv first frame from host");
        assert!(fds.is_empty(), "ready must not carry fds");
        match msg {
            EventMsg::Ready { .. } => { /* ok */ }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    /// Extended smoke against the C++ host's `--test-pattern` mode.
    ///
    #[test]
    fn binding_and_frames_smoke() {
        let Some(bin) = common::cpp_renderer_bin_from_env() else {
            eprintln!("skipping: WAYWALLEN_RENDERER_BIN unset");
            return;
        };

        let sock_path = common::tmp_sock("cpp-host-test-pattern");
        let _ = std::fs::remove_file(&sock_path);
        let listener = UnixListener::bind(&sock_path).expect("bind");
        let _cleanup = common::SockCleanup(sock_path.clone());

        let child = Command::new(&bin)
            .arg("--ipc")
            .arg(&sock_path)
            .arg("--width")
            .arg("1280")
            .arg("--height")
            .arg("720")
            .arg("--fps")
            .arg("30")
            .arg("--test-pattern")
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn host");
        let mut guard = common::ChildGuard(child);

        let (stream, _) = match common::accept_with_timeout(&listener, Duration::from_secs(10)) {
            Some(Ok(x)) => x,
            _ => {
                let _ = guard.0.kill();
                panic!("accept timed out");
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(8)))
            .expect("set rd timeout");

        // Drain until Ready → BindBuffers → >=5 FrameReady, or timeout.
        let mut saw_ready = false;
        let mut bind: Option<(Vec<i32>, (u32, u32, u32, u32, u64, u64))> = None;
        let mut frames = 0usize;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            let (msg, fds): (EventMsg, _) = match recv_event(&stream) {
                Ok(x) => x,
                Err(e) => {
                    eprintln!("recv error (expected if hung): {e}");
                    break;
                }
            };
            match msg {
                EventMsg::Ready { .. } => saw_ready = true,
                EventMsg::BindBuffers { pool } => {
                    let count = pool.count;
                    let fourcc = pool.format.fourcc;
                    let width = pool.extent.width;
                    let height = pool.extent.height;
                    let modifier = pool.format.modifier;
                    let planes_per_buffer = pool.format.plane_count;
                    eprintln!(
                        "BindBuffers: count={} fourcc=0x{:08x} {}x{} planes={} mod=0x{:x} \
                     stride={:?} plane_offset={:?} size={:?} fds={}",
                        count,
                        fourcc,
                        width,
                        height,
                        planes_per_buffer,
                        modifier,
                        pool.stride,
                        pool.plane_offset,
                        pool.size,
                        fds.len()
                    );
                    assert_eq!(count, 3, "expected 3 slots");
                    let expected_fds = (count as usize) * (planes_per_buffer as usize);
                    assert_eq!(
                        fds.len(),
                        expected_fds,
                        "expected {expected_fds} FDs via SCM_RIGHTS (count*planes)"
                    );
                    assert!(fourcc != 0, "fourcc must be non-zero");
                    assert!(
                        u64::from(pool.stride[0]) >= u64::from(width) * 4,
                        "stride sanity"
                    );
                    bind = Some((
                        fds.iter().map(|f| f.as_raw_fd()).collect(),
                        (
                            count,
                            fourcc,
                            width,
                            height,
                            u64::from(pool.stride[0]),
                            modifier,
                        ),
                    ));
                    std::mem::forget(fds);
                }
                EventMsg::FrameReady { .. } => {
                    frames += 1;
                    if frames >= 5 && bind.is_some() {
                        break;
                    }
                }
                other => eprintln!("unexpected msg: {other:?}"),
            }
        }

        assert!(saw_ready, "never saw Ready event");
        let bind = bind.expect("never saw BindBuffers under --test-pattern mode");
        assert!(
            frames >= 5,
            "expected >=5 FrameReady, got {frames}; bind={bind:?}"
        );
    }
}

mod handshake_rust {
    #[allow(unused_imports)]
    use super::common;
    // Rust waywallen_renderer handshake: spawn the Rust `waywallen_renderer`
    // binary against a listening Unix-domain socket, expect

    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::time::Duration;
    use waywallen::wallframe::ipc::proto::{ControlMsg, EventMsg};
    use waywallen::wallframe::ipc::uds::{recv_event, send_control};

    const DRM_FORMAT_ABGR8888: u32 = 0x34324241;

    #[test]
    fn waywallen_renderer_bind_handshake() {
        let bin = PathBuf::from(env!("CARGO_BIN_EXE_waywallen_renderer"));
        assert!(bin.exists(), "renderer binary missing: {}", bin.display());

        let sock_path = common::tmp_sock("rust-renderer-handshake");
        let _ = std::fs::remove_file(&sock_path);

        let listener = UnixListener::bind(&sock_path).expect("bind uds listener");
        let _cleanup = common::SockCleanup(sock_path.clone());

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
        let mut guard = common::ChildGuard(child);

        let (stream, _) = match common::accept_with_timeout(&listener, Duration::from_secs(15)) {
            Some(Ok(x)) => x,
            Some(Err(e)) => panic!("accept: {e}"),
            None => {
                let _ = guard.0.kill();
                panic!("timed out waiting for renderer connect");
            }
        };

        // 1. Ready, no fds. Render node fields are best-effort (driver may
        //    or may not advertise VK_EXT_physical_device_drm); we just
        let (msg, fds) = recv_event(&stream).expect("recv Ready");
        assert!(fds.is_empty(), "Ready must not carry fds");
        assert!(
            matches!(msg, EventMsg::Ready { .. }),
            "expected Ready, got {msg:?}"
        );

        // 2. BindBuffers with 3 fds (LINEAR → planes_per_buffer = 1, so
        //    count * planes_per_buffer = 3 fds).
        let (msg, fds) = recv_event(&stream).expect("recv BindBuffers");
        match msg {
            EventMsg::BindBuffers { pool } => {
                let generation = pool.generation;
                let flags = pool.flags;
                let count = pool.count;
                let fourcc = pool.format.fourcc;
                let width = pool.extent.width;
                let height = pool.extent.height;
                let modifier = pool.format.modifier;
                let planes_per_buffer = pool.format.plane_count;
                assert_eq!(generation, 1, "first BindBuffers must report gen=1");
                assert_eq!(flags, 0, "initial pool must be DEVICE_LOCAL (flags=0)");
                assert_eq!(count, 3);
                assert_eq!(
                    fourcc, DRM_FORMAT_ABGR8888,
                    "renderer advertised wrong fourcc 0x{fourcc:08x}"
                );
                assert_eq!(width, 256);
                assert_eq!(height, 256);
                assert_eq!(modifier, 0, "expected DRM_FORMAT_MOD_LINEAR");
                assert_eq!(planes_per_buffer, 1, "LINEAR → single plane");
                let n = (count as usize) * (planes_per_buffer as usize);
                assert_eq!(fds.len(), n, "expected count*planes={n} DMA-BUF fds");
                assert_eq!(pool.stride.len(), n);
                assert_eq!(pool.plane_offset.len(), n);
                assert_eq!(pool.size.len(), n);
                for &s in &pool.stride {
                    assert!(s >= 256 * 4, "stride {s} below minimum");
                }
                for &o in &pool.plane_offset {
                    assert_eq!(o, 0);
                }
                for (i, &sz) in pool.size.iter().enumerate() {
                    assert_eq!(sz, u64::from(pool.stride[i]) * u64::from(height));
                }
            }
            other => panic!("expected BindBuffers, got {other:?}"),
        }

        // 3. Drain 6 FrameReady events (2 full cycles) and assert that the
        //    slot index cycles 0,1,2,0,1,2 — i.e. the renderer's frame loop
        let mut observed_slots = Vec::<u32>::new();
        let mut last_seq: i64 = -1;
        for _ in 0..6 {
            let (ev, fds) = recv_event(&stream).expect("recv FrameReady");
            assert_eq!(fds.len(), 1, "FrameReady must carry exactly one sync_fd");
            match ev {
                EventMsg::FrameReady { frame } => {
                    assert!(frame.produced_at_ns > 0, "produced_at_ns must be monotonic");
                    assert!(
                        (frame.sequence as i64) > last_seq,
                        "sequence must be monotonic"
                    );
                    last_seq = frame.sequence as i64;
                    observed_slots.push(frame.image_index);
                }
                other => panic!("expected FrameReady, got {other:?}"),
            }
        }
        assert_eq!(observed_slots, vec![0, 1, 2, 0, 1, 2]);

        // Pixel-level verification via mmap is deliberately skipped: AMD
        // RADV allocates the DMA-BUFs in DEVICE_LOCAL VRAM, which isn't

        // 4. Send Shutdown and poll-wait up to 3s for the child to exit.
        send_control(&stream, &ControlMsg::Shutdown, &[]).expect("send Shutdown");
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
}

mod lifecycle {
    #[allow(unused_imports)]
    use super::common;
    // RendererManager lifecycle: spawn → control → kill.
    //

    use std::sync::Arc;
    use std::time::Duration;
    use waywallen::wallframe::ipc::proto::{ControlMsg, ControlTransition, PointerMotion};
    use waywallen::wallframe::renderer_manager::{RendererManager, SpawnRequest};

    #[tokio::test]
    async fn spawn_control_kill_roundtrip() {
        if common::cpp_renderer_bin_from_env().is_none() {
            eprintln!(
                "skipping ipc_renderer_lifecycle: set WAYWALLEN_RENDERER_BIN to the path \
             of the compiled waywallen-renderer binary to run this test"
            );
            return;
        }

        let mgr = Arc::new(RendererManager::new_default());

        // Spawn with bogus scene/assets. The host emits Ready before it
        // notices the missing scene.
        let req = SpawnRequest {
            wp_type: "scene".into(),
            extras: std::collections::HashMap::new(),
            settings: std::collections::HashMap::new(),
            test_pattern: false,
            renderer_name: None,
            user_property_overrides: std::collections::HashMap::new(),
            default_user_properties: std::collections::HashMap::new(),
            display_size: None,
        };
        let id = mgr.spawn(req).await.expect("spawn");
        assert!(!id.is_empty());

        // The renderer should be discoverable via list().
        let listed = mgr.list().await;
        assert!(
            listed.contains(&id),
            "list() should contain {id}: {listed:?}"
        );

        // Push a few control messages. Each one is a fire-and-forget round
        // trip on the unix socket; success means the host's reader thread
        mgr.send_control(
            &id,
            ControlMsg::Play {
                transition: ControlTransition { fade_ms: 0 },
            },
        )
        .await
        .expect("Play");
        mgr.send_control(
            &id,
            ControlMsg::Pause {
                transition: ControlTransition { fade_ms: 0 },
            },
        )
        .await
        .expect("Pause");
        mgr.send_control(
            &id,
            ControlMsg::PointerMotion {
                event: PointerMotion {
                    x: 0.5,
                    y: 0.25,
                    timestamp_us: 0,
                    modifiers: 0,
                },
            },
        )
        .await
        .expect("PointerMotion");
        mgr.send_control(
            &id,
            ControlMsg::SettingChanged {
                settings: vec![("fps".into(), "24".into())],
            },
        )
        .await
        .expect("SettingChanged");

        // Tiny delay to let the host process the messages before we tear it
        // down — without this we sometimes race the kill ahead of the host's
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Kill cleans up. After kill the id should no longer list.
        mgr.kill(&id).await.expect("kill");
        let listed = mgr.list().await;
        assert!(
            !listed.contains(&id),
            "list() should not contain {id} after kill: {listed:?}"
        );

        // send_control on a killed renderer must error.
        let err = mgr
            .send_control(
                &id,
                ControlMsg::Play {
                    transition: ControlTransition { fade_ms: 0 },
                },
            )
            .await
            .expect_err("send to dead renderer should error");
        assert!(err.to_string().contains("unknown renderer"));
    }
}
