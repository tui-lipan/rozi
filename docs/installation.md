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

The Unix installer puts the command at `$HOME/.local/bin/rozi`; the Windows command is
`%LOCALAPPDATA%\rozi\bin\rozi.exe`. Neither directory is added to `PATH` unless you ask, so each
installer finishes with `Run rozi` when the command is already on PATH, or with `Not on PATH`,
the full path, and a pasteable `Add` snippet when it is not.

To let the Windows script add its command directory to your user `PATH`, run the downloaded script
with `-AddToPath`:

```powershell
.\install.ps1 -AddToPath
```

The piped form takes no arguments, because `iex` receives only the script's text. To pass one
without downloading the file first, turn that text into a script block and call it:

```powershell
& ([scriptblock]::Create((irm https://rozi.tui-lipan.dev/install.ps1))) -AddToPath
```

That opts in *during* an install. If an install has already finished and only `PATH` is wrong, set
`PATH` directly instead of re-running: a re-run re-downloads the archive and re-verifies its
checksum and signature to append one string, and it does that after the payload probe, so on a
machine whose application-control policy refuses the payload it fails before reaching the `PATH`
step at all. The installer prints the exact commands, guarded so they are safe to run more than
once, whenever it finds the command directory missing.

On Unix, add the directory to `PATH` in your shell profile:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

A downloaded Unix script accepts `--version VERSION`; the Windows script accepts
`-Version VERSION`.

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

The TUI also checks on its own, on a worker thread shortly after a client starts and every six
hours it stays open. It stays silent when rozi is current or the network is unavailable. A newer
release gets a short toast titled `rozi vX.Y.Z is available`, with the command for the detected
install channel. If the release raises the extension API or session protocol, the toast keeps that
title and becomes a warning naming the compatibility change. Rozi records the announced version in
its state directory, so other clients, later launches, and the re-checks in this one do not repeat
the same release.

[`[updates]`](configuration.md#updates) turns those checks off or changes the interval.

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
not currently provide automatic pruning. The startup check never installs anything. Updating a
local client does not restart or change sessions on a remote host. On Windows, rerun the bootstrap
installer when an update needs to replace the stable launcher itself.

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

It picks the archive for your platform, checks the published checksum, and — for releases that
carry them — verifies the GitHub artifact attestations described in
[Download checks](#download-checks) before extracting.

An install made this way is **not** a managed installation: mise owns the binary and its versions,
so `rozi update` will decline and point you back at `mise upgrade rozi`. Use whichever owns your
install, not both.

## Nightly builds

A nightly is a disposable build of the newest `master` commit whose CI matrix passed. It exists so
a fix can be tried before it is released — "try tonight's build" instead of cutting a version to
test one change — and so a regression can be found in daily use rather than at release time.

Nightlies are published to one rolling prerelease that is replaced every night:

<https://github.com/tui-lipan/rozi/releases/tag/nightly>

The assets are named for the platform rather than a version, so the same link keeps working:

```text
rozi-nightly-x86_64-unknown-linux-gnu.tar.gz
rozi-nightly-aarch64-unknown-linux-gnu.tar.gz
rozi-nightly-x86_64-apple-darwin.tar.gz
rozi-nightly-aarch64-apple-darwin.tar.gz
rozi-nightly-x86_64-pc-windows-msvc.zip
```

Unpack one and run the binary from where you unpacked it:

```bash
curl -fsSLO https://github.com/tui-lipan/rozi/releases/download/nightly/rozi-nightly-x86_64-unknown-linux-gnu.tar.gz
tar -xzf rozi-nightly-x86_64-unknown-linux-gnu.tar.gz
./rozi-nightly-*/rozi --version
```

The archive directory is named for the build date and commit, the archive contains a `NIGHTLY`
file with the same detail, and the binary reports it too:

```console
$ rozi --version
rozi 0.0.18
nightly_commit=a257206
nightly_built=2026-09-10T03:17:00Z
extension_api=1
protocol_min=5
protocol_max=5
```

Quote those lines in a bug report from a nightly. The first line is whatever version the last
release left in `Cargo.toml`, so on its own it cannot tell tonight's master from the published
release.

What a nightly is not:

- **Not a release channel.** `rozi update` and the bootstrap installers only ever select signed,
  v-tagged releases. Nothing moves a stable install onto nightly, and installing a nightly by hand
  does not make a managed installation.
- **Not signed.** There is no release manifest and there never will be: signing unreviewed master
  commits with the release key is the one thing the nightly workflow must not do. The `.sha256`
  files beside the archives come from the same place as the archives, so they detect corruption
  only. What a nightly does carry is a build provenance attestation, which is signed by GitHub
  rather than by rozi's key, so you can still confirm the file came out of this repository's
  nightly workflow before you run it:

  ```bash
  gh attestation verify rozi-nightly-x86_64-unknown-linux-gnu.tar.gz --repo tui-lipan/rozi
  ```

  That says where the file came from. It says nothing about the commit having been reviewed.
- **Not kept.** Tonight's build replaces last night's, and the tag moves with it. Keep a copy if
  you need to come back to a particular nightly.

Run it beside a release rather than over one — a nightly unpacked in its own directory leaves a
managed install untouched, and going back is deleting the directory.

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

Three different things check a rozi download, and it is worth knowing which one covers what.

**The bootstrap scripts** require HTTPS, validate archive paths and sizes, and compare the
downloaded archive with its published checksum. That checksum comes from the same release location
as the archive, so it detects corruption and nothing else: whoever could replace the archive could
replace the checksum beside it.

**The managed updater** verifies signed release metadata before it activates a downloaded version.
A release manifest is signed with Ed25519, and the public key is compiled into every rozi binary,
so `rozi update` accepts only archives whose hashes appear in a manifest that key signed. If
installation or activation fails, the previously active version remains available.

This is the part worth being precise about: that signature is checked when rozi **installs or
updates**, not when it runs. A rozi binary that was never signed — one you built, one from a
distribution package, a nightly — runs normally and warns about nothing. Nothing in rozi verifies
rozi; the signature decides only whether the managed updater will replace your install with bytes
it just downloaded.

**Build provenance attestations** cover the gap the other two leave. Releases from this change
onwards, and every nightly, publish a GitHub artifact attestation: a Sigstore signature over the
archive's digest, bound to the workflow that produced it and recorded in a public transparency
log. It is signed by GitHub rather than by rozi's release key, which makes it an independent
answer to a different question — did this file come out of this repository's workflow? — and one
you can ask *before* running anything:

```bash
gh attestation verify rozi-0.1.0-x86_64-unknown-linux-gnu.tar.gz --repo tui-lipan/rozi
```

That matters most for a first install, because the bootstrap scripts have to execute a downloaded
binary before any signed manifest is consulted. Verifying the archive yourself first closes that
step; an installed rozi's own updates were already covered by the manifest.

To check a signed release the way the updater does, use the release tool pinned by `Cargo.lock`:

```bash
gh release download v0.1.0 --dir rozi-check
relswap verify \
  --name rozi \
  --manifest rozi-check/rozi-release.json \
  --signatures rozi-check/rozi-release.signatures.json \
  --keys release-keys.json \
  --artifacts-dir rozi-check
```

A signed manifest also carries an expiry, so clients refuse one that has lapsed. A scheduled
workflow watches the published release's remaining validity and fails long before that can happen;
see [Release process](release-process.md#release-health).

## Maintainers

Release signing and publication instructions belong in the
[release process](release-process.md).
