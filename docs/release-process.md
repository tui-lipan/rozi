# Release process

This page is for maintainers publishing signed Rozi releases. User installation and update behavior
is documented in [Installation and releases](installation.md). The workflow in
`.github/workflows/release.yml` is the source of truth when this page and automation differ.

## Prerequisites

A release maintainer needs:

- permission to push a tag and approve the GitHub `release` environment;
- Rust 1.90 or newer with Cargo, rustfmt, and Clippy;
- `cargo-audit` and `cargo-deny`;
- GitHub CLI access for post-publication inspection;
- access to the protected `ROZI_RELEASE_PRIVATE_KEY` environment secret;
- a crates.io API token stored as `CARGO_REGISTRY_TOKEN` in the same environment.

The committed `release-keys.json` trust store is populated. It currently contains the Ed25519 key
`release-2026-a`. The GitHub `release` environment must hold the matching base64 private key in
`ROZI_RELEASE_PRIVATE_KEY`. `ROZI_RELEASE_KEY_ID` may select another committed key; the workflow
defaults to `release-2026-a`.

Keep private keys outside the repository. The signing job exposes the private value only as
`RELSWAP_RELEASE_PRIVATE_KEY` during the signing step. Build, test, package, pull-request, and manual
workflow runs do not receive it.

For local manifest inspection, install the same `relswap` release tool pinned by `Cargo.lock`:

```bash
RELSWAP_VERSION=$(
  cargo metadata --locked --format-version 1 |
    python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="relswap"))'
)
cargo install relswap --version "=$RELSWAP_VERSION" --locked --features release-tool
```

Key generation and rotation are separate from a normal release, and have their own section below.
Never commit a private key.

## Prepare the release

1. Choose a semantic version and update `package.version` in `Cargo.toml`.
2. Let Cargo update `Cargo.lock` and confirm both files describe the same dependency graph.
3. Update user documentation for release behavior that changed.
4. Confirm the release commit is on the intended branch and every commit has a DCO sign-off.
5. Run the repository checks:

```bash
cargo fmt --all -- --check
cargo test
cargo clippy --all-targets -- -D warnings
git diff --check
cargo check --locked --all-targets
cargo build --locked --release
cargo publish --locked --dry-run
cargo deny check licenses sources advisories bans
cargo audit
```

If documentation changed, also run:

```bash
cd docs
npm ci
npm run docs:build
```

Push the release commit and wait for the normal CI matrix to pass on Linux, macOS, and Windows.

## Tag and run the workflow

Create an annotated `v<version>` tag on the reviewed release commit:

```bash
VERSION=0.2.0
test "$(cargo metadata --locked --no-deps --format-version 1 |
  python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')" = "$VERSION"
git tag -a "v$VERSION" -m "rozi $VERSION"
git push origin "v$VERSION"
```

Pushing the tag starts the Release workflow. Do not move or reuse a release tag. Pull-request and
manual workflow runs test packaging but cannot sign or publish.

When a maintainer has GitHub website access but no local Git client, the new-release form may create
the same tag on the reviewed release commit. Publish it as a **prerelease**: this keeps the incomplete
release out of `/releases/latest`. The workflow detects that placeholder, uploads only the verified
signed bundle, and promotes it to the latest stable release after publication succeeds. Never publish
an empty stable release before the signing workflow completes.

The workflow performs these gates:

1. `cargo test --locked` and `cargo check --locked --all-targets` run on Linux.
2. Release archives build for Linux x86_64 and arm64, macOS x86_64 and arm64, and Windows x86_64.
   Linux payloads build in pinned manylinux 2.28 containers, and the workflow rejects binaries whose
   ELF version requirements exceed `GLIBC_2.28`.
3. Each payload reports the tag version and prints help.
4. Each final archive is extracted and smoke-tested. Windows also tests launcher version selection,
   argument and environment forwarding, working-directory forwarding, and exit-code propagation.
5. The signing job confirms the tag version matches `Cargo.toml`, checks every expected archive,
   and runs `relswap trust-check` against the committed trust store before reading the private key.
6. The workflow generates the manifest and checksums from final archive bytes, signs the exact
   manifest bytes, and verifies every archive against `release-keys.json`.
