# shellcheck shell=bash
# Sourced by run.sh and apple-mobile.sh: HyperCast's C archive for a platform, which the C++
# smoke programs build against beside HyperTabular's (hypertabular.hpp stands on
# hypercast.hpp). The version is the one rust/Cargo.toml requires of HyperCast, the release
# this core was built against.
#
#   HYPERCAST_PACKAGES=<dir>   a directory holding unpacked HyperCast archives
#                              (hypercast-<version>-<rid>/), used as they are: a local build,
#                              or HyperCast's own CI artifacts
#
# Otherwise the archive is downloaded from HyperCast's GitHub Release for that version and
# unpacked under the caller's work directory.

# The version rust/Cargo.toml requires of HyperCast: "1.1.0" of `hypercast = { version = "1.1.0"`.
hypercast_version() {
  sed -n -E 's/^hypercast = \{ *version = "[=^~]?([0-9][^"]*)".*/\1/p' "$1/rust/Cargo.toml" | head -n 1
}

# The platform of an unpacked archive, from its directory's name: hypertabular-<version>-<rid>.
archive_rid() {
  local name
  name="$(basename "$1")"
  name="${name#*-}"
  echo "${name#*-}"
}

# Prints the unpacked HyperCast archive for <rid> at <version>, downloading it into <work>.
hypercast_archive() {
  local rid="$1" version="$2" work="$3" name archive url
  name="hypercast-$version-$rid"
  if [ -n "${HYPERCAST_PACKAGES:-}" ]; then
    [ -d "$HYPERCAST_PACKAGES/$name" ] || { echo "smoke: no $name under $HYPERCAST_PACKAGES" >&2; return 1; }
    (cd "$HYPERCAST_PACKAGES/$name" && pwd)
    return 0
  fi
  case "$rid" in
    win-*) archive="$name.zip" ;;
    *) archive="$name.tar.gz" ;;
  esac
  url="https://github.com/SkunkWerkx/HyperCast/releases/download/v$version/$archive"
  mkdir -p "$work/hypercast"
  if command -v curl >/dev/null; then
    curl -fsSL --retry 3 -o "$work/hypercast/$archive" "$url"
  else
    wget -q -O "$work/hypercast/$archive" "$url"
  fi || { echo "smoke: cannot download $url" >&2; return 1; }
  case "$archive" in
    *.zip)
      if command -v unzip >/dev/null; then
        unzip -q "$work/hypercast/$archive" -d "$work/hypercast"
      else
        powershell -NoProfile -Command "Expand-Archive -Path '$(cygpath -w "$work/hypercast/$archive")' -DestinationPath '$(cygpath -w "$work/hypercast")'"
      fi ;;
    *) tar -xzf "$work/hypercast/$archive" -C "$work/hypercast" ;;
  esac
  [ -d "$work/hypercast/$name" ] || { echo "smoke: $archive does not unpack to $name" >&2; return 1; }
  echo "$work/hypercast/$name"
}
