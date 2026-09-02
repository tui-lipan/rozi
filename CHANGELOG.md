# Changelog

## Unreleased

### Changed

- Session and extension CLI commands now use namespaces. `list-sessions`, `kill-session`, `attach`,
  and `new` are replaced by `sessions list`, `sessions kill`, `sessions attach`, and `sessions new`.
  `list-extensions`, `new-extension`, and `check-extension` are replaced by `extensions list`,
  `extensions new`, and `extensions check`. The `new-pane` CLI alias is removed; use `split`.
  Remote hosts must run this same release for `sessions list --remote` and
  `sessions kill --remote`.
- CLI help and the final detach summary now use the same palette as Rozi's interface and updater.
- Takes tui-lipan 0.4.1, which requires a `termina` that no longer panics the input worker on a
  mouse report at column or row 0. Rozi's `[patch.crates-io]` for that fix is gone, so builds from
  the published crate - `cargo install rozi` included - carry it like every other build.

### Fixed

- `rozi sessions kill <NAME>` exits with an error when no live or restorable session has that name
  instead of succeeding silently.
- The extension detail overlay scrolls its own document with an integrated scrollbar and even side
  padding. It had been wrapped in an outer `ScrollView` with a one-column left inset to work around
  a tui-lipan scrollbar reservation the 0.4.1 renderer no longer needs.

## 0.0.15 - 2026-09-01

Windows fixes found while testing the installer and updater.

### Changed

- Decoded terminal images are capped per pane, so panes and attached clients can no longer multiply
  into an unbounded pixel cache.

### Fixed

- PowerShell keeps its scrollback on Windows. Rozi restored ConPTY's startup cursor after every
  command, which sent the cursor back up the screen and let the next command overwrite the output
  above it.
- Opening the config file or a scrollback dump works when `EDITOR` is set. Both actions built a
  shell command string and single-quoted the path into it, but on Windows the command shell is
  `cmd.exe`, which does not strip those quotes - the editor opened a file whose name still carried
  them, under the home directory, and it could not be saved. The editor is now launched directly
  and the path is passed as its own argument. Shell syntax in `EDITOR` is no longer interpreted;
  quoted program paths still work.
- `rozi update` no longer flickers its download row in Windows Terminal. Each repaint now sends the
  erase sequence and its replacement in one console write, matching the working progress row in
  `install.ps1` instead of exposing a blank frame between two writes.
- Toasts no longer repeat session changes already shown by the picker, workbar, pane set, or
  collaborator list. In particular, disconnecting an SSH host no longer ends with a stale
  `Not connected to <host>` message.
- A Claude pane stays marked as working while a long prompt is being edited. A tall draft pushes
  the activity chrome off screen, which read as the run having finished.

## 0.0.14 - 2026-09-01

Reaches other machines: a picker for remote hosts, ssh prompts answered inside the UI, and one
rule for which machine a key acts on. Extensions gain the user-visible surfaces that previously
needed a hand-edited `config.toml`, and API 1 is frozen.

### Added

- A remote host picker. Hosts are browsed and their sessions listed from inside rozi, `^N`
  connects one, and the target is persisted in canonical form so the same host reached by two
  spellings is one entry. A host is remembered only once a connection to it succeeded, and a
  remembered host can be forgotten from the picker. Discovery on a host runs when the picker asks
  for it rather than for every host up front.
- Rozi answers ssh's own prompts. `ssh` reads a password, a key passphrase, or a host-key question
  from `/dev/tty`, not from the stdin rozi hands it, so a prompt raised while the TUI owned the
  terminal painted over the running UI and swallowed the keystrokes meant for it. Rozi is now its
  own `SSH_ASKPASS` helper: the prompt is relayed to a modal, a password is masked, and a host-key
  fingerprint is shown in clear, because the fingerprint is the whole point of the question. One
  Esc covers that connection's retries rather than being asked three times. Command-line runs,
  where no TUI is up, keep their terminal prompt.
- Opening a host authenticates once. It is four ssh invocations - two probes, a re-check, the
  attach - which on a password host meant typing the password four times. They share one
  authenticated connection now (ControlMaster, Unix only).
- Extensions declare settings, suggest chords, and contribute sidebar tabs. `[settings]` carries
  defaults that users override per extension under `[extensions.<id>]`, and the merged result
  reaches every command, service, and tab as JSON in `ROZI_EXTENSION_CONFIG`. A command may suggest
  a chord with `key`, in a reserved `<prefix> x` space that no built-in ever claims, so a
  suggestion can never collide with rozi or be taken away by a later release. `[[sidebar_tabs]]`
  contributes tabs whose placement is durable: a tab the user drags somewhere keeps that spot, and
  a panel entry naming an absent extension is pruned only once the extension leaves the disk.
  API 1 is frozen - `docs/extensions.md` states what that covers, and a test spells out the
  accepted manifest keys so a change to the surface shows up in the diff as a deliberate edit.
