# Release process

This page is for maintainers publishing a signed rozi release. It covers preparation, tagging, what
the Release workflow checks, verification after publication, signing-key rotation, and what to do
when a release fails. Installation and update behavior for users is in
[Installation](installation.md). When this page and `.github/workflows/release.yml` disagree, the
workflow is the source of truth.

## Prerequisites

A release maintainer needs:

- permission to push a tag and approve the GitHub `release` environment;
- Rust 1.90 or newer with Cargo, rustfmt, and Clippy;
- `cargo-audit` and `cargo-deny`;
- the GitHub CLI, for inspecting the release after publication;
- these secrets in the protected `release` environment:
  - `ROZI_RELEASE_PRIVATE_KEY` — the active signing key's private half;
  - `GOOGLE_GENERATIVE_AI_API_KEY` — a Google Gemini API key for release notes;
  - `CARGO_REGISTRY_TOKEN` — a crates.io API token.

The committed `release-keys.json` trust store holds two Ed25519 keys:

- `release-2026-a` is the active signing key. Its private half is `ROZI_RELEASE_PRIVATE_KEY`. The
  `ROZI_RELEASE_KEY_ID` Actions variable may select another committed key; the workflow defaults
  to `release-2026-a`.
- `release-2026-b` is the cold spare. Its private half stays offline and must never be stored in
  GitHub, CI, or this repository. An existing install trusts it only after taking an update signed
  by `release-2026-a` whose binary already includes the spare.

