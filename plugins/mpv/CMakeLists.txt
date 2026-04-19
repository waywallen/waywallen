cmake_minimum_required(VERSION 3.16)
project(waywallen-mpv
    VERSION 0.1.0
    LANGUAGES C CXX
    DESCRIPTION "libmpv-based video renderer subprocess for waywallen"
)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

# ---------------------------------------------------------------------------
# Dependencies
# ---------------------------------------------------------------------------

find_package(PkgConfig REQUIRED)
pkg_check_modules(MPV REQUIRED IMPORTED_TARGET mpv>=0.33)
pkg_check_modules(EGL REQUIRED IMPORTED_TARGET egl)
pkg_check_modules(GLES REQUIRED IMPORTED_TARGET glesv2)
pkg_check_modules(GBM REQUIRED IMPORTED_TARGET gbm>=21.0)
find_package(Threads REQUIRED)

# waywallen-bridge resolution order:
#   1. Target already present (parent plugins/CMakeLists pre-declared it)
#   2. Installed package (distro build)
#   3. In-tree checkout at ../../bridge (new layout: waywallen/bridge)
if(NOT TARGET waywallen::bridge)
    find_package(waywallen-bridge QUIET)
    if(NOT waywallen-bridge_FOUND)
        set(_ww_bridge_tree "${CMAKE_CURRENT_SOURCE_DIR}/../../bridge")
        if(EXISTS "${_ww_bridge_tree}/CMakeLists.txt")
            message(STATUS "waywallen-mpv: using in-tree bridge at ${_ww_bridge_tree}")
            add_subdirectory("${_ww_bridge_tree}" waywallen-bridge-build EXCLUDE_FROM_ALL)
        else()
            message(FATAL_ERROR
                "waywallen-bridge not found (no package, no tree at ${_ww_bridge_tree})")
        endif()
    endif()
endif()

# ---------------------------------------------------------------------------
# Renderer binary
# ---------------------------------------------------------------------------

add_executable(waywallen-mpv-renderer src/main.cpp)

target_compile_features(waywallen-mpv-renderer PRIVATE cxx_std_20)
target_compile_options(waywallen-mpv-renderer PRIVATE
    -Wall -Wextra -Wpedantic
    # Vulkan's VkFoo{STYPE} aggregate init legitimately leaves pNext
    # default-zeroed; the warning is pure noise here.
    -Wno-missing-field-initializers
)

target_link_libraries(waywallen-mpv-renderer
    PRIVATE
        waywallen::bridge
        PkgConfig::MPV
        PkgConfig::EGL
        PkgConfig::GLES
        PkgConfig::GBM
        Threads::Threads
)

# ---------------------------------------------------------------------------
# Install: matches the layout the waywallen daemon scans via --plugin.
#   <prefix>/bin/waywallen-mpv-renderer
#   <prefix>/share/waywallen/renderers/waywallen-mpv.toml
# ---------------------------------------------------------------------------

include(GNUInstallDirs)

install(TARGETS waywallen-mpv-renderer
    RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR}
)

install(DIRECTORY ${CMAKE_CURRENT_SOURCE_DIR}/renderers/
    DESTINATION ${CMAKE_INSTALL_DATADIR}/waywallen/renderers
    FILES_MATCHING PATTERN "*.toml"
)
