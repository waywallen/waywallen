mod common;

use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::Command;
use std::time::{Duration, Instant};
use waywallen::wallframe::dma::negotiate as dma;
use waywallen::wallframe::ipc::proto::*;
use waywallen::wallframe::ipc::uds::{recv_event, send_control};
use waywallen::wallframe::sync::{DrmDevice, SyncobjHandle};

nix::ioctl_write_ptr!(dma_buf_sync, b'b', 0, u64);

fn pixel_hash(pool: &BufferPool, fd: &OwnedFd) -> u64 {
    assert_eq!(pool.format.modifier, 0);
    assert_eq!(pool.format.plane_count, 1);
    let size = usize::try_from(pool.size[0]).unwrap();
    let offset = pool.plane_offset[0] as usize;
    let stride = pool.stride[0] as usize;
    let row_bytes = pool.extent.width as usize * 4;
    assert!(offset + stride * (pool.extent.height as usize - 1) + row_bytes <= size);
    unsafe {
        let ptr = libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        );
        assert_ne!(ptr, libc::MAP_FAILED);
        dma_buf_sync(fd.as_raw_fd(), &1).unwrap();
        let data = std::slice::from_raw_parts(ptr.cast::<u8>(), size);
        let mut hash = 0xcbf29ce484222325_u64;
        for row in 0..pool.extent.height as usize {
            for byte in &data[offset + row * stride..offset + row * stride + row_bytes] {
                hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
            }
        }
        dma_buf_sync(fd.as_raw_fd(), &5).unwrap();
        assert_eq!(libc::munmap(ptr, size), 0);
        hash
    }
}

