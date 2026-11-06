module;

#include <rstd/macro.hpp>

#include <waywallen-bridge/bridge.h>
#include <waywallen-bridge/drm_fourcc.h>
#include <waywallen-bridge/pool.h>
#include <waywallen-bridge/probe_vk.h>

#define ww_resolution_apply_cap        OWE_ww_resolution_apply_cap
#define ww_resolution_apply_short_edge OWE_ww_resolution_apply_short_edge
#define ww_resolution_sanitize         OWE_ww_resolution_sanitize
#define ww_resolution_short_edge       OWE_ww_resolution_short_edge
#include <waywallen-bridge/resolution.h>
#undef ww_resolution_apply_cap
#undef ww_resolution_apply_short_edge
#undef ww_resolution_sanitize
#undef ww_resolution_short_edge

enum : uint32_t
{
    OWE_WW_BRIDGE_SUPPORTED_PROTOCOL_VERSION = WW_BRIDGE_SUPPORTED_PROTOCOL_VERSION,
    OWE_WW_BRIDGE_SUPPORTED_SPAWN_VERSION    = WW_BRIDGE_SUPPORTED_SPAWN_VERSION,
    OWE_WW_MEM_HINT_DEVICE_LOCAL             = WW_MEM_HINT_DEVICE_LOCAL,
    OWE_WW_MEM_HINT_HOST_VISIBLE             = WW_MEM_HINT_HOST_VISIBLE,
    OWE_WW_RENDERER_STATE_FIELD_CLEAR_COLOR  = WW_RENDERER_STATE_FIELD_CLEAR_COLOR,
    OWE_WW_RENDERER_STATE_FIELD_RUNTIME_TAGS = WW_RENDERER_STATE_FIELD_RUNTIME_TAGS,
    OWE_WW_DRM_FORMAT_ABGR8888               = WW_DRM_FORMAT_ABGR8888,
    OWE_WW_DRM_FORMAT_XBGR8888               = WW_DRM_FORMAT_XBGR8888,
    OWE_WW_DRM_FORMAT_ARGB8888               = WW_DRM_FORMAT_ARGB8888,
    OWE_WW_DRM_FORMAT_XRGB8888               = WW_DRM_FORMAT_XRGB8888,
    OWE_WW_DRM_FORMAT_RGBA8888               = WW_DRM_FORMAT_RGBA8888,
    OWE_WW_DRM_FORMAT_BGRA8888               = WW_DRM_FORMAT_BGRA8888,
    OWE_WW_DRM_FORMAT_RGBX8888               = WW_DRM_FORMAT_RGBX8888,
    OWE_WW_DRM_FORMAT_BGRX8888               = WW_DRM_FORMAT_BGRX8888,
};

#undef WW_BRIDGE_SUPPORTED_PROTOCOL_VERSION
#undef WW_BRIDGE_SUPPORTED_SPAWN_VERSION
#undef WW_MEM_HINT_DEVICE_LOCAL
#undef WW_MEM_HINT_HOST_VISIBLE
#undef WW_RENDERER_STATE_FIELD_CLEAR_COLOR
#undef WW_RENDERER_STATE_FIELD_RUNTIME_TAGS
#undef WW_DRM_FORMAT_ABGR8888
#undef WW_DRM_FORMAT_XBGR8888
#undef WW_DRM_FORMAT_ARGB8888
#undef WW_DRM_FORMAT_XRGB8888
#undef WW_DRM_FORMAT_RGBA8888
#undef WW_DRM_FORMAT_BGRA8888
#undef WW_DRM_FORMAT_RGBX8888
#undef WW_DRM_FORMAT_BGRX8888

export module waywallen.bridge;

export import vvk;
import rstd;
import rstd.cppstd;
import rstd.log;

export inline constexpr uint32_t WW_BRIDGE_SUPPORTED_PROTOCOL_VERSION =
    OWE_WW_BRIDGE_SUPPORTED_PROTOCOL_VERSION;
export inline constexpr uint32_t WW_BRIDGE_SUPPORTED_SPAWN_VERSION =
    OWE_WW_BRIDGE_SUPPORTED_SPAWN_VERSION;

export inline constexpr const char* WAYWALLEN_AUDIO_APPLICATION_NAME =
    WW_BRIDGE_AUDIO_APPLICATION_NAME;
export inline constexpr const char* WAYWALLEN_AUDIO_APPLICATION_ID = WW_BRIDGE_AUDIO_APPLICATION_ID;
export inline constexpr const char* WAYWALLEN_AUDIO_STREAM_PREFIX  = WW_BRIDGE_AUDIO_STREAM_PREFIX;

