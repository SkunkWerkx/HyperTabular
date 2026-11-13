# The smoke consumers, through find_package: run.sh points CMAKE_PREFIX_PATH at an unpacked
# release archive, and at HyperCast's for the same platform, and builds them once against each
# library the archives have.
#
#   smoke_c<std>                 main.c as C99, C11 and C23, each where the compiler has it,
#                                against hypertabular alone
#   smoke_cxx<std>               main.cpp through hypertabular.hpp as C++17, C++20 and C++23
#   smoke_cxx<std>_noexceptions  the same with exceptions off (-fno-exceptions; /EHs-c- and
#                                _HAS_EXCEPTIONS=0 on MSVC)
#   smoke_corpus                 corpus.cpp: the conformance corpus replayed through the header
#   smoke_aborts_<way>           aborts.cpp, each passing only by the header's std::abort() of a
#                                caller bug: a faults or row walk past the next read(), a reader
#                                destroyed or moved by its own source, a sheet destroyed by its own
#                                for_each_row callback (aborts.cpp lists the ways)
#
# The C++ programs link HyperCast's library of the same kind beside this one (hypercast.hpp is
# the header the C++ one stands on, and NumFormat::declare asks HyperCast's core), so they are
# built only where find_package(hypercast) finds HyperCast's archive; HYPERTABULAR_SMOKE_CXX
# makes that, and a C++ compiler, required.
#
# Every one with warnings as errors (-Wall -Wextra -Wpedantic -Wshadow -Werror; /W4 /WX on MSVC),
# which reach the headers too: an imported target's includes are system headers, whose warnings a
# compiler keeps to itself, so NO_SYSTEM_FROM_IMPORTED takes that away.
# programs.txt lists what was built, and <program>-path-<config>.txt where each landed (inside
# an .app on iOS; under the configuration's folder in Visual Studio).
cmake_minimum_required(VERSION 3.21)
project(hypertabular_smoke C)

option(HYPERTABULAR_SMOKE_STATIC "Link the _static targets instead of the shared ones" OFF)
option(HYPERTABULAR_SMOKE_CXX "Require the C++ smoke programs" OFF)
set(HYPERTABULAR_SMOKE_HYPERCAST "" CACHE STRING "The HyperCast version the C++ programs ask find_package for")

find_package(hypertabular REQUIRED CONFIG)

if(MSVC)
  set(_warnings /W4 /WX)
else()
  set(_warnings -Wall -Wextra -Wpedantic -Wshadow -Werror)
endif()

set(_programs)
set(_cxx_programs)

foreach(standard 99 11 23)
  if(DEFINED CMAKE_C${standard}_STANDARD_COMPILE_OPTION)
    add_executable(smoke_c${standard} main.c)
    set_target_properties(smoke_c${standard} PROPERTIES C_STANDARD ${standard} C_STANDARD_REQUIRED ON
      C_EXTENSIONS OFF)
    if(MSVC)
      # fopen and sprintf are what a portable C program has.
      target_compile_definitions(smoke_c${standard} PRIVATE _CRT_SECURE_NO_WARNINGS)
    endif()
    list(APPEND _programs smoke_c${standard})
  else()
    message(STATUS "smoke: no C${standard} here, smoke_c${standard} not built")
  endif()
endforeach()

include(CheckLanguage)
check_language(CXX)
if(CMAKE_CXX_COMPILER)
  find_package(hypercast ${HYPERTABULAR_SMOKE_HYPERCAST} CONFIG)