#[test]
fn paused_renderer_resizes_without_restarting() {
    let Ok(binary) = std::env::var("WAYWALLEN_AUTO_RENDERER_BIN") else {
        eprintln!(
            "skip: set WAYWALLEN_AUTO_RENDERER_BIN and WAYWALLEN_AUTO_MEDIA_PATH for the GPU test"
        );
        return;
    };
    let media = std::env::var("WAYWALLEN_AUTO_MEDIA_PATH").expect("media path");
    let drm = DrmDevice::open_first_render_node().expect("DRM device");
    let socket = common::tmp_sock("auto-resolution");
    let _cleanup = common::SockCleanup(socket.clone());
    let listener = UnixListener::bind(&socket).unwrap();
    let mut command = Command::new(binary);
    command.args(["--ipc", socket.to_str().unwrap(), "--path", &media]);
    if let Ok(assets) = std::env::var("WAYWALLEN_AUTO_ASSETS_PATH") {
        command.args(["--assets", &assets]);
    }
    let mut child = common::ChildGuard(command.spawn().unwrap());
    let (stream, _) = common::accept_with_timeout(&listener, Duration::from_secs(10))
        .unwrap()
        .unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    send_control(
        &stream,
        &ControlMsg::Init {
            config: RendererInit {
                protocol_version: PROTOCOL_VERSION,
                spawn_version: waywallen::wallframe::renderer_manager::SPAWN_VERSION,
                settings: vec![
                    ("resolution".into(), "-2".into()),
                    ("enable_audio".into(), "false".into()),
                    (
                        "hwdec".into(),
                        std::env::var("WAYWALLEN_AUTO_HWDEC").unwrap_or("none".into()),
                    ),
                ],
                user_properties: String::new(),
            },
        },
        &[],
    )
    .unwrap();
    let mut timeline = None;
    let caps = loop {
        let (event, mut fds) = recv_event(&stream).unwrap();
        match event {
            EventMsg::ReleaseSyncobj => timeline = Some(drm.fd_to_handle(&fds.remove(0)).unwrap()),
            EventMsg::RenderTargetCapabilities { capabilities } => break capabilities,
            EventMsg::BindBuffers { .. } => panic!("allocated before receiving the first target"),
            EventMsg::Error { message } => panic!("{message}"),
            _ => {}
        }
    };
    send_control(
        &stream,
        &ControlMsg::Pause {
            transition: ControlTransition { fade_ms: 0 },
        },
        &[],
    )
    .unwrap();
    let mut old_buffers = Vec::new();
    let mut pools = Vec::new();
    let mut hashes = Vec::new();
    let mut fd_counts = Vec::new();
    let mut generation = 0;
    let mut sequence = 0;
    for (index, divisor) in [2, 1, 4, 2].into_iter().enumerate() {
        let scale = if caps.mode == 2
            && index == 1
            && caps.reference_width <= caps.max_width / 2
            && caps.reference_height <= caps.max_height / 2
        {
            2
        } else {
            1
        };
        let width = (caps.reference_width * scale / divisor / caps.width_alignment
            * caps.width_alignment)
            .max(caps.min_width);
        let height = (caps.reference_height * scale / divisor / caps.height_alignment
            * caps.height_alignment)
            .max(caps.min_height);
        let config = RenderTargetConfig {
            revision: index as u64 + 1,
            capabilities_revision: caps.revision,
            width,
            height,
            fourcc: dma::DRM_FORMAT_ABGR8888,
            modifier: 0,
            plane_count: 1,
            sync_mode: dma::SYNC_SYNCOBJ_TIMELINE,
            color: dma::DEFAULT_COLOR,
            mem_hint: dma::MEM_HINT_HOST_VISIBLE,
            count: 1,
            path: BufferPath::CompatLinear,
            memory_source: BufferMemorySource::GpuLinear,
        };
        if index == 1 {
            let mut invalid = config.clone();
            invalid.revision = 99;
            invalid.width = caps.reference_width + caps.width_alignment;
            send_control(
                &stream,
                &ControlMsg::ConfigureRenderTarget { config: invalid },
                &[],
            )
            .unwrap();
            loop {
                match recv_event(&stream).unwrap().0 {
                    EventMsg::RenderTargetResult { result } => {
                        assert_eq!(result.revision, 99);
                        assert_eq!(result.status, 1);
                        break;
                    }
                    EventMsg::BindBuffers { .. } | EventMsg::FrameReady { .. } => {
                        panic!("invalid target changed the active pool")
                    }
                    _ => {}
                }
            }
        }
        send_control(
            &stream,
            &ControlMsg::ConfigureRenderTarget {
                config: config.clone(),
            },
            &[],
        )
        .unwrap();
        let mut frame_seen = false;
        let mut result_seen = false;
        loop {
            let (event, fds) = recv_event(&stream).unwrap();
            match event {
                EventMsg::BindBuffers { pool } => {
                    assert!(pool.generation > generation);
                    generation = pool.generation;
                    assert_eq!((pool.extent.width, pool.extent.height), (width, height));
                    pools.push(pool);
                    old_buffers.extend(fds);
                }
                EventMsg::FrameReady { frame } => {
                    assert_eq!(pools.len(), index + 1, "frame arrived before the new pool");
                    assert!(!frame_seen, "paused renderer published multiple frames");
                    frame_seen = true;
                    assert!(frame.sequence > sequence);
                    sequence = frame.sequence;
                    let acquire = drm.create_binary_syncobj().unwrap();
                    drm.import_sync_file(&acquire, &fds[0]).unwrap();
                    let mut fence = libc::pollfd {
                        fd: fds[0].as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    assert_eq!(unsafe { libc::poll(&mut fence, 1, 10_000) }, 1);
                    hashes.push(pixel_hash(
                        pools.last().unwrap(),
                        old_buffers.last().unwrap(),
                    ));
                    drm.transfer(&acquire, 0, timeline.as_ref().unwrap(), frame.release_point)
                        .unwrap();
                }
                EventMsg::RenderTargetResult { result } => {
                    assert_eq!(result.revision, config.revision);
                    assert_eq!(result.status, 0, "{}", result.message);
                    assert_eq!(result.pool_generation, generation);
                    result_seen = true;
                }
                EventMsg::Error { message } => panic!("{message}"),
                _ => {}
            }
            if frame_seen && result_seen {
                break;
            }
        }
        assert!(child.0.try_wait().unwrap().is_none());
        fd_counts.push(
            std::fs::read_dir(format!("/proc/{}/fd", child.0.id()))
                .unwrap()
                .count(),
        );
        eprintln!(
            "Auto revision={} extent={}x{} generation={} sequence={sequence}",
            config.revision, width, height, generation
        );
    }
    assert_eq!(
        old_buffers.len(),
        4,
        "old exports remain alive across resize"
    );
    assert_eq!(
        hashes[0], hashes[3],
        "resize must not advance paused content"
    );
    for ((pool, fd), expected) in pools.iter().zip(&old_buffers).zip(&hashes) {
        assert_eq!(
            pixel_hash(pool, fd),
            *expected,
            "retired backing was overwritten"
        );
    }
    assert!(
        fd_counts[3] <= fd_counts[0] + 2,
        "renderer leaked FDs: {fd_counts:?}"
    );
    send_control(&stream, &ControlMsg::Shutdown, &[]).unwrap();
}

fn readable(fd: i32, duration: Duration) -> bool {
    let mut descriptor = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut descriptor, 1, duration.as_millis() as i32) };
    assert!(result >= 0, "poll: {}", std::io::Error::last_os_error());
    if result == 0 {
        return false;
    }
    assert_ne!(
        descriptor.revents & libc::POLLIN,
        0,
        "unexpected socket hangup"
    );
    true
}

