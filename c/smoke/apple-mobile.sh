#!/usr/bin/env bash
# Builds the smoke consumers (CMakeLists.txt has the list: main.c as each C standard,
# main.cpp as C++17, C++20 and C++23 with exceptions on and off, the corpus replay and the stale
# faults walks)
# against the four Apple mobile C archives through find_package, linked to
# hypertabular::hypertabular_static (they carry no shared library), and the C++ programs to
# hypercast::hypercast_static from HyperCast's archive for the same platform beside it
# (hypercast.sh says where it comes from), on a Mac with Xcode:
#
#   ios-arm64           linked for an iOS device and checked to be one; no device runs it
#   iossimulator-arm64  run in the booted iOS simulator (`xcrun simctl spawn booted`)
#   maccatalyst-arm64   run as a Mac Catalyst process
#   maccatalyst-x64     run as a Mac Catalyst process under Rosetta 2
#
# Each run must print the archive's version; every program reads the checkout's corpus/, which
# the simulator and a Mac Catalyst process see at its own path. A cross build re-roots
# find_package's search paths into the SDK, so the archives are named by hypertabular_DIR and
# hypercast_DIR, the way an iOS toolchain file's user would name them.
#
# usage: c/smoke/apple-mobile.sh <directory holding the four unpacked archives> <version>
set -euo pipefail
root="$(cd "$1" && pwd)"
expected="$2"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
corpus="$(cd "$here/../../corpus" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
# shellcheck source=/dev/null # hypercast.sh, beside this script, linted on its own
. "$here/hypercast.sh"
hypercast_wanted="$(hypercast_version "$here/../..")"

fail() { echo "smoke: $*" >&2; exit 1; }

build() {
  local rid="$1"
  shift
  local pkg
  pkg="$(find "$root" -maxdepth 1 -type d -name "*-$expected-$rid" | head -n 1)"
  [ -n "$pkg" ] || fail "no $rid archive at $expected under $root"
  [ ! -e "$pkg/lib/libhypertabular.dylib" ] || fail "$rid carries a shared library"
  hypercast="$(hypercast_archive "$rid" "$hypercast_wanted" "$work")" || fail "no HyperCast $hypercast_wanted archive for $rid"
  cmake -S "$here" -B "$work/$rid" -DHYPERTABULAR_SMOKE_STATIC=ON -DHYPERTABULAR_SMOKE_CXX=ON -DCMAKE_BUILD_TYPE=Release \
    -Dhypertabular_DIR="$pkg/lib/cmake/hypertabular" -Dhypercast_DIR="$hypercast/lib/cmake/hypercast" \
    -DHYPERTABULAR_SMOKE_HYPERCAST="$hypercast_wanted" "$@" >"$work/$rid.log" 2>&1 \
    || { cat "$work/$rid.log"; fail "cmake configure ($rid)"; }
  cmake --build "$work/$rid" >>"$work/$rid.log" 2>&1 || { cat "$work/$rid.log"; fail "cmake build ($rid)"; }
  [ "$(cat "$work/$rid/package-version.txt")" = "$expected" ] \
    || fail "$rid: the CMake package is $(cat "$work/$rid/package-version.txt"), expected $expected"
  echo "$work/$rid"
}

# The platform a Mach-O was linked for, from LC_BUILD_VERSION, which otool prints as a number
# (mach-o/loader.h: 2 iOS, 6 Mac Catalyst, 7 the iOS simulator).
platform() {
  case "$(otool -l "$1" | awk '/LC_BUILD_VERSION/ { found = 1 } found && $1 == "platform" { print $2; exit }')" in
    2 | IOS) echo IOS ;;
    6 | MACCATALYST) echo MACCATALYST ;;
    7 | IOSSIMULATOR) echo IOSSIMULATOR ;;
    *) echo unknown ;;
  esac
}

# Checks every program a build made.
check() {
  local rid="$1" build="$2" want="$3" program
  shift 3
  while read -r program; do
    [ -n "$program" ] || continue
    check_program "$rid ($program)" "$(cat "$build/$program-path-Release.txt")" "$want" "$@"
  done <"$build/programs.txt"
  grep -qx smoke_corpus "$build/programs.txt" || fail "$rid: the C++ programs were not built"
}

check_program() {
  local rid="$1" program="$2" want="$3"
  shift 3
  local got output
  got="$(platform "$program")"
  [ "$got" = "$want" ] || fail "$rid: linked for platform $got, expected $want"
  if [ "$#" -gt 0 ]; then
    output="$("$@" "$program" "$corpus")" || fail "$rid: $program failed"
    output="$(printf '%s' "$output" | tr -d '\r')"
    [ "$output" = "$expected" ] || fail "$rid: the library reports $output, expected $expected"
    echo "  $rid: $output ($want)"
  else
    echo "  $rid: linked ($want)"
  fi
}

ios=(-DCMAKE_SYSTEM_NAME=iOS -DCMAKE_OSX_ARCHITECTURES=arm64 -DCMAKE_OSX_DEPLOYMENT_TARGET=15.0)
catalyst_arm64=(-DCMAKE_OSX_ARCHITECTURES=arm64 "-DCMAKE_C_FLAGS=-target arm64-apple-ios15.0-macabi"
  "-DCMAKE_CXX_FLAGS=-target arm64-apple-ios15.0-macabi")
catalyst_x64=(-DCMAKE_OSX_ARCHITECTURES=x86_64 "-DCMAKE_C_FLAGS=-target x86_64-apple-ios15.0-macabi"
  "-DCMAKE_CXX_FLAGS=-target x86_64-apple-ios15.0-macabi")

echo "smoke: Apple mobile archives under $root"
build_dir="$(build ios-arm64 "${ios[@]}" -DCMAKE_OSX_SYSROOT=iphoneos)"
check ios-arm64 "$build_dir" IOS
build_dir="$(build iossimulator-arm64 "${ios[@]}" -DCMAKE_OSX_SYSROOT=iphonesimulator)"
check iossimulator-arm64 "$build_dir" IOSSIMULATOR xcrun simctl spawn booted
build_dir="$(build maccatalyst-arm64 "${catalyst_arm64[@]}")"
check maccatalyst-arm64 "$build_dir" MACCATALYST env
build_dir="$(build maccatalyst-x64 "${catalyst_x64[@]}")"
check maccatalyst-x64 "$build_dir" MACCATALYST arch -x86_64
echo "smoke: every Apple mobile archive reports $expected"
