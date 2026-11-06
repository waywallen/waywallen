module;

#include "../src/frame_rate_limit.hpp"
#include <cstdio>
#include <initializer_list>
#include <limits>

export module waywallen.video.tests;

using waywallen::video::FrameRateLimit;

#define CHECK(condition)                                                                         \
    do {                                                                                         \
        if (! (condition)) {                                                                     \
            std::fprintf(                                                                        \
                stderr, "frame rate limit check failed at line %d: %s\n", __LINE__, #condition); \
            return 1;                                                                            \
        }                                                                                        \
    } while (false)

int main() {
    for (unsigned fps : { 1u, 15u, 30u, 60u, 240u }) {
        FrameRateLimit limit;
        unsigned       count = 0;
        for (unsigned i = 0; i < 600; ++i) count += limit.accept(i / 60.0, fps, 1.0);
        CHECK(count == 10 * (fps < 60 ? fps : 60));
    }

    for (double rate : { 0.5, 2.0, 4.0 }) {
        FrameRateLimit limit;
        unsigned       count = 0;
        for (unsigned i = 0; i < 600; ++i) count += limit.accept(i / 60.0, 1, rate);
        CHECK(count == static_cast<unsigned>(std::ceil(10.0 / rate)));
    }

    FrameRateLimit limit;
    CHECK(limit.accept(0.0, 1, 1.0));
    CHECK(! limit.accept(0.5, 1, 1.0));
    CHECK(limit.accept(0.5, 60, 1.0));
    CHECK(! limit.accept(0.5, 60, 1.0));
    CHECK(limit.accept(0.0, 60, 1.0));
    limit.reset();
    CHECK(limit.accept(0.0, 1, 1.0));
    CHECK(limit.accept(-1.0, 1, 1.0));
    CHECK(limit.accept(std::numeric_limits<double>::quiet_NaN(), 1, 1.0));
    CHECK(! limit.accept(0.5, 1, 1.0));
    return 0;
}