struct NativeSession {
    child: common::ChildGuard,
    stream: UnixStream,
    timeline: SyncobjHandle,
    drm: DrmDevice,
    caps: RenderTargetCapabilities,
    _cleanup: common::SockCleanup,
}

impl NativeSession {
    fn start(user_properties: String) -> Option<Self> {
        if std::env::var("WAYWALLEN_AUTO_RUN_LOOP").as_deref() != Ok("1") {
            eprintln!("skip: set WAYWALLEN_AUTO_RUN_LOOP=1 for the native run-loop GPU tests");
            return None;
        }
        let binary = std::env::var("WAYWALLEN_AUTO_RENDERER_BIN").expect("native renderer binary");
        let media = std::env::var("WAYWALLEN_AUTO_MEDIA_PATH").expect("native media path");
        let drm = DrmDevice::open_first_render_node().expect("DRM render node");
        let socket = common::tmp_sock("renderer-run-loop");
        let cleanup = common::SockCleanup(socket.clone());
        let listener = UnixListener::bind(&socket).unwrap();
        let child = common::ChildGuard(
            Command::new(binary)
                .args(["--ipc", socket.to_str().unwrap(), "--path", &media])
                .spawn()
                .unwrap(),
        );
        let (stream, _) = common::accept_with_timeout(&listener, Duration::from_secs(10))
            .expect("renderer connection timed out")
            .unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        send_control(
            &stream,
            &ControlMsg::Init {
                config: RendererInit {
                    protocol_version: PROTOCOL_VERSION,
                    spawn_version: waywallen::wallframe::renderer_manager::SPAWN_VERSION,
                    settings: vec![
                        ("resolution".into(), "-2".into()),
                        ("enable_audio".into(), "false".into()),
                        ("hwdec".into(), "none".into()),
                        ("render_node".into(), drm.path().to_str().unwrap().into()),
                    ],
                    user_properties,
                },
            },
            &[],
        )
        .unwrap();
        let mut timeline = None;
        let mut caps = None;
        while timeline.is_none() || caps.is_none() {
            let (event, mut fds) = recv_event(&stream).unwrap();
            match event {
                EventMsg::ReleaseSyncobj => {
                    assert_eq!(fds.len(), 1);
                    timeline = Some(drm.fd_to_handle(&fds.remove(0)).unwrap());
                }
                EventMsg::RenderTargetCapabilities { capabilities } => caps = Some(capabilities),
                EventMsg::BindBuffers { .. } | EventMsg::FrameReady { .. } => {
                    panic!("renderer allocated before its target")
                }
                EventMsg::Error { message } => panic!("{message}"),
                _ => {}
            }
        }
        Some(Self {
            child,
            stream,
            timeline: timeline.unwrap(),
            drm,
            caps: caps.unwrap(),
            _cleanup: cleanup,
        })
    }

    fn send(&self, control: ControlMsg) {
        send_control(&self.stream, &control, &[]).unwrap();
    }

    fn receive(&self) -> (EventMsg, Vec<OwnedFd>) {
        assert!(
            readable(self.stream.as_raw_fd(), Duration::from_secs(10)),
            "renderer event timed out"
        );
        let (event, fds) = recv_event(&self.stream).unwrap();
        match &event {
            EventMsg::Error { message } => panic!("renderer error: {message}"),
            EventMsg::InitNack { rejection } => panic!("renderer rejected Init: {rejection:?}"),
            EventMsg::BindFailed { failure } => panic!("renderer rejected buffers: {failure:?}"),
            _ => {}
        }
        (event, fds)
    }