#undef WW_BRIDGE_AUDIO_APPLICATION_NAME
#undef WW_BRIDGE_AUDIO_APPLICATION_ID
#undef WW_BRIDGE_AUDIO_STREAM_PREFIX

export inline constexpr uint32_t WW_MEM_HINT_DEVICE_LOCAL = OWE_WW_MEM_HINT_DEVICE_LOCAL;
export inline constexpr uint32_t WW_MEM_HINT_HOST_VISIBLE = OWE_WW_MEM_HINT_HOST_VISIBLE;
export inline constexpr uint32_t WW_RENDERER_STATE_FIELD_CLEAR_COLOR =
    OWE_WW_RENDERER_STATE_FIELD_CLEAR_COLOR;
export inline constexpr uint32_t WW_RENDERER_STATE_FIELD_RUNTIME_TAGS =
    OWE_WW_RENDERER_STATE_FIELD_RUNTIME_TAGS;

export inline constexpr uint32_t WW_DRM_FORMAT_ABGR8888 = OWE_WW_DRM_FORMAT_ABGR8888;
export inline constexpr uint32_t WW_DRM_FORMAT_XBGR8888 = OWE_WW_DRM_FORMAT_XBGR8888;
export inline constexpr uint32_t WW_DRM_FORMAT_ARGB8888 = OWE_WW_DRM_FORMAT_ARGB8888;
export inline constexpr uint32_t WW_DRM_FORMAT_XRGB8888 = OWE_WW_DRM_FORMAT_XRGB8888;
export inline constexpr uint32_t WW_DRM_FORMAT_RGBA8888 = OWE_WW_DRM_FORMAT_RGBA8888;
export inline constexpr uint32_t WW_DRM_FORMAT_BGRA8888 = OWE_WW_DRM_FORMAT_BGRA8888;
export inline constexpr uint32_t WW_DRM_FORMAT_RGBX8888 = OWE_WW_DRM_FORMAT_RGBX8888;
export inline constexpr uint32_t WW_DRM_FORMAT_BGRX8888 = OWE_WW_DRM_FORMAT_BGRX8888;

export using ::WW_EVT_IN_INIT;
export using ::WW_EVT_IN_MUTE;
export using ::WW_EVT_IN_NEGOTIATE_BUFFERS;
export using ::WW_EVT_IN_CONFIGURE_RENDER_TARGET;
export using ::WW_EVT_IN_EVENT_SUBSCRIPTIONS_APPLIED;
export using ::WW_EVT_IN_PAUSE;
export using ::WW_EVT_IN_PLAY;
export using ::WW_EVT_IN_POINTER_AXIS;
export using ::WW_EVT_IN_POINTER_BUTTON;
export using ::WW_EVT_IN_POINTER_MOTION;
export using ::WW_EVT_IN_MPRIS;
export using ::WW_EVT_IN_REQUEST_FRAME;
export using ::WW_EVT_IN_SETTING_CHANGED;
export using ::WW_EVT_IN_SET_LOG_LEVEL;
export using ::WW_EVT_IN_SHUTDOWN;
export using ::WW_EVT_IN_UNMUTE;
export using ::WW_EVT_FORMAT_CAPS;
export using ::WW_POOL_BACKEND_VULKAN;
export using ::WW_POOL_SLOT_ACQUIRE_BUSY;
export using ::WW_POOL_SLOT_ACQUIRE_CANCELLED;
export using ::WW_POOL_SLOT_ACQUIRE_ERROR;
export using ::WW_POOL_SLOT_ACQUIRE_READY_RELEASED;
export using ::WW_POOL_SLOT_ACQUIRE_READY_UNUSED;
export using ::WW_POOL_SLOT_ACQUIRE_SESSION_LOST;
export using ::WW_POOL_SLOT_SUBMIT_ERROR;
export using ::WW_POOL_SLOT_SUBMIT_SESSION_LOST;
export using ::WW_POOL_SLOT_SUBMIT_SUBMITTED;
export using ::WW_POOL_REPUBLISH_BUSY;
export using ::WW_POOL_REPUBLISH_CANCELLED;
export using ::WW_POOL_REPUBLISH_ERROR;
export using ::WW_POOL_REPUBLISH_NO_CONTENT;
export using ::WW_POOL_REPUBLISH_PUBLISHED;
export using ::WW_POOL_REPUBLISH_SESSION_LOST;
export using ::WW_RESOLUTION_720P;
export using ::WW_RESOLUTION_1080P;
export using ::WW_RESOLUTION_1440P;
export using ::WW_RESOLUTION_2160P;
export using ::WW_RESOLUTION_CAP_DEFAULT;
export using ::WW_RESOLUTION_CAP_ALLOW_UPSCALE;
export using ::WW_RESOLUTION_CUSTOM;
export using ::WW_RESOLUTION_ORIGIN;
export using ::WW_RESOLUTION_AUTO;
export using ::WAYWALLEN_EVENT_SUBSCRIPTION_STATUS_APPLIED;
export using ::WAYWALLEN_POINTER_BUTTON_STATE_PRESSED;
export using ::WAYWALLEN_POINTER_BUTTON_STATE_RELEASED;
export using ::VK_FORMAT_FEATURE_BLIT_DST_BIT;
export using ::VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT;
export using ::VK_FORMAT_FEATURE_STORAGE_IMAGE_BIT;
export using ::VK_FORMAT_FEATURE_TRANSFER_DST_BIT;
export using ::VK_IMAGE_USAGE_STORAGE_BIT;
export using ::VK_IMAGE_USAGE_SAMPLED_BIT;
export using ::VK_IMAGE_USAGE_TRANSFER_SRC_BIT;

