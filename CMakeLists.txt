cmake_minimum_required(VERSION 3.28)
project(waywallen
    VERSION 0.1.0
    LANGUAGES C CXX
    DESCRIPTION "waywallen: Qt/QML UI + renderer subprocess plugins"
)

if(POLICY CMP0077)
    cmake_policy(SET CMP0077 NEW)
endif()

option(WAYWALLEN_BUILD_UI      "Build the Qt/QML UI"                   ON)
option(WAYWALLEN_BUILD_PLUGINS "Build renderer plugins (mpv, image)"   ON)

if(WAYWALLEN_BUILD_PLUGINS)
    add_subdirectory(plugins)
endif()

if(WAYWALLEN_BUILD_UI)
    add_subdirectory(ui)
endif()

message(STATUS
    "waywallen components: ui=${WAYWALLEN_BUILD_UI} plugins=${WAYWALLEN_BUILD_PLUGINS}")
