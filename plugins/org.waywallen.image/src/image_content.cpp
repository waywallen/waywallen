module;
#include <rstd/macro.hpp>

module waywallen.image.content;
import rstd.json;
import rstd.log;

using namespace rstd::prelude;
using namespace rstd::literals;
using rstd::uint32_t;
using rstd::uint64_t;
using rstd::uint8_t;
using rstd::path::Path;
using rstd::path::PathBuf;
using rstd::time::SystemTime;

namespace ww_image
{
namespace
{
struct Candidate {
    String   path;
    uint32_t width, height;
};

Vec<Candidate> images(ref<Path> directory) {
    Vec<Candidate> result;
    auto           entries = rstd::fs::read_dir(directory);
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
        auto filename   = entry.file_name().as_os_str().to_string_lossy();
        auto suffix     = filename.as_str().split_once("."_str);
        auto base       = suffix ? suffix->get<0>() : filename.as_str();
        auto dimensions = base.split_once("x"_str);
        if (! dimensions) continue;
        auto width  = rstd::from_str<i32>(dimensions->get<0>());
        auto height = rstd::from_str<i32>(dimensions->get<1>());
        if (width.is_err() || height.is_err() || *width <= i32() || *height <= i32()) continue;
        auto text = path.as_path().to_str();
        if (! text) continue;
        DecodeError error;
        auto        session = ImageSession::open(*text, &error);
        if (session && session->width() && session->height())
            result.push(Candidate { String::make(*text),
                                    static_cast<uint32_t>(width->to_primitive()),
                                    static_cast<uint32_t>(height->to_primitive()) });
        else
            rstd_warn("image package: skipping {}: {}", *text, error.message);
    }
    rstd::slice_::sort_by(result.as_mut_slice().as_mut_ref(), [](const auto& a, const auto& b) {
        return a.path <=> b.path;
    });
    return result;
}

const Candidate& best(const Vec<Candidate>& values, uint32_t width, uint32_t height) {
    const Candidate* result = &values[usize()];
    double           score  = f64::INFINITY_.to_primitive();
    for (const auto& value : values) {
        const double aspect =
            f64(double(value.width) / value.height - double(width) / height).abs().to_primitive();
        const double delta           = double(value.width) - width;
        const double candidate_score = aspect * 25000 + (delta >= 0 ? delta : -delta * 2);
        if (candidate_score < score) {
            result = &value;
            score  = candidate_score;
        }
    }
    return *result;
}

bool crossfade(const PathBuf& root) {
    const auto path     = root.join(ref<Path>("metadata.json"_str));
    auto       metadata = rstd::fs::metadata(path);
    if (metadata.is_err()) return true;
    if (metadata->len() > u64(1024 * 1024)) {
        rstd_warn("image package: metadata too large: {}", path.as_path());
        return true;
    }
    auto bytes = rstd::fs::read(path);
    if (bytes.is_err()) return true;
    auto parsed = rstd::json::from_slice(bytes->as_slice());
    if (parsed.is_err()) {
        rstd_warn("image package: invalid metadata: {}", path.as_path());
        return true;
    }
    auto value = parsed.unwrap();
    auto field = value.get("X-KDE-CrossFade"_str);
    if (field && (**field).is_boolean()) return *(**field).as_bool();
    return true;
}
} // namespace

struct ImageContent::Impl {
    Option<ImageSession> file;
    Vec<Candidate>       light, dark;
    uint32_t             width {}, height {};
    bool                 crossfade = true;
    double               darkness  = 0;
    String               mode      = String::make("day-night"_str);
};

ImageContent::ImageContent(Box<Impl> impl): impl_(rstd::move(impl)) {}
ImageContent::ImageContent(ImageContent&&) noexcept                    = default;
auto ImageContent::operator=(ImageContent&&) noexcept -> ImageContent& = default;
ImageContent::~ImageContent()                                          = default;

auto ImageContent::open(ref<str> path, DecodeError* error) -> Option<ImageContent> {
    auto impl     = Box<Impl>::make();
    auto root     = PathBuf::from(path);
    auto metadata = rstd::fs::metadata(root);
    if (metadata.is_ok() && metadata->is_dir()) {
        auto filename = root.as_path().file_name();
        if (filename && filename->to_str() == Some("contents"_str)) root.pop();
        impl->light = images(root.join(ref<Path>("contents/images"_str)));
        impl->dark  = images(root.join(ref<Path>("contents/images_dark"_str)));
        if (impl->light.is_empty()) {
            if (error)
                error->message =
                    String::make("image package has no valid light images named WIDTHxHEIGHT"_str);
            return None();
        }
        impl->crossfade          = crossfade(root);
        const auto&      values  = impl->light;
        const Candidate* largest = &values[usize()];
        for (const auto& value : values)
            if (uint64_t(value.width) * value.height > uint64_t(largest->width) * largest->height)
                largest = &value;
        auto reference = ImageSession::open(largest->path, error);
        if (! reference) return None();
        impl->width  = reference->width();
        impl->height = reference->height();
    } else {
        impl->file = ImageSession::open(path, error);
        if (! impl->file) return None();
        impl->width  = impl->file->width();
        impl->height = impl->file->height();
    }
    auto result = ImageContent(rstd::move(impl));
    result.update(SystemTime::now());
    return Some(rstd::move(result));
}