export using ::ww_bridge_close;
export using ::ww_bridge_connect;
export using ::ww_bridge_control_free;
export using ::ww_bridge_control_t;
export using ::ww_bridge_log_level_t;
export using ::ww_bridge_pool_abort_acquired_slot;
export using ::ww_bridge_pool_advertise_caps;
export using ::ww_bridge_pool_apply_directive;
export using ::ww_bridge_pool_prepare_target;
export using ::ww_bridge_pool_candidate_slot;
export using ::ww_bridge_pool_commit_target;
export using ::ww_bridge_pool_abort_target;
export using ::ww_bridge_render_target_directive;
export using ::ww_bridge_send_render_target_capabilities;
export using ::ww_bridge_send_render_target_result;
export using ::waywallen_render_target_capabilities_t;
export using ::waywallen_render_target_config_t;
export using ::waywallen_render_target_outcome_t;
export using ::ww_bridge_pool_create;
export using ::ww_bridge_pool_destroy;
export using ::ww_bridge_pool_get_extent;
export using ::ww_bridge_pool_submit_acquired_slot;
export using ::ww_bridge_pool_try_acquire_any_for_render;
export using ::ww_bridge_pool_try_republish_latest;
export using ::ww_bridge_pool_wait_acquire_any_for_render;
export using ::ww_bridge_pool_wait_republish_latest;
export using ::ww_bridge_recv_control;
export using ::ww_bridge_recv_frame;
export using ::ww_bridge_recv_init;
export using ::ww_bridge_send_bind_failed;
export using ::ww_bridge_send_init_nack;
export using ::ww_bridge_send_report_state_clear_color;
export using ::ww_bridge_send_report_state_tags;
export using ::ww_bridge_set_event_subscriptions;
export using ::ww_bridge_set_log_callback;
export using ::ww_bridge_vk_dt_load;
export using ::ww_bridge_vk_dt_t;
export using ::ww_bridge_vk_log_gpu_info;
export using ::ww_bridge_vk_query_render_node;
export using ::ww_bridge_vk_resolve_render_node;
export using ::ww_array_u32_t;
export using ::ww_event_in_op_t;
export using ::ww_evt_format_caps_decode;
export using ::ww_evt_format_caps_free;
export using ::ww_evt_format_caps_t;
export using ::ww_kv_t;
export using ::ww_kv_list_t;
export using ::ww_pool_cancel_fn;
export using ::ww_pool_directive_t;
export using ::ww_pool_republish_result_t;
export using ::ww_pool_republish_status_t;
export using ::ww_pool_slot_t;
export using ::ww_pool_slot_acquire_result_t;
export using ::ww_pool_slot_acquire_status_t;
export using ::ww_pool_slot_identity_t;
export using ::ww_pool_slot_submit_result_t;
export using ::ww_pool_slot_submit_status_t;
export using ::ww_pool_t;
export using ::ww_pool_vulkan_init_t;
export using ::waywallen_bind_failure_t;
export using ::waywallen_buffer_allocation_failure_kind_t;
export using ::waywallen_buffer_directive_t;
export using ::waywallen_event_subscription_result_t;
export using ::waywallen_event_subscription_t;
export using ::waywallen_init_rejection_t;
export using ::waywallen_log_level_t;
export using ::waywallen_mpris_snapshot_t;
export using ::waywallen_pointer_axis_t;
export using ::waywallen_pointer_button_t;
export using ::waywallen_pointer_motion_t;
export using ::waywallen_renderer_init_free;
export using ::waywallen_renderer_init_t;

