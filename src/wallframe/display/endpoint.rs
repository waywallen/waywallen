use anyhow::anyhow;
use std::collections::HashMap;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::events::GlobalEvent;
use crate::wallframe::display::proto::generated::{self as wire, Rect};
use crate::wallframe::display::proto::{codec, opcode, Event, Request, PROTOCOL_VERSION};
use crate::wallframe::ipc::proto::{
    PointerAxis as RendererPointerAxis, PointerAxisSource as RendererPointerAxisSource,
    PointerButton as RendererPointerButton, PointerButtonState as RendererPointerButtonState,
    PointerMotion as RendererPointerMotion,
};
// Display-protocol failures are daemon-internal; this layer talks to
// display consumers over a UDS, not public WS or D-Bus surfaces.
use crate::error::{Error, Result, ResultExt};
use crate::wallframe::display::layout::display_point_to_texture;
use crate::wallframe::renderer_manager::{PublishedPool, RendererHandle};
use crate::wallframe::routing::{
    ConsumerImportFailureKind, ConsumerImportFailureOutcome, DisplayConsumptionPermit,
    DisplayHandle, DisplayOutEvent, DisplayRegistration, PresentationSnapshot, PresentationState,
    Router,
};
use crate::wallframe::scheduler::{CompositionConfig, DisplayMetrics};
use crate::wallframe::sync::drm_device;

/// Server version string advertised in `welcome.server_version`.
/// Free-form, informational; consumers do not gate on this.
pub const SERVER_VERSION: &str = concat!("waywallen ", env!("CARGO_PKG_VERSION"));

/// Inclusive range of protocol versions this daemon
/// accepts. Unsupported versions are rejected during handshake.
pub const MIN_SUPPORTED_CLIENT_VERSION: u32 = PROTOCOL_VERSION;
pub const MAX_SUPPORTED_CLIENT_VERSION: u32 = PROTOCOL_VERSION;

// ---------------------------------------------------------------------------
// Public entry point

pub fn default_socket_path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let dir = runtime.join("waywallen");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("display.sock")
}

/// Back-compat 2-arg entry point used by integration tests that
/// don't care about daemon-level shutdown.
pub async fn serve(
    sock_path: &Path,
    router: Arc<Router>,
    events_tx: tokio::sync::broadcast::Sender<GlobalEvent>,
) -> Result<()> {
    // Holding `_never_tx` in scope keeps `wait_for` parked on `Pending`
    // instead of making subscribers observe `RecvError::Closed`.
    let (_never_tx, rx) = tokio::sync::watch::channel(false);
    let res = serve_with_shutdown(sock_path, router, events_tx, rx).await;
    drop(_never_tx);
    res
}

pub async fn serve_with_shutdown(
    sock_path: &Path,
    router: Arc<Router>,
    events_tx: tokio::sync::broadcast::Sender<GlobalEvent>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let listener = bind_socket(sock_path).await?;
    serve_bound(sock_path, listener, router, events_tx, shutdown_rx).await
}

pub async fn bind_socket(sock_path: &Path) -> Result<tokio::net::UnixListener> {
    match std::fs::remove_file(sock_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            log::warn!(
                "failed to remove stale display socket {}: {e}",
                sock_path.display()
            );
        }
    }
    if let Some(parent) = sock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let listener = tokio::net::UnixListener::bind(sock_path)
        .with_context(|| format!("bind display socket at {}", sock_path.display()))?;
    log::info!("display endpoint listening on {}", sock_path.display());
    Ok(listener)
}

