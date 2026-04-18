# FetchDeps.cmake
#
# JSON-driven FetchContent driver. `include()` after project() and call
# fetchdeps() to declare/resolve dependencies from a deps.json manifest.
#
#     include(${CMAKE_CURRENT_SOURCE_DIR}/cmake/FetchDeps.cmake)
#     fetchdeps(${CMAKE_CURRENT_SOURCE_DIR}/deps.json)
#
# The deps.json format is a valid flatpak-builder sources array — the
# `x-cmake` sidecar is an extension key (flatpak-builder ignores `x-*`
# keys). Reference the same file from a flatpak manifest with:
#
#     sources:
#       - path/to/deps.json
#
# For the flatpak-aware dependency provider (source-tree redirect inside
# flatpak-builder + auto-record of transitive deps), load its companion
# FetchDepsProvider.cmake via CMAKE_PROJECT_TOP_LEVEL_INCLUDES *before*
# project(). See FetchDepsProvider.cmake for details.

include_guard(GLOBAL)
include(FetchContent)

# ---------------------------------------------------------------------------
# JSON helpers
# ---------------------------------------------------------------------------

function(_fetchdeps_json_has_key out json key)
  string(JSON _t ERROR_VARIABLE _err TYPE "${json}" "${key}")
  if(_err)
    set(${out} FALSE PARENT_SCOPE)
  else()
    set(${out} TRUE PARENT_SCOPE)
  endif()
endfunction()

function(_fetchdeps_json_get_opt out json)
  string(JSON _v ERROR_VARIABLE _err GET "${json}" ${ARGN})
  if(_err)
    set(${out} "" PARENT_SCOPE)
  else()
    set(${out} "${_v}" PARENT_SCOPE)
  endif()
endfunction()

# ---------------------------------------------------------------------------
# Registry (global properties so nested subdirs share state)
# ---------------------------------------------------------------------------

function(_fetchdeps_is_declared out name)
  get_property(_d GLOBAL PROPERTY _FETCHDEPS_DECLARED)
  if(name IN_LIST _d)
    set(${out} TRUE PARENT_SCOPE)
  else()
    set(${out} FALSE PARENT_SCOPE)
  endif()
endfunction()

function(_fetchdeps_mark_declared name)
  set_property(GLOBAL APPEND PROPERTY _FETCHDEPS_DECLARED "${name}")
endfunction()

# ---------------------------------------------------------------------------
# Root-entry fetch
#
# Implemented as a macro (not a function) so that <name>_SOURCE_DIR and
# <name>_BINARY_DIR populated by FetchContent_MakeAvailable remain visible
# to the caller's scope — users rely on those to add_subdirectory or set
# INTERFACE include paths post-fetch.
# ---------------------------------------------------------------------------

