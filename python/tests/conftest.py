"""Puts the in-repo package on the path so the suite runs without any install step — the
development-loop counterpart of the other bindings' local native-library staging — and
makes sure the package's one runtime dependency is there to import.

HyperCast's package, the template for this one, depends on nothing, so the forge's leg
installs pytest and mypy and runs the suite (hyper-build-native.yml, "Test Python
bindings"). This package depends on ``hypercast`` — its verdict types are HyperCast's own —
and running from the source tree, nothing has installed it. An installed wheel brings it
along, and then none of this runs.
"""

import importlib
import importlib.util
import subprocess
import sys
from pathlib import Path

PYTHON = Path(__file__).resolve().parent.parent

sys.path.insert(0, str(PYTHON / "src"))

if importlib.util.find_spec("hypercast") is None:
    import tomllib

    # The requirement as pyproject.toml declares it: one place says which HyperCast.
    project = tomllib.loads((PYTHON / "pyproject.toml").read_text(encoding="utf-8"))["project"]
    subprocess.check_call(
        [sys.executable, "-m", "pip", "install", "--quiet", "--disable-pip-version-check",
         *project["dependencies"]]
    )
    importlib.invalidate_caches()
