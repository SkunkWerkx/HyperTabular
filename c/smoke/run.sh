#!/usr/bin/env bash
# Builds the smoke consumers against an unpacked C release archive, every way a consumer would
# on this platform, runs each program, and holds the version each reports to the archive's:
#
#   CMake       find_package(hypertabular), hypertabular::hypertabular and
#               hypertabular::hypertabular_static: main.c as each C standard the compiler
#               has, main.cpp through hypertabular.hpp as C++17, C++20 and C++23 with
#               exceptions on and off, and corpus.cpp, which replays the conformance corpus
#               (CMakeLists.txt has the list; SMOKE_CXX=1 makes the C++ programs required)
#   pkg-config  `pkg-config --cflags --libs hypertabular`, and the archive in ${libdir} named
#               directly, from C, and from C++ beside hypercast (not on Windows, where CMake
#               is the way in)
#
# The C++ programs build against HyperCast's archive for the same platform too, the release
# rust/Cargo.toml requires (hypercast.sh says where it comes from), and link its library of
# the same kind: a program with both static archives, or both shared libraries, is what a C++
# consumer has.
#
# A program linked to the shared library must record the library's own name (@rpath/ on
# macOS, the soname elsewhere), never the path it was linked from, and one linked statically
# must not need the shared library at all. The WebAssembly archive has no shared library and
# no runner to execute it, so it is not smoke-tested here.
#
# usage: c/smoke/run.sh <unpacked archive> [<expected version>]
#        SMOKE_ARCH=x86_64 c/smoke/run.sh ...   on Apple silicon, osx-x64 under Rosetta
#        SMOKE_CORPUS=<dir>                     the corpus to read; the checkout's by default
#        HYPERCAST_PACKAGES=<dir>               unpacked HyperCast archives (hypercast.sh)
set -euo pipefail
pkg="$(cd "$1" && pwd)"
expected="${2:-}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
corpus="$(cd "${SMOKE_CORPUS:-$here/../../corpus}" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
# shellcheck source=/dev/null # hypercast.sh, beside this script, linted on its own
. "$here/hypercast.sh"

case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) os=windows ;;
  Darwin) os=macos ;;
  *) os=linux ;;
esac

fail() { echo "smoke: $*" >&2; exit 1; }

# macOS only: build for SMOKE_ARCH and run under it, so osx-x64 is tested on an arm64 runner.
arch_flags=() cmake_arch=() run_as=()
if [ -n "${SMOKE_ARCH:-}" ]; then
  [ "$os" = macos ] || fail "SMOKE_ARCH is for macOS only"
  arch_flags=(-arch "$SMOKE_ARCH") cmake_arch=(-DCMAKE_OSX_ARCHITECTURES="$SMOKE_ARCH") run_as=(arch "-$SMOKE_ARCH")
fi

# One version for the archive: the CMake package's, pkg-config's and the library's must agree,
# and with the caller's when one is given.
check_version() {
  local how="$1" reported="$2"
  echo "  $how: $reported"
  if [ -z "$expected" ]; then
    expected="$reported"
  elif [ "$reported" != "$expected" ]; then
    fail "$how reports $reported, expected $expected"
  fi
}

# What a program asks the dynamic loader for, one name per line. Its readers below use
# `grep >/dev/null`, never `grep -q`: -q exits at the first match, and under pipefail the
# writer's SIGPIPE then fails the pipeline (musl's readelf, on a program that matched).
needed() {
  case "$os" in
    macos) otool -L "$1" | tail -n +2 | awk '{print $1}' ;;
    linux) readelf -d "$1" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' ;;
  esac
}

check_shared_link() {
  [ "$os" = windows ] && return 0
  local want=libhypertabular.so
  [ "$os" = macos ] && want=@rpath/libhypertabular.dylib
  needed "$1" | grep -x "$want" >/dev/null || fail "$1 does not load $want: $(needed "$1" | tr '\n' ' ')"
}

check_static_link() {
  [ "$os" = windows ] && return 0
  if needed "$1" | grep -E 'hypertabular|hypercast' >/dev/null; then
    fail "$1, linked statically, still loads $(needed "$1" | grep -E 'hypertabular|hypercast')"
  fi
}

# Runs a smoke program, after checking what it links, with the corpus directory, and holds its
# version to the archive's.
run_program() {
  local how="$1" program="$2" linkage="$3" output
  if [ "$linkage" = static ]; then check_static_link "$program"; else check_shared_link "$program"; fi
  output="$("${run_as[@]}" "$program" "$corpus")" || fail "$program ($how) failed"
  check_version "hypertabular_version() ($how)" "$(printf '%s' "$output" | tr -d '\r')"
}

