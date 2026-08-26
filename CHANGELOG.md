# Changelog

## Unreleased

## 0.0.4 - 2026-08-26

Fixes an installation that could not finish on any platform: the bootstrap script downloaded and
checksummed the release, and the managed step it hands off to then refused the very same files.

### Fixed

- Managed installation no longer fails with `release verification error: release download error:
  io: invalid peer certificate: UnknownIssuer`. The release downloader verified TLS against a
  compiled-in Mozilla root snapshot, and GitHub's release-asset host now presents a chain anchored
  at the ISRG `Root YR` certificate, which that snapshot does not carry. Rustls neither builds an
  alternate path nor fetches a missing issuer, so it rejected the connection outright. `curl` and
  `Invoke-WebRequest` use the host trust store, which does carry the root, which is why the
  bootstrap download succeeded and only the payload's own fetch failed - on Windows and Linux
  alike, because the bundled snapshot is the same everywhere. Release downloads now use the host
  trust store too, by way of `relswap` 0.0.7. Ed25519 verification against `release-keys.json` is
  unchanged and still runs after TLS succeeds.
- Linux release binaries are built against glibc 2.28 instead of the CI runner's own glibc, so they
  run on distributions older than the build host.

### Changed

- The install scripts keep one rewritten status row while a step runs rather than appending a line
  per step, and `NO_COLOR` now turns off styling without also turning off that compact display.
  Redirected and CI output remains a plain append-only transcript.
- `rozi-launcher` is gated behind the `windows-launcher` feature. It only means something on
  Windows, where it activates a managed install; every other target linked it for nothing.

## 0.0.3 - 2026-08-24

### Added

- The install scripts now show what they are doing. Each step is named, the release archive shows
  download progress instead of several silent megabytes, and the run ends with where to go next.
  Colour and progress appear only when stdout is a terminal that wants them - `curl … | sh` and
  `irm … | iex` still qualify, a redirected install stays plain - and `NO_COLOR` is honoured.
- AUR packaging under `packaging/aur`: `rozi-bin` repackages the signed release archive, `rozi`
  builds from the tag.

### Fixed

- A service whose executable reports as busy is retried rather than failing. Rozi writes extension
  payloads and then runs them, so a fork inheriting a write handle to the file being executed is a
  race the design invites; it is not a broken service.

### Changed

- **Windows only, and a breaking change for existing installs:** the state directory moves from
  `%LOCALAPPDATA%\rozi` to `%LOCALAPPDATA%\rozi\state`. It previously resolved to the same directory
  as the managed installation root, so session autosaves and persisted preferences were written
  beside `versions\`, `bin\`, `active`, `install.json`, and the mutation lock. Existing state is not
  migrated: sessions and preferences saved before this version are not read after it. Move
  `session.toml` and any sibling state files into the new `state\` directory to keep them. The
  managed root, the command path, and the `PATH` entry are unchanged.

## 0.0.2 - 2026-08-24

Makes the Windows installer work, and continues the pipeline exercise begun in 0.0.1 by testing an
update between two real releases.

### Fixed

- `rozi --help` and `rozi --version` no longer create the runtime directory they name. On Windows
  that directory is `%LOCALAPPDATA%\rozi\run`, so creating it also created `%LOCALAPPDATA%\rozi` -
  the managed installation root - with inherited permissions. `install.ps1` probes the payload with
  `--help` before running `install`, so the installer established an unprotected install root
  itself, immediately before failing on it. Deleting the directory beforehand could not help.
- Windows endpoint tests place their socket in a private directory rather than in the shared
  temporary directory, which can never satisfy the privacy an endpoint parent requires.

### Changed

- Updated `relswap` to 0.0.6, which creates the ancestors of a private directory privately instead
  of leaving them with inherited permissions, and treats a directory it did not create at the
  managed root as unmanaged rather than as a fatal error.

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

### Fixed

- `rozi --version` and `rozi --help` no longer abort when the managed installation cannot be
  reconciled. Startup recovery ran ahead of both, so a layout that failed to validate silenced the
  two commands most useful for diagnosing it.

### Changed

- Version 0.0.0 and all earlier distributions were published under `MIT OR Apache-2.0`.
  `MPL-2.0` applies beginning with this version. Existing grants for earlier versions remain
  valid under their original terms.
- Updated `tui-lipan` to 0.3.1, `relswap` to 0.0.6, and `notify` to 8. `relswap` 0.0.4 fixes a
  validation error that rejected every Windows managed install whose command was not named
  `rozi-launcher.exe`; 0.0.5 stops the release tool requiring a Unix executable bit inside the
  Windows ZIP, which no archive built on Windows can carry; 0.0.6 creates the ancestors of a
  private directory privately, so `%LOCALAPPDATA%\rozi` is no longer left unprotected by the first
  runtime directory created inside it, and treats a directory it did not create at the managed root
  as unmanaged rather than as a fatal error.
