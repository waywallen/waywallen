/* waywallen-bridge — Vulkan pool backend.
 *
 * Owns: per-slot VkImage + VkDeviceMemory + dmabuf fd. Bridge does
 * not link libvulkan — function pointers come from the plugin's
 * vkGetInstanceProcAddr.
 *
 * Plugin still owns: VkInstance, VkPhysicalDevice, VkDevice, VkQueue,
 * its own staging buffer + command pool. Plugin uploads pixels into
 * the slot's VkImage by issuing transfer commands targeting the
 * VkImage handle returned from `populate_slot_view`. */

#include <waywallen-bridge/pool.h>
#include <waywallen-bridge/protocol_bits.h>
#include <waywallen-bridge/probe_vk.h>
#include <waywallen-bridge/drm_fourcc.h>

#include "log_internal.h"
#include "pool_internal.h"
#include "sync_release.h"

#include <vulkan/vulkan.h>

#include <errno.h>
#include <fcntl.h>
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

/* RADV GFX12 rejects NVIDIA's 64-byte LINEAR pitch on explicit import.
 * CompatLinear uses 256 bytes as the shared cross-vendor layout contract. */
#define DRM_FORMAT_MOD_LINEAR                0ULL
#define WW_COMPAT_LINEAR_ROW_PITCH_ALIGNMENT 256ULL

typedef struct vk_state {
    /* Plugin-owned. */
    VkInstance       instance;
    VkPhysicalDevice phys;
    VkDevice         device;
    VkQueue          queue;
    uint32_t         queue_family;

    /* Resolved through vkGetInstanceProcAddr / vkGetDeviceProcAddr. */
    PFN_vkGetInstanceProcAddr                    vkGetInstanceProcAddr;
    PFN_vkGetDeviceProcAddr                      vkGetDeviceProcAddr;
    PFN_vkGetPhysicalDeviceProperties2           vkGetPhysicalDeviceProperties2;
    PFN_vkGetPhysicalDeviceFormatProperties2     vkGetPhysicalDeviceFormatProperties2;
    PFN_vkGetPhysicalDeviceMemoryProperties      vkGetPhysicalDeviceMemoryProperties;
    PFN_vkCreateImage                            vkCreateImage;
    PFN_vkDestroyImage                           vkDestroyImage;
    PFN_vkGetImageMemoryRequirements2            vkGetImageMemoryRequirements2;
    PFN_vkAllocateMemory                         vkAllocateMemory;
    PFN_vkFreeMemory                             vkFreeMemory;
    PFN_vkBindImageMemory                        vkBindImageMemory;
    PFN_vkGetImageSubresourceLayout              vkGetImageSubresourceLayout;
    PFN_vkGetMemoryFdKHR                         vkGetMemoryFdKHR;
    PFN_vkGetImageDrmFormatModifierPropertiesEXT vkGetImageDrmFormatModifierPropertiesEXT;
    PFN_vkDeviceWaitIdle                         vkDeviceWaitIdle;

    /* Producer-configured slot image usage. Populated from
     * ww_pool_vulkan_init_t::image_usage_flags (defaulted in
     * backend_init when caller passed 0). Used at vkCreateImage
     * time only; the modifier filter does not derive features
     * from this. */
    VkImageUsageFlags image_usage;

    /* Format features the negotiated DRM modifier must support.
     * Populated from ww_pool_vulkan_init_t::format_feature_flags
     * (defaulted to TRANSFER_DST_BIT when the caller passes 0).
     * Sole input to the advertise-time modifier filter. */
    VkFormatFeatureFlags format_features;

    /* False on GPUs without VK_EXT_image_drm_format_modifier (e.g. RADV
     * GFX8/Tonga): slots are plain VK_IMAGE_TILING_LINEAR dma-bufs. */
    bool have_modifier_ext;

    /* Per-slot resources. */
    struct {
        VkImage        image;
        VkDeviceMemory memory;
    } slots[WW_POOL_MAX_SLOTS];
} vk_state_t;

