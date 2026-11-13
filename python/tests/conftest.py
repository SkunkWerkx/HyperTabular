"""Puts the in-repo package on the path so the suite runs without any install step — the
development-loop counterpart of the other bindings' local native-library staging — and
makes sure the package's one runtime dependency is there to import.

HyperCast's package, the template for this one, depends on nothing, so the forge's leg
installs pytest and mypy and runs the suite (hyper-build-native.yml, "Test Python
bindings"). This package depends on ``hypercast`` — its verdict types are HyperCast's own —
and running from the source tree, nothing has installed it. An installed wheel brings it
along, and then none of this runs.

Development uses the HyperCast checkout beside this one instead (HyperForge's workspace/, where
each Hyper* repo builds against its peers' working trees): its package from the source tree,
the same way as this one. Never in CI (GitHub Actions sets CI), where companion mode installs
the pinned package and release mode the index's.
"""

import importlib
import importlib.util
import os
import subprocess
import sys
from pathlib import Path

PYTHON = Path(__file__).resolve().parent.parent
PEER_HYPERCAST = PYTHON.parent.parent / "HyperCast" / "python" / "src"

sys.path.insert(0, str(PYTHON / "src"))

if "CI" not in os.environ and (PEER_HYPERCAST / "hypercast").is_dir():
    # Its extension is staged the way CI stages this one's, never quietly replaced by the
    # index's release, which lacks what this package is written against.
    if not any((PEER_HYPERCAST / "hypercast").glob("_native*")):
        raise SystemExit(
            f"{PEER_HYPERCAST / 'hypercast'} has no _native extension: in ../HyperCast/rust run "
            "`cargo rustc --release --crate-type cdylib --features python`, then copy "
            "target/release/libhypercast.so (.dylib on a Mac) to python/src/hypercast/_native.so"
        )
    sys.path.insert(1, str(PEER_HYPERCAST))
elif importlib.util.find_spec("hypercast") is None:
    import tomllib

    # The requirement as pyproject.toml declares it: one place says which HyperCast.
    project = tomllib.loads((PYTHON / "pyproject.toml").read_text(encoding="utf-8"))["project"]
    subprocess.check_call(
        [
            sys.executable,
            "-m",
            "pip",
            "install",
            "--quiet",
            "--disable-pip-version-check",
            *project["dependencies"],
        ]
    )
    importlib.invalidate_caches()
