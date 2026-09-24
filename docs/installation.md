# Installation

This page covers installing `rozi`, putting it on your `PATH`, keeping it up to date, and checking
what you downloaded. Most people need only the first three sections.

## Install a release

Linux and macOS:

```bash
curl -fsSL https://rozi.tui-lipan.dev/install | bash
```

Windows PowerShell:

```powershell
irm https://rozi.tui-lipan.dev/install.ps1 | iex
```

NetBSD, from pkgsrc:

```sh
pkgin install rozi
```

The Linux and macOS script supports x86-64 and ARM64. The Windows script supports x86-64 only.
There are no prebuilt NetBSD archives, so the install scripts and `rozi update` do not cover it;
pkgsrc owns that install. See [Platform support](platform-support.md#netbsd).

The install scripts download the release archive for your platform, check it against its published
checksum, and hand it to rozi's managed installer, which verifies the signed release before
activating it. They do not edit shell startup files. To install with Cargo or mise instead, see
[Install with Cargo](#install-with-cargo) and [Install with mise](#install-with-mise).

### Install a specific version

Download the script and pass a version. The Unix script takes `--version`:

```bash
curl -fsSLO https://rozi.tui-lipan.dev/install
bash install --version 0.1.0
```

The Windows script takes `-Version`:

```powershell
irm https://rozi.tui-lipan.dev/install.ps1 -OutFile install.ps1
.\install.ps1 -Version 0.1.0
```

## Verify the install

Each install script ends with one of these results:

- `Run rozi` — the command is already on your `PATH`.
- `Not on PATH` — the script prints the full path to the command and an `Add` snippet to paste.
  See [Add rozi to PATH](#add-rozi-to-path).
- `Not this terminal` (Windows only) — your user `PATH` already has the directory, but this terminal
  started before it was added. Open a new terminal, or paste the printed snippet.

Then confirm the installed version and that the updater can reach the release:

```bash
rozi --version
rozi update --check
```

## Add rozi to PATH

The managed command lives here:

| Platform | Command |
| --- | --- |
| Linux and macOS | `$HOME/.local/bin/rozi` |
| Windows | `%LOCALAPPDATA%\rozi\bin\rozi.exe` |

Neither install script adds that directory to `PATH` unless you ask.

On Linux and macOS, add it in your shell profile:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

On Windows, pass `-AddToPath` to add the directory to your user `PATH` during the install:

```powershell
.\install.ps1 -AddToPath
```

The piped `irm … | iex` form cannot take arguments, because `iex` receives only the script's text.
To pass one without saving the file, turn that text into a script block and call it:

```powershell
& ([scriptblock]::Create((irm https://rozi.tui-lipan.dev/install.ps1))) -AddToPath
```

If rozi is already installed and only `PATH` is wrong, paste the snippet the installer printed
rather than running it again. A second run downloads and verifies the whole release again before it
reaches the `PATH` step, and on a machine whose application-control policy blocks the downloaded
binary it fails before that step. The printed snippet checks before it writes, so it is safe to run
more than once.

## Install with Cargo

```bash
cargo install rozi --locked
```

This needs Rust 1.90 or newer. Cargo owns this install: update it by running the same command
again. `rozi update` does not manage it; see [Installs rozi does not manage](#installs-rozi-does-not-manage).

## Install with mise

[mise](https://mise.jdx.dev) installs rozi straight from the GitHub releases, with no plugin and no
registry entry:

```bash
mise use -g github:tui-lipan/rozi
```

mise picks the archive for your platform, checks the published checksum and, for releases that
carry them, verifies the GitHub artifact attestations described in
[Download checks](#download-checks) before extracting.

mise owns this install and its versions, so `rozi update` declines and points you to
`mise upgrade rozi`. Update with whichever tool installed rozi, not both.

## Build from source

rozi uses Rust edition 2024 and requires Rust 1.90 or newer.

```bash
git clone https://github.com/tui-lipan/rozi.git
cd rozi
cargo build --release
```

The binary is `target/release/rozi`, or `target\release\rozi.exe` on Windows.

The manifest resolves `tui-lipan` from crates.io, so you do not need a separate `tui-lipan`
checkout.

## Update or roll back

These commands work on installs made by the install scripts:

```bash
rozi update --check
rozi update
rozi update --rollback
```

- `rozi update --check` reports whether a newer release exists without installing it.
- `rozi update` downloads and activates the current release.
- `rozi update --rollback` activates the previously installed version without downloading
  anything.

After a successful update or rollback, rozi also refreshes unchanged agent skills installed by
`rozi skill install` to match the activated binary. Modified or untracked copies are left alone with
a warning; see [Agent skill](agent-skill.md#check-refresh-or-remove).

If an install or activation fails, the previously active version stays available. The installer
refuses to replace a command it does not own. It keeps every installed version and does not prune
old ones automatically.

### What an update restarts

An update installs the new build for the next launch; it restarts nothing that is running:

- The client you updated from keeps the old build until you quit and start rozi again.
- Each session server keeps its old build until you restart that session with `restart-session`,
  or by pressing `Ctrl+E` twice in Sessions. Restarting a session ends the programs running in its
  panes.
- A new client attaches to an old server normally while the session protocol is unchanged. When a
  release raises the protocol, the old server refuses the new client, so restart the session before
  attaching.
- Updating a local client does not change sessions on a remote host.

On Windows, run the install script again when an update needs to replace the stable launcher
itself.

### Update notifications

The TUI checks for a new release shortly after a client starts, then every six hours while it stays
open. The check never installs anything, and it stays silent when rozi is current or the network is
unavailable.

When a newer release exists, rozi shows a toast titled `rozi vX.Y.Z available`, with the version
step and the update command for your install channel. If the release raises the extension API or
the session protocol, the toast uses warning colors and adds a row naming the change. rozi records
the announced version in its state directory, so other clients, later launches, and later checks
do not toast the same release again. Every client that finds the release still lists
**Update rozi to vX.Y.Z** in Commands, which runs the update in a popup (see
[Update notices](configuration.md#update-notices)).

Use [`[updates]`](configuration.md#updates) to turn these checks off or change the interval.

### Progress output

While the archive downloads, `rozi update` draws one progress row on stderr: a spinner, the bytes
transferred, the transfer rate, and the estimated time left. A narrow terminal drops the estimate,
then the rate, then the bar, rather than wrapping. Pressing `Ctrl+C` restores the cursor before
exiting.

Because the row goes to stderr, redirecting stdout still gives you a clean stream. The row is
suppressed entirely when stderr is not a terminal or `NO_COLOR` is set, so scripted updates stay
quiet.

### Managed install locations

| Platform | Version data | Command |
| --- | --- | --- |
| Linux and macOS | `${XDG_DATA_HOME:-$HOME/.local/share}/rozi` | `$HOME/.local/bin/rozi` |
| Windows | `%LOCALAPPDATA%\rozi` | `%LOCALAPPDATA%\rozi\bin\rozi.exe` |

On Windows the managed root also contains `state\`, `cache\`, `run\`, and `extensions\`. It is
private to your user account, and rozi creates it that way whichever of those it creates first.

## Installs rozi does not manage

`rozi update` only acts on an install the managed installer created. For any other install, it
names the tool that owns it and prints the command that updates it:

```console
$ rozi update
rozi: this rozi was installed with cargo, which owns its updates - run: cargo install rozi --locked
```

rozi recognises the channel from where the binary sits: `cargo`, `mise`, Homebrew, Scoop, WinGet,
or a system package manager. For a layout it does not recognise, it points you to this page instead
of guessing a command. `rozi update --check` names the channel too, and shows the command as an
`Update` row when a newer version exists:

```console
$ rozi update --check
Current  v0.0.2 (cargo)
Latest   v0.1.0
Status   update available
Update   cargo install rozi --locked
```

rozi never runs a package manager for you. Update a distribution package through your
distribution.

## Nightly builds

A nightly is a disposable build of the newest `master` commit that passed CI. Use one to try a fix
before it is released, or to catch a regression in daily use.

Nightlies are published to one rolling prerelease that is replaced every night:

<https://github.com/tui-lipan/rozi/releases/tag/nightly>

Asset names carry the platform rather than a version, so the same link keeps working:

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

The directory inside the archive is named for the build date and commit, a `NIGHTLY` file inside it
records the same detail, and the binary reports it too:

```console
$ rozi --version
rozi 0.0.23
nightly_commit=a257206
nightly_built=2026-09-10T03:17:00Z
extension_api=1
protocol_min=14
protocol_max=14
```

Include those lines in a bug report from a nightly. The first line is whatever version the last
release left in `Cargo.toml`, so on its own it does not distinguish a nightly from the release.

Unpack a nightly in its own directory, beside any release install rather than over it. That leaves a
managed install untouched, and going back means deleting the directory.

Keep these limits in mind:

- **Not a release channel.** `rozi update` and the install scripts only select signed, `v`-tagged
  releases. Nothing moves a stable install onto a nightly, and a nightly installed by hand is not a
  managed install.
- **Not signed.** Nightlies have no signed release manifest. The `.sha256` files beside the archives
  come from the same place as the archives, so they detect corruption only. Each nightly does carry
  a build provenance attestation, signed by GitHub rather than by rozi's release key. It confirms
  the file came from this repository's nightly workflow, but not that the commit was reviewed:

  ```bash
  gh attestation verify rozi-nightly-x86_64-unknown-linux-gnu.tar.gz --repo tui-lipan/rozi
  ```

- **Not kept.** Each night's build replaces the previous one, and the tag moves with it. Keep a copy
  if you need to return to a particular nightly.

## Download checks

Three separate mechanisms check a rozi download, and each answers a different question.

**Install scripts** require HTTPS, validate archive paths and sizes, and compare the archive with its
published checksum. That checksum comes from the same release location as the archive, so it
detects corruption only: anyone who could replace the archive could replace the checksum too.

**The managed updater** verifies signed release metadata before it activates a downloaded version.
Each release manifest is signed with Ed25519, and the public key is compiled into every rozi binary,
so `rozi update` accepts only archives whose hashes appear in a manifest signed by that key.

This signature is checked when rozi installs or updates, not when it runs. A binary that was never
signed, such as one you built, a distribution package, or a nightly, runs normally without
warnings. The signature only decides whether the managed updater replaces your install with a
download.

**Build provenance attestations** answer whether a file came from this repository's workflow. Every
nightly, and every release since attestations were introduced, publishes a GitHub artifact
attestation: a Sigstore signature over the archive's digest, tied to the workflow that built it and
recorded in a public transparency log. GitHub signs it, not rozi's release key, and you can check it
before running anything:

```bash
gh attestation verify rozi-0.1.0-x86_64-unknown-linux-gnu.tar.gz --repo tui-lipan/rozi
```

This matters most for a first install, because the install scripts run the downloaded binary before
any signed manifest is checked. Verifying the archive yourself first closes that gap. Later updates
are already covered by the signed manifest.

To check a signed release the way the updater does, use the `relswap` release tool at the version
pinned by `Cargo.lock` (see [Release process](release-process.md#prerequisites) for the install
command):

```bash
gh release download v0.1.0 --dir rozi-check
relswap verify \
  --name rozi \
  --manifest rozi-check/rozi-release.json \
  --signatures rozi-check/rozi-release.signatures.json \
  --keys release-keys.json \
  --artifacts-dir rozi-check
```

A signed manifest also has an expiry date, and clients refuse one that has lapsed. A scheduled
workflow watches the published release's remaining validity and fails well before that happens; see
[Release process](release-process.md#release-health).

## Maintainers

Release signing and publication are covered in the [release process](release-process.md).
