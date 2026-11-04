module;

#include <rstd/macro.hpp>
#include <errno.h>
#include <signal.h>
#include <sys/prctl.h>
#include <unistd.h>

module waywallen.gltf.entry;

import rstd;
import rstd.argparse;
import rstd.log;
import vecgltf.presentation;
import waywallen.bridge;
import waywallen.plugin.run_loop;

using namespace rstd::prelude;
using namespace rstd::literals;
using rstd::ffi::CStr;
using rstd::ffi::CString;
using rstd::sync::Arc;
using RunLoop   = waywallen::plugin::RunLoop;
using LoopStep  = waywallen::plugin::LoopStep;
using ExitCause = waywallen::plugin::ExitCause;
using Instant   = rstd::time::Instant;
using Duration  = rstd::time::Duration;
namespace gltf  = vecgltf;
namespace vk    = vrento::vulkan;
namespace tex   = vrento::texture;

namespace
{
struct Options {
    String                         path, ipc, render_node, capture, asset_root;
    Option<gltf::PresentationSpec> presentation;
    unsigned                       fps { 30 }, custom_extent { 1080 }, frames { 3 };
    int                            resolution { -2 }, animation {};
    float                          exposure { 1 };
    bool                           loop { true }, selftest {}, validation {}, snapshot {};
    VkExtent2D                     snapshot_extent { 128, 128 };
};

auto text(const char* value) -> ref<str> {
    return value ? CStr::from_ptr(value).to_str().unwrap_or(""_str) : ""_str;
}
auto terminated(ref<str> value) -> CString {
    return CString::make(Vec<u8>::from(value.as_bytes())).unwrap();
}
void report(const gltf::Error& error) {
    rstd_error(
        "gltf: {} [{}] {}: {}", error.source, unsigned(error.kind), error.location, error.message);
}
template<class T>
auto number(ref<str> value) -> Option<T> {
    auto parsed = rstd::from_str<T>(value);
    return parsed.is_ok() ? Some(parsed.unwrap()) : None<T>();
}

void settings(Options& options, const ww_kv_list_t& values) {
    for (unsigned i = 0; i < values.count; ++i) {
        auto key = text(values.data[i].key), value = text(values.data[i].value);
        if (key == "render_node"_str)
            options.render_node = String::make(value);
        else if (key == "fps"_str) {
            auto v = number<u32>(value);
            if (v && *v >= u32(1) && *v <= u32(240)) options.fps = v->to_primitive();
        } else if (key == "resolution"_str) {
            auto v = number<i32>(value);
            if (v) options.resolution = ww_resolution_sanitize(v->to_primitive());
        } else if (key == "custom_extent"_str) {
            auto v = number<u32>(value);
            if (v && *v >= u32(1) && *v <= u32(8192)) options.custom_extent = v->to_primitive();
        } else if (key == "animation"_str) {
            auto v = number<i32>(value);
            if (v && *v >= i32(-1)) options.animation = v->to_primitive();
        } else if (key == "exposure"_str) {
            auto v = number<f32>(value);
            if (v && v->is_finite() && *v >= f32(0.1) && *v <= f32(16))
                options.exposure = v->to_primitive();
        } else if (key == "loop"_str) {
            if (value == "true"_str) options.loop = true;
            if (value == "false"_str) options.loop = false;
        }
    }
}

auto parse(int argc, char** argv, int& status) -> Option<Options> {
    using namespace rstd::argparse;
    auto command = Command::make("waywallen-gltf-renderer"_str);
    command.about("Render glTF and GLB wallpapers"_str);
    auto path =
        command.add_arg(Arg<String>::value("path"_str, string_parser()).long_name("path"_str));
    auto ipc = command.add_arg(Arg<String>::value("ipc"_str, string_parser()).long_name("ipc"_str));
    auto node = command.add_arg(
        Arg<String>::value("render-node"_str, string_parser()).long_name("render-node"_str));
    auto frames =
        command.add_arg(Arg<String>::value("frames"_str, string_parser()).long_name("frames"_str));
    auto capture = command.add_arg(
        Arg<String>::value("capture"_str, string_parser()).long_name("capture"_str));
    auto asset_root = command.add_arg(
        Arg<String>::value("asset-root"_str, string_parser()).long_name("asset-root"_str));
    auto profile = command.add_arg(
        Arg<String>::value("render-profile"_str, string_parser()).long_name("render-profile"_str));
    auto width =
        command.add_arg(Arg<String>::value("width"_str, string_parser()).long_name("width"_str));
    auto height =
        command.add_arg(Arg<String>::value("height"_str, string_parser()).long_name("height"_str));
    command.add_arg(Arg<bool>::flag("selftest"_str).long_name("selftest"_str));
    command.add_arg(Arg<bool>::flag("snapshot"_str).long_name("snapshot"_str));
    command.add_arg(Arg<bool>::flag("validation"_str).long_name("validation"_str));
    auto built = rstd::move(command).build();
    if (built.is_err()) {
        status = 1;
        return None();
    }
    auto                     parser = built.unwrap();
    Vec<rstd::ffi::OsString> args;
    for (int i = 0; i < argc; ++i)
        args.push(rstd::ffi::OsString::from(
            ref<rstd::ffi::OsStr>::from_encoded_bytes_unchecked(text(argv[i]).as_bytes())));
    auto parsed = parser.parse_from(rstd::move(args));
    if (parsed.is_err()) {
        auto output = parser.render_error(parsed.unwrap_err());
        rstd::io::eprint("{}", output.text());
        status = output.exit_code().to_primitive();
        return None();
    }
    auto outcome = parsed.unwrap();
    if (outcome.is_Display()) {
        rstd::io::println("{}", outcome.as_Display().request.text());
        status = outcome.as_Display().request.exit_code().to_primitive();
        return None();
    }
    auto    matches = rstd::move(outcome).as_Parsed().value;
    Options options;
    auto    copy = [&](const auto& key, String& target) {
        auto value = matches.get_one(key).unwrap();
        if (value) target = (**value).clone();
    };
    copy(path, options.path);
    copy(ipc, options.ipc);
    copy(node, options.render_node);
    copy(capture, options.capture);
    copy(asset_root, options.asset_root);
    String presentation;
    copy(profile, presentation);
    if (matches.contains("render-profile"_str)) {
        auto parsed =
            gltf::ParsePresentation(presentation.as_str().as_bytes(), "--render-profile"_str);
        if (parsed.is_err()) {
            report(parsed.unwrap_err());
            status = 2;
            return None();
        }
        options.presentation = Some(parsed.unwrap());
    }
    String count;
    copy(frames, count);
    if (! count.is_empty()) {
        auto value = number<u32>(count.as_str());
        if (! value || *value < u32(1) || *value > u32(1000)) {
            status = 2;
            return None();
        }
        options.frames = value->to_primitive();
    }
    options.selftest   = matches.contains("selftest"_str);
    options.validation = matches.contains("validation"_str);
    options.snapshot   = matches.contains("snapshot"_str);
    String w, h;
    copy(width, w);
    copy(height, h);
    if (! w.is_empty() || ! h.is_empty()) {
        auto x = number<u32>(w.as_str()), y = number<u32>(h.as_str());
        if (! x || ! y || *x < u32(1) || *x > u32(7680) || *y < u32(1) || *y > u32(4320)) {
            status = 2;
            return None();
        }
        options.snapshot_extent = { x->to_primitive(), y->to_primitive() };
    }
    if (! options.selftest && (options.snapshot || ! w.is_empty() || ! h.is_empty())) {
        rstd_error("gltf: --snapshot, --width and --height require --selftest");
        status = 2;
        return None();
    }
    if (options.path.is_empty() || (! options.selftest && options.ipc.is_empty())) {
        rstd_error("--path MODEL and either --ipc SOCKET or --selftest are required");
        status = 2;
        return None();
    }
    return Some(rstd::move(options));
}

VkExtent2D initial_extent(const Options& options) {
    VkExtent2D extent { 1920, 1080 };
    unsigned   cap = options.resolution == WW_RESOLUTION_CUSTOM
                         ? options.custom_extent
                         : ww_resolution_short_edge(options.resolution);
    ww_resolution_apply_short_edge(
        cap, WW_RESOLUTION_CAP_ALLOW_UPSCALE, &extent.width, &extent.height);
    if (extent.width > 7680 || extent.height > 4320) {
        if (uint64_t(extent.width) * 4320 > uint64_t(extent.height) * 7680) {
            extent.height = unsigned(uint64_t(extent.height) * 7680 / extent.width);
            extent.width  = 7680;
        } else {
            extent.width  = unsigned(uint64_t(extent.width) * 4320 / extent.height);
            extent.height = 4320;
        }
    }
    if (options.selftest) extent = options.snapshot_extent;
    return extent;
}

struct Gpu {
    vk::DeviceSession           session;
    vk::TextureContext          queue;
    gltf::Renderer              renderer;
    gltf::DisplayRenderer       display;
    gltf::SceneInstance         instance;
    gltf::PreparedAsset         prepared;
    gltf::BoundPresentation     presentation;
    gltf::OrbitCamera           camera;
    gltf::CameraTransition      camera_transition;
    Option<gltf::CameraFrame>   camera_target;
    gltf::Vec3                  camera_up { 0, 1, 0 };
    Option<tex::TextureViewRef> target;
    Option<tex::TextureViewRef> hdr, coverage;
    tex::SubmissionToken        last;
    VkExtent2D                  extent {};
    Gpu(vk::DeviceSession device, vk::TextureContext context, gltf::Renderer graphics,
        gltf::DisplayRenderer output, gltf::SceneInstance scene, gltf::PreparedAsset model,
        gltf::BoundPresentation profile)
        : session(rstd::move(device)),
          queue(rstd::move(context)),
          renderer(rstd::move(graphics)),
          display(rstd::move(output)),
          instance(rstd::move(scene)),
          prepared(rstd::move(model)),
          presentation(rstd::move(profile)) {
        camera.fit(instance.bounds());
    }
    bool resize(VkExtent2D size) {
        if (target && size.width == extent.width && size.height == extent.height) return true;
        auto resolved = gltf::ResolvePresentation(
            presentation,
            instance,
            { .camera = Some(camera.frame(float(size.width) / float(size.height))) });
        if (resolved.is_err()) {
            report(resolved.unwrap_err());
            return false;
        }
        auto texture = [&](vrento::TextureFormat format) -> Option<tex::TextureViewRef> {
            tex::TextureDesc desc;
            desc.extent = { size.width, size.height };
            desc.format = format;
            desc.usage =
                tex::Usage::ColorAttachment | tex::Usage::Sampled | tex::Usage::TransferSource;
            auto image = queue.CreateTexture(desc);
            if (image.is_err()) return None();
            auto view = queue.CreateView(image.unwrap(), {});
            return view.is_ok() ? Some(view.unwrap()) : None<tex::TextureViewRef>();
        };
        auto output   = texture(vrento::TextureFormat::RGBA8);
        auto radiance = texture(vrento::TextureFormat::RGBA16F);
        if (! output || ! radiance) return false;
        Option<tex::TextureViewRef> mask;
        const auto&                 effects = resolved->display;
        if ((effects.bloom && effects.bloom->intensity > 0) || effects.sharpen > 0) {
            mask = texture(vrento::TextureFormat::RGBA16F);
            if (! mask) return false;
        }
        auto resized = renderer.resize(size);
        if (resized.is_err()) {
            report(resized.unwrap_err());
            return false;
        }
        gltf::RenderTarget scene { *radiance };
        if (mask) scene.coverage = Some(tex::TextureViewRef(*mask));
        auto scene_ready   = renderer.prepare_scene_target(scene, resolved->scene);
        auto display_ready = display.prepare_target(
            { *output, vk::AttachmentLoad::Clear, gltf::DisplayEncoding::Srgb }, effects);
        if (scene_ready.is_err() || display_ready.is_err()) {
            if (scene_ready.is_err()) report(scene_ready.unwrap_err());
            if (display_ready.is_err()) report(display_ready.unwrap_err());
            if (target) (void)renderer.resize(extent);
            return false;
        }
        target   = rstd::move(output);
        hdr      = rstd::move(radiance);
        coverage = rstd::move(mask);
        extent   = size;
        return true;
    }
    void configure(const Options& options) {
        unsigned selected =
            options.animation < 0 ? gltf::invalid_index : unsigned(options.animation);
        auto count = gltf::count(instance.asset().animations);
        if (! count && options.animation == 0) selected = gltf::invalid_index;
        if (selected != gltf::invalid_index && selected >= count) {
            rstd_warn("gltf: animation {} is out of range; keeping current selection",
                      options.animation);
            selected = instance.animation();
        }
        if (instance.animation() != selected) {
            instance.set_animation(selected);
            if (! presentation.spec().camera) camera.fit(instance.bounds());
        }
        instance.set_looping(options.loop);
    }
    bool animating(const Options& options) const {
        auto animation = instance.animation();
        return camera_target.is_some() || camera_transition.playing() ||
               (animation != gltf::invalid_index &&
                (options.loop ||
                 instance.time() < gltf::at(instance.asset().animations, animation).end));
    }
    void cancel_camera_transition() {
        camera_target = None();
        camera_transition.stop();
    }
    bool update_camera(float delta) {
        float aspect = float(extent.width) / float(extent.height);
        if (camera_target) {
            auto target = camera_target.take().unwrap();
            auto from = camera.frame(aspect).look_at(), to = target.look_at();
            gltf::CameraTransitionOptions options;
            options.up = camera_up;
            if (! from || ! to ||
                ! camera_transition.start(*from, *to, target.projection, options)) {
                rstd_warn("gltf: camera transition path is invalid; using the preset directly");
                return camera.restore(target, aspect);
            }
        }
        if (camera_transition.playing()) {
            (void)camera_transition.update(delta);
            auto frame = rstd::as<gltf::CameraSource>(camera_transition).Snapshot(aspect);
            if (! frame || ! camera.restore(*frame, aspect)) {
                rstd_error("gltf: camera transition frame is invalid");
                return false;
            }
        }
        return true;
    }
    auto record(float delta, const Options& options) -> Option<tex::Submission> {
        instance.update(delta);
        if (! update_camera(delta)) return None();
        auto resolved = gltf::ResolvePresentation(
            presentation,
            instance,
            { .exposure_multiplier = options.exposure,
              .camera = Some(camera.frame(float(extent.width) / float(extent.height))) });
        if (resolved.is_err()) {
            report(resolved.unwrap_err());
            return None();
        }
        auto values = resolved.unwrap();
        auto view   = values.camera->view(float(extent.width) / float(extent.height));
        if (! view) return None();
        auto begun = queue.Begin();
        if (begun.is_err()) return None();
        auto               work = begun.unwrap();
        gltf::RenderTarget scene { *hdr };
        if (coverage) scene.coverage = Some(tex::TextureViewRef(*coverage));
        auto recorded =
            renderer.record_scene(work, rstd::move(scene), prepared, instance, *view, values.scene);
        if (recorded.is_err()) {
            report(recorded.unwrap_err());
            return None();
        }
        gltf::DisplayInput input { *hdr };
        if (coverage) input.coverage = Some(tex::TextureViewRef(*coverage));
        auto displayed =
            display.record(work,
                           rstd::move(input),
                           { *target, vk::AttachmentLoad::Clear, gltf::DisplayEncoding::Srgb },
                           values.display);
        if (displayed.is_err()) {
            report(displayed.unwrap_err());
            return None();
        }
        return Some(rstd::move(work));
    }
};

auto create_gpu(Options& options) -> Option<Box<Gpu>> {
    auto path      = rstd::path::PathBuf::from(options.path.as_str());
    auto canonical = rstd::fs::canonicalize(path.as_path());
    if (canonical.is_err()) {
        rstd_error("gltf: model path: {}", canonical.unwrap_err());
        return None();
    }
    auto canonical_path = canonical.unwrap();
    auto source         = canonical_path.as_path().to_str();
    if (! source) {
        rstd_error("gltf: model path is not UTF-8");
        return None();
    }
    auto              root = rstd::path::PathBuf::from(options.asset_root.as_str());
    gltf::LoadOptions load_options;
    if (! options.asset_root.is_empty()) load_options.resource_root = Some(root.as_path());
    auto loaded = gltf::LoadAsset(source.unwrap(), load_options);
    if (loaded.is_err()) {
        report(loaded.unwrap_err());
        return None();
    }
    auto asset = loaded.unwrap();
    for (const auto& diagnostic : asset->diagnostics)
        rstd_warn("gltf: {}: {}", diagnostic.location, diagnostic.message);
    gltf::PresentationResources resources;
    auto                        parent = canonical_path.as_path().parent();
    if (parent) resources.base_directory = rstd::path::PathBuf::from(*parent);
    resources.root =
        Some(options.asset_root.is_empty() ? resources.base_directory.clone() : rstd::move(root));
    auto bound =
        gltf::BindPresentation(options.presentation.take().unwrap_or(gltf::PresentationSpec {}),
                               asset.clone(),
                               rstd::move(resources));
    if (bound.is_err()) {
        report(bound.unwrap_err());
        return None();
    }
    auto                presentation = bound.unwrap();
    gltf::SceneInstance scene(asset.clone());
    auto                initial =
        gltf::ResolvePresentation(presentation, scene, { .exposure_multiplier = options.exposure });
    if (initial.is_err()) {
        report(initial.unwrap_err());
        return None();
    }
    vk::DeviceSessionOptions device;
    device.extent     = initial_extent(options);
    device.validation = options.validation;
    Vec<vk::Extension> extensions;
    if (! options.selftest) {
        ref<str> names[] {
            "VK_KHR_external_memory_fd"_str,    "VK_EXT_external_memory_dma_buf"_str,
            "VK_KHR_image_format_list"_str,     "VK_EXT_image_drm_format_modifier"_str,
            "VK_KHR_external_semaphore_fd"_str, "VK_EXT_queue_family_foreign"_str
        };
        for (auto name : names) extensions.push({ true, name });
        extensions.push({ false, "VK_EXT_physical_device_drm"_str });
    }
    device.device_extensions = extensions.as_slice();
    rstd::uint8_t uuid[16] {};
    if (! options.render_node.is_empty()) {
        vrento::vulkan::Instance probe;
        auto                     created = vk::Instance::TryCreate(probe, {}, {});
        if (created.is_err()) {
            rstd_error("render-node Vulkan instance failed");
            return None();
        }
        ww_bridge_vk_dt_t dispatch {};
        auto&             native = probe.inst();
        ww_bridge_vk_dt_load(&dispatch, native.Dispatch().resolver, *native);
        auto path = terminated(options.render_node.as_str());
        int  rc   = ww_bridge_vk_resolve_render_node(&dispatch, *native, path.as_ptr(), uuid);
        if (rc) {
            rstd_error("render-node resolution failed: {}", rc);
            return None();
        }
        device.physical_device_uuid = slice<rstd::uint8_t>::from_raw_parts(uuid, usize(16));
    }
    auto created = vk::DeviceSession::create(device);
    if (created.is_err()) {
        rstd_error("Vulkan session: {}", created.unwrap_err());
        return None();
    }
    auto        session    = created.unwrap();
    const auto& limits     = session.device().limits();
    auto        max_extent = limits.maxImageDimension2D;
    if (limits.maxFramebufferWidth < max_extent) max_extent = limits.maxFramebufferWidth;
    if (limits.maxFramebufferHeight < max_extent) max_extent = limits.maxFramebufferHeight;
    auto view = initial->camera->view(float(device.extent.width) / float(device.extent.height));
    if (! view) return None();
    auto scene_valid = gltf::ValidateSceneOptions(initial->scene, *view, max_extent);
    if (scene_valid.is_err() || device.extent.width > max_extent ||
        device.extent.height > max_extent) {
        if (scene_valid.is_err())
            report(scene_valid.unwrap_err());
        else
            rstd_error("gltf: render target exceeds device dimensions");
        return None();
    }
    auto context = vk::TextureContext::create(session);
    if (context.is_err()) {
        rstd_error("Vulkan texture context failed");
        return None();
    }
    auto graphics = gltf::Renderer::create(session, device.extent);
    if (graphics.is_err()) {
        report(graphics.unwrap_err());
        return None();
    }
    auto renderer = graphics.unwrap();
    if (auto environment = presentation.take_environment()) {
        auto uploaded = renderer.set_environment(environment.take().unwrap());
        if (uploaded.is_err()) {
            report(uploaded.unwrap_err());
            return None();
        }
    }
    auto output = gltf::DisplayRenderer::create(session);
    if (output.is_err()) {
        report(output.unwrap_err());
        return None();
    }
    auto prepared = renderer.prepare(asset.clone(), presentation.prepare_options());
    if (prepared.is_err()) {
        report(prepared.unwrap_err());
        return None();
    }
    auto gpu = Box<Gpu>::make(session,
                              context.unwrap(),
                              rstd::move(renderer),
                              output.unwrap(),
                              rstd::move(scene),
                              prepared.unwrap(),
                              rstd::move(presentation));
    gpu->configure(options);
    gpu->camera.fit(gpu->instance.bounds());
    if (gpu->presentation.spec().camera) {
        auto resolved = gltf::ResolvePresentation(gpu->presentation, gpu->instance);
        if (resolved.is_err()) {
            report(resolved.unwrap_err());
            return None();
        }
        auto frame         = resolved->camera.unwrap();
        gpu->camera_target = Some(frame);
        gpu->camera_up     = gltf::compose({}, frame.orientation, { 1, 1, 1 }).vector({ 0, 1, 0 });
    }
    if (! gpu->resize(device.extent)) return None();
    return Some(rstd::move(gpu));
}

struct PointerInput {
    static constexpr uint64_t revision = 1;
    static constexpr unsigned left = 0x110, right = 0x111, middle = 0x112;
    struct Drag {
        unsigned button;
        float    x, y;
    };
    bool         enabled {};
    Option<Drag> drag;

