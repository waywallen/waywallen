# FetchDeps.cmake
#
# JSON-driven FetchContent with a flatpak-aware dependency provider.
#
# Load via `CMAKE_PROJECT_TOP_LEVEL_INCLUDES` *before* project() so the
# provider can be registered (cmake_language(SET_DEPENDENCY_PROVIDER ...)
# only works from top-level-include scripts). Example:
#
#     cmake_minimum_required(VERSION 3.28)
#     list(APPEND CMAKE_PROJECT_TOP_LEVEL_INCLUDES
#          ${CMAKE_CURRENT_SOURCE_DIR}/cmake/FetchDeps.cmake)
#     project(myapp)
#     fetchdeps(${CMAKE_CURRENT_SOURCE_DIR}/deps.json
#               [FLATPAK_OUT <path>])
#
# Each entry in deps.json carries its own flatpak `dest` path. Inside
# flatpak-builder ($FLATPAK_ID set), the provider redirects FetchContent
# to "$FLATPAK_BUILDER_BUILDDIR/<dest>" — no network required.
#
# Transitive FetchContent_MakeAvailable calls from subprojects that are
# not listed in deps.json are captured by the provider and appended to
# deps.json automatically, so the next flatpak-builder run already has
# a complete manifest.

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
# Auto-record: capture a transitive FetchContent dep into deps.json
# ---------------------------------------------------------------------------

# Rebuild a deps.json entry from a dep's saved FetchContent_Declare args,
# append it to deps.json on disk, and populate the cache vars the provider
# needs (dest / exclude).
function(_fetchdeps_autorecord name)
  get_property(deps_path GLOBAL PROPERTY _FETCHDEPS_JSON_PATH)
  if(NOT deps_path)
    return()
  endif()

  # Undocumented but stable across CMake 3.24+ — same escape hatch used by
  # the upstream flatpak provider reference implementation.
  __fetchcontent_getsaveddetails("${name}" _details)
  if(NOT _details)
    message(WARNING "fetchdeps: no saved details for transitive dep '${name}'")
    return()
  endif()

  set(_opts EXCLUDE_FROM_ALL DOWNLOAD_NO_EXTRACT)
  set(_one
      GIT_REPOSITORY GIT_TAG
      URL URL_HASH URL_MD5 DOWNLOAD_NAME
      SOURCE_SUBDIR)
  set(_multi GIT_SUBMODULES FIND_PACKAGE_ARGS)
  cmake_parse_arguments(FD "${_opts}" "${_one}" "${_multi}" ${_details})

  set(entry "{}")
  string(JSON entry SET "${entry}" name "\"${name}\"")

  if(FD_GIT_REPOSITORY)
    string(JSON entry SET "${entry}" type "\"git\"")
    string(JSON entry SET "${entry}" url "\"${FD_GIT_REPOSITORY}\"")
    if(FD_GIT_TAG)
      # Heuristic: 40-char hex treated as commit, anything else as tag.
      string(LENGTH "${FD_GIT_TAG}" _taglen)
      if(_taglen EQUAL 40)
        string(JSON entry SET "${entry}" commit "\"${FD_GIT_TAG}\"")
      else()
        string(JSON entry SET "${entry}" tag "\"${FD_GIT_TAG}\"")
      endif()
    endif()
    # GIT_SHALLOW default is FALSE unless set, so default disable-shallow=true.
    list(FIND _details GIT_SHALLOW _gs_idx)
    set(_disable_shallow "true")
    if(NOT _gs_idx EQUAL -1)
      math(EXPR _gs_val_idx "${_gs_idx} + 1")
      list(GET _details ${_gs_val_idx} _gs_val)
      if(_gs_val AND NOT _gs_val STREQUAL "FALSE" AND NOT _gs_val STREQUAL "0")
        set(_disable_shallow "false")
      endif()
    endif()
    string(JSON entry SET "${entry}" "disable-shallow-clone" "${_disable_shallow}")
  elseif(FD_URL)
    if(FD_DOWNLOAD_NO_EXTRACT)
      string(JSON entry SET "${entry}" type "\"file\"")
    else()
      string(JSON entry SET "${entry}" type "\"archive\"")
    endif()
    string(JSON entry SET "${entry}" url "\"${FD_URL}\"")
    if(FD_URL_HASH)
      if(FD_URL_HASH MATCHES "^([a-zA-Z0-9]+)=(.+)$")
        string(JSON entry SET "${entry}" "${CMAKE_MATCH_1}" "\"${CMAKE_MATCH_2}\"")
      endif()
    elseif(FD_URL_MD5)
      string(JSON entry SET "${entry}" md5 "\"${FD_URL_MD5}\"")
    endif()
    if(FD_DOWNLOAD_NAME)
      string(JSON entry SET "${entry}" "dest-filename" "\"${FD_DOWNLOAD_NAME}\"")
    endif()
  else()
    message(WARNING
      "fetchdeps: cannot auto-record '${name}' (no GIT_REPOSITORY or URL)")
    return()
  endif()

  set(_dest "build/_flatpak_deps/${name}-src")
  string(JSON entry SET "${entry}" dest "\"${_dest}\"")

  if(FD_EXCLUDE_FROM_ALL)
    set(_xc "{}")
    string(JSON _xc SET "${_xc}" exclude_from_all "true")
    string(JSON entry SET "${entry}" "x-cmake" "${_xc}")
  endif()

  # Append to deps.json on disk.
  file(READ "${deps_path}" _cur)
  string(JSON _n LENGTH "${_cur}")
  string(JSON _cur SET "${_cur}" ${_n} "${entry}")
  # Pretty-ish output: re-dump with newline at EOF.
  file(WRITE "${deps_path}" "${_cur}\n")

  # Populate provider-side state for this newly-recorded dep.
  set(_FETCHDEPS_DEST_${name} "${_dest}" CACHE INTERNAL "" FORCE)
  if(FD_EXCLUDE_FROM_ALL)
    set(_FETCHDEPS_EXCLUDE_${name} TRUE CACHE INTERNAL "" FORCE)
  else()
    set(_FETCHDEPS_EXCLUDE_${name} FALSE CACHE INTERNAL "" FORCE)
  endif()

  message(STATUS "fetchdeps: recorded transitive '${name}' -> ${deps_path}")