static int load_dispatch(vk_state_t* st, PFN_vkGetInstanceProcAddr getipa) {
    st->vkGetInstanceProcAddr = getipa;
#define LOAD_INSTANCE(name) st->name = (PFN_##name)getipa(st->instance, #name)
    LOAD_INSTANCE(vkGetDeviceProcAddr);
    LOAD_INSTANCE(vkGetPhysicalDeviceProperties2);
    LOAD_INSTANCE(vkGetPhysicalDeviceFormatProperties2);
    LOAD_INSTANCE(vkGetPhysicalDeviceMemoryProperties);
#undef LOAD_INSTANCE
    if (! st->vkGetDeviceProcAddr) return -ENOSYS;

#define LOAD_DEVICE(name) st->name = (PFN_##name)st->vkGetDeviceProcAddr(st->device, #name)
    LOAD_DEVICE(vkCreateImage);
    LOAD_DEVICE(vkDestroyImage);
    LOAD_DEVICE(vkGetImageMemoryRequirements2);
    LOAD_DEVICE(vkAllocateMemory);
    LOAD_DEVICE(vkFreeMemory);
    LOAD_DEVICE(vkBindImageMemory);
    LOAD_DEVICE(vkGetImageSubresourceLayout);
    LOAD_DEVICE(vkGetMemoryFdKHR);
    LOAD_DEVICE(vkGetImageDrmFormatModifierPropertiesEXT);
    LOAD_DEVICE(vkDeviceWaitIdle);
#undef LOAD_DEVICE

    if (! st->vkCreateImage || ! st->vkDestroyImage || ! st->vkGetImageMemoryRequirements2 ||
        ! st->vkAllocateMemory || ! st->vkFreeMemory || ! st->vkBindImageMemory ||
        ! st->vkGetImageSubresourceLayout || ! st->vkGetMemoryFdKHR ||
        ! st->vkGetPhysicalDeviceFormatProperties2 || ! st->vkGetPhysicalDeviceMemoryProperties) {
        return -ENOSYS;
    }
    st->have_modifier_ext = st->vkGetImageDrmFormatModifierPropertiesEXT != NULL;
    return 0;
}

/* fourcc → VkFormat. Mirrors waywallen-display/src/backend_vulkan.c
 * s_vk_fourcc_table — producer and consumer must agree on the same
 * advertised set, since the daemon negotiator compares fourccs by
 * exact integer. */
struct vk_fourcc_entry {
    uint32_t fourcc;
    VkFormat vk_format;
    uint32_t bytes_per_pixel;
};
static const struct vk_fourcc_entry s_vk_fourcc_table[] = {
    { WW_DRM_FORMAT_ABGR8888, VK_FORMAT_R8G8B8A8_UNORM, 4 },
    { WW_DRM_FORMAT_XBGR8888, VK_FORMAT_R8G8B8A8_UNORM, 4 },
    { WW_DRM_FORMAT_ARGB8888, VK_FORMAT_B8G8R8A8_UNORM, 4 },
    { WW_DRM_FORMAT_XRGB8888, VK_FORMAT_B8G8R8A8_UNORM, 4 },
    { WW_DRM_FORMAT_RGBA8888, VK_FORMAT_R8G8B8A8_UNORM, 4 },
    { WW_DRM_FORMAT_BGRA8888, VK_FORMAT_B8G8R8A8_UNORM, 4 },
    { WW_DRM_FORMAT_RGBX8888, VK_FORMAT_R8G8B8A8_UNORM, 4 },
    { WW_DRM_FORMAT_BGRX8888, VK_FORMAT_B8G8R8A8_UNORM, 4 },
};

static const struct vk_fourcc_entry* find_fourcc(uint32_t fourcc) {
    for (size_t i = 0; i < sizeof(s_vk_fourcc_table) / sizeof(s_vk_fourcc_table[0]); ++i) {
        if (s_vk_fourcc_table[i].fourcc == fourcc) return &s_vk_fourcc_table[i];
    }
    return NULL;
}

