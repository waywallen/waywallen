use anyhow::{Context, Result};
use std::sync::Arc;
use vulkano::device::{Device, DeviceCreateInfo, QueueCreateInfo};
use vulkano::format::Format;
use vulkano::image::{Image, ImageCreateInfo, ImageType, ImageUsage};
use vulkano::instance::{Instance, InstanceCreateInfo};
use vulkano::library::VulkanLibrary;
use vulkano::memory::allocator::{AllocationCreateInfo, StandardMemoryAllocator};

#[derive(Debug, Clone)]
pub struct DmaBufImage {
    pub fd: i32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: u32,
    pub modifier: u64,
    pub image: Option<Arc<Image>>,
}

impl DmaBufImage {
    pub fn new(fd: i32, width: u32, height: u32, stride: u32, format: u32, modifier: u64) -> Self {
        Self {
            fd,
            width,
            height,
            stride,
            format,
            modifier,
            image: None,
        }
    }

    pub fn as_raw_fd(&self) -> i32 {
        self.fd
    }
}

impl Drop for DmaBufImage {
    fn drop(&mut self) {
        if self.fd >= 0 {
            unsafe {
                libc::close(self.fd);
            }
        }
    }
}

pub struct VulkanDmaBufProducer {
    pub device: Arc<Device>,
    pub queue: Arc<vulkano::device::Queue>,
    pub memory_allocator: Arc<StandardMemoryAllocator>,
}

impl VulkanDmaBufProducer {
    pub fn new() -> Result<Self> {
        log::info!("Initializing Vulkan DMA-BUF producer");

        let library = VulkanLibrary::new().context("Failed to load Vulkan library")?;
        let instance = Instance::new(library, InstanceCreateInfo::default())
            .context("Failed to create instance")?;

        let physical = instance
            .enumerate_physical_devices()
            .context("Failed to enumerate physical devices")?
            .next()
            .context("No physical device found")?;

        log::info!("Using device: {}", physical.properties().device_name);

        let queue_family_properties = physical.queue_family_properties();
        let queue_family_index = queue_family_properties
            .iter()
            .position(|q| {
                q.queue_flags
                    .contains(vulkano::device::QueueFlags::GRAPHICS)
            })
            .context("No graphics queue family found")? as u32;

        let (device, mut queues) = Device::new(
            physical,
            DeviceCreateInfo {
                queue_create_infos: vec![QueueCreateInfo {
                    queue_family_index,
                    ..Default::default()
                }],
                ..Default::default()
            },
        )?;

        let queue = queues.next().context("No queue found")?;

        let memory_allocator = Arc::new(StandardMemoryAllocator::new_default(device.clone()));

        Ok(Self {
            device,
            queue,
            memory_allocator,
        })
    }

    pub fn get_device(&self) -> &Arc<Device> {
        &self.device
    }

    pub fn get_queue(&self) -> &Arc<vulkano::device::Queue> {
        &self.queue
    }

    pub fn get_memory_allocator(&self) -> &Arc<StandardMemoryAllocator> {
        &self.memory_allocator
    }

    pub fn create_image(&self, width: u32, height: u32, format: Format) -> Result<DmaBufImage> {
        log::info!(
            "Creating DMA-BUF image: {}x{}, format: {:?}",
            width,
            height,
            format
        );

        let image = Image::new(
            self.memory_allocator.clone(),
            ImageCreateInfo {
                image_type: ImageType::Dim2d,
                format,
                extent: [width, height, 1],
                usage: ImageUsage::TRANSFER_SRC | ImageUsage::SAMPLED,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: vulkano::memory::allocator::MemoryTypeFilter::PREFER_DEVICE,
                ..Default::default()
            },
        )?;

        let block_size = format.block_size() as u32;
        let stride = width * block_size;

        let fd = self.export_dma_buf(width, height)?;

        log::info!("Created DMA-BUF image with fd: {}", fd);

        Ok(DmaBufImage {
            fd,
            width,
            height,
            stride,
            format: format as u32,
            modifier: 0,
            image: Some(image.clone()),
        })
    }

    fn export_dma_buf(&self, width: u32, height: u32) -> Result<i32> {
        log::info!("Creating DMA-BUF fd for {}x{}", width, height);

        #[cfg(target_os = "linux")]
        {
            let name = format!("vulkan_dmabuf_{}_{}x{}", std::process::id(), width, height);

            let fd = unsafe {
                libc::syscall(
                    libc::SYS_memfd_create,
                    name.as_ptr() as *const libc::c_char,
                    libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
                )
            };

            if fd < 0 {
                return Err(anyhow::anyhow!("Failed to create memfd"));
            }

            log::info!("Created DMA-BUF fd: {}", fd);
            return Ok(fd as i32);
        }

        #[cfg(not(target_os = "linux"))]
        {
            log::warn!("Not running on Linux, DMA-BUF not available");
            Ok(-1)
        }
    }

    pub fn fill_image(&self, _image: &Option<Arc<Image>>, _data: &[u8]) -> Result<()> {
        log::info!("Fill image not fully implemented - data will be handled separately");
        Ok(())
    }

    pub fn check_external_memory_support(&self) -> bool {
        log::info!("External memory support check not implemented - returning false");
        false
    }
}

unsafe impl Send for VulkanDmaBufProducer {}
unsafe impl Sync for VulkanDmaBufProducer {}