endif()
if(CMAKE_CXX_COMPILER AND hypercast_FOUND)
  enable_language(CXX)
  if(MSVC)
    # Exception handling is chosen per program below, so the default /EHsc goes.
    string(REPLACE "/EHsc" "" CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS}")
  endif()
  foreach(standard 17 20 23)
    if(NOT DEFINED CMAKE_CXX${standard}_STANDARD_COMPILE_OPTION)
      if(HYPERTABULAR_SMOKE_CXX)
        message(FATAL_ERROR "HYPERTABULAR_SMOKE_CXX is on, but the C++ compiler here has no C++${standard}")
      endif()
      message(STATUS "smoke: no C++${standard} here, smoke_cxx${standard} not built")
      continue()
    endif()
    foreach(variant "" _noexceptions)
      set(program smoke_cxx${standard}${variant})
      add_executable(${program} main.cpp)
      set_target_properties(${program} PROPERTIES CXX_STANDARD ${standard} CXX_STANDARD_REQUIRED ON
        CXX_EXTENSIONS OFF)
      if(variant STREQUAL "")
        target_compile_options(${program} PRIVATE $<$<CXX_COMPILER_ID:MSVC>:/EHsc>)
      elseif(MSVC)
        target_compile_options(${program} PRIVATE /EHs-c-)
        target_compile_definitions(${program} PRIVATE _HAS_EXCEPTIONS=0)
      else()
        target_compile_options(${program} PRIVATE -fno-exceptions)
      endif()
      list(APPEND _programs ${program})
      list(APPEND _cxx_programs ${program})
    endforeach()
  endforeach()
  add_executable(smoke_corpus corpus.cpp)
  set_target_properties(smoke_corpus PROPERTIES CXX_STANDARD 17 CXX_STANDARD_REQUIRED ON CXX_EXTENSIONS OFF)
  target_compile_options(smoke_corpus PRIVATE $<$<CXX_COMPILER_ID:MSVC>:/EHsc>)
  list(APPEND _programs smoke_corpus)
  list(APPEND _cxx_programs smoke_corpus)
  foreach(way faults_begin faults_advance rows_deref rows_advance rows_postfix destroyed moved_from
          moved_onto sheet_destroyed checked_get checked_text)
    set(program smoke_aborts_${way})
    add_executable(${program} aborts.cpp)
    set_target_properties(${program} PROPERTIES CXX_STANDARD 17 CXX_STANDARD_REQUIRED ON CXX_EXTENSIONS OFF)
    target_compile_options(${program} PRIVATE $<$<CXX_COMPILER_ID:MSVC>:/EHsc>)
    target_compile_definitions(${program} PRIVATE SMOKE_WAY=${way})
    list(APPEND _programs ${program})
    list(APPEND _cxx_programs ${program})
  endforeach()
elseif(HYPERTABULAR_SMOKE_CXX)
  if(NOT CMAKE_CXX_COMPILER)
    message(FATAL_ERROR "HYPERTABULAR_SMOKE_CXX is on, but there is no C++ compiler here")
  endif()
  message(FATAL_ERROR "HYPERTABULAR_SMOKE_CXX is on, but find_package found no HyperCast ${HYPERTABULAR_SMOKE_HYPERCAST} archive")
else()
  message(STATUS "smoke: no C++ compiler or no HyperCast archive here, the C++ programs not built")
endif()

foreach(program IN LISTS _programs)
  target_compile_options(${program} PRIVATE ${_warnings})
  set_target_properties(${program} PROPERTIES NO_SYSTEM_FROM_IMPORTED ON)
  if(HYPERTABULAR_SMOKE_STATIC)
    target_link_libraries(${program} PRIVATE hypertabular::hypertabular_static)
    if(program IN_LIST _cxx_programs)
      target_link_libraries(${program} PRIVATE hypercast::hypercast_static)
    endif()
  else()
    target_link_libraries(${program} PRIVATE hypertabular::hypertabular)
    if(program IN_LIST _cxx_programs)
      target_link_libraries(${program} PRIVATE hypercast::hypercast)
    endif()
    # Windows has no rpath: the DLLs go beside the program.
    if(WIN32)
      add_custom_command(TARGET ${program} POST_BUILD
        COMMAND ${CMAKE_COMMAND} -E copy_if_different $<TARGET_RUNTIME_DLLS:${program}> $<TARGET_FILE_DIR:${program}>
        COMMAND_EXPAND_LISTS)
    endif()
  endif()
  file(GENERATE OUTPUT "${CMAKE_BINARY_DIR}/${program}-path-$<CONFIG>.txt" CONTENT "$<TARGET_FILE:${program}>")
endforeach()

list(JOIN _programs "\n" _program_lines)
file(WRITE "${CMAKE_BINARY_DIR}/programs.txt" "${_program_lines}\n")

# What find_package resolved, for run.sh to hold against what the library reports.
file(WRITE "${CMAKE_BINARY_DIR}/package-version.txt" "${hypertabular_VERSION}")
