#![cfg(feature = "legacy_proto_tests")]
// Legacy viewer-protocol integration test; see Cargo.toml feature docs.

//! Iteration 7 end-to-end test: DMA-BUF import roundtrip.
//!
//! Builds on iter6 by actually consuming the renderer's exported FDs:
//! after the display client receives `BindBuffers`, the test stands up a
//! fresh ash `VkInstance` + `VkDevice` and re-imports each FD as a
//! local `VkImage` via `VK_KHR_external_memory_fd` +
//! `VK_EXT_image_drm_format_modifier`. This proves the metadata
//! advertised by the renderer is sufficient for an unrelated Vulkan
//! consumer to bind the same DMA-BUF — i.e. the protocol carries
//! enough state for a real viewer process to import the renderer's
//! triple buffer.
//!
//! The test does not draw anything, just exercises the import path.
//! Window-based presentation lives in the `waywallen_viewer` binary
//! and is verified manually.

use waywallen::ipc::proto::{EventMsg, ViewerMsg, PROTOCOL_VERSION};
use waywallen::ipc::uds::{recv_msg, send_msg};
use waywallen::renderer_manager::{RendererManager, SpawnRequest};
use waywallen::display_endpoint;

use std::ffi::{CStr, CString};
use std::os::fd::{IntoRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use ash::{vk, Entry, Instance};

const DRM_FORMAT_ABGR8888: u32 = 0x34324241;

#[tokio::test]
async fn end_to_end_dma_buf_vulkan_import() {
    // SAFETY: single-threaded test runtime.
    unsafe {
        std::env::set_var(
            "WAYWALLEN_RENDERER_BIN",
            env!("CARGO_BIN_EXE_waywallen_renderer"),
        );
    }

    let mgr = Arc::new(RendererManager::new());
    let display_sock: PathBuf = std::env::temp_dir().join(format!(
        "waywallen-iter7-display-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mgr_clone = Arc::clone(&mgr);
    let display_sock_for_task = display_sock.clone();
    let endpoint = tokio::spawn(async move {
        let _ = display_endpoint::serve(&display_sock_for_task, mgr_clone).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let id = mgr
        .spawn(SpawnRequest {
            scene_pkg: String::new(),
            assets: String::new(),
            width: 256,
            height: 256,
            fps: 30,
            test_pattern: false,
        })
        .await
        .expect("spawn waywallen_renderer");

    let handle = mgr.get(&id).await.expect("renderer in map");
    let bind_arc = handle.bind_snapshot();
    let mut waited = Duration::ZERO;
    loop {
        if bind_arc.lock().unwrap().is_some() {
            break;
        }
        if waited > Duration::from_secs(10) {
            panic!("BindBuffers never cached");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        waited += Duration::from_millis(50);
    }

    // Run the sync IPC + Vulkan import on a blocking thread so the
    // tokio runtime stays responsive (see iter6 for the underlying
    // deadlock fix).
    let display_sock_for_client = display_sock.clone();
    let renderer_id_for_client = id.clone();
    let import_summary = tokio::task::spawn_blocking(move || {
        let stream = UnixStream::connect(&display_sock_for_client)
            .expect("connect display socket");
        send_msg(
            &stream,
            &ViewerMsg::Hello {
                client: "iter7-test".to_string(),
                version: PROTOCOL_VERSION,
            },
            &[],
        )
        .expect("send Hello");
        send_msg(
            &stream,
            &ViewerMsg::Subscribe {
                renderer_id: renderer_id_for_client,
            },
            &[],
        )
        .expect("send Subscribe");

        let (msg, fds): (EventMsg, Vec<OwnedFd>) =
            recv_msg(&stream).expect("recv BindBuffers");
        let (fourcc, width, height, stride, modifier, plane_offset) = match msg {
            EventMsg::BindBuffers {
                fourcc,
                width,
                height,
                stride,
                modifier,
                plane_offset,
                ..
            } => (fourcc, width, height, stride, modifier, plane_offset),
            other => panic!("expected BindBuffers, got {other:?}"),
        };
        assert_eq!(fourcc, DRM_FORMAT_ABGR8888);
        assert_eq!(fds.len(), 3);

        // Now import each fd into a fresh Vulkan instance. Bubble any
        // ash error up via panic so the test fails clearly.
        import_into_fresh_vulkan(width, height, stride, modifier, plane_offset, fds)
            .expect("import into fresh Vulkan");
        format!("imported 3 DMA-BUF slots ({width}x{height} stride={stride})")
    })
    .await
    .expect("blocking join");
    eprintln!("[iter7] {import_summary}");

    mgr.kill(&id).await.expect("kill");
    endpoint.abort();
    let _ = std::fs::remove_file(&display_sock);
}

/// Spin up a minimal headless ash device with the DMA-BUF import set
/// and import each fd as a `VkImage`. Cleans up everything before
/// returning so the test process exits with no Vulkan leaks.
fn import_into_fresh_vulkan(
    width: u32,
    height: u32,
    stride: u32,
    modifier: u64,
    plane_offset: u64,
    fds: Vec<OwnedFd>,
) -> Result<()> {
    let entry = unsafe { Entry::load().context("load libvulkan.so")? };

    let app_name = CString::new("iter7").unwrap();
    let app_info = vk::ApplicationInfo::default()
        .application_name(app_name.as_c_str())
        .api_version(vk::make_api_version(0, 1, 2, 0));
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
            .context("vkCreateInstance")?
    };

    let result = (|| -> Result<()> {
        let phys_devices = unsafe {
            instance
                .enumerate_physical_devices()
                .context("enumerate_physical_devices")?
        };
        let phys = phys_devices
            .into_iter()
            .find(|&pd| has_dma_buf_extensions(&instance, pd))
            .ok_or_else(|| anyhow!("no device with DMA-BUF import extensions"))?;

        // Find a graphics queue family.
        let families = unsafe { instance.get_physical_device_queue_family_properties(phys) };
        let gfx_family = families
            .iter()
            .position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS))
            .ok_or_else(|| anyhow!("no graphics queue family"))? as u32;

        let priorities = [1.0f32];
        let queue_infos = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(gfx_family)
            .queue_priorities(&priorities)];
        let device_exts = [
            vk::KHR_EXTERNAL_MEMORY_FD_NAME.as_ptr(),
            vk::EXT_EXTERNAL_MEMORY_DMA_BUF_NAME.as_ptr(),
            vk::EXT_IMAGE_DRM_FORMAT_MODIFIER_NAME.as_ptr(),
        ];
        let features = vk::PhysicalDeviceFeatures::default();
        let device_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_infos)
            .enabled_extension_names(&device_exts)
            .enabled_features(&features);
        let device = unsafe {
            instance
                .create_device(phys, &device_info, None)
                .context("vkCreateDevice")?
        };

        let mem_props = unsafe { instance.get_physical_device_memory_properties(phys) };
        let mut images: Vec<vk::Image> = Vec::with_capacity(3);
        let mut memories: Vec<vk::DeviceMemory> = Vec::with_capacity(3);

        for (i, fd) in fds.into_iter().enumerate() {
            let plane_layouts = [vk::SubresourceLayout {
                offset: plane_offset,
                size: 0,
                row_pitch: stride as u64,
                array_pitch: 0,
                depth_pitch: 0,
            }];
            let mut explicit = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
                .drm_format_modifier(modifier)
                .plane_layouts(&plane_layouts);

            let handle_types = vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT;
            let mut external_info =
                vk::ExternalMemoryImageCreateInfo::default().handle_types(handle_types);

            let image_info = vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(vk::Format::R8G8B8A8_UNORM)
                .extent(vk::Extent3D {
                    width,
                    height,
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

            let raw_fd = fd.into_raw_fd();
            let mem_req = unsafe { device.get_image_memory_requirements(image) };
            let mem_type_index = (0..mem_props.memory_type_count)
                .find(|&i| {
                    let bit = 1u32 << i;
                    let t = mem_props.memory_types[i as usize];
                    (mem_req.memory_type_bits & bit) != 0
                        && t.property_flags
                            .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
                })
                .ok_or_else(|| anyhow!("no DEVICE_LOCAL memory type"))?;

            let mut import_info = vk::ImportMemoryFdInfoKHR::default()
                .handle_type(handle_types)
                .fd(raw_fd);
            let mut dedicated =
                vk::MemoryDedicatedAllocateInfo::default().image(image);
            let alloc_info = vk::MemoryAllocateInfo::default()
                .allocation_size(mem_req.size)
                .memory_type_index(mem_type_index)
                .push_next(&mut import_info)
                .push_next(&mut dedicated);

            let memory = unsafe {
                device
                    .allocate_memory(&alloc_info, None)
                    .with_context(|| format!("import allocate_memory slot {i}"))?
            };
            unsafe {
                device
                    .bind_image_memory(image, memory, 0)
                    .with_context(|| format!("bind_image_memory slot {i}"))?;
            }
            images.push(image);
            memories.push(memory);
        }

        // Cleanup before destroying the device.
        unsafe {
            for img in images {
                device.destroy_image(img, None);
            }
            for mem in memories {
                device.free_memory(mem, None);
            }
            device.destroy_device(None);
        }
        Ok(())
    })();

    unsafe { instance.destroy_instance(None) };
    result
}

fn has_dma_buf_extensions(instance: &Instance, pd: vk::PhysicalDevice) -> bool {
    let Ok(exts) = (unsafe { instance.enumerate_device_extension_properties(pd) }) else {
        return false;
    };
    let need = [
        vk::KHR_EXTERNAL_MEMORY_FD_NAME,
        vk::EXT_EXTERNAL_MEMORY_DMA_BUF_NAME,
        vk::EXT_IMAGE_DRM_FORMAT_MODIFIER_NAME,
    ];
    need.iter().all(|n| {
        exts.iter().any(|e| {
            let nm = unsafe { CStr::from_ptr(e.extension_name.as_ptr()) };
            nm == *n
        })
    })
}
