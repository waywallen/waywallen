cmake_minimum_required(VERSION 3.28)

set(APP_ID io.github.waywallen)
set(APP_NAME "waywallen-ui")
set(APP_VERSION "0.1.0")
set(APP_SUMMARY "")
set(APP_AUTHOR "hypengw")

project(waywallen-ui LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

include(FetchContent)

if(EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/rstd")
  add_subdirectory(rstd)
else()
  FetchContent_Declare(
    rstd
    GIT_REPOSITORY https://github.com/hypengw/rstd.git
    GIT_TAG 2528b1cdb76624df9e01c97a7ab23dbd905afe3a
    GIT_SHALLOW 0
    GIT_PROGRESS 1
    EXCLUDE_FROM_ALL)
  FetchContent_MakeAvailable(rstd)
endif()

if(EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/ncrequest")
  add_subdirectory(ncrequest)
else()
  FetchContent_Declare(
    ncrequest
    GIT_REPOSITORY https://github.com/hypengw/ncrequest.git
    GIT_TAG 609e63b801a9f96d1d7a5f9b71d34158640bbfa1
    GIT_SHALLOW 0
    GIT_PROGRESS 1
    EXCLUDE_FROM_ALL)
  FetchContent_MakeAvailable(ncrequest)
endif()

if(EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/kstore")
  add_subdirectory(kstore)
else()
  FetchContent_Declare(
    kstore
    GIT_REPOSITORY https://github.com/hypengw/kstore.git
    GIT_TAG 4a706464da0459762e709f2c3f233d3ba3c5818b
    GIT_SHALLOW 1
    GIT_PROGRESS 1)
  FetchContent_MakeAvailable(kstore)
endif()


FetchContent_Declare(
  qml_material
  GIT_REPOSITORY https://github.com/hypengw/QmlMaterial.git
  GIT_TAG f8e53dfc6e340b817edccc7b7c56d7b1544052ba
  GIT_SHALLOW 0
  GIT_PROGRESS 1
  FIND_PACKAGE_ARGS GLOBAL)
FetchContent_MakeAvailable(qml_material)

if(EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/QExtra")
  add_subdirectory(QExtra)
else()
  FetchContent_Declare(
    qextra
    GIT_REPOSITORY https://github.com/hypengw/QExtra.git
    GIT_TAG 3577fe189ad642239f536f8b8f179590437027c6
    GIT_SHALLOW 0
    GIT_PROGRESS 1
    FIND_PACKAGE_ARGS GLOBAL)
  FetchContent_MakeAvailable(qextra)
endif()

#include(${CMAKE_SOURCE_DIR}/cmake/SplitMocFiles.cmake)
find_package(Qt6 REQUIRED COMPONENTS Quick Protobuf ProtobufQuick ProtobufWellKnownTypes)
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

set(SOURCES
)

set(MOC_SOURCE_FILES
)
set(MODULES
    src/mod.cppm
)

set(QML_FILES
  qml/Window.qml
)
set(RESOURCES)

#set_source_files_properties(qml/Global.qml PROPERTIES QT_QML_SINGLETON_TYPE TRUE)

set(QML_REG_INCLUDE_DIRS "${CMAKE_CURRENT_SOURCE_DIR}/include/Qcm/qml")

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

#set_source_files_properties(
#  "${CMAKE_CURRENT_BINARY_DIR}/waywallen.ui.qml_qmltyperegistrations.cpp" TARGET_DIRECTORY
#  waywallen.ui PROPERTIES INCLUDE_DIRECTORIES "${QML_REG_INCLUDE_DIRS}")
#set_source_files_properties(${SOURCES2} PROPERTIES SKIP_AUTOMOC ON)
#set_source_files_properties( PROPERTIES COMPILE_DEFINITIONS "${APP_INFO_MACROS}")

set_target_properties(waywallen-qml PROPERTIES CXX_SCAN_FOR_MODULES ON)

#split_moc_files(
#  TARGET
#  waywallen
#  BASE_DIR
#  src
#  PREFIX
#  waywallen
#  MOC_SOURCE_FILES
#  ${MOC_SOURCE_FILES})

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
    Qt6::Core
    Qt6::Protobuf
    Qt6::ProtobufQuick
    Qt6::ProtobufWellKnownTypes
    waywallen-control
    qml_material
    ncrequest
)

qt_add_executable(${PROJECT_NAME} MANUAL_FINALIZATION src/main.cpp)
set_target_properties(${PROJECT_NAME} PROPERTIES CXX_SCAN_FOR_MODULES ON)
target_compile_definitions(${PROJECT_NAME} PRIVATE "${APP_INFO_MACROS}")
target_link_libraries(${PROJECT_NAME} PRIVATE waywallen-qml)

#set(APP_DESKTOP assets/waywallen-ui.desktop)
#set(APP_METAINFO assets/waywallen-ui.metainfo.xml)
#set(APP_SVG assets/waywallen-ui.svg)
#file(READ assets/waywallen-ui.releases.xml APP_RELEASES)
#configure_file(${APP_DESKTOP}.in ${APP_DESKTOP} @ONLY)
#configure_file(${APP_METAINFO}.in ${APP_METAINFO} @ONLY)

install(
  TARGETS ${PROJECT_NAME}
  BUNDLE DESTINATION .
  RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR}
  LIBRARY DESTINATION ${CMAKE_INSTALL_LIBDIR})

#  install(
#    FILES "${CMAKE_CURRENT_BINARY_DIR}/${APP_DESKTOP}"
#    RENAME ${APP_ID}.desktop
#    DESTINATION share/applications)
#  install(
#    FILES "${CMAKE_CURRENT_BINARY_DIR}/${APP_METAINFO}"
#    RENAME ${APP_ID}.metainfo.xml
#    DESTINATION share/metainfo)
#  install(
#    FILES ${APP_SVG}
#    RENAME ${APP_ID}.svg
#    DESTINATION share/icons/hicolor/scalable/apps)

qt_finalize_target(${PROJECT_NAME})
