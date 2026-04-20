//! waywallen-display-layer-shell — Wayland layer-shell wallpaper client.
 
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_compositor::WlCompositor, wl_output::WlOutput,
    wl_registry::WlRegistry, wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1},
    zwp_linux_dmabuf_v1::{self, ZwpLinuxDmabufV1},
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

use waywallen::display_proto::{codec, Event as ProtoEvent, Request as ProtoRequest, PROTOCOL_NAME};

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

struct Args {
    socket: PathBuf,
    name: String,
}

fn usage() -> ! {
    eprintln!(
        "usage: waywallen-display-layer-shell [--socket PATH] [--name STR]\n\
         \n\
         Environment:\n\
           WAYWALLEN_SOCKET   fallback UDS path when --socket is omitted\n\
           WAYLAND_DISPLAY    required — picks the compositor to attach to"
    );
    std::process::exit(2);
}

fn parse_args() -> Args {
    let mut socket: Option<PathBuf> = None;
    let mut name = String::from("waywallen-layer-shell");
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--socket" => {
                socket = it.next().map(PathBuf::from);
                if socket.is_none() {
                    eprintln!("--socket requires a value");
                    usage();
                }
            }
            "--name" => {
                name = it.next().unwrap_or_else(|| {
                    eprintln!("--name requires a value");
                    usage();
                });
            }
            "-h" | "--help" => usage(),
            other => {
                eprintln!("unknown argument: {other}");
                usage();
            }
        }
    }
    let socket = socket
        .or_else(|| std::env::var_os("WAYWALLEN_SOCKET").map(PathBuf::from))
        .unwrap_or_else(default_socket_path);
    Args { socket, name }
}

fn default_socket_path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    runtime.join("waywallen").join("display.sock")
}

// ---------------------------------------------------------------------------
// Wayland state machine
// ---------------------------------------------------------------------------

/// Shared surface + protocol proxies the UDS worker needs to attach
/// frames. All proxies in wayland-client 0.31 are `Send + Sync`, so the
/// worker thread can call into them freely; request writes are
/// serialized through the shared `Connection` and flushed explicitly.
struct OutputBinding {
    surface: WlSurface,
    dmabuf: ZwpLinuxDmabufV1,
    conn: Connection,
    /// Size assigned by the compositor's first `configure` event. The
    /// worker waits until this is populated before starting the UDS
    /// session — the layer-surface is not renderable before configure.
    configured_size: Mutex<Option<(u32, u32)>>,
}

/// Everything the main thread tracks while driving the Wayland event
/// queue. Only written from the main thread's dispatch callbacks; the
/// worker holds an `Arc<OutputBinding>` instead.
struct App {
    compositor: Option<WlCompositor>,
    layer_shell: Option<ZwlrLayerShellV1>,
    dmabuf: Option<ZwpLinuxDmabufV1>,
    output: Option<WlOutput>,
    surface: Option<WlSurface>,
    layer_surface: Option<ZwlrLayerSurfaceV1>,
    binding: Option<Arc<OutputBinding>>,
    worker_started: bool,
    uds_sock: PathBuf,
    display_name: String,
}

impl App {
    fn new(uds_sock: PathBuf, display_name: String) -> Self {
        Self {
            compositor: None,
            layer_shell: None,
            dmabuf: None,
            output: None,
            surface: None,
            layer_surface: None,
            binding: None,
            worker_started: false,
            uds_sock,
            display_name,
        }
    }

    /// Called once we have `WlCompositor`, `ZwlrLayerShellV1` and at
    /// least one `WlOutput`. Creates the `wl_surface` + layer_surface,
    /// anchors them BG full-screen, and commits so the compositor
    /// sends back a `configure` with the actual size.
    fn bring_up_surface(&mut self, qh: &QueueHandle<App>, conn: &Connection) {
        if self.surface.is_some() {
            return;
        }
        let (Some(comp), Some(shell), Some(output)) = (
            self.compositor.as_ref(),
            self.layer_shell.as_ref(),
            self.output.as_ref(),
        ) else {
            return;
        };
        let surface = comp.create_surface(qh, ());
        let layer_surface = shell.get_layer_surface(
            &surface,
            Some(output),
            Layer::Background,
            "waywallen-wallpaper".to_string(),
            qh,
            (),
        );
        layer_surface.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
        layer_surface.set_exclusive_zone(-1);
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
        // Size (0, 0) asks the compositor to pick — it'll tell us via
        // the first configure event.
        layer_surface.set_size(0, 0);
        surface.commit();
        self.surface = Some(surface);
        self.layer_surface = Some(layer_surface);
        log::info!("layer_surface committed, waiting for configure");
        let _ = conn;
    }

