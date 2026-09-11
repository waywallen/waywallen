use std::os::fd::OwnedFd;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

mod common;

use waywallen::wallframe::ipc::generated::{
    BufferDirective, BufferFormat, BufferMemorySource, BufferPath, Event as EventMsg,
    EventIn as ControlMsg, RendererInit, PROTOCOL_VERSION,
};
use waywallen::wallframe::ipc::uds::{recv_event, send_control};
use waywallen::wallframe::sync::DrmDevice;

fn renderer_bin() -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidate = manifest.join("plugins/image/build/waywallen-image-renderer");
    if candidate.exists() {
        return Some(candidate);
    }
    let install = manifest
        .parent()
        .map(|p| p.join("install/bin/waywallen-image-renderer"))?;
    install.exists().then_some(install)
}

fn image_path() -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidate = manifest.join("ui/assets/main_page.png");
    candidate.exists().then_some(candidate)
}

#[test]
fn release_syncobj_round_trip() {
    let Some(bin) = renderer_bin() else {
        eprintln!("skip: waywallen-image-renderer binary not found");
        return;
    };
    let Some(img) = image_path() else {
        eprintln!("skip: ui/assets/main_page.png not found");
        return;
    };
    let drm = match DrmDevice::open_first_render_node() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("skip: no DRM render node ({e})");
            return;
        }
    };

    let sock_path = common::tmp_sock("release-syncobj-e2e");
    let _ = std::fs::remove_file(&sock_path);
    let listener = UnixListener::bind(&sock_path).expect("bind");
    let _cleanup = common::SockCleanup(sock_path.clone());

    // SPAWN_VERSION 3 passes the image path via `--path`.
    // Extent and settings ride on the typed Init message below.
    let child = Command::new(&bin)
        .arg("--ipc")
        .arg(&sock_path)
        .arg("--path")
        .arg(&img)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn waywallen-image-renderer");
    let mut guard = common::ChildGuard(child);

    let (stream, _) = match common::accept_with_timeout(&listener, Duration::from_secs(10)) {
        Some(Ok(x)) => x,
        _ => {
            let _ = guard.0.kill();
            panic!("accept timed out");
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set rd timeout");

    // Drive the daemon's role: send Init so the renderer can move
    // past `ww_bridge_recv_init`.
    send_control(
        &stream,
        &ControlMsg::Init {
            config: RendererInit {
                protocol_version: PROTOCOL_VERSION,
                spawn_version: waywallen::wallframe::renderer_manager::SPAWN_VERSION,
                settings: Vec::new(),
                user_properties: String::new(),
                display_width: 0,
                display_height: 0,
            },
        },
        &[],
    )
    .expect("send Init");

    let mut saw_ready = false;
    let mut saw_release_syncobj_fd: Option<OwnedFd> = None;
    let mut saw_frame_with_release_point = false;
    let mut saw_format_caps = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);

    while std::time::Instant::now() < deadline {
        let (msg, mut fds) = match recv_event(&stream) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("recv error: {e}");
                break;
            }
        };
        match msg {
            EventMsg::Ready { .. } => {
                saw_ready = true;
            }
            EventMsg::FormatCaps { capabilities } => {
                use waywallen::wallframe::dma::negotiate as N;
                let caps = N::unflatten_caps(
                    &capabilities.fourccs,
                    &capabilities.mod_counts,
                    &capabilities.modifiers,
                    &capabilities.plane_counts,
                    &capabilities.device_uuid,
                    &capabilities.driver_uuid,
                    waywallen::wallframe::renderer_manager::DrmNode {
                        major: capabilities.drm_node.major,
                        minor: capabilities.drm_node.minor,
                    },
                    capabilities.sync_caps,
                    capabilities.color_caps,
                    capabilities.mem_hints,
                    (
                        capabilities.max_extent.width,
                        capabilities.max_extent.height,
                    ),
                )
                .expect("FormatCaps unflatten");
                // Producer must advertise ABGR8888.
                let abgr = caps
                    .formats
                    .by_fourcc
                    .get(&N::DRM_FORMAT_ABGR8888)
                    .expect("FormatCaps must list ABGR8888");
                assert!(
                    abgr.iter().any(|m| m.modifier == N::DRM_FORMAT_MOD_LINEAR),
                    "FormatCaps must include LINEAR modifier"
                );
                // Mesa always reports a non-zero device UUID.
                assert!(
                    caps.identity.device_uuid != [0u8; 16],
                    "Mesa device_uuid should be non-zero"
                );
                assert!(
                    caps.mem_hint & N::MEM_HINT_HOST_VISIBLE != 0,
                    "image renderer should advertise HOST_VISIBLE"
                );
                assert!(
                    caps.sync & N::SYNC_SYNCOBJ_TIMELINE != 0,
                    "image renderer should advertise SYNCOBJ_TIMELINE"
                );
                saw_format_caps = true;

                // Renderer waits for NegotiateBuffers before binding.
                // Drive that handshake here so the test controls format.
                send_control(
                    &stream,
                    &ControlMsg::NegotiateBuffers {
                        directive: BufferDirective {
                            format: BufferFormat {
                                fourcc: N::DRM_FORMAT_ABGR8888,
                                modifier: N::DRM_FORMAT_MOD_LINEAR,
                                plane_count: 1,
                            },
                            sync_mode: N::SYNC_SYNCOBJ_TIMELINE,
                            color: N::DEFAULT_COLOR,
                            mem_hint: N::MEM_HINT_HOST_VISIBLE,
                            count: 1,
                            path: BufferPath::CompatLinear,
                            memory_source: BufferMemorySource::GpuLinear,
                        },
                    },
                    &[],
                )
                .expect("send NegotiateBuffers");
            }
            EventMsg::ReleaseSyncobj => {
                assert_eq!(fds.len(), 1, "ReleaseSyncobj expected exactly 1 fd");
                let fd = fds.remove(0);
                // Verify it imports cleanly as a drm_syncobj.
                let handle = drm.fd_to_handle(&fd).expect("import release_syncobj fd");
                drop(handle);
                saw_release_syncobj_fd = Some(fd);
            }
            EventMsg::BindBuffers { .. } => {
                // Drop dma-buf fds; we're not actually binding.
                drop(fds);
            }
            EventMsg::FrameReady { frame } => {
                assert_eq!(fds.len(), 1, "FrameReady expected 1 acquire sync_fd");
                drop(fds);
                assert!(
                    frame.release_point > 0,
                    "FrameReady sequence={} release_point must be > 0 (got {})",
                    frame.sequence,
                    frame.release_point,
                );
                saw_frame_with_release_point = true;
                break;
            }
            other => {
                eprintln!("unexpected msg: {other:?}");
            }
        }
    }

    assert!(saw_ready, "never saw Ready");
    assert!(saw_format_caps, "never saw FormatCaps");
    assert!(
        saw_release_syncobj_fd.is_some(),
        "never saw ReleaseSyncobj event with importable drm_syncobj fd"
    );
    assert!(
        saw_frame_with_release_point,
        "never saw FrameReady with release_point > 0"
    );
}
