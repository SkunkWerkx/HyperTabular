#!/usr/bin/env bash
# The reverse-dependency step a hypercast release takes before it is submitted to CRAN:
# installs the candidate hypercast source tarball into a temporary library, then runs
# `R CMD check --as-cran --no-manual` on the hypertabular source tarball against it, so
# hypertabular's examples and tests (its corpus replay among them) load the candidate and
# nothing else. The PDF manual doesn't depend on hypercast and needs LaTeX, so it is left to
# the package's own check.
#
# It passes on `Status: OK`, or on one NOTE that is the incoming feasibility check saying only
# what a new submission whose hypercast isn't on CRAN yet says (the maintainer, "New
# submission", and hypercast as a strong dependency not in mainstream repositories). Any other
# NOTE, any WARNING or ERROR, a hypercast that won't install, or a check that loaded another
# hypercast than the candidate exits 1, and the check's output directory is kept and named.
#
# R, the Rust toolchain both packages build with and hypertabular's Suggests (testthat,
# jsonlite) come from the caller's environment; the temporary library is put first in
# R_LIBS, ahead of whatever the caller's holds.
#
# usage: .github/scripts/r_revdep_check.sh <hypercast_X.Y.Z.tar.gz> <hypertabular_X.Y.Z.tar.gz>
set -euo pipefail

[ $# -eq 2 ] && [ -f "$1" ] && [ -f "$2" ] || {
  echo "usage: r_revdep_check.sh <hypercast tarball> <hypertabular tarball>" >&2
  exit 2
}
hypercast="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
hypertabular="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"
case "$(basename "$hypercast")" in hypercast_*.tar.gz) ;; *)
  echo "r_revdep_check.sh: $1 is not a hypercast source tarball" >&2; exit 2;; esac
case "$(basename "$hypertabular")" in hypertabular_*.tar.gz) ;; *)
  echo "r_revdep_check.sh: $2 is not a hypertabular source tarball" >&2; exit 2;; esac
want="$(basename "$hypercast" .tar.gz)"
want="${want#hypercast_}"

work="$(cd "$(mktemp -d)" && pwd -P)"
lib="$work/lib"
mkdir -p "$lib"
keep() {
  echo "r_revdep_check.sh: $1; output kept in $work" >&2
  exit 1
}

echo "installing hypercast $want into $lib"
R CMD INSTALL -l "$lib" "$hypercast" > "$work/hypercast-install.log" 2>&1 \
  || keep "hypercast did not install (hypercast-install.log)"

export R_LIBS="$lib${R_LIBS:+:$R_LIBS}"
# Not the check's own incoming checks against CRAN: hypercast is a candidate, so they could
# only say it isn't published, and they need the network.
export _R_CHECK_CRAN_INCOMING_REMOTE_=FALSE
echo "checking $(basename "$hypertabular") against it"
status=0
(cd "$work" && R CMD check --as-cran --no-manual "$hypertabular") > "$work/check.log" 2>&1 || status=$?
log="$work/hypertabular.Rcheck/00check.log"
[ -f "$log" ] || keep "R CMD check wrote no 00check.log (check.log)"
tail -n 3 "$log"

# The hypercast the check resolved: the first on its library path, the check's own directory
# (which holds hypertabular alone) ahead of R_LIBS, and the library it was found in.
loaded="$(R_LIBS="$work/hypertabular.Rcheck:$R_LIBS" Rscript -e \
  'cat(format(packageVersion("hypercast")), normalizePath(dirname(system.file(package = "hypercast"))))')"
[ "$loaded" = "$want $lib" ] || keep "the check resolved hypercast as '$loaded', not $want from $lib"

[ "$status" -eq 0 ] || keep "R CMD check exited $status"
python3 - "$log" << 'PY' || keep "the check reported more than its NOTE"
import re, sys
text = open(sys.argv[1], encoding='utf-8', errors='replace').read()
status = re.search(r'^Status: (.*)$', text, re.M)
if not status:
    sys.exit('no Status line')
if status.group(1) == 'OK':
    sys.exit(0)
if status.group(1) != '1 NOTE':
    sys.exit(f'Status: {status.group(1)}')
# The one NOTE, with its body: the lines after it up to the next "* " line.
found = re.search(r'^\* (checking .*?) \.\.\. NOTE\n(.*?)(?=^\* )', text, re.M | re.S)
if not found or found.group(1) != 'checking CRAN incoming feasibility':
    sys.exit('the NOTE is not the incoming feasibility check')
allowed = re.compile(r"Maintainer: .*|New submission|"
                     r"Strong dependencies not in mainstream repositories:|  hypercast")
extra = [l for l in found.group(2).splitlines() if l.strip() and not allowed.fullmatch(l)]
if extra:
    sys.exit('the incoming NOTE also says:\n' + '\n'.join(extra))
PY
echo "hypertabular passes against hypercast $want"
rm -rf "$work"