7. The GitHub publication job receives only the verified bundle. It has no signing secret. It
   attests each final archive's provenance before uploading.
8. After the signed GitHub release exists, a final protected job rechecks the tag and runs
   `cargo publish --locked` with `CARGO_REGISTRY_TOKEN`.

The `release` environment can require maintainer approval before signing. Review the tag, commit,
completed package jobs, and selected key ID before approving it.

## Release archive and signature contract

The workflow publishes these archives:

```text
rozi-<version>-x86_64-unknown-linux-gnu.tar.gz
rozi-<version>-aarch64-unknown-linux-gnu.tar.gz
rozi-<version>-x86_64-apple-darwin.tar.gz
rozi-<version>-aarch64-apple-darwin.tar.gz
rozi-<version>-x86_64-pc-windows-msvc.zip
```

Each archive has a same-named root directory. Unix archives contain `rozi`. The Windows archive
contains `rozi.exe` and `rozi-launcher.exe`. Archives also contain `README.md`, `LICENSE`, and
`examples/`.

The publication bundle also contains:

```text
rozi-release.json
rozi-release.signatures.json
rozi-compatibility.json
<archive>.sha256
```

`rozi-release.json` uses schema version 2 and records the version, publication and expiry times,
target archive names, SHA-256 values, byte sizes, payload metadata, and Windows launcher metadata.
`relswap sign` signs the exact manifest bytes with Ed25519. Do not reformat or reserialize the
manifest after signing. `relswap verify` requires a trusted signature and rechecks archives and
adjacent checksums.

`rozi-compatibility.json` records the release's extension API and session protocol. The workflow
derives it from the packaged Linux binary's `--version` output. Clients use it only to choose
between an informational update toast and a compatibility warning; installation still trusts the
signed release manifest and archive hash.

The adjacent `.sha256` files let bootstrap installers detect corruption. They come from the same
release location as the archives, so they are not an independent authenticity check. Managed
updates verify the signed manifest against the public keys compiled into Rozi.

The publication job also attests every archive with `actions/attest-build-provenance`. That is a
Sigstore signature over each archive's digest, bound to this workflow's OIDC identity and recorded
in a public transparency log, and it is deliberately independent of rozi's own release key: losing
control of that key does not confer the ability to forge an attestation, and a user can check one
before executing anything, which is the one thing the bootstrap installers cannot do for
themselves. Users verify with `gh attestation verify <archive> --repo tui-lipan/rozi`. Nightly
archives carry the same attestation; they have no manifest and no signature.

Package and verified-bundle workflow artifacts are retained for 14 days. The published GitHub
release is the durable public copy.

## Publication

After signing succeeds, the GitHub publication job runs `gh release create` with `--verify-tag`
and generated release notes. It uploads the manifest, signature envelope, compatibility document,
archives, and checksums.

The workflow publishes the crate to crates.io only after the signed GitHub release succeeds.
crates.io versions cannot be replaced or deleted. A publication failure leaves the GitHub release
usable and can be retried without changing its assets.

Confirm the workflow completed rather than relying on the tag push alone:

```bash
gh run list --workflow Release --limit 5
gh release view "v$VERSION"
```

Check that all five archives, all five adjacent checksums, `rozi-release.json`,
`rozi-release.signatures.json`, and `rozi-compatibility.json` are present. Confirm that crates.io
lists the same version only after the final workflow job succeeds.

## Smoke verification after publication

Download the public assets into an ignored directory and verify them with the pinned release tool:

```bash
SMOKE_DIR="target/release-smoke/$VERSION"
mkdir -p "$SMOKE_DIR"
gh release download "v$VERSION" --dir "$SMOKE_DIR"
relswap verify \
  --name rozi \
  --manifest "$SMOKE_DIR/rozi-release.json" \
  --signatures "$SMOKE_DIR/rozi-release.signatures.json" \
  --keys release-keys.json \
  --artifacts-dir "$SMOKE_DIR"
```