- The website's install box opens on the visitor's own platform. It always opened on `curl`, so
  every Windows visitor arrived at a command their shell cannot run.

### Changed

- Every action operates on the scope its surface shows. The global Sessions picker's `^T` used to
  read the host off whatever session happened to be attached behind the overlay, so the same key
  on the same screen meant local or remote depending on what was underneath it. Sessions stays
  global and its footer reads `new local` once any host is in play; a host-scoped picker acts on
  that host. Startup policy runs inside the scope the launch names, so `--remote <host>` applies
  the configured `[session] startup` value there instead of skipping it. `last` reopens a session
  and never revives one; `profile` still creates its session, and that is now the difference
  between the two.
  **Breaking change:** the remembered session name is kept per workplace, so `last-session` is
  replaced by `last-sessions.json` and one remembered name is lost on the first launch after this.
- `rozi` is the default theme. It shipped wearing `lipan`, tui-lipan's signature palette rather
  than this app's. An unknown `[theme].name` and the `default` alias both resolve to rozi now. A
  custom theme file whose `extends` is omitted still starts from lipan, because that is the base
  those overlays are written against.
- Focus routing is built on the tui-lipan 0.4 primitives. Pane selection is driven by queued
  framework focus transitions, sidebar pointer and Tab acquisition stay independent, and a
  capturing overlay can defer a pane focus request.
- An idle session server costs less. Its loop backs off, queues wake idle consumers rather than
  being polled, and idle waits are capped at four milliseconds so the backoff cannot delay a
  wake-up that matters. Inbound UI draining is batched, reuses its queue weights across a drain,
  and fast-paths the single-entry case; the dispatch result and per-output bookkeeping were trimmed
  to match.

### Fixed

- Typing a queued or steering prompt no longer makes a streaming Claude Code pane look finished.
  Claude removes its interrupt hint while the composer is occupied and a wrapped draft can push
  the animated activity row off-screen; Rozi now reads Claude's star animation directly, then
  holds the last state when the draft leaves no live state evidence to read.
- The Windows client no longer overflows its stack. Opening the sidebar while attached to a remote
  host killed it with `thread 'main' has overflowed its stack`. Rendering recurses through the
  whole view tree, and Windows hands the first thread 1 MiB where Linux and macOS hand it 8 MiB -
  a budget smaller than the one this repository's own render tests had already found too small.
  The UI now runs on a stack sized for it.
- App chords no longer reach a picker, for every overlay rather than one at a time. Two lists
  enumerated the overlays that own the keyboard and had already drifted apart, and the command
  registry was resynced by a flag set by hand in about seventy places - opening a picker set the
  flag without rebuilding, so a leader prefix put rozi into PREFIX mode behind the modal. There is
  one list now, and the gate is compared against what the registry was last built with, so an
  overlay written later is covered whether its author thinks about this or not.
- A `pick` row keeps its label when the description is long. `pick` is the only picker relaying
  arbitrary text from another program, and a description such as a full build command line pushed
  the label out of its own row. The label is served first, the description takes what is left and
  loses its tail to an ellipsis, and the fit is applied in the renderer the palette actually draws
  rather than on an entry that never reaches the screen. The gap between the two is the three cells
  the reader sees, not six cells of stacked chrome guesses.
- A sidebar command tab never shows output from a directory the pane has left. A tab stops polling
  off screen, so its cached rows outlived the directory they were collected in; moving to another
  project and showing the tab again rendered the previous project's rows until the next poll. Rows
  now record their directory and count as absent once the focused pane is elsewhere.
- Abandoned snapshot staging directories are no longer offered as sessions. A writer killed after
  `meta.json` lands but before the rename leaves a complete-looking `.<session>.tmp-*` behind, and
  it listed as restorable; forgetting it deleted a path that never existed and silently succeeded,
  so the row survived every refresh. Identity comes from the directory a snapshot is published
  under, and deleting one sweeps its staging and backup leftovers.
- Dropping a tiled pane onto another halves the slot it lands in. The new split used to size itself
  so the dragged pane kept the width or height it had before the drag; a drop reads as a fresh
  split.
- The which-key strip is dismissed while the mouse is reshaping the layout. A pending prefix owns
  mouse gestures and the chord clears only on release, so a prefix drag left a table of keys
  sitting over the very panes being moved. The `PREFIX` badge is deliberately left alone.
