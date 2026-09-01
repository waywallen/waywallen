module;

#include <rstd/macro.hpp>

#include <errno.h>
#include <signal.h>
#include <stdio.h>

#include <sys/prctl.h>
#include <sys/socket.h>
#include <unistd.h>

module waywallen.video.entry;

import rstd.cppstd;
import rstd.argparse;
import rstd.json;
import rstd.log;
import wavsen.video;
import wavsen.audio;
import waywallen.bridge;

namespace
{

using namespace rstd::literals;

struct Options {
    std::string ipc_path;
    std::string video_path;
    std::string render_node; // e.g. "/dev/dri/renderD128"; empty → auto-pick
    std::string hwdec;       // selftest: "auto" | "vulkan" | "vaapi" | "none"
    uint32_t    width { 1920 };
    uint32_t    height { 1080 };
    bool        loop_file { true };
    bool        selftest { false };
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

constexpr const char* kSchemeColorKey   = "waywallen.scheme_color";
constexpr const char* kEnableAudioKey   = "waywallen.enable_audio";
constexpr const char* kPlaybackSpeedKey = "waywallen.playback_speed";

[[noreturn]] void die(const std::string& msg) {
    rstd_error("waywallen-video-renderer: {}", msg);
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
        rstd_error("waywallen-video-renderer: argparse match access failed: {}",
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

bool parse_bool_wire(const char* raw, bool& out) {
    if (! raw) return false;
    std::string s     = raw;
    const auto  first = s.find_first_not_of(" \t\r\n");
    if (first == std::string::npos) return false;
    const auto last = s.find_last_not_of(" \t\r\n");
    s               = s.substr(first, last - first + 1);
    for (char& ch : s) {
        if (ch >= 'A' && ch <= 'Z') ch = static_cast<char>(ch - 'A' + 'a');
    }
    if (s == "true" || s == "1" || s == "yes" || s == "on") {
        out = true;
        return true;
    }
    if (s == "false" || s == "0" || s == "no" || s == "off") {
        out = false;
        return true;
    }
    return false;
}

bool playback_rate_from_percent(double pct, float& out) {
    if (! std::isfinite(pct) || pct < 10.0 || pct > 400.0) return false;
    out = static_cast<float>(pct / 100.0);
    return true;
}

bool parse_playback_rate_wire(const char* raw, float& out) {
    if (! raw || ! *raw) return false;
    errno      = 0;
    char*  end = nullptr;
    double pct = std::strtod(raw, &end);
    if (end == raw || errno == ERANGE) return false;
    while (*end && std::isspace(static_cast<unsigned char>(*end))) ++end;
    return ! *end && playback_rate_from_percent(pct, out);
}

// SPAWN_VERSION 3: video path arrives via `--path`; everything else
// (loop_file, hwdec, render_node, fps, volume) rides on Init.settings
// kv. Keep `--no-loop` / `--render-node` as standalone-debug escape
// hatches (set them before init; daemon doesn't emit them).
ParseArgsResult parse_args(int argc, char** argv) {
    using namespace rstd::argparse;

    auto command = Command::make("waywallen-video-renderer"_str);
    command.about("Render video wallpapers for waywallen"_str);
    auto ipc  = command.add_arg(Arg<rstd::string::String>::value("ipc"_str, string_parser())
                                    .long_name("ipc"_str)
                                    .value_name("SOCKET"_str)
                                    .help("Connect to the renderer IPC socket"_str));
    auto path = command.add_arg(Arg<rstd::string::String>::value("path"_str, string_parser())
                                    .long_name("path"_str)
                                    .value_name("VIDEO"_str)
                                    .help("Video wallpaper path"_str));
    command.add_arg(Arg<bool>::flag("no-loop"_str)
                        .long_name("no-loop"_str)
                        .help("Disable playback looping"_str));
    auto render_node =
        command.add_arg(Arg<rstd::string::String>::value("render-node"_str, string_parser())
                            .long_name("render-node"_str)
                            .value_name("DEVICE"_str)
                            .help("Use a specific DRM render node"_str));
    auto hwdec = command.add_arg(Arg<rstd::string::String>::value("hwdec"_str, string_parser())
                                     .long_name("hwdec"_str)
                                     .value_name("MODE"_str)
                                     .help("Select the hardware decoding mode"_str));
    auto selftest =
        command.add_arg(Arg<rstd::string::String>::value("selftest"_str, string_parser())
                            .long_name("selftest"_str)
                            .value_name("VIDEO"_str)
                            .help("Run the standalone decoder self-test"_str));

    auto built = std::move(command).build();
    if (built.is_err()) {
        rstd_error("waywallen-video-renderer: invalid CLI definition: {}",
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
        options.video_path = to_std_string(**value);
    }
    if (auto value = get_arg(*matches, render_node); value.is_some()) {
        options.render_node = to_std_string(**value);
    }
    if (auto value = get_arg(*matches, hwdec); value.is_some()) {
        options.hwdec = to_std_string(**value);
    }
    if (auto value = get_arg(*matches, selftest); value.is_some()) {
        options.selftest   = true;
        options.video_path = to_std_string(**value);
    }
    options.loop_file = ! matches->contains("no-loop"_str);
    return { .options = std::move(options), .should_run = true };
}

const char* kv_get(const ww_kv_list_t& kv, const char* key) {
    for (uint32_t i = 0; i < kv.count; ++i) {
        if (kv.data[i].key && std::strcmp(kv.data[i].key, key) == 0) return kv.data[i].value;
    }
    return nullptr;
}

wavsen::audio::AudioClientIdentity audio_client_identity() {
    return {
        .application_name =
            rstd::string::String::make(as_rstd_str(WAYWALLEN_AUDIO_APPLICATION_NAME)),
        .application_id = rstd::string::String::make(as_rstd_str(WAYWALLEN_AUDIO_APPLICATION_ID)),
        .stream_prefix  = rstd::string::String::make(as_rstd_str(WAYWALLEN_AUDIO_STREAM_PREFIX)),
        .component      = rstd::string::String::make("video"_str),
        .media_name     = rstd::string::String::make("Waywallen Video Renderer"_str),
        .media_role     = rstd::string::String::make("music"_str),
    };
}

struct HostState {
    int               sock { -1 };
    ww_pool_t*        pool { nullptr };
    std::atomic<bool> shutdown { false };
    std::atomic<bool> negotiated { false };
    std::atomic<bool> paused { false };
    std::atomic<bool> muted { false };
    std::atomic<bool> audio_gate_open { false };
    std::atomic<bool> settings_enable_audio { true };
    std::atomic<bool> property_enable_audio { true };

    std::mutex              neg_mu;
    std::condition_variable neg_cv;
    bool                    neg_pending { false };
    uint64_t                frame_request_revision { 0 };
    uint64_t                served_frame_request_revision { 0 };
    ww_pool_directive_t     neg_directive {};
    std::mutex              send_mu;
    std::string             reported_hwdec;

    std::atomic<bool> loop_pending { false };
    std::atomic<bool> loop_value { true };

    /* hwdec changes are applied at the next file/loop boundary, not
     * mid-stream — store the pending value here. */
    std::mutex  hwdec_mu;
    std::string pending_hwdec;
    bool        hwdec_pending { false };

    /* Audio runtime settings — applied to AvPlayer atomically when
     * pending flag is set; no decoder rebuild. */
    std::atomic<uint32_t> pending_volume { 100 };
    std::atomic<bool>     volume_pending { false };
    std::atomic<bool>     enable_audio_pending { false };
    std::atomic<float>    playback_rate { 1.0f };
    std::atomic<bool>     playback_rate_pending { false };
    std::atomic<uint32_t> mute_fade_ms { 0 };
    std::atomic<uint32_t> pause_fade_ms { 0 };
    ClearColor            scheme_color {};
};

bool effective_audio_enabled(const HostState& host) {
    return host.settings_enable_audio.load(std::memory_order_acquire) &&
           host.property_enable_audio.load(std::memory_order_acquire);
}

using Clock = std::chrono::steady_clock;

struct AudioRuntime {
    uint32_t          volume_pct { 100 };
    bool              enabled { true };
    bool              paused { false };
    bool              muted { false };
    bool              device_muted { false };
    bool              pause_pending { false };
    Clock::time_point pause_at {};
    float             target_scale { 1.0f };
};

wavsen::video::HwAccel parse_hwdec(const char* v) {
    if (! v || ! *v) return wavsen::video::HwAccel::Auto;
    if (std::strcmp(v, "vulkan") == 0) return wavsen::video::HwAccel::Vulkan;
    if (std::strcmp(v, "vaapi") == 0) return wavsen::video::HwAccel::Vaapi;
    if (std::strcmp(v, "none") == 0) return wavsen::video::HwAccel::None;
    return wavsen::video::HwAccel::Auto;
}

const char* hwdec_label(wavsen::video::HwAccel h) {
    switch (h) {
    case wavsen::video::HwAccel::Auto: return "auto";
    case wavsen::video::HwAccel::Vulkan: return "vulkan";
    case wavsen::video::HwAccel::Vaapi: return "vaapi";
    case wavsen::video::HwAccel::None: return "none";
    }
    return "?";
}

const char* kind_label(wavsen::video::FrameKind k) {
    switch (k) {
    case wavsen::video::FrameKind::Sw: return "sw";
    case wavsen::video::FrameKind::VulkanShared: return "vulkan-shared";
    case wavsen::video::FrameKind::VaapiDrm: return "vaapi-drm";
    }
    return "?";
}

const char* runtime_hwdec_label(wavsen::video::FrameKind kind) {
    switch (kind) {
    case wavsen::video::FrameKind::Sw: return "sw";
    case wavsen::video::FrameKind::VulkanShared: return "vulkan";
    case wavsen::video::FrameKind::VaapiDrm: return "vaapi";
    }
    return "unknown";
}

void signal_shutdown(HostState& s) {
    s.shutdown.store(true, std::memory_order_release);
    s.neg_cv.notify_all();
}

void publish_clear_color(HostState& host, const ClearColor& c) {
    std::lock_guard<std::mutex> send_lk(host.send_mu);
    if (int rc = ww_bridge_send_report_state_clear_color(host.sock, c.r, c.g, c.b, c.a); rc != 0) {
        rstd_warn("waywallen-video-renderer: report_state(clear_color) failed ({})", rc);
    }
}

void publish_hwdec_tag(HostState& host, wavsen::video::FrameKind kind) {
    const char*                 value = runtime_hwdec_label(kind);
    std::lock_guard<std::mutex> send_lk(host.send_mu);
    if (host.reported_hwdec == value) return;

    ww_kv_t tag {
        .key   = const_cast<char*>("hwdec"),
        .value = const_cast<char*>(value),
    };
    ww_kv_list_t tags {
        .count = 1,
        .data  = &tag,
    };
    if (int rc = ww_bridge_send_report_state_tags(host.sock, &tags); rc != 0) {
        rstd_warn("waywallen-video-renderer: report_state(runtime_tags) failed ({})", rc);
        return;
    }
    host.reported_hwdec = value;
}

void set_scheme_color(HostState& host, const char* value, bool publish) {
    ClearColor next {};
    if (value && *value && ! parse_color_wire(value, next)) {
        rstd_warn("waywallen-video-renderer: invalid {} value '{}'; ignoring",
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
        rstd_warn("waywallen-video-renderer: init.user_properties is not an object; ignored");
        return;
    }
    auto scheme = parsed.get("waywallen.scheme_color"_str);
    if (scheme.is_some()) {
        if (! (**scheme).is_string()) {
            rstd_warn("waywallen-video-renderer: {} is not a string; ignored",
                      static_cast<const char*>(kSchemeColorKey));
        } else {
            const auto value = rstd::cppstd::to_string(*(**scheme).as_str());
            set_scheme_color(host, value.c_str(), false);
        }
    }

    if (auto audio = parsed.get("waywallen.enable_audio"_str); audio.is_some()) {
        const auto& audio_value = **audio;
        bool        enabled     = true;
        bool        valid       = false;
        if (audio_value.is_boolean()) {
            enabled = *audio_value.as_bool();
            valid   = true;
        } else if (audio_value.is_string()) {
            const auto value = rstd::cppstd::to_string(*audio_value.as_str());
            if (! parse_bool_wire(value.c_str(), enabled)) {
                rstd_warn("waywallen-video-renderer: invalid {} value '{}'; ignoring",
                          static_cast<const char*>(kEnableAudioKey),
                          value.c_str());
            } else {
                valid = true;
            }
        } else {
            rstd_warn("waywallen-video-renderer: {} is not a bool/string; ignored",
                      static_cast<const char*>(kEnableAudioKey));
        }
        if (valid) {
            host.property_enable_audio.store(enabled, std::memory_order_release);
        }
    }

    if (auto speed = parsed.get("waywallen.playback_speed"_str); speed.is_some()) {
        float rate  = 1.0f;
        bool  valid = false;
        if ((**speed).is_string()) {
            const auto value = rstd::cppstd::to_string(*(**speed).as_str());
            valid            = parse_playback_rate_wire(value.c_str(), rate);
        } else if ((**speed).is_number()) {
            auto pct = (**speed).as_f64();
            valid    = pct.is_some() && playback_rate_from_percent(pct->to_primitive(), rate);
        }
        if (! valid) {
            rstd_warn("waywallen-video-renderer: invalid {} initial value; ignoring",
                      static_cast<const char*>(kPlaybackSpeedKey));
        } else {
            host.playback_rate.store(rate, std::memory_order_release);
        }
    }
}

void set_property_enable_audio(HostState& host, const char* value) {
    bool enabled = true;
    if (! parse_bool_wire(value, enabled)) {
        rstd_warn("waywallen-video-renderer: invalid {} value '{}'; ignoring",
                  static_cast<const char*>(kEnableAudioKey),
                  static_cast<const char*>(value ? value : ""));
        return;
    }
    host.property_enable_audio.store(enabled, std::memory_order_release);
    host.enable_audio_pending.store(true, std::memory_order_release);
}

void set_property_playback_rate(HostState& host, const char* value) {
    float rate = 1.0f;
    if (! parse_playback_rate_wire(value, rate)) {
        rstd_warn("waywallen-video-renderer: invalid {} value '{}'; ignoring",
                  static_cast<const char*>(kPlaybackSpeedKey),
                  static_cast<const char*>(value ? value : ""));
        return;
    }
    host.playback_rate.store(rate, std::memory_order_release);
    host.playback_rate_pending.store(true, std::memory_order_release);
}

void apply_control(HostState& host, ww_bridge_control_t& c) {
    switch (c.op) {
    case WW_EVT_IN_INIT:
        rstd_warn("waywallen-video-renderer: unexpected late Init; ignoring");
        break;
    case WW_EVT_IN_PLAY:
        host.pause_fade_ms.store(c.u.play.transition.fade_ms, std::memory_order_release);
        host.paused.store(false, std::memory_order_release);
        host.audio_gate_open.store(true, std::memory_order_release);
        host.neg_cv.notify_all();
        break;
    case WW_EVT_IN_PAUSE:
        host.pause_fade_ms.store(c.u.pause.transition.fade_ms, std::memory_order_release);
        host.paused.store(true, std::memory_order_release);
        host.neg_cv.notify_all();
        break;
    case WW_EVT_IN_UNMUTE:
        host.mute_fade_ms.store(c.u.unmute.transition.fade_ms, std::memory_order_release);
        host.muted.store(false, std::memory_order_release);
        host.audio_gate_open.store(true, std::memory_order_release);
        break;
    case WW_EVT_IN_MUTE:
        host.mute_fade_ms.store(c.u.mute.transition.fade_ms, std::memory_order_release);
        host.muted.store(true, std::memory_order_release);
        break;
    case WW_EVT_IN_POINTER_MOTION:
    case WW_EVT_IN_POINTER_BUTTON:
    case WW_EVT_IN_POINTER_AXIS: break;
    case WW_EVT_IN_SETTING_CHANGED: {
        const auto& settings = c.u.setting_changed.settings;
        for (uint32_t i = 0; i < settings.count; ++i) {
            const char* key = settings.data[i].key;
            const char* val = settings.data[i].value;
            if (! key || ! val) continue;
            if (std::strcmp(key, "loop_file") == 0) {
                bool enabled = ! (std::strcmp(val, "no") == 0);
                host.loop_value.store(enabled, std::memory_order_release);
                host.loop_pending.store(true, std::memory_order_release);
            } else if (std::strcmp(key, "hwdec") == 0) {
                std::lock_guard<std::mutex> lk(host.hwdec_mu);
                host.pending_hwdec = val;
                host.hwdec_pending = true;
            } else if (std::strcmp(key, "volume") == 0) {
                int n = std::atoi(val);
                if (n < 0) n = 0;
                if (n > 100) n = 100;
                host.pending_volume.store(static_cast<uint32_t>(n), std::memory_order_release);
                host.volume_pending.store(true, std::memory_order_release);
            } else if (std::strcmp(key, "enable_audio") == 0) {
                bool v = true;
                if (! parse_bool_wire(val, v)) {
                    rstd_warn("waywallen-video-renderer: invalid enable_audio value '{}'; ignoring",
                              static_cast<const char*>(val));
                    continue;
                }
                host.settings_enable_audio.store(v, std::memory_order_release);
                host.enable_audio_pending.store(true, std::memory_order_release);
            } else if (std::strcmp(key, kSchemeColorKey) == 0) {
                set_scheme_color(host, val, true);
            } else if (std::strcmp(key, kEnableAudioKey) == 0) {
                set_property_enable_audio(host, val);
            } else if (std::strcmp(key, kPlaybackSpeedKey) == 0) {
                set_property_playback_rate(host, val);
            } else {
                rstd_warn("waywallen-video-renderer: ApplySettings: unknown key '{}'; ignoring",
                          static_cast<const char*>(key));
            }
        }
        host.neg_cv.notify_all();
        break;
    }
    case WW_EVT_IN_SHUTDOWN: signal_shutdown(host); break;
    case WW_EVT_IN_NEGOTIATE_BUFFERS: {
        const ww_pool_directive_t& d = c.u.negotiate_buffers.directive;
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
            if (host.frame_request_revision == std::numeric_limits<uint64_t>::max()) {
                host.frame_request_revision        = 1;
                host.served_frame_request_revision = 0;
            } else {
                ++host.frame_request_revision;
            }
        }
        host.neg_cv.notify_all();
        break;
    }
    case WW_EVT_IN_SET_LOG_LEVEL: ww_renderer_log_set_level(c.u.set_log_level.level); break;
    default:
        rstd_warn("waywallen-video-renderer: unknown control op {}", static_cast<int>(c.op));
        break;
    }
}

uint64_t pending_frame_request(HostState& host) {
    std::lock_guard<std::mutex> lk(host.neg_mu);
    return host.frame_request_revision > host.served_frame_request_revision
               ? host.frame_request_revision
               : 0;
}

void complete_frame_request(HostState& host, uint64_t revision) {
    if (revision == 0) return;
    std::lock_guard<std::mutex> lk(host.neg_mu);
    host.served_frame_request_revision = std::max(host.served_frame_request_revision, revision);
}

int cancel_republish_wait(void* userdata) {
    auto& host = *static_cast<HostState*>(userdata);
    if (host.shutdown.load(std::memory_order_acquire)) return 1;
    std::lock_guard<std::mutex> lk(host.neg_mu);
    return host.neg_pending ? 1 : 0;
}

bool republish_latest(HostState& host, uint64_t revision) {
    std::lock_guard<std::mutex> send_lk(host.send_mu);
    ww_pool_republish_result_t  result {};
    const int                   rc = ww_bridge_pool_wait_republish_latest(
        host.pool, host.sock, cancel_republish_wait, &host, &result);
    if (rc != 0) {
        rstd_error("waywallen-video-renderer: republish contract failed: {}", rc);
        return false;
    }
    switch (result.status) {
    case WW_POOL_REPUBLISH_PUBLISHED:
        complete_frame_request(host, revision);
        rstd_debug("waywallen-video-renderer: republished slot {} seq={}",
                   result.slot_index,
                   result.sequence);
        return true;
    case WW_POOL_REPUBLISH_CANCELLED: return true;
    case WW_POOL_REPUBLISH_NO_CONTENT:
    case WW_POOL_REPUBLISH_BUSY:
        rstd_error("waywallen-video-renderer: current frame cannot be republished "
                   "(status={}, error={})",
                   static_cast<int>(result.status),
                   result.error_code);
        return false;
    case WW_POOL_REPUBLISH_SESSION_LOST:
    case WW_POOL_REPUBLISH_ERROR:
        rstd_error("waywallen-video-renderer: republish failed (status={}, error={})",
                   static_cast<int>(result.status),
                   result.error_code);
        return false;
    }
    return false;
}

void apply_audio_scale(wavsen::audio::AvPlayer* av_player, AudioRuntime& audio, float scale,
                       uint32_t fade_ms, bool force = false) {
    if (! av_player) return;
    scale = clamp01(scale);
    if (! force && audio.target_scale == scale) return;
    audio.target_scale = scale;
    av_player->set_volume_scale(rstd::f32(scale), rstd::u32(fade_ms));
}

bool set_audio_device_enabled(wavsen::audio::AvPlayer* av_player, AudioRuntime& audio, bool enabled,
                              rstd::f64 seek_seconds, uint32_t volume_pct) {
    if (! av_player) {
        audio.enabled       = enabled;
        audio.pause_pending = false;
        return false;
    }
    if (! enabled) {
        av_player->close_device();
        audio.enabled       = false;
        audio.pause_pending = false;
        audio.device_muted  = false;
        audio.target_scale  = -1.0f;
        return true;
    }
    if (! av_player->is_device_open()) {
        if (seek_seconds >= rstd::f64() && seek_seconds.is_finite()) {
            av_player->seek_to(seek_seconds);
        }
        if (! av_player->open_device()) {
            rstd_warn("waywallen-video-renderer: audio device open failed");
            audio.enabled       = false;
            audio.pause_pending = false;
            audio.device_muted  = false;
            audio.target_scale  = -1.0f;
            return false;
        }
        av_player->set_volume(rstd::f32(static_cast<float>(volume_pct) / 100.0f));
        audio.target_scale = -1.0f;
    }
    audio.enabled = true;
    return true;
}

void sync_audio_state(wavsen::audio::AvPlayer* av_player, AudioRuntime& audio, bool paused,
                      bool muted, uint32_t pause_fade_ms, uint32_t mute_fade_ms,
                      Clock::time_point now) {
    if (! av_player) {
        audio.paused        = paused;
        audio.muted         = muted;
        audio.pause_pending = false;
        return;
    }
    if (! audio.enabled) {
        av_player->close_device();
        audio.paused        = paused;
        audio.muted         = muted;
        audio.pause_pending = false;
        audio.device_muted  = false;
        return;
    }

    const bool was_audible   = ! audio.paused && ! audio.muted;
    const bool will_audible  = ! paused && ! muted;
    const bool pause_changed = audio.paused != paused;
    const bool mute_changed  = audio.muted != muted;
    uint32_t   fade_ms       = 0;
    if (was_audible != will_audible) {
        if (pause_changed) {
            fade_ms = pause_fade_ms;
        } else if (mute_changed) {
            fade_ms = mute_fade_ms;
        }
    }

    if (audio.device_muted) {
        av_player->set_muted(false);
        audio.device_muted = false;
    }

    if (! paused && av_player->is_paused()) av_player->play();

    audio.paused = paused;
    audio.muted  = muted;
    apply_audio_scale(av_player, audio, will_audible ? 1.0f : 0.0f, fade_ms);

    if (paused) {
        if (! av_player->is_paused()) {
            if (audio.pause_pending) {
                if (now >= audio.pause_at) {
                    av_player->pause();
                    audio.pause_pending = false;
                }
            } else if (was_audible && fade_ms > 0) {
                audio.pause_pending = true;
                audio.pause_at      = now + std::chrono::milliseconds(fade_ms);
            } else {
                av_player->pause();
                audio.pause_pending = false;
            }
        } else {
            audio.pause_pending = false;
        }
    } else {
        audio.pause_pending = false;
    }
}

// --selftest: open a video, decode frames, run YuvToRgba against
// throw-away VkImages we allocate ourselves. Validates that the GPU
// pipeline (device, shader compile/load, descriptor set, queue submit)
// works on this box before relying on the renderer in a real shell. No
// IPC, no daemon — strictly local. Returns 0 on success.
int run_selftest(const Options& opt) {
    if (opt.video_path.empty()) {
        rstd_error("waywallen-video-renderer: --selftest needs a video path");
        return 1;
    }

    uint32_t even_w = opt.width + (opt.width & 1u);
    uint32_t even_h = opt.height + (opt.height & 1u);

    auto producer_res = wavsen::video::Producer::create_with_render_node(
        rstd::u32(even_w), rstd::u32(even_h), as_rstd_str(opt.render_node));
    if (producer_res.is_err()) {
        rstd_error("selftest vk: {}", std::move(producer_res).unwrap_err().message.as_str());
        return 1;
    }
    auto producer = std::move(producer_res).unwrap();

    auto yuv_res = wavsen::video::YuvToRgba::create(producer->instance(),
                                                    producer->physical_device(),
                                                    producer->device(),
                                                    producer->queue_family_index(),
                                                    producer->queue(),
                                                    rstd::u32(even_w),
                                                    rstd::u32(even_h));
    if (yuv_res.is_err()) {
        rstd_error("selftest yuv: {}", std::move(yuv_res).unwrap_err().message.as_str());
        return 1;
    }
    auto yuv = std::move(yuv_res).unwrap();

    wavsen::video::OpenOpts dec_opts {
        parse_hwdec(opt.hwdec.empty() ? nullptr : opt.hwdec.c_str()),
        rstd::string::String::make(as_rstd_str(opt.render_node)),
    };
    auto decoder_res = wavsen::video::VideoDecoder::open_with_vk(as_rstd_str(opt.video_path),
                                                                 rstd::u32(even_w),
                                                                 rstd::u32(even_h),
                                                                 /*loop=*/false,
                                                                 *producer,
                                                                 dec_opts);
    if (decoder_res.is_err()) {
        rstd_error("selftest decode: {}", std::move(decoder_res).unwrap_err().message.as_str());
        return 1;
    }
    auto decoder = std::move(decoder_res).unwrap();
    rstd_info("selftest: hwdec={}, decoder kind={}",
              hwdec_label(dec_opts.hwaccel),
              kind_label(decoder->kind()));

    const auto     kind         = decoder->kind();
    const auto     target_count = kind == wavsen::video::FrameKind::Sw ? 1u : 3u;
    VkImage        dst_images[3] {};
    VkDeviceMemory dst_memories[3] {};
    for (uint32_t target_index = 0; target_index < target_count; ++target_index) {
        VkImageCreateInfo ici {};
        ici.sType         = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO;
        ici.imageType     = VK_IMAGE_TYPE_2D;
        ici.format        = VK_FORMAT_R8G8B8A8_UNORM;
        ici.extent        = { even_w, even_h, 1 };
        ici.mipLevels     = 1;
        ici.arrayLayers   = 1;
        ici.samples       = VK_SAMPLE_COUNT_1_BIT;
        ici.tiling        = VK_IMAGE_TILING_OPTIMAL;
        ici.usage         = VK_IMAGE_USAGE_STORAGE_BIT | VK_IMAGE_USAGE_TRANSFER_SRC_BIT;
        ici.sharingMode   = VK_SHARING_MODE_EXCLUSIVE;
        ici.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
        if (vkCreateImage(producer->device(), &ici, nullptr, &dst_images[target_index]) !=
            VK_SUCCESS) {
            rstd_error("selftest vkCreateImage failed");
            return 1;
        }
        VkMemoryRequirements mr {};
        vkGetImageMemoryRequirements(producer->device(), dst_images[target_index], &mr);
        VkPhysicalDeviceMemoryProperties mp {};
        vkGetPhysicalDeviceMemoryProperties(producer->physical_device(), &mp);
        uint32_t type = std::numeric_limits<uint32_t>::max();
        for (uint32_t i = 0; i < mp.memoryTypeCount; ++i) {
            if ((mr.memoryTypeBits & (1u << i)) &&
                (mp.memoryTypes[i].propertyFlags & VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT)) {
                type = i;
                break;
            }
        }
        if (type == std::numeric_limits<uint32_t>::max()) {
            rstd_error("selftest no DEVICE_LOCAL memory");
            return 1;
        }
        VkMemoryAllocateInfo mai {};
        mai.sType           = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO;
        mai.allocationSize  = mr.size;
        mai.memoryTypeIndex = type;
        if (vkAllocateMemory(producer->device(), &mai, nullptr, &dst_memories[target_index]) !=
                VK_SUCCESS ||
            vkBindImageMemory(
                producer->device(), dst_images[target_index], dst_memories[target_index], 0) !=
                VK_SUCCESS) {
            rstd_error("selftest vkAllocateMemory/Bind failed");
            return 1;
        }
    }

    /* Rotate targets repeatedly so reservation covers both fresh and reused images. */
    int sync_fd = -1;
    if (kind == wavsen::video::FrameKind::VulkanShared) {
        for (uint32_t frame_index = 0; frame_index < target_count * 3; ++frame_index) {
            const auto target_index = frame_index % target_count;
            auto       fs_res       = decoder->next_vk_frame();
            if (fs_res.is_err()) {
                rstd_error("selftest next_vk_frame: {}",
                           rstd::move(fs_res).unwrap_err().message.as_str());
                return 1;
            }
            auto pull = rstd::move(fs_res).unwrap();
            if (pull.status != wavsen::video::NextFrame::Ok || pull.frame.is_none()) return 1;
            const auto& info = pull.frame->info();
            const auto  cm   = wavsen::video::make_color_matrix(
                static_cast<wavsen::video::ColorSpace>(info.colorspace.to_primitive()),
                static_cast<wavsen::video::ColorRange>(info.color_range.to_primitive()));
            auto reserved = yuv->reserve({
                .target = {
                    .image  = dst_images[target_index],
                    .width  = rstd::u32(even_w),
                    .height = rstd::u32(even_h),
                    .kind   = wavsen::video::ConvertTarget::BridgeForeign,
                },
                .deadline = rstd::Some(rstd::time::Instant::now() +
                                       rstd::time::Duration::from_secs(rstd::u64(1))),
            });
            if (reserved.is_err() || reserved->is_none()) return 1;
            auto cv_res =
                yuv->submit_av_vk_frame(rstd::move(**reserved), rstd::move(*pull.frame), cm);
            if (cv_res.is_err()) {
                rstd_error("selftest convert: {}", std::move(cv_res).unwrap_err().message.as_str());
                sync_fd = -1;
                break;
            }
            if (sync_fd >= 0) ::close(sync_fd);
            sync_fd = std::move(cv_res).unwrap().sync_fd;
        }
    } else if (kind == wavsen::video::FrameKind::VaapiDrm) {
        for (uint32_t frame_index = 0; frame_index < target_count * 3; ++frame_index) {
            const auto target_index = frame_index % target_count;
            auto       fs_res       = decoder->next_vaapi_frame();
            if (fs_res.is_err()) {
                rstd_error("selftest next_vaapi_frame: {}",
                           std::move(fs_res).unwrap_err().message.as_str());
                return 1;
            }
            auto pull = rstd::move(fs_res).unwrap();
            if (pull.status != wavsen::video::NextFrame::Ok || pull.frame.is_none()) return 1;
            const auto& vaapi = pull.frame->view();
            const auto  cm    = wavsen::video::make_color_matrix(
                static_cast<wavsen::video::ColorSpace>(vaapi.colorspace.to_primitive()),
                static_cast<wavsen::video::ColorRange>(vaapi.color_range.to_primitive()));
            auto reserved = yuv->reserve({
            .target = {
                .image  = dst_images[target_index],
                .width  = rstd::u32(even_w),
                .height = rstd::u32(even_h),
                .kind   = wavsen::video::ConvertTarget::BridgeForeign,
            },
            .deadline = rstd::Some(rstd::time::Instant::now() +
                                   rstd::time::Duration::from_secs(rstd::u64(1))),
        });
            if (reserved.is_err() || reserved->is_none()) return 1;
            auto mapped = rstd::move(*pull.frame).into_drm();
            if (mapped.is_err()) {
                rstd_error("selftest VAAPI DRM mapping: {}",
                           rstd::move(mapped).unwrap_err().message.as_str());
                return 1;
            }
            auto        drm_frame = rstd::move(mapped).unwrap();
            const auto& drmv      = drm_frame.view();
            rstd_info("selftest drm_prime: {}x{}, modifier=0x{:x}, objects={}, layers={}",
                      drmv.width,
                      drmv.height,
                      drmv.objects[0].format_modifier,
                      drmv.object_count,
                      drmv.layer_count);
            auto cv_res = yuv->submit_drm_prime(rstd::move(**reserved), rstd::move(drm_frame), cm);
            if (cv_res.is_err()) {
                rstd_error("selftest convert (drm): {}",
                           rstd::move(cv_res).unwrap_err().message.as_str());
                sync_fd = -1;
                break;
            } else {
                auto submitted = rstd::move(cv_res).unwrap();
                if (submitted.is_none()) {
                    sync_fd = -1;
                    break;
                }
                if (sync_fd >= 0) ::close(sync_fd);
                sync_fd = submitted->sync_fd;
            }
        }
    } else {
        wavsen::video::Nv12Frame frame;
        auto                     fs_res = decoder->next_frame(frame);
        if (fs_res.is_err()) {
            rstd_error("selftest next_frame: {}", std::move(fs_res).unwrap_err().message.as_str());
            return 1;
        }
        if (std::move(fs_res).unwrap() != wavsen::video::NextFrame::Ok) return 1;
        const auto cm = wavsen::video::make_color_matrix(
            static_cast<wavsen::video::ColorSpace>(frame.colorspace.to_primitive()),
            static_cast<wavsen::video::ColorRange>(frame.color_range.to_primitive()));
        auto cv_res = yuv->convert_nv12(dst_images[0],
                                        rstd::u32(even_w),
                                        rstd::u32(even_h),
                                        frame.data.data(),
                                        frame.data.len(),
                                        cm);
        if (cv_res.is_err()) {
            rstd_error("selftest convert: {}", std::move(cv_res).unwrap_err().message.as_str());
            sync_fd = -1;
        } else {
            sync_fd = std::move(cv_res).unwrap();
        }
    }

    /* Release exported handles, then wait once before destroying test targets. */
    if (sync_fd >= 0) {
        ::close(sync_fd);
    }
    if (kind == wavsen::video::FrameKind::Sw) {
        (void)vkDeviceWaitIdle(producer->device());
    } else if (auto drained = yuv->drain_submissions(rstd::u64(1'000'000'000)); drained.is_err()) {
        rstd_error("selftest conversion drain: {}",
                   rstd::move(drained).unwrap_err().message.as_str());
        (void)vkDeviceWaitIdle(producer->device());
    }
    (void)yuv->reclaim_submissions();
    (void)yuv->invalidate_targets();
    for (uint32_t target_index = 0; target_index < target_count; ++target_index) {
        vkDestroyImage(producer->device(), dst_images[target_index], nullptr);
        vkFreeMemory(producer->device(), dst_memories[target_index], nullptr);
    }

    if (sync_fd < 0) return 1;
    rstd_info("waywallen-video-renderer: --selftest ok (kind={}, {}x{})",
              kind_label(decoder->kind()),
              even_w,
              even_h);
    return 0;
}

void reader_loop(HostState& host) {
    while (! host.shutdown.load(std::memory_order_acquire)) {
        ww_bridge_control_t msg {};
        int                 rc = ww_bridge_recv_control(host.sock, &msg);
        if (rc != 0) {
            if (! host.shutdown.load(std::memory_order_acquire)) {
                rstd_error("waywallen-video-renderer: recv_control failed: {}", rc);
            }
            signal_shutdown(host);
            return;
        }
        apply_control(host, msg);
        ww_bridge_control_free(&msg);
    }
}

} // namespace

namespace waywallen::video
{

int run(int argc, char** argv) {
    ww_renderer_log_init();

    auto parsed_args = parse_args(argc, argv);
    if (! parsed_args.should_run) return parsed_args.exit_code;
    Options opt = std::move(parsed_args.options);
    if (opt.selftest) return run_selftest(opt);
    if (opt.ipc_path.empty()) die("--ipc <socket_path> is required");

    ::prctl(PR_SET_PDEATHSIG, SIGTERM);

    HostState host;
    host.sock = ww_bridge_connect(opt.ipc_path.c_str());
    if (host.sock < 0) die("ww_bridge_connect: " + std::string(std::strerror(-host.sock)));

    waywallen_renderer_init_t init {};
    if (int rc = ww_bridge_recv_init(host.sock, &init); rc < 0) {
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
    // Video path arrives via CLI argv `--path` (already in
    // opt.video_path). Init carries only the resolved settings kv.
    if (const char* v = kv_get(init.settings, "loop_file")) {
        opt.loop_file = ! (std::strcmp(v, "no") == 0);
    }
    if (opt.render_node.empty()) {
        if (const char* v = kv_get(init.settings, "render_node"); v && *v) {
            opt.render_node = v;
        }
    }
    wavsen::video::HwAccel hwaccel = wavsen::video::HwAccel::Auto;
    if (const char* v = kv_get(init.settings, "hwdec")) {
        hwaccel = parse_hwdec(v);
    }
    bool     enable_audio = true;
    uint32_t volume_pct   = 100;
    if (const char* v = kv_get(init.settings, "enable_audio")) {
        if (! parse_bool_wire(v, enable_audio)) {
            rstd_warn("waywallen-video-renderer: invalid enable_audio setting '{}'; using true",
                      static_cast<const char*>(v));
            enable_audio = true;
        }
    }
    if (const char* v = kv_get(init.settings, "volume")) {
        int n = std::atoi(v);
        if (n < 0) n = 0;
        if (n > 100) n = 100;
        volume_pct = static_cast<uint32_t>(n);
    }
    host.pending_volume.store(volume_pct, std::memory_order_release);
    host.settings_enable_audio.store(enable_audio, std::memory_order_release);
    int32_t resolution = static_cast<int32_t>(WW_RESOLUTION_ORIGIN);
    if (const char* v = kv_get(init.settings, "resolution"); v && *v) {
        char* end  = nullptr;
        long  n    = std::strtol(v, &end, 10);
        resolution = (end != v) ? ww_resolution_sanitize(static_cast<int32_t>(n))
                                : static_cast<int32_t>(WW_RESOLUTION_1080P);
    }
    apply_user_properties(host, init.user_properties);
    waywallen_renderer_init_free(&init);
    if (opt.video_path.empty()) die("--path <video-file> is required");

    /* Probe the file's native dimensions. `Producer::create_with_render_node`
     * needs the final size up front, so this has to happen here in
     * main, not inside VideoDecoder. */
    uint32_t native_w = 0, native_h = 0;
    {
        auto probe_res = wavsen::video::VideoDecoder::probe_native(as_rstd_str(opt.video_path));
        if (probe_res.is_err()) {
            die("probe_native " + opt.video_path + ": " +
                to_std_string(std::move(probe_res).unwrap_err().message));
        }
        auto probe = std::move(probe_res).unwrap();
        native_w   = probe.width.to_primitive();
        native_h   = probe.height.to_primitive();
    }
    opt.width  = native_w;
    opt.height = native_h;
    ww_resolution_apply_cap(resolution, WW_RESOLUTION_CAP_DEFAULT, &opt.width, &opt.height);

    /* NV12 chroma is 4:2:0 → both extents must be even. The decoder
     * rounds up internally too; do it here so all our state agrees. */
    uint32_t even_w = opt.width + (opt.width & 1u);
    uint32_t even_h = opt.height + (opt.height & 1u);

    /* --- Vulkan device first, so the decoder can share it --- */
    auto producer_res = wavsen::video::Producer::create_with_render_node(
        rstd::u32(even_w), rstd::u32(even_h), as_rstd_str(opt.render_node));
    if (producer_res.is_err()) {
        die("vk producer: " + to_std_string(std::move(producer_res).unwrap_err().message));
    }
    auto producer = std::move(producer_res).unwrap();

    /* --- Decoder: hwaccel chain per the `hwdec` setting (Auto =
     *   Vulkan → VAAPI → SW). VAAPI takes the render_node path; on any
     *   per-frame mapping failure we fall through to sw via the helper. */
    wavsen::video::OpenOpts dec_opts {
        hwaccel,
        rstd::string::String::make(as_rstd_str(opt.render_node)),
    };
    auto decoder_res = wavsen::video::VideoDecoder::open_with_vk(as_rstd_str(opt.video_path),
                                                                 rstd::u32(even_w),
                                                                 rstd::u32(even_h),
                                                                 opt.loop_file,
                                                                 *producer,
                                                                 dec_opts);
    if (decoder_res.is_err()) {
        die("decode " + opt.video_path + ": " +
            to_std_string(std::move(decoder_res).unwrap_err().message));
    }
    auto decoder = rstd::Some(std::move(decoder_res).unwrap());
    host.loop_value.store(opt.loop_file, std::memory_order_release);
    rstd_info("waywallen-video-renderer: hwdec={}, decoder kind={}",
              hwdec_label(hwaccel),
              kind_label(decoder->get()->kind()));

    /* --- Audio: open the same file as an rstd byte stream and attach AvPlayer.
     *   Failure (missing audio stream, unsupported codec, no audio device)
     *   is non-fatal: log and continue without audio (presenter falls
     *   back to wall-clock pacing). */
    rstd::Option<rstd::boxed::Box<wavsen::audio::AvPlayer>> av_player;
    {
        auto audio_file_res =
            wavsen::audio::open_file(rstd::ref<rstd::path::Path>(as_rstd_str(opt.video_path)));
        if (audio_file_res.is_err()) {
            rstd_warn("waywallen-video-renderer: audio file open failed");
        } else {
            auto p_res = wavsen::audio::AvPlayer::open(
                rstd::move(audio_file_res).unwrap_unchecked(), false, audio_client_identity());
            if (p_res.is_err()) {
                rstd_warn("waywallen-video-renderer: audio open failed: {}",
                          std::move(p_res).unwrap_err().message);
            } else {
                auto       player = rstd::move(p_res).unwrap();
                const auto rate   = host.playback_rate.load(std::memory_order_acquire);
                if (! player->set_playback_rate(rstd::f64(static_cast<double>(rate)))) {
                    rstd_warn("waywallen-video-renderer: invalid initial playback rate {}",
                              rstd::f32(rate));
                }
                player->set_volume(rstd::f32(volume_pct / 100.0f));
                player->set_volume_scale(rstd::f32());
                av_player.insert(rstd::move(player));
                rstd_info("waywallen-video-renderer: audio decoder attached (volume={}%)",
                          volume_pct);
            }
        }
    }
    auto current_av_player = [&av_player]() -> wavsen::audio::AvPlayer* {
        return av_player.is_some() ? av_player->get() : nullptr;
    };
    AudioRuntime audio_runtime {
        .volume_pct   = volume_pct,
        .enabled      = false,
        .paused       = false,
        .muted        = true,
        .device_muted = false,
    };
    set_audio_device_enabled(current_av_player(),
                             audio_runtime,
                             effective_audio_enabled(host),
                             rstd::f64(-1.0),
                             volume_pct);

    ww_bridge_vk_dt_t vdt {};
    ww_bridge_vk_dt_load(&vdt, vkGetInstanceProcAddr, producer->instance());
    ww_bridge_vk_log_gpu_info("waywallen-video-renderer", &vdt, producer->physical_device());

    auto yuv_res = wavsen::video::YuvToRgba::create(producer->instance(),
                                                    producer->physical_device(),
                                                    producer->device(),
                                                    producer->queue_family_index(),
                                                    producer->queue(),
                                                    rstd::u32(even_w),
                                                    rstd::u32(even_h));
    if (yuv_res.is_err()) {
        die("yuv_to_rgba: " + to_std_string(rstd::move(yuv_res).unwrap_err().message));
    }
    auto yuv                      = rstd::move(yuv_res).unwrap();
    auto prepare_pool_reconfigure = [&](uint32_t slot_count) {
        auto drained = yuv->drain_submissions(rstd::u64(1'000'000'000));
        if (drained.is_err()) {
            rstd_error("waywallen-video-renderer: drain conversions: {}",
                       rstd::move(drained).unwrap_err().message.as_str());
            return false;
        }
        auto invalidated = yuv->invalidate_targets();
        if (invalidated.is_err()) {
            rstd_error("waywallen-video-renderer: invalidate conversion targets: {}",
                       rstd::move(invalidated).unwrap_err().message.as_str());
            return false;
        }
        auto configured = yuv->configure_pipeline({
            .max_in_flight   = rstd::u32(slot_count),
            .max_drm_imports = rstd::u32(32),
        });
        if (configured.is_err()) {
            rstd_error("waywallen-video-renderer: configure conversion pipeline: {}",
                       rstd::move(configured).unwrap_err().message.as_str());
            return false;
        }
        return true;
    };

    /* --- Bridge pool --- */
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
            rstd_warn("waywallen-video-renderer: drm render-node query failed ({}); "
                      "topology will be unknown to daemon",
                      rc);
        }
    }
    pool_init.drm_render_fd = producer->drm_render_fd();
    /* The bridge's slot VkImage will be the dst of our compute shader's
     * storage-image binding, so it needs STORAGE usage in addition to
     * the default TRANSFER_DST. The modifier filter mirrors the
     * required features. */
    pool_init.image_usage_flags = VK_IMAGE_USAGE_STORAGE_BIT | VK_IMAGE_USAGE_TRANSFER_DST_BIT;
    pool_init.format_feature_flags =
        VK_FORMAT_FEATURE_STORAGE_IMAGE_BIT | VK_FORMAT_FEATURE_TRANSFER_DST_BIT;

    if (int rc = ww_bridge_pool_create(WW_POOL_BACKEND_VULKAN, &pool_init, &host.pool); rc != 0)
        die("ww_bridge_pool_create failed: " + std::to_string(rc));

    if (int rc = ww_bridge_pool_advertise_caps(host.pool,
                                               host.sock,
                                               opt.width,
                                               opt.height,
                                               WW_MEM_HINT_DEVICE_LOCAL | WW_MEM_HINT_HOST_VISIBLE);
        rc != 0)
        die("ww_bridge_pool_advertise_caps failed: " + std::to_string(rc));

    publish_clear_color(host, host.scheme_color);
    publish_hwdec_tag(host, decoder->get()->kind());
    rstd_info("waywallen-video-renderer: ready ({}x{}, loop={}, GPU YUV→RGB), "
              "waiting for NegotiateBuffers",
              even_w,
              even_h,
              opt.loop_file ? 1 : 0);

    std::thread reader([&]() {
        reader_loop(host);
    });

    /* Block until first NegotiateBuffers. */
    {
        std::unique_lock<std::mutex> lk(host.neg_mu);
        host.neg_cv.wait(lk, [&] {
            return host.neg_pending || host.shutdown.load(std::memory_order_acquire);
        });
        if (host.neg_pending && ! host.shutdown.load(std::memory_order_acquire)) {
            ww_pool_directive_t d = host.neg_directive;
            host.neg_pending      = false;
            lk.unlock();
            if (! prepare_pool_reconfigure(d.count)) {
                signal_shutdown(host);
            } else {
                int rc = 0;
                {
                    std::lock_guard<std::mutex> send_lk(host.send_mu);
                    rc = ww_bridge_pool_apply_directive(host.pool, host.sock, &d);
                }
                if (rc != 0) {
                    rstd_error("waywallen-video-renderer: pool_apply_directive (initial) rc={}",
                               rc);
                    signal_shutdown(host);
                } else {
                    host.negotiated.store(true, std::memory_order_release);
                }
            }
        }
    }

    rstd_info("waywallen-video-renderer: decoder mode = {}", kind_label(decoder->get()->kind()));

    /* --- Main loop ----------------------------------------------------- */
    wavsen::video::Presenter presenter; // PTS-driven pacing.
    (void)presenter.set_playback_rate(
        rstd::f64(static_cast<double>(host.playback_rate.load(std::memory_order_acquire))));
    if (auto* player = current_av_player()) {
        presenter.set_external_clock([p = player] {
            return p->current_time_seconds();
        });
    }
    wavsen::video::Nv12Frame frame;
    rstd::f64                prev_pts { -1.0 };      // for loop-boundary detection (PTS regression)
    uint32_t                 stall_warn_counter = 0; // throttle ETIME log spam during backpressure
    bool                     submitted_since_negotiate = false;

    while (! host.shutdown.load(std::memory_order_acquire)) {
        {
            std::unique_lock<std::mutex> lk(host.neg_mu);
            if (host.neg_pending) {
                ww_pool_directive_t d = host.neg_directive;
                host.neg_pending      = false;
                lk.unlock();
                if (! prepare_pool_reconfigure(d.count)) {
                    signal_shutdown(host);
                    break;
                }
                int rc = 0;
                {
                    std::lock_guard<std::mutex> send_lk(host.send_mu);
                    rc = ww_bridge_pool_apply_directive(host.pool, host.sock, &d);
                }
                if (rc != 0) {
                    rstd_error("waywallen-video-renderer: pool_apply_directive (re) rc={}", rc);
                    if (rc > 0) {
                        signal_shutdown(host);
                        break;
                    }
                } else {
                    submitted_since_negotiate = false;
                }
            }
        }

        if (host.loop_pending.exchange(false, std::memory_order_acq_rel)) {
            decoder->get()->set_loop(host.loop_value.load(std::memory_order_acquire));
            // Loop toggled — let the presenter re-baseline on next frame.
            presenter.reset();
        }

        /* Runtime audio controls do not reopen the media decoder. */
        if (host.volume_pending.exchange(false, std::memory_order_acq_rel)) {
            audio_runtime.volume_pct = host.pending_volume.load(std::memory_order_acquire);
            if (auto* player = current_av_player()) {
                player->set_volume(
                    rstd::f32(static_cast<float>(audio_runtime.volume_pct) / 100.0f));
            }
        }
        if (host.playback_rate_pending.exchange(false, std::memory_order_acq_rel)) {
            const auto rate  = host.playback_rate.load(std::memory_order_acquire);
            const auto value = rstd::f64(static_cast<double>(rate));
            if (auto* player = current_av_player(); player && ! player->set_playback_rate(value)) {
                rstd_warn("waywallen-video-renderer: failed to apply audio playback rate {}",
                          rstd::f32(rate));
            }
            if (! presenter.set_playback_rate(value)) {
                rstd_warn("waywallen-video-renderer: failed to apply playback rate {}",
                          rstd::f32(rate));
            }
        }
        if (host.enable_audio_pending.exchange(false, std::memory_order_acq_rel)) {
            const bool audio_enabled = effective_audio_enabled(host);
            set_audio_device_enabled(current_av_player(),
                                     audio_runtime,
                                     audio_enabled,
                                     prev_pts,
                                     audio_runtime.volume_pct);
            presenter.reset();
        }
        const bool     audio_gate_open = host.audio_gate_open.load(std::memory_order_acquire);
        const bool     paused_now      = host.paused.load(std::memory_order_acquire);
        const bool     muted_now = ! audio_gate_open || host.muted.load(std::memory_order_acquire);
        const bool     resumed_now   = audio_runtime.paused && ! paused_now;
        const uint64_t frame_request = pending_frame_request(host);
        sync_audio_state(current_av_player(),
                         audio_runtime,
                         paused_now,
                         muted_now,
                         host.pause_fade_ms.load(std::memory_order_acquire),
                         host.mute_fade_ms.load(std::memory_order_acquire),
                         Clock::now());
        if (resumed_now) presenter.reset();

        /* hwdec change requested — apply at this loop boundary by
         * tearing down + reopening the decoder. The reopen runs the
         * full hwaccel trial again with the new mode. */
        {
            std::string new_hwdec;
            bool        do_reopen = false;
            {
                std::lock_guard<std::mutex> lk(host.hwdec_mu);
                if (host.hwdec_pending) {
                    new_hwdec          = host.pending_hwdec;
                    host.hwdec_pending = false;
                    do_reopen          = true;
                }
            }
            if (do_reopen) {
                wavsen::video::HwAccel new_h = parse_hwdec(new_hwdec.c_str());
                if (new_h != hwaccel) {
                    rstd_info("waywallen-video-renderer: hwdec change {} → {}, reopening decoder",
                              hwdec_label(hwaccel),
                              hwdec_label(new_h));
                    auto drained = yuv->drain_submissions(rstd::u64(1'000'000'000));
                    if (drained.is_err()) {
                        rstd_error("waywallen-video-renderer: drain before decoder reopen: {}",
                                   rstd::move(drained).unwrap_err().message.as_str());
                        signal_shutdown(host);
                        break;
                    }
                    (void)decoder.take();
                    wavsen::video::OpenOpts new_opts {
                        new_h, rstd::string::String::make(as_rstd_str(opt.render_node))
                    };
                    auto re_res = wavsen::video::VideoDecoder::open_with_vk(
                        as_rstd_str(opt.video_path),
                        rstd::u32(even_w),
                        rstd::u32(even_h),
                        host.loop_value.load(std::memory_order_acquire),
                        *producer,
                        new_opts);
                    if (re_res.is_err()) {
                        rstd_error("waywallen-video-renderer: reopen failed: {}",
                                   std::move(re_res).unwrap_err().message.as_str());
                        signal_shutdown(host);
                        break;
                    }
                    decoder = rstd::Some(std::move(re_res).unwrap());
                    hwaccel = new_h;
                    presenter.reset();
                    // Video reopened at PTS 0 — keep audio aligned.
                    if (auto* player = current_av_player()) player->seek_to_start();
                    prev_pts = rstd::f64(-1.0);
                    publish_hwdec_tag(host, decoder->get()->kind());
                    rstd_info("waywallen-video-renderer: reopened, kind={}",
                              kind_label(decoder->get()->kind()));
                }
            }
        }

        if (paused_now && submitted_since_negotiate) {
            if (frame_request != 0) {
                if (! republish_latest(host, frame_request)) {
                    signal_shutdown(host);
                    break;
                }
                continue;
            }
            std::unique_lock<std::mutex> lk(host.neg_mu);
            auto                         wake = [&] {
                return host.shutdown.load(std::memory_order_acquire) || host.neg_pending ||
                       ! host.paused.load(std::memory_order_acquire) ||
                       host.playback_rate_pending.load(std::memory_order_acquire) ||
                       host.frame_request_revision > host.served_frame_request_revision;
            };
            if (audio_runtime.pause_pending) {
                host.neg_cv.wait_until(lk, audio_runtime.pause_at, wake);
            } else {
                host.neg_cv.wait(lk, wake);
            }
            continue;
        }

        rstd::f64                                                    frame_pts { -1.0 };
        const auto                                                   fkind = decoder->get()->kind();
        rstd::Option<wavsen::video::VkFrameLease>                    vulkan_frame;
        rstd::Option<wavsen::video::VaapiFrameLease>                 vaapi_frame;
        rstd::Result<wavsen::video::NextFrame, wavsen::video::Error> fs_res =
            rstd::Ok(wavsen::video::NextFrame::Ok);
        switch (fkind) {
        case wavsen::video::FrameKind::VulkanShared: {
            auto pulled = decoder->get()->next_vk_frame();
            if (pulled.is_err()) {
                fs_res = rstd::Err(rstd::move(pulled).unwrap_err());
            } else {
                auto value   = rstd::move(pulled).unwrap();
                fs_res       = rstd::Ok(value.status);
                vulkan_frame = rstd::move(value.frame);
            }
            break;
        }
        case wavsen::video::FrameKind::VaapiDrm: {
            auto pulled = decoder->get()->next_vaapi_frame();
            if (pulled.is_err()) {
                fs_res = rstd::Err(rstd::move(pulled).unwrap_err());
            } else {
                auto value  = rstd::move(pulled).unwrap();
                fs_res      = rstd::Ok(value.status);
                vaapi_frame = rstd::move(value.frame);
            }
            break;
        }
        case wavsen::video::FrameKind::Sw: fs_res = decoder->get()->next_frame(frame); break;
        }
        if (fs_res.is_err()) {
            rstd_error("waywallen-video-renderer: decode error (hwdec={}): {}",
                       hwdec_label(hwaccel),
                       std::move(fs_res).unwrap_err().message.as_str());
            signal_shutdown(host);
            break;
        }
        const auto fs = std::move(fs_res).unwrap();
        if (fs == wavsen::video::NextFrame::Eof) {
            if (frame_request != 0 && submitted_since_negotiate) {
                if (! republish_latest(host, frame_request)) {
                    signal_shutdown(host);
                    break;
                }
                continue;
            }
            rstd_info("waywallen-video-renderer: clean EOF (loop=off); idling until shutdown");
            std::unique_lock<std::mutex> lk(host.neg_mu);
            host.neg_cv.wait(lk, [&] {
                return host.shutdown.load(std::memory_order_acquire) || host.neg_pending ||
                       host.loop_pending.load(std::memory_order_acquire) ||
                       host.playback_rate_pending.load(std::memory_order_acquire) ||
                       host.frame_request_revision > host.served_frame_request_revision;
            });
            continue;
        }
        const bool decoder_looped = fs == wavsen::video::NextFrame::Looped;
        switch (fkind) {
        case wavsen::video::FrameKind::VulkanShared:
            if (vulkan_frame.is_none()) {
                rstd_error("waywallen-video-renderer: Vulkan decode returned no frame lease");
                signal_shutdown(host);
                continue;
            }
            frame_pts = vulkan_frame->info().pts_seconds;
            break;
        case wavsen::video::FrameKind::VaapiDrm:
            if (vaapi_frame.is_none()) {
                rstd_error("waywallen-video-renderer: VAAPI decode returned no surface lease");
                signal_shutdown(host);
                continue;
            }
            frame_pts = vaapi_frame->view().pts_seconds;
            break;
        case wavsen::video::FrameKind::Sw: frame_pts = frame.pts_seconds; break;
        }

        const bool pts_regressed = frame_pts >= rstd::f64() && prev_pts >= rstd::f64() &&
                                   frame_pts + rstd::f64(0.5) < prev_pts;
        if (decoder_looped || pts_regressed) {
            if (auto* player = current_av_player()) player->seek_to_start();
            presenter.reset();
        }
        prev_pts = frame_pts;

        const auto presentation = presenter.schedule_frame(frame_pts);
        if (! presentation.present) continue;

        ww_pool_slot_acquire_result_t acquired {};
        int acquire_rc = ww_bridge_pool_try_acquire_any_for_render(host.pool, &acquired);
        if (acquire_rc != 0) {
            rstd_error("waywallen-video-renderer: acquire slot contract failed: {}", acquire_rc);
            signal_shutdown(host);
            break;
        }
        if (acquired.status == WW_POOL_SLOT_ACQUIRE_BUSY) {
            if ((stall_warn_counter++ % 30) == 0) {
                rstd_warn("waywallen-video-renderer: all slots are busy, dropping frame");
            }
            wavsen::video::Presenter::wait_until(presentation);
            continue;
        }
        stall_warn_counter = 0;
        if (acquired.status == WW_POOL_SLOT_ACQUIRE_SESSION_LOST ||
            acquired.status == WW_POOL_SLOT_ACQUIRE_ERROR) {
            rstd_error("waywallen-video-renderer: cannot acquire a slot (status={}, error={})",
                       static_cast<int>(acquired.status),
                       acquired.error_code);
            signal_shutdown(host);
            break;
        }
        const auto& s = acquired.slot;
        if (! s.vk_image) {
            ww_bridge_pool_abort_acquired_slot(host.pool, &acquired.identity);
            rstd_error("waywallen-video-renderer: slot {} has no VkImage handle", s.index);
            signal_shutdown(host);
            break;
        }

        const wavsen::video::ConversionTargetView conversion_target {
            .image  = reinterpret_cast<VkImage>(s.vk_image),
            .width  = rstd::u32(s.width),
            .height = rstd::u32(s.height),
            .kind   = wavsen::video::ConvertTarget::BridgeForeign,
        };
        rstd::Option<wavsen::video::ConversionReservation> conversion_reservation;
        if (fkind != wavsen::video::FrameKind::Sw) {
            auto reserved = yuv->reserve({
                .target   = conversion_target,
                .deadline = presentation.deadline,
            });
            if (reserved.is_err()) {
                ww_bridge_pool_abort_acquired_slot(host.pool, &acquired.identity);
                rstd_error("waywallen-video-renderer: reserve conversion: {}",
                           rstd::move(reserved).unwrap_err().message.as_str());
                signal_shutdown(host);
                break;
            }
            auto value = rstd::move(reserved).unwrap();
            if (value.is_none()) {
                ww_bridge_pool_abort_acquired_slot(host.pool, &acquired.identity);
                wavsen::video::Presenter::wait_until(presentation);
                continue;
            }
            conversion_reservation = rstd::move(value);
        }

        int       sync_fd = -1;
        rstd::u32 cs_id;
        rstd::u32 cr_id;
        switch (fkind) {
        case wavsen::video::FrameKind::VulkanShared:
            cs_id = vulkan_frame->info().colorspace;
            cr_id = vulkan_frame->info().color_range;
            break;
        case wavsen::video::FrameKind::VaapiDrm:
            cs_id = vaapi_frame->view().colorspace;
            cr_id = vaapi_frame->view().color_range;
            break;
        case wavsen::video::FrameKind::Sw:
            cs_id = frame.colorspace;
            cr_id = frame.color_range;
            break;
        }
        const auto color_matrix = wavsen::video::make_color_matrix(
            static_cast<wavsen::video::ColorSpace>(cs_id.to_primitive()),
            static_cast<wavsen::video::ColorRange>(cr_id.to_primitive()));
        rstd::Result<int, wavsen::video::Error> cv_res = rstd::Ok(-1);
        switch (fkind) {
        case wavsen::video::FrameKind::VulkanShared: {
            auto converted = yuv->submit_av_vk_frame(
                rstd::move(*conversion_reservation), rstd::move(*vulkan_frame), color_matrix);
            if (converted.is_err()) {
                cv_res = rstd::Err(rstd::move(converted).unwrap_err());
            } else {
                cv_res = rstd::Ok(rstd::move(converted).unwrap().sync_fd);
            }
            break;
        }
        case wavsen::video::FrameKind::VaapiDrm: {
            auto mapped = rstd::move(*vaapi_frame).into_drm();
            if (mapped.is_err()) {
                cv_res = rstd::Err(rstd::move(mapped).unwrap_err());
                break;
            }
            auto converted = yuv->submit_drm_prime(
                rstd::move(*conversion_reservation), rstd::move(mapped).unwrap(), color_matrix);
            if (converted.is_err()) {
                cv_res = rstd::Err(rstd::move(converted).unwrap_err());
                break;
            }
            auto submission = rstd::move(converted).unwrap();
            if (submission.is_none()) {
                ww_bridge_pool_abort_acquired_slot(host.pool, &acquired.identity);
                wavsen::video::Presenter::wait_until(presentation);
                continue;
            }
            cv_res = rstd::Ok(submission->sync_fd);
            break;
        }
        case wavsen::video::FrameKind::Sw:
            cv_res = yuv->convert_nv12(reinterpret_cast<VkImage>(s.vk_image),
                                       rstd::u32(s.width),
                                       rstd::u32(s.height),
                                       frame.data.data(),
                                       frame.data.len(),
                                       color_matrix);
            break;
        }
        if (cv_res.is_err()) {
            ww_bridge_pool_abort_acquired_slot(host.pool, &acquired.identity);
            rstd_error("waywallen-video-renderer: yuv conversion failed: {}",
                       std::move(cv_res).unwrap_err().message.as_str());
            signal_shutdown(host);
            break;
        }
        sync_fd = std::move(cv_res).unwrap();
        wavsen::video::Presenter::wait_until(presentation);
        std::lock_guard<std::mutex>  send_lk(host.send_mu);
        ww_pool_slot_submit_result_t submitted {};
        int                          submit_rc = ww_bridge_pool_submit_acquired_slot(
            host.pool, host.sock, &acquired.identity, sync_fd, &submitted);
        if (submit_rc != 0 || submitted.status != WW_POOL_SLOT_SUBMIT_SUBMITTED) {
            rstd_error("waywallen-video-renderer: submit slot failed (rc={}, status={}, error={})",
                       submit_rc,
                       static_cast<int>(submitted.status),
                       submitted.error_code);
            signal_shutdown(host);
            break;
        }
        submitted_since_negotiate = true;
        complete_frame_request(host, frame_request);
    }

    if (reader.joinable()) {
        ::shutdown(host.sock, SHUT_RD);
        reader.join();
    }
    if (auto drained = yuv->drain_submissions(rstd::u64(1'000'000'000)); drained.is_err()) {
        rstd_warn("waywallen-video-renderer: final conversion drain failed: {}",
                  rstd::move(drained).unwrap_err().message.as_str());
        (void)vkDeviceWaitIdle(producer->device());
        (void)yuv->reclaim_submissions();
    }
    if (auto invalidated = yuv->invalidate_targets(); invalidated.is_err()) {
        rstd_warn("waywallen-video-renderer: final target invalidation failed: {}",
                  rstd::move(invalidated).unwrap_err().message.as_str());
    }
    if (host.pool) ww_bridge_pool_destroy(host.pool);
    ww_bridge_close(host.sock);
    return 0;
}

} // namespace waywallen::video
