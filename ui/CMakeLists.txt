cmake_minimum_required(VERSION 3.28)

set(APP_ID io.github.waywallen)
set(APP_NAME "waywallen-ui")
set(APP_VERSION "0.1.0")
set(APP_SUMMARY "")
set(APP_AUTHOR "hypengw")

project(waywallen-ui LANGUAGES CXX)

#include(${CMAKE_SOURCE_DIR}/cmake/SplitMocFiles.cmake)
find_package(Qt6 REQUIRED COMPONENTS Quick ProtobufQuick ProtobufWellKnownTypes) 

qt_standard_project_setup()

set(SOURCES
)

set(MOC_SOURCE_FILES
)
set(MODULES
    src/mod.cppm
)

set(QML_FILES
)
set(RESOURCES)

#set_source_files_properties(qml/Global.qml PROPERTIES QT_QML_SINGLETON_TYPE TRUE)

set(QML_REG_INCLUDE_DIRS "${CMAKE_CURRENT_SOURCE_DIR}/include/Qcm/qml")

set(APP_INFO_MACROS
    APP_ID="${APP_ID}" APP_VERSION="${APP_VERSION}" APP_AUTHOR="${APP_AUTHOR}"
    APP_NAME="${APP_NAME}" APP_SUMMARY="${APP_SUMMARY}")

qt_add_qml_module(
  waywallen.ui.qml
  URI
  waywallen.ui
  STATIC
  PLUGIN_TARGET
  waywallen.ui.qml
  DEPENDENCIES
  QtCore
  QtQuick
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

target_sources(waywallen.ui.qml PUBLIC FILE_SET CXX_MODULES FILES ${MODULES} ${MOC_SOURCE_FILES})

#set_source_files_properties(
#  "${CMAKE_CURRENT_BINARY_DIR}/waywallen.ui.qml_qmltyperegistrations.cpp" TARGET_DIRECTORY
#  waywallen.ui.qml PROPERTIES INCLUDE_DIRECTORIES "${QML_REG_INCLUDE_DIRS}")

set_source_files_properties(${SOURCES2} PROPERTIES SKIP_AUTOMOC ON)

#set_source_files_properties( PROPERTIES COMPILE_DEFINITIONS "${APP_INFO_MACROS}")

# QT_QMLCACHEGEN_ARGUMENTS "--only-bytecode"
set_target_properties(waywallen.ui.qml PROPERTIES CXX_SCAN_FOR_MODULES ON)

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
#  waywallen.ui.qml
#  PUBLIC APP_ID="${APP_ID}" APP_VERSION="${APP_VERSION}"
#         APP_AUTHOR="${APP_AUTHOR}" APP_NAME="${APP_NAME}"
#         APP_SUMMARY="${APP_SUMMARY}")

target_include_directories(
  waywallen.ui.qml
  PUBLIC include
  PRIVATE . # src/
)
target_compile_features(waywallen.ui.qml PUBLIC cxx_std_20)

qt_add_executable(${PROJECT_NAME} MANUAL_FINALIZATION src/main.cpp)
set_target_properties(${PROJECT_NAME} PROPERTIES CXX_SCAN_FOR_MODULES ON
                                                 WIN32_EXECUTABLE FALSE)
target_compile_definitions(${PROJECT_NAME} PRIVATE "${APP_INFO_MACROS}")

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
