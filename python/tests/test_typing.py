"""Pins the typed surface: ``py.typed`` ships, ``_native.pyi`` describes what the loaded
extension actually has, and a consumer's code — a ``match`` over a cell included —
type-checks against the package under ``mypy --strict``. The last needs mypy and is skipped
without it."""

from __future__ import annotations

import ast
import inspect
import os
from pathlib import Path

import pytest

import hypertabular
from hypertabular import _native

PACKAGE = Path(hypertabular.__file__).resolve().parent
SAMPLES = Path(__file__).resolve().parent / "typecheck"
STUB = ast.parse((PACKAGE / "_native.pyi").read_text(encoding="utf-8"))


def _functions(body: list[ast.stmt]) -> dict[str, ast.FunctionDef]:
    return {node.name: node for node in body if isinstance(node, ast.FunctionDef)}


def _classes() -> dict[str, ast.ClassDef]:
    return {node.name: node for node in STUB.body if isinstance(node, ast.ClassDef) and not node.name.startswith("_")}


def _is_static(node: ast.FunctionDef) -> bool:
    return any(isinstance(d, ast.Name) and d.id == "staticmethod" for d in node.decorator_list)


def test_the_package_is_marked_typed():
    assert (PACKAGE / "py.typed").is_file()


def test_the_stub_and_the_loaded_backend_name_the_same_surface():
    stubbed = set(_functions(STUB.body)) | set(_classes())
    missing = {name for name in stubbed if not hasattr(_native, name)}
    assert not missing, f"in _native.pyi but not on the extension: {missing}"
    # And the other way: everything the extension has is stubbed, and everything the
    # package re-exports from it.
    loaded = {name for name in dir(_native) if not name.startswith("__")}
    assert loaded <= stubbed, f"on the extension but not in _native.pyi: {loaded - stubbed}"
    reexported = {name for name in hypertabular.__all__ if getattr(_native, name, None) is getattr(hypertabular, name)}
    assert reexported == {"DelimitedReader", "Batch", "ColumnData", "native_version"}


def test_the_stub_and_the_loaded_backend_agree_on_parameter_names():
    # Parameter names are part of the surface — a keyword call must work.
    for name, node in _functions(STUB.body).items():
        expected = [arg.arg for arg in node.args.args]
        actual = list(inspect.signature(getattr(_native, name)).parameters)
        assert actual == expected, f"{name} on the extension"
    for name, node in _classes().items():
        cls = getattr(_native, name)
        for method, declared in _functions(node.body).items():
            if method.startswith("__") and method != "__new__":
                continue
            target = cls if method == "__new__" else getattr(cls, method)
            if isinstance(target, property) or type(target).__name__ == "getset_descriptor":
                continue
            args = declared.args
            expected = [arg.arg for arg in args.args + args.kwonlyargs]
            if not _is_static(declared):
                expected = expected[1:]
            actual = [p for p in inspect.signature(target).parameters if p not in ("self", "cls")]
            assert actual == expected, f"{name}.{method} on the extension"


def test_the_stub_and_the_loaded_backend_agree_on_class_members():
    for name, node in _classes().items():
        cls = getattr(_native, name)
        declared = set(_functions(node.body)) - {"__new__"}
        declared |= {
            stmt.target.id
            for stmt in node.body
            if isinstance(stmt, ast.AnnAssign) and isinstance(stmt.target, ast.Name)
        }
        missing = {member for member in declared if not hasattr(cls, member)}
        assert not missing, f"{name} on the extension lacks {missing}"
        # And nothing public on the class the stub does not say.
        public = {member for member in vars(cls) if not member.startswith("_")}
        assert public <= declared, f"{name} has {public - declared}, which _native.pyi does not declare"


def test_a_consumers_code_type_checks(monkeypatch):
    api = pytest.importorskip("mypy.api")
    # The package as this checkout has it, the way conftest.py puts it on sys.path. --strict
    # follows the import, so hypertabular's own annotations are checked against the stub too.
    monkeypatch.setenv("MYPYPATH", str(PACKAGE.parent))
    out, err, status = api.run(["--strict", "--cache-dir", os.devnull, str(SAMPLES / "consumer.py")])
    assert status == 0, out + err


def test_the_checker_catches_a_missing_case(monkeypatch, tmp_path):
    # The other half of the promise: drop the Fault arm and the same assert_never is an
    # error. Without this, "it type-checks" could just mean the checker saw Any.
    api = pytest.importorskip("mypy.api")
    monkeypatch.setenv("MYPYPATH", str(PACKAGE.parent))
    sample = tmp_path / "missing_case.py"
    sample.write_text(
        "from typing import assert_never\n"
        "from hypertabular import ColumnData, Success\n"
        "def describe(column: ColumnData) -> str:\n"
        "    verdict = column[0]\n"
        "    match verdict:\n"
        "        case Success(value):\n"
        "            return str(value)\n"
        "        case _:\n"
        "            assert_never(verdict)\n",
        encoding="utf-8",
    )
    out, err, status = api.run(["--strict", "--cache-dir", os.devnull, str(sample)])
    assert status != 0, "a match with no Fault arm type-checked as exhaustive"
    assert "Fault" in out and "assert_never" in out, out + err


def test_the_checker_catches_a_plan_that_is_not_columns(monkeypatch, tmp_path):
    api = pytest.importorskip("mypy.api")
    monkeypatch.setenv("MYPYPATH", str(PACKAGE.parent))
    sample = tmp_path / "bad_plan.py"
    sample.write_text(
        "from hypertabular import DelimitedReader, Dialect\n"
        "DelimitedReader(b'', Dialect.CSV, ['i32'])\n",
        encoding="utf-8",
    )
    out, err, status = api.run(["--strict", "--cache-dir", os.devnull, str(sample)])
    assert status != 0 and "Column" in out, out + err
