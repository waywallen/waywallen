mod common;

use std::collections::HashSet;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::Command;
use std::time::{Duration, Instant};
use waywallen::wallframe::dma::negotiate as dma;
use waywallen::wallframe::ipc::proto::*;
use waywallen::wallframe::ipc::uds::{recv_event, send_control};
use waywallen::wallframe::renderer_manager::render_target::{Capabilities, Size};
use waywallen::wallframe::renderer_manager::DrmNode;
use waywallen::wallframe::sync::{DrmDevice, SyncobjHandle};

nix::ioctl_write_ptr!(dma_buf_sync, b'b', 0, u64);

fn pixel_hash(pool: &BufferPool, buffers: &[OwnedFd], frame: &Frame, fence: &OwnedFd) -> u64 {
    assert!(
        readable(fence.as_raw_fd(), Duration::from_secs(10)),
        "renderer GPU work did not complete"
    );
    assert_eq!(pool.format.modifier, dma::DRM_FORMAT_MOD_LINEAR);
    assert_eq!(pool.format.plane_count, 1);
    let index = frame.image_index as usize;
    let fd = &buffers[index];
    let size = usize::try_from(pool.size[index]).unwrap();
    let offset = pool.plane_offset[index] as usize;
    let stride = pool.stride[index] as usize;
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

fn motion(stream: &UnixStream, x: f32, y: f32) {
    send(
        stream,
        ControlMsg::PointerMotion {
            event: PointerMotion {
                x,
                y,
                timestamp_us: 0,
                modifiers: 0,
            },
        },
    );
}

fn button(stream: &UnixStream, x: f32, y: f32, button: u32, state: PointerButtonState) {
    send(
        stream,
        ControlMsg::PointerButton {
            event: PointerButton {
                x,
                y,
                button,
                state,
                timestamp_us: 0,
                modifiers: 0,
            },
        },
    );
}

fn readable(fd: i32, timeout: Duration) -> bool {
    let mut poll = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut poll, 1, timeout.as_millis().try_into().unwrap()) };
    assert!(result >= 0, "poll: {}", std::io::Error::last_os_error());
    if result == 0 {
        return false;
    }
    assert_eq!(poll.revents & (libc::POLLERR | libc::POLLNVAL), 0);
    assert_ne!(poll.revents & libc::POLLIN, 0, "unexpected hangup");
    true
}

fn receive(stream: &UnixStream) -> (EventMsg, Vec<OwnedFd>) {
    assert!(
        readable(stream.as_raw_fd(), Duration::from_secs(15)),
        "renderer event timed out"
    );
    let (event, fds) = recv_event(stream).expect("receive renderer event");
    match &event {
        EventMsg::Error { message } => panic!("renderer error: {message}"),
        EventMsg::InitNack { rejection } => panic!("renderer rejected Init: {rejection:?}"),
        EventMsg::BindFailed { failure } => panic!("renderer rejected buffers: {failure:?}"),
        _ => {}
    }
    (event, fds)
}

fn send(stream: &UnixStream, control: ControlMsg) {
    send_control(stream, &control, &[]).expect("send renderer control");
}

fn assert_no_frames(stream: &UnixStream, duration: Duration) {
    let deadline = Instant::now() + duration;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        if !readable(stream.as_raw_fd(), remaining) {
            return;
        }
        let (event, _) = receive(stream);
        assert!(
            matches!(event, EventMsg::ReportState { .. }),
            "unexpected event while output was blocked or paused: {event:?}"
        );
    }
}

fn release(drm: &DrmDevice, timeline: &SyncobjHandle, frame: &Frame, fence: &OwnedFd) {
    let acquire = drm.create_binary_syncobj().expect("create acquire syncobj");
    drm.import_sync_file(&acquire, fence)
        .expect("import renderer SYNC_FD");
    assert!(
        readable(fence.as_raw_fd(), Duration::from_secs(10)),
        "renderer GPU work did not complete"
    );
    drm.transfer(&acquire, 0, timeline, frame.release_point)
        .expect("release renderer timeline");
}

fn receive_frame(stream: &UnixStream, sequence: &mut u64) -> (Frame, OwnedFd) {
    loop {
        let (event, mut fds) = receive(stream);
        match event {
            EventMsg::FrameReady { frame } => {
                assert!(frame.sequence > *sequence);
                assert!(frame.release_point > 0);
                assert_eq!(fds.len(), 1);
                *sequence = frame.sequence;
                return (frame, fds.remove(0));
            }
            EventMsg::ReportState { .. } => {}
            _ => panic!("expected a rendered frame: {event:?}"),
        }
    }
}