export inline void ww_renderer_log_set_level(waywallen_log_level_t level) {
    using rstd::log::LevelFilter;
    switch (level) {
    case WAYWALLEN_LOG_LEVEL_OFF: rstd::log::set_max_level(LevelFilter::Off); break;
    case WAYWALLEN_LOG_LEVEL_ERROR: rstd::log::set_max_level(LevelFilter::Error); break;
    case WAYWALLEN_LOG_LEVEL_WARN: rstd::log::set_max_level(LevelFilter::Warn); break;
    case WAYWALLEN_LOG_LEVEL_INFO: rstd::log::set_max_level(LevelFilter::Info); break;
    case WAYWALLEN_LOG_LEVEL_DEBUG: rstd::log::set_max_level(LevelFilter::Debug); break;
    case WAYWALLEN_LOG_LEVEL_TRACE: rstd::log::set_max_level(LevelFilter::Trace); break;
    }
}

export inline void ww_renderer_log_init() {
    using namespace rstd::literals;
    static rstd::log::EnvLogger logger("trace"_str);
    rstd::log::set_logger(logger);
    rstd::log::set_max_level(rstd::log::LevelFilter::Trace);

    auto initial = rstd::log::LevelFilter::Info;
    if (auto value = rstd::env::var("WW_LOG"_str).ok(); value.is_some()) {
        auto parsed = rstd::log::parse_level_filter(value->as_str());
        if (parsed.is_some()) {
            initial = parsed.unwrap_unchecked();
        } else {
            rstd_warn("invalid WW_LOG value '{}'; using info", value->as_str());
        }
    }
    rstd::log::set_max_level(initial);

    ww_bridge_set_log_callback(
        [](ww_bridge_log_level_t level, const char* msg, void*) {
            constexpr rstd::log::Level levels[4] = {
                rstd::log::Level::Debug,
                rstd::log::Level::Info,
                rstd::log::Level::Warn,
                rstd::log::Level::Error,
            };
            auto lvl =
                levels[static_cast<unsigned>(level) <= 3u ? static_cast<unsigned>(level) : 3u];
            auto args = rstd::fmt::Arguments::make("{}", msg);
            rstd::log::log(rstd::log::Record { rstd::log::Metadata { lvl, {} }, args });
        },
        nullptr);
}

export inline uint32_t ww_resolution_short_edge(int32_t r) {
    switch (r) {
    case WW_RESOLUTION_720P: return 720u;
    case WW_RESOLUTION_1080P: return 1080u;
    case WW_RESOLUTION_1440P: return 1440u;
    case WW_RESOLUTION_2160P: return 2160u;
    default: return 0u;
    }
}

export inline int32_t ww_resolution_sanitize(int32_t raw) {
    if (raw == static_cast<int32_t>(WW_RESOLUTION_AUTO) ||
        raw == static_cast<int32_t>(WW_RESOLUTION_CUSTOM))
        return raw;
    if (raw < static_cast<int32_t>(WW_RESOLUTION_ORIGIN) ||
        raw > static_cast<int32_t>(WW_RESOLUTION_2160P))
        return static_cast<int32_t>(WW_RESOLUTION_1080P);
    return raw;
}

export inline void ww_resolution_apply_short_edge(uint32_t cap, uint32_t option, uint32_t* w,
                                                  uint32_t* h) {
    if (cap == 0 || w == nullptr || h == nullptr || *w == 0 || *h == 0) return;
    uint32_t short_edge = (*w < *h) ? *w : *h;
    if (short_edge == cap) return;
    if (short_edge < cap && option != static_cast<uint32_t>(WW_RESOLUTION_CAP_ALLOW_UPSCALE)) {
        return;
    }
    if (*w < *h) {
        *h = static_cast<uint32_t>(static_cast<uint64_t>(*h) * cap / short_edge);
        *w = cap;
    } else {
        *w = static_cast<uint32_t>(static_cast<uint64_t>(*w) * cap / short_edge);
        *h = cap;
    }
    if (*w == 0) *w = 1u;
    if (*h == 0) *h = 1u;
}

export inline void ww_resolution_apply_cap(int32_t resolution, uint32_t option, uint32_t* w,
                                           uint32_t* h) {
    ww_resolution_apply_short_edge(ww_resolution_short_edge(resolution), option, w, h);
}
