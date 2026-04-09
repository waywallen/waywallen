use waywallen::vulkan_dma_buf::VulkanDmaBufProducer;
use vulkano::format::Format;

#[test]
fn test_vulkan_initialization() {
    let result = VulkanDmaBufProducer::new();

    match result {
        Ok(_producer) => {
            println!("Vulkan initialized successfully");
        }
        Err(e) => {
            println!("Vulkan initialization failed (expected if no GPU): {}", e);
        }
    }
}

#[test]
fn test_create_dma_buf_image() {
    let result = VulkanDmaBufProducer::new();

    if let Ok(producer) = result {
        let image_result = producer.create_image(1920, 1080, Format::R8G8B8A8_UNORM);

        match image_result {
            Ok(dma_buf) => {
                println!("Created DMA-BUF: {}x{}", dma_buf.width, dma_buf.height);
                println!("FD: {}", dma_buf.fd);
                println!("Stride: {}", dma_buf.stride);
            }
            Err(e) => {
                println!("Failed to create DMA-BUF: {}", e);
            }
        }
    }
}

#[test]
fn test_dma_buf_fd_creation() {
    let result = VulkanDmaBufProducer::new();

    if let Ok(producer) = result {
        let width = 640u32;
        let height = 480u32;

        if let Ok(dma_buf) = producer.create_image(width, height, Format::R8G8B8A8_UNORM) {
            assert!(dma_buf.fd >= 0, "FD should be valid");
            assert_eq!(dma_buf.width, width);
            assert_eq!(dma_buf.height, height);
            assert!(dma_buf.stride > 0, "Stride should be positive");
        }
    }
}

#[test]
fn test_multiple_image_creation() {
    let result = VulkanDmaBufProducer::new();

    if let Ok(producer) = result {
        let sizes = [(1920, 1080), (1280, 720), (640, 480)];

        for (width, height) in sizes {
            let image_result = producer.create_image(width, height, Format::R8G8B8A8_UNORM);

            match image_result {
                Ok(dma_buf) => {
                    println!("Created {}x{}: fd={}", width, height, dma_buf.fd);
                }
                Err(e) => {
                    println!("Failed to create {}x{}: {}", width, height, e);
                }
            }
        }
    }
}

#[test]
fn test_dma_buf_image_properties() {
    let result = VulkanDmaBufProducer::new();

    if let Ok(producer) = result {
        let width = 1920u32;
        let height = 1080u32;

        match producer.create_image(width, height, Format::R8G8B8A8_UNORM) {
            Ok(dma_buf) => {
                println!("DMA-BUF properties:");
                println!("  Width: {}", dma_buf.width);
                println!("  Height: {}", dma_buf.height);
                println!("  Stride: {}", dma_buf.stride);
                println!("  Format: {}", dma_buf.format);
                println!("  FD: {}", dma_buf.fd);
                println!("  Modifier: {}", dma_buf.modifier);

                assert_eq!(dma_buf.width, width);
                assert_eq!(dma_buf.height, height);
                assert!(dma_buf.stride > 0);
                assert!(dma_buf.fd >= 0);

                println!("Image properties test PASSED");
            }
            Err(e) => {
                println!("Failed to create DMA-BUF: {}", e);
            }
        }
    }
}

#[test]
fn test_dma_buf_various_formats() {
    let result = VulkanDmaBufProducer::new();

    if let Ok(producer) = result {
        let formats = [
            ("R8G8B8A8_UNORM", Format::R8G8B8A8_UNORM),
            ("B8G8R8A8_UNORM", Format::B8G8R8A8_UNORM),
        ];

        for (name, format) in formats {
            match producer.create_image(640, 480, format) {
                Ok(dma_buf) => {
                    println!(
                        "Format {}: fd={}, stride={}",
                        name, dma_buf.fd, dma_buf.stride
                    );
                }
                Err(e) => {
                    println!("Format {} failed: {}", name, e);
                }
            }
        }
    }
}