endfunction()

# ---------------------------------------------------------------------------
# Dependency provider
# ---------------------------------------------------------------------------

# Invoked by CMake for every FetchContent_MakeAvailable(dep_name).
# * Transitive deps not in deps.json are auto-recorded first.
# * In flatpak-builder: redirect to pre-staged sources at <BUILDDIR>/<dest>.
# * Otherwise: forward to default FetchContent by re-invoking
#   FetchContent_MakeAvailable — CMake detects the recursion and runs the
#   default fetch logic.
macro(_fetchdeps_provider method dep_name)
  _fetchdeps_is_declared(_fd_known "${dep_name}")
  if(NOT _fd_known)
    _fetchdeps_autorecord("${dep_name}")
    _fetchdeps_mark_declared("${dep_name}")
  endif()

  if(DEFINED ENV{FLATPAK_ID} AND DEFINED _FETCHDEPS_DEST_${dep_name})
    set(_fd_src "$ENV{FLATPAK_BUILDER_BUILDDIR}/${_FETCHDEPS_DEST_${dep_name}}")
    set(_fd_bin "${CMAKE_BINARY_DIR}/_deps/${dep_name}-build")
    message(STATUS "fetchdeps[flatpak]: ${dep_name} <- ${_fd_src}")
    if(EXISTS "${_fd_src}/CMakeLists.txt")
      set(_fd_extra "")
      if(_FETCHDEPS_EXCLUDE_${dep_name})
        list(APPEND _fd_extra EXCLUDE_FROM_ALL)
      endif()
      add_subdirectory("${_fd_src}" "${_fd_bin}" ${_fd_extra})
    endif()
    FetchContent_SetPopulated(${dep_name}
      SOURCE_DIR "${_fd_src}"
      BINARY_DIR "${_fd_bin}")
  else()
    FetchContent_MakeAvailable(${dep_name})
  endif()
endmacro()

cmake_language(SET_DEPENDENCY_PROVIDER _fetchdeps_provider
  SUPPORTED_METHODS FETCHCONTENT_MAKEAVAILABLE_SERIAL)

# ---------------------------------------------------------------------------
# Root-entry fetch
# ---------------------------------------------------------------------------

