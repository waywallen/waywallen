module;

#include <rstd/macro.hpp>

#include "av_image.hpp"

#include <errno.h>
#include <signal.h>
#include <stdio.h>

#include <sys/prctl.h>
#include <sys/socket.h>
#include <unistd.h>

module waywallen.image.entry;

import rstd.cppstd;
import rstd.argparse;
import rstd.json;
import rstd.log;
import wavsen.video;
import waywallen.bridge;

namespace
{

using namespace rstd::literals;

struct Options {
    std::string ipc_path;
    std::string image_path;
    /* Final decoded extent, set by `decode_to_rgba`. */
    uint32_t width { 0 };
    uint32_t height { 0 };
    /* Short-edge cap from the user's `resolution` setting. 0 = ORIGIN. */
    int32_t resolution { 0 };
    bool    decode_only { false };
    bool    vulkan_probe { false };
    // Test hook
    bool        print_caps { false };
    std::string render_node;
};

struct ParseArgsResult {
    Options options;
    int     exit_code { 0 };
    bool    should_run { false };
};

struct ClearColor {
    float r { 0.0f };
    float g { 0.0f };
    float b { 0.0f };
    float a { 1.0f };
};

constexpr const char* kSchemeColorKey = "waywallen.scheme_color";

[[noreturn]] void die(const std::string& msg) {
    rstd_error("waywallen-image-renderer: {}", msg);
    std::exit(1);
}

std::string to_std_string(const rstd::string::String& value) {
    return rstd::cppstd::to_string(value);
}

auto as_rstd_str(std::string_view value) -> rstd::ref<rstd::str> {
    return rstd::move(rstd::cppstd::as_str(value)).unwrap();
}

void write_cli_output(rstd::ref<rstd::str> text, rstd::argparse::OutputTarget::Tag target) {
    FILE* stream = target == rstd::argparse::OutputTarget::Tag::Stderr ? stderr : stdout;
    std::fwrite(text.data(), 1, text.size().to_primitive(), stream);
}

rstd::prelude::Vec<rstd::ffi::OsString> cli_argv(int argc, char** argv) {
    auto values =
        rstd::prelude::Vec<rstd::ffi::OsString>::with_capacity(static_cast<rstd::usize>(argc));
    for (int i = 0; i < argc; ++i) {
        auto bytes = rstd::slice<rstd::u8>::from_raw_parts(
            reinterpret_cast<const rstd::byte*>(argv[i]), rstd::usize(std::strlen(argv[i])));
        values.push(rstd::ffi::OsString::from(
            rstd::ref<rstd::ffi::OsStr>::from_encoded_bytes_unchecked(bytes)));
    }
    return values;
}

template<typename T>
auto get_arg(const rstd::argparse::Matches& matches, const rstd::argparse::ArgKey<T>& key)
    -> rstd::Option<rstd::ref<T>> {
    auto value = matches.get_one(key);
    if (value.is_err()) {
        rstd_error("waywallen-image-renderer: argparse match access failed: {}",
                   std::move(value).unwrap_err());
        std::exit(1);
    }
    return std::move(value).unwrap();
}

float clamp01(float v) {
    if (v < 0.0f) return 0.0f;
    if (v > 1.0f) return 1.0f;
    return v;
}

bool parse_color_wire(const char* raw, ClearColor& out) {
    if (! raw || ! *raw) return false;
    std::string s = raw;
    for (char& ch : s) {
        if (ch == ',') ch = ' ';
    }

    float       values[4] = {};
    int         count     = 0;
    const char* p         = s.c_str();
    while (*p) {
        while (*p && std::isspace(static_cast<unsigned char>(*p))) ++p;
        if (! *p) break;
        if (count >= 4) return false;
        errno     = 0;
        char* end = nullptr;
        float v   = std::strtof(p, &end);
        if (end == p || errno == ERANGE || ! std::isfinite(v)) return false;
        values[count++] = clamp01(v);
        p               = end;
    }
    if (count < 3) return false;
    out = ClearColor {
        .r = values[0],
        .g = values[1],
        .b = values[2],
        .a = count >= 4 ? values[3] : 1.0f,
    };
    return true;
}

// SPAWN_VERSION 3: argv carries the canonical `--path` for the image
// resource plus `--ipc`. Per-plugin runtime settings (fps, etc.) come
// in via `Init.settings` kv. Standalone-debug flags (`--decode-only`,
// `--vulkan-probe`, `--print-caps`) are still parsed here.
ParseArgsResult parse_args(int argc, char** argv) {
    using namespace rstd::argparse;

    auto command = Command::make("waywallen-image-renderer"_str);
    command.about("Render image wallpapers for waywallen"_str);
    auto ipc  = command.add_arg(Arg<rstd::string::String>::value("ipc"_str, string_parser())
                                    .long_name("ipc"_str)
                                    .value_name("SOCKET"_str)
                                    .help("Connect to the renderer IPC socket"_str));
    auto path = command.add_arg(Arg<rstd::string::String>::value("path"_str, string_parser())
                                    .long_name("path"_str)
                                    .value_name("IMAGE"_str)
                                    .help("Image wallpaper path"_str));
    command.add_arg(Arg<bool>::flag("decode-only"_str)
                        .long_name("decode-only"_str)
                        .help("Decode the image without starting the renderer"_str));
    command.add_arg(Arg<bool>::flag("vulkan-probe"_str)
                        .long_name("vulkan-probe"_str)
                        .help("Probe Vulkan renderer initialization"_str));
    command.add_arg(Arg<bool>::flag("print-caps"_str)
                        .long_name("print-caps"_str)
                        .help("Print renderer capabilities as JSON"_str));
    auto render_node =
        command.add_arg(Arg<rstd::string::String>::value("render-node"_str, string_parser())
                            .long_name("render-node"_str)
                            .value_name("DEVICE"_str)
                            .help("Use a specific DRM render node"_str));

    auto built = std::move(command).build();
    if (built.is_err()) {
        rstd_error("waywallen-image-renderer: invalid CLI definition: {}",
                   std::move(built).unwrap_err());
        return { .exit_code = 1 };
    }
    auto parser = std::move(built).unwrap();
    auto parsed = parser.parse_known_from(cli_argv(argc, argv));
    if (parsed.is_err()) {
        auto error  = std::move(parsed).unwrap_err();
        auto report = parser.render_error(error);
        write_cli_output(report.text(), report.target());
        return { .exit_code = report.exit_code().to_primitive() };
    }

    auto outcome = std::move(parsed).unwrap();
    if (outcome.is_Display()) {
        const auto& request = outcome.as_Display().request;
        write_cli_output(request.text(), request.target());
        return { .exit_code = request.exit_code().to_primitive() };
    }

    auto    known   = std::move(outcome).as_Parsed().value;
    auto    matches = known.matches();
    Options options;
    if (auto value = get_arg(*matches, ipc); value.is_some()) {
        options.ipc_path = to_std_string(**value);
    }
    if (auto value = get_arg(*matches, path); value.is_some()) {
        options.image_path = to_std_string(**value);
    }
    if (auto value = get_arg(*matches, render_node); value.is_some()) {
        options.render_node = to_std_string(**value);
    }
    options.decode_only  = matches->contains("decode-only"_str);
    options.vulkan_probe = matches->contains("vulkan-probe"_str);
    options.print_caps   = matches->contains("print-caps"_str);
    return { .options = std::move(options), .should_run = true };
}

struct HostState {
    int               sock { -1 };
    ww_pool_t*        pool { nullptr };
    std::atomic<bool> shutdown { false };
    std::atomic<bool> negotiated { false };