    fn configure_paused(&self) -> (BufferPool, OwnedFd, Frame, OwnedFd) {
        self.send(ControlMsg::Pause {
            transition: ControlTransition { fade_ms: 0 },
        });
        self.send(ControlMsg::ConfigureRenderTarget {
            config: RenderTargetConfig {
                revision: 1,
                capabilities_revision: self.caps.revision,
                width: self.caps.reference_width,
                height: self.caps.reference_height,
                fourcc: dma::DRM_FORMAT_ABGR8888,
                modifier: 0,
                plane_count: 1,
                sync_mode: dma::SYNC_SYNCOBJ_TIMELINE,
                color: dma::DEFAULT_COLOR,
                mem_hint: dma::MEM_HINT_HOST_VISIBLE,
                count: 1,
                path: BufferPath::CompatLinear,
                memory_source: BufferMemorySource::GpuLinear,
            },
        });
        let mut bound = None;
        let mut rendered = None;
        let mut applied = false;
        while bound.is_none() || rendered.is_none() || !applied {
            let (event, mut fds) = self.receive();
            match event {
                EventMsg::BindBuffers { pool } => {
                    assert_eq!(pool.count, 1);
                    assert_eq!(fds.len(), 1);
                    bound = Some((pool, fds.remove(0)));
                }
                EventMsg::FrameReady { frame } => {
                    assert!(bound.is_some());
                    assert_eq!(fds.len(), 1);
                    assert!(rendered.is_none(), "paused renderer emitted extra frames");
                    rendered = Some((frame, fds.remove(0)));
                }
                EventMsg::RenderTargetResult { result } => {
                    assert_eq!(result.status, 0, "{}", result.message);
                    applied = true;
                }
                EventMsg::ReportState { .. } => {}
                _ => panic!("unexpected configuration event: {event:?}"),
            }
        }
        let (pool, buffer) = bound.unwrap();
        let (frame, fence) = rendered.unwrap();
        (pool, buffer, frame, fence)
    }

    fn frame(&self) -> (Frame, OwnedFd) {
        loop {
            let (event, mut fds) = self.receive();
            match event {
                EventMsg::FrameReady { frame } => {
                    assert_eq!(fds.len(), 1);
                    return (frame, fds.remove(0));
                }
                EventMsg::ReportState { .. } => {}
                _ => panic!("expected frame: {event:?}"),
            }
        }
    }

    fn release(&self, frame: &Frame, fence: &OwnedFd) {
        let acquire = self.drm.create_binary_syncobj().unwrap();
        self.drm.import_sync_file(&acquire, fence).unwrap();
        assert!(readable(fence.as_raw_fd(), Duration::from_secs(10)));
        self.drm
            .transfer(&acquire, 0, &self.timeline, frame.release_point)
            .unwrap();
    }

