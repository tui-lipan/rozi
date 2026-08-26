# Installation

## Install a release

Linux and macOS:

```bash
curl -fsSL https://rozi.tui-lipan.dev/install | bash
```

Windows PowerShell:

```powershell
irm https://rozi.tui-lipan.dev/install.ps1 | iex
```

With Cargo:

```bash
cargo install rozi
```

The bootstrap scripts download the release for your platform, verify its checksum, and hand it to
rozi's managed installer. They do not edit shell startup files.

The Unix installer puts the command at `$HOME/.local/bin/rozi`. Add that directory to `PATH` if it
is not already there. The Windows command is `%LOCALAPPDATA%\rozi\bin\rozi.exe`.

To let the Windows script add its command directory to your user `PATH`, download the script and
run:

```powershell
.\install.ps1 -AddToPath
```

Piped installer commands do not accept arguments. A downloaded Unix script accepts
`--version VERSION`; the Windows script accepts `-Version VERSION`.

## Update or roll back

These commands apply to installations made by the bootstrap script:

```bash
rozi update --check
rozi update
rozi update --rollback
```

`rozi update --check` checks for a newer release without installing it. `rozi update` downloads
and activates the current release, drawing a progress meter on stderr while the archive arrives.
`rozi update --rollback` activates the previously installed version without another download.

The meter is written to stderr, so redirecting stdout keeps a clean stream while the row still
reaches a watching terminal. It is suppressed entirely when stderr is not a terminal, or when
`NO_COLOR` is set - a redirected or scripted update stays quiet.

The row shows a spinner, the bytes transferred, the transfer rate, and an estimate of the time
left. It is fitted to the terminal's width: a narrow terminal drops the estimate, then the rate,
then the bar itself, rather than wrapping. Interrupting an update with `Ctrl+C` restores the
cursor before exiting.

Managed installations use these locations:

| Platform | Version data | Command |
| --- | --- | --- |
| Linux and macOS | `${XDG_DATA_HOME:-$HOME/.local/share}/rozi` | `$HOME/.local/bin/rozi` |
| Windows | `%LOCALAPPDATA%\rozi` | `%LOCALAPPDATA%\rozi\bin\rozi.exe` |

On Windows the managed root also contains `state\`, `cache\`, `run\`, and `extensions\`. It is
private to your user account, and rozi creates it that way whichever of those it makes first.

The installer refuses to replace a command it does not own. It retains installed versions and does
not currently provide automatic pruning. Update checks are explicit. Updating a local client does
not restart or change sessions on a remote host. On Windows, rerun the bootstrap installer when an
update needs to replace the stable launcher itself.

## Installs rozi does not manage

`rozi update` only acts on an installation the managed installer created. That is deliberate: the
managed layout owns version retention, activation, and rollback, and none of it exists for a binary
another tool placed on your `PATH`.

Rather than refusing and leaving it there, rozi names the channel that *does* own the install and
prints the command that updates it:

```console
$ rozi update
rozi: this rozi was installed with cargo, which owns its updates - run: cargo install rozi --locked
```

The channel is recognised from where the binary sits: `cargo`, `mise`, Homebrew, Scoop, WinGet, or a
system package manager. A layout rozi does not recognise points back at this page instead of
guessing a command. `rozi update --check` names the channel too, and shows the command as an
`Update` row when a newer version exists:

```console
$ rozi update --check
Current  v0.0.2 (cargo)
Latest   v0.1.0
Status   update available
Update   cargo install rozi --locked
```

rozi never runs a package manager on your behalf. A distribution package in particular is updated
through your distribution, and rozi will say so rather than invoking anything itself.

## Install with mise

[mise](https://mise.jdx.dev) installs rozi straight from the GitHub releases, with no plugin and no
registry entry:

```bash
mise use -g github:tui-lipan/rozi
```

It picks the archive for your platform, checks the published checksum, and verifies the release's
GitHub artifact attestations and SLSA provenance before extracting.

An install made this way is **not** a managed installation: mise owns the binary and its versions,
so `rozi update` will decline and point you back at `mise upgrade rozi`. Use whichever owns your
install, not both.

## Build from source

rozi uses Rust edition 2024 and requires Rust 1.90 or newer.

```bash
git clone https://github.com/tui-lipan/rozi.git
cd rozi
cargo build --release
```

The binary is `target/release/rozi`, or `target\release\rozi.exe` on Windows.

The manifest resolves `tui-lipan` from crates.io. A separate `tui-lipan` checkout is not required.

## Download checks

The bootstrap scripts require HTTPS, validate archive paths and sizes, and compare the downloaded
archive with its published checksum. A checksum fetched from the same release location detects
corruption, but it cannot protect against a compromised release account or compromised release
assets.

The managed updater verifies signed release metadata before it activates a downloaded version. If
installation or activation fails, the previously active version remains available.

## Maintainers

Release signing and publication instructions belong in the
[release process](release-process.md).