static int probe_caps(ww_pool_t* pool, uint32_t width, uint32_t height) {
    (void)width;
    (void)height; /* Vulkan modifiers don't depend on extent at probe time. */
    vk_state_t* st = (vk_state_t*)pool->backend_data;

    /* Walk every candidate fourcc, pull the modifier list per-format,
     * and keep only the modifiers whose tilingFeatures cover the
     * producer and consumer usage contract. */
    const VkFormatFeatureFlags want_features = st->format_features;

    /* Worst-case sizing: every fourcc × every modifier. We grow the
     * entries array dynamically as we go. */
    ww_format_entry_t* entries       = NULL;
    size_t             entries_count = 0;
    size_t             entries_cap   = 0;

    for (size_t fi = 0; fi < sizeof(s_vk_fourcc_table) / sizeof(s_vk_fourcc_table[0]); ++fi) {
        uint32_t fourcc    = s_vk_fourcc_table[fi].fourcc;
        VkFormat vk_format = s_vk_fourcc_table[fi].vk_format;

        if (! st->have_modifier_ext) {
            /* No modifier extension: advertise (fourcc, LINEAR) when the
             * format supports the required features with linear tiling. */
            VkFormatProperties2 lp = { 0 };
            lp.sType               = VK_STRUCTURE_TYPE_FORMAT_PROPERTIES_2;
            st->vkGetPhysicalDeviceFormatProperties2(st->phys, vk_format, &lp);
            if ((lp.formatProperties.linearTilingFeatures & want_features) != want_features)
                continue;
            if (entries_count == entries_cap) {
                size_t             new_cap = entries_cap ? entries_cap * 2 : 16;
                ww_format_entry_t* grow =
                    (ww_format_entry_t*)realloc(entries, new_cap * sizeof(*grow));
                if (! grow) {
                    free(entries);
                    return -ENOMEM;
                }
                entries     = grow;
                entries_cap = new_cap;
            }
            entries[entries_count].fourcc      = fourcc;
            entries[entries_count].modifier    = DRM_FORMAT_MOD_LINEAR;
            entries[entries_count].plane_count = 1;
            ++entries_count;
            continue;
        }

        /* Two-call enumeration of modifier list for this fourcc. */
        VkDrmFormatModifierPropertiesListEXT mod_list = { 0 };
        mod_list.sType          = VK_STRUCTURE_TYPE_DRM_FORMAT_MODIFIER_PROPERTIES_LIST_EXT;
        VkFormatProperties2 fp2 = { 0 };
        fp2.sType               = VK_STRUCTURE_TYPE_FORMAT_PROPERTIES_2;
        fp2.pNext               = &mod_list;
        st->vkGetPhysicalDeviceFormatProperties2(st->phys, vk_format, &fp2);

        if (mod_list.drmFormatModifierCount == 0) continue;

        VkDrmFormatModifierPropertiesEXT* probed = (VkDrmFormatModifierPropertiesEXT*)calloc(
            mod_list.drmFormatModifierCount, sizeof(*probed));
        if (! probed) {
            free(entries);
            return -ENOMEM;
        }
        mod_list.pDrmFormatModifierProperties = probed;
        st->vkGetPhysicalDeviceFormatProperties2(st->phys, vk_format, &fp2);

        for (uint32_t i = 0; i < mod_list.drmFormatModifierCount; ++i) {
            VkFormatFeatureFlags ff = probed[i].drmFormatModifierTilingFeatures;
            if ((ff & want_features) != want_features) continue;

            if (entries_count == entries_cap) {
                size_t             new_cap = entries_cap ? entries_cap * 2 : 16;
                ww_format_entry_t* grow =
                    (ww_format_entry_t*)realloc(entries, new_cap * sizeof(*grow));
                if (! grow) {
                    free(probed);
                    free(entries);
                    return -ENOMEM;
                }
                entries     = grow;
                entries_cap = new_cap;
            }
            entries[entries_count].fourcc      = fourcc;
            entries[entries_count].modifier    = probed[i].drmFormatModifier;
            entries[entries_count].plane_count = probed[i].drmFormatModifierPlaneCount;
            ++entries_count;
        }
        free(probed);
    }

    if (entries_count == 0) {
        ww_bridge_logf(WW_BRIDGE_LOG_WARN,
                       "ww_pool[vulkan]: no (fourcc, modifier) pairs satisfy producer "
                       "format_features=0x%x — falling back to single LINEAR ABGR8888",
                       st->format_features);
        free(entries);
        ww_format_entry_t* ent = (ww_format_entry_t*)calloc(1, sizeof(*ent));
        if (! ent) return -ENOMEM;
        ent[0].fourcc      = WW_DRM_FORMAT_ABGR8888;
        ent[0].modifier    = DRM_FORMAT_MOD_LINEAR;
        ent[0].plane_count = 1;
        pool->caps.entries = ent;
        pool->caps.count   = 1;
    } else {
        pool->caps.entries = entries;
        pool->caps.count   = entries_count;
    }
    /* Fill device/driver UUID. */
    if (st->vkGetPhysicalDeviceProperties2) {
        VkPhysicalDeviceIDProperties id_props = { 0 };
        id_props.sType                        = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_ID_PROPERTIES;
        VkPhysicalDeviceProperties2 pd2       = { 0 };
        pd2.sType                             = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2;
        pd2.pNext                             = &id_props;
        st->vkGetPhysicalDeviceProperties2(st->phys, &pd2);
        memcpy(pool->caps.device_uuid, id_props.deviceUUID, 16);
        memcpy(pool->caps.driver_uuid, id_props.driverUUID, 16);
        pool->caps.have_uuid = true;
    }

    pool->caps.sync_caps    = WW_SYNC_SYNCOBJ_TIMELINE;
    pool->caps.color_caps   = WW_COLOR_ENC_SRGB | WW_COLOR_RANGE_LIMITED | WW_COLOR_ALPHA_PREMUL;
    pool->caps.extent_max_w = 16384;
    pool->caps.extent_max_h = 16384;
    return 0;
}

/* Pick a memory type from the producer device that can back an
 * exportable VkImage.
 *
 * `prefer_host_visible` is the cross-vendor switch. When the daemon
 * negotiates the COMPAT_LINEAR path (consumer is on a different vendor),
 * VRAM is the wrong answer: the dma-buf points at producer-only memory
 * the importing GPU cannot reference in a CS submit. radv reports this
 * as "Not enough memory for command submission" and loses the device.
 * See cross_gpu.md and the test path `waywallen --test --test-gpus 1,0`.
 *
 * For OPTIMIZED same-GPU paths, DEVICE_LOCAL is still the right answer
 * (faster, and the consumer's GPU can read its own VRAM via PCIe).
 *
 * Returns UINT32_MAX only if typeBits is empty. */
