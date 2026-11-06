#pragma once

#include <cmath>

namespace waywallen::video
{
struct FrameRateLimit {
    double last_pts { -1.0 };

    void reset() { last_pts = -1.0; }

    bool accept(double pts, unsigned fps, double playback_rate) {
        if (! std::isfinite(pts) || pts < 0.0) return true;
        if (last_pts >= 0.0 && pts >= last_pts && pts - last_pts + 1e-9 < playback_rate / fps)
            return false;
        last_pts = pts;
        return true;
    }
};
} // namespace waywallen::video
