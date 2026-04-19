cmake_minimum_required(VERSION 3.16)
project(waywallen-plugins
    VERSION 0.1.0
    LANGUAGES C CXX
    DESCRIPTION "waywallen renderer subprocess plugins (mpv, image, ...)"
)

# Toggle which plugins are built. Each defaults ON; turn an individual plugin
# OFF if its native deps (libmpv, ffmpeg, Vulkan, ...) aren't available.
option(WAYWALLEN_BUILD_MPV_PLUGIN   "Build the libmpv video plugin" ON)
option(WAYWALLEN_BUILD_IMAGE_PLUGIN "Build the ffmpeg+Vulkan image plugin" ON)

# Resolve waywallen-bridge once so every plugin in this tree shares the same
# target instead of rebuilding it per-subdir. Plugins that also stand alone
# guard their own bridge block with `if(NOT TARGET waywallen::bridge)`.
if(NOT TARGET waywallen::bridge)
    add_subdirectory(../bridge waywallen-bridge-build)
endif()

if(WAYWALLEN_BUILD_MPV_PLUGIN)
    add_subdirectory(mpv)
endif()

if(WAYWALLEN_BUILD_IMAGE_PLUGIN)
    add_subdirectory(image)
endif()
