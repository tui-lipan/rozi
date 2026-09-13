#!/usr/bin/env bash
set -euo pipefail

# rozi targets the Rust 2024 edition and needs Rust 1.90 or newer. Cloud Agent
# base images may ship an older default toolchain, so pin the stable channel.
rustup toolchain install stable --profile minimal --component rustfmt --component clippy
rustup default stable

# Extension-manifest tests resolve a `python` executable on PATH; Ubuntu images
# only provide `python3` by default.
if ! command -v python >/dev/null 2>&1; then
  sudo apt-get update
  sudo apt-get install -y python-is-python3
fi

# Warm the dependency cache and produce the debug build. --locked matches CI and
# fails loudly if the manifest and lockfile disagree.
cargo fetch --locked
cargo build --locked
