#!/usr/bin/env bash
# Proves, on the code and on the library it becomes, the three things src/kernel says of
# itself: no standard library, no allocation, no panic. Runs unchanged on a developer's
# machine (Linux) and is what CI runs. Each check is one a mistake cannot get past
# quietly: the list of what may be imported is an allow-list, the list of exports is
# compared whole, and the no-panic proof is shown a function it has to reject.
#
#   rust/check-core.sh
set -euo pipefail
cd "$(dirname "$0")"

kernel=src/kernel
fail() { printf 'check-core: FAILED: %s\n' "$*" >&2; exit 1; }
step() { printf '\n== %s\n' "$*"; }

step "the source names nothing the core may not have"
# No path into std or alloc, and no extern crate at all, anywhere under the kernel. The
# no_std build below would refuse `std::` anyway; `alloc::` is the one it would not, if
# the crate ever declared the alloc crate — so nothing in the crate may.
if grep -rnE '\bstd::|\balloc::|extern[[:space:]]+crate' "$kernel"; then
  fail "the lines above name std, alloc or an extern crate"
fi
if grep -rnE 'extern[[:space:]]+crate[[:space:]]+alloc' src; then
  fail "the crate declares the alloc crate"
fi
# The same code in every build: no feature decides anything in the kernel, apart from the
# two in the one macro every export is declared with.
# (`target_feature`, which asks about the CPU being compiled for, is not a Cargo feature.)
if grep -rnE '(^|[^_])feature[[:space:]]*=' "$kernel" | grep -v "^$kernel/exports.rs:"; then
  fail "the lines above make the core depend on a feature"
fi
[ "$(grep -c 'cfg_attr(feature = ' "$kernel/exports.rs")" -eq 2 ] \
  || fail "exports.rs should hold exactly the macro's two feature attributes"
# One way to make a symbol, in the whole crate and not only in the kernel: the layers above
# the core are Rust API, and a build with every feature on exports what the core-only build
# does. (The two extension modules, php_ext and python_ext, register with their host through
# their framework's macro; neither declares a C symbol of its own.)
[ "$(grep -rnE 'unsafe\(no_mangle\)' src | wc -l)" -eq 1 ] \
  || fail "no_mangle belongs in the export! macro and nowhere else"
grep -qE 'unsafe\(no_mangle\)' "$kernel/exports.rs" \
  || fail "the one no_mangle is not the export! macro's"
if grep -rnE '#\[no_mangle\]|export_name|#\[unsafe\(export_name' src; then
  fail "the lines above make a symbol outside the export! macro"
fi
echo "ok"

step "it compiles where there is no standard library, on both architectures"
cargo check --quiet --lib --no-default-features --target thumbv7em-none-eabi
cargo check --quiet --lib --no-default-features --target x86_64-unknown-linux-gnu
cargo check --quiet --lib --no-default-features --target aarch64-unknown-linux-gnu
echo "ok"

step "it depends on HyperCast and nothing else"
deps="$(cargo tree --no-default-features -e normal --prefix none | sed 's/ .*//' | sort -u | tr '\n' ' ')"
[ "$deps" = "hypercast hypertabular " ] || fail "the core's dependencies are: $deps"
echo "ok: $deps"

step "the shared library exports what exports.rs declares, and imports no allocator"
cargo cdylib --quiet
lib=target/release/libhypertabular.so
[ -f "$lib" ] || fail "$lib was not produced"
declared="$(sed -n 's/^    fn \(hypertabular_[a-z0-9_]*\)(.*/\1/p' "$kernel/exports.rs" | grep -vx 'hypertabular_canary' | sort)"
exported="$(nm -D --defined-only "$lib" | awk '{print $3}' | sort)"
[ -n "$declared" ] || fail "found no declarations in exports.rs"
[ "$declared" = "$exported" ] || fail "exports differ. declared: $(echo $declared) / exported: $(echo $exported)"
# Everything the library asks of the process it is loaded into. Memory comes from the
# caller, so there is no allocator here to ask for it; a new name on this list is a new
# dependency on the host and gets added on purpose or not at all.
for symbol in $(nm -D --undefined-only "$lib" | awk '{print $NF}' | sed 's/@.*//' | sort -u); do
  case "$symbol" in
    memcpy|memmove|memset|memcmp|bcmp|abort) ;;
    # arm64 Linux: the one question the core asks the system, once — which CPU this is.
    getauxval) ;;
    __cxa_finalize|__gmon_start__|_ITM_deregisterTMCloneTable|_ITM_registerTMCloneTable) ;;
    *) fail "the library imports $symbol" ;;
  esac
