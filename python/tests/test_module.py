"""The compiled module loads and reports the package version."""

import tomllib
from pathlib import Path

import taktsim


def test_version_matches_workspace() -> None:
    """`__version__` is the workspace version from Cargo.toml."""
    manifest = Path(__file__).parents[2] / "Cargo.toml"
    with manifest.open("rb") as f:
        version = tomllib.load(f)["workspace"]["package"]["version"]
    assert taktsim.__version__ == version
