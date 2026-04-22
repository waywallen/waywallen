//! waywallen-display-layer-shell — Wayland layer-shell wallpaper client.

use std::collections::HashMap;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::net::Shutdown;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
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
    name_prefix: String,
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
    let mut name_prefix = String::from("waywallen-layer-shell");
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
                name_prefix = it.next().unwrap_or_else(|| {
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
    Args { socket, name_prefix }
}

fn default_socket_path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    runtime.join("waywallen").join("display.sock")
}

// ---------------------------------------------------------------------------
// Per-output state
// ---------------------------------------------------------------------------

/// Shared surface + protocol proxies a single output's UDS worker needs
/// to attach frames. All proxies in wayland-client 0.31 are `Send + Sync`,
/// so the worker thread can invoke requests freely; writes are serialized
/// through the shared `Connection` and flushed explicitly.
struct OutputBinding {
    display_name: String,
    surface: WlSurface,
    dmabuf: ZwpLinuxDmabufV1,
    conn: Connection,
    /// Populated by the main thread on the first Configure event for
    /// this output's layer_surface. The worker waits until this is
    /// Some before starting the UDS session.
    configured_size: Mutex<Option<(u32, u32)>>,
    /// Set to `true` when the corresponding `wl_output` is removed at
    /// runtime (hot-unplug). The worker checks before reconnect; the
    /// main thread also `shutdown(2)`s the active stream so any
    /// blocking `recv_event` returns immediately.
    closed: AtomicBool,
    /// Most-recent live UDS connection. Worker stashes it after a
    /// successful `connect`; cleared on session exit. Main thread
    /// reads + shutdowns it on hot-unplug.
    stream: RwLock<Option<Arc<UnixStream>>>,
}

/// One logical output — wl_output plus the layer_surface/UDS worker
/// set we attached to it.
struct OutputEntry {
    wl_output: WlOutput,
    surface: Option<WlSurface>,
    layer_surface: Option<ZwlrLayerSurfaceV1>,
    binding: Option<Arc<OutputBinding>>,
    worker_started: bool,
}

struct App {
    compositor: Option<WlCompositor>,
    layer_shell: Option<ZwlrLayerShellV1>,
    dmabuf: Option<ZwpLinuxDmabufV1>,
    /// Keyed by `wl_output` global name (u32). The same key is used as
    /// Dispatch user-data for every per-output child proxy so events
    /// find their owning entry in O(1).
    outputs: HashMap<u32, OutputEntry>,
    uds_sock: PathBuf,
    name_prefix: String,
}

impl App {
    fn new(uds_sock: PathBuf, name_prefix: String) -> Self {
        Self {
            compositor: None,
            layer_shell: None,
            dmabuf: None,
            outputs: HashMap::new(),
            uds_sock,
            name_prefix,
        }
    }

    /// Create the `wl_surface` + layer_surface for a specific output.
    /// Idempotent — skips outputs that already have their surface up.
    fn bring_up_surface(&mut self, output_name: u32, qh: &QueueHandle<App>) {
        let Some(entry) = self.outputs.get_mut(&output_name) else {
            return;
        };
        if entry.surface.is_some() {
            return;
        }
        let (Some(comp), Some(shell)) = (self.compositor.as_ref(), self.layer_shell.as_ref())
        else {
            return;
        };
        let surface = comp.create_surface(qh, output_name);
        let layer_surface = shell.get_layer_surface(
            &surface,
            Some(&entry.wl_output),
            Layer::Background,
            "waywallen-wallpaper".to_string(),
            qh,
            output_name,
        );
        layer_surface.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
        layer_surface.set_exclusive_zone(-1);
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer_surface.set_size(0, 0);
        surface.commit();
        entry.surface = Some(surface);
        entry.layer_surface = Some(layer_surface);
        log::info!("output {output_name}: layer_surface committed, waiting for configure");
    }

    /// Spawn the per-output UDS worker once the compositor has
    /// configured its layer_surface.
    fn maybe_spawn_worker(&mut self, output_name: u32) {
        let Some(entry) = self.outputs.get_mut(&output_name) else {
            return;
        };
        if entry.worker_started {
            return;
        }
        let Some(binding) = entry.binding.as_ref() else {
            return;
        };
        if binding.configured_size.lock().unwrap().is_none() {
            return;
        }
        entry.worker_started = true;
        let binding = Arc::clone(binding);
        let sock = self.uds_sock.clone();
        log::info!(
            "output {output_name}: spawning UDS worker ('{}')",
            binding.display_name
        );
        thread::spawn(move || uds_worker_loop(sock, binding));
    }
}

