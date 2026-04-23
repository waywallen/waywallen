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
    wl_buffer::WlBuffer, wl_callback::{self, WlCallback}, wl_compositor::WlCompositor,
    wl_output::{self, Transform, WlOutput},
    wl_registry::WlRegistry, wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1},
    zwp_linux_dmabuf_v1::{self, ZwpLinuxDmabufV1},
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::{self, WpViewport},
    wp_viewporter::{self, WpViewporter},
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
    /// QueueHandle used for child proxies created from the worker
    /// thread (frame callbacks, dmabuf params). Clone of the main
    /// thread's queue handle.
    qh: QueueHandle<App>,
    /// Global name of the owning `wl_output`. Used as user-data when
    /// requesting frame callbacks so the main-thread Dispatch routes
    /// the `Done` event back to the right `frame_pending` flag.
    output_name: u32,
    /// Physical buffer size (logical × integer_scale) the daemon must
    /// render at for 1:1 mapping on HiDPI. Populated on the first
    /// layer_surface Configure; worker advertises this as the display
    /// size when registering with the daemon.
    configured_size: Mutex<Option<(u32, u32)>>,
    /// Logical surface size (from `zwlr_layer_surface_v1::configure`).
    /// Used as the viewport destination so the compositor maps the
    /// physical-size buffer onto the correct surface extent.
    logical_size: Mutex<Option<(u32, u32)>>,
    /// Integer output scale from `wl_output::scale`. Defaults to 1;
    /// updated before worker spawns (we roundtrip after bind so
    /// output metadata has landed).
    scale: std::sync::atomic::AtomicI32,
    /// Optional `wp_viewport` — when bound, gives us explicit
    /// source-rect/dest-rect mapping between buffer and surface
    /// (handles HiDPI + `SetConfig` crop). Absent → fall back to
    /// `wl_surface::set_buffer_scale`.
    viewport: Option<WpViewport>,
    /// Set to `true` when the corresponding `wl_output` is removed at
    /// runtime (hot-unplug). The worker checks before reconnect; the
    /// main thread also `shutdown(2)`s the active stream so any
    /// blocking `recv_event` returns immediately.
    closed: AtomicBool,
    /// Most-recent live UDS connection. Worker stashes it after a
    /// successful `connect`; cleared on session exit. Main thread
    /// reads + shutdowns it on hot-unplug.
    stream: RwLock<Option<Arc<UnixStream>>>,
    /// `true` while a `wl_callback::done` is outstanding. Set after
    /// commit + frame(); cleared by the `WlCallback` Dispatch impl.
    /// Gates whether the worker commits a new buffer (throttles to
    /// compositor vblank) — `BufferRelease` is always sent so the
    /// daemon keeps producing.
    frame_pending: AtomicBool,
}

/// One logical output — wl_output plus the layer_surface/UDS worker
/// set we attached to it.
struct OutputEntry {
    wl_output: WlOutput,
    surface: Option<WlSurface>,
    layer_surface: Option<ZwlrLayerSurfaceV1>,
    viewport: Option<WpViewport>,
    binding: Option<Arc<OutputBinding>>,
    worker_started: bool,
    /// Latest integer scale from `wl_output::scale`. Sampled into the
    /// binding on first configure. `1` when the event hasn't fired.
    scale: i32,
}

