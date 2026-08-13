# Installation and releases

`rozi` has a small bootstrap helper for Unix and Windows. It downloads the host release archive,
downloads the adjacent checksum, verifies the archive bytes, safely extracts the canonical payload,
and executes that payload with `install`. The payload's managed installer verifies the signed
release metadata before activating the version. Bootstrap helpers do not edit shell startup files
or create the managed layout themselves.

## Bootstrap trust caveat

> Downloading an archive and its checksum from the same HTTPS release location protects against corruption, but does not provide independent authenticity if the release account or release assets are compromised.

This caveat applies to both installer help outputs. The bootstrap helpers require HTTPS, exact asset
names, archive-root validation, and checksum matching. Those checks detect transfer corruption but
do not authenticate a release location. The extracted payload then verifies signed release metadata
before the managed installer activates its downloaded version; that verification cannot make a
compromised bootstrap payload safe before it runs.

## Bootstrap resource limits

Both helpers reject archives larger than 256 MiB or checksums larger than 1 MiB while downloading.
Windows also rejects any declared ZIP member over 256 MiB or total declared uncompressed content over
256 MiB; extracted payload and launcher files are capped at 256 MiB. Unix applies the 256 MiB cap to
the declared and extracted payload. Only canonical executable members are written from an archive.

## Managed layout contract

The installed CLI owns version retention, activation, rollback state, and the stable command path.
An install keeps immutable version directories and switches one authoritative active pointer. It
does not prune retained versions automatically, and it refuses to replace an unmanaged command.

Default locations are:

| Platform | Managed data root | Stable command |
| --- | --- | --- |
| Unix/macOS | `${XDG_DATA_HOME:-$HOME/.local/share}/rozi` | `$HOME/.local/bin/rozi` |
| Windows | `%LOCALAPPDATA%\rozi` | `%LOCALAPPDATA%\rozi\bin\rozi.exe` |

`XDG_DATA_HOME` changes the Unix data root only; the Unix stable command remains under
`$HOME/.local/bin`. The CLI has no `--root`, `--bin-dir`, or updater-version argument.

Unix layout:

```text
<managed data root>/
├── versions/<version>/
│   ├── rozi
│   ├── release.json
│   ├── release.signatures.json
│   └── version.json
├── install.json
├── pending-activation.json  # transient activation journal
├── .lock                    # mutation lock
└── .staging/                # transaction staging

$HOME/.local/bin/rozi -> <managed data root>/versions/<version>/rozi
```

Windows layout:

```text
%LOCALAPPDATA%\rozi\
├── versions\<version>\
│   ├── rozi.exe
│   ├── release.json
│   ├── release.signatures.json
│   └── version.json
├── bin\rozi.exe         # stable launcher; retained across updates
├── active                   # authoritative active-version selector
├── install.json
├── pending-activation.json  # transient activation journal
├── .lock                    # mutation lock
└── .staging\                # transaction staging
```

The stable Windows launcher is created during the first managed install and is verified for
ownership before an update reuses it. `install.json` records the active and previous versions;
`update --rollback` selects the recorded previous version without downloading another archive.

## Bootstrap installation

Unix:

```bash
# Resolve the current released tag and install its exact host archive.
./install.sh

# Select an exact release archive for bootstrap.
./install.sh --version 0.1.0
```

Windows PowerShell:

```powershell
# Resolve the current released tag and install its exact host archive.
.\install.ps1

# Select an exact release archive and opt in to the user PATH entry.
.\install.ps1 -Version 0.1.0 -AddToPath
```

The helper's optional version selects the archive used to bootstrap. It is not forwarded as an
argument to the payload: the extracted binary's package version is the version passed to its
`install` command. The helper deletes its temporary download and extraction directory after the
payload exits. Unix PATH setup is left to the user; Windows `-AddToPath` appends
`%LOCALAPPDATA%\rozi\bin` to the user PATH only after a successful install.

`ROZI_RELEASE_REPO` changes the GitHub repository used by the helper. `ROZI_RELEASE_BASE_URL`
can point at an HTTPS mirror with the same release-directory layout. When no version is supplied,
`ROZI_RELEASE_LATEST_URL` selects an HTTPS `.../releases/latest` redirect endpoint; both
installers resolve its final URL and require a `v<version>` tag. These variables affect bootstrap
downloads only; managed updates use the repository compiled into the installed binary.

## Update, check, and rollback

Run lifecycle operations through the installed command. These commands take no root, binary, or
version arguments:

```bash
# Verify signed latest metadata and compare it with the active managed version.
rozi update --check

# Download and activate the signed latest release.
rozi update

# Activate the retained previous version.
rozi update --rollback
```

`rozi install` is the bootstrap payload operation. It installs the exact package version compiled
into that payload and is normally invoked by `install.sh` or `install.ps1`.

### 0.2 update boundaries

- Every installed version is retained. There is no automatic pruning, cleanup command, or process
  lease yet.
