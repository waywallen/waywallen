#pragma once

#include <cstdint>
#include <string>
#include <vector>

namespace ww_image {

// Tightly-packed RGBA8 (R,G,B,A in memory order).
struct RgbaBuf {
    std::vector<uint8_t> data;
    uint32_t             width { 0 };
    uint32_t             height { 0 };
    uint32_t             stride { 0 }; // bytes per row; == width * 4 (no padding)
};

struct DecodeError {
    std::string message;
};

// Decode `path` (any container/codec FFmpeg understands) and scale its first
// frame to exactly `target_w` x `target_h` RGBA8. Scaling uses SWS_BICUBIC.
// Returns the scaled frame on success; populates `err->message` and returns
// empty buffer on failure.
RgbaBuf decode_to_rgba(const std::string& path,
                       uint32_t           target_w,
                       uint32_t           target_h,
                       DecodeError*       err);

} // namespace ww_image
