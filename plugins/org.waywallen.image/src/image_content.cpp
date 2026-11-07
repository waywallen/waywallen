module;
#include <rstd/macro.hpp>

module waywallen.image.content;
import waywallen.image.definition;

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
struct ImageContent::Impl {
    Option<ImageSession>    file;
    Option<ImageDefinition> definition;
    const ImageFrame*       frame = nullptr;
    uint32_t                width = 0, height = 0;
    double                  darkness    = 0;
    uint32_t                interval_ms = 30000;
    String                  mode        = String::make("day-night"_str);
};
ImageContent::ImageContent(Box<Impl> impl): impl_(rstd::move(impl)) {}
ImageContent::ImageContent(ImageContent&&) noexcept                    = default;
auto ImageContent::operator=(ImageContent&&) noexcept -> ImageContent& = default;
ImageContent::~ImageContent()                                          = default;

auto ImageContent::open(ref<str> path, DecodeError* error) -> Option<ImageContent> {
    auto impl      = Box<Impl>::make();
    auto input     = PathBuf::from(path);
    auto extension = input.as_path().extension();
    auto ext       = extension ? extension->to_string_lossy() : String();
    ext.as_mut_str().make_ascii_lowercase();
    const bool definition = ext == "json"_str || ext == "xml"_str;
    if (definition) {
        String reason;
        impl->definition = ImageDefinition::open(path, reason);
        if (! impl->definition) {
            if (error) error->message = rstd::format("{}: {}", path, reason);
            return None();
        }
        const auto&          values    = impl->definition->frames[usize()].light;
        auto*                preferred = impl->definition->preferred(values, 1920, 1080);
        Option<ImageSession> reference;
        if (preferred) reference = ImageSession::open(preferred->path, error);
        if (! reference) {
            for (const auto& value : values) {
                reference = ImageSession::open(value.path, error);
                if (reference) break;
            }
        }
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
    return impl_->definition && impl_->mode == "day-night"_str &&
           (impl_->definition->timed || ! impl_->definition->frames[usize()].dark.is_empty());
}
uint32_t ImageContent::update_interval_ms() const { return impl_->interval_ms; }

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
    if (! impl_->definition) return false;
    auto         timestamp = now.as_unix_time();
    double       next = 0, remaining = 30;
    const double seconds =
        timestamp.seconds.to_primitive() + timestamp.nanoseconds.to_primitive() / 1000000000.0;
    const auto&  frame   = impl_->definition->frame_at(seconds, next, &remaining);
    const double maximum = impl_->definition->timed && frame.transition ? 0.1 : 30;
    const double delay   = remaining < maximum ? remaining : maximum;
    impl_->interval_ms   = delay < 0.001 ? 1 : static_cast<uint32_t>(delay * 1000);
    if (! impl_->definition->timed) {
        auto utc =
            rstd::time::OffsetDateTime::from_unix_time(timestamp.seconds, timestamp.nanoseconds);
        if (utc.is_err()) return false;
        auto offset = rstd::time::UtcOffset::local_offset_at(*utc);
        if (offset.is_err()) return false;
        auto local = utc->checked_to_offset(*offset);
        if (! local) return false;
        auto time = local->time();
        next =
            simulated_darkness(time.hour().to_primitive() * 3600 +
                                   time.minute().to_primitive() * 60 + time.second().to_primitive(),
                               impl_->definition->crossfade);
    }
    if (impl_->mode == "light"_str) next = 0;
    if (impl_->mode == "dark"_str) next = 1;
    if (impl_->frame == &frame && impl_->darkness == next) return false;
    impl_->frame    = &frame;
    impl_->darkness = next;
    return true;
}
RgbaBuf ImageContent::decode(uint32_t width, uint32_t height, DecodeError* error) {
    if (impl_->file) return impl_->file->decode(width, height, error);
    if (! impl_->frame) return {};
    const auto decode_set = [&](const Vec<ImageCandidate>& values) {
        const auto* preferred = impl_->definition->preferred(values, width, height);
        if (! preferred) return RgbaBuf {};
        const auto decode = [&](const ImageCandidate& candidate) {
            auto session = ImageSession::open(candidate.path, error);
            return session ? session->decode(width, height, error, true) : RgbaBuf {};
        };
        auto image = decode(*preferred);
        if (! image.data.is_empty()) return image;
        for (const auto& candidate : values) {
            if (&candidate == preferred) continue;
            image = decode(candidate);
            if (! image.data.is_empty()) return image;
        }
        return RgbaBuf {};
    };
    const auto& light_set = impl_->frame->light;
    const auto& dark_set  = impl_->frame->dark;
    RgbaBuf     light, dark;
    if (impl_->darkness < 1 || dark_set.is_empty()) light = decode_set(light_set);
    if (impl_->darkness > 0 || light_set.is_empty() || light.data.is_empty())
        dark = decode_set(dark_set);
    if (dark.data.is_empty() && light.data.is_empty()) light = decode_set(light_set);
    if (light.data.is_empty()) return dark;
    if (dark.data.is_empty()) return light;
    if (dark.data.len() != light.data.len()) {
        if (error)
            error->message = String::make("wallpaper variants decoded to different extents"_str);
        return {};
    }
    const unsigned weight =
        static_cast<unsigned>(f64(impl_->darkness * 256).round().to_primitive());
    auto*       pixels = reinterpret_cast<uint8_t*>(light.data.data());
    const auto* other  = reinterpret_cast<const uint8_t*>(dark.data.data());
    for (rstd::size_t i = 0; i < light.data.len().to_primitive(); ++i)
        pixels[i] =
            static_cast<uint8_t>((pixels[i] * (256 - weight) + other[i] * weight + 128) / 256);
    return light;
}
} // namespace ww_image
