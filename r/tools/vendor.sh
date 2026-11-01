#!/usr/bin/env bash
# Stages what the R package builds and tests from: the wrapper crate's src/rust/Cargo.lock
# derived from the core's, the vendored crates in src/rust/vendor.tar.xz (CRAN builds
# offline) with the core itself among them, from this checkout's rust/, the core's C header
# from include/ as src/include/hypertabular.h, the two corpus files with the workbook packages
# workbook.json reads under tests/testthat/corpus/, and the workbook the examples read under
# inst/extdata/.
#   (no argument)  the full run: stage, then regenerate inst/AUTHORS from the vendored crates
#   --stage        derive the lock, vendor the crates and the core, copy the header, the corpus
#                  and the example workbook
#   --verify       check the vendor tarball (with the core), the lock, the header, the corpus
#                  and the example workbook are present and are this checkout's
#
# The wrapper depends on the hypertabular crate by exact version, as any crates.io consumer
# does, so the dependency graph CRAN builds is the published one. That version is not on
# crates.io while the release that publishes it is being packed, and `cargo vendor` leaves a
# `[patch]`ed crate out of the vendor directory, so: the lock is resolved with the core patched
# in from rust/ on the command line (never in a committed file), the crates.io dependencies are
# vendored (the hypercast crate the core judges every cell with among them), the core is added
# to the vendor directory as `cargo package` writes it (the form the crates.io release of that
# version is uploaded in), and the lock is rewritten offline against the vendored sources, so it
# names the crates.io source for the core and the tarball builds with no patch and no network.
# A pin that differs from rust/Cargo.toml's version is refused: with the patch unused, cargo
# would quietly vendor the published core instead.
#
# HYPERCAST_PATH, when set, names a hypercast crate directory (its rust/) to take in place of
# crates.io's, the same way as the core: for a HyperTabular that requires a hypercast release
# still being packed. It is a local check only; CI and the release vendor crates.io's.
set -euo pipefail

CRATE=hypertabular

PKG="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO="$(cd "$PKG/.." && pwd)"
# The two corpus files, and every workbook package workbook.json reads: the replay
# (tests/testthat/test-corpus-*.R) reads each one by name, and a file missing from the tarball
# is a test failure, never a skip.
CORPUS_FILES=(delimited.json workbook.json)
# The core's C header (cbindgen's), which the C shim includes.
HEADER=include/hypertabular.h
# The workbook the help pages' examples read, installed as inst/extdata/basic.xlsx.
EXAMPLE=workbook/basic.xlsx
# Cargo reads these relative to the working directory, src/rust.
VENDORED=(--config 'source.crates-io.replace-with="vendored-sources"'
          --config 'source.vendored-sources.directory="vendor"')

core_version() {
  sed -n 's/^version = "\(.*\)"/\1/p' "$1/Cargo.toml" | head -1
}

# The packages workbook.json names, relative to corpus/, sorted and once each.
workbook_files() {
  grep -o '"file": *"workbook/[^"]*"' "$REPO/corpus/workbook.json" \
    | sed 's/.*"\(workbook\/[^"]*\)"/\1/' | LC_ALL=C sort -u
}

# GNU tar, for a reproducible tarball: sorted names, a fixed mtime and owner, so the same
# sources always produce the same bytes. macOS's bsdtar lacks the flags, so gtar (brew
# install gnu-tar) is used when present.
gnu_tar() {
  local tar=tar
  command -v gtar > /dev/null && tar=gtar
  "$tar" --version 2> /dev/null | grep 'GNU tar' > /dev/null || {
    echo "vendor.sh: GNU tar is required (on macOS: brew install gnu-tar)" >&2
    exit 1
  }
  echo "$tar"
}

# Adds the crate at $1 to vendor/ as `cargo package` writes it. --no-verify: the verify build
# is the crate's own CI; --allow-dirty: a working tree with edits is the normal local case.
package_into_vendor() {
  local dir="$1" name version pkgdir
  name="$(sed -n 's/^name = "\(.*\)"/\1/p' "$dir/Cargo.toml" | head -1)"
  version="$(core_version "$dir")"
  pkgdir="$(mktemp -d)"
  cargo package -q --no-verify --allow-dirty "${PATCHES[@]}" --manifest-path "$dir/Cargo.toml" \
    --target-dir "$pkgdir"
  mkdir -p "vendor/$name"
  tar -xzf "$pkgdir/package/$name-$version.crate" --strip-components=1 -C "vendor/$name"
  rm -rf "$pkgdir"
  # A directory source needs a checksum file per crate; with no files listed nothing is
  # checked, which is what `cargo vendor` itself writes for a git dependency.
  echo '{"files":{},"package":null}' > "vendor/$name/.cargo-checksum.json"
}

