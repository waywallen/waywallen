//! waywallen-viewer — minimal consumer window for the waywallen IPC
//! fabric.
//!
//! Long-term goal (M2 milestone roadmap):
//!   M2.1/M2.2  open a winit window, bring up a Vulkan swapchain,
//!              present a solid clear colour every frame. [done]
//!   M2.3       background IPC thread: Hello/Subscribe handshake +
//!              BindBuffers receive + FrameReady tracking. [done]
//!   M2.4       (this iteration) import the 3 DMA-BUF fds as local
//!              VkImages via VK_EXT_external_memory_dma_buf +
//!              VK_EXT_image_drm_format_modifier. Imports happen
//!              lazily on the first draw call after BindBuffers
//!              lands; the window keeps showing its clear colour
//!              until M2.5 wires the blit.
//!   M2.5       on FrameReady, blit the matching imported image into
//!              the current swapchain image and present.
//!   M2.6       resize / close / disconnect handling.
//!
//! Viewer lives entirely in one file during bring-up; it'll split into
//! modules once the IPC path + DMA-BUF import path stabilise.

use std::ffi::{CStr, CString};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{anyhow, Context, Result};
use ash::{khr, vk, Device, Entry, Instance};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use kwallpaper_backend::ipc::proto::{EventMsg, ViewerMsg, PROTOCOL_VERSION};
use kwallpaper_backend::ipc::uds::{recv_msg, send_msg};

const WINDOW_WIDTH: u32 = 1280;
const WINDOW_HEIGHT: u32 = 720;
const CLEAR_COLOR: [f32; 4] = [0.05, 0.05, 0.1, 1.0];

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

struct Args {
    /// Daemon viewer socket. Defaults to $XDG_RUNTIME_DIR/waywallen/viewer.sock.
    viewer_sock: Option<PathBuf>,
    /// Renderer id to subscribe to. When absent, the IPC path is skipped
    /// entirely and the window just shows a clear colour — useful for
    /// bringing up the window outside of a full daemon+renderer stack.
    renderer_id: Option<String>,
}

fn parse_args() -> Args {
    let mut viewer_sock: Option<PathBuf> = None;
    let mut renderer_id: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--viewer-sock" => viewer_sock = args.next().map(PathBuf::from),
            "--renderer-id" => renderer_id = args.next(),
            _ => {}
        }
    }
    Args {
        viewer_sock,
        renderer_id,
    }
}

fn default_viewer_sock() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("waywallen")
        .join("viewer.sock")
}

// ---------------------------------------------------------------------------
// Shared IPC state
// ---------------------------------------------------------------------------

/// Captured BindBuffers metadata + owned DMA-BUF FDs. The IPC thread
/// populates this once after the handshake; the main thread reads it
/// when it's ready to import the buffers (M2.4).
struct BindState {
    count: u32,
    fourcc: u32,
    width: u32,
    height: u32,
    stride: u32,
    modifier: u64,
    plane_offset: u64,
    sizes: Vec<u64>,
    fds: Vec<OwnedFd>,
}

struct SharedIpc {
    /// Populated exactly once after the initial handshake.
    bind: Mutex<Option<BindState>>,
    /// Last image_index the renderer announced. Read by the draw loop.
    current_slot: AtomicU32,
    /// Number of frames observed (mostly for logging / tests).
    frame_count: AtomicU32,
    /// Set by the IPC thread on socket error / clean shutdown.
    ipc_dead: AtomicBool,
    /// M3: sync_file FD for the next blit. Consumed by the draw loop.
    pending_sync_fd: Mutex<Option<OwnedFd>>,
}

impl SharedIpc {
    fn new() -> Self {
        Self {
            bind: Mutex::new(None),
            current_slot: AtomicU32::new(0),
            frame_count: AtomicU32::new(0),
            ipc_dead: AtomicBool::new(false),
            pending_sync_fd: Mutex::new(None),
        }
    }
}

// ---------------------------------------------------------------------------
// IPC worker thread
// ---------------------------------------------------------------------------

fn spawn_ipc_thread(
    viewer_sock: PathBuf,
    renderer_id: String,
    shared: Arc<SharedIpc>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        if let Err(e) = run_ipc(&viewer_sock, &renderer_id, &shared) {
            log::error!("viewer IPC thread died: {e:#}");
        }
        shared.ipc_dead.store(true, Ordering::SeqCst);
    })
}

