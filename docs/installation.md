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
and activates the current release. `rozi update --rollback` activates the previously installed
version without another download.

Managed installations use these locations:

| Platform | Version data | Command |
| --- | --- | --- |
| Linux and macOS | `${XDG_DATA_HOME:-$HOME/.local/share}/rozi` | `$HOME/.local/bin/rozi` |
| Windows | `%LOCALAPPDATA%\rozi` | `%LOCALAPPDATA%\rozi\bin\rozi.exe` |

The installer refuses to replace a command it does not own. It retains installed versions and does
not currently provide automatic pruning. Update checks are explicit. Updating a local client does
not restart or change sessions on a remote host. On Windows, rerun the bootstrap installer when an
update needs to replace the stable launcher itself.

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