macro(_fetchdeps_fetch_one _fd_entry _fd_source_root)
  string(JSON _fd_name  GET "${_fd_entry}" "x-cmake" name)
  string(JSON _fd_dtype GET "${_fd_entry}" type)

  # Mark before any work so even the local-override path counts as declared.
  _fetchdeps_mark_declared("${_fd_name}")

  # If a transitive FetchContent already populated this dep earlier in the
  # same configure (e.g. ncrequest's CMakeLists pulled pegtl before the root
  # loop reached the pegtl entry), don't re-declare it — FetchContent would
  # rerun find_package and collide with the existing binary dir.
  FetchContent_GetProperties(${_fd_name})
  if(${_fd_name}_POPULATED)
    # already populated — nothing to do
  elseif(EXISTS "${_fd_source_root}/${_fd_name}")
    # Workspace-local override.
    message(STATUS "fetchdeps: using local ${_fd_source_root}/${_fd_name}")
    add_subdirectory("${_fd_source_root}/${_fd_name}" "${_fd_name}")
  else()
    # Stash dest + exclude-from-all so the provider can reach them.
    _fetchdeps_json_get_opt(_fd_dest "${_fd_entry}" dest)
    if(_fd_dest)
      set(_FETCHDEPS_DEST_${_fd_name} "${_fd_dest}" CACHE INTERNAL "" FORCE)
    endif()

    set(_fd_declare_args "")
    set(_fd_exclude_from_all FALSE)

    if(_fd_dtype STREQUAL "git")
      _fetchdeps_json_get_opt(_fd_url    "${_fd_entry}" url)
      _fetchdeps_json_get_opt(_fd_commit "${_fd_entry}" commit)
      _fetchdeps_json_get_opt(_fd_tag    "${_fd_entry}" tag)
      _fetchdeps_json_get_opt(_fd_branch "${_fd_entry}" branch)
      _fetchdeps_json_get_opt(_fd_dshallow "${_fd_entry}" "disable-shallow-clone")

      if(NOT _fd_url)
        message(FATAL_ERROR "fetchdeps: '${_fd_name}' type=git requires 'url'")
      endif()
      list(APPEND _fd_declare_args GIT_REPOSITORY "${_fd_url}")

      if(_fd_commit)
        list(APPEND _fd_declare_args GIT_TAG "${_fd_commit}")
      elseif(_fd_tag)
        list(APPEND _fd_declare_args GIT_TAG "${_fd_tag}")
      elseif(_fd_branch)
        list(APPEND _fd_declare_args GIT_TAG "${_fd_branch}")
      else()
        message(FATAL_ERROR
          "fetchdeps: '${_fd_name}' type=git requires one of commit/tag/branch")
      endif()

      if(_fd_dshallow STREQUAL "true")
        list(APPEND _fd_declare_args GIT_SHALLOW FALSE)
      else()
        list(APPEND _fd_declare_args GIT_SHALLOW TRUE)
      endif()

    elseif(_fd_dtype STREQUAL "archive" OR _fd_dtype STREQUAL "file")
      _fetchdeps_json_get_opt(_fd_url           "${_fd_entry}" url)
      _fetchdeps_json_get_opt(_fd_dest_filename "${_fd_entry}" "dest-filename")
      if(NOT _fd_url)
        message(FATAL_ERROR "fetchdeps: '${_fd_name}' type=${_fd_dtype} requires 'url'")
      endif()
      list(APPEND _fd_declare_args URL "${_fd_url}")
      if(_fd_dest_filename)
        list(APPEND _fd_declare_args DOWNLOAD_NAME "${_fd_dest_filename}")
      endif()
      if(_fd_dtype STREQUAL "file")
        list(APPEND _fd_declare_args DOWNLOAD_NO_EXTRACT TRUE)
      endif()

      set(_fd_hash "")
      foreach(_fd_algo sha512 sha256 sha1 md5)
        _fetchdeps_json_get_opt(_fd_v "${_fd_entry}" "${_fd_algo}")
        if(_fd_v)
          set(_fd_hash "${_fd_algo}=${_fd_v}")
          break()
        endif()
      endforeach()
      if(NOT _fd_hash)
        message(FATAL_ERROR
          "fetchdeps: '${_fd_name}' type=${_fd_dtype} requires sha512/sha256/sha1/md5")
      endif()
      list(APPEND _fd_declare_args URL_HASH "${_fd_hash}")

    else()
      message(FATAL_ERROR "fetchdeps: '${_fd_name}' unsupported type '${_fd_dtype}'")
    endif()

    # x-cmake sidecar.
    _fetchdeps_json_has_key(_fd_has_xc "${_fd_entry}" "x-cmake")
    if(_fd_has_xc)
      string(JSON _fd_xc GET "${_fd_entry}" "x-cmake")

      _fetchdeps_json_get_opt(_fd_v "${_fd_xc}" exclude_from_all)
      if(_fd_v STREQUAL "true")
        set(_fd_exclude_from_all TRUE)
        list(APPEND _fd_declare_args EXCLUDE_FROM_ALL)
      endif()

      _fetchdeps_json_get_opt(_fd_v "${_fd_xc}" find_package_args)
      if(_fd_v)
        separate_arguments(_fd_fpa UNIX_COMMAND "${_fd_v}")
        list(APPEND _fd_declare_args FIND_PACKAGE_ARGS ${_fd_fpa})
      endif()

      _fetchdeps_json_get_opt(_fd_v "${_fd_xc}" source_subdir)
      if(_fd_v)
        list(APPEND _fd_declare_args SOURCE_SUBDIR "${_fd_v}")
        set(_FETCHDEPS_SOURCE_SUBDIR_${_fd_name} "${_fd_v}" CACHE INTERNAL "" FORCE)
      endif()

      _fetchdeps_json_has_key(_fd_has_sub "${_fd_xc}" git_submodules)
      if(_fd_has_sub)
        string(JSON _fd_sub_len LENGTH "${_fd_xc}" git_submodules)
        if(_fd_sub_len GREATER 0)
          set(_fd_subs "")
          math(EXPR _fd_sub_last "${_fd_sub_len} - 1")
          foreach(_fd_i RANGE 0 ${_fd_sub_last})
            string(JSON _fd_s GET "${_fd_xc}" git_submodules ${_fd_i})
            list(APPEND _fd_subs "${_fd_s}")
          endforeach()
          list(APPEND _fd_declare_args GIT_SUBMODULES ${_fd_subs})
        endif()
      endif()
    endif()

    set(_FETCHDEPS_EXCLUDE_${_fd_name} "${_fd_exclude_from_all}" CACHE INTERNAL "" FORCE)

    FetchContent_Declare(${_fd_name} ${_fd_declare_args})
    FetchContent_MakeAvailable(${_fd_name})
  endif()