fn run_ipc(viewer_sock: &std::path::Path, renderer_id: &str, shared: &SharedIpc) -> Result<()> {
    loop {
        shared.ipc_dead.store(false, Ordering::SeqCst);
        let res = run_ipc_session(viewer_sock, renderer_id, shared);
        shared.ipc_dead.store(true, Ordering::SeqCst);

        match res {
            Ok(_) => log::info!("IPC session ended cleanly; reconnecting in 2s..."),
            Err(e) => log::error!("IPC session failed: {e:#}; reconnecting in 2s..."),
        }
        thread::sleep(std::time::Duration::from_secs(2));
    }
}

fn run_ipc_session(
    viewer_sock: &std::path::Path,
    renderer_id: &str,
    shared: &SharedIpc,
) -> Result<()> {
    let stream = UnixStream::connect(viewer_sock)
        .with_context(|| format!("connect {}", viewer_sock.display()))?;
    log::info!("viewer IPC connected to {}", viewer_sock.display());

    send_msg(
        &stream,
        &ViewerMsg::Hello {
            client: "waywallen-viewer".into(),
            version: PROTOCOL_VERSION,
        },
        &[],
    )
    .context("send Hello")?;
    send_msg(
        &stream,
        &ViewerMsg::Subscribe {
            renderer_id: renderer_id.into(),
        },
        &[],
    )
    .context("send Subscribe")?;

    // Expect BindBuffers first.
    let (msg, fds) = recv_msg::<EventMsg>(&stream).context("recv BindBuffers")?;
    let bind = match msg {
        EventMsg::BindBuffers {
            count,
            fourcc,
            width,
            height,
            stride,
            modifier,
            plane_offset,
            sizes,
        } => BindState {
            count,
            fourcc,
            width,
            height,
            stride,
            modifier,
            plane_offset,
            sizes,
            fds,
        },
        other => return Err(anyhow!("expected BindBuffers first, got {other:?}")),
    };
    log::info!(
        "viewer got BindBuffers: {} fds, {}x{} fourcc=0x{:08x} mod=0x{:016x} stride={}",
        bind.fds.len(),
        bind.width,
        bind.height,
        bind.fourcc,
        bind.modifier,
        bind.stride,
    );
    {
        let mut guard = shared.bind.lock().map_err(|e| anyhow!("bind mutex: {e}"))?;
        *guard = Some(bind);
    }

    // Stream FrameReady events.
    loop {
        match recv_msg::<EventMsg>(&stream) {
            Ok((EventMsg::FrameReady { image_index, seq, has_sync_fd, .. }, fds)) => {
                shared.current_slot.store(image_index, Ordering::SeqCst);
                let n = shared.frame_count.fetch_add(1, Ordering::SeqCst) + 1;
                if n % 30 == 0 {
                    log::info!("viewer observed {n} frames (seq={seq}, slot={image_index})");
                }

                if has_sync_fd {
                    if let Some(fd) = fds.into_iter().next() {
                        let mut guard = shared.pending_sync_fd.lock().map_err(|e| anyhow!("sync_fd mutex: {e}"))?;
                        *guard = Some(fd);
                    }
                }
            }
            Ok((other, _)) => {
                log::debug!("viewer ignored event {other:?}");
            }
            Err(e) => return Err(anyhow!("recv: {e}")),
        }
    }
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_micros()
        .init();

    let args = parse_args();
    let shared = Arc::new(SharedIpc::new());

    // Spawn the IPC thread only when the user passed a renderer id.
    // This keeps the window bring-up path runnable outside of a full
    // daemon+renderer stack.
    let ipc_thread = args.renderer_id.as_ref().map(|id| {
        let sock = args.viewer_sock.clone().unwrap_or_else(default_viewer_sock);
        log::info!(
            "viewer: IPC enabled, sock={}, renderer_id={id}",
            sock.display()
        );
        spawn_ipc_thread(sock, id.clone(), shared.clone())
    });
    if ipc_thread.is_none() {
        log::info!("viewer: no --renderer-id, running standalone (clear colour only)");
    }

    let event_loop = EventLoop::new().context("winit EventLoop::new")?;
    let mut app = App {
        shared,
        state: None,
        error: None,
    };
    event_loop
        .run_app(&mut app)
        .context("winit event_loop.run_app")?;
    if let Some(err) = app.error.take() {
        return Err(err);
    }
    // Best-effort join — the IPC thread only exits on socket error.
    drop(ipc_thread);
    Ok(())
}

