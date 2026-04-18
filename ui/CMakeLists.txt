cmake_minimum_required(VERSION 3.28)

set(APP_ID org.waywallen.waywallen)
set(APP_NAME "waywallen-ui")
set(APP_VERSION "0.1.0")
set(APP_SUMMARY "Wallpaper manager for Wayland")
set(APP_AUTHOR "hypengw")

project(waywallen-ui LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 23)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

option(BUILD_SHARED_LIBS "Build using shared libraries" OFF)

find_package(Qt6 REQUIRED COMPONENTS Quick DBus Protobuf ProtobufQuick ProtobufWellKnownTypes)
include(${CMAKE_SOURCE_DIR}/cmake/SplitMocFiles.cmake)

if(QT_KNOWN_POLICY_QTP0004)
  qt_policy(SET QTP0003 NEW)
  qt_policy(SET QTP0004 NEW)
endif()
set(QT_QML_GENERATE_QMLLS_INI ON)

set(QT_QML_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/qml_modules")
set(QML_IMPORT_PATH
    "${CMAKE_CURRENT_BINARY_DIR}/qml_modules"
    CACHE STRING "QML import paths for IDE and tooling" FORCE
)
list(APPEND QML_DIRS "${QT_QML_OUTPUT_DIRECTORY}")
if("${QT_INSTALL_QML}" STREQUAL "")
  set(QT_INSTALL_QML "${CMAKE_INSTALL_LIBDIR}/qt6/qml")
endif()

# FetchContent driver. To enable the flatpak-aware dependency provider
# (source redirect + transitive auto-record), pass on the cmake command line:
#   -DCMAKE_PROJECT_TOP_LEVEL_INCLUDES=cmake/FetchDepsProvider.cmake
include(${CMAKE_CURRENT_SOURCE_DIR}/cmake/FetchDeps.cmake)
fetchdeps(${CMAKE_CURRENT_SOURCE_DIR}/deps.json)

qt_standard_project_setup()

# ---------------------------------------------------------------------------
# Control-plane protobuf (shared schema with the Rust backend).
# Generates C++/QML types from ../proto/control.proto via qt_add_protobuf.
# ---------------------------------------------------------------------------
set(WAYWALLEN_PROTO_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../proto")
qt_add_protobuf(waywallen-control
    QML
    QML_URI waywallen.control
    PROTO_FILES
        "${WAYWALLEN_PROTO_DIR}/control.proto"
    PROTO_INCLUDES
        "${WAYWALLEN_PROTO_DIR}"
)

set(SOURCES_NO_MOC
  src/backend.cpp
  src/app.cpp
  src/daemon_dbus.cpp
  src/display.cpp
  src/query/query.cpp
  src/query/wallpaper_query.cpp
  src/query/renderer_query.cpp
  src/query/source_query.cpp
  src/query/health_query.cpp
  src/query/display_query.cpp
)

set(SOURCES
  src/qml_register.cpp
)

set(MOC_SOURCE_FILES
  src/app.cppm
  src/backend.cppm
  src/daemon_dbus.cppm
  src/display.cppm
  src/query/query.cppm
  src/query/wallpaper_query.cppm
  src/query/renderer_query.cppm
  src/query/source_query.cppm
  src/query/health_query.cppm
  src/query/display_query.cppm
)
set(MODULES
    src/mod.cppm
    src/proto.cppm
    src/query/mod.cppm
)

set(QML_FILES
  qml/Window.qml
  qml/component/StatusDot.qml
  qml/page/WallpaperPage.qml
  qml/page/WallpaperCard.qml
  qml/page/StatusPage.qml
  qml/page/DisplaysPage.qml
)
set(RESOURCES)

#set_source_files_properties(qml/Global.qml PROPERTIES QT_QML_SINGLETON_TYPE TRUE)

set(QML_REG_INCLUDE_DIRS
    "${CMAKE_CURRENT_SOURCE_DIR}/include/waywallen/register"
    "${CMAKE_CURRENT_SOURCE_DIR}/include/waywallen/query"
)

set(APP_INFO_MACROS
    APP_ID="${APP_ID}" APP_VERSION="${APP_VERSION}" APP_AUTHOR="${APP_AUTHOR}"
    APP_NAME="${APP_NAME}" APP_SUMMARY="${APP_SUMMARY}")

qt_add_qml_module(
  waywallen-qml
  URI
  waywallen.ui
  STATIC
  PLUGIN_TARGET
  waywallen-qml
  DEPENDENCIES
  QtCore
  QtQuick
  QtQuick.Shapes
  Qcm.Material
  waywallen.control
  RESOURCE_PREFIX
  /
  VERSION
  ${APP_VERSION}
  QML_FILES
  ${QML_FILES}
  SOURCES
  ${SOURCES}
  #RESOURCES
  #${RESOURCES}
  NO_PLUGIN_OPTIONAL)

target_sources(waywallen-qml PUBLIC FILE_SET CXX_MODULES FILES ${MODULES} ${MOC_SOURCE_FILES})
target_sources(waywallen-qml PRIVATE ${SOURCES_NO_MOC})

set_source_files_properties(
  "${CMAKE_CURRENT_BINARY_DIR}/waywallen-qml_qmltyperegistrations.cpp" TARGET_DIRECTORY
  waywallen-qml PROPERTIES INCLUDE_DIRECTORIES "${QML_REG_INCLUDE_DIRS}")
set_source_files_properties(${SOURCES_NO_MOC} PROPERTIES SKIP_AUTOMOC ON)
#set_source_files_properties( PROPERTIES COMPILE_DEFINITIONS "${APP_INFO_MACROS}")

set_target_properties(waywallen-qml PROPERTIES CXX_SCAN_FOR_MODULES ON)

split_moc_files(
  TARGET
  waywallen-qml
  BASE_DIR
  src
  PREFIX
  waywallen
  MOC_SOURCE_FILES
  ${MOC_SOURCE_FILES})

#target_compile_definitions(
#  waywallen.ui
#  PUBLIC APP_ID="${APP_ID}" APP_VERSION="${APP_VERSION}"
#         APP_AUTHOR="${APP_AUTHOR}" APP_NAME="${APP_NAME}"
#         APP_SUMMARY="${APP_SUMMARY}")

target_include_directories(
  waywallen-qml
  PUBLIC include
  PRIVATE . # src/
)
target_compile_features(waywallen-qml PUBLIC cxx_std_20)
target_link_libraries(waywallen-qml
  PUBLIC
    Qt6::DBus
    Qt6::Protobuf
    Qt6::ProtobufQuick
    Qt6::ProtobufWellKnownTypes
    waywallen-control
    qml_material
    ncrequest
    qextra
)

qt_add_executable(${PROJECT_NAME} MANUAL_FINALIZATION src/main.cpp)
set_target_properties(${PROJECT_NAME} PROPERTIES CXX_SCAN_FOR_MODULES ON)
target_compile_definitions(${PROJECT_NAME} PRIVATE "${APP_INFO_MACROS}")
target_link_libraries(${PROJECT_NAME} PRIVATE waywallen-qml)

set(APP_DESKTOP assets/waywallen-ui.desktop)
set(APP_METAINFO assets/waywallen-ui.metainfo.xml)
set(APP_SVG assets/waywallen-ui.svg)
file(READ assets/waywallen-ui.releases.xml APP_RELEASES)
configure_file(${APP_DESKTOP}.in ${APP_DESKTOP} @ONLY)
configure_file(${APP_METAINFO}.in ${APP_METAINFO} @ONLY)

install(
  TARGETS ${PROJECT_NAME}
  BUNDLE DESTINATION .
  RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR}
  LIBRARY DESTINATION ${CMAKE_INSTALL_LIBDIR})

install(
  FILES "${CMAKE_CURRENT_BINARY_DIR}/${APP_DESKTOP}"
  RENAME ${APP_ID}.desktop
  DESTINATION share/applications)
install(
  FILES "${CMAKE_CURRENT_BINARY_DIR}/${APP_METAINFO}"
  RENAME ${APP_ID}.metainfo.xml
  DESTINATION share/metainfo)
if(EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/${APP_SVG}")
  install(
    FILES ${APP_SVG}
    RENAME ${APP_ID}.svg
    DESTINATION share/icons/hicolor/scalable/apps)
endif()

qt_finalize_target(${PROJECT_NAME})
