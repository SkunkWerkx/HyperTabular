#!/usr/bin/env bash
# Runs a binding's test suite, cross-compiled for Android, on the device or emulator adb is
# attached to: pushes a prepared directory (the test executable, any shared libraries it
# needs, and the conformance corpus) to /data/local/tmp, runs a command inside it, and exits
# with that command's status.
#
#   android_device_test.sh <staged-dir> <command...>
#
# The command runs in the pushed directory with LD_LIBRARY_PATH and TMPDIR pointing at it,
# HYPERTABULAR_CORPUS at its corpus/ subdirectory and HYPERTABULAR_CRATE_VERSION set to the
# crate's version (read here from rust/Cargo.toml, since the device has no source tree), e.g.
#
#   android_device_test.sh "$stage" ./HyperTabularPackageTests.xctest
#   android_device_test.sh "$stage" ./hypertabular.test -test.v -test.skip '^ExampleWorkbook$'
#
# TMPDIR because both suites write the files their file readers read, and this way those go
# where the run's own cleanup removes them. Go's ExampleWorkbook is skipped, as it is in the
# iOS simulator: it opens ../corpus/workbook/basic.xlsx by its path, as the documentation
# shows it, and on the device nothing is above the pushed directory. The suites' own corpus
# lookups find corpus/ inside it.
#
# Used by ci.yml's test-android job for Go and Swift, and runnable by hand against a local
# emulator. Only /data/local/tmp is writable and executable for the shell user there.
set -euo pipefail

stage="${1:?usage: android_device_test.sh <staged-dir> <command...>}"
shift
[ $# -gt 0 ] || { echo "usage: android_device_test.sh <staged-dir> <command...>" >&2; exit 2; }

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo/rust/Cargo.toml" | head -1)"
remote="/data/local/tmp/hypertabular-test"
adb shell "rm -rf $remote && mkdir -p $remote"
adb push "$stage/." "$remote/" >/dev/null
echo "device: API $(adb shell getprop ro.build.version.sdk | tr -d '\r'), $(adb shell getprop ro.product.cpu.abi | tr -d '\r'), page size $(adb shell getconf PAGE_SIZE | tr -d '\r')"

# Quoted once for the device's shell; adb shell has returned the remote exit status since
# platform-tools 24.
command=""
for word in "$@"; do command+=" '${word//\'/\'\\\'\'}'"; done
status=0
adb shell "cd $remote && chmod +x '$1' && LD_LIBRARY_PATH=$remote TMPDIR=$remote HYPERTABULAR_CORPUS=$remote/corpus HYPERTABULAR_CRATE_VERSION=$version$command" || status=$?
adb shell "rm -rf $remote" || true
exit "$status"
