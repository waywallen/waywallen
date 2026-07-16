cmake_minimum_required(VERSION 3.28)

set(APP_ID org.waywallen.waywallen)
set(APP_NAME "waywallen")
set(APP_VERSION "0.2.5")
set(APP_SUMMARY "Wallpaper Manager for Linux")
set(APP_AUTHOR "hypengw")

project(waywallen-ui LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 23)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(BUILD_SHARED_LIBS OFF)

find_package(Qt6 REQUIRED COMPONENTS Quick DBus Protobuf ProtobufQuick ProtobufWellKnownTypes QuickPrivate)
include(${CMAKE_CURRENT_SOURCE_DIR}/cmake/SplitMocFiles.cmake)

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

set(CMAKE_POLICY_DEFAULT_CMP0077 NEW)
include(${CMAKE_SOURCE_DIR}/cmake/FetchDeps.cmake)
fetchdeps(${CMAKE_SOURCE_DIR}/deps.json)

qt_standard_project_setup()

# ---------------------------------------------------------------------------
# Control-plane protobuf (shared schema with the Rust backend).
# Generates C++/QML types from ../proto/control.proto and ../proto/filter.proto
# via qt_add_protobuf.
# ---------------------------------------------------------------------------
set(WAYWALLEN_PROTO_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../proto")
qt_add_protobuf(waywallen-control
    QML
    QML_URI waywallen.control
    PROTO_FILES
        "${WAYWALLEN_PROTO_DIR}/control.proto"
        "${WAYWALLEN_PROTO_DIR}/filter.proto"
    PROTO_INCLUDES
        "${WAYWALLEN_PROTO_DIR}"
)

set(SOURCES_NO_MOC
  src/entry.cpp
  src/action.cpp
  src/util.cpp
  src/backend.cpp
  src/app.cpp
  src/daemon_dbus.cpp
  src/objmodel/display.cpp
  src/objmodel/gpu.cpp
  src/objmodel/library.cpp
  src/notify.cpp
  src/objmodel/renderer.cpp
  src/msg/store.cpp
  src/model/store_item.cpp
  src/model/list_models.cpp
  src/model/wallpaper_select_storage.cpp
  src/model/remote_model.cpp
  src/model/filter_rule_model.cpp
  src/model/user_property_model.cpp
  src/query/wallpaper_query.cpp
  src/query/progress_query.cpp
  src/query/renderer_query.cpp
  src/query/source_query.cpp
  src/query/plugin_query.cpp
  src/query/tag_query.cpp
  src/query/health_query.cpp
  src/query/display_query.cpp
  src/query/gpu_query.cpp
  src/query/library_query.cpp
  src/query/settings_query.cpp
  src/query/autostart_query.cpp
  src/query/remote_query.cpp
  src/query/playlist_query.cpp
  src/thumb/service.cpp
)

set(SOURCES
  src/qml_register.cpp
)

set(MOC_SOURCE_FILES
  src/action.cppm
  src/util.cppm
  src/app.cppm
  src/backend.cppm
  src/daemon_dbus.cppm
  src/objmodel/display.cppm
  src/objmodel/gpu.cppm
  src/objmodel/library.cppm
  src/notify.cppm
  src/objmodel/renderer.cppm
  src/msg/store.cppm
  src/model/store_item.cppm
  src/model/list_models.cppm
  src/model/wallpaper_select_storage.cppm
  src/model/remote_model.cppm
  src/model/filter_rule_model.cppm
  src/model/user_property_model.cppm
  src/query/wallpaper_query.cppm
  src/query/progress_query.cppm
  src/query/renderer_query.cppm
  src/query/source_query.cppm
  src/query/plugin_query.cppm
  src/query/tag_query.cppm
  src/query/health_query.cppm
  src/query/display_query.cppm
  src/query/gpu_query.cppm
  src/query/library_query.cppm
  src/query/settings_query.cppm
  src/query/autostart_query.cppm
  src/query/remote_query.cppm
  src/query/playlist_query.cppm
  src/thumb/service.cppm
)
set(MODULES
    src/entry.cppm
    src/mod.cppm
    src/proto.cppm
    src/model/share_store.cppm
    src/msg/backend_msg.cppm
    src/query/query.cppm
    src/query/mod.cppm
)

set(QML_FILES
  qml/Window.qml
  qml/Global.qml
  qml/component/StatusDot.qml
  qml/component/PagePopup.qml
  qml/component/Tag.qml
  qml/component/GpuTag.qml
  qml/component/KdeDisplaysHelp.qml
  qml/component/GnomeDisplaysHelp.qml
  qml/component/LayerShellDisplaysHelp.qml
  qml/component/ThumbnailImage.qml
  qml/component/SearchChip.qml
  qml/component/ValueSlider.qml
  qml/component/DisplayLayoutSection.qml
  qml/component/filter/StringFilter.qml
  qml/component/filter/IntFilter.qml
  qml/component/filter/WpTypeFilter.qml
  qml/component/filter/TagFilter.qml
  qml/component/filter/TagPickerDialog.qml
  qml/component/filter/ContentRatingFilter.qml
  qml/component/filter/EmptyFilter.qml
  qml/component/filter/WallpaperFilter.qml
  qml/component/settings/SettingField.qml
  qml/dialog/DaemonNotRunDialog.qml
  qml/dialog/WallpaperFilterDialog.qml
  qml/dialog/RemoteFilterDialog.qml
  qml/page/WallpaperPage.qml
  qml/page/PlaylistDetailPanel.qml
  qml/page/WallpaperDetailPanel.qml
  qml/page/WallpaperInfoPage.qml
  qml/page/WallpaperCard.qml
  qml/page/wallpaper/SelectSheet.qml
  qml/page/wallpaper/TweakSheet.qml
  qml/page/wallpaper/TweakState.qml
  qml/page/wallpaper/NewPlaylistSheetContent.qml
  qml/page/wallpaper/AddToPlaylistSheetContent.qml
  qml/page/wallpaper/SelectSheetContentState.qml
  qml/page/wallpaper/PlaylistListSheet.qml
  qml/page/wallpaper/PlaylistListSheetState.qml
  qml/page/DiscoverPage.qml
  qml/page/RemoteInfoPage.qml
  qml/page/RemoteCard.qml
  qml/page/StatusPage.qml
  qml/page/PluginManagePage.qml
  qml/page/SettingsPage.qml
  qml/page/DisplaysPage.qml
  qml/page/SourceManagePage.qml
  qml/page/AddLibraryPage.qml
  qml/page/AboutPage.qml
  qml/page/PluginSettingsPage.qml
)
set(RESOURCES
  assets/waywallen-ui.svg
)

set_source_files_properties(qml/Global.qml PROPERTIES QT_QML_SINGLETON_TYPE TRUE)

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
  QtQml.Models
  Qcm.Material
  waywallen.control
  OUTPUT_TARGETS
  waywallen_ui_targets
  RESOURCE_PREFIX
  /
  VERSION
  ${APP_VERSION}
  QML_FILES
  ${QML_FILES}
  SOURCES
  ${SOURCES}
  RESOURCES
  ${RESOURCES}
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
target_compile_definitions(waywallen-qml PRIVATE "${APP_INFO_MACROS}")
target_link_libraries(waywallen-qml
  PUBLIC
    Qt6::DBus
    Qt6::Protobuf
    Qt6::ProtobufQuick
    Qt6::ProtobufWellKnownTypes
    waywallen-control
    qml_material::qml_material
    ncrequest
    qextra
    wavsen::decode
)

qt_add_executable(${PROJECT_NAME} MANUAL_FINALIZATION src/main.cpp)
set_target_properties(${PROJECT_NAME} PROPERTIES 
  CXX_SCAN_FOR_MODULES ON
  INSTALL_RPATH "${WAYWALLEN_BIN_RPATH}"
)
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