static uint32_t pick_memory_type(vk_state_t* st, uint32_t type_bits, bool prefer_host_visible) {
    VkPhysicalDeviceMemoryProperties mp = { 0 };
    st->vkGetPhysicalDeviceMemoryProperties(st->phys, &mp);

    if (prefer_host_visible) {
        /* Pass 1: HOST_VISIBLE && !DEVICE_LOCAL — true GTT/sysmem,
         * the only PRIME-importable type cross-vendor. */
        for (uint32_t i = 0; i < mp.memoryTypeCount; ++i) {
            if (! (type_bits & (1u << i))) continue;
            VkMemoryPropertyFlags f = mp.memoryTypes[i].propertyFlags;
            if ((f & VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT) &&
                ! (f & VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT)) {
                return i;
            }
        }
        /* Pass 2: any HOST_VISIBLE (BAR is acceptable on some HW). */
        for (uint32_t i = 0; i < mp.memoryTypeCount; ++i) {
            if (! (type_bits & (1u << i))) continue;
            if (mp.memoryTypes[i].propertyFlags & VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT) {
                return i;
            }
        }
        /* Pass 3: any matching type (last resort; dma-buf may still
         * be importable depending on the driver pair). */
    } else {
        /* Pass 1: prefer DEVICE_LOCAL (fast path on same-GPU). */
        for (uint32_t i = 0; i < mp.memoryTypeCount; ++i) {
            if (! (type_bits & (1u << i))) continue;
            if (mp.memoryTypes[i].propertyFlags & VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT) {
                return i;
            }
        }
    }
    /* Final pass: any allowed type. */
    for (uint32_t i = 0; i < mp.memoryTypeCount; ++i) {
        if (type_bits & (1u << i)) return i;
    }
    return UINT32_MAX;
}