- The documentation site keeps its navbar controls and layout breakpoints aligned when narrow.

## 0.0.13 - 2026-08-28

Brings `install.sh` up to the Windows installer it is supposed to mirror.

### Changed

- `install.sh` matches `install.ps1` again. Three releases of Windows-only work had left two
  scripts that are meant to be the same program in two languages looking nothing alike. The shell
  installer gains the turning spinner, the gradient header and success line, the amber `!` on the
  `PATH` warning, single-column padding, and a download row that reports its size.
- The shell spinner turns in a background subshell, for the reason the Windows one needed a
  thread: a checksum, an HTTPS request and the payload's own install all block the script, so a row
  the main shell redraws can only advance where the work happens to loop. Every write to the row
  stops it first, the header included - otherwise the header lands on the same line as a live
  frame.
- The shell palette gained the gradient's two middle bands and a warning colour, and its violet was
  corrected to the value `install.ps1` uses. `C_VIOLET` had drifted to a different purple and was
  never referenced, so nothing had ever shown the difference.

### Fixed

- The `PATH` section is aligned under its marker. It had three indents doing two jobs: the marker's
  text at column four, the paths under it at column three, and the line introducing the second
  command at column one - further left than the path it followed, so one thought read as three.
- `install.sh` reports download sizes invariantly. `awk`'s `%.1f` follows `LC_NUMERIC`, so a Polish
  shell rendered `7,6 MB` - the same trap `{0:N1}` set on the Windows side in 0.0.12.

## 0.0.12 - 2026-08-28

Gives the Windows installer a working spinner and a legible shape.

### Fixed

- The spinner turns. It reported blocking work - a hash, an HTTPS request, the payload's own
  install - all of which hold the pipeline, so a row redrawn by the main thread could only advance
  where the work happened to loop, and every step but the download looked frozen. It runs on its
  own thread now.
- The spinner is no longer drawn as `?` on a console whose code page cannot carry its glyph. The
  thread has no PowerShell host to write through, so it writes through `[Console]::OutputEncoding`
  - code page 852 on a Polish console, which has no `U+25D0` - while the ticks beside it came out
  intact, because `Write-Host` reaches the console as wide characters and never passes through
  that encoding. The console is switched to UTF-8 for the duration and restored on every exit
  path, and only when the encoding in place cannot carry the glyph.
- The header no longer lands on the same row as the spinner. It was printed while the `Resolve`
  row was still active, so the turning frame and the header shared a line.
- A finished download reports its size invariantly. `{0:N1}` follows the console's culture, and a
  Polish console rendered `7,6 MB`, which reads as a different number.

### Changed

- The run has one thing trying to catch an eye: a green tick and the version painted in the
  wordmark's gradient. The `PATH` warning answers it with an amber `!` in the same column.
- The header reads `rozi 0.0.11` again. Removing the repeated name left a bare version under a
  logo that says nothing about which version it is.
- A finished download reports what arrived - `7.6 MB` - rather than restating the archive name the
  version and target already imply.
- Everything below the wordmark is indented one column instead of two, and the `PATH` command and
  the target are drawn in the gradient's violet rather than the grey used for secondary text.

## 0.0.11 - 2026-08-27

Fixes the PATH command the 0.0.10 installer printed, and makes what it prints legible.

### Fixed

- The "For this terminal" command the installer printed could not be run. It referenced `$bin`,
  which only the *other* block defined, so pasting the one a user actually needs - the session fix,
  for a terminal that is one entry behind - failed with `The variable '$bin' cannot be retrieved
  because it has not been set`. Each block now defines every variable it uses, and a `PATH` that
  is missing the entry outright gets a single block that repairs both the persisted entry and the
  open session, rather than two to choose between.

### Changed

- The installer's `PATH` guidance is laid out to be read. It previously printed the full command
  path as though it were the next thing to run, then explained underneath that `PATH` was wrong,
  then gave two dense one-liners that wrapped mid-argument in a normal terminal. It now states the
  situation, offers the command that works immediately, and prints the remediation indented on its
  own lines. `install.sh` follows the same shape.
- Both installers draw the wordmark in the rose-to-violet gradient the download meter uses, in
  place of the dim grey reserved for secondary text, and the blank line that separated it from the
  command the user had just typed is gone. Where colour is off - `NO_COLOR`, or a redirected
  stream - both still print the art character-for-character identically, which is the property the
  two wordmarks exist to hold.

### Added

- The installer tests cover the two properties its remediation has to have: that every block
  defines the variables it reads, which is the defect 0.0.10 shipped, and that every `PATH` write
  sits inside a `-notcontains` check somewhere above it. The guard check previously looked for that
  on the writing line itself, which the persisted write never satisfies.

