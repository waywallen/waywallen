export module waywallen.plugin.frame_rate_limit;

import rstd;

export namespace waywallen::plugin
{
struct FrameRateLimit {
    double   last_pts { -1.0 };
    double   next_pts { -1.0 };
    unsigned current_fps { 0 };
    double   current_rate { 0.0 };

    void reset() { last_pts = next_pts = -1.0; }

    bool ready(double pts, unsigned fps, double playback_rate) const {
        if (! rstd::f64(pts).is_finite() || pts < 0.0) return true;
        if (last_pts < 0.0 || pts < last_pts || fps != current_fps || playback_rate != current_rate)
            return true;
        return pts + 1e-9 >= next_pts;
    }

    void submitted(double pts, unsigned fps, double playback_rate) {
        if (! rstd::f64(pts).is_finite() || pts < 0.0 || fps == 0 ||
            ! rstd::f64(playback_rate).is_finite() || playback_rate <= 0.0)
            return;
        const double interval = playback_rate / fps;
        if (last_pts < 0.0 || pts < last_pts || fps != current_fps ||
            playback_rate != current_rate) {
            next_pts = pts + interval;
        } else {
            const double delta  = pts > next_pts ? pts - next_pts : 0.0;
            const double missed = rstd::f64(delta / interval).floor().to_primitive();
            next_pts += (missed + 1.0) * interval;
            if (next_pts <= pts + 1e-9) next_pts += interval;
        }
        last_pts     = pts;
        current_fps  = fps;
        current_rate = playback_rate;
    }
};
} // namespace waywallen::plugin