fn configure(
    stream: &UnixStream,
    config: RenderTargetConfig,
    sequence: &mut u64,
) -> (BufferPool, Vec<OwnedFd>, Frame, OwnedFd) {
    send(
        stream,
        ControlMsg::ConfigureRenderTarget {
            config: config.clone(),
        },
    );
    let mut bound = None;
    let mut rendered = None;
    let mut applied = None;
    while bound.is_none() || rendered.is_none() || applied.is_none() {
        let (event, mut fds) = receive(stream);
        match event {
            EventMsg::BindBuffers { pool } => {
                assert!(bound.is_none(), "configured the same target twice");
                assert_eq!(pool.count, config.count);
                assert_eq!(pool.extent.width, config.width);
                assert_eq!(pool.extent.height, config.height);
                assert_eq!(pool.format.fourcc, config.fourcc);
                assert_eq!(pool.format.modifier, config.modifier);
                assert_eq!(pool.format.plane_count, config.plane_count);
                assert_eq!(fds.len(), (pool.count * pool.format.plane_count) as usize);
                bound = Some((pool, fds));
            }
            EventMsg::FrameReady { frame } => {
                assert!(bound.is_some(), "published before BindBuffers");
                assert!(
                    rendered.is_none(),
                    "paused target published multiple frames"
                );
                assert!(frame.sequence > *sequence);
                assert!(frame.image_index < config.count);
                assert_eq!(fds.len(), 1);
                *sequence = frame.sequence;
                rendered = Some((frame, fds.remove(0)));
            }
            EventMsg::RenderTargetResult { result } => {
                assert_eq!(result.revision, config.revision);
                assert_eq!(result.status, 0, "{}", result.message);
                assert_eq!((result.width, result.height), (config.width, config.height));
                assert!(applied.is_none());
                applied = Some(result);
            }
            EventMsg::ReportState { .. } => {}
            _ => panic!("unexpected render target event: {event:?}"),
        }
    }
    let (pool, buffers) = bound.unwrap();
    assert_eq!(applied.unwrap().pool_generation, pool.generation);
    let (frame, fence) = rendered.unwrap();
    (pool, buffers, frame, fence)
}

fn shutdown(stream: &UnixStream, child: &mut common::ChildGuard) {
    send(stream, ControlMsg::Shutdown);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success(), "renderer shutdown failed: {status}");
            break;
        }
        assert!(Instant::now() < deadline, "renderer ignored Shutdown");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn camera_transition(
    stream: &UnixStream,
    drm: &DrmDevice,
    timeline: &SyncobjHandle,
    pool: &BufferPool,
    buffers: &[OwnedFd],
    initial: (Frame, OwnedFd),
    sequence: &mut u64,
) {
    send(
        stream,
        ControlMsg::Play {
            transition: ControlTransition { fade_ms: 0 },
        },
    );
    let mut held = vec![initial];
    for _ in 0..2 {
        held.push(receive_frame(stream, sequence));
    }
    let (frame, fence) = held.last().unwrap();
    let blocked_hash = pixel_hash(pool, buffers, frame, fence);
    assert_no_frames(stream, Duration::from_secs(3));
    send(
        stream,
        ControlMsg::Pause {
            transition: ControlTransition { fade_ms: 0 },
        },
    );
    send(stream, ControlMsg::RequestFrame);
    assert_no_frames(stream, Duration::from_millis(250));
    for (frame, fence) in &held {
        release(drm, timeline, frame, fence);
    }
    let (paused, paused_fence) = receive_frame(stream, sequence);
    assert_eq!(
        blocked_hash,
        pixel_hash(pool, buffers, &paused, &paused_fence),
        "Busy advanced the camera transition"
    );
    release(drm, timeline, &paused, &paused_fence);
    assert_no_frames(stream, Duration::from_millis(250));
    send(stream, ControlMsg::RequestFrame);
    let (requested, requested_fence) = receive_frame(stream, sequence);
    assert_eq!(
        blocked_hash,
        pixel_hash(pool, buffers, &requested, &requested_fence),
        "paused RequestFrame advanced the camera transition"
    );
    release(drm, timeline, &requested, &requested_fence);
    assert_no_frames(stream, Duration::from_millis(250));

    send(
        stream,
        ControlMsg::Play {
            transition: ControlTransition { fade_ms: 0 },
        },
    );
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut frames = 0;
    let mut final_hash = blocked_hash;
    while readable(stream.as_raw_fd(), Duration::from_millis(300)) {
        assert!(
            Instant::now() < deadline,
            "camera transition did not finish"
        );
        let (event, mut fds) = receive(stream);
        match event {
            EventMsg::FrameReady { frame } => {
                assert!(frame.sequence > *sequence);
                assert_eq!(fds.len(), 1);
                *sequence = frame.sequence;
                let fence = fds.remove(0);
                final_hash = pixel_hash(pool, buffers, &frame, &fence);
                release(drm, timeline, &frame, &fence);
                frames += 1;
            }
            EventMsg::ReportState { .. } => {}
            _ => panic!("unexpected camera transition event: {event:?}"),
        }
    }
    assert!(
        frames >= 10,
        "Busy consumed the remaining camera transition"
    );
    assert_ne!(
        final_hash, blocked_hash,
        "static camera transition did not move"
    );
    assert_no_frames(stream, Duration::from_millis(250));
    send(stream, ControlMsg::RequestFrame);
    let (finished, finished_fence) = receive_frame(stream, sequence);
    assert_eq!(
        final_hash,
        pixel_hash(pool, buffers, &finished, &finished_fence),
        "completed transition did not retain the preset camera"
    );
    release(drm, timeline, &finished, &finished_fence);
    assert_no_frames(stream, Duration::from_millis(250));
}

