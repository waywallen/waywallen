export module waywallen.image.definition;
import rstd;

using namespace rstd::prelude;

export namespace ww_image
{
struct ImageCandidate {
    String         path;
    rstd::uint32_t width = 0, height = 0;
};
struct ImageFrame {
    Vec<ImageCandidate> light, dark;
    double              duration   = 0;
    bool                transition = false;
};
struct DefinitionAccess {
    void* context                    = nullptr;
    void (*observe)(void*, ref<str>) = nullptr;
};
struct ImageDefinition {
    Vec<ImageFrame> frames;
    Vec<String>     dependencies;
    bool            crossfade  = true;
    bool            timed      = false;
    double          start_time = 0;

    static auto           open(ref<str> path, String& error, Vec<String>* dependencies = nullptr,
                               DefinitionAccess access = {}) -> Option<ImageDefinition>;
    const ImageCandidate* representative() const;
    const ImageCandidate* preferred(const Vec<ImageCandidate>& values, rstd::uint32_t width,
                                    rstd::uint32_t height, bool preview = false) const;
    const ImageFrame& frame_at(double unix_seconds, double& mix, double* remaining = nullptr) const;
    auto              preview() const -> Vec<String>;
};
} // namespace ww_image
