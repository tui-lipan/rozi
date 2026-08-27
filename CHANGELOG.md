# Changelog

## 0.0.6 - 2026-08-27

Gives `rozi update` the download meter it was missing: the transfer streams behind a progress row
that fits the terminal it is drawn on, reports rate and time remaining once an average means
anything, and leaves the cursor where it found it even under `Ctrl+C`. Also corrects two Windows
installer reports that named the wrong thing - a launch the operating system blocked was reported
as a failed signature, and the script itself arrived mangled over the wire.

### Added

- `rozi update` draws a progress meter while it downloads. `relswap`'s downloader returned a
  response body in a single call, so a multi-megabyte archive arrived in silence and the command
  printed nothing until it had finished. `relswap` 0.0.8 adds `UreqDownloader::with_progress`,
  which streams the body and reports bytes as they land; rozi supplies the observer that draws
  them. The meter goes to stderr, so redirecting stdout keeps a clean stream, and it is suppressed
  when stderr is not a terminal or `NO_COLOR` is set.
- The download row carries a spinner, a transfer rate, and an estimate of the time left. The
  spinner advances per redraw rather than per unit of time, so a stalled transfer visibly stops
  rather than continuing to spin over a bar that is not moving. Rate and estimate are withheld
  until the transfer has run long enough for an average to mean anything, and an estimate is
  dropped once there is nothing left to wait for.
- The cursor is restored when `rozi update` is interrupted. Hiding it for the meter was undone on
  every path the program controls, but `Ctrl+C` terminates without unwinding, so no destructor ran
  and the cursor stayed hidden in the shell until the user typed `reset`. A `SIGINT`/`SIGQUIT`
  handler (and a console control handler on Windows) now restores it and re-raises, so the process
  still dies of the signal it was sent. `SIGTERM` and `SIGHUP` are deliberately left to
  `server_lifecycle`, which owns the clean-detach path.

### Changed

- CLI output and both install scripts now use the rozi palette - the rose-to-violet gradient the
  logo and the app's own theme carry - instead of the basic ANSI colours the CLI had been using.
  Terminals that do not advertise 24-bit colour through `COLORTERM` get the nearest 256-colour cube
  entry rather than losing the styling.
- The download row fits the terminal it is drawn on. It was laid out at a fixed width, so a
  narrow terminal wrapped it - and because the erase-line escape only reaches the row the cursor is
  on, every redraw left the previous fragment behind and the meter walked down the screen. The row
  now degrades in tiers against the measured width, dropping the estimate, then the rate, then the
  bar, then the label, and re-measures on every redraw so a mid-download resize re-fits.
- Progress meters style their filled run and their track separately. Both were previously painted
  in the accent, which made the meter read as one solid shape and hid where the fill actually
  ended. The filled run now carries the brand gradient in a heavy glyph and the track is a light
  glyph in the app's border colour, so the boundary survives `NO_COLOR` on glyph weight alone. A
  meter also reserves its last cell until the work is genuinely complete, rather than rounding up
  to a full bar at 99%.

### Fixed

- The Windows installer no longer reports `Signature failed` when Windows refuses to run the
  downloaded payload. Smart App Control and WDAC block unsigned executables that carry no
  established reputation, and `$ErrorActionPreference = 'Stop'` turned that refusal into a
  terminating error before `$LASTEXITCODE` could be read - so the exit-code check never ran and
  the raw PowerShell error surfaced under whichever status row happened to be active. The row was
  labelled `Signature`, which named the one thing that provably had not happened: the Ed25519
  check runs inside the payload, and the payload never started. The probe is now its own `Payload`
  row, a refused launch is caught on the exception type rather than on Windows' localized message
  text, and the report says the machine's policy blocked execution, notes when Smart App Control
  is enabled, and states that the archive's SHA-256 matched.
- `irm https://rozi.tui-lipan.dev/install.ps1 | iex` no longer mangles the installer's output. The
  site served the script with no `Content-Type` at all, and PowerShell 5.1 decodes a text response
  that declares no charset as ISO-8859-1, so every UTF-8 status glyph arrived as mojibake - the
  script was corrupted in transit before `iex` parsed it. The two install scripts are now served
  as `text/plain; charset=utf-8`.

## 0.0.5 - 2026-08-26

Follows 0.0.4's installation fixes with the one that was still left: a Windows install that
downloaded, verified, and staged the payload correctly, then failed its own self-test because
Defender was still reading the file.

### Fixed

- A healthy Windows install no longer fails with `installation failed: self-test timed out`. The
  probe runs the staged payload immediately after writing it, so an unsigned ~18 MB binary meets
  real-time protection at its least informed: a full scan of a file nothing has seen before, plus a
  cloud-delivered protection lookup that blocks for up to 10 seconds on its own. The 10-second
  budget sat exactly on that ceiling, so the install failed and rolled back while an immediate retry
  - reading a file that was cached by then - passed in about two seconds. The budget is now 90
  seconds, which costs a healthy payload nothing: the probe waits on the process, not on the clock.

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