    /* Reader → main negotiate handoff. */
    std::mutex              neg_mu;
    std::condition_variable neg_cv;
    bool                    neg_pending { false };
    bool                    frame_request_pending { false };
    ww_pool_directive_t     neg_directive {};
    std::mutex              send_mu;

    /* Cached RGBA buffer (kept alive across re-negotiations so we
     * can re-upload after a directive change). */
    const uint8_t* rgba_data { nullptr };
    size_t         rgba_size { 0 };
    ClearColor     scheme_color {};
};

void signal_shutdown(HostState& s) {
    s.shutdown.store(true, std::memory_order_release);
    s.neg_cv.notify_all();
}

// Test hook: when WAYWALLEN_IMAGE_DUMP_DIR is set, write the RGBA8
// bytes the renderer is about to upload to the GPU to a file the
// orchestrator can compare against the consumer-side dump. The dump
// captures the *input* (post-decode, pre-staging) so it's always
// linear regardless of the picked DRM modifier — the consumer also
// dumps post-readback linear bytes, so byte-equality is meaningful.
//
// Filename: producer-{seq:06}-0x{fourcc:08x}-0x{modifier:016x}.bin
// Sidecar:  same name with .json — width/height/stride/fourcc/modifier.
static void maybe_dump_producer_frame(const HostState& host, const ww_pool_directive_t& d,
                                      const ww_pool_slot_t& s, uint64_t seq) {
    const char* dir = std::getenv("WAYWALLEN_IMAGE_DUMP_DIR");
    if (! dir || ! *dir) return;
    if (! host.rgba_data || host.rgba_size == 0) return;

    char path[512];
    std::snprintf(path,
                  sizeof(path),
                  "%s/producer-%06llu-0x%08x-0x%016llx.bin",
                  dir,
                  static_cast<unsigned long long>(seq),
                  d.format.fourcc,
                  static_cast<unsigned long long>(d.format.modifier));
    FILE* f = std::fopen(path, "wb");
    if (! f) {
        rstd_warn("waywallen-image-renderer: dump open {}: {}",
                  static_cast<const char*>(path),
                  static_cast<const char*>(std::strerror(errno)));
        return;
    }
    size_t w = std::fwrite(host.rgba_data, 1, host.rgba_size, f);
    std::fclose(f);
    if (w != host.rgba_size) {
        rstd_warn("waywallen-image-renderer: dump short write {}/{} to {}",
                  w,
                  host.rgba_size,
                  static_cast<const char*>(path));
        return;
    }

    char sidecar[520];
    std::snprintf(sidecar,
                  sizeof(sidecar),
                  "%s/producer-%06llu-0x%08x-0x%016llx.json",
                  dir,
                  static_cast<unsigned long long>(seq),
                  d.format.fourcc,
                  static_cast<unsigned long long>(d.format.modifier));
    FILE* sf = std::fopen(sidecar, "w");
    if (! sf) return;
    // Note: the dump is always tightly-packed RGBA8 (`width*height*4`
    // bytes) — that's the input format `decode_to_rgba` produces and
    // what `upload_into` accepts. The DMA-BUF stride/plane_offset are
    // the *destination* layout in the GPU buffer, which the consumer
    // reads back into the same tightly-packed shape; both sides' dumps
    // are therefore directly comparable.
    std::fprintf(sf,
                 "{\n"
                 "  \"kind\": \"producer\",\n"
                 "  \"seq\": %llu,\n"
                 "  \"fourcc\": \"0x%08x\",\n"
                 "  \"modifier\": \"0x%016llx\",\n"
                 "  \"width\": %u,\n"
                 "  \"height\": %u,\n"
                 "  \"stride\": %u,\n"
                 "  \"plane_offset\": %u,\n"
                 "  \"size\": %u,\n"
                 "  \"row_bytes\": %u,\n"
                 "  \"row_count\": %u,\n"
                 "  \"dump_layout\": \"tightly_packed_rgba8\"\n"
                 "}\n",
                 static_cast<unsigned long long>(seq),
                 d.format.fourcc,
                 static_cast<unsigned long long>(d.format.modifier),
                 s.width,
                 s.height,
                 s.stride,
                 s.plane_offset,
                 s.size,
                 s.width * 4u,
                 s.height);
    std::fclose(sf);
}

enum class UploadStatus
{
    Submitted,
    Cancelled,
    Failed,
};

int cancel_slot_wait(void* userdata) {
    auto& host = *static_cast<HostState*>(userdata);
    if (host.shutdown.load(std::memory_order_acquire)) return 1;
    std::lock_guard<std::mutex> lk(host.neg_mu);
    return host.neg_pending ? 1 : 0;
}

enum class RepublishStatus
{
    Published,
    Cancelled,
    Failed,
};

RepublishStatus republish_latest(HostState& host) {
    std::lock_guard<std::mutex> send_lk(host.send_mu);
    ww_pool_republish_result_t  result {};
    const int                   rc = ww_bridge_pool_wait_republish_latest(
        host.pool, host.sock, cancel_slot_wait, &host, &result);
    if (rc != 0) {
        rstd_error("waywallen-image-renderer: republish contract failed: {}", rc);
        return RepublishStatus::Failed;
    }
    switch (result.status) {
    case WW_POOL_REPUBLISH_PUBLISHED:
        rstd_debug("waywallen-image-renderer: republished slot {} seq={}",
                   result.slot_index,
                   result.sequence);
        return RepublishStatus::Published;
    case WW_POOL_REPUBLISH_CANCELLED: return RepublishStatus::Cancelled;
    case WW_POOL_REPUBLISH_NO_CONTENT:
    case WW_POOL_REPUBLISH_BUSY:
        rstd_error("waywallen-image-renderer: current frame cannot be republished "
                   "(status={}, error={})",
                   static_cast<int>(result.status),
                   result.error_code);
        return RepublishStatus::Failed;
    case WW_POOL_REPUBLISH_SESSION_LOST:
    case WW_POOL_REPUBLISH_ERROR:
        rstd_error("waywallen-image-renderer: republish failed (status={}, error={})",
                   static_cast<int>(result.status),
                   result.error_code);
        return RepublishStatus::Failed;
    }
    return RepublishStatus::Failed;
}

UploadStatus upload_to_slot(HostState& host, wavsen::video::Producer& producer,
                            const ww_pool_directive_t& directive) {
    ww_pool_slot_acquire_result_t acquired {};
    if (int rc = ww_bridge_pool_wait_acquire_any_for_render(
            host.pool, cancel_slot_wait, &host, &acquired);
        rc != 0) {
        rstd_error("waywallen-image-renderer: acquire slot contract failed: {}", rc);
        return UploadStatus::Failed;
    }
    if (acquired.status == WW_POOL_SLOT_ACQUIRE_CANCELLED) {
        return UploadStatus::Cancelled;
    }
    if (acquired.status != WW_POOL_SLOT_ACQUIRE_READY_UNUSED &&
        acquired.status != WW_POOL_SLOT_ACQUIRE_READY_RELEASED) {
        rstd_error("waywallen-image-renderer: no slot is writable (status={}, error={})",
                   static_cast<int>(acquired.status),
                   acquired.error_code);
        return UploadStatus::Failed;
    }
    const auto& s = acquired.slot;
    if (! s.vk_image) {
        ww_bridge_pool_abort_acquired_slot(host.pool, &acquired.identity);
        rstd_error("waywallen-image-renderer: slot {} has no VkImage handle", s.index);
        return UploadStatus::Failed;
    }

    static std::atomic<uint64_t> g_dump_seq { 0 };
    maybe_dump_producer_frame(
        host, directive, s, g_dump_seq.fetch_add(1, std::memory_order_relaxed));

    auto upload_res = producer.upload_into(reinterpret_cast<VkImage>(s.vk_image),
                                           rstd::u32(s.width),
                                           rstd::u32(s.height),
                                           host.rgba_data,
                                           rstd::usize(host.rgba_size));
    if (upload_res.is_err()) {
        ww_bridge_pool_abort_acquired_slot(host.pool, &acquired.identity);
        rstd_error("waywallen-image-renderer: upload_into failed: {}",
                   std::move(upload_res).unwrap_err().message.as_str());
        return UploadStatus::Failed;
    }
    int                          sync_fd = std::move(upload_res).unwrap();
    std::lock_guard<std::mutex>  send_lk(host.send_mu);
    ww_pool_slot_submit_result_t submitted {};
    int                          rc = ww_bridge_pool_submit_acquired_slot(
        host.pool, host.sock, &acquired.identity, sync_fd, &submitted);
    if (rc != 0 || submitted.status != WW_POOL_SLOT_SUBMIT_SUBMITTED) {
        rstd_error("waywallen-image-renderer: submit slot failed (rc={}, status={}, error={})",
                   rc,
                   static_cast<int>(submitted.status),
                   submitted.error_code);
        return UploadStatus::Failed;
    }
    return UploadStatus::Submitted;
}

void publish_clear_color(HostState& host, const ClearColor& c) {
    std::lock_guard<std::mutex> send_lk(host.send_mu);
    if (int rc = ww_bridge_send_report_state_clear_color(host.sock, c.r, c.g, c.b, c.a); rc != 0) {
        rstd_warn("waywallen-image-renderer: report_state(clear_color) failed ({})", rc);
    }
}

void set_scheme_color(HostState& host, const char* value, bool publish) {
    ClearColor next {};
    if (value && *value && ! parse_color_wire(value, next)) {
        rstd_warn("waywallen-image-renderer: invalid {} value '{}'; ignoring",
                  static_cast<const char*>(kSchemeColorKey),
                  static_cast<const char*>(value));
        return;
    }
    host.scheme_color = next;
    if (publish) publish_clear_color(host, host.scheme_color);
}

void apply_user_properties(HostState& host, const char* json) {
    if (! json || ! *json) return;
    auto bytes = rstd::slice<rstd::u8>::from_raw_parts(reinterpret_cast<const rstd::byte*>(json),
                                                       rstd::usize(std::strlen(json)));
    auto parsed_result =
        rstd::json::from_slice(bytes, rstd::json::ParseOptions { .allow_comments = true });
    if (parsed_result.is_err()) return;
    auto parsed = parsed_result.unwrap();
    if (! parsed.is_object()) {
        rstd_warn("waywallen-image-renderer: init.user_properties is not an object; ignored");
        return;
    }
    auto value = parsed.get("waywallen.scheme_color"_str);
    if (value.is_none()) return;
    if ((**value).is_string()) {
        const auto text = rstd::cppstd::to_string(*(**value).as_str());
        set_scheme_color(host, text.c_str(), false);
    } else {
        rstd_warn("waywallen-image-renderer: {} is not a string; ignored",
                  static_cast<const char*>(kSchemeColorKey));
    }
}

/* Apply a directive received from the daemon. After bridge brings the
 * slots up, upload our cached RGBA into slot 0 and submit one frame.
 * Static images: a single submit per (re-)negotiation is enough. */
void apply_negotiate_request(HostState& host, wavsen::video::Producer& producer,
                             const ww_pool_directive_t& d) {
    int rc = 0;
    {
        std::lock_guard<std::mutex> send_lk(host.send_mu);
        rc = ww_bridge_pool_apply_directive(host.pool, host.sock, &d);
    }
    if (rc != 0) {
        rstd_error("waywallen-image-renderer: pool_apply_directive failed: {}", rc);
        if (rc > 0) signal_shutdown(host);
        return;
    }
    const auto upload = upload_to_slot(host, producer, d);
    if (upload == UploadStatus::Cancelled) return;
    if (upload == UploadStatus::Failed) {
        signal_shutdown(host);
        return;
    }
    host.negotiated.store(true, std::memory_order_release);
    rstd_info("waywallen-image-renderer: NegotiateBuffers honored "
              "(path={} mem_source={} modifier=0x{:016x}) — bind+frame emitted",
              static_cast<uint32_t>(d.path),
              static_cast<uint32_t>(d.memory_source),
              static_cast<unsigned long long>(d.format.modifier));
}

void apply_control(HostState& host, ww_bridge_control_t& c) {
    switch (c.op) {
    case WW_EVT_IN_INIT:
        // Init is consumed by ww_bridge_recv_init at the top of main
        // before the reader thread is even spawned. Anything that
        // arrives here is either a buggy daemon resending it or a
        // protocol violation; log and ignore to stay liberal.
        rstd_warn("waywallen-image-renderer: unexpected late Init; ignoring");
        break;
    case WW_EVT_IN_PLAY:
    case WW_EVT_IN_PAUSE:
    case WW_EVT_IN_POINTER_MOTION:
    case WW_EVT_IN_POINTER_BUTTON:
    case WW_EVT_IN_POINTER_AXIS:
        // image renderer doesn't subscribe to pointer events; daemon
        // already gates these (manifest sans `events`), but stay
        // permissive in case a misconfigured daemon forwards anyway.
        break;
    case WW_EVT_IN_SETTING_CHANGED: {
        const auto& settings = c.u.setting_changed.settings;
        {
            for (uint32_t i = 0; i < settings.count; ++i) {
                const char* key = settings.data[i].key;
                const char* val = settings.data[i].value;
                if (! key || ! val) continue;
                if (std::strcmp(key, kSchemeColorKey) == 0) {
                    set_scheme_color(host, val, true);
                } else {
                    rstd_warn("waywallen-image-renderer: ApplySettings: unknown key '{}'; "
                              "ignoring",
                              static_cast<const char*>(key));
                }
            }
        }
        break;
    }
    case WW_EVT_IN_SHUTDOWN: signal_shutdown(host); break;
    case WW_EVT_IN_NEGOTIATE_BUFFERS: {
        ww_pool_directive_t d = c.u.negotiate_buffers.directive;
        /* Static image: one slot is enough. */
        d.count = 1;
        {
            std::lock_guard<std::mutex> lk(host.neg_mu);
            host.neg_directive = d;
            host.neg_pending   = true;
        }
        host.neg_cv.notify_all();
        break;
    }
    case WW_EVT_IN_REQUEST_FRAME: {
        {
            std::lock_guard<std::mutex> lk(host.neg_mu);
            host.frame_request_pending = true;
        }
        host.neg_cv.notify_all();
        break;
    }
    case WW_EVT_IN_SET_LOG_LEVEL: ww_renderer_log_set_level(c.u.set_log_level.level); break;
    default:
        rstd_warn("waywallen-image-renderer: unknown control op {}", static_cast<int>(c.op));
        break;
    }
}

void reader_loop(HostState& host) {
    while (! host.shutdown.load(std::memory_order_acquire)) {
        ww_bridge_control_t msg {};
        int                 rc = ww_bridge_recv_control(host.sock, &msg);
        if (rc != 0) {
            if (! host.shutdown.load(std::memory_order_acquire)) {
                rstd_error("waywallen-image-renderer: recv_control failed: {}", rc);
            }
            signal_shutdown(host);
            return;
        }
        apply_control(host, msg);
        ww_bridge_control_free(&msg);
    }
}

// ---------------------------------------------------------------------------
// --print-caps
// ---------------------------------------------------------------------------

// Emit a single JSON document on stdout that mirrors the
// `PeerCapsJson` shape consumed by `dmabuf_roundtrip_e2e`. Keep the
// field names and ordering in sync with
// `displays/dump-test/src/main.rs::PeerCapsJson`.
//
// We don't have a public "query caps without a socket" entry point on
// the bridge pool; instead we build a Vulkan pool, hand it one end of
// a `socketpair(AF_UNIX)`, ask it to advertise, then drain the
// `format_caps` message on the other end and decode it.
static int print_caps_json(const Options& opt) {
    auto producer_res =
        wavsen::video::Producer::create(rstd::u32(opt.width), rstd::u32(opt.height));
    if (producer_res.is_err()) {
        rstd_error("waywallen-image-renderer: vk_producer: {}",
                   std::move(producer_res).unwrap_err().message.as_str());
        return 1;
    }
    auto producer = std::move(producer_res).unwrap();

    int sv[2] = { -1, -1 };
    if (::socketpair(AF_UNIX, SOCK_STREAM, 0, sv) != 0) {
        rstd_error("waywallen-image-renderer: socketpair: {}",
                   static_cast<const char*>(std::strerror(errno)));
        return 1;
    }

    ww_pool_vulkan_init_t pool_init {};
    pool_init.instance           = producer->instance();
    pool_init.physical_device    = producer->physical_device();
    pool_init.device             = producer->device();
    pool_init.queue              = producer->queue();
    pool_init.queue_family_index = producer->queue_family_index().to_primitive();
    pool_init.get_instance_proc_addr =
        reinterpret_cast<void* (*)(void*, const char*)>(vkGetInstanceProcAddr);
    pool_init.device_uuid = producer->device_uuid();
    pool_init.driver_uuid = producer->driver_uuid();
    {
        ww_bridge_vk_dt_t dt {};
        ww_bridge_vk_dt_load(&dt, vkGetInstanceProcAddr, producer->instance());
        if (int rc = ww_bridge_vk_query_render_node(&dt,
                                                    producer->physical_device(),
                                                    &pool_init.drm_render_major,
                                                    &pool_init.drm_render_minor);
            rc != 0) {
            rstd_warn("waywallen-image-renderer: drm render-node query failed ({}); "
                      "topology will be unknown to daemon",
                      rc);
        }
    }
    pool_init.drm_render_fd = producer->drm_render_fd();
    /* Image plugin uses vkCmdCopyBufferToImage (TRANSFER_DST feature)
     * to upload decoded pixels into the slot. */
    pool_init.image_usage_flags    = VK_IMAGE_USAGE_TRANSFER_DST_BIT;
    pool_init.format_feature_flags = VK_FORMAT_FEATURE_TRANSFER_DST_BIT;

    ww_pool_t* pool = nullptr;
    if (int rc = ww_bridge_pool_create(WW_POOL_BACKEND_VULKAN, &pool_init, &pool); rc != 0) {
        rstd_error("waywallen-image-renderer: pool_create: {}", rc);
        ::close(sv[0]);
        ::close(sv[1]);
        return 1;
    }

    if (int rc = ww_bridge_pool_advertise_caps(pool,
                                               sv[0],
                                               opt.width,
                                               opt.height,
                                               WW_MEM_HINT_DEVICE_LOCAL | WW_MEM_HINT_HOST_VISIBLE);
        rc != 0) {
        rstd_error("waywallen-image-renderer: advertise_caps: {}", rc);
        ww_bridge_pool_destroy(pool);
        ::close(sv[0]);
        ::close(sv[1]);
        return 1;
    }

    /* Drain frames on sv[1] until we get the FormatCaps. The pool
     * writes (in order): Ready, ReleaseSyncobj (with a syncobj fd),
     * FormatCaps. */
    ww_evt_format_caps_t caps {};
    bool                 got_caps = false;
    for (int frame = 0; frame < 6 && ! got_caps; ++frame) {
        uint16_t op       = 0;
        uint8_t* body     = nullptr;
        size_t   body_len = 0;
        int      fds[2]   = { -1, -1 };
        size_t   n_fds    = 0;
        int      rc       = ww_bridge_recv_frame(sv[1], &op, &body, &body_len, fds, 2, &n_fds);
        if (rc != 0) {
            rstd_error("waywallen-image-renderer: recv_frame: {}", rc);
            break;
        }
        for (size_t i = 0; i < n_fds; ++i) {
            if (fds[i] >= 0) ::close(fds[i]);
        }
        if (op == WW_EVT_FORMAT_CAPS) {
            if (ww_evt_format_caps_decode(body, body_len, &caps) == 0) {
                got_caps = true;
            }
        }
        free(body);
    }

    ww_bridge_pool_destroy(pool);
    ::close(sv[0]);
    ::close(sv[1]);

    if (! got_caps) {
        rstd_error("waywallen-image-renderer: did not observe FormatCaps");
        return 1;
    }
    const auto& capabilities = caps.capabilities;

    auto put_uuid = [](const ww_array_u32_t& a) -> std::string {
        // device_uuid / driver_uuid are 16 bytes packed as 4×u32 LE on
        // the wire. Unpack back to 16 bytes for the JSON output.
        uint8_t bytes[16] = { 0 };
        for (uint32_t i = 0; i < a.count && i < 4; ++i) {
            uint32_t v       = a.data[i];
            bytes[i * 4 + 0] = static_cast<uint8_t>(v & 0xff);
            bytes[i * 4 + 1] = static_cast<uint8_t>((v >> 8) & 0xff);
            bytes[i * 4 + 2] = static_cast<uint8_t>((v >> 16) & 0xff);
            bytes[i * 4 + 3] = static_cast<uint8_t>((v >> 24) & 0xff);
        }
        std::string s = "[";
        for (int i = 0; i < 16; ++i) {
            char buf[8];
            std::snprintf(buf, sizeof(buf), "%s%u", i ? "," : "", bytes[i]);
            s += buf;
        }
        s += "]";
        return s;
    };

    std::printf("{\n");
    std::printf("  \"by_fourcc\": {\n");
    size_t cursor = 0;
    for (uint32_t i = 0; i < capabilities.fourccs.count; ++i) {
        const uint32_t fc = capabilities.fourccs.data[i];
        const uint32_t n  = capabilities.mod_counts.data[i];
        std::printf("    \"0x%08x\": [", fc);
        for (uint32_t j = 0; j < n; ++j) {
            std::printf("%s\n      {\"modifier\": %llu, \"plane_count\": %u}",
                        j ? "," : "",
                        static_cast<unsigned long long>(capabilities.modifiers.data[cursor + j]),
                        capabilities.plane_counts.data[cursor + j]);
        }
        cursor += n;
        std::printf("\n    ]%s\n", (i + 1 < capabilities.fourccs.count) ? "," : "");
    }
    std::printf("  },\n");
    std::printf("  \"device_uuid\": %s,\n", put_uuid(capabilities.device_uuid).c_str());
    std::printf("  \"driver_uuid\": %s,\n", put_uuid(capabilities.driver_uuid).c_str());
    std::printf("  \"drm_render_major\": %u,\n", capabilities.drm_node.major);
    std::printf("  \"drm_render_minor\": %u,\n", capabilities.drm_node.minor);
    std::printf("  \"sync\": %u,\n", capabilities.sync_caps);
    std::printf("  \"color\": %u,\n", capabilities.color_caps);
    std::printf("  \"mem_hint\": %u,\n", capabilities.mem_hints);
    std::printf("  \"extent_max_w\": %u,\n", capabilities.max_extent.width);
    std::printf("  \"extent_max_h\": %u\n", capabilities.max_extent.height);
    std::printf("}\n");
    std::fflush(stdout);
    ww_evt_format_caps_free(&caps);
    return 0;
}

} // namespace

