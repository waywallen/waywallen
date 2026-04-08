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
//! Iteration M1.1 (this file): bring up VkInstance + pick a physical
//! device and print its name. No DMA-BUF export yet, no IPC yet —
//! getting ash + the required extensions working in isolation before
//! wiring anything to the daemon.
//!
//! Roadmap for the next sub-iterations:
//!   M1.2  create 3 exportable VkImages (DMA-BUF + DRM format modifier)
//!   M1.3  export FDs + send BindBuffers to daemon
//!   M1.4  per-frame vkCmdClearColorImage + FrameReady
//!   M1.5  pipeline + triangle-strip solid quad on top of clear
//!   M1.6  Shutdown handling

use std::ffi::{CStr, CString};

use anyhow::{anyhow, Context, Result};
use ash::{vk, Entry, Instance};

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

    let picked = chosen.ok_or_else(|| {
        anyhow!(
            "no physical device supports all required extensions: {:?}",
            REQUIRED_DEVICE_EXTENSIONS
        )
    })?;
    let props = unsafe { instance.get_physical_device_properties(picked) };
    let name = unsafe { CStr::from_ptr(props.device_name.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    log::info!("picked device: {name}");

    Ok(())
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
