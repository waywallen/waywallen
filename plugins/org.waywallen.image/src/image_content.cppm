export module waywallen.image.content;
import rstd;
export import waywallen.image.av;

using namespace rstd::prelude;

export namespace ww_image
{

// KDE package resources and ordinary files share the renderer's decode interface.
class ImageContent {
public:
    static auto open(ref<str> path, DecodeError* error) -> Option<ImageContent>;
    ImageContent(ImageContent&&) noexcept;
    auto operator=(ImageContent&&) noexcept -> ImageContent&;
    ~ImageContent();
    rstd::uint32_t width() const;
    rstd::uint32_t height() const;
    bool           dynamic() const;
    bool           set_mode(ref<str> mode);
    bool           update(rstd::time::SystemTime now);
    RgbaBuf        decode(rstd::uint32_t width, rstd::uint32_t height, DecodeError* error);
    static double  simulated_darkness(int seconds, bool crossfade);

private:
    struct Impl;
    explicit ImageContent(Box<Impl>);
    Box<Impl> impl_;
};
} // namespace ww_image