static int alloc_slot(ww_pool_t* pool, uint32_t slot_index, ww_pool_slot_layout_t* out) {
    vk_state_t* st = (vk_state_t*)pool->backend_data;
    if (slot_index >= WW_POOL_MAX_SLOTS) return -EINVAL;

    const waywallen_buffer_directive_t* d      = &pool->cur.directive;
    const waywallen_extent_t*           extent = &pool->cur.extent;

    /* For the COMPAT_LINEAR path we override the modifier to LINEAR
     * regardless of what the directive says, since GPU_LINEAR is the
     * Vulkan analogue of `gbm_bo_create(USE_LINEAR)`. For OPTIMIZED
     * paths we use whatever modifier the daemon picked. */
    bool     linear_path  = (d->path == WAYWALLEN_BUFFER_PATH_COMPAT_LINEAR) ||
                            (d->memory_source == WAYWALLEN_BUFFER_MEMORY_SOURCE_GPU_LINEAR);
    if (! st->have_modifier_ext) linear_path = true;
    uint64_t modifiers[1] = { linear_path ? DRM_FORMAT_MOD_LINEAR : d->format.modifier };

    VkImageDrmFormatModifierListCreateInfoEXT mod_list = { 0 };
    mod_list.sType = VK_STRUCTURE_TYPE_IMAGE_DRM_FORMAT_MODIFIER_LIST_CREATE_INFO_EXT;
    mod_list.drmFormatModifierCount = 1;
    mod_list.pDrmFormatModifiers    = modifiers;

    const struct vk_fourcc_entry* format = find_fourcc(d->format.fourcc);
    if (! format) {
        ww_bridge_logf(WW_BRIDGE_LOG_ERROR,
                       "ww_pool[vulkan]: directive fourcc 0x%08x has no VkFormat mapping",
                       d->format.fourcc);
        return -EINVAL;
    }

    VkSubresourceLayout                           explicit_plane       = { 0 };
    VkImageDrmFormatModifierExplicitCreateInfoEXT explicit_modifier    = { 0 };
    const void*                                   modifier_create_info = &mod_list;
    if (linear_path) {
        if (d->format.plane_count != 1) {
            ww_bridge_logf(WW_BRIDGE_LOG_ERROR,
                           "ww_pool[vulkan]: COMPAT_LINEAR requires one plane, got %u",
                           d->format.plane_count);
            return -ENOTSUP;
        }
        VkDeviceSize min_row_pitch = (VkDeviceSize)extent->width * format->bytes_per_pixel;
        explicit_plane.rowPitch    = (min_row_pitch + WW_COMPAT_LINEAR_ROW_PITCH_ALIGNMENT - 1) &
                                     ~(WW_COMPAT_LINEAR_ROW_PITCH_ALIGNMENT - 1);

        explicit_modifier.sType =
            VK_STRUCTURE_TYPE_IMAGE_DRM_FORMAT_MODIFIER_EXPLICIT_CREATE_INFO_EXT;
        explicit_modifier.drmFormatModifier           = DRM_FORMAT_MOD_LINEAR;
        explicit_modifier.drmFormatModifierPlaneCount = 1;
        explicit_modifier.pPlaneLayouts               = &explicit_plane;
        modifier_create_info                          = &explicit_modifier;
    }

    if (! st->have_modifier_ext) modifier_create_info = NULL;

    VkExternalMemoryImageCreateInfo ext_img = { 0 };
    ext_img.sType                           = VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO;
    ext_img.pNext                           = modifier_create_info;
    ext_img.handleTypes                     = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT;

    VkImageCreateInfo img_ci = { 0 };
    img_ci.sType             = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO;
    img_ci.pNext             = &ext_img;
    img_ci.imageType         = VK_IMAGE_TYPE_2D;
    img_ci.format            = format->vk_format;
    img_ci.extent.width      = extent->width;
    img_ci.extent.height     = extent->height;
    img_ci.extent.depth      = 1;
    img_ci.mipLevels         = 1;
    img_ci.arrayLayers       = 1;
    img_ci.samples           = VK_SAMPLE_COUNT_1_BIT;
    img_ci.tiling            = st->have_modifier_ext ? VK_IMAGE_TILING_DRM_FORMAT_MODIFIER_EXT
                                                     : VK_IMAGE_TILING_LINEAR;
    /* Producer and consumer must use the same modifier usage contract;
     * mismatches can select incompatible compressed sub-layouts. */
    img_ci.usage         = st->image_usage;
    img_ci.sharingMode   = VK_SHARING_MODE_EXCLUSIVE;
    img_ci.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;

    VkImage  image = VK_NULL_HANDLE;
    VkResult r     = st->vkCreateImage(st->device, &img_ci, NULL, &image);
    if (r != VK_SUCCESS) {
        ww_bridge_logf(WW_BRIDGE_LOG_ERROR,
                       "ww_pool[vulkan]: vkCreateImage failed (modifier=0x%016llx linear=%d "
                       "row_pitch=%llu): %d",
                       (unsigned long long)modifiers[0],
                       linear_path ? 1 : 0,
                       (unsigned long long)explicit_plane.rowPitch,
                       r);
        return -EIO;
    }

    VkImageMemoryRequirementsInfo2 mri = { 0 };
    mri.sType                          = VK_STRUCTURE_TYPE_IMAGE_MEMORY_REQUIREMENTS_INFO_2;
    mri.image                          = image;
    VkMemoryRequirements2 mr           = { 0 };
    mr.sType                           = VK_STRUCTURE_TYPE_MEMORY_REQUIREMENTS_2;
    st->vkGetImageMemoryRequirements2(st->device, &mri, &mr);

    uint32_t mem_type = pick_memory_type(st, mr.memoryRequirements.memoryTypeBits, linear_path);
    if (mem_type == UINT32_MAX) {
        ww_bridge_logf(WW_BRIDGE_LOG_ERROR,
                       "ww_pool[vulkan]: no memory type matches image typeBits=0x%x "
                       "(linear_path=%d)",
                       mr.memoryRequirements.memoryTypeBits,
                       (int)linear_path);
        st->vkDestroyImage(st->device, image, NULL);
        return -EIO;
    }
    if (linear_path) {
        VkPhysicalDeviceMemoryProperties mp = { 0 };
        st->vkGetPhysicalDeviceMemoryProperties(st->phys, &mp);
        VkMemoryPropertyFlags pf = mp.memoryTypes[mem_type].propertyFlags;
        if (! (pf & VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT)) {
            /* Last-resort fallback inside pick_memory_type picked a
             * non-HOST_VISIBLE type. The cross-vendor consumer is very
             * likely to fail to import this dma-buf for GPU use. Warn
             * loudly so the operator sees it before the consumer dies. */
            ww_bridge_logf(WW_BRIDGE_LOG_WARN,
                           "ww_pool[vulkan]: COMPAT_LINEAR slot fell back to "
                           "non-HOST_VISIBLE memtype %u (flags=0x%x); cross-vendor "
                           "consumers may fail. typeBits=0x%x",
                           mem_type,
                           (unsigned)pf,
                           mr.memoryRequirements.memoryTypeBits);
        }
    }

    VkMemoryDedicatedAllocateInfo ded = { 0 };
    ded.sType                         = VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO;
    ded.image                         = image;

    VkExportMemoryAllocateInfo exp = { 0 };
    exp.sType                      = VK_STRUCTURE_TYPE_EXPORT_MEMORY_ALLOCATE_INFO;
    exp.pNext                      = &ded;
    exp.handleTypes                = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT;

    VkMemoryAllocateInfo mai = { 0 };
    mai.sType                = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO;
    mai.pNext                = &exp;
    mai.allocationSize       = mr.memoryRequirements.size;
    mai.memoryTypeIndex      = mem_type;

    VkDeviceMemory memory = VK_NULL_HANDLE;
    r                     = st->vkAllocateMemory(st->device, &mai, NULL, &memory);
    if (r != VK_SUCCESS) {
        ww_bridge_logf(WW_BRIDGE_LOG_ERROR, "ww_pool[vulkan]: vkAllocateMemory failed: %d", r);
        st->vkDestroyImage(st->device, image, NULL);
        return -ENOMEM;
    }
    r = st->vkBindImageMemory(st->device, image, memory, 0);
    if (r != VK_SUCCESS) {
        ww_bridge_logf(WW_BRIDGE_LOG_ERROR, "ww_pool[vulkan]: vkBindImageMemory failed: %d", r);
        st->vkFreeMemory(st->device, memory, NULL);
        st->vkDestroyImage(st->device, image, NULL);
        return -EIO;
    }

    VkMemoryGetFdInfoKHR fdi = { 0 };
    fdi.sType                = VK_STRUCTURE_TYPE_MEMORY_GET_FD_INFO_KHR;
    fdi.memory               = memory;
    fdi.handleType           = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT;
    int fd                   = -1;
    r                        = st->vkGetMemoryFdKHR(st->device, &fdi, &fd);
    if (r != VK_SUCCESS) {
        ww_bridge_logf(WW_BRIDGE_LOG_ERROR, "ww_pool[vulkan]: vkGetMemoryFdKHR failed: %d", r);
        st->vkFreeMemory(st->device, memory, NULL);
        st->vkDestroyImage(st->device, image, NULL);
        return -EIO;
    }

    /* Read back the actual modifier + per-plane layouts. The directive
     * tells us how many memory planes the modifier defines; LINEAR is
     * always 1, AMD DCC w/o RETILE = 2, DCC w/ RETILE = 3, etc. */
    VkImageDrmFormatModifierPropertiesEXT mod_props = { 0 };
    mod_props.sType = VK_STRUCTURE_TYPE_IMAGE_DRM_FORMAT_MODIFIER_PROPERTIES_EXT;
    if (st->have_modifier_ext) {
        st->vkGetImageDrmFormatModifierPropertiesEXT(st->device, image, &mod_props);
    } else {
        mod_props.drmFormatModifier = DRM_FORMAT_MOD_LINEAR;
    }

    uint32_t plane_count = linear_path ? 1u : d->format.plane_count;
    if (plane_count == 0) plane_count = 1;
    if (plane_count > WW_POOL_MAX_PLANES) {
        ww_bridge_logf(WW_BRIDGE_LOG_ERROR,
                       "ww_pool[vulkan]: alloc_slot[%u]: directive plane_count=%u "
                       "exceeds WW_POOL_MAX_PLANES=%d",
                       slot_index,
                       plane_count,
                       WW_POOL_MAX_PLANES);
        close(fd);
        st->vkFreeMemory(st->device, memory, NULL);
        st->vkDestroyImage(st->device, image, NULL);
        return -ENOSPC;
    }

    static const VkImageAspectFlagBits kAspects[WW_POOL_MAX_PLANES] = {
        VK_IMAGE_ASPECT_MEMORY_PLANE_0_BIT_EXT,
        VK_IMAGE_ASPECT_MEMORY_PLANE_1_BIT_EXT,
        VK_IMAGE_ASPECT_MEMORY_PLANE_2_BIT_EXT,
        VK_IMAGE_ASPECT_MEMORY_PLANE_3_BIT_EXT,
    };
    uint32_t plane_strides[WW_POOL_MAX_PLANES] = { 0 };
    uint32_t plane_offsets[WW_POOL_MAX_PLANES] = { 0 };
    uint64_t plane_sizes[WW_POOL_MAX_PLANES]   = { 0 };
    for (uint32_t p = 0; p < plane_count; ++p) {
        VkImageSubresource s   = { 0 };
        s.aspectMask           = st->have_modifier_ext ? kAspects[p] : VK_IMAGE_ASPECT_COLOR_BIT;
        VkSubresourceLayout pl = { 0 };
        st->vkGetImageSubresourceLayout(st->device, image, &s, &pl);
        plane_strides[p] = (uint32_t)pl.rowPitch;
        plane_offsets[p] = (uint32_t)pl.offset;
        plane_sizes[p]   = (uint64_t)pl.size;
    }

    ww_bridge_logf(WW_BRIDGE_LOG_DEBUG,
                   "ww_pool[vulkan]: alloc_slot[%u] %ux%u fourcc=0x%08x "
                   "mod=0x%016llx linear=%d planes=%u mem_size=%llu",
                   slot_index,
                   extent->width,
                   extent->height,
                   d->format.fourcc,
                   (unsigned long long)mod_props.drmFormatModifier,
                   linear_path ? 1 : 0,
                   plane_count,
                   (unsigned long long)mr.memoryRequirements.size);
    for (uint32_t p = 0; p < plane_count; ++p) {
        ww_bridge_logf(WW_BRIDGE_LOG_DEBUG,
                       "ww_pool[vulkan]:   plane[%u] stride=%u offset=%u size=%llu",
                       p,
                       plane_strides[p],
                       plane_offsets[p],
                       (unsigned long long)plane_sizes[p]);
    }

    /* Vulkan's non-disjoint dmabuf-import gives a single VkDeviceMemory
     * with one fd; the multi-plane layout is internal to that
     * allocation. The wire protocol carries one fd per plane, so dup
     * the same fd into every plane index. The consumer receives N
     * fds that all reference the same dma-buf and applies the
     * appropriate stride/offset to each. */
    int plane_fds[WW_POOL_MAX_PLANES] = { -1, -1, -1, -1 };
    plane_fds[0]                      = fd;
    for (uint32_t p = 1; p < plane_count; ++p) {
        plane_fds[p] = fcntl(fd, F_DUPFD_CLOEXEC, 3);
        if (plane_fds[p] < 0) {
            ww_bridge_logf(WW_BRIDGE_LOG_ERROR,
                           "ww_pool[vulkan]: F_DUPFD_CLOEXEC for plane %u failed: %s",
                           p,
                           strerror(errno));
            for (uint32_t q = 0; q < p; ++q) close(plane_fds[q]);
            st->vkFreeMemory(st->device, memory, NULL);
            st->vkDestroyImage(st->device, image, NULL);
            return -EIO;
        }
    }

    st->slots[slot_index].image  = image;
    st->slots[slot_index].memory = memory;

    out->plane_count = plane_count;
    for (uint32_t p = 0; p < plane_count; ++p) {
        out->fds[p]           = plane_fds[p];
        out->strides[p]       = plane_strides[p];
        out->plane_offsets[p] = plane_offsets[p];
        out->sizes[p]         = plane_sizes[p];
    }
    for (uint32_t p = plane_count; p < WW_POOL_MAX_PLANES; ++p) {
        out->fds[p] = -1;
    }
    out->modifier = mod_props.drmFormatModifier;
    return 0;
}