struct App {
    shared: Arc<SharedIpc>,
    state: Option<VulkanState>,
    error: Option<anyhow::Error>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("waywallen viewer")
            .with_inner_size(LogicalSize::new(WINDOW_WIDTH, WINDOW_HEIGHT));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => w,
            Err(e) => {
                self.error = Some(anyhow::Error::from(e).context("create_window"));
                event_loop.exit();
                return;
            }
        };
        match VulkanState::new(window, self.shared.clone()) {
            Ok(s) => self.state = Some(s),
            Err(e) => {
                self.error = Some(e.context("VulkanState::new"));
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Err(e) = state.handle_resize(size.width, size.height) {
                    self.error = Some(e.context("handle_resize"));
                    event_loop.exit();
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = state.draw() {
                    self.error = Some(e.context("draw"));
                    event_loop.exit();
                }
                state.window.request_redraw();
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Vulkan state
// ---------------------------------------------------------------------------

#[allow(dead_code)]
struct VulkanState {
    window: Window,

    entry: Entry,
    instance: Instance,
    surface_loader: khr::surface::Instance,
    surface: vk::SurfaceKHR,

    physical: vk::PhysicalDevice,
    graphics_family: u32,
    device: Device,
    graphics_queue: vk::Queue,

    swapchain_loader: khr::swapchain::Device,
    semaphore_loader: ash::khr::external_semaphore_fd::Device,
    swapchain: vk::SwapchainKHR,
    swapchain_format: vk::Format,
    swapchain_extent: vk::Extent2D,
    swapchain_images: Vec<vk::Image>,

    cmd_pool: vk::CommandPool,
    cmd_buffers: Vec<vk::CommandBuffer>,

    image_available: vk::Semaphore,
    render_finished: vk::Semaphore,
    sync_semaphore: vk::Semaphore,
    in_flight: vk::Fence,

    // M2.4: imported DMA-BUF slots, populated lazily on first draw
    // after BindBuffers arrives. None until then; Some forever after.
    imported: Option<ImportedSlots>,

    // Shared IPC state, cloned from the App so the draw loop can read
    // the latest slot index and the BindState.
    shared: Arc<SharedIpc>,

    is_stale: bool,
}

/// Local mirror of the renderer's triple buffer: 3 VkImages + the
/// VkDeviceMemory each was imported into. The producer's FDs were
/// transferred to Vulkan during vkAllocateMemory, so this struct owns
/// only the resulting handles, not the original OwnedFds.
struct ImportedSlots {
    images: [vk::Image; 3],
    memories: [vk::DeviceMemory; 3],
    width: u32,
    height: u32,
}

impl VulkanState {
    fn new(window: Window, shared: Arc<SharedIpc>) -> Result<Self> {
        let entry = unsafe { Entry::load().context("load libvulkan.so")? };

        let app_name = CString::new("waywallen-viewer").unwrap();
        let app_info = vk::ApplicationInfo::default()
            .application_name(app_name.as_c_str())
            .api_version(vk::make_api_version(0, 1, 2, 0));

        // Surface-related instance extensions depend on the display
        // server; ash-window picks the right ones from the window
        // handle and returns them as a slice of raw pointers.
        let display_handle = window.display_handle().context("display_handle")?;
        let surface_exts =
            ash_window::enumerate_required_extensions(display_handle.as_raw())
                .context("enumerate_required_extensions")?
                .to_vec();

        let instance_exts = surface_exts;
        let create_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_extension_names(&instance_exts);
        let instance = unsafe {
            entry
                .create_instance(&create_info, None)
                .context("vkCreateInstance")?
        };

        let surface_loader = khr::surface::Instance::new(&entry, &instance);
        let surface = unsafe {
            let window_handle = window.window_handle().context("window_handle")?;
            ash_window::create_surface(
                &entry,
                &instance,
                display_handle.as_raw(),
                window_handle.as_raw(),
                None,
            )
            .context("ash_window::create_surface")?
        };

        // Pick the first device that has both a graphics queue and
        // surface support on the same family. Good enough for a
        // bring-up; M2.4 will also require DMA-BUF import extensions.
        let phys_devices = unsafe {
            instance
                .enumerate_physical_devices()
                .context("enumerate_physical_devices")?
        };
        let (physical, graphics_family) = phys_devices
            .iter()
            .find_map(|&pd| find_graphics_present_family(&instance, &surface_loader, pd, surface))
            .ok_or_else(|| anyhow!("no device with graphics+present queue"))?;
        let name = unsafe {
            CStr::from_ptr(instance.get_physical_device_properties(physical).device_name.as_ptr())
        }
        .to_string_lossy()
        .into_owned();
        log::info!("viewer picked device: {name} (family {graphics_family})");

        // Swapchain for the window + DMA-BUF import set so M2.4 can
        // pull renderer output into local VkImages.
        let device_exts = [
            vk::KHR_SWAPCHAIN_NAME.as_ptr(),
            vk::KHR_EXTERNAL_MEMORY_FD_NAME.as_ptr(),
            vk::EXT_EXTERNAL_MEMORY_DMA_BUF_NAME.as_ptr(),
            vk::EXT_IMAGE_DRM_FORMAT_MODIFIER_NAME.as_ptr(),
            vk::KHR_EXTERNAL_SEMAPHORE_FD_NAME.as_ptr(),
        ];
        let priorities = [1.0f32];
        let queue_infos = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(graphics_family)
            .queue_priorities(&priorities)];
        let features = vk::PhysicalDeviceFeatures::default();
        let device_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_infos)
            .enabled_extension_names(&device_exts)
            .enabled_features(&features);
        let device = unsafe {
            instance
                .create_device(physical, &device_info, None)
                .context("vkCreateDevice")?
        };
        let graphics_queue = unsafe { device.get_device_queue(graphics_family, 0) };

        let swapchain_loader = khr::swapchain::Device::new(&instance, &device);
        let semaphore_loader = ash::khr::external_semaphore_fd::Device::new(&instance, &device);
        let (swapchain, swapchain_format, swapchain_extent, swapchain_images) =
            create_swapchain(
                &instance,
                &device,
                &surface_loader,
                &swapchain_loader,
                physical,
                surface,
                graphics_family,
                window.inner_size().width,
                window.inner_size().height,
                vk::SwapchainKHR::null(),
            )?;
        log::info!(
            "swapchain: {}x{} format={:?} images={}",
            swapchain_extent.width,
            swapchain_extent.height,
            swapchain_format,
            swapchain_images.len()
        );

        let cmd_pool = unsafe {
            device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(graphics_family)
                        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                    None,
                )
                .context("create_command_pool")?
        };
        let cmd_buffers = unsafe {
            device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(cmd_pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(swapchain_images.len() as u32),
                )
                .context("allocate_command_buffers")?
        };

        let image_available = unsafe {
            device
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                .context("semaphore")?
        };
        let render_finished = unsafe {
            device
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                .context("semaphore")?
        };
        let sync_semaphore = unsafe {
            device
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                .context("semaphore")?
        };
        let in_flight = unsafe {
            device
                .create_fence(
                    &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                    None,
                )
                .context("fence")?
        };

        Ok(Self {
            window,
            entry,
            instance,
            surface_loader,
            surface,
            physical,
            graphics_family,
            device,
            graphics_queue,
            swapchain_loader,
            semaphore_loader,
            swapchain,
            swapchain_format,
            swapchain_extent,
            swapchain_images,
            cmd_pool,
            cmd_buffers,
            image_available,
            render_finished,
            sync_semaphore,
            in_flight,
            imported: None,
            shared,
            is_stale: false,
        })
    }

    /// Lazily import the renderer's 3 DMA-BUF fds into local VkImages
    /// the first time we observe a populated `BindState`. The IPC
    /// thread parks the BindState behind the shared mutex; we move
    /// the FDs into Vulkan exactly once and then mark the slot
    /// permanently imported.
    fn try_import_buffers(&mut self) -> Result<()> {
        let mut bind_guard = match self.shared.bind.lock() {
            Ok(g) => g,
            Err(_) => return Ok(()),
        };
        let bind = match bind_guard.take() {
            Some(b) => b,
            None => return Ok(()),
        };
        drop(bind_guard);

        log::info!(
            "viewer importing 3 DMA-BUF slots ({}x{} fourcc=0x{:08x} mod=0x{:016x})",
            bind.width,
            bind.height,
            bind.fourcc,
            bind.modifier
        );

        // If we already have imported slots, destroy them first. This
        // happens on renderer restart/rebind.
        if let Some(old) = self.imported.take() {
            log::info!("destroying old imported slots before re-import");
            unsafe {
                let _ = self.device.device_wait_idle();
                for img in old.images {
                    self.device.destroy_image(img, None);
                }
                for mem in old.memories {
                    self.device.free_memory(mem, None);
                }
            }
        }

        let imported = import_dma_buf_slots(&self.instance, &self.device, self.physical, bind)
            .context("import_dma_buf_slots")?;
        self.imported = Some(imported);
        Ok(())
    }

    fn handle_resize(&mut self, w: u32, h: u32) -> Result<()> {
        if w == 0 || h == 0 {
            return Ok(());
        }
        log::info!("resize to {w}x{h}, recreating swapchain");
        unsafe {
            self.device
                .device_wait_idle()
                .context("device_wait_idle before resize")?;
        }

        let (swapchain, format, extent, images) = create_swapchain(
            &self.instance,
            &self.device,
            &self.surface_loader,
            &self.swapchain_loader,
            self.physical,
            self.surface,
            self.graphics_family,
            w,
            h,
            self.swapchain,
        )?;

        unsafe {
            // Destroy the old swapchain. This also destroys the old
            // swapchain images, so we must refresh our local vec.
            self.swapchain_loader.destroy_swapchain(self.swapchain, None);

            // Re-allocate command buffers to match the new image count.
            self.device
                .free_command_buffers(self.cmd_pool, &self.cmd_buffers);
            self.cmd_buffers = self.device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(self.cmd_pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(images.len() as u32),
                )
                .context("allocate_command_buffers after resize")?;
        }

        self.swapchain = swapchain;
        self.swapchain_format = format;
        self.swapchain_extent = extent;
        self.swapchain_images = images;

        Ok(())
    }

    fn draw(&mut self) -> Result<()> {
        let dead = self.shared.ipc_dead.load(Ordering::Relaxed);
        if dead != self.is_stale {
            self.is_stale = dead;
            let title = if dead {
                "waywallen viewer [stale]"
            } else {
                "waywallen viewer"
            };
            self.window.set_title(title);
        }

        // M3: pull in any pending sync_file FD and import it into
        // sync_semaphore. We must only wait on sync_semaphore in the
        // submit below if we successfully imported one *this frame* —
        // the temporary import is consumed by the wait, after which the
        // semaphore reverts to its prior (unsignaled) state, and any
        // subsequent submit that waits on it would block forever GPU-
        // side. That bug previously caused the viewer to display only
        // the first blit forever (wallpaper looked frozen).
        let mut have_sync_fd_this_frame = false;
        let sync_fd = match self.shared.pending_sync_fd.lock() {
            Ok(mut g) => g.take(),
            Err(_) => None,
        };
        if let Some(fd) = sync_fd {
            use std::os::fd::IntoRawFd;
            let import_info = vk::ImportSemaphoreFdInfoKHR::default()
                .semaphore(self.sync_semaphore)
                .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD)
                .fd(fd.into_raw_fd())
                .flags(vk::SemaphoreImportFlags::TEMPORARY);
            unsafe {
                self.semaphore_loader
                    .import_semaphore_fd(&import_info)
                    .context("import_semaphore_fd")?;
            }
            have_sync_fd_this_frame = true;
        }

        // Best-effort: pull in any pending BindBuffers before drawing.
        // A failure here doesn't kill the window — we still present
        // the clear colour so the user can see something on screen.
        if let Err(e) = self.try_import_buffers() {
            log::error!("import failed: {e:#}");
        }

        let fences = [self.in_flight];
        unsafe {
            self.device
                .wait_for_fences(&fences, true, u64::MAX)
                .context("wait_for_fences")?;
            self.device.reset_fences(&fences).context("reset_fences")?;
        }
        let (image_index, _suboptimal) = unsafe {
            self.swapchain_loader
                .acquire_next_image(
                    self.swapchain,
                    u64::MAX,
                    self.image_available,
                    vk::Fence::null(),
                )
                .context("acquire_next_image")?
        };
        let cmd_buf = self.cmd_buffers[image_index as usize];
        let image = self.swapchain_images[image_index as usize];

        unsafe {
            self.device
                .reset_command_buffer(cmd_buf, vk::CommandBufferResetFlags::empty())?;
            self.device.begin_command_buffer(
                cmd_buf,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;

            let mut used_blit = false;
            if let Some(imported) = &self.imported {
                let slot = self.shared.current_slot.load(Ordering::SeqCst) as usize % 3;
                let src_image = imported.images[slot];
                let src_extent = vk::Extent2D {
                    width: imported.width,
                    height: imported.height,
                };
                record_blit(
                    &self.device,
                    cmd_buf,
                    src_image,
                    src_extent,
                    image,
                    self.swapchain_extent,
                );
                used_blit = true;
            }

            if !used_blit {
                record_clear(&self.device, cmd_buf, image, CLEAR_COLOR);
            }

            self.device.end_command_buffer(cmd_buf)?;

            let mut wait_semaphores = vec![self.image_available];
            let mut wait_stages = vec![vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
            if have_sync_fd_this_frame {
                // Only wait on sync_semaphore if we just imported a
                // payload into it. Waiting on it without a fresh
                // import deadlocks the GPU side of the submit.
                wait_semaphores.push(self.sync_semaphore);
                wait_stages.push(vk::PipelineStageFlags::TRANSFER);
            }

            let signal_semaphores = [self.render_finished];
            let cmd_bufs = [cmd_buf];
            let submit = vk::SubmitInfo::default()
                .wait_semaphores(&wait_semaphores)
                .wait_dst_stage_mask(&wait_stages)
                .command_buffers(&cmd_bufs)
                .signal_semaphores(&signal_semaphores);
            self.device
                .queue_submit(self.graphics_queue, &[submit], self.in_flight)
                .context("queue_submit")?;

            let swapchains = [self.swapchain];
            let image_indices = [image_index];
            let present_info = vk::PresentInfoKHR::default()
                .wait_semaphores(&signal_semaphores)
                .swapchains(&swapchains)
                .image_indices(&image_indices);
            let _ = self
                .swapchain_loader
                .queue_present(self.graphics_queue, &present_info);
        }
        Ok(())
    }
}

impl Drop for VulkanState {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            if let Some(imported) = self.imported.take() {
                for img in imported.images {
                    self.device.destroy_image(img, None);
                }
                for mem in imported.memories {
                    self.device.free_memory(mem, None);
                }
            }
            self.device.destroy_fence(self.in_flight, None);
            self.device.destroy_semaphore(self.render_finished, None);
            self.device.destroy_semaphore(self.image_available, None);
            self.device.destroy_command_pool(self.cmd_pool, None);
            self.swapchain_loader.destroy_swapchain(self.swapchain, None);
            self.device.destroy_device(None);
            self.surface_loader.destroy_surface(self.surface, None);
            self.instance.destroy_instance(None);
        }
    }
}

