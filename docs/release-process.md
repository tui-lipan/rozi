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

Key generation and rotation are separate from a normal release. `relswap keygen` requires explicit
private and public output paths and refuses to overwrite either. Review any trust-store change,
preserve still-supported public keys, and commit the public document before selecting the new key
in the release environment. Never commit a private key.

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
7. The GitHub publication job receives only the verified bundle. It has no signing secret.
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
<archive>.sha256
```

`rozi-release.json` uses schema version 2 and records the version, publication and expiry times,
target archive names, SHA-256 values, byte sizes, payload metadata, and Windows launcher metadata.
`relswap sign` signs the exact manifest bytes with Ed25519. Do not reformat or reserialize the
manifest after signing. `relswap verify` requires a trusted signature and rechecks archives and
adjacent checksums.

The adjacent `.sha256` files let bootstrap installers detect corruption. They come from the same
release location as the archives, so they are not an independent authenticity check. Managed
updates verify the signed manifest against the public keys compiled into Rozi.

Package and verified-bundle workflow artifacts are retained for 14 days. The published GitHub
release is the durable public copy.

## Publication

After signing succeeds, the GitHub publication job runs `gh release create` with `--verify-tag`
and generated release notes. It uploads only the verified manifest, signature envelope, archives,
and checksums.

The workflow publishes the crate to crates.io only after the signed GitHub release succeeds.
crates.io versions cannot be replaced or deleted. A publication failure leaves the GitHub release
usable and can be retried without changing its assets.

Confirm the workflow completed rather than relying on the tag push alone:

```bash
gh run list --workflow Release --limit 5
gh release view "v$VERSION"
```

Check that all five archives, all five adjacent checksums, `rozi-release.json`, and
`rozi-release.signatures.json` are present. Confirm that crates.io lists the same version only after
the final workflow job succeeds.

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
already trusted by installed binaries. A trust-store update and replacement release need a
separate reviewed response.
