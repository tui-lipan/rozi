# Changelog

## Unreleased

## 0.0.1 - 2026-08-24

First release built and signed by the full release pipeline: every archive is verified against a
signed manifest before publication, and the managed installer checks that signature before it
activates a version.

This version exists to exercise that pipeline end to end - build, sign, publish, install, update -
on real infrastructure rather than in a dry run. `0.1.0` is the first release intended for general
use.

### Added

- Signed release manifests and per-archive checksums for every published target
  (`x86_64`/`aarch64` Linux, `x86_64`/`aarch64` macOS, `x86_64` Windows).
- `rozi update`, `rozi update --check`, and `rozi update --rollback` over the managed install
  layout, with a Windows launcher that keeps the stable command path stable across activations.
- `rozi update` now names the channel that owns an install it does not manage - cargo, mise,
  Homebrew, Scoop, WinGet, or a system package - and prints the command that does update it,
  instead of reporting only that no managed installation is present.
- Publication to crates.io as part of the tagged release pipeline.

### Changed

- Version 0.0.0 and all earlier distributions were published under `MIT OR Apache-2.0`.
  `MPL-2.0` applies beginning with this version. Existing grants for earlier versions remain
  valid under their original terms.
- Updated `tui-lipan` to 0.3.1, `relswap` to 0.0.3, and `notify` to 8.