cmake_build() {
  local name="$1" static="$2" build="$work/cmake-$1"
  cmake -S "$here" -B "$build" -DCMAKE_PREFIX_PATH="$pkg;$hypercast" -DHYPERTABULAR_SMOKE_STATIC="$static" \
    "${cmake_arch[@]}" -DHYPERTABULAR_SMOKE_HYPERCAST="$hypercast_wanted" \
    -DHYPERTABULAR_SMOKE_CXX="$([ "${SMOKE_CXX:-}" = 1 ] && echo ON || echo OFF)" \
    -DCMAKE_BUILD_TYPE=Release >"$work/$name.log" 2>&1 || { cat "$work/$name.log"; fail "cmake configure ($name)"; }
  cmake --build "$build" --config Release >>"$work/$name.log" 2>&1 || { cat "$work/$name.log"; fail "cmake build ($name)"; }
  sed -n 's/^-- smoke: /  (/p' "$work/$name.log" | sed 's/$/)/'
  check_version "CMake package ($name)" "$(cat "$build/package-version.txt")"
  local program path linkage
  linkage="$([ "$static" = ON ] && echo static || echo shared)"
  # CMake writes its text files with CRLF on Windows: every read strips the CR.
  while read -r program; do
    program=${program%$'\r'}
    [ -n "$program" ] || continue
    path="$(tr -d '\r' <"$build/$program-path-Release.txt")"
    [ -f "$path" ] || fail "no $program at $path"
    run_program "$program, $name" "$path" "$linkage"
  done <"$build/programs.txt"
  tr -d '\r' <"$build/programs.txt" | grep -x smoke_c99 >/dev/null || fail "smoke_c99 was not built ($name)"
}

echo "smoke: $pkg"
# Every function the archive's header declares is called by main.c and by hypertabular.hpp,
# so an export added to the core without either fails here.
functions="$(sed -n -E 's/^[a-z0-9_]+ ([a-z0-9_]+)\(.*\);$/\1/p' "$pkg/include/hypertabular.h")"
[ -n "$functions" ] || fail "no functions found in $pkg/include/hypertabular.h"
for function in $functions; do
  grep -qE "(^|[^a-z0-9_])$function\(" "$here/main.c" || fail "main.c never calls $function"
  grep -qE "::$function([^a-z0-9_]|\$)" "$pkg/include/hypertabular.hpp" || fail "hypertabular.hpp never calls $function"
done
echo "  every one of the $(echo "$functions" | wc -l | tr -d ' ') functions hypertabular.h declares is in main.c and hypertabular.hpp"

# HyperCast's archive for the C++ programs; without it they are skipped, unless SMOKE_CXX says
# they are required.
hypercast_wanted="$(hypercast_version "$here/../..")"
[ -n "$hypercast_wanted" ] || fail "no hypercast requirement in rust/Cargo.toml"
hypercast=""
if ! hypercast="$(hypercast_archive "$(archive_rid "$pkg")" "$hypercast_wanted" "$work")"; then
  [ "${SMOKE_CXX:-}" = 1 ] && fail "SMOKE_CXX is set, but there is no HyperCast $hypercast_wanted archive"
  echo "  (no HyperCast $hypercast_wanted archive: the C++ programs are not built)"
  hypercast=""
fi
[ -z "$hypercast" ] || echo "  HyperCast: $hypercast"

cmake_build shared OFF
cmake_build static ON

if [ "$os" != windows ]; then
  export PKG_CONFIG_PATH="$pkg/lib/pkgconfig${hypercast:+:$hypercast/lib/pkgconfig}"
  check_version "pkg-config --modversion" "$(pkg-config --modversion hypertabular)"
  libdir="$(pkg-config --variable=libdir hypertabular)"
  # shellcheck disable=SC2046 # pkg-config's flags are meant to split
  cc "${arch_flags[@]}" -o "$work/pc-shared" "$here/main.c" $(pkg-config --cflags --libs hypertabular) -Wl,-rpath,"$libdir"
  run_program "pkg-config, shared" "$work/pc-shared" shared
  # shellcheck disable=SC2046
  cc "${arch_flags[@]}" -o "$work/pc-static" "$here/main.c" $(pkg-config --cflags hypertabular) "$libdir/libhypertabular.a"
  run_program "pkg-config, static" "$work/pc-static" static
  if command -v c++ >/dev/null && [ -n "$hypercast" ]; then
    hypercast_libdir="$(pkg-config --variable=libdir hypercast)"
    # shellcheck disable=SC2046
    c++ "${arch_flags[@]}" -std=c++17 -o "$work/pc-cxx-shared" "$here/main.cpp" \
      $(pkg-config --cflags --libs hypertabular hypercast) -Wl,-rpath,"$libdir" -Wl,-rpath,"$hypercast_libdir"
    run_program "pkg-config, C++, shared" "$work/pc-cxx-shared" shared
    # shellcheck disable=SC2046
    c++ "${arch_flags[@]}" -std=c++17 -o "$work/pc-cxx-static" "$here/main.cpp" \
      $(pkg-config --cflags hypertabular hypercast) "$libdir/libhypertabular.a" "$hypercast_libdir/libhypercast.a"
    run_program "pkg-config, C++, static" "$work/pc-cxx-static" static
  elif [ "${SMOKE_CXX:-}" = 1 ]; then
    fail "SMOKE_CXX is set, but there is no c++ here"
  fi
fi

echo "smoke: every build of $pkg reports $expected"