// ---------------------------------------------------------------------------
// DMA-BUF import
// ---------------------------------------------------------------------------

fn import_dma_buf_slots(
    instance: &Instance,
    device: &Device,
    phys: vk::PhysicalDevice,
    bind: BindState,
) -> Result<ImportedSlots> {
    use std::os::fd::IntoRawFd;

    if bind.fds.len() != 3 {
        return Err(anyhow!(
            "expected 3 DMA-BUF fds, got {}",
            bind.fds.len()
        ));
    }
    let format = drm_fourcc_to_vk(bind.fourcc)
        .ok_or_else(|| anyhow!("no VkFormat mapping for fourcc 0x{:08x}", bind.fourcc))?;

    let ext_mem_fd = ash::khr::external_memory_fd::Device::new(instance, device);
    let mem_props = unsafe { instance.get_physical_device_memory_properties(phys) };

    let mut images = [vk::Image::null(); 3];
    let mut memories = [vk::DeviceMemory::null(); 3];

    // Move fds out of the BindState — vkAllocateMemory takes ownership.
    let fds: Vec<std::os::fd::OwnedFd> = bind.fds.into_iter().collect();

    for (i, fd) in fds.into_iter().enumerate() {
        // ---- create the image with explicit DRM modifier ----
        let plane_layouts = [vk::SubresourceLayout {
            offset: bind.plane_offset,
            size: 0,
            row_pitch: bind.stride as u64,
            array_pitch: 0,
            depth_pitch: 0,
        }];
        let mut explicit = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
            .drm_format_modifier(bind.modifier)
            .plane_layouts(&plane_layouts);

        let handle_types = vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT;
        let mut external_info =
            vk::ExternalMemoryImageCreateInfo::default().handle_types(handle_types);

        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width: bind.width,
                height: bind.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut explicit)
            .push_next(&mut external_info);

        let image = unsafe {
            device
                .create_image(&image_info, None)
                .with_context(|| format!("create_image slot {i}"))?
        };

        // ---- import the FD into VkDeviceMemory ----
        let raw_fd = fd.into_raw_fd();
        let mem_req = unsafe { device.get_image_memory_requirements(image) };
        let mem_type_index = pick_memory_type(
            &mem_props,
            mem_req.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .ok_or_else(|| anyhow!("no DEVICE_LOCAL memory type for slot {i}"))?;

        let mut import_info = vk::ImportMemoryFdInfoKHR::default()
            .handle_type(handle_types)
            .fd(raw_fd);
        let mut dedicated_info =
            vk::MemoryDedicatedAllocateInfo::default().image(image);
        let alloc_info = vk::MemoryAllocateInfo::default()
            .allocation_size(mem_req.size)
            .memory_type_index(mem_type_index)
            .push_next(&mut import_info)
            .push_next(&mut dedicated_info);

        let memory = unsafe {
            device
                .allocate_memory(&alloc_info, None)
                .with_context(|| format!("import allocate_memory slot {i}"))?
        };
        // After vkAllocateMemory succeeds with VK_KHR_external_memory_fd
        // semantics, the kernel transfers FD ownership to Vulkan; we
        // must NOT close raw_fd ourselves.

        unsafe {
            device
                .bind_image_memory(image, memory, 0)
                .with_context(|| format!("bind_image_memory slot {i}"))?;
        }

        images[i] = image;
        memories[i] = memory;
    }

    // Suppress the unused warning on ext_mem_fd: the import path goes
    // through vkAllocateMemory + VkImportMemoryFdInfoKHR rather than
    // calling get_memory_fd directly. We keep the loader instance for
    // future calls (e.g. sync_file import in M3).
    let _ = ext_mem_fd;

    Ok(ImportedSlots {
        images,
        memories,
        width: bind.width,
        height: bind.height,
    })
}