struct App {
    compositor: Option<WlCompositor>,
    layer_shell: Option<ZwlrLayerShellV1>,
    dmabuf: Option<ZwpLinuxDmabufV1>,
    /// Optional `wp_viewporter` — if the compositor exposes it, each
    /// surface gets a viewport and we set explicit source/dest rects
    /// every commit. Older compositors without it fall back to
    /// `wl_surface::set_buffer_scale`.
    viewporter: Option<WpViewporter>,
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
            viewporter: None,
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
        // If the compositor advertises wp_viewporter, attach a viewport
        // to this surface so we can map arbitrary buffer regions to
        // arbitrary surface extents (needed for HiDPI + SetConfig).
        let viewport = self
            .viewporter
            .as_ref()
            .map(|vp| vp.get_viewport(&surface, qh, output_name));
        surface.commit();
        entry.surface = Some(surface);
        entry.layer_surface = Some(layer_surface);
        entry.viewport = viewport;
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
                            viewport: None,
                            binding: None,
                            worker_started: false,
                            scale: 1,
                        },
                    );
                    log::info!("hot-plug: wl_output name={name} added; bringing up surface");
                    state.bring_up_surface(name, qh);
                }
            }
            Event::GlobalRemove { name } => {
                if let Some(entry) = state.outputs.remove(&name) {
                    log::info!("hot-unplug: wl_output name={name} removed");
                    // Tear down the worker thread cooperatively:
                    //   1. flip `closed` so the reconnect loop exits
                    //      after its current session.
                    //   2. if the worker is mid-session (blocked on
                    //      `recv_event`), shutdown its UnixStream —
                    //      the kernel unblocks the read and the
                    //      session returns with an error.
                    if let Some(binding) = entry.binding.as_ref() {
                        binding.closed.store(true, Ordering::SeqCst);
                        if let Some(stream) = binding.stream.read().unwrap().clone() {
                            let _ = stream.shutdown(Shutdown::Both);
                        }
                    }
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

impl Dispatch<WlCallback, u32> for App {
    fn event(
        state: &mut Self,
        _cb: &WlCallback,
        event: wl_callback::Event,
        data: &u32,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // Compositor signalled that it presented the last commit; it's
        // safe to commit another buffer now. user_data carries the
        // owning wl_output name so we target the right binding.
        if let wl_callback::Event::Done { .. } = event {
            let output_name = *data;
            if let Some(binding) = state
                .outputs
                .get(&output_name)
                .and_then(|e| e.binding.as_ref())
            {
                binding.frame_pending.store(false, Ordering::SeqCst);
            }
        }
    }
}

impl Dispatch<WlOutput, u32> for App {
    fn event(
        state: &mut Self,
        _p: &WlOutput,
        event: wl_output::Event,
        data: &u32,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // Track the integer buffer scale so HiDPI outputs get a
        // physically-sized buffer + viewporter mapping.
        if let wl_output::Event::Scale { factor } = event {
            let output_name = *data;
            if let Some(entry) = state.outputs.get_mut(&output_name) {
                entry.scale = factor.max(1);
                if let Some(binding) = entry.binding.as_ref() {
                    binding
                        .scale
                        .store(factor.max(1), Ordering::SeqCst);
                }
            }
        }
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
        qh: &QueueHandle<Self>,
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
                        qh: qh.clone(),
                        output_name,
                        configured_size: Mutex::new(None),
                        logical_size: Mutex::new(None),
                        scale: std::sync::atomic::AtomicI32::new(entry.scale.max(1)),
                        viewport: entry.viewport.clone(),
                        closed: AtomicBool::new(false),
                        stream: RwLock::new(None),
                        frame_pending: AtomicBool::new(false),
                    })
                });
                // `width` / `height` from `configure` are in *logical*
                // (surface-local) coordinates. For 1:1 rendering on
                // HiDPI we ask the daemon to produce a buffer of
                // `logical × integer_scale` physical pixels and then
                // map that full buffer back down to the logical surface
                // extent via `wp_viewporter`.
                let scale = entry.scale.max(1);
                binding
                    .scale
                    .store(scale, Ordering::SeqCst);
                let physical = (
                    width.saturating_mul(scale as u32),
                    height.saturating_mul(scale as u32),
                );
                *binding.logical_size.lock().unwrap() = Some((width, height));
                *binding.configured_size.lock().unwrap() = Some(physical);
                if scale > 1 {
                    log::info!(
                        "output {output_name}: logical {width}x{height} × scale {scale} → physical {}x{}",
                        physical.0,
                        physical.1
                    );
                }
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

impl Dispatch<WpViewporter, ()> for App {
    fn event(
        _state: &mut Self,
        _p: &WpViewporter,
        _e: wp_viewporter::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // wp_viewporter has no events.
    }
}

impl Dispatch<WpViewport, u32> for App {
    fn event(
        _state: &mut Self,
        _p: &WpViewport,
        _e: wp_viewport::Event,
        _data: &u32,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // wp_viewport has no events.
    }
}

// ---------------------------------------------------------------------------
// UDS worker — one per output, each an independent daemon display.
// ---------------------------------------------------------------------------

fn uds_worker_loop(sock: PathBuf, binding: Arc<OutputBinding>) {
    loop {
        if binding.closed.load(Ordering::SeqCst) {
            log::info!(
                "[{}] output closed; worker exiting",
                binding.display_name
            );
            return;
        }
        let res = run_uds_session(&sock, &binding);
        // Always clear the active stream slot on session exit so the
        // hot-unplug path doesn't shutdown a stale fd on the next
        // connection.
        binding.stream.write().unwrap().take();
        match res {
            Ok(()) => log::info!(
                "[{}] UDS session ended cleanly",
                binding.display_name
            ),
            Err(e) => log::warn!("[{}] UDS session error: {e:#}", binding.display_name),
        }
        if binding.closed.load(Ordering::SeqCst) {
            log::info!(
                "[{}] output closed; worker exiting after session end",
                binding.display_name
            );
            return;
        }
        thread::sleep(Duration::from_secs(2));
    }
}

