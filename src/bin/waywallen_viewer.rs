//! waywallen-viewer — minimal consumer window for the waywallen IPC
//! fabric.
//!
//! Long-term goal (M2 milestone roadmap):
//!   M2.1/M2.2  (this iteration) open a winit window, bring up a
//!              Vulkan swapchain, present a solid clear colour every
//!              frame. No IPC, no DMA-BUF import.
//!   M2.3       connect to the daemon's viewer socket and perform the
//!              Hello/Subscribe handshake.
//!   M2.4       on BindBuffers, import the 3 DMA-BUF fds as local
//!              VkImages via VK_EXT_external_memory_dma_buf +
//!              VK_EXT_image_drm_format_modifier.
//!   M2.5       on FrameReady, blit the matching imported image into
//!              the current swapchain image and present.
//!   M2.6       resize / close / disconnect handling.
//!
//! Viewer lives entirely in one file during bring-up; it'll split into
//! modules once the IPC path + DMA-BUF import path stabilise.

use std::ffi::{CStr, CString};

use anyhow::{anyhow, Context, Result};
use ash::{khr, vk, Device, Entry, Instance};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

const WINDOW_WIDTH: u32 = 1280;
const WINDOW_HEIGHT: u32 = 720;
const CLEAR_COLOR: [f32; 4] = [0.05, 0.05, 0.1, 1.0];

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_micros()
        .init();

    let event_loop = EventLoop::new().context("winit EventLoop::new")?;
    let mut app = App::default();
    event_loop
        .run_app(&mut app)
        .context("winit event_loop.run_app")?;
    // Anyhow can't cross the run_app boundary directly; if a frame
    // failed we stored it in the app and surface it here.
    if let Some(err) = app.error.take() {
        return Err(err);
    }
    Ok(())
}

#[derive(Default)]
struct App {
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
        match VulkanState::new(window) {
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
    swapchain: vk::SwapchainKHR,
    swapchain_format: vk::Format,
    swapchain_extent: vk::Extent2D,
    swapchain_images: Vec<vk::Image>,

    cmd_pool: vk::CommandPool,
    cmd_buffers: Vec<vk::CommandBuffer>,

    image_available: vk::Semaphore,
    render_finished: vk::Semaphore,
    in_flight: vk::Fence,
}

impl VulkanState {
    fn new(window: Window) -> Result<Self> {
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

        let device_exts = [vk::KHR_SWAPCHAIN_NAME.as_ptr()];
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
            swapchain,
            swapchain_format,
            swapchain_extent,
            swapchain_images,
            cmd_pool,
            cmd_buffers,
            image_available,
            render_finished,
            in_flight,
        })
    }

    fn handle_resize(&mut self, _w: u32, _h: u32) -> Result<()> {
        // Swapchain recreation lands with M2.6 — for the bring-up iter
        // a stale swapchain just keeps rendering the clear colour.
        Ok(())
    }

    fn draw(&mut self) -> Result<()> {
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
            record_clear(&self.device, cmd_buf, image, CLEAR_COLOR);
            self.device.end_command_buffer(cmd_buf)?;

            let wait_semaphores = [self.image_available];
            let signal_semaphores = [self.render_finished];
            let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
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
