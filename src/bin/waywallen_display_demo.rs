//! `waywallen_display_demo` — minimal headless client that exercises
//! the `waywallen-display-v1` wire protocol end-to-end.

use anyhow::{anyhow, Context, Result};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use waywallen::display_proto::{codec, Event, Request, PROTOCOL_NAME};

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
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .init();
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
    // after 2 seconds.
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
            protocol: PROTOCOL_NAME.to_string(),
            client_name: args.name.clone(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
        },
        &[],
    )
    .map_err(|e| anyhow!("send hello: {e}"))?;

    let (welcome, _fds) =
        codec::recv_event(&stream).map_err(|e| anyhow!("recv welcome: {e}"))?;
    match welcome {
        Event::Welcome { server_version, features } => {
            log::info!("welcome from {server_version}, features={features:?}");
            if !features.iter().any(|s| s == "explicit_sync_fd") {
                return Err(anyhow!(
                    "server missing mandatory feature \"explicit_sync_fd\""
                ));
            }
        }
        other => {
            return Err(anyhow!(
                "expected welcome, got opcode {}",
                other.opcode()
            ))
        }
    }

    // ---- register_display / display_accepted ----
    codec::send_request(
        &stream,
        &Request::RegisterDisplay {
            name: args.name.clone(),
            width: args.width,
            height: args.height,
            refresh_mhz: args.refresh_mhz,
            properties: Vec::new(),
        },
        &[],
    )
    .map_err(|e| anyhow!("send register_display: {e}"))?;

    let display_id = match codec::recv_event(&stream)
        .map_err(|e| anyhow!("recv display_accepted: {e}"))?
    {
        (Event::DisplayAccepted { display_id }, _) => display_id,
        (other, _) => {
            return Err(anyhow!(
                "expected display_accepted, got opcode {}",
                other.opcode()
            ))
        }
    };
    log::info!("registered as display_id={display_id}");

    // ---- bind_buffers + set_config ----
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
                 (received {} dma-buf fds; closing without import)",
                first_fds.len()
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

    let (cfg, _fds) =
        codec::recv_event(&stream).map_err(|e| anyhow!("recv set_config: {e}"))?;
    match cfg {
        Event::SetConfig {
            config_generation,
            source_rect,
            dest_rect,
            transform,
            ..
        } => {
            log::info!(
                "set_config gen={config_generation} source=({:.0},{:.0},{:.0},{:.0}) \
                 dest=({:.0},{:.0},{:.0},{:.0}) xform={transform}",
                source_rect.x, source_rect.y, source_rect.w, source_rect.h,
                dest_rect.x, dest_rect.y, dest_rect.w, dest_rect.h,
            );
        }
        other => {
            return Err(anyhow!(
                "expected set_config, got opcode {}",
                other.opcode()
            ))
        }
    }

    // ---- frame loop ----
    let mut frames_seen: u64 = 0;
    loop {
        let (evt, fds) =
            codec::recv_event(&stream).map_err(|e| anyhow!("recv event: {e}"))?;
        match evt {
            Event::FrameReady {
                buffer_generation: g,
                buffer_index,
                seq,
            } => {
                if g != buffer_generation {
                    log::warn!(
                        "stray frame_ready gen={g} (current={buffer_generation}); dropping"
                    );
                    drop(fds);
                    continue;
                }
                if fds.len() != 1 {
                    return Err(anyhow!(
                        "frame_ready expected 1 acquire sync_fd, got {}",
                        fds.len()
                    ));
                }
                // Drop the fd — Phase 1 demo does not import it. The
                // OwnedFd destructor closes it.
                drop(fds);

                frames_seen += 1;
                log::info!(
                    "display {display_id}: frame {frames_seen} ready (idx={buffer_index} seq={seq})"
                );

                codec::send_request(
                    &stream,
                    &Request::BufferRelease {
                        buffer_generation: g,
                        buffer_index,
                        seq,
                    },
                    &[],
                )
                .map_err(|e| anyhow!("send buffer_release: {e}"))?;

                if let Some(max) = args.max_frames {
                    if frames_seen >= max {
                        log::info!("max-frames reached; sending bye");
                        codec::send_request(&stream, &Request::Bye, &[])
                            .map_err(|e| anyhow!("send bye: {e}"))?;
                        return Ok(());
                    }
                }
            }
            Event::BindBuffers { .. } => {
                log::warn!("mid-session bind_buffers ignored (Phase 1)");
                drop(fds);
            }
            Event::SetConfig { .. } => {
                log::info!("received updated set_config");
            }
            Event::Unbind { buffer_generation: g } => {
                log::info!("server unbound generation {g}; ending session");
                return Ok(());
            }
            Event::Error { code, message } => {
                return Err(anyhow!("server error {code}: {message}"));
            }
            other => {
                log::warn!("ignoring unexpected event opcode {}", other.opcode());
            }
        }
    }
}
