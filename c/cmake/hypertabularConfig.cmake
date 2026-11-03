# find_package(hypertabular) for an unpacked release archive; ships as
# lib/cmake/hypertabular/hypertabularConfig.cmake. Every path is relative to this file, so the
# archive works wherever it is unpacked, and a target is declared only for a library the
# archive carries (the WebAssembly archive has no shared library).
#
#   hypertabular::hypertabular          the shared library
#   hypertabular::hypertabular_static   the static library, which needs nothing past the C runtime
#
# Both carry include/, so hypertabular.h and the C++ header, hypertabular.hpp, come with either.
# The C++ header stands on HyperCast's, hypercast.hpp: a C++ program also finds HyperCast's
# archive (find_package(hypercast 1.1)) and links one of its targets beside this one.
#
# The version is the package's, from hypertabularConfigVersion.cmake; at run time the library
# reports its own through hypertabular_version().

get_filename_component(_hypertabular_prefix "${CMAKE_CURRENT_LIST_DIR}/../../.." ABSOLUTE)

if(WIN32)
  set(_hypertabular_shared "${_hypertabular_prefix}/bin/hypertabular.dll")
  set(_hypertabular_implib "${_hypertabular_prefix}/lib/hypertabular.dll.lib")
  set(_hypertabular_static "${_hypertabular_prefix}/lib/hypertabular.lib")
elseif(APPLE)
  set(_hypertabular_shared "${_hypertabular_prefix}/lib/libhypertabular.dylib")
  set(_hypertabular_static "${_hypertabular_prefix}/lib/libhypertabular.a")
else()
  set(_hypertabular_shared "${_hypertabular_prefix}/lib/libhypertabular.so")
  set(_hypertabular_static "${_hypertabular_prefix}/lib/libhypertabular.a")
endif()

if(NOT TARGET hypertabular::hypertabular AND EXISTS "${_hypertabular_shared}")
  add_library(hypertabular::hypertabular SHARED IMPORTED)
  set_target_properties(hypertabular::hypertabular PROPERTIES
    IMPORTED_LOCATION "${_hypertabular_shared}"
    INTERFACE_INCLUDE_DIRECTORIES "${_hypertabular_prefix}/include")
  if(WIN32)
    set_target_properties(hypertabular::hypertabular PROPERTIES IMPORTED_IMPLIB "${_hypertabular_implib}")
  elseif(APPLE)
    set_target_properties(hypertabular::hypertabular PROPERTIES IMPORTED_SONAME "@rpath/libhypertabular.dylib")
  else()
    set_target_properties(hypertabular::hypertabular PROPERTIES IMPORTED_SONAME "libhypertabular.so")
  endif()
endif()

if(NOT TARGET hypertabular::hypertabular_static AND EXISTS "${_hypertabular_static}")
  add_library(hypertabular::hypertabular_static STATIC IMPORTED)
  set_target_properties(hypertabular::hypertabular_static PROPERTIES
    IMPORTED_LOCATION "${_hypertabular_static}"
    IMPORTED_LINK_INTERFACE_LANGUAGES C
    INTERFACE_INCLUDE_DIRECTORIES "${_hypertabular_prefix}/include")
endif()

unset(_hypertabular_prefix)
unset(_hypertabular_shared)
unset(_hypertabular_implib)
unset(_hypertabular_static)