#[test]
fn gltf_renderer_handles_busy_pause_settings_resize_and_shutdown() {
    let Ok(binary) = std::env::var("WAYWALLEN_GLTF_RENDERER_BIN") else {
        eprintln!(
            "skip: set WAYWALLEN_GLTF_RENDERER_BIN and WAYWALLEN_GLTF_MODEL_PATH for the GPU test"
        );
        return;
    };
    let model = std::env::var("WAYWALLEN_GLTF_MODEL_PATH").expect("glTF model path");
    let preset = std::env::var("WAYWALLEN_GLTF_CAMERA_PRESET")
        .ok()
        .filter(|value| !value.is_empty());
    let transition_only = std::env::var("WAYWALLEN_GLTF_TRANSITION_ONLY").as_deref() == Ok("1");
    assert!(
        !transition_only || preset.is_some(),
        "transition test requires a preset"
    );
    if !transition_only {
        run_renderer(&binary, &model, preset.as_deref(), false);
    }
    if preset.is_some() {
        run_renderer(&binary, &model, preset.as_deref(), true);
    }
}

fn run_renderer(binary: &str, model: &str, preset: Option<&str>, transition_case: bool) {
    let drm = DrmDevice::open_first_render_node().expect("DRM render node");
    let socket = common::tmp_sock("gltf-renderer");
    let _cleanup = common::SockCleanup(socket.clone());
    let listener = UnixListener::bind(&socket).expect("listen for renderer");
    let mut command = Command::new(binary);
    command.args(["--ipc", socket.to_str().unwrap(), "--path", model]);
    if let Some(preset) = preset {
        command.args(["--camera-preset", preset]);
    }
    if std::env::var("WAYWALLEN_GLTF_VALIDATION").as_deref() == Ok("1") {
        command.arg("--validation");
    }
    let mut child = common::ChildGuard(command.spawn().expect("spawn glTF renderer"));
    let (stream, _) = common::accept_with_timeout(&listener, Duration::from_secs(10))
        .expect("renderer connection timed out")
        .expect("accept renderer");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    send(
        &stream,
        ControlMsg::Init {
            config: RendererInit {
                protocol_version: PROTOCOL_VERSION,
                spawn_version: waywallen::wallframe::renderer_manager::SPAWN_VERSION,
                settings: vec![
                    ("resolution".into(), "-2".into()),
                    ("fps".into(), "30".into()),
                    ("render_node".into(), drm.path().to_str().unwrap().into()),
                    ("exposure".into(), "1".into()),
                    (
                        "animation".into(),
                        if transition_case { "-1" } else { "0" }.into(),
                    ),
                    ("loop".into(), "true".into()),
                ],
                user_properties: String::new(),
            },
        },
    );
    let mut ready = false;
    let mut timeline = None;
    let mut formats = None;
    let mut targets = None;
    let mut subscribed = false;
    while !ready || timeline.is_none() || formats.is_none() || targets.is_none() || !subscribed {
        let (event, mut fds) = receive(&stream);
        match event {
            EventMsg::Ready { .. } => ready = true,
            EventMsg::ReleaseSyncobj => {
                assert_eq!(fds.len(), 1);
                timeline = Some(drm.fd_to_handle(&fds.remove(0)).unwrap());
            }
            EventMsg::FormatCaps { capabilities } => formats = Some(capabilities),
            EventMsg::RenderTargetCapabilities { capabilities } => targets = Some(capabilities),
            EventMsg::SetEventSubscriptions { subscription } => {
                assert!(!subscribed, "subscribed more than once");
                assert_eq!(subscription.revision, 1);
                assert_eq!(subscription.kinds, ["pointer"]);
                send(
                    &stream,
                    ControlMsg::EventSubscriptionsApplied {
                        result: EventSubscriptionResult {
                            revision: subscription.revision,
                            status: EventSubscriptionStatus::Applied,
                            kinds: subscription.kinds,
                            reason: String::new(),
                        },
                    },
                );
                subscribed = true;
            }
            EventMsg::ReportState { .. } => {}
            _ => panic!("renderer allocated before its first target: {event:?}"),
        }
    }
    let timeline = timeline.unwrap();
    let caps = targets.unwrap();
    assert_eq!(caps.mode, 2, "glTF has no native pixel extent");
    assert!(caps.revision > 0 && caps.reference_width > 0 && caps.reference_height > 0);
    assert!(caps.min_width > 0 && caps.min_height > 0);
    assert!(caps.width_alignment > 0 && caps.height_alignment > 0);
    let limit = if caps.reference_width >= caps.reference_height {
        (7680, 4320)
    } else {
        (4320, 7680)
    };
    assert!(caps.max_width <= limit.0 && caps.max_height <= limit.1);
    let producer = formats.unwrap();
    let producer = dma::unflatten_caps(
        &producer.fourccs,
        &producer.mod_counts,
        &producer.modifiers,
        &producer.plane_counts,
        &producer.device_uuid,
        &producer.driver_uuid,
        DrmNode {
            major: producer.drm_node.major,
            minor: producer.drm_node.minor,
        },
        producer.sync_caps,
        producer.color_caps,
        producer.mem_hints,
        (producer.max_extent.width, producer.max_extent.height),
    )
    .expect("valid producer capabilities");
    assert_ne!(producer.mem_hint & dma::MEM_HINT_HOST_VISIBLE, 0);
    assert_ne!(producer.sync & dma::SYNC_SYNCOBJ_TIMELINE, 0);
    assert!(
        producer
            .formats
            .by_fourcc
            .get(&dma::DRM_FORMAT_ABGR8888)
            .is_some_and(|modifiers| modifiers.iter().any(|entry| {
                entry.modifier == dma::DRM_FORMAT_MOD_LINEAR && entry.plane_count == 1
            })),
        "GPU test requires the advertised linear RGBA path"
    );
    let sizing = Capabilities::try_from(caps.clone()).expect("valid no-origin capabilities");
    let extent = |density| {
        sizing
            .select(
                density,
                Size {
                    width: producer.extent_max.0,
                    height: producer.extent_max.1,
                },
            )
            .expect("valid coordinated target extent")
    };
    let Size { width, height } = extent(0.25);
    let config = RenderTargetConfig {
        revision: 1,
        capabilities_revision: caps.revision,
        width,
        height,
        fourcc: dma::DRM_FORMAT_ABGR8888,
        modifier: dma::DRM_FORMAT_MOD_LINEAR,
        plane_count: 1,
        sync_mode: dma::SYNC_SYNCOBJ_TIMELINE,
        color: dma::DEFAULT_COLOR,
        mem_hint: dma::MEM_HINT_HOST_VISIBLE,
        count: 3,
        path: BufferPath::CompatLinear,
        memory_source: BufferMemorySource::GpuLinear,
    };
    send(
        &stream,
        ControlMsg::Pause {
            transition: ControlTransition { fade_ms: 0 },
        },
    );
    let mut sequence = 0;
    let (pool, old_buffers, initial, initial_fence) =
        configure(&stream, config.clone(), &mut sequence);
    if transition_case {
        camera_transition(
            &stream,
            &drm,
            &timeline,
            &pool,
            &old_buffers,
            (initial, initial_fence),
            &mut sequence,
        );
        shutdown(&stream, &mut child);
        return;
    }
    send(
        &stream,
        ControlMsg::Play {
            transition: ControlTransition { fade_ms: 0 },
        },
    );
    let mut held_frames = vec![(initial, initial_fence)];
    for _ in 0..2 {
        held_frames.push(receive_frame(&stream, &mut sequence));
    }
    let indices: HashSet<_> = held_frames
        .iter()
        .map(|(frame, _)| frame.image_index)
        .collect();
    assert_eq!(indices.len(), 3, "overwrote a consumer-owned slot");
    assert_no_frames(&stream, Duration::from_millis(250));
    assert!(child.0.try_wait().unwrap().is_none());
    send(
        &stream,
        ControlMsg::Pause {
            transition: ControlTransition { fade_ms: 0 },
        },
    );
    send(
        &stream,
        ControlMsg::SettingChanged {
            settings: vec![
                ("exposure".into(), "0.5".into()),
                ("animation".into(), "-1".into()),
                ("loop".into(), "false".into()),
            ],
        },
    );
    send(&stream, ControlMsg::RequestFrame);
    button(&stream, 100.0, 100.0, 0x110, PointerButtonState::Pressed);
    motion(&stream, 120.0, 110.0);
    button(&stream, 120.0, 110.0, 0x110, PointerButtonState::Released);
    assert_no_frames(&stream, Duration::from_millis(250));
    let (frame, fence) = held_frames.last().unwrap();
    release(&drm, &timeline, frame, fence);
    let (resumed, resumed_fence) = receive_frame(&stream, &mut sequence);
    assert!(resumed.image_index < 3);
    assert_no_frames(&stream, Duration::from_millis(250));
    release(&drm, &timeline, &resumed, &resumed_fence);

    send(
        &stream,
        ControlMsg::SettingChanged {
            settings: vec![("animation".into(), i32::MAX.to_string())],
        },
    );
    let (static_frame, static_fence) = receive_frame(&stream, &mut sequence);
    release(&drm, &timeline, &static_frame, &static_fence);
    send(&stream, ControlMsg::RequestFrame);
    let (requested, requested_fence) = receive_frame(&stream, &mut sequence);
    let original_hash = pixel_hash(&pool, &old_buffers, &requested, &requested_fence);
    release(&drm, &timeline, &requested, &requested_fence);
    assert_no_frames(&stream, Duration::from_millis(250));

    let x = pool.extent.width as f32 * 0.5;
    let y = pool.extent.height as f32 * 0.5;
    motion(&stream, x, y);
    assert_no_frames(&stream, Duration::from_millis(100));
    button(&stream, x, y, 0x110, PointerButtonState::Pressed);
    motion(&stream, x + 35.0, y + 15.0);
    button(
        &stream,
        x + 35.0,
        y + 15.0,
        0x110,
        PointerButtonState::Released,
    );
    let (rotated, rotated_fence) = receive_frame(&stream, &mut sequence);
    let rotated_hash = pixel_hash(&pool, &old_buffers, &rotated, &rotated_fence);
    assert_ne!(original_hash, rotated_hash, "drag did not rotate the model");
    release(&drm, &timeline, &rotated, &rotated_fence);
    motion(&stream, x + 60.0, y + 30.0);
    assert_no_frames(&stream, Duration::from_millis(100));

    button(&stream, x, y, 0x110, PointerButtonState::Pressed);
    motion(&stream, -1.0, -1.0);
    motion(&stream, x + 60.0, y + 30.0);
    motion(&stream, f32::NAN, y);
    assert_no_frames(&stream, Duration::from_millis(100));

    let mut previous_hash = rotated_hash;
    for pan_button in [0x111, 0x112] {
        button(&stream, x, y, pan_button, PointerButtonState::Pressed);
        motion(&stream, x + 12.0, y - 8.0);
        button(
            &stream,
            x + 12.0,
            y - 8.0,
            pan_button,
            PointerButtonState::Released,
        );
        let (panned, panned_fence) = receive_frame(&stream, &mut sequence);
        let panned_hash = pixel_hash(&pool, &old_buffers, &panned, &panned_fence);
        assert_ne!(previous_hash, panned_hash, "drag did not pan the model");
        previous_hash = panned_hash;
        release(&drm, &timeline, &panned, &panned_fence);
        assert_no_frames(&stream, Duration::from_millis(100));
    }
    send(
        &stream,
        ControlMsg::SettingChanged {
            settings: vec![
                ("fps".into(), "24".into()),
                ("exposure".into(), "0.5".into()),
                ("loop".into(), "true".into()),
            ],
        },
    );
    let (configured, configured_fence) = receive_frame(&stream, &mut sequence);
    assert_eq!(
        previous_hash,
        pixel_hash(&pool, &old_buffers, &configured, &configured_fence),
        "ordinary settings reset the interactive camera"
    );
    release(&drm, &timeline, &configured, &configured_fence);
    assert_no_frames(&stream, Duration::from_millis(100));
    send(
        &stream,
        ControlMsg::Play {
            transition: ControlTransition { fade_ms: 0 },
        },
    );
    let (played, played_fence) = receive_frame(&stream, &mut sequence);
    assert_eq!(
        previous_hash,
        pixel_hash(&pool, &old_buffers, &played, &played_fence),
        "playing a static model changed the frame"
    );
    release(&drm, &timeline, &played, &played_fence);
    assert_no_frames(&stream, Duration::from_millis(100));
    for (axis_x, delta_x, delta_y) in [(x, 2.0, 0.0), (x, 0.0, f32::NAN), (f32::NAN, 0.0, 2.0)] {
        send(
            &stream,
            ControlMsg::PointerAxis {
                event: PointerAxis {
                    x: axis_x,
                    y,
                    delta_x,
                    delta_y,
                    source: PointerAxisSource::Wheel,
                    timestamp_us: 0,
                    modifiers: 0,
                },
            },
        );
    }
    assert_no_frames(&stream, Duration::from_millis(100));
    send(
        &stream,
        ControlMsg::PointerAxis {
            event: PointerAxis {
                x,
                y,
                delta_x: 0.0,
                delta_y: 2.0,
                source: PointerAxisSource::Wheel,
                timestamp_us: 0,
                modifiers: 0,
            },
        },
    );
    let (zoomed, zoomed_fence) = receive_frame(&stream, &mut sequence);
    let zoomed_hash = pixel_hash(&pool, &old_buffers, &zoomed, &zoomed_fence);
    assert_ne!(previous_hash, zoomed_hash, "wheel did not zoom the model");
    release(&drm, &timeline, &zoomed, &zoomed_fence);
    assert_no_frames(&stream, Duration::from_millis(100));
    send(
        &stream,
        ControlMsg::Pause {
            transition: ControlTransition { fade_ms: 0 },
        },
    );

    let mut invalid = config.clone();
    invalid.revision = 2;
    invalid.capabilities_revision = caps.revision + 1;
    send(
        &stream,
        ControlMsg::ConfigureRenderTarget { config: invalid },
    );
    loop {
        let (event, _) = receive(&stream);
        match event {
            EventMsg::RenderTargetResult { result } => {
                assert_eq!(result.revision, 2);
                assert_eq!(result.status, 1);
                break;
            }
            EventMsg::ReportState { .. } => {}
            _ => panic!("invalid target changed the active pool: {event:?}"),
        }
    }
    let Size { width, height } = extent(0.5);
    let resize = RenderTargetConfig {
        revision: 3,
        width,
        height,
        ..config
    };
    button(&stream, x, y, 0x110, PointerButtonState::Pressed);
    let (resized, new_buffers, resized_frame, resized_fence) =
        configure(&stream, resize, &mut sequence);
    assert!(resized.generation > pool.generation);
    assert_eq!(old_buffers.len(), 3);
    assert_eq!(new_buffers.len(), 3);
    motion(&stream, width as f32 * 0.5, height as f32 * 0.5);
    assert_no_frames(&stream, Duration::from_millis(250));
    send(
        &stream,
        ControlMsg::SettingChanged {
            settings: vec![
                ("animation".into(), "0".into()),
                ("loop".into(), "true".into()),
            ],
        },
    );
    send(
        &stream,
        ControlMsg::Play {
            transition: ControlTransition { fade_ms: 0 },
        },
    );
    let mut shutdown_frames = vec![(resized_frame, resized_fence)];
    for _ in 0..2 {
        shutdown_frames.push(receive_frame(&stream, &mut sequence));
    }
    assert_eq!(
        shutdown_frames
            .iter()
            .map(|(frame, _)| frame.image_index)
            .collect::<HashSet<_>>()
            .len(),
        3,
        "overwrote a consumer-owned resized slot"
    );
    assert_no_frames(&stream, Duration::from_millis(250));
    shutdown(&stream, &mut child);
}