fn drm_fourcc_to_vk(fourcc: u32) -> Option<vk::Format> {
    use drm_fourcc::DrmFourcc;
    match DrmFourcc::try_from(fourcc).ok()? {
        DrmFourcc::Abgr8888 => Some(vk::Format::R8G8B8A8_UNORM),
        DrmFourcc::Argb8888 => Some(vk::Format::B8G8R8A8_UNORM),
        _ => None,
    }
}

fn pick_memory_type(
    mem_props: &vk::PhysicalDeviceMemoryProperties,
    type_bits: u32,
    required_flags: vk::MemoryPropertyFlags,
) -> Option<u32> {
    (0..mem_props.memory_type_count).find(|&i| {
        let bit = 1u32 << i;
        let t = mem_props.memory_types[i as usize];
        (type_bits & bit) != 0 && t.property_flags.contains(required_flags)
    })
}

fn record_clear(device: &Device, cmd_buf: vk::CommandBuffer, image: vk::Image, color: [f32; 4]) {
    let range = vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .base_mip_level(0)
        .level_count(1)
        .base_array_layer(0)
        .layer_count(1);
    unsafe {
        // UNDEFINED -> TRANSFER_DST -> PRESENT_SRC using pipeline
        // barriers. Keep the ownership transfers at IGNORED since the
        // single graphics queue owns every stage here.
        let to_transfer = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(range);
        device.cmd_pipeline_barrier(
            cmd_buf,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_transfer],
        );

        let clear = vk::ClearColorValue { float32: color };
        device.cmd_clear_color_image(
            cmd_buf,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &clear,
            &[range],
        );

        let to_present = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::empty())
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(range);
        device.cmd_pipeline_barrier(
            cmd_buf,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::BOTTOM_OF_PIPE,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_present],
        );
    }
}