stage() {
  [ -f "$REPO/rust/Cargo.toml" ] || { echo "vendor.sh: no $CRATE core at $REPO/rust" >&2; exit 1; }
  local tar version pin f
  tar="$(gnu_tar)"
  version="$(core_version "$REPO/rust")"
  pin="$(sed -n "s/^$CRATE = { version = \"=\([^\"]*\)\".*/\1/p" "$PKG/src/rust/Cargo.toml")"
  [ "$pin" = "$version" ] || {
    echo "vendor.sh: src/rust/Cargo.toml pins $CRATE =${pin:-?} but rust/Cargo.toml is $version; the pin is a version file prepare-release bumps" >&2
    exit 1
  }
  PATCHES=(--config "patch.crates-io.$CRATE.path=\"$REPO/rust\"")
  if [ -n "${HYPERCAST_PATH:-}" ]; then
    [ -f "$HYPERCAST_PATH/Cargo.toml" ] || { echo "vendor.sh: no hypercast crate at $HYPERCAST_PATH" >&2; exit 1; }
    PATCHES+=(--config "patch.crates-io.hypercast.path=\"$(cd "$HYPERCAST_PATH" && pwd)\"")
  fi
  cd "$PKG/src/rust"
  rm -rf vendor vendor.tar.xz
  # The wrapper's lock is derived, never committed: start from the core's lock and let
  # cargo prune it to the wrapper's own packages. --workspace touches only the wrapper, so
  # every crates.io package keeps the version rust/Cargo.lock pins. Nothing can move the two
  # locks apart, so prepare-release and Dependabot have nothing to update here.
  cp "$REPO/rust/Cargo.lock" Cargo.lock
  cargo update --workspace -q "${PATCHES[@]}"
  cargo vendor -q --locked "${PATCHES[@]}" vendor
  package_into_vendor "$REPO/rust"
  if [ -n "${HYPERCAST_PATH:-}" ]; then
    package_into_vendor "$HYPERCAST_PATH"
  fi
  cargo update --workspace -q --offline "${VENDORED[@]}"
  "$tar" --sort=name --mtime='2000-01-01 00:00Z' --owner=0 --group=0 --numeric-owner \
    -cJf vendor.tar.xz vendor
  # vendor/ stays for authors(); the modes below remove it.
  cd "$PKG"
  mkdir -p src/include
  cp "$REPO/$HEADER" "src/$HEADER"
  rm -rf tests/testthat/corpus
  mkdir -p tests/testthat/corpus/workbook
  for f in "${CORPUS_FILES[@]}" $(workbook_files); do
    cp "$REPO/corpus/$f" "tests/testthat/corpus/$f"
  done
  # The workbook the examples read.
  mkdir -p inst/extdata
  cp "$REPO/corpus/$EXAMPLE" "inst/extdata/$(basename "$EXAMPLE")"
}

# inst/AUTHORS from the vendored crates' manifests, read offline from the vendor directory.
authors() {
  cd "$PKG"
  Rscript tools/vendor-authors.R
}

case "${1:-}" in
  "")
    stage
    authors
    rm -rf "$PKG/src/rust/vendor"
    ;;
  --stage)
    stage
    rm -rf "$PKG/src/rust/vendor"
    ;;
  --verify)
    cmp -s "$REPO/$HEADER" "$PKG/src/$HEADER" || {
      echo "vendor.sh: src/$HEADER is missing or differs from $HEADER; run tools/vendor.sh" >&2
      exit 1
    }
    for f in "${CORPUS_FILES[@]}" $(workbook_files); do
      cmp -s "$REPO/corpus/$f" "$PKG/tests/testthat/corpus/$f" || {
        echo "vendor.sh: tests/testthat/corpus/$f is missing or differs from corpus/$f; run tools/vendor.sh" >&2
        exit 1
      }
    done
    cmp -s "$REPO/corpus/$EXAMPLE" "$PKG/inst/extdata/$(basename "$EXAMPLE")" || {
      echo "vendor.sh: inst/extdata/$(basename "$EXAMPLE") is missing or differs from corpus/$EXAMPLE; run tools/vendor.sh" >&2
      exit 1
    }
    for f in src/rust/vendor.tar.xz src/rust/Cargo.lock; do
      [ -f "$PKG/$f" ] || {
        echo "vendor.sh: $f is missing; run tools/vendor.sh" >&2
        exit 1
      }
    done
    version="$(core_version "$REPO/rust")"
    tar -xJOf "$PKG/src/rust/vendor.tar.xz" "vendor/$CRATE/Cargo.toml" 2> /dev/null \
      | grep "^version = \"$version\"\$" > /dev/null || {
      echo "vendor.sh: src/rust/vendor.tar.xz does not carry $CRATE $version; run tools/vendor.sh" >&2
      exit 1
    }
    ;;
  *)
    echo "usage: vendor.sh [--stage | --verify]" >&2
    exit 2
    ;;
esac