On disposable hosts for each supported platform family, install the exact version with the public
bootstrap helper. Confirm `rozi --version`, `rozi --help`, and `rozi update --check`. On Windows,
also launch through the stable managed launcher. Do not use a maintainer's normal managed install
as release-test state.

Check that `https://github.com/tui-lipan/rozi/releases/latest` resolves to the new tag and that the
documentation-site installers resolve the expected archive names.

## Signing keys and rotation

The trust anchor is `release-keys.json`, compiled into every binary at build time
(`src/release_app.rs`). That single fact decides everything about rotation: **a binary only ever
trusts the keys that existed when it was built.** A key added today reaches an installed rozi only
through an update signed by a key that binary already carries.

Three consequences follow, and the order of operations below exists because of them:

- Retiring a key in the same release that introduces its replacement strands every install made
  before that release. Those binaries will not accept the new key, because the release carrying it
  is signed with a key they do not trust.
- A key added to the trust store is inert until a release is built with it committed. Committing
  the public half is what puts it into binaries; selecting it in the release environment is what
  starts signing with it. Those are two separate steps, and they belong in two separate releases.
- If the only trusted key is lost or compromised, there is no recovery path through `rozi update`
  at all. Every user has to reinstall by hand, from a channel they trust.

The third is the reason to hold **two** keys before you need them: an active signing key and a
cold spare whose private half never touches CI. Both public halves ship in every binary, so if the
active key has to be abandoned, the next release is signed with the spare and existing installs
accept it. Generating the spare after a compromise is too late — it would not be in anybody's
binary.

Generate a key with the tool pinned by `Cargo.lock`; it requires explicit output paths and refuses
to overwrite either:

```bash
relswap keygen --private release-2027-a.private --public release-2027-a.public
```

Keep the private half offline. Then rotate in this order, one release at a time:

1. Commit the new public key **alongside** every still-supported key in `release-keys.json`. Do not
   remove anything yet. `cargo test` covers the shape of this file, and the release workflow's
   `relswap trust-check` fails closed on an empty or malformed one.
2. Cut a release signed with the **old** key. Its only job is to distribute the new key: after it,
   installs that have updated carry both.
3. Store the new private key in the `release` environment and set `ROZI_RELEASE_KEY_ID` to its id.
   The next release is signed with the new key, and every install from step 2 accepts it.
4. Remove the retired public key only once you are willing to abandon installs that never took
   step 2. There is no telemetry saying how many those are, so prefer leaving a retired key in
   place for several releases; a public key that signs nothing costs nothing.

Treat a compromise differently: abandoning the key is urgent, but the steps do not change, because
nothing can reach a binary except through a release it will accept. Remove the compromised key,
sign with the spare, publish, and say plainly in the release notes that anyone who cannot update
should reinstall. A published release cannot be un-signed, and removing a release does not revoke
a key already trusted by installed binaries.

## Manifest lifetime and expiry

`relswap manifest` writes an `expires_at` into every manifest, and clients refuse one that has
lapsed (allowing 12 hours of clock skew). The window is set explicitly by `MANIFEST_LIFETIME_DAYS`
at the top of `.github/workflows/release.yml`, and the workflow asserts that the signed manifest
actually carries the lifetime it asked for.

It is a two-sided number. A short window bounds a freeze attack, where someone who can keep serving
an old release indefinitely holds users on a version with a known bug. A long window protects
liveness: the day the latest release's manifest lapses, `rozi update` and the startup check begin
failing for every user, with a verification error, whether or not anything is actually wrong.

The current value is 365 days. Shortening it is defensible, but only alongside a release cadence
that reliably beats it — for a single maintainer, a window short enough to matter to an attacker is
also short enough to break every install during one quiet stretch.

## Release health

`.github/workflows/release-health.yml` runs every Monday, and by hand on demand. It holds no
secret, reads only public assets, and asks whether the published release is still one an installed
rozi would accept:

- `/releases/latest` resolves to a `v`-prefixed release tag. Every installer rejects a tag without
  that prefix and `rozi update` reads its metadata from this pointer, so a prerelease or draft that
  somehow became "latest" is a user-visible outage; the rolling `nightly` prerelease sits one wrong
  click from here.