fn record_blit(
    device: &Device,
    cmd_buf: vk::CommandBuffer,
    src_image: vk::Image,
    src_extent: vk::Extent2D,
    dst_image: vk::Image,
    dst_extent: vk::Extent2D,
) {
    let range = vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .base_mip_level(0)
        .level_count(1)
        .base_array_layer(0)
        .layer_count(1);

    unsafe {
        // Transition src: UNDEFINED -> TRANSFER_SRC_OPTIMAL
        // Transition dst: UNDEFINED -> TRANSFER_DST_OPTIMAL
        //
        // Note: Using UNDEFINED for src_image is slightly risky but per
        // plan.md M2.5 it's the bring-up path.
        let barriers = [
            vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(src_image)
                .subresource_range(range),
            vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(dst_image)
                .subresource_range(range),
        ];

        device.cmd_pipeline_barrier(
            cmd_buf,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &barriers,
        );

        let blit = vk::ImageBlit::default()
            .src_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .mip_level(0)
                    .base_array_layer(0)
                    .layer_count(1),
            )
            .src_offsets([
                vk::Offset3D { x: 0, y: 0, z: 0 },
                vk::Offset3D {
                    x: src_extent.width as i32,
                    y: src_extent.height as i32,
                    z: 1,
                },
            ])
            .dst_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .mip_level(0)
                    .base_array_layer(0)
                    .layer_count(1),
            )
            .dst_offsets([
                vk::Offset3D { x: 0, y: 0, z: 0 },
                vk::Offset3D {
                    x: dst_extent.width as i32,
                    y: dst_extent.height as i32,
                    z: 1,
                },
            ]);

        device.cmd_blit_image(
            cmd_buf,
            src_image,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            dst_image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &[blit],
            vk::Filter::LINEAR,
        );

        // Transition dst: TRANSFER_DST_OPTIMAL -> PRESENT_SRC_KHR
        let to_present = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::empty())
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(dst_image)
            .subresource_range(range);

        device.cmd_pipeline_barrier(
            cmd_buf,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::BOTTOM_OF_PIPE,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_present],
        );
    }
}

