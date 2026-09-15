use anyhow::{anyhow, Context, Result};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use waywallen::wallframe::display::proto::{
    codec, ConsumerCapabilities, DisplayMetrics, Event, PresentationCapabilities, Request,
    PROTOCOL_VERSION,
};

#[derive(Debug)]
struct Args {
    socket: Option<PathBuf>,
    name: String,
    width: u32,
    height: u32,
    refresh_mhz: u32,
    max_frames: Option<u64>,
}

fn parse_args() -> Args {
    let mut a = Args {
        socket: None,
        name: "waywallen-display-demo".to_string(),
        width: 1920,
        height: 1080,
        refresh_mhz: 60_000,
        max_frames: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--socket" | "--display-sock" => {
                a.socket = it.next().map(PathBuf::from);
            }
            "--name" => {
                if let Some(v) = it.next() {
                    a.name = v;
                }
            }
            "--width" => {
                if let Some(v) = it.next() {
                    a.width = v.parse().unwrap_or(a.width);
                }
            }
            "--height" => {
                if let Some(v) = it.next() {
                    a.height = v.parse().unwrap_or(a.height);
                }
            }
            "--refresh-mhz" => {
                if let Some(v) = it.next() {
                    a.refresh_mhz = v.parse().unwrap_or(a.refresh_mhz);
                }
            }
            "--max-frames" => {
                if let Some(v) = it.next() {
                    a.max_frames = v.parse().ok();
                }
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => {
                eprintln!("warning: ignoring unknown arg {other}");
            }
        }
    }
    a
}

fn print_usage() {
    eprintln!(
        "usage: waywallen_display_demo \
[--socket PATH] [--name STR] [--width W] [--height H] \
[--refresh-mhz MHZ] [--max-frames N]"
    );
}

fn default_socket_path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    runtime.join("waywallen").join("display.sock")
}

fn main() {
    waywallen::logging::init_stderr("info");
    let args = parse_args();
    let sock_path = args.socket.clone().unwrap_or_else(default_socket_path);
    log::info!(
        "display demo: sock={} name={} size={}x{}",
        sock_path.display(),
        args.name,
        args.width,
        args.height
    );

    // Reconnect loop: any session-level failure (connect refused,
    // daemon died, protocol mismatch, etc.) is logged and retried
    loop {
        match run_session(&sock_path, &args) {
            Ok(()) => {
                log::info!("session ended cleanly");
                if args.max_frames.is_some() {
                    // In smoke-test mode (max_frames set) exit once the
                    // budget is reached rather than looping forever.
                    return;
                }
            }
            Err(e) => log::warn!("session error: {e:#}"),
        }
        thread::sleep(Duration::from_secs(2));
    }
}

