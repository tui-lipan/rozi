#!/usr/bin/env python3
"""Apply the dense-viewport/compact-history experiment to disposable source copies."""

import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tomllib


def registry_source(name: str, version: str) -> Path:
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    matches = sorted((cargo_home / "registry" / "src").glob(f"*/{name}-{version}"))
    if len(matches) != 1:
        raise ValueError(f"Expected one cached {name} {version}; supply its source explicitly")
    return matches[0]


def copy_and_patch(source: Path, destination: Path, patch: Path, name: str, version: str):
    package = tomllib.loads((source / "Cargo.toml").read_text())["package"]
    if (package["name"], package["version"]) != (name, version):
        raise ValueError(f"{source} must contain {name} {version}")
    shutil.copytree(
        source,
        destination,
        ignore=shutil.ignore_patterns(".git", "target", "node_modules", ".cargo"),
    )
    # Validate every hunk before applying it. Never patch the caller's source tree.
    command = ["patch", "--batch", "--forward", "--fuzz=0", "-p1", "-i", str(patch)]
    subprocess.run(command + ["--dry-run"], cwd=destination, check=True)
    subprocess.run(command, cwd=destination, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path, help="New disposable directory")
    parser.add_argument("--engine-source", type=Path)
    parser.add_argument("--framework-source", type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    patches = Path(__file__).resolve().parent
    engine = args.engine_source or registry_source("alacritty_terminal", "0.26.0")
    framework = args.framework_source or registry_source("tui-lipan", "0.9.0")
    output.mkdir(parents=True, exist_ok=False)
    copy_and_patch(engine, output / "engine", patches / "engine.patch", "alacritty_terminal", "0.26.0")
    copy_and_patch(framework, output / "framework", patches / "framework.patch", "tui-lipan", "0.9.0")
    print(f"Prepared {output}. See README.md for tests and benchmark commands.")


if __name__ == "__main__":
    main()