    fn no_frames(&self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            if !readable(self.stream.as_raw_fd(), remaining) {
                return;
            }
            let (event, _) = self.receive();
            assert!(matches!(event, EventMsg::ReportState { .. }), "{event:?}");
        }
    }

    fn setting_color(&self) {
        self.send(ControlMsg::SetLogLevel {
            level: LogLevel::Info,
        });
        self.send(ControlMsg::SettingChanged {
            settings: vec![("waywallen.scheme_color".into(), "0.25 0.5 0.75".into())],
        });
    }

    fn color_report(&self) {
        loop {
            let (event, _) = self.receive();
            let EventMsg::ReportState { state } = event else {
                panic!("expected a state report while waiting: {event:?}");
            };
            if state.fields & RENDERER_STATE_FIELD_CLEAR_COLOR != 0
                && state.clear_color.r == 0.25
                && state.clear_color.g == 0.5
                && state.clear_color.b == 0.75
            {
                return;
            }
        }
    }

    fn shutdown(&mut self, timeout: Duration) {
        let started = Instant::now();
        self.send(ControlMsg::Shutdown);
        loop {
            if let Some(status) = self.child.0.try_wait().unwrap() {
                assert!(status.success(), "renderer exit: {status}");
                return;
            }
            assert!(
                started.elapsed() < timeout,
                "renderer ignored Shutdown while waiting"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

#[test]
fn image_package_replaces_single_buffer_while_old_content_is_held() {
    if std::env::var("WAYWALLEN_IMAGE_PACKAGE_TEST").as_deref() != Ok("1") {
        eprintln!("skip: set WAYWALLEN_IMAGE_PACKAGE_TEST=1 with a two-variant image package");
        return;
    }
    let Some(mut session) = NativeSession::start(r#"{"waywallen.image_variant":"light"}"#.into())
    else {
        return;
    };
    let (pool, buffer, _first, first_fence) = session.configure_paused();
    assert!(readable(first_fence.as_raw_fd(), Duration::from_secs(10)));
    let light_hash = pixel_hash(&pool, &buffer);
    session.send(ControlMsg::RequestFrame);
    session.send(ControlMsg::SettingChanged {
        settings: vec![("waywallen.image_variant".into(), "dark".into())],
    });
    let (replacement, replacement_buffer) = loop {
        let (event, mut fds) = session.receive();
        match event {
            EventMsg::BindBuffers { pool: replacement } => {
                assert_eq!(replacement.count, 1);
                assert_eq!(fds.len(), 1);
                break (replacement, fds.remove(0));
            }
            EventMsg::ReportState { .. } => {}
            _ => panic!("expected replacement bind: {event:?}"),
        }
    };
    let (dark, dark_fence) = session.frame();
    assert!(readable(dark_fence.as_raw_fd(), Duration::from_secs(10)));
    assert_ne!(pixel_hash(&replacement, &replacement_buffer), light_hash);
    assert_eq!(
        pixel_hash(&pool, &buffer),
        light_hash,
        "old backing storage was overwritten"
    );
    session.release(&dark, &dark_fence);
    session.no_frames(Duration::from_millis(100));
    session.send(ControlMsg::RequestFrame);
    let (repeated, repeated_fence) = session.frame();
    session.release(&repeated, &repeated_fence);
    session.shutdown(Duration::from_secs(1));
}

#[test]
fn blocked_republish_handles_controls_without_losing_the_request() {
    let Some(mut session) = NativeSession::start(String::new()) else {
        return;
    };
    let (pool, buffer, first, first_fence) = session.configure_paused();
    assert!(readable(first_fence.as_raw_fd(), Duration::from_secs(10)));
    let expected_hash = pixel_hash(&pool, &buffer);
    session.send(ControlMsg::RequestFrame);
    session.no_frames(Duration::from_millis(30));
    session.setting_color();
    session.send(ControlMsg::RequestFrame);
    session.color_report();
    session.no_frames(Duration::from_millis(30));
    session.release(&first, &first_fence);
    let (republished, republished_fence) = session.frame();
    assert!(republished.sequence > first.sequence);
    assert!(republished.release_point > first.release_point);
    assert!(readable(
        republished_fence.as_raw_fd(),
        Duration::from_secs(10)
    ));
    assert_eq!(pixel_hash(&pool, &buffer), expected_hash);

    session.send(ControlMsg::RequestFrame);
    session.setting_color();
    session.color_report();
    session.no_frames(Duration::from_millis(30));
    session.release(&republished, &republished_fence);
    let (latest, latest_fence) = session.frame();
    assert!(latest.sequence > republished.sequence);
    assert!(latest.release_point > republished.release_point);
    assert!(readable(latest_fence.as_raw_fd(), Duration::from_secs(10)));
    assert_eq!(pixel_hash(&pool, &buffer), expected_hash);
    session.release(&latest, &latest_fence);
    session.shutdown(Duration::from_secs(2));
}

#[test]
fn video_pacing_controls_preserve_deadline_and_shutdown_interrupts_wait() {
    let Ok(fps) = std::env::var("WAYWALLEN_AUTO_VIDEO_FPS") else {
        eprintln!("skip: set WAYWALLEN_AUTO_VIDEO_FPS to the test video's frame rate");
        return;
    };
    let fps: f64 = fps.parse().unwrap();
    let period = Duration::from_secs_f64(1.0 / fps / 0.25);
    assert!(period >= Duration::from_millis(500) && period <= Duration::from_secs(1));
    let Some(mut session) = NativeSession::start(r#"{"waywallen.playback_speed":25}"#.into())
    else {
        return;
    };
    let (_, _buffer, first, first_fence) = session.configure_paused();
    session.release(&first, &first_fence);
    session.send(ControlMsg::Play {
        transition: ControlTransition { fade_ms: 0 },
    });
    let (primed, primed_fence) = session.frame();
    session.release(&primed, &primed_fence);
    let (before, before_fence) = session.frame();
    session.release(&before, &before_fence);
    session.setting_color();
    session.color_report();
    let (after, after_fence) = session.frame();
    let produced_delta = Duration::from_nanos(
        after
            .produced_at_ns
            .checked_sub(before.produced_at_ns)
            .expect("production clock regressed"),
    );
    assert!(
        produced_delta >= period.mul_f64(0.75),
        "unrelated control published before its deadline: {produced_delta:?} < {period:?}"
    );
    session.release(&after, &after_fence);
    session.setting_color();
    session.color_report();
    session.shutdown(period.mul_f64(0.75));
}