## 0.0.10 - 2026-08-27

Replaces the Windows installer's PATH advice with commands that change PATH, and gives the script
its first tests.

### Changed

- A Windows install that leaves the command off `PATH` now prints the PowerShell that fixes it
  rather than telling the user to re-run the installer with `-AddToPath`. Re-running re-downloads
  the archive and re-verifies its checksum and signature in order to append one string to the
  registry, and it does that work *after* the payload probe - so on a machine whose
  application-control policy refuses the payload, the re-run fails before it ever reaches the
  `PATH` code. `-AddToPath` is unchanged and remains the right way to opt in during an install.
- Both printed commands check before they write, so pasting either one twice cannot leave a
  duplicate `PATH` entry. The two are offered separately because they do different things: one
  persists the entry for new terminals, the other repairs the session that is already open.

### Added

- Windows contract tests for `install.ps1`. The bootstrap script is the only Windows-specific
  surface a user meets before rozi has run at all, and it had none: three defects shipped in it in
  a single day, each reasoned about rather than exercised. The tests load the functions out of the
  file that ships and cover the three `PATH` states, the entry matching that separates them
  (case, trailing separators, and a directory that merely shares a prefix), that every printed
  `PATH` change is guarded against a second run, and that the hint never prescribes reinstalling.

## 0.0.9 - 2026-08-27

Both installers now say whether the command they installed can actually be run by name.

### Changed

- The installers report whether the managed command is on `PATH` instead of always printing
  `$ rozi`. Neither script adds its command directory to `PATH` unless asked, so a successful
  install routinely ended by suggesting a command the shell could not find. When the directory is
  reachable the hint is unchanged; when it is not, the full path is printed alongside the way to
  add it - `-AddToPath` on Windows, a `PATH` line for a shell profile on Unix.
- The Windows installer distinguishes "on `PATH` for new terminals" from "on `PATH` here". The
  persisted user `PATH` and the current process's `PATH` diverge whenever an entry is added to a
  shell that is already running, so a correct setup can still need the full path in the session
  that made it. Unix has no equivalent split to report: a shell's `PATH` comes from startup files
  the script neither reads nor writes.
- The installation docs record how to pass an argument to the piped Windows form. `iex` receives
  only the script's text and cannot take parameters, but the same text run as a script block can:
  `& ([scriptblock]::Create((irm ...))) -AddToPath`.

## 0.0.8 - 2026-08-27

Stops the Windows installer taking the terminal down with it. Every install through the advertised
`irm ... | iex` path ended by closing the user's shell, whether it succeeded or failed.

### Fixed

- The installer no longer exits the shell that ran it. `iex` runs the script text in the caller's
  own session rather than in a child process, so the script's top-level `exit` terminated the
  user's terminal, and the installer's status was handed back as the shell's own exit code - a run
  blocked by application control ended `[process exited with code 1 (0x00000001)]`, and a
  successful one closed the window. The install had always finished first, which is what made this
  look like rozi killing the terminal on success. The script now exits only when there is a real
  script invocation to leave, detected through `$PSCommandPath`, which is empty under `iex` even
  when nested inside another script; every other caller receives the status through
  `$LASTEXITCODE`. `install.sh` was never affected, because `curl ... | bash` runs the script in a
  child shell.

## 0.0.7 - 2026-08-27

Repairs a 0.0.6 regression that failed every interactive Windows install partway through the
download, and makes the installer's output survive being read or served under any encoding.

### Fixed

- The Windows installer no longer fails at 90% of the download with `Index was outside the bounds
  of the array`. The download meter added in 0.0.6 picks a gradient band with `[int]($cell * 4 /
  $width)`, and `[int]` rounds in PowerShell rather than truncating: at cell 28 that is `[int]3.5`,
  which is 4, one past the end of a four-entry array. `install.sh` computes the same index with
  `$(( ))`, which truncates, and sends anything unexpected to a `case` catch-all, so only the
  PowerShell port was affected. The same cast on the percentage reported 100% two bytes before the
  last one arrived, filling the bar early and defeating the final cell the meter deliberately
  reserves. All three sites now floor explicitly and the band index is clamped.
- The installer prints its status glyphs correctly when run from disk. Serving the script as
  `text/plain; charset=utf-8` fixed `irm ... | iex`, but PowerShell 5.1 reads a script file with no
  byte-order mark as the system ANSI code page, so the documented `.\install.ps1` form still
  mangled every glyph. A byte-order mark would fix that and break the piped form, because `iex`
  treats a leading U+FEFF as part of the first token; the six glyphs are now built from code
  points instead, leaving the source pure ASCII that no decoding can damage.

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