    static bool valid(float x, float y) {
        return f32(x).is_finite() && f32(y).is_finite() && x >= 0 && y >= 0;
    }
    void reset() { drag = None(); }
    void applied(const waywallen_event_subscription_result_t& result) {
        if (result.revision != revision) return;
        reset();
        enabled = false;
        if (result.status == WAYWALLEN_EVENT_SUBSCRIPTION_STATUS_APPLIED)
            for (unsigned i = 0; i < result.kinds.count; ++i)
                if (text(result.kinds.data[i]) == "pointer"_str) enabled = true;
        if (! enabled) rstd_warn("gltf: pointer subscription unavailable: {}", text(result.reason));
    }
    void button(const waywallen_pointer_button_t& event) {
        if (! enabled) return;
        if (event.state == WAYWALLEN_POINTER_BUTTON_STATE_RELEASED) {
            if (drag && drag->button == event.button) reset();
        } else if (event.state == WAYWALLEN_POINTER_BUTTON_STATE_PRESSED &&
                   (event.button == left || event.button == right || event.button == middle)) {
            reset();
            if (valid(event.x, event.y)) drag = Some(Drag { event.button, event.x, event.y });
        }
    }
    bool motion(gltf::OrbitCamera& camera, VkExtent2D extent,
                const waywallen_pointer_motion_t& event) {
        if (! enabled) return false;
        if (! valid(event.x, event.y)) {
            reset();
            return false;
        }
        if (! drag) return false;
        float x = event.x - drag->x, y = event.y - drag->y;
        drag->x = event.x;
        drag->y = event.y;
        if (! f32(x).is_finite() || ! f32(y).is_finite() || (x == 0 && y == 0)) return false;
        if (drag->button == left)
            camera.rotate(-x * 0.008f, -y * 0.008f);
        else if (extent.height)
            camera.pan(x / float(extent.height), y / float(extent.height));
        else
            return false;
        return true;
    }
    bool axis(gltf::OrbitCamera& camera, const waywallen_pointer_axis_t& event) {
        if (! enabled || ! valid(event.x, event.y) || ! f32(event.delta_y).is_finite() ||
            event.delta_y == 0)
            return false;
        camera.zoom(event.delta_y);
        return true;
    }
};

struct Host {
    Option<RunLoop>                        events;
    Option<vk::DeviceSession>              device_owner;
    int                                    socket { -1 };
    ww_pool_t*                             pool {};
    bool                                   ready {}, paused {}, frame_requested { true };
    bool                                   camera_changed {};
    uint64_t                               target_revision {};
    PointerInput                           pointer;
    waywallen_render_target_capabilities_t capabilities { .revision         = 1,
                                                          .reference_width  = 1920,
                                                          .reference_height = 1080,
                                                          .mode             = 2,
                                                          .min_width        = 1,
                                                          .min_height       = 1,
                                                          .max_width        = 7680,
                                                          .max_height       = 4320,
                                                          .width_alignment  = 1,
                                                          .height_alignment = 1 };
    void                                   close_pool() {
        if (device_owner) (void)device_owner->wait_idle();
        if (pool) ww_bridge_pool_destroy(pool);
        pool = nullptr;
    }
    ~Host() {
        if (events) events->close();
        close_pool();
        if (socket >= 0) ww_bridge_close(socket);
    }
};

bool open_pool(Host& host, Gpu& gpu, const Options& options) {
    host.device_owner                     = Some(vk::DeviceSession(gpu.session));
    auto&                        session  = gpu.session;
    const auto&                  device   = session.device();
    const auto&                  instance = session.instance_owner().inst();
    VkPhysicalDeviceIDProperties ids { VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_ID_PROPERTIES };
    VkPhysicalDeviceProperties2 properties { VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2, &ids };
    device.gpu().GetProperties2KHR(properties);
    ww_pool_vulkan_init_t init {};
    init.instance           = session.instance();
    init.physical_device    = *device.gpu();
    init.device             = *device.handle();
    init.queue              = *device.graphics_queue().handle;
    init.queue_family_index = device.graphics_queue().family_index;
    init.get_instance_proc_addr =
        reinterpret_cast<void* (*)(void*, const char*)>(instance.Dispatch().resolver);
    init.device_uuid          = ids.deviceUUID;
    init.driver_uuid          = ids.driverUUID;
    init.drm_render_fd        = -1;
    init.image_usage_flags    = VK_IMAGE_USAGE_TRANSFER_DST_BIT;
    init.format_feature_flags = VK_FORMAT_FEATURE_BLIT_DST_BIT;
    ww_bridge_vk_dt_t dispatch {};
    ww_bridge_vk_dt_load(&dispatch, instance.Dispatch().resolver, *instance);
    (void)ww_bridge_vk_query_render_node(
        &dispatch, *device.gpu(), &init.drm_render_major, &init.drm_render_minor);
    ww_bridge_vk_log_gpu_info("waywallen-gltf-renderer", &dispatch, *device.gpu());
    int rc = ww_bridge_pool_create(WW_POOL_BACKEND_VULKAN, &init, &host.pool);
    if (! rc)
        rc = ww_bridge_pool_advertise_caps(host.pool,
                                           host.socket,
                                           gpu.extent.width,
                                           gpu.extent.height,
                                           WW_MEM_HINT_DEVICE_LOCAL | WW_MEM_HINT_HOST_VISIBLE);
    if (! rc && options.resolution == WW_RESOLUTION_AUTO)
        rc = ww_bridge_send_render_target_capabilities(host.socket, &host.capabilities);
    if (! rc) {
        char                                 pointer[] = "pointer";
        char*                                kinds[]   = { pointer };
        const waywallen_event_subscription_t subscription { PointerInput::revision, { 1, kinds } };
        rc = ww_bridge_set_event_subscriptions(host.socket, &subscription);
    }
    if (rc) rstd_error("bridge pool: {}", rc);
    return rc == 0;
}

auto render_slot(Gpu& gpu, const ww_pool_slot_t& slot, bool reused, float delta,
                 const Options& options) -> Option<int> {
    if (! slot.vk_image || ! gpu.resize({ slot.width, slot.height })) return None();
    auto begun = gpu.record(delta, options);
    if (! begun) return None();
    auto work   = begun.take().unwrap();
    auto source = work.commands()->UseTexture(
        { *gpu.target, tex::Access::TransferSource, tex::Stages::Transfer });
    if (source.is_err()) return None();
    auto command = vk::native_recording(work, gpu.session);
    auto image   = vk::native_texture(gpu.target->texture());
    if (command.is_err() || image.is_err()) return None();
    const auto&          device      = gpu.session.device().handle();
    const auto&          dispatch    = device.Dispatch();
    VkImage              destination = reinterpret_cast<VkImage>(slot.vk_image);
    VkImageMemoryBarrier acquire { VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER };
    acquire.dstAccessMask       = VK_ACCESS_TRANSFER_WRITE_BIT;
    acquire.oldLayout           = reused ? VK_IMAGE_LAYOUT_GENERAL : VK_IMAGE_LAYOUT_UNDEFINED;
    acquire.newLayout           = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
    acquire.srcQueueFamilyIndex = reused ? VK_QUEUE_FAMILY_FOREIGN_EXT : VK_QUEUE_FAMILY_IGNORED;
    acquire.dstQueueFamilyIndex = reused ? gpu.queue.queue().family_index : VK_QUEUE_FAMILY_IGNORED;
    acquire.image               = destination;
    acquire.subresourceRange    = { VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1 };
    dispatch.vkCmdPipelineBarrier(*command,
                                  VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT,
                                  VK_PIPELINE_STAGE_TRANSFER_BIT,
                                  0,
                                  0,
                                  nullptr,
                                  0,
                                  nullptr,
                                  1,
                                  &acquire);
    VkImageBlit blit {};
    blit.srcSubresource = blit.dstSubresource = { VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1 };
    blit.srcOffsets[1] = blit.dstOffsets[1] = { int(slot.width), int(slot.height), 1 };
    dispatch.vkCmdBlitImage(*command,
                            *image,
                            VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
                            destination,
                            VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
                            1,
                            &blit,
                            VK_FILTER_NEAREST);
    VkImageMemoryBarrier release { VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER };
    release.srcAccessMask       = VK_ACCESS_TRANSFER_WRITE_BIT;
    release.oldLayout           = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
    release.newLayout           = VK_IMAGE_LAYOUT_GENERAL;
    release.srcQueueFamilyIndex = gpu.queue.queue().family_index;
    release.dstQueueFamilyIndex = VK_QUEUE_FAMILY_FOREIGN_EXT;
    release.image               = destination;
    release.subresourceRange    = acquire.subresourceRange;
    dispatch.vkCmdPipelineBarrier(*command,
                                  VK_PIPELINE_STAGE_TRANSFER_BIT,
                                  VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT,
                                  0,
                                  0,
                                  nullptr,
                                  0,
                                  nullptr,
                                  1,
                                  &release);
    VkExportSemaphoreCreateInfo export_info { VK_STRUCTURE_TYPE_EXPORT_SEMAPHORE_CREATE_INFO };
    export_info.handleTypes = VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT;
    VkSemaphoreCreateInfo semaphore_info { VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO, &export_info };
    vvk::Semaphore        semaphore;
    if (device.CreateSemaphore(semaphore_info, semaphore) != VK_SUCCESS) return None();
    auto handle   = *semaphore;
    auto owner    = Arc<dyn<rstd::any::Any>>::make(rstd::move(semaphore));
    auto attached = gpu.queue.AttachSignal(
        work, { *device, handle, vk::NativeSemaphoreKind::Binary, 0, owner.clone() });
    if (attached.is_err()) return None();
    auto submitted = gpu.queue.Submit(rstd::move(work));
    if (submitted.is_err()) return None();
    gpu.last = submitted.unwrap();
    VkSemaphoreGetFdInfoKHR fd_info { VK_STRUCTURE_TYPE_SEMAPHORE_GET_FD_INFO_KHR };
    fd_info.semaphore  = handle;
    fd_info.handleType = VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT;
    int fd             = -1;
    if (device.GetSemaphoreFdKHR(fd_info, &fd) != VK_SUCCESS) return None();
    return Some(fd);
}

bool publish(Host& host, Gpu& gpu, const ww_pool_slot_acquire_result_t& acquired, float delta,
             const Options& options) {
    auto fd = render_slot(
        gpu, acquired.slot, acquired.status == WW_POOL_SLOT_ACQUIRE_READY_RELEASED, delta, options);
    if (! fd) {
        (void)gpu.session.wait_idle();
        ww_bridge_pool_abort_acquired_slot(host.pool, &acquired.identity);
        return false;
    }
    ww_pool_slot_submit_result_t result {};
    int                          rc = ww_bridge_pool_submit_acquired_slot(
        host.pool, host.socket, &acquired.identity, fd.unwrap(), &result);
    if (rc || result.status != WW_POOL_SLOT_SUBMIT_SUBMITTED) {
        rstd_error("bridge submit failed: rc={} status={} error={}",
                   rc,
                   unsigned(result.status),
                   result.error_code);
        return false;
    }
    return true;
}

bool configure_target(Host& host, Gpu& gpu, const Options& options,
                      const waywallen_render_target_config_t& config) {
    waywallen_render_target_outcome_t result {};
    result.revision             = config.revision;
    result.status               = 1;
    String              message = "invalid or stale render target"_Str;
    ww_pool_directive_t directive {};
    int rc = ww_bridge_render_target_directive(&host.capabilities, &config, &directive);
    if (! rc && config.revision > host.target_revision &&
        options.resolution == WW_RESOLUTION_AUTO) {
        auto previous_camera     = gpu.camera;
        auto previous_transition = gpu.camera_transition;
        auto previous_target =
            gpu.camera_target ? Some(*gpu.camera_target) : None<gltf::CameraFrame>();
        auto previous_output =
            gpu.target ? Some(tex::TextureViewRef(*gpu.target)) : None<tex::TextureViewRef>();
        auto previous_hdr =
            gpu.hdr ? Some(tex::TextureViewRef(*gpu.hdr)) : None<tex::TextureViewRef>();
        auto previous_coverage =
            gpu.coverage ? Some(tex::TextureViewRef(*gpu.coverage)) : None<tex::TextureViewRef>();
        auto previous_extent = gpu.extent;
        host.target_revision = config.revision;
        result.status        = 2;
        auto drained         = gpu.last.valid() ? gpu.queue.wait(gpu.last) : gpu.queue.Poll();
        rc = drained.is_ok()
                 ? ww_bridge_pool_prepare_target(host.pool, &directive, config.width, config.height)
                 : -EIO;
        if (! rc) {
            ww_pool_slot_t slot {};
            rc      = ww_bridge_pool_candidate_slot(host.pool, 0, &slot);
            auto fd = rc ? None<int>() : render_slot(gpu, slot, false, 0, options);
            if (fd && gpu.queue.wait(gpu.last).is_ok()) {
                rc = ww_bridge_pool_commit_target(host.pool, host.socket, &result.pool_generation);
                if (! rc) {
                    ww_pool_slot_acquire_result_t acquired {};
                    rc = ww_bridge_pool_try_acquire_any_for_render(host.pool, &acquired);
                    if (! rc && acquired.status == WW_POOL_SLOT_ACQUIRE_READY_UNUSED &&
                        acquired.slot.index == 0) {
                        ww_pool_slot_submit_result_t submitted {};
                        rc = ww_bridge_pool_submit_acquired_slot(host.pool,
                                                                 host.socket,
                                                                 &acquired.identity,
                                                                 fd.take().unwrap(),
                                                                 &submitted);
                        if (! rc && submitted.status == WW_POOL_SLOT_SUBMIT_SUBMITTED) {
                            result.status        = 0;
                            result.width         = config.width;
                            result.height        = config.height;
                            host.ready           = true;
                            host.frame_requested = false;
                            host.camera_changed  = false;
                            host.pointer.reset();
                        }
                    }
                    if (result.status != 0) host.events->request_stop(ExitCause::Failure);
                }
            }
            if (fd) ::close(fd.unwrap());
            if (gpu.session.wait_idle() != VK_SUCCESS && ! gpu.session.lost())
                host.events->request_stop(ExitCause::Failure);
            ww_bridge_pool_abort_target(host.pool);
        }
        if (result.status != 0) {
            gpu.camera            = previous_camera;
            gpu.camera_transition = previous_transition;
            gpu.camera_target     = previous_target;
            gpu.target            = rstd::move(previous_output);
            gpu.hdr               = rstd::move(previous_hdr);
            gpu.coverage          = rstd::move(previous_coverage);
            gpu.extent            = previous_extent;
            if (gpu.renderer.resize(previous_extent).is_err())
                host.events->request_stop(ExitCause::Failure);
        }
        message =
            result.status == 0 ? ""_Str : "render target allocation or publication failed"_Str;
    }
    auto wire      = terminated(message.as_str());
    result.message = const_cast<char*>(wire.as_ptr());
    return ww_bridge_send_render_target_result(host.socket, &result) == 0 && host.events->running();
}

bool control(Host& host, Gpu& gpu, Options& options, ww_bridge_control_t& event) {
    int  rc {};
    bool ok = true;
    switch (event.op) {
    case WW_EVT_IN_PLAY:
        host.paused          = false;
        host.frame_requested = true;
        break;
    case WW_EVT_IN_PAUSE: host.paused = true; break;
    case WW_EVT_IN_REQUEST_FRAME: host.frame_requested = true; break;
    case WW_EVT_IN_EVENT_SUBSCRIPTIONS_APPLIED:
        host.pointer.applied(event.u.event_subscriptions_applied.result);
        break;
    case WW_EVT_IN_POINTER_BUTTON:
        if (host.ready) host.pointer.button(event.u.pointer_button.event);
        break;
    case WW_EVT_IN_POINTER_MOTION: {
        VkExtent2D extent {};
        if (host.ready &&
            ww_bridge_pool_get_extent(host.pool, &extent.width, &extent.height) == 0 &&
            host.pointer.motion(gpu.camera, extent, event.u.pointer_motion.event)) {
            gpu.cancel_camera_transition();
            host.camera_changed = true;
        }
        break;
    }
    case WW_EVT_IN_POINTER_AXIS:
        if (host.ready && host.pointer.axis(gpu.camera, event.u.pointer_axis.event)) {
            gpu.cancel_camera_transition();
            host.camera_changed = true;
        }
        break;
    case WW_EVT_IN_SET_LOG_LEVEL: ww_renderer_log_set_level(event.u.set_log_level.level); break;
    case WW_EVT_IN_SETTING_CHANGED:
        settings(options, event.u.setting_changed.settings);
        gpu.configure(options);
        host.frame_requested = true;
        break;
    case WW_EVT_IN_NEGOTIATE_BUFFERS:
        ok = (gpu.last.valid() ? gpu.queue.wait(gpu.last) : gpu.queue.Poll()).is_ok();
        if (ok) {
            rc = ww_bridge_pool_apply_directive(
                host.pool, host.socket, &event.u.negotiate_buffers.directive);
            ok                   = rc <= 0;
            host.ready           = rc == 0;
            host.frame_requested = true;
            host.pointer.reset();
        }
        break;
    case WW_EVT_IN_CONFIGURE_RENDER_TARGET:
        ok = configure_target(host, gpu, options, event.u.configure_render_target.config);
        break;
    default: break;
    }
    return ok;
}

bool readback(Gpu& gpu, const Options& options) {
    VkBufferCreateInfo info { VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO };
    info.size  = uint64_t(gpu.extent.width) * gpu.extent.height * 4;
    info.usage = VK_BUFFER_USAGE_TRANSFER_DST_BIT;
    auto allocated =
        gpu.session.device().memory_allocator().create_buffer(info, vvk::MemoryRequest::Readback());
    if (allocated.is_err()) return false;
    auto buffer = allocated.unwrap();
    auto begun  = gpu.queue.Begin();
    if (begun.is_err()) return false;
    auto work   = begun.unwrap();
    auto access = work.commands()->UseTexture(
        { *gpu.target, tex::Access::TransferSource, tex::Stages::Transfer });
    if (access.is_err()) return false;
    auto command = vk::native_recording(work, gpu.session);
    auto source  = vk::native_texture(gpu.target->texture());
    if (command.is_err() || source.is_err()) return false;
    VkBufferImageCopy copy {};
    copy.imageSubresource = { VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1 };
    copy.imageExtent      = { gpu.extent.width, gpu.extent.height, 1 };
    const auto& dispatch  = gpu.session.device().handle().Dispatch();
    dispatch.vkCmdCopyImageToBuffer(
        *command, *source, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, buffer.handle(), 1, &copy);
    VkBufferMemoryBarrier host { VK_STRUCTURE_TYPE_BUFFER_MEMORY_BARRIER };
    host.srcAccessMask       = VK_ACCESS_TRANSFER_WRITE_BIT;
    host.dstAccessMask       = VK_ACCESS_HOST_READ_BIT;
    host.srcQueueFamilyIndex = host.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
    host.buffer                                         = buffer.handle();
    host.size                                           = VK_WHOLE_SIZE;
    dispatch.vkCmdPipelineBarrier(*command,
                                  VK_PIPELINE_STAGE_TRANSFER_BIT,
                                  VK_PIPELINE_STAGE_HOST_BIT,
                                  0,
                                  0,
                                  nullptr,
                                  1,
                                  &host,
                                  0,
                                  nullptr);
    auto submitted = gpu.queue.Submit(rstd::move(work));
    if (submitted.is_err()) return false;
    gpu.last = submitted.unwrap();
    if (gpu.queue.wait(gpu.last).is_err()) return false;
    auto memory = buffer.allocation();
    auto mapped = memory.map();
    if (mapped.is_err() || memory.invalidate().is_err()) return false;
    auto     pixels = static_cast<const unsigned char*>(mapped->data());
    uint64_t hash   = 14695981039346656037ull, visible {};
    for (uint64_t i = 0; i < info.size; ++i) {
        hash ^= pixels[i];
        hash *= 1099511628211ull;
    }
    for (uint64_t i = 0; i < info.size; i += 4)
        if (pixels[i] || pixels[i + 1] || pixels[i + 2]) ++visible;
    rstd::io::println("gltf readback: hash={:016x} non_background_pixels={}", hash, visible);
    if (! options.capture.is_empty()) {
        auto header = rstd::format("P6\n{} {}\n255\n", gpu.extent.width, gpu.extent.height);
        auto bytes  = Vec<u8>::from(header.as_str().as_bytes());
        bytes.reserve(usize(gpu.extent.width) * usize(gpu.extent.height) * usize(3));
        for (uint64_t i = 0; i < info.size; i += 4)
            for (unsigned c = 0; c < 3; ++c) bytes.push(u8(pixels[i + c]));
        auto path    = rstd::path::PathBuf::from(options.capture.as_str());
        auto written = rstd::fs::write(path.as_path(), bytes.as_slice());
        if (written.is_err()) {
            rstd_error("gltf capture: {}", written.unwrap_err());
            return false;
        }
    }
    return true;
}

int selftest(Gpu& gpu, const Options& options) {
    if (options.snapshot && gpu.camera_target) {
        auto target = gpu.camera_target.take().unwrap();
        if (! gpu.camera.restore(target, float(gpu.extent.width) / float(gpu.extent.height)))
            return 1;
        gpu.camera_transition.stop();
    }
    for (unsigned i = 0; i < options.frames; ++i) {
        auto work = gpu.record(i && ! options.snapshot ? 1.0f / float(options.fps) : 0, options);
        if (! work) return 1;
        auto submitted = gpu.queue.Submit(work.take().unwrap());
        if (submitted.is_err()) return 1;
        gpu.last = submitted.unwrap();
        if (gpu.queue.wait(gpu.last).is_err()) return 1;
    }
    if (! readback(gpu, options)) return 1;
    auto statistics = gpu.renderer.statistics();
    rstd::io::println("gltf selftest: frames={} extent={}x{} animation={} validation_errors={}",
                      statistics.recorded_frames,
                      gpu.extent.width,
                      gpu.extent.height,
                      gpu.instance.animation(),
                      gpu.session.validation_errors());
    return gpu.session.validation_errors() ? 1 : 0;
}
} // namespace