- `relswap trust-check` passes against the committed `release-keys.json`.
- `relswap verify` accepts the manifest, signature, and every archive **as published right now**,
  which is the same check the update engine performs against the bytes a user would download today.
- The manifest version matches the tag, and more than 90 days of validity remain.

A failure is not an emergency by design — the expiry threshold leaves a full quarter to act — but
it is real. Cut a release, or re-sign the current version with a fresh expiry, and confirm the
workflow goes green.

## Nightly builds

`.github/workflows/nightly.yml` publishes a disposable build of master every night. It is not a
release channel and shares nothing with release signing: no protected environment, no private key,
no crates.io token. User-facing behavior is documented in
[Installation and releases](installation.md#nightly-builds).

The workflow runs at 03:17 UTC and can be started by hand with `workflow_dispatch`. It does three
things in order:

1. **Select.** It asks GitHub for the newest successful `ci.yml` run on master, confirms master
   still contains that commit, and compares it with the `commit` recorded in the published
   `rozi-nightly.json`. Identical means master has not moved and nothing is built; the
   `force` dispatch input builds anyway. A red tip falls back to the newest green commit rather
   than publishing something the matrix rejected. Finding *no* green run in the last twenty is a
   job failure, not a skip: it means CI has been red for a long time or the query has stopped
   matching the repository, and a nightly that quietly skips forever looks exactly like one that
   is working. A green commit that is not reachable from master only skips, because a force-push
   or revert produces that for a few hours and it resolves itself.
2. **Build.** The same five targets a release ships, from the selected commit. Linux payloads use
   the same pinned manylinux 2.28 containers and the same `GLIBC_2.28` ceiling, so a nightly runs
   where a release runs. `ROZI_NIGHTLY_COMMIT` and `ROZI_NIGHTLY_BUILT` are compiled into the
   payload, and each one is checked for its own commit stamp before it is packaged. Nothing is
   cached, the way the release matrix caches nothing.
3. **Publish.** It moves the `nightly` tag onto the selected commit, updates one rolling
   prerelease, and uploads the archives, their `.sha256` files, and `rozi-nightly.json` with
   `--clobber`. Asset names carry the target and no version, so download links stay valid and last
   night's assets are replaced rather than accumulated.

A nightly build failing on one target fails the whole night: the publication job checks that all
five archives arrived, so a partial set is never published.

The tag is `nightly`, not `vX.Y.Z`, and the release is always a prerelease. Both matter. The
bootstrap installers resolve `/releases/latest` and reject a tag without a `v` prefix, `rozi
update` selects signed release metadata only, and GitHub never reports a prerelease as the latest
release — so no stable user is moved onto nightly by any of the paths that install rozi. Keep it
that way: an explicit opt-in such as `rozi update --channel nightly` is a separate decision, worth
making only if people start using nightlies.

`ci.yml` triggers on branches and `v*` tags, deliberately not on the moving `nightly` tag. A
nightly is built from a commit the matrix has already passed, so re-running it there would cost a
second full matrix for no new fact.

## Failed release and rollback response

If a workflow fails before publication, inspect the failed job and keep the tag fixed while
rerunning unchanged jobs. If source or packaged bytes must change, make a new release commit and
use a new version. Never replace signed assets under an existing version.

If a published release is defective:

1. Record the tag, workflow run, affected assets, and observed impact.
2. Mark the GitHub release as a draft to stop it being selected as the latest public release while
   the issue is assessed.
3. Tell managed-install users to run `rozi update --rollback` when the retained previous version is
   safe.
4. Yank the matching crates.io version if users should not install it through Cargo. Yanking blocks
   new dependency resolution but does not remove existing downloads.
5. Publish the fix under a higher version with a fresh manifest and signatures.
6. Restore public release visibility only if the original bytes are known to be safe.

Treat a signing-key or release-account compromise as a security incident. Restrict the affected
secret, preserve workflow and publication evidence, and contact
[security@tui-lipan.dev](mailto:security@tui-lipan.dev). Removing a release does not revoke a key
already trusted by installed binaries; moving off a compromised key follows
[Signing keys and rotation](#signing-keys-and-rotation), which is also why a cold spare key is
worth holding before one is needed.
