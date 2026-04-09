//! waywallen-renderer — Rust-side producer subprocess (M1 milestone).

use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use anyhow::{anyhow, Context, Result};
use ash::{vk, Entry, Instance};

use waywallen::ipc::proto::{ControlMsg, EventMsg};
use waywallen::ipc::uds::{recv_msg, send_msg};

const SLOT_COUNT: usize = 3;
const RENDER_FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;

struct FrameSlot {
    image: vk::Image,
    memory: vk::DeviceMemory,
}

#[derive(Debug, serde::Deserialize)]
struct Args {
    #[serde(default)]
    ipc: Option<String>,
    #[serde(default = "default_width")]
    width: u32,
    #[serde(default = "default_height")]
    height: u32,
    #[serde(default = "default_fps")]
    fps: u32,
}

fn default_width() -> u32 { 1280 }
fn default_height() -> u32 { 720 }
fn default_fps() -> u32 { 60 }

fn parse_args() -> Args {
    let mut args = Args { ipc: None, width: 1280, height: 720, fps: 60 };
    let mut iter = std::env::args().skip(1);
    while let Some(a) = iter.next() {
        match a.as_str() {
            "--ipc" => args.ipc = iter.next(),
            "--width" => args.width = iter.next().and_then(|s| s.parse().ok()).unwrap_or(1280),
            "--height" => args.height = iter.next().and_then(|s| s.parse().ok()).unwrap_or(720),
            "--fps" => args.fps = iter.next().and_then(|s| s.parse().ok()).unwrap_or(60),
            _ => { let _ = iter.next(); }
        }
    }
    args
}

fn main() -> Result<()> {
    env_logger::init();
    let args = parse_args();
    let entry = unsafe { Entry::load().context("load Vulkan")? };
    let app_info = vk::ApplicationInfo::default().api_version(vk::make_api_version(0, 1, 2, 0));
    let instance = unsafe {
        entry.create_instance(&vk::InstanceCreateInfo::default().application_info(&app_info), None)?
    };
    unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM); }
    let result = run(&instance, &args);
    unsafe { instance.destroy_instance(None); }
    result
}