- Startup performs local crash recovery for managed installs, but no passive network check. Update
  checks are explicit through `rozi update --check`; there is no workbar notice or in-TUI install.
- Local activation does not update, restart, or otherwise change remote sessions. The existing
  exact-version remote bootstrap remains separate until its later signed-manifest migration.
- Windows launcher protocol 1 is stable and is not replaced by self-update. A launcher security fix
  requires rerunning the bootstrap installer after closing rozi.

Help exits `0`. Download, checksum, archive, or managed-install failures exit `1`; invalid
bootstrap command-line usage exits `2`. A failed bootstrap leaves no managed layout changes made
by the helper itself.

## Release archive contract

Every release contains canonical assets for the supported targets:

```text
rozi-<version>-<target>.tar.gz       # Unix
rozi-<version>-<target>.zip          # Windows
```

Each archive has a root directory with the same stem. It contains `rozi` on Unix, `rozi.exe`
on Windows, and `rozi-launcher.exe` on Windows. The release assets also include:

```text
rozi-release.json
rozi-release.signatures.json
<archive>.sha256                         # adjacent to every archive
```

The release tool computes archive, payload, and launcher sizes/hashes from the final bytes and
rejects target, path, size, or hash mismatches before publication. The adjacent checksums are
published for bootstrap corruption detection; see the trust-boundary caveat above.

The generated manifest is the canonical JSON shape
`{schema_version, version, published_at, expires_at, targets}` (schema version 2).
`published_at` and `expires_at` are RFC3339 UTC timestamps; verification rejects expired
manifests after signature checks. `targets` is keyed by the target triple. Each value contains
`archive`, `archive_sha256`, `archive_size`, `payload`, and the Windows-only `launcher`.

Both `tui-lipan` and `relswap` come from crates.io, so `Cargo.lock` pins the exact engine each
signed release was built and signed with, and the release workflow builds from a single checkout.

## Maintainer key generation and signing

Signing and metadata live in [`relswap`](https://github.com/tui-lipan/relswap). Install its optional
`release-tool` binary once:

```bash
cargo install relswap --features release-tool
```

Then generate the key pair:

```bash
mkdir -p "$HOME/.config/rozi/release-keys"
relswap keygen \
  --id release-2026-a \
  --private-key "$HOME/.config/rozi/release-keys/release-2026-a.private.b64" \
  --public-key /tmp/rozi-release-keys.json

# Review the strict document, then replace the initial empty trust file deliberately.
mv /tmp/rozi-release-keys.json release-keys.json
```

Both paths are mandatory. Key generation uses OS randomness, refuses to overwrite either path, and
writes the Unix private file as mode `0600`. The private path above is outside the repository by
design; only the strict public document belongs in committed `release-keys.json`. There is no
production default key or test key. `rotate` is an alias of `keygen` for issuing a successor key;
`sign --append` adds another signature to an existing envelope.

After build/package jobs have produced final archives, generate and sign metadata:

```bash
relswap manifest --name rozi --version 0.1.0 --artifacts-dir dist --output dist/rozi-release.json

ROZI_RELEASE_PRIVATE_KEY="$(tr -d '\n' < "$HOME/.config/rozi/release-keys/release-2026-a.private.b64")" \
relswap sign --name rozi --manifest dist/rozi-release.json \
       --output dist/rozi-release.signatures.json \
       --key-id release-2026-a

relswap verify --name rozi --manifest dist/rozi-release.json \
         --signatures dist/rozi-release.signatures.json \
         --keys release-keys.json --artifacts-dir dist
```

Signing covers the exact bytes on disk; reparsing and reserializing the manifest after signing will
invalidate its digest/signature. The envelope contains a list of key-id/signature records so a
rotation can append a second trusted signature without changing the manifest. Verification
requires at least one valid Ed25519 signature from the committed trust document and checks the final
archives and adjacent checksums again.

## Protected publication setup

Production publication requires a generated `release-2026-a` entry in `release-keys.json`. An empty
or missing trust-key file is an intentional fail-closed condition: the workflow runs `trust-check`
before it reads any signing secret and prints setup guidance.

After committing that public document:

1. Create a GitHub Actions environment named `release` and require maintainer approval as desired.
2. Add the environment secret `ROZI_RELEASE_PRIVATE_KEY` containing the base64 32-byte private
   key value, not a repository file or checked-in secret.
3. Optionally set the environment variable `ROZI_RELEASE_KEY_ID`; otherwise the workflow uses
   `release-2026-a`.
4. Keep the private value available only to the workflow's signing step. Build/test/package jobs,
   pull requests, and manual dispatches never receive it.

The protected signing job downloads immutable completed archives and a prebuilt Linux release tool,
checks the tag against `Cargo.toml`, generates hashes from final bytes, signs, verifies against the
committed key, and uploads one verified publication bundle. A separate publication job with
`contents: write` downloads only that bundle and runs `gh release create`; it receives no signing
secret. The signing job never runs Cargo after the private key is available. Manual dispatch can
exercise artifact builds but cannot publish a release.