fn find_graphics_present_family(
    instance: &Instance,
    surface_loader: &khr::surface::Instance,
    pd: vk::PhysicalDevice,
    surface: vk::SurfaceKHR,
) -> Option<(vk::PhysicalDevice, u32)> {
    let families = unsafe { instance.get_physical_device_queue_family_properties(pd) };
    for (i, f) in families.iter().enumerate() {
        if !f.queue_flags.contains(vk::QueueFlags::GRAPHICS) {
            continue;
        }
        let supports = unsafe {
            surface_loader
                .get_physical_device_surface_support(pd, i as u32, surface)
                .unwrap_or(false)
        };
        if supports {
            return Some((pd, i as u32));
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn create_swapchain(
    _instance: &Instance,
    _device: &Device,
    surface_loader: &khr::surface::Instance,
    swapchain_loader: &khr::swapchain::Device,
    physical: vk::PhysicalDevice,
    surface: vk::SurfaceKHR,
    graphics_family: u32,
    req_w: u32,
    req_h: u32,
    old_swapchain: vk::SwapchainKHR,
) -> Result<(vk::SwapchainKHR, vk::Format, vk::Extent2D, Vec<vk::Image>)> {
    let caps = unsafe {
        surface_loader
            .get_physical_device_surface_capabilities(physical, surface)
            .context("surface_capabilities")?
    };
    let formats = unsafe {
        surface_loader
            .get_physical_device_surface_formats(physical, surface)
            .context("surface_formats")?
    };
    let present_modes = unsafe {
        surface_loader
            .get_physical_device_surface_present_modes(physical, surface)
            .context("surface_present_modes")?
    };

    let surface_format = formats
        .iter()
        .copied()
        .find(|f| {
            f.format == vk::Format::B8G8R8A8_SRGB
                && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
        })
        .unwrap_or(formats[0]);
    let present_mode = present_modes
        .iter()
        .copied()
        .find(|&m| m == vk::PresentModeKHR::MAILBOX)
        .unwrap_or(vk::PresentModeKHR::FIFO);

    let extent = if caps.current_extent.width != u32::MAX {
        caps.current_extent
    } else {
        vk::Extent2D {
            width: req_w.clamp(caps.min_image_extent.width, caps.max_image_extent.width),
            height: req_h.clamp(caps.min_image_extent.height, caps.max_image_extent.height),
        }
    };

    let min_image_count = (caps.min_image_count + 1).min(if caps.max_image_count == 0 {
        u32::MAX
    } else {
        caps.max_image_count
    });

    let queue_family_indices = [graphics_family];
    let create_info = vk::SwapchainCreateInfoKHR::default()
        .surface(surface)
        .min_image_count(min_image_count)
        .image_format(surface_format.format)
        .image_color_space(surface_format.color_space)
        .image_extent(extent)
        .image_array_layers(1)
        .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST)
        .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
        .queue_family_indices(&queue_family_indices)
        .pre_transform(caps.current_transform)
        .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
        .present_mode(present_mode)
        .old_swapchain(old_swapchain)
        .clipped(true);
    let swapchain = unsafe {
        swapchain_loader
            .create_swapchain(&create_info, None)
            .context("create_swapchain")?
    };
    let images = unsafe {
        swapchain_loader
            .get_swapchain_images(swapchain)
            .context("get_swapchain_images")?
    };
    Ok((swapchain, surface_format.format, extent, images))
}
