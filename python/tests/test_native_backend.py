"""Pins what the forge and a consumer probe before the first read: which backend loaded,
which core answered, and that everything public answers ``help()``."""

from __future__ import annotations

import importlib.metadata
import re
from pathlib import Path

import pytest

import hypertabular
from hypertabular import Batch, Column, ColumnData, DelimitedReader, Dialect, TabularError


def _expected_version() -> str:
    # The one version source is rust/Cargo.toml: maturin bakes it into the package
    # metadata, and the crate bakes it into hypertabular_version — so the expectation is
    # derived, never a literal that goes stale on the next bump. The metadata is absent
    # when the suite runs off the source tree with no install, so fall back to the manifest.
    try:
        return importlib.metadata.version("hypertabular")
    except importlib.metadata.PackageNotFoundError:
        pass
    for parent in Path(__file__).resolve().parents:
        manifest = parent / "rust" / "Cargo.toml"
        if manifest.is_file():
            found = re.search(r'^version\s*=\s*"([^"]+)"', manifest.read_text(encoding="utf-8"), re.MULTILINE)
            assert found, f"no version in {manifest}"
            return found.group(1)
    raise FileNotFoundError("rust/Cargo.toml not found")


def test_the_native_backend_loaded():
    # What the forge's wheel workflow asserts of every wheel it builds.
    assert hypertabular.BACKEND == "native"


def test_native_version_names_the_loaded_core():
    assert hypertabular.native_version() == _expected_version()


def test_everything_exported_is_there():
    for name in hypertabular.__all__:
        assert hasattr(hypertabular, name), name
    assert hypertabular.DelimitedReader.__module__ == "hypertabular"
    assert hypertabular.Batch.__module__ == "hypertabular"
    assert hypertabular.ColumnData.__module__ == "hypertabular"


@pytest.mark.parametrize("name", [name for name in hypertabular.__all__ if name not in ("BACKEND", "Verdict")])
def test_everything_exported_has_a_docstring(name: str):
    doc = getattr(hypertabular, name).__doc__
    assert doc and doc.strip(), f"help(hypertabular.{name}) is empty"


_MEMBERS = [
    (DelimitedReader, ["open", "read", "close", "header", "dialect", "plan", "batch_rows", "records"]),
    (Batch, ["rows", "columns", "column", "raw"]),
    (ColumnData, ["column", "values", "verdicts", "fault_count", "faults", "raw"]),
    (Column, [door.name.lower() for door in hypertabular.Door]),
    (Dialect, ["__post_init__"]),
    (TabularError, ["__init__", "__str__"]),
]


@pytest.mark.parametrize("cls, members", _MEMBERS, ids=[cls.__name__ for cls, _ in _MEMBERS])
def test_members_have_docstrings(cls: type, members: list[str]):
    for member in members:
        doc = getattr(cls, member).__doc__
        assert doc and doc.strip(), f"help({cls.__name__}.{member}) is empty"
