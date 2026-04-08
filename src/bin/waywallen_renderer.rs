//! waywallen-renderer — Rust-side producer subprocess (M1 milestone).
//!
//! The long-term renderer is the C++ `open-wallpaper-engine` host under
//! `/open-wallpaper-engine/host/`. While the C++ pipeline is under
//! construction this Rust binary stands in as a minimal producer: it
//! opens a headless Vulkan 1.2 context, allocates 3 DMA-BUF-backed
//! VkImages, and cycles through them clearing to solid colours. The
//! daemon routes the resulting frames to subscribed viewers exactly as
//! it will for the real renderer.
//!
//! Iteration M1.3a (this file): export DMA-BUF file descriptors from
//! the 3 VkDeviceMemory objects, query each image's plane-0 subresource
//! layout to get stride and offset, and log the per-slot DRM metadata.
//! Still no IPC.
//!
//! Roadmap for the next sub-iterations:
//!   M1.3b wire IPC, send Ready + BindBuffers with the exported FDs
//!   M1.4  per-frame vkCmdClearColorImage + FrameReady
//!   M1.5  pipeline + triangle-strip solid quad on top of clear
//!   M1.6  Shutdown handling

use std::ffi::{CStr, CString};

use anyhow::{anyhow, Context, Result};
use ash::{vk, Device, Entry, Instance};

/// One logical frame slot in the triple buffer. Each slot owns a
/// VkImage + the VkDeviceMemory backing it. The memory is allocated
/// with VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT so its FD can be
/// exported and handed to consumers via SCM_RIGHTS.
struct FrameSlot {
    image: vk::Image,
    memory: vk::DeviceMemory,
    width: u32,
    height: u32,
}

/// Metadata needed by a consumer to import a slot's DMA-BUF back into
/// its own Vulkan instance. Maps directly onto the fields the daemon
/// forwards in `ipc::proto::BindBuffers`.
#[derive(Debug)]
struct SlotExport {
    fd: std::os::fd::OwnedFd,
    drm_fourcc: u32,
    drm_modifier: u64,
    plane0_offset: u64,
    plane0_stride: u64,
}

const SLOT_COUNT: usize = 3;
const RENDER_FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;

/// Required device extensions for DMA-BUF export + DRM modifier paths.
/// Matches the C++ host's `VulkanRender.cpp` when `offscreen=true`.
const REQUIRED_DEVICE_EXTENSIONS: &[&CStr] = &[
    vk::KHR_EXTERNAL_MEMORY_FD_NAME,
    vk::EXT_EXTERNAL_MEMORY_DMA_BUF_NAME,
    vk::EXT_IMAGE_DRM_FORMAT_MODIFIER_NAME,
    vk::EXT_QUEUE_FAMILY_FOREIGN_NAME,
];

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_micros()
        .init();

    // Safety: `Entry::load()` dlopens libvulkan.so at runtime. We must
    // keep the returned Entry alive for the lifetime of the Instance
    // below, which is the case here.
    let entry = unsafe { Entry::load().context("failed to load libvulkan.so")? };

    let app_name = CString::new("waywallen-renderer").unwrap();
    let engine_name = CString::new("waywallen").unwrap();
    let app_info = vk::ApplicationInfo::default()
        .application_name(app_name.as_c_str())
        .application_version(vk::make_api_version(0, 0, 1, 0))
        .engine_name(engine_name.as_c_str())
        .engine_version(vk::make_api_version(0, 0, 1, 0))
        .api_version(vk::make_api_version(0, 1, 2, 0));

    // Instance-level extensions we require: external-memory capabilities
    // must be queryable at the physical-device level before we pick one.
    let instance_exts = [
        vk::KHR_EXTERNAL_MEMORY_CAPABILITIES_NAME.as_ptr(),
        vk::KHR_GET_PHYSICAL_DEVICE_PROPERTIES2_NAME.as_ptr(),
    ];
    let create_info = vk::InstanceCreateInfo::default()
        .application_info(&app_info)
        .enabled_extension_names(&instance_exts);

    let instance: Instance = unsafe {
        entry
            .create_instance(&create_info, None)
            .context("vkCreateInstance failed — check Vulkan driver installation")?
    };
    log::info!("VkInstance created (api 1.2)");

    let result = run(&instance);

    unsafe {
        instance.destroy_instance(None);
    }
    result
}