static void free_slot(ww_pool_t* pool, uint32_t slot_index) {
    if (slot_index >= WW_POOL_MAX_SLOTS) return;
    vk_state_t* st = (vk_state_t*)pool->backend_data;
    /* Idle the device so we don't tear down resources still in use. */
    if (st->vkDeviceWaitIdle && st->device) {
        st->vkDeviceWaitIdle(st->device);
    }
    if (st->slots[slot_index].image != VK_NULL_HANDLE) {
        st->vkDestroyImage(st->device, st->slots[slot_index].image, NULL);
        st->slots[slot_index].image = VK_NULL_HANDLE;
    }
    if (st->slots[slot_index].memory != VK_NULL_HANDLE) {
        st->vkFreeMemory(st->device, st->slots[slot_index].memory, NULL);
        st->slots[slot_index].memory = VK_NULL_HANDLE;
    }
}

static int populate_slot_view(ww_pool_t* pool, uint32_t slot_index, ww_pool_slot_t* out) {
    vk_state_t* st = (vk_state_t*)pool->backend_data;
    if (slot_index >= WW_POOL_MAX_SLOTS) return -EINVAL;
    out->vk_image          = st->slots[slot_index].image;
    out->vk_memory         = st->slots[slot_index].memory;
    out->gl_export_fbo     = 0;
    out->gl_export_texture = 0;
    return 0;
}