fn run_uds_session(sock: &Path, binding: &OutputBinding) -> Result<()> {
    let stream = Arc::new(
        UnixStream::connect(sock)
            .with_context(|| format!("connect {}", sock.display()))?,
    );
    // Publish the live stream so the main thread can `shutdown(2)` it
    // on hot-unplug — that unblocks the blocking `recv_event` below.
    *binding.stream.write().unwrap() = Some(stream.clone());
    let stream: &UnixStream = &stream;
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
    // Latest SetConfig values, applied on each FrameReady commit.
    // Units are buffer pixels (source) / surface logical pixels (dest) /
    // wl_output::Transform enum index (transform).
    let mut cfg_source: Option<(f32, f32, f32, f32)> = None;
    let mut cfg_dest_size: Option<(f32, f32)> = None;
    let mut cfg_transform: u32 = 0;
    // Set once on first SetConfig (or first FrameReady with defaults)
    // so we only call `set_buffer_transform` when it actually changes.
    let mut transform_dirty: bool = true;

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
            ProtoEvent::SetConfig {
                source_rect,
                dest_rect,
                transform,
                ..
            } => {
                cfg_source = Some((source_rect.x, source_rect.y, source_rect.w, source_rect.h));
                cfg_dest_size = Some((dest_rect.w, dest_rect.h));
                if cfg_transform != transform {
                    cfg_transform = transform;
                    transform_dirty = true;
                }
                log::debug!(
                    "[{}] set_config src=({:.0},{:.0} {:.0}x{:.0}) dest_size=({:.0}x{:.0}) xform={}",
                    binding.display_name,
                    source_rect.x,
                    source_rect.y,
                    source_rect.w,
                    source_rect.h,
                    dest_rect.w,
                    dest_rect.h,
                    transform
                );
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
                    // Throttle commits to compositor vblank: if the
                    // last frame_callback hasn't fired yet, skip this
                    // commit (but always ack BufferRelease below so
                    // the daemon isn't starved). The compositor will
                    // redraw from whatever buffer is currently
                    // attached.
                    if binding.frame_pending.load(Ordering::SeqCst) {
                        log::trace!(
                            "[{}] skip commit: frame callback pending",
                            binding.display_name
                        );
                    } else {
                        binding.surface.attach(Some(buffer), 0, 0);

                        // Map buffer → surface via wp_viewporter when
                        // available. Source defaults to the full buffer;
                        // SetConfig can crop. Destination defaults to
                        // the logical surface size; SetConfig can shrink.
                        let src = cfg_source.unwrap_or((
                            0.0,
                            0.0,
                            buf_width as f32,
                            buf_height as f32,
                        ));
                        let logical = binding
                            .logical_size
                            .lock()
                            .unwrap()
                            .unwrap_or((buf_width, buf_height));
                        let dest = cfg_dest_size
                            .unwrap_or((logical.0 as f32, logical.1 as f32));

                        if let Some(vp) = binding.viewport.as_ref() {
                            // wayland-scanner maps `fixed` args to f64.
                            vp.set_source(
                                src.0 as f64,
                                src.1 as f64,
                                src.2 as f64,
                                src.3 as f64,
                            );
                            vp.set_destination(dest.0 as i32, dest.1 as i32);
                        } else {
                            // Fallback: tell the compositor the buffer
                            // is scale× larger than the surface.
                            let scale = binding.scale.load(Ordering::SeqCst);
                            if scale > 1 {
                                binding.surface.set_buffer_scale(scale);
                            }
                        }

                        // Transform — only re-emit when changed.
                        if transform_dirty {
                            binding
                                .surface
                                .set_buffer_transform(map_transform(cfg_transform));
                            transform_dirty = false;
                        }

                        binding
                            .surface
                            .damage_buffer(0, 0, buf_width as i32, buf_height as i32);
                        // Request a frame callback *before* committing
                        // so the callback is tied to this surface
                        // state. user_data = output_name so the
                        // Dispatch impl can find the right binding.
                        binding.surface.frame(&binding.qh, binding.output_name);
                        binding.frame_pending.store(true, Ordering::SeqCst);
                        binding.surface.commit();
                        frames_presented += 1;
                        if let Err(e) = binding.conn.flush() {
                            log::warn!(
                                "[{}] wayland flush failed: {e}",
                                binding.display_name
                            );
                        }
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
/// Map the daemon's `transform` u32 (matching `wl_output::transform`
/// semantics per `protocol/waywallen_display_v1.xml`) to the
/// wayland-client enum. Unknown values fall back to `Normal` rather
/// than erroring — the daemon owns the protocol and invalid values
/// would break far bigger things.
fn map_transform(t: u32) -> Transform {
    match t {
        0 => Transform::Normal,
        1 => Transform::_90,
        2 => Transform::_180,
        3 => Transform::_270,
        4 => Transform::Flipped,
        5 => Transform::Flipped90,
        6 => Transform::Flipped180,
        7 => Transform::Flipped270,
        _ => Transform::Normal,
    }
}

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
            "wp_viewporter" => {
                app.viewporter = Some(globals.registry().bind::<WpViewporter, _, _>(
                    g.name,
                    g.version.min(1),
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
                        viewport: None,
                        binding: None,
                        worker_started: false,
                        scale: 1,
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
        "bound globals: compositor + layer_shell + dmabuf + viewporter:{} + {} output(s)",
        app.viewporter.is_some(),
        app.outputs.len()
    );

    // Roundtrip once so every `wl_output` has delivered its initial
    // metadata (Scale / Geometry / Mode / Done) before we create
    // layer-surfaces. Without this, outputs on HiDPI compositors
    // would configure us at logical size with `scale=1` and we'd
    // advertise the wrong physical size to the daemon.
    queue
        .roundtrip(&mut app)
        .context("initial wl_output metadata roundtrip")?;

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