int waywallen::gltf::run(int argc, char** argv) {
    ww_renderer_log_init();
    int  status {};
    auto parsed = parse(argc, argv, status);
    if (! parsed) return status;
    auto options = parsed.take().unwrap();
    if (options.selftest) {
        auto gpu = create_gpu(options);
        return gpu ? selftest(**gpu, options) : 1;
    }
    ::signal(SIGPIPE, SIG_IGN);
    const auto parent = ::getppid();
    if (::prctl(PR_SET_PDEATHSIG, SIGTERM) != 0 || ::getppid() != parent) return 1;
    Host host;
    auto socket = terminated(options.ipc.as_str());
    host.socket = ww_bridge_connect(socket.as_ptr());
    if (host.socket < 0) {
        rstd_error("bridge connect: {}", host.socket);
        return 1;
    }
    waywallen_renderer_init_t init {};
    int                       rc = ww_bridge_recv_init(host.socket, &init);
    if (rc) {
        char                       reason[] = "unsupported renderer init";
        waywallen_init_rejection_t rejection { init.protocol_version,
                                               WW_BRIDGE_SUPPORTED_PROTOCOL_VERSION,
                                               init.spawn_version,
                                               WW_BRIDGE_SUPPORTED_SPAWN_VERSION,
                                               reason };
        ww_bridge_send_init_nack(host.socket, &rejection);
        waywallen_renderer_init_free(&init);
        return 1;
    }
    settings(options, init.settings);
    waywallen_renderer_init_free(&init);
    auto events = RunLoop::start(host.socket);
    if (events.is_err()) {
        rstd_error("gltf: control reader failed: {}", events.unwrap_err());
        return 1;
    }
    host.events  = Some(events.unwrap());
    auto created = create_gpu(options);
    if (! created) return 1;
    auto gpu = created.take().unwrap();
    if (! open_pool(host, *gpu, options)) return 1;
    auto          previous = Instant::now();
    auto          due      = previous;
    Option<float> pending_delta;
    auto          exit = host.events->run(
        [&](ww_bridge_control_t& event) {
            bool was_paused    = host.paused;
            auto frames_before = gpu->renderer.statistics().recorded_frames;
            if (! control(host, *gpu, options, event)) {
                host.events->request_stop(ExitCause::Failure);
                return;
            }
            if (was_paused != host.paused) previous = Instant::now();
            if (gpu->renderer.statistics().recorded_frames != frames_before) {
                previous      = Instant::now();
                due           = previous + Duration::from_nanos(u64(1000000000 / options.fps));
                pending_delta = None();
            }
            if (host.frame_requested || host.paused) pending_delta = Some(0.0f);
        },
        [&]() -> LoopStep {
            if (gpu->queue.Poll().is_err()) {
                host.events->request_stop(ExitCause::Failure);
                return LoopStep::again();
            }
            auto now       = Instant::now();
            bool animation = ! host.paused && gpu->animating(options);
            bool wanted = host.ready && (host.frame_requested || host.camera_changed || animation);
            if (! wanted) return LoopStep::wait_until(now + Duration::from_secs(u64(1)));
            if (now < due) return LoopStep::wait_until(due);
            if (! pending_delta)
                pending_delta = Some(host.paused || host.frame_requested
                                         ? 0.0f
                                         : float((now - previous).as_secs_f64()));
            ww_pool_slot_acquire_result_t acquired {};
            rc = ww_bridge_pool_try_acquire_any_for_render(host.pool, &acquired);
            if (! rc && acquired.status == WW_POOL_SLOT_ACQUIRE_BUSY) {
                due = now + Duration::from_millis(u64(5));
                return LoopStep::wait_until(due);
            }
            if (rc || (acquired.status != WW_POOL_SLOT_ACQUIRE_READY_UNUSED &&
                       acquired.status != WW_POOL_SLOT_ACQUIRE_READY_RELEASED)) {
                host.events->request_stop(ExitCause::Failure, rc);
                return LoopStep::again();
            }
            float delta = pending_delta.take().unwrap();
            if (! publish(host, *gpu, acquired, delta, options)) {
                host.events->request_stop(ExitCause::Failure);
                return LoopStep::again();
            }
            previous             = now;
            due                  = now + Duration::from_nanos(u64(1000000000 / options.fps));
            host.frame_requested = false;
            host.camera_changed  = false;
            return LoopStep::wait_until(due);
        });
    if (exit.cause == ExitCause::Transport)
        rstd_error("gltf: control receive failed: {}", exit.code);
    status = exit.cause == ExitCause::Transport || exit.cause == ExitCause::Failure ? 1 : 0;
    host.events->close();
    (void)gpu->session.wait_idle();
    (void)gpu->queue.Poll();
    host.close_pool();
    if (options.validation) {
        auto errors = gpu->session.validation_errors();
        rstd_info("gltf: validation_errors={}", errors);
        if (errors) status = 1;
    }
    return status;
}