function(_fetchdeps_fetch_one entry source_root)
  string(JSON name  GET "${entry}" name)
  string(JSON dtype GET "${entry}" type)

  # Mark before any work so even the local-override path counts as declared.
  _fetchdeps_mark_declared("${name}")

  # If a transitive FetchContent already populated this dep earlier in the
  # same configure (e.g. ncrequest's CMakeLists pulled pegtl before the root
  # loop reached the pegtl entry), don't re-declare it — FetchContent would
  # rerun find_package and collide with the existing binary dir.
  FetchContent_GetProperties(${name})
  if(${name}_POPULATED)
    return()
  endif()

  # Workspace-local override.
  if(EXISTS "${source_root}/${name}")
    message(STATUS "fetchdeps: using local ${source_root}/${name}")
    add_subdirectory("${source_root}/${name}" "${name}")
    return()
  endif()

  # Stash dest + exclude-from-all so the provider can reach them.
  _fetchdeps_json_get_opt(dest "${entry}" dest)
  if(dest)
    set(_FETCHDEPS_DEST_${name} "${dest}" CACHE INTERNAL "" FORCE)
  endif()

  set(declare_args "")
  set(exclude_from_all FALSE)

  if(dtype STREQUAL "git")
    _fetchdeps_json_get_opt(url    "${entry}" url)
    _fetchdeps_json_get_opt(commit "${entry}" commit)
    _fetchdeps_json_get_opt(tag    "${entry}" tag)
    _fetchdeps_json_get_opt(branch "${entry}" branch)
    _fetchdeps_json_get_opt(dshallow "${entry}" "disable-shallow-clone")

    if(NOT url)
      message(FATAL_ERROR "fetchdeps: '${name}' type=git requires 'url'")
    endif()
    list(APPEND declare_args GIT_REPOSITORY "${url}")

    if(commit)
      list(APPEND declare_args GIT_TAG "${commit}")
    elseif(tag)
      list(APPEND declare_args GIT_TAG "${tag}")
    elseif(branch)
      list(APPEND declare_args GIT_TAG "${branch}")
    else()
      message(FATAL_ERROR
        "fetchdeps: '${name}' type=git requires one of commit/tag/branch")
    endif()

    if(dshallow STREQUAL "true")
      list(APPEND declare_args GIT_SHALLOW FALSE)
    else()
      list(APPEND declare_args GIT_SHALLOW TRUE)
    endif()

  elseif(dtype STREQUAL "archive" OR dtype STREQUAL "file")
    _fetchdeps_json_get_opt(url           "${entry}" url)
    _fetchdeps_json_get_opt(dest_filename "${entry}" "dest-filename")
    if(NOT url)
      message(FATAL_ERROR "fetchdeps: '${name}' type=${dtype} requires 'url'")
    endif()
    list(APPEND declare_args URL "${url}")
    if(dest_filename)
      list(APPEND declare_args DOWNLOAD_NAME "${dest_filename}")
    endif()
    if(dtype STREQUAL "file")
      list(APPEND declare_args DOWNLOAD_NO_EXTRACT TRUE)
    endif()

    set(_hash "")
    foreach(algo sha512 sha256 sha1 md5)
      _fetchdeps_json_get_opt(_v "${entry}" "${algo}")
      if(_v)
        set(_hash "${algo}=${_v}")
        break()
      endif()
    endforeach()
    if(NOT _hash)
      message(FATAL_ERROR
        "fetchdeps: '${name}' type=${dtype} requires sha512/sha256/sha1/md5")
    endif()
    list(APPEND declare_args URL_HASH "${_hash}")

  else()
    message(FATAL_ERROR "fetchdeps: '${name}' unsupported type '${dtype}'")
  endif()

  # x-cmake sidecar.
  _fetchdeps_json_has_key(has_xc "${entry}" "x-cmake")
  if(has_xc)
    string(JSON xc GET "${entry}" "x-cmake")

    _fetchdeps_json_get_opt(v "${xc}" exclude_from_all)
    if(v STREQUAL "true")
      set(exclude_from_all TRUE)
      list(APPEND declare_args EXCLUDE_FROM_ALL)
    endif()

    _fetchdeps_json_get_opt(v "${xc}" find_package_args)
    if(v)
      separate_arguments(_fpa UNIX_COMMAND "${v}")
      list(APPEND declare_args FIND_PACKAGE_ARGS ${_fpa})
    endif()

    _fetchdeps_json_get_opt(v "${xc}" source_subdir)
    if(v)
      list(APPEND declare_args SOURCE_SUBDIR "${v}")
    endif()

    _fetchdeps_json_has_key(has_sub "${xc}" git_submodules)
    if(has_sub)
      string(JSON sub_len LENGTH "${xc}" git_submodules)
      if(sub_len GREATER 0)
        set(_subs "")
        math(EXPR _last "${sub_len} - 1")
        foreach(i RANGE 0 ${_last})
          string(JSON _s GET "${xc}" git_submodules ${i})
          list(APPEND _subs "${_s}")
        endforeach()
        list(APPEND declare_args GIT_SUBMODULES ${_subs})
      endif()
    endif()
  endif()

  set(_FETCHDEPS_EXCLUDE_${name} "${exclude_from_all}" CACHE INTERNAL "" FORCE)

  FetchContent_Declare(${name} ${declare_args})
  FetchContent_MakeAvailable(${name})