Keep private keys outside the repository and never commit one. The signing job exposes the private
key only as `RELSWAP_RELEASE_PRIVATE_KEY`, and only during the signing step. Build, test, package,
pull-request, and manual workflow runs never receive it. Key generation and rotation are separate
from a normal release; see [Signing keys and rotation](#signing-keys-and-rotation).

To inspect manifests locally, install the `relswap` release tool at the version pinned by
`Cargo.lock`:

```bash
RELSWAP_VERSION=$(
  cargo metadata --locked --format-version 1 |
    python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="relswap"))'
)
cargo install relswap --version "=$RELSWAP_VERSION" --locked --features release-tool
```

## Prepare the release

GitHub Releases is the canonical changelog, including the migrated history through 0.0.22. The
repository intentionally has no `CHANGELOG.md`; do not recreate one or add per-PR note fragments.

1. Choose a semantic version and update `package.version` in `Cargo.toml`.
2. Let Cargo update `Cargo.lock`, and confirm both files describe the same dependency graph.
3. Update user documentation for any release behavior that changed.
4. Confirm the release commit is on `master` and every commit has a DCO sign-off.
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

6. If documentation changed, build the site:

   ```bash
   cd docs
   npm ci
   npm run docs:build
   ```

7. Push the release commit and wait for the normal CI matrix to pass on Linux, macOS, and Windows.

## Tag and run the workflow

Create and push an annotated `v<version>` tag on the reviewed release commit. The `test` line stops
you if the tag and `Cargo.toml` disagree:

```bash
VERSION=0.2.0
test "$(cargo metadata --locked --no-deps --format-version 1 |
  python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')" = "$VERSION"
git tag -a "v$VERSION" -m "rozi $VERSION"
git push origin "v$VERSION"
```

Pushing the tag starts the Release workflow. Never move or reuse a release tag.

The `release` environment can require maintainer approval before signing. Before approving, review
the tag, the commit, the completed package jobs, and the selected key ID.

Other runs of the Release workflow never sign or publish:

- A pull request that changes release infrastructure, `Cargo.toml`, or `Cargo.lock` packages every
  target and checks the release scripts, but leaves the test matrix to CI, which runs it on the same
  commit.
- A manual run also runs the test matrix.
- Ordinary pull requests skip the Release workflow and rely on CI.

### Tag from the GitHub website

A maintainer without a local Git client can create the same tag on the reviewed commit through the
new-release form. Publish it as a **prerelease** so the incomplete release stays out of
`/releases/latest`. The workflow detects the placeholder, uploads only the verified signed bundle,
and promotes it to the latest stable release once publication succeeds. Never publish an empty
stable release before the signing workflow completes.

## What the workflow checks

1. **Source.** Before any release-only work, the workflow proves the tagged commit is reachable
   from `master` and that the exact tagged SHA already has a completed, successful master CI push
   run. Normal CI does not run again for release tags; this carries forward its formatting, Clippy,
   native test, integration, dependency-policy, and NetBSD cross-check results.
2. **Release notes.** In parallel with testing, the release-note job finds the newest published,
   non-draft `v` tag other than the one being built, resolves both tags to exact commits, and proves
   the previous release is an ancestor. It passes every candidate commit's metadata and changed
   paths to the reviewed OpenCode command in `.opencode/commands/changelog.md`. Rosie writes notes
   from each candidate's subject, PR URL, and Summary using `google/gemini-3.5-flash-lite`, without
   reading diffs. Obvious isolated CI, test, documentation, and release-metadata commits are dropped
   conservatively; every other commit remains evidence, even with a `refactor`, `perf`, or `chore`
   prefix.
3. **Note format.** The generated Markdown is rejected unless it contains non-empty `Added`,
   `Changed`, `Fixed`, `Compatibility`, or `Security` sections in that order. The accepted bytes,
   range metadata, and structured agent input are uploaded once as the `release-notes` workflow
   artifact. A generation, permission, model, or validation failure stops the release; there is no
   fallback note style.
4. **Tests.** `cargo test --locked --no-fail-fast` and `cargo check --locked --all-targets` run on
   Linux, macOS, and Windows.
5. **Packaging.** Release archives build for Linux x86-64 and ARM64, macOS x86-64 and ARM64, and
   Windows x86-64. Linux payloads build in pinned manylinux 2.28 containers, and the workflow rejects
   binaries whose ELF version requirements exceed `GLIBC_2.28`.
6. **Payload smoke test.** Each payload reports the tag version and prints help.
7. **Archive smoke test.** Each final archive is extracted and smoke-tested. On Windows this also
   covers launcher version selection, argument and environment forwarding, working-directory
   forwarding, and exit-code propagation.
8. **Pre-signing checks.** The signing job starts only after tests, release-script checks,
   packaging, and note generation succeed. Before reading the private key, it confirms the tag
   version matches `Cargo.toml`, checks every expected archive is present, and runs
   `relswap trust-check` against the committed trust store.
9. **Signing.** The workflow generates the manifest and checksums from the final archive bytes,
   signs the exact manifest bytes, and verifies every archive against `release-keys.json`.
10. **GitHub publication.** The publication job receives the verified bundle and the frozen
    release-note artifact, and has no signing secret. It attests each archive's provenance,
    publishes the generated Markdown unchanged as the release body, and uploads the assets.
11. **crates.io.** After the signed GitHub release exists, a final protected job rechecks the tag
    and runs `cargo publish --locked` with `CARGO_REGISTRY_TOKEN`.

## Release archive and signature contract

The workflow publishes these archives:

```text
rozi-<version>-x86_64-unknown-linux-gnu.tar.gz
rozi-<version>-aarch64-unknown-linux-gnu.tar.gz
rozi-<version>-x86_64-apple-darwin.tar.gz
rozi-<version>-aarch64-apple-darwin.tar.gz
rozi-<version>-x86_64-pc-windows-msvc.zip
```

Each archive has a root directory with the same name. Unix archives contain `rozi`; the Windows
archive contains `rozi.exe` and `rozi-launcher.exe`. Every archive also contains `README.md`,
`LICENSE`, and `examples/`.

The publication bundle also contains:

```text
rozi-release.json
rozi-release.signatures.json
rozi-compatibility.json
<archive>.sha256
```

| File | Purpose |
| --- | --- |
| `rozi-release.json` | Signed manifest (schema version 2) |
| `rozi-release.signatures.json` | Ed25519 signature envelope for the manifest |
| `rozi-compatibility.json` | Extension API and session protocol of the release |
| `<archive>.sha256` | Corruption check for the install scripts |

`rozi-release.json` records the version, publication and expiry times, archive names, SHA-256
values, byte sizes, payload metadata, and Windows launcher metadata. `relswap sign` signs the exact
manifest bytes, so never reformat or reserialize the manifest after signing. `relswap verify`
requires a trusted signature and rechecks the archives and their adjacent checksums.

The workflow derives `rozi-compatibility.json` from the packaged Linux binary's `--version` output.
Clients use it only to choose between an informational update toast and a compatibility warning;
installation still relies on the signed manifest and archive hashes.

The `.sha256` files come from the same release location as the archives, so they detect corruption
but are not an independent authenticity check. Managed updates verify the signed manifest against
the public keys compiled into rozi.

The publication job also attests every archive with `actions/attest-build-provenance`: a Sigstore
signature over the archive's digest, bound to this workflow's OIDC identity and recorded in a public
transparency log. It is independent of rozi's release key, so losing control of that key does not
let anyone forge an attestation. Users can check one before running anything, which the install
scripts cannot do for themselves:

```bash
gh attestation verify <archive> --repo tui-lipan/rozi
```

Nightly archives carry the same attestation, but no manifest and no signature.

Package, verified-bundle, and release-note workflow artifacts are kept for 14 days. The release-note
artifact contains `release-notes.md`, `release-notes-range.json`, and `release-notes-input.md`, so
the accepted output and its exact inputs stay inspectable. The published GitHub release is the
durable public changelog and asset copy.

## Publication

After signing succeeds, the publication job downloads the existing `release-notes` artifact and
passes its `release-notes.md` to `gh release create --notes-file` with `--verify-tag`. If a
website-created prerelease already exists, the job replaces its body from the same file and promotes
it. The job never runs the agent or regenerates notes. It uploads the manifest, signature envelope,
compatibility document, archives, and checksums.

The crate is published to crates.io only after the signed GitHub release succeeds, because crates.io
versions cannot be replaced or deleted. If crates.io publication fails, the GitHub release is still
usable, and you can retry the job without changing its assets.

Confirm the workflow completed rather than relying on the tag push:

```bash
gh run list --workflow Release --limit 5
gh release view "v$VERSION"
```

Check that all five archives, all five checksums, `rozi-release.json`,
`rozi-release.signatures.json`, and `rozi-compatibility.json` are present. crates.io should list the
version only after the final workflow job succeeds.

## Smoke verification after publication

1. Download the public assets into an ignored directory and verify them with the pinned release
   tool:

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

2. On a disposable host for each supported platform family, install the exact version with the
   public install script. Confirm `rozi --version`, `rozi --help`, and `rozi update --check`. On
   Windows, also launch through the stable managed launcher. Do not use your normal managed install
   as release-test state.
3. Check that `https://github.com/tui-lipan/rozi/releases/latest` resolves to the new tag and that
   the documentation-site install scripts resolve the expected archive names.

## Signing keys and rotation

The trust anchor is `release-keys.json`, compiled into every binary at build time
(`src/release_app.rs`). **A binary only trusts the keys that existed when it was built.** A new key
reaches an installed rozi only through an update signed by a key that binary already trusts. That
has three consequences:

- Retiring a key in the same release that introduces its replacement strands every earlier install:
  that release is signed with a key those binaries do not trust.
- A key added to the trust store is inert until a release is built with it committed. Committing
  the public half puts it into binaries; selecting it in the release environment starts signing
  with it. These are two steps in two separate releases.
- If the only trusted key is lost or compromised, `rozi update` has no recovery path. Every user has
  to reinstall by hand from a channel they trust.

That is why the trust store holds an active key and a cold spare whose private half never touches
CI. Both public halves ship in every binary, so if the active key must be abandoned, the next
release is signed with the spare and existing installs accept it. A spare generated after a
compromise is too late, because no installed binary contains it.

Generate a key with the tool pinned by `Cargo.lock`. It requires explicit output paths and refuses
to overwrite either:

```bash
relswap keygen \
  --id release-2027-a \
  --private-key release-2027-a.private \
  --public-key release-2027-a.public.json
```

Keep the private half offline. The spare `release-2026-b` is already in the trust store, so generate
another key only when rotating or replacing that spare. Then rotate one release at a time:

1. Commit the new public key **alongside** every still-supported key in `release-keys.json`. Remove
   nothing yet. `cargo test` covers the shape of this file, and the workflow's `relswap trust-check`
   fails closed on an empty or malformed one.
2. Cut a release signed with the **old** key. Its only job is to distribute the new key; installs
   that take it trust both.
3. Store the new private key in the `release` environment and set `ROZI_RELEASE_KEY_ID` to its ID.
   The next release is signed with the new key, and every install from step 2 accepts it.
4. Remove the retired public key only when you are willing to abandon installs that never took the
   step 2 release. There is no telemetry on how many those are, so prefer leaving a retired key in
   place for several releases; a public key that signs nothing costs nothing.

After a compromise, abandoning the key is urgent but the steps are the same, because nothing reaches
a binary except through a release it accepts. Remove the compromised key, sign with the spare,
publish, and say plainly in the release notes that anyone who cannot update should reinstall. A
published release cannot be unsigned, and removing a release does not revoke a key that installed
binaries already trust.

## Manifest lifetime and expiry

`relswap manifest` writes an `expires_at` into every manifest, and clients refuse a lapsed manifest,
allowing 12 hours of clock skew. `MANIFEST_LIFETIME_DAYS` at the top of
`.github/workflows/release.yml` sets the window, and the workflow asserts that the signed manifest
carries that lifetime.

The current value is 365 days. A shorter window limits how long someone could keep serving an old,
known-buggy release. But the day the latest release's manifest lapses, `rozi update` and the startup
check fail with a verification error for every user. Shorten the window only alongside a release
cadence that reliably beats it.

## Release health

`.github/workflows/release-health.yml` runs every Monday and on demand. It holds no secret, reads
only public assets, and checks that the published release is still one an installed rozi would
accept:

- `/releases/latest` resolves to a `v`-prefixed release tag. Every install script rejects a tag
  without that prefix, and `rozi update` reads its metadata from this pointer, so a prerelease or
  draft that becomes "latest" is a user-visible outage. The rolling `nightly` prerelease is one
  wrong click away.
- `relswap trust-check` passes against the committed `release-keys.json`.
- `relswap verify` accepts the manifest, the signature, and every archive as currently published —
  the same check the updater runs on what a user downloads today.
- The manifest version matches the tag, and more than 90 days of validity remain.

A failure is not an emergency, because the 90-day threshold leaves a full quarter to act, but it is
real release work. Cut a release, or re-sign the current version with a fresh expiry, and confirm
the workflow passes.

## Nightly builds

`.github/workflows/nightly.yml` publishes a disposable build of `master` every night. It is not a
release channel and shares nothing with release signing: no protected environment, no private key,
no crates.io token. What users get is described in
[Installation](installation.md#nightly-builds).

The workflow runs at 03:17 UTC and can be started by hand with `workflow_dispatch`. It runs three
stages:

1. **Select.** It finds the newest successful `ci.yml` run on `master`, confirms `master` still
   contains that commit, and compares it with the `commit` recorded in the published
   `rozi-nightly.json`. If they match, nothing is built; the `force` dispatch input builds anyway.
   A red tip falls back to the newest green commit.
   - No green run among the last 20 fails the job: either CI has been red for a long time or the
     query no longer matches the repository.
   - A green commit that is no longer reachable from `master`, as after a force-push or revert, only
     skips the night. It resolves once CI runs on the new tip.
2. **Build.** It builds the same five targets as a release from the selected commit. Linux payloads
   use the same pinned manylinux 2.28 containers and `GLIBC_2.28` ceiling, so a nightly runs where a
   release runs. `ROZI_NIGHTLY_COMMIT` and `ROZI_NIGHTLY_BUILT` are compiled into each payload, and
   each payload is checked for its own commit stamp before packaging. Nothing is cached, as in the
   release matrix.
3. **Publish.** It moves the `nightly` tag to the selected commit, updates the one rolling
   prerelease, and uploads the archives, their `.sha256` files, and `rozi-nightly.json` with
   `--clobber`. Asset names carry the target and no version, so download links stay valid and each
   night's assets replace the previous ones.

If any target fails, the whole night fails: the publication job checks that all five archives
arrived, so a partial set is never published.

The tag is `nightly`, not `vX.Y.Z`, and the release is always a prerelease. Both keep stable users
off nightlies: the install scripts resolve `/releases/latest` and reject a tag without a `v` prefix,
`rozi update` selects only signed release metadata, and GitHub never reports a prerelease as the
latest release. Keep it that way. An explicit opt-in such as `rozi update --channel nightly` would be
a separate decision.

`ci.yml` runs on pull requests, pushes to `master`, and manual dispatch — not on release tags or the
moving `nightly` tag. The Release workflow owns the tagged test and package matrix, and a nightly is
built from a commit that already passed the `master` matrix.

## Failed release and rollback response

If the workflow fails before publication, inspect the failed job, keep the tag fixed, and rerun the
unchanged jobs. Release-note generation deliberately gates the release: fix the Google API key,
quota, model, or transient API failure and rerun the job. Do not write notes by hand or fall back to
GitHub-generated notes. If source, note policy, generator code, or packaged bytes must change, make
a new release commit with a new version. Never replace signed assets under an existing version.

If a published release is defective:

1. Record the tag, workflow run, affected assets, and observed impact.
2. Mark the GitHub release as a draft, so it stops being selected as the latest public release
   while you assess the issue.
3. Tell managed-install users to run `rozi update --rollback` when the retained previous version is
   safe.
4. Yank the matching crates.io version if users should not install it through Cargo. Yanking blocks
   new dependency resolution but does not remove existing downloads.
5. Publish the fix under a higher version with a fresh manifest and signatures.
6. Restore public visibility of the release only if the original bytes are known to be safe.

Treat a compromised signing key or release account as a security incident. Restrict the affected
secret, preserve workflow and publication evidence, and contact
[security@tui-lipan.dev](mailto:security@tui-lipan.dev). Removing a release does not revoke a key
that installed binaries already trust; move off a compromised key as described in
[Signing keys and rotation](#signing-keys-and-rotation).
