# SplitMocFiles.cmake
# ~~~
#
# This module provides functionality to split Qt MOC files into separate header and source files.
# This is useful when working with C++ modules that need to include MOC files.
#
# Usage:
#   include(cmake/SplitMocFiles.cmake)
#   split_moc_files(
#     TARGET target_name
#     MOC_SOURCE_FILES file1 file2 ...
#   )
#
# The function will:
# 1. Create custom commands to split each MOC file after Qt's autogen runs
# 2. Create a custom target that depends on all split operations
# 3. Set up proper dependencies so source files wait for their MOC files to be split
#
# Main function to set up MOC file splitting for a target
#
# Arguments:
#   TARGET - The CMake target that needs MOC splitting
#   MOC_SOURCE_FILES - List of base names of source files that generate MOC files
#                      (e.g., "appModel" for "appModel.cppm" which generates "appModel.moc")
#   SOURCE_EXTENSION - (Optional) File extension of source files (default: .cppm)
#
# Example:
#   split_moc_files(
#     TARGET module_rueckruf_models
#     MOC_SOURCE_FILES appModel deckModel
#   )
# ~~~

function(split_moc_files)
  # Parse arguments
  set(options "")
  set(oneValueArgs TARGET BASE_DIR PREFIX)
  set(multiValueArgs MOC_SOURCE_FILES)
  cmake_parse_arguments(ARG "${options}" "${oneValueArgs}" "${multiValueArgs}"
                        ${ARGN})

  # Validate required arguments
  if(NOT ARG_TARGET)
    message(FATAL_ERROR "split_moc_files: TARGET argument is required")
  endif()

  if(NOT ARG_MOC_SOURCE_FILES)
    message(
      FATAL_ERROR "split_moc_files: MOC_SOURCE_FILES argument is required")
  endif()

  # Get the current source directory
  set(SOURCE_DIR "${CMAKE_CURRENT_SOURCE_DIR}")
  set(BINARY_DIR "${CMAKE_CURRENT_BINARY_DIR}")
  set(AUTOGEN_DIR "${BINARY_DIR}/${ARG_TARGET}_autogen/include")

  # Path to the implementation script
  set(SPLIT_SCRIPT "${CMAKE_SOURCE_DIR}/cmake/SplitSingleMocImpl.cmake")

  # Create individual custom commands for each .moc file
  set(MOC_SPLIT_OUTPUTS "")
  foreach(SOURCE_FILE ${ARG_MOC_SOURCE_FILES})
    cmake_path(RELATIVE_PATH SOURCE_FILE BASE_DIRECTORY ${ARG_BASE_DIR} OUTPUT_VARIABLE MOC_BASE)
    cmake_path(REMOVE_EXTENSION MOC_BASE LAST_ONLY)
    set(MOC_BASE "${ARG_PREFIX}/${MOC_BASE}")
    set(MOC_FILE "${AUTOGEN_DIR}/${MOC_BASE}.moc")
    set(OUTPUT_FILE "${MOC_FILE}.h")

    add_custom_command(
      OUTPUT ${OUTPUT_FILE}
      COMMAND ${CMAKE_COMMAND} -DMOC_FILE=${MOC_FILE} -DMOC_BASE=${MOC_BASE}
              -DAUTOGEN_DIR=${AUTOGEN_DIR} -P ${SPLIT_SCRIPT}
      # COMMAND ${CMAKE_COMMAND} -E touch ${STAMP_FILE}
      DEPENDS ${SOURCE_DIR}/${SOURCE_FILE}
      COMMENT "Splitting ${MOC_FILE}")

    list(APPEND MOC_SPLIT_OUTPUTS ${OUTPUT_FILE})

    # Set source file dependency on stamp file
    set_source_files_properties(${SOURCE_DIR}/${SOURCE_FILE}
                                PROPERTIES OBJECT_DEPENDS ${OUTPUT_FILE})
  endforeach()

  # Create a custom target that depends on all outputs
  set(SPLIT_TARGET_NAME "${ARG_TARGET}_moc_split")
  add_custom_target(${SPLIT_TARGET_NAME} ALL DEPENDS ${MOC_SPLIT_OUTPUTS})

  # Ensure the split happens after autogen completes
  add_dependencies(${SPLIT_TARGET_NAME} ${ARG_TARGET}_autogen)

  list(LENGTH ARG_MOC_SOURCE_FILES NUM_FILES)
  message(
    STATUS
      "Set up MOC splitting for target ${ARG_TARGET} with ${NUM_FILES} files")
endfunction()