fn run(instance: &Instance) -> Result<()> {
    let phys_devices = unsafe {
        instance
            .enumerate_physical_devices()
            .context("enumerate_physical_devices failed")?
    };
    if phys_devices.is_empty() {
        return Err(anyhow!("no Vulkan-capable physical devices"));
    }

    let mut chosen: Option<vk::PhysicalDevice> = None;
    for &pd in &phys_devices {
        let props = unsafe { instance.get_physical_device_properties(pd) };
        let name = unsafe { CStr::from_ptr(props.device_name.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        let has_all = check_device_extensions(instance, pd)?;
        log::info!(
            "  candidate: {name} (type={:?}, ext_ok={has_all})",
            props.device_type
        );
        if has_all && chosen.is_none() {
            chosen = Some(pd);
        }
    }

    let phys = chosen.ok_or_else(|| {
        anyhow!(
            "no physical device supports all required extensions: {:?}",
            REQUIRED_DEVICE_EXTENSIONS
        )
    })?;
    let props = unsafe { instance.get_physical_device_properties(phys) };
    let name = unsafe { CStr::from_ptr(props.device_name.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    log::info!("picked device: {name}");

    let gfx_family = pick_graphics_queue_family(instance, phys)?;
    log::info!("graphics queue family: {gfx_family}");

    let device = create_device(instance, phys, gfx_family)?;
    log::info!("VkDevice created");

    // For M1.2 hard-code 512x512 until --width/--height arrive in M1.3.
    let width = 512u32;
    let height = 512u32;

    let slots = create_frame_slots(instance, phys, &device, width, height)?;
    log::info!(
        "allocated {} exportable DMA-BUF frame slots ({width}x{height})",
        slots.len()
    );

    let exports = export_slots(instance, &device, &slots)?;
    for (i, e) in exports.iter().enumerate() {
        log::info!(
            "  slot {i}: fd={} fourcc=0x{:08x} modifier=0x{:016x} offset={} stride={}",
            std::os::fd::AsRawFd::as_raw_fd(&e.fd),
            e.drm_fourcc,
            e.drm_modifier,
            e.plane0_offset,
            e.plane0_stride,
        );
    }

    // The OwnedFds in `exports` drop here, closing the duplicated DMA-BUF
    // FDs. In M1.3b they will instead be handed off to the IPC sender so
    // the daemon can forward them to viewers via SCM_RIGHTS.
    drop(exports);

    // Tear down in reverse order. Kernel refcounting keeps the DMA-BUFs
    // alive across consumer imports regardless of local cleanup order.
    for slot in &slots {
        unsafe {
            device.destroy_image(slot.image, None);
            device.free_memory(slot.memory, None);
        }
    }
    unsafe { device.destroy_device(None) };
    Ok(())
}

/// Map an R8G8B8A8_UNORM VkFormat to its DRM fourcc code. Matches
/// `VkFormatToDrmFourcc()` in the C++ host (TextureCache.cpp).
fn vk_format_to_drm_fourcc(fmt: vk::Format) -> Result<u32> {
    use drm_fourcc::DrmFourcc;
    match fmt {
        vk::Format::R8G8B8A8_UNORM => Ok(DrmFourcc::Abgr8888 as u32),
        vk::Format::B8G8R8A8_UNORM => Ok(DrmFourcc::Argb8888 as u32),
        _ => Err(anyhow!("no DRM fourcc mapping for {fmt:?}")),
    }
}

fn export_slots(
    instance: &Instance,
    device: &Device,
    slots: &[FrameSlot],
) -> Result<Vec<SlotExport>> {
    use std::os::fd::FromRawFd;

    let ext_mem_fd = ash::khr::external_memory_fd::Device::new(instance, device);
    let drm_mod = ash::ext::image_drm_format_modifier::Device::new(instance, device);

    let mut out = Vec::with_capacity(slots.len());
    for (i, slot) in slots.iter().enumerate() {
        let fd_info = vk::MemoryGetFdInfoKHR::default()
            .memory(slot.memory)
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let raw_fd = unsafe {
            ext_mem_fd
                .get_memory_fd(&fd_info)
                .with_context(|| format!("vkGetMemoryFdKHR failed for slot {i}"))?
        };
        // SAFETY: raw_fd is a freshly-created OS fd the kernel just handed
        // us; taking ownership is correct and releases on drop.
        let owned_fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw_fd) };

        // Query the modifier the driver actually ended up assigning. With
        // only DRM_FORMAT_MOD_LINEAR in the list this will always be 0,
        // but keep the query in place so future tiled modifiers light up
        // automatically.
        let mut mod_props = vk::ImageDrmFormatModifierPropertiesEXT::default();
        unsafe {
            drm_mod
                .get_image_drm_format_modifier_properties(slot.image, &mut mod_props)
                .with_context(|| format!("vkGetImageDrmFormatModifierPropertiesEXT slot {i}"))?;
        }

        // Plane-0 layout: vkGetImageSubresourceLayout needs
        // MEMORY_PLANE_0_BIT aspect for DRM-modifier images.
        let subresource = vk::ImageSubresource {
            aspect_mask: vk::ImageAspectFlags::MEMORY_PLANE_0_EXT,
            mip_level: 0,
            array_layer: 0,
        };
        let layout = unsafe { device.get_image_subresource_layout(slot.image, subresource) };

        out.push(SlotExport {
            fd: owned_fd,
            drm_fourcc: vk_format_to_drm_fourcc(RENDER_FORMAT)?,
            drm_modifier: mod_props.drm_format_modifier,
            plane0_offset: layout.offset,
            plane0_stride: layout.row_pitch,
        });
    }
    Ok(out)
}

fn pick_graphics_queue_family(instance: &Instance, phys: vk::PhysicalDevice) -> Result<u32> {
    let families = unsafe { instance.get_physical_device_queue_family_properties(phys) };
    families
        .iter()
        .enumerate()
        .find(|(_, f)| f.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        .map(|(i, _)| i as u32)
        .ok_or_else(|| anyhow!("no graphics queue family"))
}

fn create_device(
    instance: &Instance,
    phys: vk::PhysicalDevice,
    gfx_family: u32,
) -> Result<Device> {
    let priorities = [1.0f32];
    let queue_infos = [vk::DeviceQueueCreateInfo::default()
        .queue_family_index(gfx_family)
        .queue_priorities(&priorities)];

    // Raw const* -> needs stable backing store for the lifetime of the
    // DeviceCreateInfo.
    let ext_ptrs: Vec<*const i8> = REQUIRED_DEVICE_EXTENSIONS
        .iter()
        .map(|e| e.as_ptr())
        .collect();

    let features = vk::PhysicalDeviceFeatures::default();
    let create_info = vk::DeviceCreateInfo::default()
        .queue_create_infos(&queue_infos)
        .enabled_extension_names(&ext_ptrs)
        .enabled_features(&features);

    let device = unsafe {
        instance
            .create_device(phys, &create_info, None)
            .context("vkCreateDevice failed")?
    };
    Ok(device)
}

fn create_frame_slots(
    instance: &Instance,
    phys: vk::PhysicalDevice,
    device: &Device,
    width: u32,
    height: u32,
) -> Result<Vec<FrameSlot>> {
    let mut slots = Vec::with_capacity(SLOT_COUNT);
    for i in 0..SLOT_COUNT {
        let slot = create_one_slot(instance, phys, device, width, height)
            .with_context(|| format!("failed to allocate slot {i}"))?;
        slots.push(slot);
    }
    Ok(slots)
}

fn create_one_slot(
    instance: &Instance,
    phys: vk::PhysicalDevice,
    device: &Device,
    width: u32,
    height: u32,
) -> Result<FrameSlot> {
    // Pick DRM_FORMAT_MOD_LINEAR (0) to match what the C++ host currently
    // exports. Future work: query vkGetPhysicalDeviceImageFormatProperties2
    // + VkPhysicalDeviceImageDrmFormatModifierInfoEXT to negotiate a
    // tiled modifier with the consumer.
    let modifiers = [0u64]; // DRM_FORMAT_MOD_LINEAR
    let mut modifier_list = vk::ImageDrmFormatModifierListCreateInfoEXT::default()
        .drm_format_modifiers(&modifiers);

    let handle_types = vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT;
    let mut external_info =
        vk::ExternalMemoryImageCreateInfo::default().handle_types(handle_types);

    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(RENDER_FORMAT)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .push_next(&mut modifier_list)
        .push_next(&mut external_info);

    let image = unsafe {
        device
            .create_image(&image_info, None)
            .context("vkCreateImage (DMA-BUF) failed")?
    };

    let mem_req = unsafe { device.get_image_memory_requirements(image) };

    let mem_props = unsafe { instance.get_physical_device_memory_properties(phys) };
    let mem_type_index = pick_memory_type(
        &mem_props,
        mem_req.memory_type_bits,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    )
    .ok_or_else(|| anyhow!("no suitable device-local memory type"))?;

    let mut export_info =
        vk::ExportMemoryAllocateInfo::default().handle_types(handle_types);
    let mut dedicated_info = vk::MemoryDedicatedAllocateInfo::default().image(image);
    let alloc_info = vk::MemoryAllocateInfo::default()
        .allocation_size(mem_req.size)
        .memory_type_index(mem_type_index)
        .push_next(&mut export_info)
        .push_next(&mut dedicated_info);

    let memory = unsafe {
        device
            .allocate_memory(&alloc_info, None)
            .context("vkAllocateMemory (exportable) failed")?
    };

    unsafe {
        device
            .bind_image_memory(image, memory, 0)
            .context("vkBindImageMemory failed")?;
    }

    Ok(FrameSlot {
        image,
        memory,
        width,
        height,
    })
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

fn check_device_extensions(instance: &Instance, pd: vk::PhysicalDevice) -> Result<bool> {
    let available = unsafe {
        instance
            .enumerate_device_extension_properties(pd)
            .context("enumerate_device_extension_properties failed")?
    };
    for required in REQUIRED_DEVICE_EXTENSIONS {
        let found = available.iter().any(|ext| {
            let n = unsafe { CStr::from_ptr(ext.extension_name.as_ptr()) };
            n == *required
        });
        if !found {
            log::debug!("  missing extension: {}", required.to_string_lossy());
            return Ok(false);
        }
    }
    Ok(true)
}
