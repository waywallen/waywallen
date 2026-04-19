cmake_minimum_required(VERSION 3.16)
project(waywallen-image
    VERSION 0.1.0
    LANGUAGES C CXX
    DESCRIPTION "FFmpeg + Vulkan image renderer subprocess for waywallen"
)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

# ---------------------------------------------------------------------------
# Dependencies
# ---------------------------------------------------------------------------

find_package(PkgConfig REQUIRED)
pkg_check_modules(AVFORMAT REQUIRED IMPORTED_TARGET libavformat>=58)
pkg_check_modules(AVCODEC  REQUIRED IMPORTED_TARGET libavcodec>=58)
pkg_check_modules(AVUTIL   REQUIRED IMPORTED_TARGET libavutil>=56)
pkg_check_modules(SWSCALE  REQUIRED IMPORTED_TARGET libswscale>=5)
find_package(Vulkan REQUIRED)
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
            message(STATUS "waywallen-image: using in-tree bridge at ${_ww_bridge_tree}")
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

add_executable(waywallen-image-renderer
    src/main.cpp
    src/av_image.cpp
    src/vk_producer.cpp
)

target_compile_features(waywallen-image-renderer PRIVATE cxx_std_20)
target_compile_options(waywallen-image-renderer PRIVATE
    -Wall -Wextra -Wpedantic
    -Wno-missing-field-initializers
)

target_link_libraries(waywallen-image-renderer
    PRIVATE
        waywallen::bridge
        PkgConfig::AVFORMAT
        PkgConfig::AVCODEC
        PkgConfig::AVUTIL
        PkgConfig::SWSCALE
        Vulkan::Vulkan
        Threads::Threads
)

# ---------------------------------------------------------------------------
# Install layout matches the daemon's --plugin scan convention:
#   <prefix>/bin/waywallen-image-renderer
#   <prefix>/share/waywallen/renderers/waywallen-image.toml
# ---------------------------------------------------------------------------

include(GNUInstallDirs)

install(TARGETS waywallen-image-renderer
    RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR}
)

install(DIRECTORY ${CMAKE_CURRENT_SOURCE_DIR}/renderers/
    DESTINATION ${CMAKE_INSTALL_DATADIR}/waywallen/renderers
    FILES_MATCHING PATTERN "*.toml"
)

install(DIRECTORY ${CMAKE_CURRENT_SOURCE_DIR}/sources/
    DESTINATION ${CMAKE_INSTALL_DATADIR}/waywallen/sources
    FILES_MATCHING PATTERN "*.lua"
)
