# FetchDepsProvider.cmake
#
# Opt-in FetchContent dependency provider paired with FetchDeps.cmake.
# Load via `CMAKE_PROJECT_TOP_LEVEL_INCLUDES` *before* project() — this is
# the only context where cmake_language(SET_DEPENDENCY_PROVIDER ...) works.
#
#     list(APPEND CMAKE_PROJECT_TOP_LEVEL_INCLUDES
#          ${CMAKE_CURRENT_SOURCE_DIR}/cmake/FetchDepsProvider.cmake)
#     project(myapp)
#
# Or on the command line (e.g. flatpak config-opts):
#
#     -DCMAKE_PROJECT_TOP_LEVEL_INCLUDES=cmake/FetchDepsProvider.cmake
#
# With the provider active:
# * Transitive FetchContent_MakeAvailable calls for deps not in deps.json
#   are auto-recorded and appended to deps.json on disk.
# * Inside flatpak-builder ($FLATPAK_ID set), declared deps are redirected
#   to "$FLATPAK_BUILDER_BUILDDIR/<dest>" — no network required.
#
# Without the provider loaded, FetchDeps.cmake still works for regular
# (online) FetchContent builds; only the flatpak redirect and auto-record
# behavior are disabled.

include_guard(GLOBAL)
include(${CMAKE_CURRENT_LIST_DIR}/FetchDeps.cmake)

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

  set(_xc "{}")
  string(JSON _xc SET "${_xc}" name "\"${name}\"")
  if(FD_EXCLUDE_FROM_ALL)
    string(JSON _xc SET "${_xc}" exclude_from_all "true")
  endif()
  if(FD_SOURCE_SUBDIR)
    string(JSON _xc SET "${_xc}" source_subdir "\"${FD_SOURCE_SUBDIR}\"")
  endif()
  string(JSON entry SET "${entry}" "x-cmake" "${_xc}")

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
  if(FD_SOURCE_SUBDIR)
    set(_FETCHDEPS_SOURCE_SUBDIR_${name} "${FD_SOURCE_SUBDIR}" CACHE INTERNAL "" FORCE)
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
    # Honor x-cmake.source_subdir: point add_subdirectory at <src>/<subdir>.
    # When the subdir has no CMakeLists.txt (e.g. cmake-noop), this skips
    # add_subdirectory entirely — matching FetchContent_MakeAvailable's
    # behavior for SOURCE_SUBDIR.
    set(_fd_add_src "${_fd_src}")
    if(DEFINED _FETCHDEPS_SOURCE_SUBDIR_${dep_name})
      set(_fd_add_src "${_fd_src}/${_FETCHDEPS_SOURCE_SUBDIR_${dep_name}}")
    endif()
    if(EXISTS "${_fd_add_src}/CMakeLists.txt")
      set(_fd_extra "")
      if(_FETCHDEPS_EXCLUDE_${dep_name})
        list(APPEND _fd_extra EXCLUDE_FROM_ALL)
      endif()
      add_subdirectory("${_fd_add_src}" "${_fd_bin}" ${_fd_extra})
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