endmacro()

# ---------------------------------------------------------------------------
# Public entry point
#
# Macro (not function) so <dep>_SOURCE_DIR / <dep>_BINARY_DIR populated by
# FetchContent_MakeAvailable inside the loop remain visible to the caller.
# ---------------------------------------------------------------------------

macro(fetchdeps _fd_deps_path)
  if(NOT EXISTS "${_fd_deps_path}")
    message(FATAL_ERROR "fetchdeps: ${_fd_deps_path} not found")
  endif()

  file(READ "${_fd_deps_path}" _fd_deps_json)
  string(JSON _fd_n ERROR_VARIABLE _fd_err LENGTH "${_fd_deps_json}")
  if(_fd_err)
    message(FATAL_ERROR "fetchdeps: ${_fd_deps_path} is not valid JSON: ${_fd_err}")
  endif()

  # Expose the authoritative deps.json path to the provider so auto-record
  # can write new transitive deps back into it.
  set_property(GLOBAL PROPERTY _FETCHDEPS_JSON_PATH "${_fd_deps_path}")

  get_filename_component(_fd_top_source_root "${_fd_deps_path}" DIRECTORY)

  if(_fd_n GREATER 0)
    math(EXPR _fd_top_last "${_fd_n} - 1")
    # Pre-pass: mark every root entry as declared so transitive provider
    # calls that land on a name already in deps.json (possibly later in the
    # array) don't misfire autorecord during the main loop below.
    foreach(_fd_top_i RANGE 0 ${_fd_top_last})
      string(JSON _fd_top_entry GET "${_fd_deps_json}" ${_fd_top_i})
      string(JSON _fd_pre_name GET "${_fd_top_entry}" "x-cmake" name)
      _fetchdeps_mark_declared("${_fd_pre_name}")
    endforeach()
    foreach(_fd_top_i RANGE 0 ${_fd_top_last})
      string(JSON _fd_top_entry GET "${_fd_deps_json}" ${_fd_top_i})
      _fetchdeps_fetch_one("${_fd_top_entry}" "${_fd_top_source_root}")
    endforeach()
  endif()
endmacro()
