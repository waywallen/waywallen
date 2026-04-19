#pragma once

#include <cstdint>
#include <memory>
#include <string>

#include <vulkan/vulkan.h>

namespace ww_image {

// Immutable view of the DMA-BUF backing a VkImage slot. Owned by VkProducer;
// the `dmabuf_fd` stays open for the producer's lifetime and is dup'd into
// SCM_RIGHTS messages by the IPC layer.
struct VkSlotLayout {
    int      dmabuf_fd { -1 };
    uint64_t drm_modifier { 0 };
    uint32_t drm_fourcc { 0 };
    uint32_t width { 0 };
    uint32_t height { 0 };
    uint32_t plane_offset { 0 };
    uint32_t stride { 0 }; // bytes per row (rowPitch)
    uint32_t size { 0 };   // total memory size for this plane/image
};

// Encapsulates a minimal Vulkan 1.1 instance+device set up for DMA-BUF export
// and a single VkImage slot. M3 will extend this with staging upload and
// signal-semaphore sync_fd export.
class VkProducer {
public:
    ~VkProducer();
    VkProducer(const VkProducer&)            = delete;
    VkProducer& operator=(const VkProducer&) = delete;

    // Create a producer with one `width` x `height` slot. On failure returns
    // nullptr and populates `*err` with a human-readable reason.
    static std::unique_ptr<VkProducer>
    create(uint32_t width, uint32_t height, std::string* err);

    const VkSlotLayout& layout() const { return layout_; }

    // Copy `data` (tightly packed RGBA8, `width*height*4` bytes) into the
    // slot's DMA-BUF via a staging buffer, transition the image layout to
    // GENERAL and release queue-family ownership to FOREIGN so the external
    // consumer can read it, then export a one-shot sync_file fd for the
    // signal. Caller owns the returned fd (sent via SCM_RIGHTS and then
    // closed). Returns -1 on failure and populates `*err`.
    int upload_and_submit(const uint8_t* data, size_t size, std::string* err);

private:
    VkProducer() = default;

    VkInstance       instance_ { VK_NULL_HANDLE };
    VkPhysicalDevice phys_ { VK_NULL_HANDLE };
    VkDevice         device_ { VK_NULL_HANDLE };
    uint32_t         queue_family_ { 0 };
    VkQueue          queue_ { VK_NULL_HANDLE };
    VkImage          image_ { VK_NULL_HANDLE };
    VkDeviceMemory   memory_ { VK_NULL_HANDLE };

    VkCommandPool    cmd_pool_ { VK_NULL_HANDLE };
    VkCommandBuffer  cmd_ { VK_NULL_HANDLE };
    VkSemaphore      signal_sem_ { VK_NULL_HANDLE };

    VkBuffer         staging_buf_ { VK_NULL_HANDLE };
    VkDeviceMemory   staging_mem_ { VK_NULL_HANDLE };
    void*            staging_map_ { nullptr };
    VkDeviceSize     staging_size_ { 0 };

    VkSlotLayout layout_ {};

    PFN_vkGetMemoryFdKHR                         vkGetMemoryFdKHR_ { nullptr };
    PFN_vkGetSemaphoreFdKHR                      vkGetSemaphoreFdKHR_ { nullptr };
    PFN_vkGetImageDrmFormatModifierPropertiesEXT vkGetImageDrmFormatModifierPropertiesEXT_ { nullptr };
};

} // namespace ww_image
