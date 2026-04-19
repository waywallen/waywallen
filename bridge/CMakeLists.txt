cmake_minimum_required(VERSION 3.16)
project(waywallen-bridge
    VERSION 0.1.0
    LANGUAGES C
    DESCRIPTION "C library for renderer subprocesses to talk to the waywallen daemon"
)

option(BUILD_SHARED_LIBS "Build using shared libraries" ON)

# ---------------------------------------------------------------------------
# Library target
# ---------------------------------------------------------------------------

add_library(waywallen-bridge
    src/bridge.c
    src/ipc_v1.c
)
add_library(waywallen::bridge ALIAS waywallen-bridge)

target_include_directories(waywallen-bridge
    PUBLIC
        $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
        $<INSTALL_INTERFACE:include>
)

target_compile_features(waywallen-bridge PUBLIC c_std_99)
target_compile_options(waywallen-bridge PRIVATE
    -Wall -Wextra -Wpedantic
    -Wno-unused-parameter
    -Wno-unused-variable
    -Wno-unused-but-set-variable
)

set_target_properties(waywallen-bridge PROPERTIES
    VERSION ${PROJECT_VERSION}
    SOVERSION ${PROJECT_VERSION_MAJOR}
    EXPORT_NAME bridge
)

# Symbol visibility: export only the public ww_* API.
# Uses a linker version script because ipc_v1.{h,c} is auto-generated and
# hand-annotating its declarations with a visibility macro would be lost on
# regeneration. Restricted to ELF/GNU-like linkers (Linux is the only
# supported target today — see README).
if(BUILD_SHARED_LIBS AND CMAKE_SYSTEM_NAME STREQUAL "Linux")
    set(_ww_bridge_version_script
        "${CMAKE_CURRENT_SOURCE_DIR}/waywallen-bridge.map")
    target_link_options(waywallen-bridge PRIVATE
        "LINKER:--version-script=${_ww_bridge_version_script}"
    )
    # Ensure relinks pick up map edits.
    set_property(TARGET waywallen-bridge APPEND PROPERTY
        LINK_DEPENDS "${_ww_bridge_version_script}")
endif()

# ---------------------------------------------------------------------------
# Installation
# ---------------------------------------------------------------------------

include(GNUInstallDirs)
include(CMakePackageConfigHelpers)

install(TARGETS waywallen-bridge
    EXPORT waywallen-bridge-targets
    LIBRARY DESTINATION ${CMAKE_INSTALL_LIBDIR}
    ARCHIVE DESTINATION ${CMAKE_INSTALL_LIBDIR}
    RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR}
)

install(DIRECTORY include/
    DESTINATION ${CMAKE_INSTALL_INCLUDEDIR}
    FILES_MATCHING PATTERN "*.h"
)

install(EXPORT waywallen-bridge-targets
    FILE waywallen-bridge-targets.cmake
    NAMESPACE waywallen::
    DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/waywallen-bridge
)

configure_package_config_file(
    ${CMAKE_CURRENT_SOURCE_DIR}/cmake/waywallen-bridge-config.cmake.in
    ${CMAKE_CURRENT_BINARY_DIR}/waywallen-bridge-config.cmake
    INSTALL_DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/waywallen-bridge
)

write_basic_package_version_file(
    ${CMAKE_CURRENT_BINARY_DIR}/waywallen-bridge-config-version.cmake
    VERSION ${PROJECT_VERSION}
    COMPATIBILITY SameMajorVersion
)

install(FILES
    ${CMAKE_CURRENT_BINARY_DIR}/waywallen-bridge-config.cmake
    ${CMAKE_CURRENT_BINARY_DIR}/waywallen-bridge-config-version.cmake
    DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/waywallen-bridge
)

# ---------------------------------------------------------------------------
# Optional: build a smoke-test executable
# ---------------------------------------------------------------------------

option(WAYWALLEN_BRIDGE_BUILD_TESTS "Build waywallen-bridge tests" OFF)
if(WAYWALLEN_BRIDGE_BUILD_TESTS)
    enable_testing()
    add_subdirectory(tests)
endif()