pub async fn serve_bound(
    sock_path: &Path,
    listener: tokio::net::UnixListener,
    router: Arc<Router>,
    events_tx: tokio::sync::broadcast::Sender<GlobalEvent>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let mut clients = tokio::task::JoinSet::new();

    loop {
        let accepted = tokio::select! {
            biased;
            _ = wait_shutdown(&mut shutdown_rx) => {
                log::info!("display endpoint: shutdown received, ceasing accept");
                break;
            }
            res = listener.accept() => res,
            joined = clients.join_next(), if !clients.is_empty() => {
                if let Some(Err(error)) = joined {
                    log::warn!("display client task join failed: {error}");
                }
                continue;
            }
        };
        let (stream, _addr) = match accepted {
            Ok(x) => x,
            Err(e) => {
                log::warn!("display accept failed: {e}");
                continue;
            }
        };
        let std_stream = match stream
            .into_std()
            .and_then(|s| s.set_nonblocking(false).map(|_| s))
        {
            Ok(s) => s,
            Err(e) => {
                log::warn!("display into_std failed: {e}");
                continue;
            }
        };
        let router = Arc::clone(&router);
        let client_shutdown_rx = shutdown_rx.clone();
        let client_events_tx = events_tx.clone();
        clients.spawn(async move {
            if let Err(e) =
                handle_client(std_stream, router, client_events_tx, client_shutdown_rx).await
            {
                log::info!("display client closed: {e}");
            }
        });
    }

    while let Some(joined) = clients.join_next().await {
        if let Err(error) = joined {
            log::warn!("display client task join failed during shutdown: {error}");
        }
    }
    drop(listener);
    match std::fs::remove_file(sock_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            log::warn!(
                "failed to remove display socket {}: {e}",
                sock_path.display()
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-client state machine

async fn handle_client(
    stream: StdUnixStream,
    router: Arc<Router>,
    events_tx: tokio::sync::broadcast::Sender<GlobalEvent>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    log::info!("display client connected; performing handshake");
    let registration = do_handshake(&stream, &events_tx, &mut shutdown_rx).await?;
    let handle = router.try_register_display(registration).await?;
    let DisplayHandle {
        id: display_id,
        session_id,
        presentation,
        rx,
    } = handle;
    log::info!("display {display_id} registered with router");

    let result = async {
        let send_ack_stream = stream.try_clone().context("clone for accepted")?;
        tokio::task::spawn_blocking(move || {
            codec::send_event(
                &send_ack_stream,
                &Event::DisplayAccepted {
                    display_id,
                    presentation: presentation_to_wire(presentation),
                },
                &[],
            )
        })
        .await
        .context("accepted join")?
        .map_err(|e| Error::Internal(anyhow!("send display_accepted: {e}")))?;

        run_frame_loop(
            stream,
            router.clone(),
            display_id,
            session_id,
            rx,
            shutdown_rx,
        )
        .await
    }
    .await;
    router.unregister_display(display_id).await;
    result
}

// ---------------------------------------------------------------------------
// Handshake

enum HandshakeAbort {
    Reject {
        code: wire::DisplayErrorCode,
        message: String,
    },
    Hangup(Error),
}

impl From<Error> for HandshakeAbort {
    fn from(error: Error) -> Self {
        Self::Hangup(error)
    }
}

fn reject(code: wire::DisplayErrorCode, message: String) -> HandshakeAbort {
    HandshakeAbort::Reject { code, message }
}

#[derive(Default)]
struct HandshakePeer {
    client_name: String,
    protocol_version: u32,
}

async fn do_handshake(
    stream: &StdUnixStream,
    events_tx: &tokio::sync::broadcast::Sender<GlobalEvent>,
    shutdown_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<DisplayRegistration> {
    let mut peer = HandshakePeer::default();
    match handshake_steps(stream, &mut peer, shutdown_rx).await {
        Ok(registration) => Ok(registration),
        Err(HandshakeAbort::Hangup(error)) => Err(error),
        Err(HandshakeAbort::Reject { code, message }) => {
            let _ = send_error(stream, code, message.clone()).await;
            report_connection_failure(
                events_tx,
                peer.client_name,
                peer.protocol_version,
                code,
                message.clone(),
            );
            Err(Error::Internal(anyhow!(message)))
        }
    }
}

/// A foreign frame shape may fail before its protocol version can be decoded.
async fn recv_handshake_frame(
    stream: &StdUnixStream,
    stage: &'static str,
    shutdown_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> std::result::Result<(Request, Vec<OwnedFd>), HandshakeAbort> {
    recv_request_cancellable(stream, shutdown_rx)
        .await
        .map_err(|error| {
            if error.is_protocol_failure() {
                reject(
                    wire::DisplayErrorCode::ProtocolViolation,
                    format!(
                        "incompatible or malformed display {stage}: {error}; \
                         daemon accepts protocol \
                         [{MIN_SUPPORTED_CLIENT_VERSION}..={MAX_SUPPORTED_CLIENT_VERSION}]"
                    ),
                )
            } else {
                HandshakeAbort::Hangup(Error::Internal(anyhow!("recv {stage}: {error}")))
            }
        })
}

async fn handshake_steps(
    stream: &StdUnixStream,
    peer: &mut HandshakePeer,
    shutdown_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> std::result::Result<DisplayRegistration, HandshakeAbort> {
    let (hello, _fds) = recv_handshake_frame(stream, "hello", shutdown_rx).await?;
    let Request::Hello {
        client_name,
        client_version,
        protocol_version,
    } = hello
    else {
        return Err(reject(
            wire::DisplayErrorCode::ProtocolViolation,
            format!("expected hello, got opcode {}", hello.opcode()),
        ));
    };
    peer.client_name = client_name.clone();
    peer.protocol_version = protocol_version;
    if !(MIN_SUPPORTED_CLIENT_VERSION..=MAX_SUPPORTED_CLIENT_VERSION).contains(&protocol_version) {
        return Err(reject(
            wire::DisplayErrorCode::VersionUnsupported,
            format!(
                "client protocol v{protocol_version} not supported; \
                 daemon accepts [{MIN_SUPPORTED_CLIENT_VERSION}..={MAX_SUPPORTED_CLIENT_VERSION}]"
            ),
        ));
    }
    log::info!("display hello: {client_name} v{client_version} (proto v{protocol_version})");

    let welcome_stream = stream.try_clone().context("clone for welcome")?;
    tokio::task::spawn_blocking(move || {
        codec::send_event(
            &welcome_stream,
            &Event::Welcome {
                server_version: SERVER_VERSION.to_string(),
            },
            &[],
        )
    })
    .await
    .context("welcome join")?
    .map_err(|e| Error::Internal(anyhow!("send welcome: {e}")))?;

    let (reg, _fds) = recv_handshake_frame(stream, "registration", shutdown_rx).await?;
    let Request::RegisterDisplay {
        name,
        instance_id,
        metrics,
        consumer_caps,
        presentation_caps,
        window_state_flags,
    } = reg
    else {
        return Err(reject(
            wire::DisplayErrorCode::ProtocolViolation,
            format!("expected register_display, got opcode {}", reg.opcode()),
        ));
    };
    let instance_id = if instance_id.is_empty() {
        None
    } else {
        Some(instance_id)
    };
    if metrics.width == 0 || metrics.height == 0 {
        return Err(reject(
            wire::DisplayErrorCode::ProtocolViolation,
            format!(
                "register_display has invalid extent {}x{}",
                metrics.width, metrics.height
            ),
        ));
    }
    if window_state_flags & !crate::wallframe::routing::auto_replay::FLAGS_KNOWN != 0 {
        return Err(reject(
            wire::DisplayErrorCode::ProtocolViolation,
            format!("register_display has unknown window flags 0x{window_state_flags:x}"),
        ));
    }
    if presentation_caps.flags & !crate::wallframe::routing::PRESENTATION_CAPS_KNOWN != 0 {
        return Err(reject(
            wire::DisplayErrorCode::ProtocolViolation,
            format!(
                "register_display has unknown presentation capability flags 0x{:x}",
                presentation_caps.flags
            ),
        ));
    }
    let drm = crate::wallframe::renderer_manager::DrmNode {
        major: consumer_caps.drm_render_major,
        minor: consumer_caps.drm_render_minor,
    };
    let consumer_caps = match crate::wallframe::dma::negotiate::unflatten_caps(
        &consumer_caps.fourccs,
        &consumer_caps.mod_counts,
        &consumer_caps.modifiers,
        &consumer_caps.plane_counts,
        &consumer_caps.device_uuid,
        &consumer_caps.driver_uuid,
        drm,
        consumer_caps.sync_caps,
        consumer_caps.color_caps,
        consumer_caps.mem_hints,
        (consumer_caps.extent_max_w, consumer_caps.extent_max_h),
    ) {
        Ok(capabilities) => capabilities,
        Err(error) => {
            return Err(reject(
                wire::DisplayErrorCode::ProtocolViolation,
                format!("malformed consumer capabilities: {error:?}"),
            ));
        }
    };
    let prefix = format!("display {name}: consumer capabilities");
    consumer_caps.log_dump(&prefix);
    log::info!(
        "display register: {name} (instance_id={}) {}x{}@{}mHz drm_render={}:{}",
        instance_id.as_deref().unwrap_or("<none>"),
        metrics.width,
        metrics.height,
        metrics.refresh_mhz,
        drm.major,
        drm.minor,
    );
    Ok(DisplayRegistration {
        name,
        instance_id,
        metrics: DisplayMetrics {
            width: metrics.width,
            height: metrics.height,
            refresh_mhz: metrics.refresh_mhz,
        },
        presentation_caps: presentation_caps.flags,
        consumer_caps,
        window_state_flags,
    })
}

// ---------------------------------------------------------------------------
// Frame loop — translate DisplayOutEvent → wire Event

async fn run_frame_loop(
    stream: StdUnixStream,
    router: Arc<Router>,
    display_id: crate::wallframe::scheduler::DisplayId,
    display_session_id: crate::wallframe::sync::DisplaySessionId,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<DisplayOutEvent>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    // Spawn the blocking reader half (client→server requests).
    let read_stream = stream.try_clone().context("clone for reader")?;
    let (req_tx, mut req_rx) =
        tokio::sync::mpsc::unbounded_channel::<codec::CodecResult<Request>>();
    let reader_handle = tokio::task::spawn_blocking(move || loop {
        let res = codec::recv_request(&read_stream);
        let is_err = res.is_err();
        let _ = req_tx.send(res.map(|(r, _fds)| r));
        if is_err {
            return;
        }
    });

    // Most-recently-bound renderer, kept so inbound pointer events can
    // be forwarded without a routing-table walk.
    let mut bound_renderer: Option<Arc<RendererHandle>> = None;
    // Latest SetCompositionConfig pushed to this display. Used to inverse-map
    // pointer coords from display pixels into renderer texture pixels.
    let mut latest_config: Option<CompositionConfig> = None;
    let mut pending_arms = HashMap::<(u64, u64), crate::wallframe::sync::FrameConsumerArm>::new();
    let mut release_sessions =
        HashMap::<String, crate::wallframe::sync::FrameConsumerSession>::new();

    let result = loop {
        tokio::select! {
            _ = wait_shutdown(&mut shutdown_rx) => {
                log::info!("display {display_id}: shutdown signalled");
                break Ok(());
            }
            evt = rx.recv() => match evt {
                None => {
                    log::info!("display {display_id}: router rx closed");
                    break Ok(());
                }
                Some(DisplayOutEvent::Bind {
                    renderer,
                    pool,
                    buffer_generation,
                    initial_config,
                    transition,
                }) => {
                    bound_renderer = Some(Arc::clone(&renderer));
                    latest_config = Some(initial_config.clone());
                    if let Err(e) = send_bind(
                        &stream,
                        &pool,
                        buffer_generation,
                        &initial_config,
                        transition,
                    ).await {
                        break Err(e);
                    }
                }
                Some(DisplayOutEvent::Unbind { buffer_generation }) => {
                    bound_renderer = None;
                    latest_config = None;
                    if let Err(e) = send_unbind(&stream, buffer_generation).await {
                        break Err(e);
                    }
                }
                Some(DisplayOutEvent::SetCompositionConfig(cfg)) => {
                    latest_config = Some(cfg.clone());
                    if let Err(e) = send_composition_config(&stream, &cfg).await {
                        break Err(e);
                    }
                }
                Some(DisplayOutEvent::SetPresentationSnapshot(presentation)) => {
                    if let Err(e) = send_presentation_snapshot(&stream, presentation).await {
                        break Err(e);
                    }
                }
                Some(DisplayOutEvent::SetPresentationState(state)) => {
                    if let Err(e) = send_presentation_state(&stream, state).await {
                        break Err(e);
                    }
                }
                Some(DisplayOutEvent::Frame {
                    renderer, buffer_generation, buffer_index, seq,
                    consumption, member,
                }) => {
                    match forward_frame_ready(
                        &stream, &renderer, buffer_generation, buffer_index, seq,
                        consumption, member, true,
                    ).await {
                        Ok(Some(forwarded)) => {
                            release_sessions
                                .entry(forwarded.session.renderer_id().to_string())
                                .or_insert(forwarded.session);
                            if let Some(arm) = forwarded.arm {
                                pending_arms.insert((buffer_generation, seq), arm);
                            }
                        }
                        Ok(None) => {}
                        Err(e) => break Err(e),
                    }
                }
            },
            maybe_req = req_rx.recv() => match maybe_req {
                Some(Ok(Request::SetDisplayMetrics { metrics })) => {
                    if metrics.width == 0 || metrics.height == 0 {
                        let message = format!(
                            "invalid display metrics {}x{}",
                            metrics.width, metrics.height,
                        );
                        let _ = send_error(
                            &stream,
                            wire::DisplayErrorCode::ProtocolViolation,
                            message.clone(),
                        ).await;
                        break Err(Error::Internal(anyhow!(message)));
                    }
                    router
                        .set_display_metrics(
                            display_id,
                            DisplayMetrics {
                                width: metrics.width,
                                height: metrics.height,
                                refresh_mhz: metrics.refresh_mhz,
                            },
                        )
                        .await;
                    log::info!(
                        "display {display_id}: metrics {}x{}@{}mHz",
                        metrics.width,
                        metrics.height,
                        metrics.refresh_mhz,
                    );
                }
                Some(Ok(Request::SetWindowState { flags })) => {
                    if flags & !crate::wallframe::routing::auto_replay::FLAGS_KNOWN != 0 {
                        let message = format!("unknown window state flags 0x{flags:x}");
                        let _ = send_error(
                            &stream,
                            wire::DisplayErrorCode::ProtocolViolation,
                            message.clone(),
                        ).await;
                        break Err(Error::Internal(anyhow!(message)));
                    }
                    log::debug!(
                        "display {display_id}: window state flags=0x{flags:x}"
                    );
                    router.update_display_window_state(display_id, flags).await;
                }
                Some(Ok(Request::FrameReleaseArmed { buffer_generation, seq })) => {
                    match pending_arms.remove(&(buffer_generation, seq)) {
                        Some(arm) => arm.arm(),
                        None => log::warn!(
                            "display {display_id} session {display_session_id}: stale or unknown frame_release_armed gen={buffer_generation} seq={seq}"
                        ),
                    }
                }
                Some(Ok(Request::BufferImportFailed {
                    buffer_generation,
                    kind,
                    message,
                })) => {
                    let domain_kind = match kind {
                        wire::BufferImportFailureKind::Unsupported => {
                            ConsumerImportFailureKind::Unsupported
                        }
                        wire::BufferImportFailureKind::ResourceExhausted => {
                            ConsumerImportFailureKind::ResourceExhausted
                        }
                        wire::BufferImportFailureKind::BackendFailure => {
                            ConsumerImportFailureKind::BackendFailure
                        }
                    };
                    match router
                        .on_consumer_import_failed(display_id, buffer_generation, domain_kind)
                        .await
                    {
                        ConsumerImportFailureOutcome::Retry { fourcc, modifier } => {
                            log::warn!(
                                "display {display_id}: import failed gen={buffer_generation} \
                                 fourcc=0x{fourcc:08x} modifier=0x{modifier:x}: {message}"
                            );
                        }
                        ConsumerImportFailureOutcome::Stale => {
                            log::warn!(
                                "display {display_id}: stale import failure gen={buffer_generation}: {message}"
                            );
                        }
                        ConsumerImportFailureOutcome::Terminal => {
                            let detail = format!(
                                "display backend import failed for generation {buffer_generation}: {message}"
                            );
                            let _ = send_error(
                                &stream,
                                wire::DisplayErrorCode::NegotiationFailed,
                                detail.clone(),
                            ).await;
                            break Err(Error::Internal(anyhow!(detail)));
                        }
                    }
                }
                Some(Ok(Request::AckUnbind { buffer_generation })) => {
                    log::debug!(
                        "display {display_id}: ack_unbind gen={buffer_generation}"
                    );
                    router.record_ack_unbind(display_id, buffer_generation).await;
                }
                Some(Ok(Request::PointerMotion { x, y, timestamp_us, modifiers })) => {
                    if !pointer_values_valid(&[x, y], modifiers) {
                        let message = "invalid pointer_motion values".to_string();
                        let _ = send_error(
                            &stream,
                            wire::DisplayErrorCode::ProtocolViolation,
                            message.clone(),
                        ).await;
                        break Err(Error::Internal(anyhow!(message)));
                    }
                    if let (Some(r), Some(cfg)) = (bound_renderer.as_ref(), latest_config.as_ref()) {
                        if let Some((tx, ty)) = display_point_to_texture(x, y, cfg) {
                            // Pointer forwarding gates on the renderer's
                            // manifest events list.
                            if let Err(e) = router
                                .forward_pointer_motion(
                                    &r.id,
                                    RendererPointerMotion {
                                        x: tx,
                                        y: ty,
                                        timestamp_us,
                                        modifiers,
                                    },
                                ).await
                            {
                                log::debug!("display {display_id}: pointer_motion forward failed: {e}");
                            }
                        }
                    }
                }
                Some(Ok(Request::PointerButton { x, y, button, state, timestamp_us, modifiers })) => {
                    if !pointer_values_valid(&[x, y], modifiers) {
                        let message = "invalid pointer_button values".to_string();
                        let _ = send_error(
                            &stream,
                            wire::DisplayErrorCode::ProtocolViolation,
                            message.clone(),
                        ).await;
                        break Err(Error::Internal(anyhow!(message)));
                    }
                    if let (Some(r), Some(cfg)) = (bound_renderer.as_ref(), latest_config.as_ref()) {
                        if let Some((tx, ty)) = display_point_to_texture(x, y, cfg) {
                            if let Err(e) = router
                                .forward_pointer_button(
                                    &r.id,
                                    RendererPointerButton {
                                        x: tx,
                                        y: ty,
                                        button,
                                        state: match state {
                                            wire::PointerButtonState::Released => {
                                                RendererPointerButtonState::Released
                                            }
                                            wire::PointerButtonState::Pressed => {
                                                RendererPointerButtonState::Pressed
                                            }
                                        },
                                        timestamp_us,
                                        modifiers,
                                    },
                                ).await
                            {
                                log::debug!("display {display_id}: pointer_button forward failed: {e}");
                            }
                        }
                    }
                }
                Some(Ok(Request::PointerAxis { x, y, delta_x, delta_y, source, timestamp_us, modifiers })) => {
                    if !pointer_values_valid(&[x, y, delta_x, delta_y], modifiers) {
                        let message = "invalid pointer_axis values".to_string();
                        let _ = send_error(
                            &stream,
                            wire::DisplayErrorCode::ProtocolViolation,
                            message.clone(),
                        ).await;
                        break Err(Error::Internal(anyhow!(message)));
                    }
                    if let (Some(r), Some(cfg)) = (bound_renderer.as_ref(), latest_config.as_ref()) {
                        if let Some((tx, ty)) = display_point_to_texture(x, y, cfg) {
                            // delta_x/delta_y are scroll quantities, not
                            // spatial; forward unchanged.
                            if let Err(e) = router
                                .forward_pointer_axis(
                                    &r.id,
                                    RendererPointerAxis {
                                        x: tx,
                                        y: ty,
                                        delta_x,
                                        delta_y,
                                        source: match source {
                                            wire::PointerAxisSource::Wheel => {
                                                RendererPointerAxisSource::Wheel
                                            }
                                            wire::PointerAxisSource::Finger => {
                                                RendererPointerAxisSource::Finger
                                            }
                                            wire::PointerAxisSource::Continuous => {
                                                RendererPointerAxisSource::Continuous
                                            }
                                        },
                                        timestamp_us,
                                        modifiers,
                                    },
                                ).await
                            {
                                log::debug!("display {display_id}: pointer_axis forward failed: {e}");
                            }
                        }
                    }
                }
                Some(Ok(other)) => {
                    let message = format!(
                        "unexpected post-handshake request opcode {}",
                        other.opcode()
                    );
                    let _ = send_error(
                        &stream,
                        wire::DisplayErrorCode::ProtocolViolation,
                        message.clone(),
                    ).await;
                    break Err(Error::Internal(anyhow!(message)));
                }
                Some(Err(e)) => {
                    log::info!("display {display_id}: client recv error: {e}");
                    break Ok(());
                }
                None => {
                    log::info!("display {display_id}: reader task ended");
                    break Ok(());
                }
            },
        }
    };
    // Force the blocking reader out of its parked `recvmsg`. `shutdown`
    // operates on the socket itself, so all dup'd handles observe it.
    let _ = stream.shutdown(std::net::Shutdown::Both);
    let _ = reader_handle.await;
    pending_arms.clear();
    for (_, session) in release_sessions {
        session.close();
    }
    result
}

fn pointer_values_valid(values: &[f32], modifiers: u32) -> bool {
    values.iter().all(|value| value.is_finite()) && modifiers & !0x0f == 0
}

const DISPLAY_UPDATE_HINT: &str = "You may need to update waywallen-display.";

fn report_connection_failure(
    events_tx: &tokio::sync::broadcast::Sender<GlobalEvent>,
    client_name: String,
    client_protocol_version: u32,
    error_code: wire::DisplayErrorCode,
    reason: String,
) {
    let _ = events_tx.send(GlobalEvent::DisplayConnectionFailed {
        client_name,
        client_protocol_version,
        error_code: error_code as u32,
        reason: format!("{reason}. {DISPLAY_UPDATE_HINT}"),
    });
}

#[derive(Debug, thiserror::Error)]
enum ReceiveRequestError {
    #[error("{context}: {source}")]
    Clone {
        context: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("recv: {0}")]
    Codec(#[source] codec::CodecError),
    #[error("recv join: {0}")]
    Join(#[source] tokio::task::JoinError),
    #[error("shutdown during recv")]
    Shutdown,
}

impl ReceiveRequestError {
    fn is_protocol_failure(&self) -> bool {
        matches!(
            self,
            Self::Codec(
                codec::CodecError::FrameTooLarge(_)
                    | codec::CodecError::BadFrameLen(_)
                    | codec::CodecError::TooManyFds(_)
                    | codec::CodecError::FdCountMismatch { .. }
                    | codec::CodecError::Decode(_)
            )
        )
    }
}

/// Run `codec::recv_request` on the blocking pool but tear down the
/// wait if `shutdown_rx` flips to `true`.
async fn recv_request_cancellable(
    stream: &StdUnixStream,
    shutdown_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> std::result::Result<(Request, Vec<OwnedFd>), ReceiveRequestError> {
    let blocking_stream = stream
        .try_clone()
        .map_err(|source| ReceiveRequestError::Clone {
            context: "clone for recv",
            source,
        })?;
    let shutdown_stream = stream
        .try_clone()
        .map_err(|source| ReceiveRequestError::Clone {
            context: "clone for shutdown-kick",
            source,
        })?;
    let mut handle = tokio::task::spawn_blocking(move || codec::recv_request(&blocking_stream));
    tokio::select! {
        biased;
        res = &mut handle => match res {
            Ok(r) => r.map_err(ReceiveRequestError::Codec),
            Err(e) => Err(ReceiveRequestError::Join(e)),
        },
        _ = wait_shutdown(shutdown_rx) => {
            let _ = shutdown_stream.shutdown(std::net::Shutdown::Both);
            let _ = handle.await;
            Err(ReceiveRequestError::Shutdown)
        }
    }
}

/// Resolve once the daemon flips the shutdown flag.
async fn wait_shutdown(rx: &mut tokio::sync::watch::Receiver<bool>) {
    let _ = rx.wait_for(|v| *v).await;
}

// ---------------------------------------------------------------------------
// Wire-event senders

fn presentation_to_wire(snapshot: PresentationSnapshot) -> wire::PresentationSnapshot {
    let kind = match snapshot.config.pause_effect.kind {
        crate::settings::PauseEffectKind::None => wire::PauseEffectKind::None,
        crate::settings::PauseEffectKind::Blur => wire::PauseEffectKind::Blur,
    };
    let transition = snapshot.config.transition;
    let transition_kind = match transition.kind {
        crate::settings::TransitionKind::None => wire::TransitionKind::None,
        crate::settings::TransitionKind::Fade => wire::TransitionKind::Fade,
        crate::settings::TransitionKind::Wipe => wire::TransitionKind::Wipe,
        crate::settings::TransitionKind::Grow => wire::TransitionKind::Grow,
    };
    wire::PresentationSnapshot {
        config: wire::PresentationConfig {
            generation: snapshot.config.generation,
            pause_effect: wire::PauseEffectConfig {
                kind,
                blur: wire::BlurEffectConfig {
                    radius: snapshot.config.pause_effect.blur.radius,
                },
            },
            transition: wire::TransitionConfig {
                kind: transition_kind,
                duration_ms: transition.duration_ms,
                angle: transition.angle,
                origin_x: transition.origin.x as f32 / 100.0,
                origin_y: transition.origin.y as f32 / 100.0,
            },
        },
        state: presentation_state_to_wire(snapshot.state),
    }
}

fn presentation_state_to_wire(config: PresentationState) -> wire::PresentationState {
    wire::PresentationState {
        generation: config.generation,
        config_generation: config.config_generation,
        pause_effect: wire::PauseEffectState {
            active: config.pause_effect.active,
        },
    }
}

fn composition_to_wire(config: &CompositionConfig) -> wire::CompositionConfig {
    wire::CompositionConfig {
        generation: config.generation,
        buffer_generation: config.buffer_generation,
        source_rect: Rect {
            x: config.source_x,
            y: config.source_y,
            w: config.source_w,
            h: config.source_h,
        },
        dest_rect: Rect {
            x: config.dest_x,
            y: config.dest_y,
            w: config.dest_w,
            h: config.dest_h,
        },
        transform: config.transform,
        clear_color: wire::RgbaColor {
            r: config.clear_rgba[0],
            g: config.clear_rgba[1],
            b: config.clear_rgba[2],
            a: config.clear_rgba[3],
        },
    }
}

async fn send_error(
    stream: &StdUnixStream,
    code: wire::DisplayErrorCode,
    message: String,
) -> Result<()> {
    let event = Event::Error { code, message };
    let stream = stream.try_clone().context("clone for error")?;
    tokio::task::spawn_blocking(move || codec::send_event(&stream, &event, &[]))
        .await
        .context("error join")?
        .map_err(|error| Error::Internal(anyhow!("send error: {error}")))?;
    Ok(())
}

async fn send_presentation_snapshot(
    stream: &StdUnixStream,
    presentation: PresentationSnapshot,
) -> Result<()> {
    let evt = Event::SetPresentationSnapshot {
        presentation: presentation_to_wire(presentation),
    };
    let s = stream
        .try_clone()
        .context("clone for set_presentation_snapshot")?;
    tokio::task::spawn_blocking(move || codec::send_event(&s, &evt, &[]))
        .await
        .context("set_presentation_snapshot join")?
        .map_err(|e| Error::Internal(anyhow!("send set_presentation_snapshot: {e}")))?;
    Ok(())
}

async fn send_presentation_state(stream: &StdUnixStream, state: PresentationState) -> Result<()> {
    let evt = Event::SetPresentationState {
        state: presentation_state_to_wire(state),
    };
    let s = stream
        .try_clone()
        .context("clone for set_presentation_state")?;
    tokio::task::spawn_blocking(move || codec::send_event(&s, &evt, &[]))
        .await
        .context("set_presentation_state join")?
        .map_err(|e| Error::Internal(anyhow!("send set_presentation_state: {e}")))?;
    Ok(())
}

async fn send_unbind(stream: &StdUnixStream, buffer_generation: u64) -> Result<()> {
    let evt = Event::Unbind { buffer_generation };
    let s = stream.try_clone().context("clone for unbind")?;
    tokio::task::spawn_blocking(move || codec::send_event(&s, &evt, &[]))
        .await
        .context("unbind join")?
        .map_err(|e| Error::Internal(anyhow!("send unbind: {e}")))?;
    Ok(())
}

async fn send_composition_config(stream: &StdUnixStream, cfg: &CompositionConfig) -> Result<()> {
    let evt = Event::SetCompositionConfig {
        config: composition_to_wire(cfg),
    };
    let s = stream
        .try_clone()
        .context("clone for set_composition_config")?;
    tokio::task::spawn_blocking(move || codec::send_event(&s, &evt, &[]))
        .await
        .context("set_composition_config join")?
        .map_err(|e| Error::Internal(anyhow!("send set_composition_config: {e}")))?;
    Ok(())
}

async fn send_bind(
    stream: &StdUnixStream,
    pool: &PublishedPool,
    buffer_generation: u64,
    initial_config: &CompositionConfig,
    transition: bool,
) -> Result<()> {
    let (event, dup_fds) = build_bind_event(pool, buffer_generation, initial_config, transition)?;
    let s = stream.try_clone().context("clone for bind")?;
    let event_for_send = event.clone();
    let dup_for_send = dup_fds.clone();
    tokio::task::spawn_blocking(move || {
        let result = codec::send_event(&s, &event_for_send, &dup_for_send);
        for fd in dup_for_send {
            unsafe { libc::close(fd) };
        }
        result
    })
    .await
    .context("bind send join")?
    .map_err(|e| Error::Internal(anyhow!("send bind_buffers: {e}")))?;
    Ok(())
}

/// Translate one immutable renderer publication into a `BindBuffers`
/// event. Both schemas use flattened parallel arrays per plane.
fn build_bind_event(
    pool: &PublishedPool,
    buffer_generation: u64,
    initial_config: &CompositionConfig,
    transition: bool,
) -> Result<(Event, Vec<RawFd>)> {
    if initial_config.buffer_generation != buffer_generation {
        return Err(Error::Internal(anyhow!(
            "initial composition generation {} targets buffer generation {}, expected {}",
            initial_config.generation,
            initial_config.buffer_generation,
            buffer_generation,
        )));
    }
    let count = pool.count;
    let planes_per_buffer = pool.planes_per_buffer;
    let n = (count as usize) * (planes_per_buffer as usize);

    if pool.stride.len() != n
        || pool.plane_offset.len() != n
        || pool.size.len() != n
        || pool.fds.len() != n
    {
        return Err(Error::Internal(anyhow!(
            "PublishedPool parallel arrays inconsistent: count={} planes={} expected={} \
             stride={} offset={} size={} fds={}",
            count,
            planes_per_buffer,
            n,
            pool.stride.len(),
            pool.plane_offset.len(),
            pool.size.len(),
            pool.fds.len()
        )));
    }

    let mut dup_fds: Vec<RawFd> = Vec::with_capacity(n);
    for fd in &pool.fds {
        let raw = nix::unistd::dup(fd.as_raw_fd())
            .map_err(|e| Error::Internal(anyhow!("dup dma-buf fd: {e}")))?;
        dup_fds.push(raw);
    }

    let event = Event::BindBuffers {
        buffer_generation,
        count,
        width: pool.width,
        height: pool.height,
        fourcc: pool.fourcc,
        modifier: pool.modifier,
        planes_per_buffer,
        stride: pool.stride.clone(),
        plane_offset: pool.plane_offset.clone(),
        size: pool.size.clone(),
        initial_config: composition_to_wire(initial_config),
        transition,
    };
    log::debug!(
        "display::endpoint: build_bind_event gen={} count={} planes={} {}x{} \
         fourcc=0x{:08x} mod=0x{:016x} transition={}",
        buffer_generation,
        count,
        planes_per_buffer,
        pool.width,
        pool.height,
        pool.fourcc,
        pool.modifier,
        transition,
    );
    for i in 0..n {
        let bi = i / (planes_per_buffer as usize).max(1);
        let pi = i % (planes_per_buffer as usize).max(1);
        log::debug!(
            "  buf[{}].plane[{}] dup_fd={} stride={} plane_offset={} size={}",
            bi,
            pi,
            dup_fds[i],
            pool.stride[i],
            pool.plane_offset[i],
            pool.size[i],
        );
    }
    Ok((event, dup_fds))
}

// ---------------------------------------------------------------------------
// Frame forwarding (with sync fence)

fn acquire_sync_fd(renderer: &Arc<RendererHandle>, seq: u64) -> Result<OwnedFd> {
    renderer.clone_sync_fd(seq).ok_or_else(|| {
        Error::Internal(anyhow!(
            "acquire sync_fd for seq={seq} missing (evicted or never arrived)"
        ))
    })
}

async fn forward_frame_ready(
    stream: &StdUnixStream,
    renderer: &Arc<RendererHandle>,
    buffer_generation: u64,
    buffer_index: u32,
    seq: u64,
    consumption: DisplayConsumptionPermit,
    member: Option<crate::wallframe::sync::FrameConsumerMember>,
    requires_arm: bool,
) -> Result<Option<ForwardedFrame>> {
    if !consumption.is_current() {
        if let Some(member) = member {
            member.skip();
        }
        return Ok(None);
    }
    let fence = acquire_sync_fd(renderer, seq)?;
    // Allocate a fresh BINARY drm_syncobj for this consumer and frame.
    // The handle stays in the daemon and is handed off to the reaper.
    let dev = drm_device().context("open DRM render node for release_syncobj")?;
    let consumer_handle = dev
        .create_binary_syncobj()
        .context("create binary release_syncobj")?;
    let release_fd = dev
        .handle_to_fd(&consumer_handle)
        .context("export release_syncobj fd")?;

    let fence_raw = fence.as_raw_fd();
    let release_raw = release_fd.as_raw_fd();
    let send_stream = stream.try_clone().context("clone for frame_ready")?;
    let evt = Event::FrameReady {
        buffer_generation,
        buffer_index,
        seq,
    };
    let send_result = tokio::task::spawn_blocking(move || {
        codec::send_event(&send_stream, &evt, &[fence_raw, release_raw])
    })
    .await
    .context("frame_ready send join")?;
    drop(fence);
    drop(release_fd);
    send_result.map_err(|e| Error::Internal(anyhow!("send frame_ready: {e}")))?;

    let forwarded = member.map(|member| {
        let session = member.session();
        let arm = member.delivered(consumer_handle, requires_arm);
        ForwardedFrame { arm, session }
    });
    Ok(forwarded)
}

struct ForwardedFrame {
    arm: Option<crate::wallframe::sync::FrameConsumerArm>,
    session: crate::wallframe::sync::FrameConsumerSession,
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn malformed_hello_publishes_connection_failure() {
        use std::io::Write;

        let (server, mut client) = StdUnixStream::pair().unwrap();
        let (events_tx, mut events_rx) = tokio::sync::broadcast::channel(4);
        let (_shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

        let hello = Request::Hello {
            client_name: "legacy-display".to_string(),
            client_version: "0.2.0".to_string(),
            protocol_version: PROTOCOL_VERSION,
        };
        let mut body = Vec::new();
        hello.encode(&mut body);
        body.extend_from_slice(&0_u32.to_le_bytes());

        let total = u16::try_from(body.len() + 4).unwrap();
        let mut frame = Vec::with_capacity(total as usize);
        frame.extend_from_slice(&hello.opcode().to_le_bytes());
        frame.extend_from_slice(&total.to_le_bytes());
        frame.extend_from_slice(&body);
        client.write_all(&frame).unwrap();

        let error = match do_handshake(&server, &events_tx, &mut shutdown_rx).await {
            Ok(_) => panic!("malformed hello unexpectedly completed the handshake"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("trailing bytes after decode"));

        match events_rx.try_recv().unwrap() {
            GlobalEvent::DisplayConnectionFailed {
                client_name,
                client_protocol_version,
                error_code,
                reason,
            } => {
                assert!(client_name.is_empty());
                assert_eq!(client_protocol_version, 0);
                assert_eq!(error_code, wire::DisplayErrorCode::ProtocolViolation as u32);
                assert!(reason.contains("incompatible or malformed display hello"));
                assert!(reason.contains("trailing bytes after decode"));
                assert!(reason.contains(DISPLAY_UPDATE_HINT));
            }
            event => panic!("unexpected event: {event:?}"),
        }

        match codec::recv_event(&client) {
            Ok((
                Event::Error {
                    code: wire::DisplayErrorCode::ProtocolViolation,
                    message,
                },
                _,
            )) => {
                assert!(message.contains("incompatible or malformed display hello"));
                assert!(message.contains(&format!(
                    "[{MIN_SUPPORTED_CLIENT_VERSION}..={MAX_SUPPORTED_CLIENT_VERSION}]"
                )));
            }
            Ok((event, _)) => panic!("expected error event, got opcode {}", event.opcode()),
            Err(error) => panic!("client got no answer at all: {error}"),
        }
    }

    #[test]
    fn build_bind_event_uses_display_generation() {
        use nix::sys::memfd::{memfd_create, MemFdCreateFlag};
        use std::ffi::CString;
        use std::os::fd::FromRawFd;
        let name = CString::new("waywallen-display-endpoint-test").unwrap();
        let fd1 = memfd_create(&name, MemFdCreateFlag::MFD_CLOEXEC).unwrap();
        let fd2 = memfd_create(&name, MemFdCreateFlag::MFD_CLOEXEC).unwrap();

        let pool = PublishedPool {
            generation: 7,
            flags: 0,
            count: 2,
            fourcc: 0x34325258,
            width: 800,
            height: 600,
            modifier: 0,
            planes_per_buffer: 1,
            stride: vec![3200, 3200],
            plane_offset: vec![0, 0],
            size: vec![1_920_000, 1_920_000],
            fds: vec![fd1, fd2],
        };

        let config = CompositionConfig {
            generation: 3,
            buffer_generation: 11,
            display_w: 800.0,
            display_h: 600.0,
            source_x: 0.0,
            source_y: 0.0,
            source_w: 800.0,
            source_h: 600.0,
            dest_x: 0.0,
            dest_y: 0.0,
            dest_w: 800.0,
            dest_h: 600.0,
            transform: 0,
            clear_rgba: [0.0, 0.0, 0.0, 1.0],
        };
        let (event, dup_fds) = build_bind_event(&pool, 11, &config, false).unwrap();
        assert_eq!(dup_fds.len(), 2);
        match event {
            Event::BindBuffers {
                buffer_generation,
                count,
                width,
                height,
                fourcc,
                modifier,
                planes_per_buffer,
                stride,
                plane_offset,
                size,
                initial_config,
                transition,
            } => {
                assert_eq!(buffer_generation, 11);
                assert!(!transition);
                assert_eq!(count, 2);
                assert_eq!(width, 800);
                assert_eq!(height, 600);
                assert_eq!(fourcc, 0x34325258);
                assert_eq!(modifier, 0);
                assert_eq!(planes_per_buffer, 1);
                assert_eq!(stride, vec![3200, 3200]);
                assert_eq!(plane_offset, vec![0, 0]);
                assert_eq!(size, vec![1_920_000, 1_920_000]);
                assert_eq!(initial_config.generation, 3);
                assert_eq!(initial_config.buffer_generation, 11);
            }
            _ => panic!("expected BindBuffers"),
        }
        for raw in dup_fds {
            let _ = unsafe { std::fs::File::from_raw_fd(raw) };
        }
    }
}

#[allow(dead_code)]
const _OPCODE_MOD_KEEPALIVE: fn() = || {
    let _ = opcode::request::HELLO;
};