uint32_t ImageContent::width() const { return impl_->width; }
uint32_t ImageContent::height() const { return impl_->height; }
bool     ImageContent::dynamic() const {
    return impl_->mode == "day-night"_str && ! impl_->light.is_empty() && ! impl_->dark.is_empty();
}

bool ImageContent::set_mode(ref<str> mode) {
    if (mode != "day-night"_str && mode != "light"_str && mode != "dark"_str) return false;
    impl_->mode = String::make(mode);
    return update(SystemTime::now());
}

double ImageContent::simulated_darkness(int seconds, bool fade) {
    // Local simulated dawn is 06:00–06:30; dusk is 18:00–18:30.
    if (seconds < 6 * 3600) return 1;
    if (seconds < 6 * 3600 + 1800) return fade ? 1.0 - double(seconds - 6 * 3600) / 1800 : 0;
    if (seconds < 18 * 3600) return 0;
    if (seconds < 18 * 3600 + 1800) return fade ? double(seconds - 18 * 3600) / 1800 : 1;
    return 1;
}

bool ImageContent::update(SystemTime now) {
    if (impl_->light.is_empty() || impl_->dark.is_empty()) return false;
    auto timestamp = now.as_unix_time();
    auto utc = rstd::time::OffsetDateTime::from_unix_time(timestamp.seconds, timestamp.nanoseconds);
    if (utc.is_err()) return false;
    auto offset = rstd::time::UtcOffset::local_offset_at(*utc);
    if (offset.is_err()) return false;
    auto local = utc->checked_to_offset(*offset);
    if (! local) return false;
    auto         time = local->time();
    const double next = impl_->mode == "light"_str ? 0
                        : impl_->mode == "dark"_str
                            ? 1
                            : simulated_darkness(time.hour().to_primitive() * 3600 +
                                                     time.minute().to_primitive() * 60 +
                                                     time.second().to_primitive(),
                                                 impl_->crossfade);
    if (next == impl_->darkness) return false;
    impl_->darkness = next;
    return true;
}

RgbaBuf ImageContent::decode(uint32_t width, uint32_t height, DecodeError* error) {
    if (impl_->file) return impl_->file->decode(width, height, error);
    const auto decode_set = [&](const Vec<Candidate>& values) {
        if (values.is_empty()) return RgbaBuf {};
        const auto& preferred = best(values, width, height);
        const auto  decode    = [&](const Candidate& candidate) {
            auto session = ImageSession::open(candidate.path, error);
            return session ? session->decode(width, height, error, true) : RgbaBuf {};
        };
        auto image = decode(preferred);
        if (! image.data.is_empty()) return image;
        for (const auto& candidate : values) {
            if (&candidate == &preferred) continue;
            image = decode(candidate);
            if (! image.data.is_empty()) return image;
        }
        return RgbaBuf {};
    };
    RgbaBuf light, dark;
    if (impl_->darkness < 1 || impl_->dark.is_empty()) light = decode_set(impl_->light);
    if (impl_->darkness > 0 || impl_->light.is_empty() || light.data.is_empty())
        dark = decode_set(impl_->dark);
    if (dark.data.is_empty() && light.data.is_empty()) light = decode_set(impl_->light);
    if (light.data.is_empty()) return dark;
    if (dark.data.is_empty()) return light;
    if (dark.data.len() != light.data.len()) {
        if (error)
            error->message =
                String::make("image package variants decoded to different extents"_str);
        return {};
    }
    const unsigned weight =
        static_cast<unsigned>(f64(impl_->darkness * 256).round().to_primitive());
    auto*       light_pixels = reinterpret_cast<uint8_t*>(light.data.data());
    const auto* dark_pixels  = reinterpret_cast<const uint8_t*>(dark.data.data());
    for (rstd::size_t i = 0; i < light.data.len().to_primitive(); ++i)
        light_pixels[i] = static_cast<uint8_t>(
            (light_pixels[i] * (256 - weight) + dark_pixels[i] * weight + 128) / 256);
    return light;
}
} // namespace ww_image
