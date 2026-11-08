module;
#include <libxml/parser.h>
#include <libxml/tree.h>
#include <time.h>

module waywallen.image.definition;
import rstd.json;

using namespace rstd::prelude;
using namespace rstd::literals;
using rstd::uint32_t;
using rstd::ffi::CStr;
using rstd::ffi::CString;
using rstd::path::Path;
using rstd::path::PathBuf;

namespace ww_image
{
namespace
{
auto text(const char* value) -> ref<str> {
    return value ? CStr::from_ptr(value).to_str().unwrap_or(""_str) : ""_str;
}
auto node_text(xmlNode* node) -> String {
    auto* value  = xmlNodeGetContent(node);
    auto  result = String::make(text(reinterpret_cast<const char*>(value)).trim_ascii());
    xmlFree(value);
    return result;
}
bool tag(xmlNode* node, const char* name) {
    return node->type == XML_ELEMENT_NODE &&
           xmlStrEqual(node->name, reinterpret_cast<const xmlChar*>(name));
}
auto child(xmlNode* node, const char* name) -> xmlNode* {
    for (auto* it = node->children; it; it = it->next)
        if (tag(it, name)) return it;
    return nullptr;
}
auto resolve(ref<str> owner, ref<str> value) -> String {
    if (auto file = value.strip_prefix("file://"_str)) value = *file;
    auto path = PathBuf::from(owner);
    path.pop();
    return path.join(ref<Path>(value)).as_path().to_string_lossy();
}
auto dimension(ref<str> value) -> uint32_t {
    auto number = rstd::from_str<u32>(value);
    return number.is_ok() && *number <= u32(2147483647) ? number->to_primitive() : 0;
}
void candidates(xmlNode* node, ref<str> owner, Vec<ImageCandidate>& result) {
    if (! node) return;
    bool sized = false;
    for (auto* it = node->children; it; it = it->next) {
        if (! tag(it, "size")) continue;
        auto* width  = xmlGetProp(it, reinterpret_cast<const xmlChar*>("width"));
        auto* height = xmlGetProp(it, reinterpret_cast<const xmlChar*>("height"));
        auto  value  = node_text(it);
        if (! value.is_empty())
            result.push(ImageCandidate { resolve(owner, value),
                                         dimension(text(reinterpret_cast<const char*>(width))),
                                         dimension(text(reinterpret_cast<const char*>(height))) });
        xmlFree(width);
        xmlFree(height);
        sized = true;
    }
    if (! sized) {
        auto value = node_text(node);
        if (! value.is_empty()) result.push(ImageCandidate { resolve(owner, value) });
    }
}
auto kde_images(ref<Path> directory) -> Vec<ImageCandidate> {
    Vec<ImageCandidate> result;
    auto                entries = rstd::fs::read_dir(directory);
    if (entries.is_err()) return result;
    while (auto next = entries->next()) {
        if (next->is_err()) continue;
        auto entry    = rstd::move(*next).unwrap();
        auto path     = entry.path();
        auto metadata = rstd::fs::metadata(path);
        if (metadata.is_err() || ! metadata->is_file()) continue;
        auto extension = path.as_path().extension();
        if (! extension) continue;
        auto ext = extension->to_string_lossy();
        ext.as_mut_str().make_ascii_lowercase();
        if (ext != "png"_str && ext != "jpg"_str && ext != "jpeg"_str && ext != "webp"_str &&
            ext != "bmp"_str && ext != "tiff"_str && ext != "tif"_str && ext != "avif"_str &&
            ext != "gif"_str)
            continue;
        auto name   = entry.file_name().as_os_str().to_string_lossy();
        auto suffix = name.as_str().split_once("."_str);
        auto base   = suffix ? suffix->get<0>() : name.as_str();
        auto size   = base.split_once("x"_str);
        if (! size) continue;
        auto width = dimension(size->get<0>()), height = dimension(size->get<1>());
        if (width && height)
            result.push(ImageCandidate { path.as_path().to_string_lossy(), width, height });
    }
    rstd::slice_::sort_by(result.as_mut_slice().as_mut_ref(), [](const auto& a, const auto& b) {
        return a.path <=> b.path;
    });
    return result;
}
bool read_definition(ref<str> path, ImageDefinition& output, String& error, Vec<String>& stack,
                     DefinitionAccess access) {
    if (access.observe) access.observe(access.context, path);
    const auto depth = stack.len().to_primitive();
    if (depth > 8) {
        error = String::make("wallpaper reference nesting exceeds limit"_str);
        return false;
    }
    for (const auto& seen : stack)
        if (seen == path) {
            error = String::make("cyclic wallpaper reference"_str);
            return false;
        }
    stack.push(String::make(path));
    struct StackGuard {
        Vec<String>& stack;
        ~StackGuard() { stack.pop(); }
    } guard { stack };
    output.dependencies.push(String::make(path));
    auto metadata = rstd::fs::metadata(ref<Path>(path));
    if (metadata.is_err() || ! metadata->is_file() || metadata->len() > u64(1024 * 1024)) {
        error = String::make("wallpaper definition is missing or too large"_str);
        return false;
    }
    auto bytes = rstd::fs::read(ref<Path>(path));
    if (bytes.is_err()) {
        error = String::make("cannot read wallpaper definition"_str);
        return false;
    }
    auto input_path = PathBuf::from(path);
    auto extension  = input_path.as_path().extension();
    auto ext        = extension ? extension->to_string_lossy() : String();
    ext.as_mut_str().make_ascii_lowercase();
    if (ext == "json"_str) {
        auto value  = rstd::json::from_slice(bytes->as_slice());
        auto plugin = value.is_ok() ? value->get("KPlugin"_str) : None();
        if (! plugin || ! (**plugin).is_object()) {
            error = String::make("unsupported wallpaper JSON"_str);
            return false;
        }
        auto root = PathBuf::from(path);
        root.pop();
        output.dependencies.push(
            root.join(ref<Path>("contents/images"_str)).as_path().to_string_lossy());
        output.dependencies.push(
            root.join(ref<Path>("contents/images_dark"_str)).as_path().to_string_lossy());
        if (access.observe) {
            access.observe(access.context,
                           output.dependencies[output.dependencies.len() - usize(2)]);
            access.observe(access.context,
                           output.dependencies[output.dependencies.len() - usize(1)]);
        }
        ImageFrame frame;
        frame.light = kde_images(root.join(ref<Path>("contents/images"_str)));
        frame.dark  = kde_images(root.join(ref<Path>("contents/images_dark"_str)));
        if (frame.light.is_empty()) {
            error = String::make("KDE wallpaper has no light images"_str);
            return false;
        }
        auto fade = value->get("X-KDE-CrossFade"_str);
        if (fade && (**fade).is_boolean()) output.crossfade = *(**fade).as_bool();
        output.frames.push(rstd::move(frame));
        return true;
    }
    auto filename = CString::make(Vec<u8>::from(path.as_bytes())).unwrap();
    struct Document {
        xmlDoc* value;
        ~Document() {
            if (value) xmlFreeDoc(value);
        }
    };
    Document document { xmlReadMemory(reinterpret_cast<const char*>(bytes->data()),
                                      static_cast<int>(bytes->len().to_primitive()),
                                      filename.as_ptr(),
                                      nullptr,
                                      XML_PARSE_NONET | XML_PARSE_NOERROR | XML_PARSE_NOWARNING) };
    auto*    root = document.value ? xmlDocGetRootElement(document.value) : nullptr;
    if (! root || (! tag(root, "wallpapers") && ! tag(root, "background"))) {
        error = String::make("unsupported or invalid wallpaper XML"_str);
        return false;
    }
    if (tag(root, "wallpapers")) {
        for (auto* item = root->children; item; item = item->next) {
            if (! tag(item, "wallpaper")) continue;
            auto*      deleted = xmlGetProp(item, reinterpret_cast<const xmlChar*>("deleted"));
            const bool skip    = text(reinterpret_cast<const char*>(deleted)) == "true"_str;
            xmlFree(deleted);
            if (skip) continue;
            ImageFrame frame;
            candidates(child(item, "filename"), path, frame.light);
            candidates(child(item, "filename-dark"), path, frame.dark);
            if (frame.light.is_empty()) continue;
            auto input_path = PathBuf::from(frame.light[usize()].path);
            auto ext        = input_path.as_path().extension();
            if (ext && ext->to_string_lossy() == "xml"_str) {
                if (! frame.dark.is_empty()) {
                    error = String::make(
                        "a timed light wallpaper with a separate dark source is unsupported"_str);
                    return false;
                }
                if (! read_definition(frame.light[usize()].path, output, error, stack, access))
                    return false;
            } else {
                output.frames.push(rstd::move(frame));
            }
        }
    } else {
        output.timed = true;
        if (auto* start = child(root, "starttime")) {
            struct tm  time {};
            const auto field = [&](const char* key) {
                auto* node = child(start, key);
                return node ? static_cast<int>(dimension(node_text(node))) : 0;
            };
            time.tm_year      = field("year") - 1900;
            time.tm_mon       = field("month") - 1;
            time.tm_mday      = field("day");
            time.tm_hour      = field("hour");
            time.tm_min       = field("minute");
            time.tm_sec       = field("second");
            time.tm_isdst     = -1;
            output.start_time = static_cast<double>(mktime(&time));
        }
        for (auto* item = root->children; item; item = item->next) {
            if (! tag(item, "static") && ! tag(item, "transition")) continue;
            ImageFrame frame;
            frame.transition = tag(item, "transition");
            if (auto* node = child(item, "duration")) {
                auto duration = rstd::from_str<f64>(node_text(node));
                if (duration.is_ok() && duration->is_finite() && *duration > f64())
                    frame.duration = duration->to_primitive();
            }
            candidates(child(item, frame.transition ? "from" : "file"), path, frame.light);
            if (frame.transition) candidates(child(item, "to"), path, frame.dark);
            if (! frame.light.is_empty() && frame.duration > 0)
                output.frames.push(rstd::move(frame));
        }
    }
    if (output.frames.is_empty()) {
        error = String::make("wallpaper XML has no supported images"_str);
        return false;
    }
    return true;
}
} // namespace
auto ImageDefinition::open(ref<str> path, String& error, Vec<String>* dependencies,
                           DefinitionAccess access) -> Option<ImageDefinition> {
    ImageDefinition result;
    Vec<String>     stack;
    bool            valid = read_definition(path, result, error, stack, access);
    for (const auto& frame : result.frames) {
        for (const auto& value : frame.light) result.dependencies.push(value.path.clone());
        for (const auto& value : frame.dark) result.dependencies.push(value.path.clone());
    }
    if (dependencies) *dependencies = result.dependencies.clone();
    if (! valid) return None();
    return Some(rstd::move(result));
}
const ImageCandidate* ImageDefinition::representative() const {
    return frames.is_empty() ? nullptr : preferred(frames[usize()].light, 16, 9, true);
}
const ImageCandidate* ImageDefinition::preferred(const Vec<ImageCandidate>& values, uint32_t width,
                                                 uint32_t height, bool preview) const {
    const ImageCandidate* result = nullptr;
    double score = f64::INFINITY_.to_primitive(), area = f64::INFINITY_.to_primitive();
    for (const auto& value : values) {
        const double aspect =
            value.height ? double(value.width) / value.height : double(width) / height;
        const double delta    = double(value.width) - width;
        const double distance = f64(aspect - double(width) / height).abs().to_primitive();
        const double next =
            preview ? distance : distance * 25000 + (delta >= 0 ? delta : -delta * 2);
        const double pixels = double(value.width) * value.height;
        if (! result || next < score || (preview && next == score && pixels < area)) {
            result = &value;
            score  = next;
            area   = pixels;
        }
    }
    return result;
}
const ImageFrame& ImageDefinition::frame_at(double seconds, double& mix, double* remaining) const {
    if (remaining) *remaining = 30;
    mix          = 0;
    double total = 0;
    for (const auto& frame : frames) total += frame.duration;
    if (! timed || total <= 0) return frames[usize()];
    double offset = seconds - start_time;
    offset -= f64(offset / total).floor().to_primitive() * total;
    for (const auto& frame : frames) {
        if (offset < frame.duration) {
            mix = frame.transition ? offset / frame.duration : 0;
            if (remaining) *remaining = frame.duration - offset;
            return frame;
        }
        offset -= frame.duration;
    }
    return frames[usize()];
}
auto ImageDefinition::preview() const -> Vec<String> {
    Vec<String> result;
    const auto& frame = frames[usize()];
    if (auto* light = preferred(frame.light, 16, 9, true)) result.push(light->path.clone());
    if (auto* dark = preferred(frame.dark, 16, 9, true)) result.push(dark->path.clone());
    if (result.len() == usize(1) && timed) {
        for (const auto& entry : frames) {
            auto* next = preferred(entry.light, 16, 9, true);
            if (next && next->path != result[usize()]) {
                result.push(next->path.clone());
                break;
            }
        }
    }
    return result;
}
} // namespace ww_image