    /// Worker hand-off: once the compositor has configured the surface,
    /// spawn the UDS session thread with a shared `OutputBinding`.
    fn maybe_spawn_worker(&mut self, conn: &Connection) {
        if self.worker_started {
            return;
        }
        let (Some(surface), Some(dmabuf), Some(binding)) =
            (self.surface.as_ref(), self.dmabuf.as_ref(), self.binding.as_ref())
        else {
            return;
        };
        if binding.configured_size.lock().unwrap().is_none() {
            return;
        }
        self.worker_started = true;
        let binding = Arc::clone(binding);
        let sock = self.uds_sock.clone();
        let name = self.display_name.clone();
        log::info!("spawning UDS worker for configured surface");
        thread::spawn(move || uds_worker_loop(sock, name, binding));
        let _ = (surface, dmabuf, conn);
    }
}

// --- Dispatch impls -------------------------------------------------------

impl Dispatch<WlRegistry, GlobalListContents> for App {
    fn event(
        _state: &mut Self,
        _registry: &WlRegistry,
        _event: wayland_client::protocol::wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // Globals are taken once at startup via registry_queue_init;
        // runtime add/remove is ignored in this pass.
    }
}

impl Dispatch<WlCompositor, ()> for App {
    fn event(
        _state: &mut Self,
        _p: &WlCompositor,
        _e: wayland_client::protocol::wl_compositor::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSurface, ()> for App {
    fn event(
        _state: &mut Self,
        _p: &WlSurface,
        _e: wayland_client::protocol::wl_surface::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlBuffer, ()> for App {
    fn event(
        _state: &mut Self,
        buffer: &WlBuffer,
        event: wayland_client::protocol::wl_buffer::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_buffer::Event;
        if let Event::Release = event {
            // Compositor released the buffer; the worker learns about
            // it indirectly via daemon-driven FrameReady pacing. In a
            // stricter implementation we'd wire release back into the
            // worker so it knows when to ack BufferRelease — the daemon
            // already expects per-frame BufferRelease, so this is
            // informational.
            log::trace!("wl_buffer {} released", buffer.id());
        }
    }
}

impl Dispatch<WlOutput, ()> for App {
    fn event(
        state: &mut Self,
        _p: &WlOutput,
        _e: wayland_client::protocol::wl_output::Event,
        _data: &(),
        conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        // Output metadata events (Geometry/Mode/Done) stream in after
        // bind. We don't need the dimensions — layer-surface configure
        // carries its own size — but the first "Done" is a convenient
        // moment to bring up the surface.
        state.bring_up_surface(qh, conn);
    }
}

impl Dispatch<ZwlrLayerShellV1, ()> for App {
    fn event(
        _state: &mut Self,
        _p: &ZwlrLayerShellV1,
        _e: zwlr_layer_shell_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for App {
    fn event(
        state: &mut Self,
        layer_surface: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _data: &(),
        conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer_surface.ack_configure(serial);
                log::info!("layer_surface configure: {width}x{height}");
                // Stash the size in the binding so the worker can start.
                let binding = state.binding.get_or_insert_with(|| {
                    let surface = state
                        .surface
                        .clone()
                        .expect("configure before surface created");
                    let dmabuf = state
                        .dmabuf
                        .clone()
                        .expect("configure before dmabuf bind");
                    Arc::new(OutputBinding {
                        surface,
                        dmabuf,
                        conn: conn.clone(),
                        configured_size: Mutex::new(None),
                    })
                });
                *binding.configured_size.lock().unwrap() = Some((width, height));
                state.maybe_spawn_worker(conn);
            }
            zwlr_layer_surface_v1::Event::Closed => {
                log::warn!("layer_surface closed by compositor; exiting");
                std::process::exit(0);
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpLinuxDmabufV1, ()> for App {
    fn event(
        _state: &mut Self,
        _p: &ZwpLinuxDmabufV1,
        _e: zwp_linux_dmabuf_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // v3 emits `modifier` events listing supported formats; we
        // trust the daemon's fourcc/modifier and use `create_immed`
        // regardless, so these are informational.
    }
}

impl Dispatch<ZwpLinuxBufferParamsV1, ()> for App {
    fn event(
        _state: &mut Self,
        _p: &ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // `create_immed` sidesteps the async `Created`/`Failed` path,
        // but compositors may still send `Failed` if the import is
        // invalid — log it for debugging.
        if let zwp_linux_buffer_params_v1::Event::Failed = event {
            log::error!("zwp_linux_buffer_params_v1 Failed: dmabuf import rejected");
        }
    }
}

// ---------------------------------------------------------------------------
// UDS worker — runs in its own thread, owns the daemon UnixStream,
// creates wl_buffers on BindBuffers, attaches on FrameReady.
// ---------------------------------------------------------------------------

fn uds_worker_loop(sock: PathBuf, name: String, binding: Arc<OutputBinding>) {
    loop {
        match run_uds_session(&sock, &name, &binding) {
            Ok(()) => log::info!("UDS session ended cleanly"),
            Err(e) => log::warn!("UDS session error: {e:#}"),
        }
        thread::sleep(Duration::from_secs(2));
    }
}

fn run_uds_session(sock: &Path, name: &str, binding: &OutputBinding) -> Result<()> {
    let stream = UnixStream::connect(sock)
        .with_context(|| format!("connect {}", sock.display()))?;
    log::info!("UDS worker connected to {}", sock.display());

    codec::send_request(
        &stream,
        &ProtoRequest::Hello {
            protocol: PROTOCOL_NAME.to_string(),
            client_name: name.to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
        },
        &[],
    )
    .map_err(|e| anyhow!("send hello: {e}"))?;
    let (welcome, _) = codec::recv_event(&stream).map_err(|e| anyhow!("recv welcome: {e}"))?;
    match welcome {
        ProtoEvent::Welcome { features, .. } => {
            if !features.iter().any(|s| s == "explicit_sync_fd") {
                bail!("server missing explicit_sync_fd feature");
            }
        }
        other => bail!("expected welcome, got opcode {}", other.opcode()),
    }

    let (width, height) = binding
        .configured_size
        .lock()
        .unwrap()
        .expect("worker started before configure");

    codec::send_request(
        &stream,
        &ProtoRequest::RegisterDisplay {
            name: name.to_string(),
            width,
            height,
            refresh_mhz: 60_000,
            properties: Vec::new(),
        },
        &[],
    )
    .map_err(|e| anyhow!("send register_display: {e}"))?;

    let display_id = match codec::recv_event(&stream)
        .map_err(|e| anyhow!("recv display_accepted: {e}"))?
    {
        (ProtoEvent::DisplayAccepted { display_id }, _) => display_id,
        (other, _) => bail!("expected display_accepted, got opcode {}", other.opcode()),
    };
    log::info!("registered as display_id={display_id} ({}x{})", width, height);

    // Buffer pool state for the active generation.
    let mut gen: Option<u64> = None;
    let mut pool: Vec<WlBuffer> = Vec::new();
    let mut buf_width: u32 = width;
    let mut buf_height: u32 = height;
    let mut frames_presented: u64 = 0;

    loop {
        let (evt, fds) = codec::recv_event(&stream).map_err(|e| anyhow!("recv event: {e}"))?;
        match evt {
            ProtoEvent::BindBuffers {
                buffer_generation,
                count,
                width: bw,
                height: bh,
                fourcc,
                modifier,
                planes_per_buffer,
                stride,
                plane_offset,
                ..
            } => {
                let expected = (count * planes_per_buffer) as usize;
                if fds.len() != expected {
                    bail!(
                        "bind_buffers expected {} fds, got {}",
                        expected,
                        fds.len()
                    );
                }
                if stride.len() != expected || plane_offset.len() != expected {
                    bail!(
                        "bind_buffers stride/offset arrays size mismatch (expected {}, stride={}, offset={})",
                        expected,
                        stride.len(),
                        plane_offset.len()
                    );
                }
                let new_pool = import_dmabufs(
                    binding,
                    count,
                    planes_per_buffer,
                    bw,
                    bh,
                    fourcc,
                    modifier,
                    &stride,
                    &plane_offset,
                    fds,
                )
                .context("import DMA-BUFs")?;
                // Drop the old pool — wl_buffer Drop sends destroy.
                pool = new_pool;
                gen = Some(buffer_generation);
                buf_width = bw;
                buf_height = bh;
                log::info!(
                    "imported {} wl_buffers for generation {} ({}x{} fourcc=0x{:08x})",
                    pool.len(),
                    buffer_generation,
                    bw,
                    bh,
                    fourcc
                );
            }
            ProtoEvent::SetConfig { .. } => {
                // Config events (source/dest rect, transform) are
                // ignored for now — we always present the full buffer
                // at the compositor-chosen surface size.
            }
            ProtoEvent::FrameReady {
                buffer_generation: g,
                buffer_index,
                seq,
            } => {
                // Drop the acquire sync_fd: compositor will wait on
                // the kernel-visible fence attached to the DMA-BUF
                // import, so we don't need to block on it here. The
                // OwnedFd destructor closes the duplicate fd we got.
                drop(fds);

                if Some(g) != gen {
                    log::warn!("stray frame_ready gen={g}, current={:?}", gen);
                } else if let Some(buffer) = pool.get(buffer_index as usize) {
                    binding.surface.attach(Some(buffer), 0, 0);
                    binding
                        .surface
                        .damage_buffer(0, 0, buf_width as i32, buf_height as i32);
                    binding.surface.commit();
                    frames_presented += 1;
                    if frames_presented == 1 || frames_presented % 60 == 0 {
                        log::info!(
                            "frame #{frames_presented}: attached buf[{buffer_index}] gen={g} seq={seq} and committed"
                        );
                    }
                    // Requests from non-dispatch threads must be
                    // flushed explicitly; the main thread's blocking
                    // dispatch will also flush, but we don't want to
                    // rely on compositor-originated wake-ups.
                    if let Err(e) = binding.conn.flush() {
                        log::warn!("wayland flush failed: {e}");
                    }
                } else {
                    log::warn!(
                        "frame_ready buffer_index {} out of range (pool size {})",
                        buffer_index,
                        pool.len()
                    );
                }

                codec::send_request(
                    &stream,
                    &ProtoRequest::BufferRelease {
                        buffer_generation: g,
                        buffer_index,
                        seq,
                    },
                    &[],
                )
                .map_err(|e| anyhow!("send buffer_release: {e}"))?;
            }
            ProtoEvent::Unbind { buffer_generation: g } => {
                if Some(g) == gen {
                    log::info!("unbind gen={g}; dropping {} buffers", pool.len());
                    pool.clear();
                    gen = None;
                }
            }
            ProtoEvent::Error { code, message } => {
                bail!("server error {code}: {message}");
            }
            _ => {}
        }
    }
}

/// Build a pool of `wl_buffer`s from the fds + per-plane metadata the
/// daemon sent in `BindBuffers`. Uses `create_immed` so we don't need
/// to round-trip through `Created`/`Failed`.
fn import_dmabufs(
    binding: &OutputBinding,
    count: u32,
    planes_per_buffer: u32,
    width: u32,
    height: u32,
    fourcc: u32,
    modifier: u64,
    stride: &[u32],
    plane_offset: &[u32],
    fds: Vec<OwnedFd>,
) -> Result<Vec<WlBuffer>> {
    // Hand-carry a queue handle for child proxy creation. We don't
    // actually own one in this thread; any QueueHandle<App> works and
    // it's cheap to clone out of the main thread's. For now the
    // worker grabs it from the Connection via a one-off queue, which
    // is enough for create_params + create_immed (no events land on
    // those proxies after).
    let queue = binding.conn.new_event_queue::<App>();
    let qh = queue.handle();

    let mut buffers = Vec::with_capacity(count as usize);
    // Consume fds by value: create_params::add takes a BorrowedFd but
    // we want to close our copy only after the buffer is created; the
    // compositor dup(2)s on its side.
    let fds: Vec<OwnedFd> = fds;
    for b in 0..count as usize {
        let params = binding.dmabuf.create_params(&qh, ());
        let mod_hi = (modifier >> 32) as u32;
        let mod_lo = (modifier & 0xffff_ffff) as u32;
        for p in 0..planes_per_buffer as usize {
            let idx = b * planes_per_buffer as usize + p;
            let fd: &OwnedFd = &fds[idx];
            params.add(
                fd.as_fd(),
                p as u32,
                plane_offset[idx],
                stride[idx],
                mod_hi,
                mod_lo,
            );
        }
        let buffer = params.create_immed(width as i32, height as i32, fourcc, zwp_linux_buffer_params_v1::Flags::empty(), &qh, ());
        // create_params is auto-destroyed after create_immed per protocol.
        buffers.push(buffer);
    }
    // Dropping the queue is fine — the params proxies may emit Failed
    // asynchronously; those would land on the main queue's Dispatch
    // impl only if registered there. Here we just discard any lingering
    // events. If users see ImportFailed in compositor logs, switch to
    // the async create path.
    drop(queue);
    drop(fds);
    Ok(buffers)
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = parse_args();

    let conn = Connection::connect_to_env()
        .context("connect to WAYLAND_DISPLAY — are you running under a Wayland compositor?")?;
    let (globals, mut queue) = registry_queue_init::<App>(&conn).context("registry init")?;
    let qh: QueueHandle<App> = queue.handle();

    let mut app = App::new(args.socket, args.name);

    // Bind each global we care about. We require all four: without
    // layer_shell + dmabuf there's no way to present a wallpaper.
    for g in globals.contents().clone_list() {
        match g.interface.as_str() {
            "wl_compositor" => {
                app.compositor = Some(globals.registry().bind::<WlCompositor, _, _>(
                    g.name,
                    g.version.min(6),
                    &qh,
                    (),
                ));
            }
            "zwlr_layer_shell_v1" => {
                app.layer_shell = Some(globals.registry().bind::<ZwlrLayerShellV1, _, _>(
                    g.name,
                    g.version.min(4),
                    &qh,
                    (),
                ));
            }
            "zwp_linux_dmabuf_v1" => {
                app.dmabuf = Some(globals.registry().bind::<ZwpLinuxDmabufV1, _, _>(
                    g.name,
                    g.version.min(3),
                    &qh,
                    (),
                ));
            }
            "wl_output" => {
                if app.output.is_none() {
                    app.output = Some(globals.registry().bind::<WlOutput, _, _>(
                        g.name,
                        g.version.min(4),
                        &qh,
                        (),
                    ));
                }
            }
            _ => {}
        }
    }
    if app.compositor.is_none() {
        bail!("compositor does not expose wl_compositor");
    }
    if app.layer_shell.is_none() {
        bail!(
            "compositor does not expose zwlr_layer_shell_v1 — \
             try a different compositor (Hyprland/Sway/KWin/new Mutter)"
        );
    }
    if app.dmabuf.is_none() {
        bail!("compositor does not expose zwp_linux_dmabuf_v1");
    }
    if app.output.is_none() {
        bail!("no wl_output available");
    }

    log::info!("bound globals: compositor + layer_shell + dmabuf + 1 output");

    // First dispatch pulls output metadata (geometry/mode/done) which
    // triggers surface creation via `bring_up_surface`.
    queue.blocking_dispatch(&mut app).context("initial dispatch")?;

    // Main event loop: keep spinning Wayland events. The UDS worker
    // runs on its own thread and mutates the surface's attached
    // buffer; we just keep the connection alive and responsive.
    loop {
        if let Err(e) = queue.blocking_dispatch(&mut app) {
            log::error!("wayland dispatch error: {e}");
            return Err(e.into());
        }
    }
}