// --- Dispatch impls -------------------------------------------------------

impl Dispatch<WlRegistry, GlobalListContents> for App {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wayland_client::protocol::wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_registry::Event;
        match event {
            Event::Global { name, interface, version } => {
                // Runtime hot-plug: only `wl_output` is interesting —
                // compositor / dmabuf / layer_shell singletons don't
                // appear post-startup in any sane setup.
                if interface == "wl_output" {
                    if state.outputs.contains_key(&name) {
                        return;
                    }
                    let wl_output = registry.bind::<WlOutput, _, _>(
                        name,
                        version.min(4),
                        qh,
                        name,
                    );
                    state.outputs.insert(
                        name,
                        OutputEntry {
                            wl_output,
                            surface: None,
                            layer_surface: None,
                            binding: None,
                            worker_started: false,
                        },
                    );
                    log::info!("hot-plug: wl_output name={name} added; bringing up surface");
                    state.bring_up_surface(name, qh);
                }
            }
            Event::GlobalRemove { name } => {
                if let Some(entry) = state.outputs.remove(&name) {
                    log::info!("hot-unplug: wl_output name={name} removed");
                    // Drop layer_surface / surface explicitly so the
                    // compositor sees the destroy. The worker thread
                    // still holds a clone of the surface via its
                    // OutputBinding `Arc`; it will start emitting
                    // protocol errors on the next FrameReady → its
                    // session loop will fail, log, and reconnect-loop
                    // forever (harmlessly — daemon won't have anything
                    // for a non-registered display). Full worker
                    // teardown is a follow-up.
                    drop(entry);
                }
            }
            _ => {}
        }
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

impl Dispatch<WlSurface, u32> for App {
    fn event(
        _state: &mut Self,
        _p: &WlSurface,
        _e: wayland_client::protocol::wl_surface::Event,
        _data: &u32,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // Enter/Leave and surface scale events ignored — the compositor
        // drives layer-surface sizing via configure, and we don't care
        // which seats hover us.
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
        if let wayland_client::protocol::wl_buffer::Event::Release = event {
            log::trace!("wl_buffer {} released", buffer.id());
        }
    }
}

impl Dispatch<WlOutput, u32> for App {
    fn event(
        _state: &mut Self,
        _p: &WlOutput,
        _e: wayland_client::protocol::wl_output::Event,
        _data: &u32,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // Output metadata (Name/Geometry/Mode/Done) is informational;
        // the layer_surface's own Configure event is authoritative.
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

impl Dispatch<ZwlrLayerSurfaceV1, u32> for App {
    fn event(
        state: &mut Self,
        layer_surface: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        data: &u32,
        conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let output_name = *data;
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer_surface.ack_configure(serial);
                log::info!(
                    "output {output_name}: layer_surface configure {width}x{height}"
                );
                // Ensure the per-output OutputBinding exists, then
                // record the size and kick the worker.
                let Some(entry) = state.outputs.get_mut(&output_name) else {
                    log::warn!("configure for unknown output_name={output_name}");
                    return;
                };
                let binding = entry.binding.get_or_insert_with(|| {
                    let surface = entry
                        .surface
                        .clone()
                        .expect("configure before surface created");
                    let dmabuf = state
                        .dmabuf
                        .clone()
                        .expect("configure before dmabuf bind");
                    Arc::new(OutputBinding {
                        display_name: format!("{}-{}", state.name_prefix, output_name),
                        surface,
                        dmabuf,
                        conn: conn.clone(),
                        configured_size: Mutex::new(None),
                    })
                });
                *binding.configured_size.lock().unwrap() = Some((width, height));
                // NOTE: `entry` borrows state.outputs mutably; drop it
                // before re-entering App via maybe_spawn_worker.
                let _ = binding;
                state.maybe_spawn_worker(output_name);
            }
            zwlr_layer_surface_v1::Event::Closed => {
                log::warn!(
                    "output {output_name}: layer_surface closed by compositor"
                );
                if let Some(entry) = state.outputs.get_mut(&output_name) {
                    entry.surface = None;
                    entry.layer_surface = None;
                    entry.binding = None;
                    entry.worker_started = false;
                }
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
        if let zwp_linux_buffer_params_v1::Event::Failed = event {
            log::error!("zwp_linux_buffer_params_v1 Failed: dmabuf import rejected");
        }
    }
}

// ---------------------------------------------------------------------------
// UDS worker — one per output, each an independent daemon display.
// ---------------------------------------------------------------------------

fn uds_worker_loop(sock: PathBuf, binding: Arc<OutputBinding>) {
    loop {
        match run_uds_session(&sock, &binding) {
            Ok(()) => log::info!(
                "[{}] UDS session ended cleanly",
                binding.display_name
            ),
            Err(e) => log::warn!("[{}] UDS session error: {e:#}", binding.display_name),
        }
        thread::sleep(Duration::from_secs(2));
    }
}

fn run_uds_session(sock: &Path, binding: &OutputBinding) -> Result<()> {
    let stream = UnixStream::connect(sock)
        .with_context(|| format!("connect {}", sock.display()))?;
    log::info!(
        "[{}] UDS worker connected to {}",
        binding.display_name,
        sock.display()
    );

    codec::send_request(
        &stream,
        &ProtoRequest::Hello {
            protocol: PROTOCOL_NAME.to_string(),
            client_name: binding.display_name.clone(),
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
            name: binding.display_name.clone(),
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
    log::info!(
        "[{}] registered as display_id={display_id} ({width}x{height})",
        binding.display_name
    );

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
                pool = new_pool;
                gen = Some(buffer_generation);
                buf_width = bw;
                buf_height = bh;
                log::info!(
                    "[{}] imported {} wl_buffers for generation {} ({}x{} fourcc=0x{:08x})",
                    binding.display_name,
                    pool.len(),
                    buffer_generation,
                    bw,
                    bh,
                    fourcc
                );
            }
            ProtoEvent::SetConfig { .. } => {
                // source/dest rect + transform: not yet honored; we
                // always present the full buffer at surface size.
            }
            ProtoEvent::FrameReady {
                buffer_generation: g,
                buffer_index,
                seq,
            } => {
                drop(fds);

                if Some(g) != gen {
                    log::warn!(
                        "[{}] stray frame_ready gen={g}, current={:?}",
                        binding.display_name,
                        gen
                    );
                } else if let Some(buffer) = pool.get(buffer_index as usize) {
                    binding.surface.attach(Some(buffer), 0, 0);
                    binding
                        .surface
                        .damage_buffer(0, 0, buf_width as i32, buf_height as i32);
                    binding.surface.commit();
                    frames_presented += 1;
                    if let Err(e) = binding.conn.flush() {
                        log::warn!(
                            "[{}] wayland flush failed: {e}",
                            binding.display_name
                        );
                    }
                } else {
                    log::warn!(
                        "[{}] frame_ready buffer_index {} out of range (pool {})",
                        binding.display_name,
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
                    log::info!(
                        "[{}] unbind gen={g}; dropping {} buffers",
                        binding.display_name,
                        pool.len()
                    );
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

/// Turn daemon-supplied DMA-BUF fds + per-plane metadata into a pool of
/// `wl_buffer`s via `zwp_linux_buffer_params_v1::create_immed`.
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
    let queue = binding.conn.new_event_queue::<App>();
    let qh = queue.handle();

    let mut buffers = Vec::with_capacity(count as usize);
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
        let buffer = params.create_immed(
            width as i32,
            height as i32,
            fourcc,
            zwp_linux_buffer_params_v1::Flags::empty(),
            &qh,
            (),
        );
        buffers.push(buffer);
    }
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

    let mut app = App::new(args.socket, args.name_prefix);

    // Bind every global we care about. Outputs are collected into the
    // App's `outputs` map keyed by global name; every per-output child
    // proxy carries that name as Dispatch user-data.
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
                let wl_output = globals.registry().bind::<WlOutput, _, _>(
                    g.name,
                    g.version.min(4),
                    &qh,
                    g.name,
                );
                app.outputs.insert(
                    g.name,
                    OutputEntry {
                        wl_output,
                        surface: None,
                        layer_surface: None,
                        binding: None,
                        worker_started: false,
                    },
                );
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
    if app.outputs.is_empty() {
        bail!("no wl_output available");
    }
    log::info!(
        "bound globals: compositor + layer_shell + dmabuf + {} output(s)",
        app.outputs.len()
    );

    // Create the per-output layer_surfaces up-front. The compositor will
    // emit a Configure event for each, which kicks off its UDS worker.
    let output_keys: Vec<u32> = app.outputs.keys().copied().collect();
    for name in output_keys {
        app.bring_up_surface(name, &qh);
    }

    loop {
        if let Err(e) = queue.blocking_dispatch(&mut app) {
            log::error!("wayland dispatch error: {e}");
            return Err(e.into());
        }
    }
}
