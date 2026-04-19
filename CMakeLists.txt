cmake_minimum_required(VERSION 3.28)
project(waywallen
    VERSION 0.1.0
    LANGUAGES C CXX
    DESCRIPTION "waywallen: Qt/QML UI + renderer subprocess plugins"
)

# Note: the Rust daemon (src/, Cargo.toml) is built via `cargo build`; this
# CMake tree only orchestrates the C/C++ components (UI + plugins) so both
# can be configured from a single `cmake -S waywallen -B build`.

# Let -D overrides from the command line beat child option() defaults,
# so a top-level `-DBUILD_SHARED_LIBS=ON` sticks across both subdirs.
if(POLICY CMP0077)
    cmake_policy(SET CMP0077 NEW)
endif()

option(WAYWALLEN_BUILD_UI      "Build the Qt/QML UI"                   ON)
option(WAYWALLEN_BUILD_PLUGINS "Build renderer plugins (mpv, image)"   ON)

# Build plugins first: the bridge shared lib is small and fast to produce,
# and the UI configure step is the slowest one — getting plugin build errors
# early saves time on iteration.
if(WAYWALLEN_BUILD_PLUGINS)
    add_subdirectory(plugins)
endif()

if(WAYWALLEN_BUILD_UI)
    add_subdirectory(ui)
endif()

message(STATUS
    "waywallen components: ui=${WAYWALLEN_BUILD_UI} plugins=${WAYWALLEN_BUILD_PLUGINS}")