endfunction()

# ---------------------------------------------------------------------------
# Flatpak manifest emit
# ---------------------------------------------------------------------------

function(_fetchdeps_emit_flatpak deps_json out_path)
  string(JSON n LENGTH "${deps_json}")
  if(n EQUAL 0)
    file(WRITE "${out_path}" "[]\n")
    return()
  endif()
  set(out "[]")
  math(EXPR _last "${n} - 1")
  foreach(i RANGE 0 ${_last})
    string(JSON entry GET "${deps_json}" ${i})
    _fetchdeps_json_has_key(has_xc "${entry}" "x-cmake")
    if(has_xc)
      string(JSON entry REMOVE "${entry}" "x-cmake")
    endif()
    string(JSON entry REMOVE "${entry}" name)
    string(JSON out SET "${out}" ${i} "${entry}")
  endforeach()
  file(WRITE "${out_path}" "${out}\n")
endfunction()

# ---------------------------------------------------------------------------
# Public entry point
# ---------------------------------------------------------------------------

function(fetchdeps deps_path)
  set(options "")
  set(oneValueArgs FLATPAK_OUT)
  set(multiValueArgs "")
  cmake_parse_arguments(FD "${options}" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

  if(NOT EXISTS "${deps_path}")
    message(FATAL_ERROR "fetchdeps: ${deps_path} not found")
  endif()

  file(READ "${deps_path}" deps_json)
  string(JSON n ERROR_VARIABLE _err LENGTH "${deps_json}")
  if(_err)
    message(FATAL_ERROR "fetchdeps: ${deps_path} is not valid JSON: ${_err}")
  endif()

  # Expose the authoritative deps.json path to the provider so auto-record
  # can write new transitive deps back into it.
  set_property(GLOBAL PROPERTY _FETCHDEPS_JSON_PATH "${deps_path}")

  get_filename_component(source_root "${deps_path}" DIRECTORY)

  if(n GREATER 0)
    math(EXPR _last "${n} - 1")
    # Pre-pass: mark every root entry as declared so transitive provider
    # calls that land on a name already in deps.json (possibly later in the
    # array) don't misfire autorecord during the main loop below.
    foreach(i RANGE 0 ${_last})
      string(JSON entry GET "${deps_json}" ${i})
      string(JSON _pre_name GET "${entry}" name)
      _fetchdeps_mark_declared("${_pre_name}")
    endforeach()
    foreach(i RANGE 0 ${_last})
      string(JSON entry GET "${deps_json}" ${i})
      _fetchdeps_fetch_one("${entry}" "${source_root}")
    endforeach()
  endif()

  if(FD_FLATPAK_OUT OR DEFINED ENV{FLATPAK_ID})
    set(_out "${FD_FLATPAK_OUT}")
    if(NOT _out)
      set(_out "${CMAKE_BINARY_DIR}/flatpak_sources.json")
    endif()
    # Re-read — transitive deps may have appended entries during fetching.
    file(READ "${deps_path}" deps_json)
    _fetchdeps_emit_flatpak("${deps_json}" "${_out}")
    message(STATUS "fetchdeps: wrote flatpak sources -> ${_out}")
  endif()
endfunction()