fn run(instance: &Instance, args: &Args) -> Result<()> {
    let phys = unsafe { instance.enumerate_physical_devices()?[0] };
    let families = unsafe { instance.get_physical_device_queue_family_properties(phys) };
    let gfx_family = families.iter().enumerate().find(|(_, f)| f.queue_flags.contains(vk::QueueFlags::GRAPHICS)).map(|(i, _)| i as u32).ok_or_else(|| anyhow!("no gfx"))?;
    
    let ext_names = [
        vk::KHR_EXTERNAL_MEMORY_NAME.as_ptr(),
        vk::KHR_EXTERNAL_MEMORY_FD_NAME.as_ptr(),
        vk::EXT_EXTERNAL_MEMORY_DMA_BUF_NAME.as_ptr(),
        vk::EXT_IMAGE_DRM_FORMAT_MODIFIER_NAME.as_ptr(),
        vk::KHR_EXTERNAL_SEMAPHORE_NAME.as_ptr(),
        vk::KHR_EXTERNAL_SEMAPHORE_FD_NAME.as_ptr(),
    ];
    let device = unsafe {
        instance.create_device(phys, &vk::DeviceCreateInfo::default()
            .queue_create_infos(&[vk::DeviceQueueCreateInfo::default().queue_family_index(gfx_family).queue_priorities(&[1.0])])
            .enabled_extension_names(&ext_names), None)?
    };

    let mut slots = vec![];
    let mem_props = unsafe { instance.get_physical_device_memory_properties(phys) };
    for _ in 0..SLOT_COUNT {
        let mut mod_list = vk::ImageDrmFormatModifierListCreateInfoEXT::default().drm_format_modifiers(&[0]);
        let mut ext_info = vk::ExternalMemoryImageCreateInfo::default().handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let img = unsafe { device.create_image(&vk::ImageCreateInfo::default().image_type(vk::ImageType::TYPE_2D).format(RENDER_FORMAT).extent(vk::Extent3D{width:args.width,height:args.height,depth:1}).mip_levels(1).array_layers(1).samples(vk::SampleCountFlags::TYPE_1).tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT).usage(vk::ImageUsageFlags::TRANSFER_DST).push_next(&mut mod_list).push_next(&mut ext_info), None)? };
        let req = unsafe { device.get_image_memory_requirements(img) };
        let mtype = (0..mem_props.memory_type_count).find(|&i| (req.memory_type_bits & (1 << i)) != 0 && mem_props.memory_types[i as usize].property_flags.contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)).unwrap_or(0);
        let mut exp = vk::ExportMemoryAllocateInfo::default().handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let mem = unsafe { device.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(mtype).push_next(&mut exp), None)? };
        unsafe { device.bind_image_memory(img, mem, 0)?; }
        slots.push(FrameSlot { image: img, memory: mem });
    }

    let ext_mem_fd = ash::khr::external_memory_fd::Device::new(instance, &device);
    let drm_mod = ash::ext::image_drm_format_modifier::Device::new(instance, &device);
    
    let mut exports = vec![];
    for s in &slots {
        let fd = unsafe { ext_mem_fd.get_memory_fd(&vk::MemoryGetFdInfoKHR::default().memory(s.memory).handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT))? };
        let mut props = vk::ImageDrmFormatModifierPropertiesEXT::default();
        unsafe { drm_mod.get_image_drm_format_modifier_properties(s.image, &mut props)?; }
        let layout = unsafe { device.get_image_subresource_layout(s.image, vk::ImageSubresource { aspect_mask: vk::ImageAspectFlags::MEMORY_PLANE_0_EXT, mip_level: 0, array_layer: 0 }) };
        exports.push((fd, props.drm_format_modifier, layout.row_pitch));
    }

    let ipc_path = args.ipc.as_ref().ok_or_else(|| anyhow!("--ipc required"))?;
    let stream = UnixStream::connect(ipc_path)?;
    send_msg(&stream, &EventMsg::Ready, &[])?;
    
    let bind = EventMsg::BindBuffers {
        count: SLOT_COUNT as u32, fourcc: 0x34324241, width: args.width, height: args.height,
        stride: exports[0].2 as u32, modifier: exports[0].1, plane_offset: 0,
        sizes: vec![exports[0].2 * args.height as u64; SLOT_COUNT],
    };
    send_msg(&stream, &bind, &exports.iter().map(|e| e.0).collect::<Vec<_>>())?;

    let shutdown = Arc::new(AtomicBool::new(false));
    let s2 = shutdown.clone();
    let rs = stream.try_clone()?;
    thread::spawn(move || {
        while let Ok((ControlMsg::Shutdown, _)) = recv_msg::<ControlMsg>(&rs) {
            s2.store(true, Ordering::SeqCst);
            break;
        }
    });

    let queue = unsafe { device.get_device_queue(gfx_family, 0) };
    let cmd_pool = unsafe { device.create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(gfx_family).flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER), None)? };
    let cmd_buf = unsafe { device.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(cmd_pool).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1))?[0] };

    // Per-frame sync_fd export: one exportable semaphore, reused across
    // frames. vkGetSemaphoreFdKHR with SYNC_FD handle type consumes the
    // signaled state and leaves the semaphore unsignaled for the next
    // submit (VK spec §7.4.3 "Importing Semaphore Payloads" note on
    // permanence). The exported fd is a dma_fence sync_file that the
    // display side can wait on via VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD
    // or EGL_ANDROID_native_fence_sync.
    let ext_sem_fd = ash::khr::external_semaphore_fd::Device::new(instance, &device);
    let mut export_sem_info = vk::ExportSemaphoreCreateInfo::default()
        .handle_types(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
    let signal_sem = unsafe {
        device.create_semaphore(
            &vk::SemaphoreCreateInfo::default().push_next(&mut export_sem_info),
            None,
        )?
    };

    // Initial transition to GENERAL
    unsafe {
        device.begin_command_buffer(cmd_buf, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT))?;
        for s in &slots {
            let b = vk::ImageMemoryBarrier::default().old_layout(vk::ImageLayout::UNDEFINED).new_layout(vk::ImageLayout::GENERAL).image(s.image).subresource_range(vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1));
            device.cmd_pipeline_barrier(cmd_buf, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[], &[b]);
        }
        device.end_command_buffer(cmd_buf)?;
        device.queue_submit(queue, &[vk::SubmitInfo::default().command_buffers(&[cmd_buf])], vk::Fence::null())?;
        device.queue_wait_idle(queue)?;
    }

    let frame_period = std::time::Duration::from_secs_f64(1.0 / args.fps as f64);
    let start = std::time::Instant::now();
    let mut seq: u64 = 0;
    while !shutdown.load(Ordering::SeqCst) {
        let slot = (seq as usize) % SLOT_COUNT;
        let r = (seq as f32 * 0.1).sin() * 0.5 + 0.5;
        unsafe {
            device.begin_command_buffer(cmd_buf, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT))?;
            device.cmd_clear_color_image(cmd_buf, slots[slot].image, vk::ImageLayout::GENERAL, &vk::ClearColorValue { float32: [r, 0.5, 0.5, 1.0] }, &[vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1)]);
            device.end_command_buffer(cmd_buf)?;
            let signal_sems = [signal_sem];
            device.queue_submit(
                queue,
                &[vk::SubmitInfo::default()
                    .command_buffers(&[cmd_buf])
                    .signal_semaphores(&signal_sems)],
                vk::Fence::null(),
            )?;
        }
        // Export the signaled semaphore as a dma_fence sync_file fd.
        // This consumes the semaphore's signaled state; after this call
        // the semaphore is unsignaled and can be signaled again by the
        // next queue_submit. The returned fd is transferred to the
        // sendmsg cmsg immediately below.
        let sync_fd = unsafe {
            ext_sem_fd.get_semaphore_fd(
                &vk::SemaphoreGetFdInfoKHR::default()
                    .semaphore(signal_sem)
                    .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD),
            )?
        };
        let send_result = send_msg(
            &stream,
            &EventMsg::FrameReady {
                image_index: slot as u32,
                seq,
                ts_ns: 0,
                has_sync_fd: true,
            },
            &[sync_fd],
        );
        // SCM_RIGHTS dup'd the fd into the kernel's message buffer on
        // success. Close our local copy either way: on success the
        // receiver has its own copy, on failure it's just a leak.
        unsafe { libc::close(sync_fd); }
        let _ = send_result;
        seq += 1;
        let next = start + frame_period * seq as u32;
        let now = std::time::Instant::now();
        if next > now { thread::sleep(next - now); }
    }

    unsafe {
        device.device_wait_idle()?;
        device.destroy_semaphore(signal_sem, None);
        for s in slots { device.destroy_image(s.image, None); device.free_memory(s.memory, None); }
        device.destroy_device(None);
    }
    Ok(())
}