done
needed="$(readelf -d "$lib" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' | tr '\n' ' ')"
case "$needed" in
  "libc.so.6 "|"libc.musl-x86_64.so.1 "|"libc.musl-aarch64.so.1 ") ;;
  *) fail "the library needs: $needed" ;;
esac
echo "ok: $(echo "$exported" | wc -l) exports, $(stat -c %s "$lib") bytes, needs $needed"

step "the static library needs nothing from Rust"
cargo staticlib --quiet
archive=target/staticlib/libhypertabular.a
[ -f "$archive" ] || fail "$archive was not produced"
host="$(rustc -vV | sed -n 's/^host: //p')"
tools="$(rustc --print sysroot)/lib/rustlib/$host/bin"
[ -x "$tools/llvm-nm" ] || fail "llvm-nm not found (rustup component add llvm-tools)"
object="$("$tools/llvm-ar" t "$archive" | grep '^hypertabular-')"
[ "$(echo "$object" | wc -l)" -eq 1 ] || fail "expected one hypertabular object, found: $object"
work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT
(cd "$work" && "$tools/llvm-ar" x "$OLDPWD/$archive" "$object")
# What whoever links the archive supplies: the C library's memory routines, and the
# compiler's own arithmetic helpers (128-bit division and the like), which gcc and clang
# link from libgcc or compiler-rt on their own. Nothing with a Rust name, and no allocator.
for symbol in $("$tools/llvm-nm" --undefined-only "$work/$object" | awk '{print $NF}' | sort -u); do
  case "$symbol" in
    memcpy|memmove|memset|memcmp|bcmp|abort|getauxval) ;;
    __divti3|__udivti3|__modti3|__umodti3|__multi3|__muloti4) ;;
    # arm64: the compiler's out-of-line atomics, for the one cached byte.
    __aarch64_*) ;;
    *) fail "the crate's object leaves $symbol undefined" ;;
  esac
done
if "$tools/llvm-nm" --defined-only --extern-only "$work/$object" | awk '{print $3}' | grep -v '^hypertabular_'; then
  fail "the crate's object defines the symbols above beside its exports"
fi
echo "ok: one object, $(stat -c %s "$work/$object") bytes"

step "no export can panic"
if ! cargo no-panic >"$work/no-panic.log" 2>&1; then
  grep -o 'detected panic in function `[a-z0-9_]*`' "$work/no-panic.log" | sort -u || tail -20 "$work/no-panic.log"
  fail "the exports above have a path that can panic"
fi
echo "ok"

step "and the proof rejects one that can"
log="$work/canary.log"
if RUSTFLAGS="--cfg hypertabular_canary" cargo rustc --quiet --release --lib --crate-type cdylib \
     --features no-panic --target-dir target/no-panic-canary -- -C link-arg=-Wl,--no-undefined \
     >"$log" 2>&1; then
  fail "a library with an export that panics passed the proof"
fi
grep -q 'detected panic in function `hypertabular_canary`' "$log" \
  || fail "the canary build failed for some other reason: $(tail -5 "$log")"
if grep -o 'detected panic in function `[a-z0-9_]*`' "$log" | grep -v hypertabular_canary; then
  fail "the proof rejected a real export alongside the canary"
fi
echo "ok: rejected, by name"

step "the core's own tests"
cargo test --quiet --test kernel_delimited --test kernel_allocation_free --test kernel_inflate \
  --test kernel_workbook --test kernel_workbook_allocation_free --test kernel_workbook_rules

printf '\ncheck-core: all of it holds.\n'