namespace waywallen::image
{

int run(int argc, char** argv) {
    ww_renderer_log_init();

    auto parsed_args = parse_args(argc, argv);
    if (! parsed_args.should_run) return parsed_args.exit_code;
    Options opt = std::move(parsed_args.options);

    if (opt.print_caps) {
        return print_caps_json(opt);
    }

    if (opt.vulkan_probe) {
        auto prod_res =
            wavsen::video::Producer::create(rstd::u32(opt.width), rstd::u32(opt.height));
        if (prod_res.is_err()) {
            rstd_error("waywallen-image-renderer: vk_producer: {}",
                       std::move(prod_res).unwrap_err().message.as_str());
            return 1;
        }
        auto prod = std::move(prod_res).unwrap();
        rstd_info("waywallen-image-renderer: vulkan_probe ok drm_render={}:{}",
                  prod->drm_render_major(),
                  prod->drm_render_minor());
        return 0;
    }

    if (opt.decode_only) {
        if (opt.image_path.empty()) die("--decode-only requires --path");
        ww_image::DecodeError derr;
        ww_image::RgbaBuf     buf = ww_image::decode_to_rgba(opt.image_path,
                                                             /* resolution = */ 0,
                                                             &derr);
        if (buf.data.empty()) {
            rstd_error("waywallen-image-renderer: decode failed: {}", derr.message);
            return 1;
        }
        uint64_t sum = 0;
        for (uint8_t b : buf.data) sum += b;
        rstd_info("waywallen-image-renderer: decoded {}x{} stride={} "
                  "bytes={} pixel_sum={}",
                  buf.width,
                  buf.height,
                  buf.stride,
                  buf.data.size(),
                  static_cast<unsigned long long>(sum));
        return 0;
    }

    if (opt.ipc_path.empty()) die("--ipc <socket_path> is required");

    ::prctl(PR_SET_PDEATHSIG, SIGTERM);

    /* --- Connect first, then read the Init message ---
     *
     * Step 3: connect() moved to before any decode / Vulkan init so
     * the daemon's typed Init payload (extent + image path) drives
     * the GPU pipeline rather than CLI argv. The legacy `--image`/
     * `--width`/`--height` argv is still emitted by the daemon
     * double-send but we ignore it here. */
    HostState host;
    host.sock = ww_bridge_connect(opt.ipc_path.c_str());
    if (host.sock < 0) die("ww_bridge_connect: " + std::string(std::strerror(-host.sock)));

    waywallen_renderer_init_t init {};
    if (int rc = ww_bridge_recv_init(host.sock, &init); rc < 0) {
        // Surface the rejection structured-ly so the daemon's spawn()
        // gets a useful error string. `init.spawn_version` is filled
        // by recv_init even on -EPROTO (version mismatch).
        const char* reason = (rc == -EPROTO) ? "init: protocol error or unsupported spawn_version"
                                             : "init: recv failed";
        waywallen_init_rejection_t rejection {
            .received_protocol_version  = init.protocol_version,
            .supported_protocol_version = WW_BRIDGE_SUPPORTED_PROTOCOL_VERSION,
            .received_spawn_version     = init.spawn_version,
            .supported_spawn_version    = WW_BRIDGE_SUPPORTED_SPAWN_VERSION,
            .reason                     = const_cast<char*>(reason),
        };
        ww_bridge_send_init_nack(host.sock, &rejection);
        waywallen_renderer_init_free(&init);
        die(std::string(reason) + " rc=" + std::to_string(rc));
    }

    // Image path arrives via CLI argv `--path` (already parsed into
    // opt.image_path). Init carries only the resolved settings kv.
    for (size_t i = 0; i < init.settings.count; ++i) {
        const ww_kv_t& kv = init.settings.data[i];
        if (! kv.key || ! kv.value) continue;
        if (opt.render_node.empty() && std::strcmp(kv.key, "render_node") == 0 && *kv.value) {
            opt.render_node = kv.value;
        } else if (std::strcmp(kv.key, "resolution") == 0 && *kv.value) {
            char* end      = nullptr;
            long  n        = std::strtol(kv.value, &end, 10);
            opt.resolution = (end != kv.value) ? ww_resolution_sanitize(static_cast<int32_t>(n))
                                               : static_cast<int32_t>(WW_RESOLUTION_1080P);
        }
    }
    apply_user_properties(host, init.user_properties);
    waywallen_renderer_init_free(&init);

    /* --- Decode + Vulkan setup --- */
    if (opt.image_path.empty()) die("--path <image-file> is required");
    ww_image::DecodeError derr;
    ww_image::RgbaBuf rgba_buf = ww_image::decode_to_rgba(opt.image_path, opt.resolution, &derr);
    if (rgba_buf.data.empty()) die("decode " + opt.image_path + ": " + derr.message);

    /* `decode_to_rgba` already applied the resolution cap against the
     * image's native size; use whatever extent it landed on. */
    opt.width  = rgba_buf.width;
    opt.height = rgba_buf.height;

    auto producer_res =
        opt.render_node.empty()
            ? wavsen::video::Producer::create(rstd::u32(opt.width), rstd::u32(opt.height))
            : wavsen::video::Producer::create_with_render_node(
                  rstd::u32(opt.width), rstd::u32(opt.height), as_rstd_str(opt.render_node));
    if (producer_res.is_err()) {
        die("vk_producer: " + to_std_string(std::move(producer_res).unwrap_err().message));
    }
    auto producer = std::move(producer_res).unwrap();

    /* GPU info diagnostic (uses bridge probe_vk dispatch table). */
    ww_bridge_vk_dt_t vdt {};
    ww_bridge_vk_dt_load(&vdt, vkGetInstanceProcAddr, producer->instance());
    ww_bridge_vk_log_gpu_info("waywallen-image-renderer", &vdt, producer->physical_device());

    host.rgba_data = rgba_buf.data.data();
    host.rgba_size = rgba_buf.data.size();

    /* --- Bridge pool: hand over Vulkan handles --- */
    ww_pool_vulkan_init_t pool_init {};
    pool_init.instance           = producer->instance();
    pool_init.physical_device    = producer->physical_device();
    pool_init.device             = producer->device();
    pool_init.queue              = producer->queue();
    pool_init.queue_family_index = producer->queue_family_index().to_primitive();
    pool_init.get_instance_proc_addr =
        reinterpret_cast<void* (*)(void*, const char*)>(vkGetInstanceProcAddr);
    pool_init.device_uuid = producer->device_uuid();
    pool_init.driver_uuid = producer->driver_uuid();
    {
        ww_bridge_vk_dt_t dt {};
        ww_bridge_vk_dt_load(&dt, vkGetInstanceProcAddr, producer->instance());
        if (int rc = ww_bridge_vk_query_render_node(&dt,
                                                    producer->physical_device(),
                                                    &pool_init.drm_render_major,
                                                    &pool_init.drm_render_minor);
            rc != 0) {
            rstd_warn("waywallen-image-renderer: drm render-node query failed ({}); "
                      "topology will be unknown to daemon",
                      rc);
        }
    }
    pool_init.drm_render_fd        = producer->drm_render_fd();
    pool_init.image_usage_flags    = VK_IMAGE_USAGE_TRANSFER_DST_BIT;
    pool_init.format_feature_flags = VK_FORMAT_FEATURE_TRANSFER_DST_BIT;

    if (int rc = ww_bridge_pool_create(WW_POOL_BACKEND_VULKAN, &pool_init, &host.pool); rc != 0)
        die("ww_bridge_pool_create failed: " + std::to_string(rc));

    /* Bridge sends ready + release_syncobj + format_caps in one go. */
    if (int rc = ww_bridge_pool_advertise_caps(host.pool,
                                               host.sock,
                                               opt.width,
                                               opt.height,
                                               WW_MEM_HINT_DEVICE_LOCAL | WW_MEM_HINT_HOST_VISIBLE);
        rc != 0)
        die("ww_bridge_pool_advertise_caps failed: " + std::to_string(rc));

    publish_clear_color(host, host.scheme_color);
    rstd_info("waywallen-image-renderer: ready, advertised caps, "
              "waiting for NegotiateBuffers");

    std::thread reader([&]() {
        reader_loop(host);
    });

    /* Main loop: negotiation publishes new content; request_frame only
     * republishes the latest released slot. */
    while (! host.shutdown.load(std::memory_order_acquire)) {
        std::unique_lock<std::mutex> lk(host.neg_mu);
        host.neg_cv.wait(lk, [&] {
            return host.neg_pending ||
                   (host.frame_request_pending &&
                    host.negotiated.load(std::memory_order_acquire)) ||
                   host.shutdown.load(std::memory_order_acquire);
        });
        if (host.shutdown.load(std::memory_order_acquire)) break;
        if (host.neg_pending) {
            ww_pool_directive_t d      = host.neg_directive;
            host.neg_pending           = false;
            host.frame_request_pending = false;
            lk.unlock();
            apply_negotiate_request(host, *producer, d);
            continue;
        }
        if (host.frame_request_pending) {
            host.frame_request_pending = false;
            lk.unlock();
            const auto status = republish_latest(host);
            if (status == RepublishStatus::Failed) {
                signal_shutdown(host);
            }
        }
    }

    if (reader.joinable()) {
        ::shutdown(host.sock, SHUT_RD);
        reader.join();
    }
    if (host.pool) ww_bridge_pool_destroy(host.pool);
    ww_bridge_close(host.sock);
    return 0;
}

} // namespace waywallen::image