static void backend_wait_idle(ww_pool_t* pool) {
    vk_state_t* st = (vk_state_t*)pool->backend_data;
    if (! st) return;
    if (st->vkDeviceWaitIdle && st->device) {
        st->vkDeviceWaitIdle(st->device);
    }
}

static void backend_destroy(ww_pool_t* pool) {
    vk_state_t* st = (vk_state_t*)pool->backend_data;
    if (! st) return;
    free(st);
    pool->backend_data = NULL;
}

static int backend_init(ww_pool_t* pool, const void* init_data) {
    const ww_pool_vulkan_init_t* init = (const ww_pool_vulkan_init_t*)init_data;
    if (! init || ! init->instance || ! init->physical_device || ! init->device ||
        ! init->get_instance_proc_addr) {
        return -EINVAL;
    }
    vk_state_t* st = (vk_state_t*)calloc(1, sizeof(*st));
    if (! st) return -ENOMEM;

    st->instance     = (VkInstance)init->instance;
    st->phys         = (VkPhysicalDevice)init->physical_device;
    st->device       = (VkDevice)init->device;
    st->queue        = (VkQueue)init->queue;
    st->queue_family = init->queue_family_index;
    /* Every slot can be copied or sampled by a display consumer. */
    st->image_usage =
        (init->image_usage_flags ? init->image_usage_flags : VK_IMAGE_USAGE_TRANSFER_DST_BIT) |
        VK_IMAGE_USAGE_TRANSFER_SRC_BIT | VK_IMAGE_USAGE_SAMPLED_BIT;
    st->format_features = (init->format_feature_flags ? init->format_feature_flags
                                                      : VK_FORMAT_FEATURE_TRANSFER_DST_BIT) |
                          VK_FORMAT_FEATURE_TRANSFER_SRC_BIT | VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT;

    /* See pool_egl_gbm: function-pointer-vs-object-pointer trick. */
    union {
        void* (*as_obj)(void*, const char*);
        PFN_vkGetInstanceProcAddr as_fn;
    } cvt;
    cvt.as_obj = init->get_instance_proc_addr;

    int rc = load_dispatch(st, cvt.as_fn);
    if (rc != 0) {
        ww_bridge_logf(WW_BRIDGE_LOG_ERROR,
                       "ww_pool[vulkan]: failed to resolve required Vulkan entry points");
        free(st);
        return rc;
    }

    pool->backend_data = st;

    /* The producer is responsible for filling drm_render_{major,minor}
     * (e.g. via `ww_bridge_vk_query_render_node` from probe_vk.h);
     * bridge does not query VK_EXT_physical_device_drm itself, to
     * keep this struct stateless w.r.t. physical-device introspection. */
    pool->caps.drm_render_major = init->drm_render_major;
    pool->caps.drm_render_minor = init->drm_render_minor;
    if (init->device_uuid) {
        memcpy(pool->caps.device_uuid, init->device_uuid, 16);
        pool->caps.have_uuid = true;
    }
    if (init->driver_uuid) {
        memcpy(pool->caps.driver_uuid, init->driver_uuid, 16);
    }

    /* drm_fd: pool.c will create the timeline drm_syncobj on this. The
     * Vulkan path does NOT export the timeline through Vulkan — the
     * old VkProducer used VkSemaphore + OPAQUE_FD, but bridge owns
     * the syncobj directly via DRM ioctls (matches the EGL/GBM path
     * and means the daemon's reaper sees one consistent fd kind).
     *
     * Resolution order: caller-supplied fd > queried renderD<minor> >
     * first-openable render node. The middle step matters on multi-GPU
     * boxes where "renderD128" may be a different device than the one
     * the producer's VkPhysicalDevice lives on. */
    if (init->drm_render_fd >= 0) {
        /* dup so we don't double-close. */
        int dup_fd = dup(init->drm_render_fd);
        if (dup_fd < 0) {
            ww_bridge_logf(WW_BRIDGE_LOG_ERROR,
                           "ww_pool[vulkan]: dup(drm_render_fd) failed: %s",
                           strerror(errno));
            free(st);
            pool->backend_data = NULL;
            return -errno;
        }
        pool->drm_fd = dup_fd;
    } else {
        /* No caller fd → bridge opens the exact render node the
         * producer advertised. We refuse to "guess" by walking
         * /dev/dri/renderD12X — on multi-GPU hosts that lands on the
         * wrong device and the daemon's topology check then silently
         * computes wrong same-device decisions. Better to fail loudly
         * here so the producer is forced to either share its fd or
         * call ww_bridge_vk_query_render_node before pool_create. */
        if (init->drm_render_minor == 0) {
            ww_bridge_logf(WW_BRIDGE_LOG_ERROR,
                           "ww_pool[vulkan]: drm_render_fd=-1 and drm_render_minor=0 "
                           "— producer must populate drm_render_minor (use "
                           "ww_bridge_vk_query_render_node from probe_vk.h) or share "
                           "an already-opened drm_render_fd");
            free(st);
            pool->backend_data = NULL;
            return -EINVAL;
        }
        int fd = ww_drm_open_render_node_by_minor(init->drm_render_minor);
        if (fd < 0) {
            ww_bridge_logf(WW_BRIDGE_LOG_ERROR,
                           "ww_pool[vulkan]: open(/dev/dri/renderD%u) failed: %d",
                           init->drm_render_minor,
                           fd);
            free(st);
            pool->backend_data = NULL;
            return fd;
        }
        pool->drm_fd = fd;
    }

    return 0;
}

static int fork_allocation(const ww_pool_t* source, ww_pool_t* candidate) {
    vk_state_t* st = malloc(sizeof(*st));
    if (! st) return -ENOMEM;
    *st = *(const vk_state_t*)source->backend_data;
    memset(st->slots, 0, sizeof(st->slots));
    candidate->backend_data = st;
    return 0;
}

static const struct ww_pool_backend_ops kVulkanOps = {
    .fork_allocation    = fork_allocation,
    .init               = backend_init,
    .probe_caps         = probe_caps,
    .alloc_slot         = alloc_slot,
    .free_slot          = free_slot,
    .populate_slot_view = populate_slot_view,
    .wait_idle          = backend_wait_idle,
    .destroy            = backend_destroy,
};

int ww_pool_vulkan_create(ww_pool_t* pool, const void* init_data) {
    pool->ops = &kVulkanOps;
    int rc    = backend_init(pool, init_data);
    if (rc != 0) {
        pool->ops = NULL;
    }
    return rc;
}
