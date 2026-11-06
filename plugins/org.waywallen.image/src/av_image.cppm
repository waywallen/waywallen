export module waywallen.image.av;
import rstd;

using namespace rstd::prelude;

export namespace ww_image
{

// Tightly-packed RGBA8 (R,G,B,A in memory order).
struct RgbaBuf {
    Vec<u8>        data;
    rstd::uint32_t width { 0 };
    rstd::uint32_t height { 0 };
    rstd::uint32_t stride { 0 }; // bytes per row; == width * 4 (no padding)
};

struct DecodeError {
    String message;
};

class ImageSession {
public:
    static auto open(ref<str> path, DecodeError* err) -> Option<ImageSession>;
    ImageSession(ImageSession&&) noexcept;
    auto operator=(ImageSession&&) noexcept -> ImageSession&;
    ~ImageSession();
    rstd::uint32_t width() const;
    rstd::uint32_t height() const;
    RgbaBuf        decode(rstd::uint32_t width, rstd::uint32_t height, DecodeError* err,
                          bool allow_upscale = false);

private:
    struct Impl;
    explicit ImageSession(Box<Impl>);
    Box<Impl> impl_;
};

// Numeric resolution presets only downscale; an empty buffer reports failure.
RgbaBuf decode_to_rgba(ref<str> path, rstd::int32_t resolution, DecodeError* err);

} // namespace ww_image