fn run_session(sock_path: &Path, args: &Args) -> Result<()> {
    // ---- connect ----
    let stream = UnixStream::connect(sock_path)
        .with_context(|| format!("connect {}", sock_path.display()))?;
    log::info!("connected to {}", sock_path.display());

    // ---- hello / welcome ----
    codec::send_request(
        &stream,
        &Request::Hello {
            client_name: args.name.clone(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: PROTOCOL_VERSION,
        },
        &[],
    )
    .map_err(|e| anyhow!("send hello: {e}"))?;

    let (welcome, _fds) = codec::recv_event(&stream).map_err(|e| anyhow!("recv welcome: {e}"))?;
    match welcome {
        Event::Welcome { server_version } => {
            log::info!("welcome from {server_version}");
        }
        other => return Err(anyhow!("expected welcome, got opcode {}", other.opcode())),
    }

    // ---- register_display / display_accepted ----
    codec::send_request(
        &stream,
        &Request::RegisterDisplay {
            name: args.name.clone(),
            instance_id: String::new(),
            metrics: DisplayMetrics {
                width: args.width,
                height: args.height,
                refresh_mhz: args.refresh_mhz,
            },
            consumer_caps: ConsumerCapabilities {
                fourccs: vec![0x3432_4241, 0x3432_4258, 0x3432_5241, 0x3432_5258],
                mod_counts: vec![1; 4],
                modifiers: vec![0; 4],
                plane_counts: vec![1; 4],
                device_uuid: vec![0; 4],
                driver_uuid: vec![0; 4],
                drm_render_major: 0,
                drm_render_minor: 0,
                mem_hints: 1 << 1,
                // The release syncobj in frame_ready is always a daemon-owned
                // binary one, so the renderer's fence flavour does not matter here.
                sync_caps: (1 << 1) | (1 << 2),
                color_caps: (1 << 0) | (1 << 6) | (1 << 7),
                extent_max_w: args.width,
                extent_max_h: args.height,
            },
            presentation_caps: PresentationCapabilities { flags: 0 },
            window_state_flags: 0,
        },
        &[],
    )
    .map_err(|e| anyhow!("send register_display: {e}"))?;

    let display_id =
        match codec::recv_event(&stream).map_err(|e| anyhow!("recv display_accepted: {e}"))? {
            (Event::DisplayAccepted { display_id, .. }, _) => display_id,
            (other, _) => {
                return Err(anyhow!(
                    "expected display_accepted, got opcode {}",
                    other.opcode()
                ))
            }
        };
    log::info!("registered as display_id={display_id}");

    // ---- initial binding ----
    let (first, first_fds) =
        codec::recv_event(&stream).map_err(|e| anyhow!("recv bind_buffers: {e}"))?;
    let buffer_generation = match first {
        Event::BindBuffers {
            buffer_generation,
            count,
            width,
            height,
            fourcc,
            modifier,
            planes_per_buffer,
            initial_config,
            ..
        } => {
            let expected_fds = (count * planes_per_buffer) as usize;
            if first_fds.len() != expected_fds {
                return Err(anyhow!(
                    "bind_buffers expected {expected_fds} fds, got {}",
                    first_fds.len()
                ));
            }
            log::info!(
                "bind_buffers gen={buffer_generation} count={count} tex={width}x{height} \
                 fourcc=0x{fourcc:08x} modifier=0x{modifier:016x} planes={planes_per_buffer} \
                 composition={} source=({:.0},{:.0},{:.0},{:.0}) \
                 dest=({:.0},{:.0},{:.0},{:.0}) ({} dma-buf fds)",
                initial_config.generation,
                initial_config.source_rect.x,
                initial_config.source_rect.y,
                initial_config.source_rect.w,
                initial_config.source_rect.h,
                initial_config.dest_rect.x,
                initial_config.dest_rect.y,
                initial_config.dest_rect.w,
                initial_config.dest_rect.h,
                first_fds.len(),
            );
            drop(first_fds);
            buffer_generation
        }
        other => {
            return Err(anyhow!(
                "expected bind_buffers, got opcode {}",
                other.opcode()
            ))
        }
    };

    // ---- frame loop ----
    let mut frames_seen: u64 = 0;
    let mut buffer_generation = Some(buffer_generation);
    loop {
        let (evt, mut fds) = codec::recv_event(&stream).map_err(|e| anyhow!("recv event: {e}"))?;
        match evt {
            Event::FrameReady {
                buffer_generation: g,
                buffer_index,
                seq,
            } => {
                if Some(g) != buffer_generation {
                    log::warn!(
                        "stray frame_ready gen={g} (current={buffer_generation:?}); dropping"
                    );
                    drop(fds);
                    continue;
                }
                if fds.len() != 2 {
                    return Err(anyhow!(
                        "frame_ready expected 2 fds (acquire + release_syncobj), got {}",
                        fds.len()
                    ));
                }
                let release_fd = fds.swap_remove(1);
                let device =
                    waywallen::wallframe::sync::drm_device().context("open DRM render node")?;
                let release = device
                    .fd_to_handle(&release_fd)
                    .context("import release syncobj")?;
                device.signal(&release).context("signal release syncobj")?;
                drop((fds, release_fd, release));
                codec::send_request(
                    &stream,
                    &Request::FrameReleaseArmed {
                        buffer_generation: g,
                        seq,
                    },
                    &[],
                )
                .map_err(|error| anyhow!("send frame_release_armed: {error}"))?;

                frames_seen += 1;
                log::info!(
                    "display {display_id}: frame {frames_seen} ready (idx={buffer_index} seq={seq})"
                );
                if let Some(max) = args.max_frames {
                    if frames_seen >= max {
                        log::info!("max-frames reached; closing session");
                        return Ok(());
                    }
                }
            }
            Event::BindBuffers {
                buffer_generation: generation,
                initial_config,
                ..
            } => {
                if initial_config.buffer_generation != generation {
                    return Err(anyhow!("bind composition targets another generation"));
                }
                buffer_generation = Some(generation);
                log::info!("installed binding generation {generation}");
                drop(fds);
            }
            Event::SetCompositionConfig { config } => {
                if Some(config.buffer_generation) == buffer_generation {
                    log::info!("received composition generation {}", config.generation);
                } else {
                    log::warn!("ignored stale composition generation {}", config.generation);
                }
            }
            Event::Unbind {
                buffer_generation: g,
            } => {
                if buffer_generation == Some(g) {
                    buffer_generation = None;
                }
                codec::send_request(
                    &stream,
                    &Request::AckUnbind {
                        buffer_generation: g,
                    },
                    &[],
                )
                .map_err(|error| anyhow!("send ack_unbind: {error}"))?;
                log::info!("server unbound generation {g}");
            }
            Event::Error { code, message } => {
                return Err(anyhow!("server error {code:?}: {message}"));
            }
            other => {
                log::warn!("ignoring unexpected event opcode {}", other.opcode());
            }
        }
    }
}
