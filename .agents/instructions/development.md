# Development

## Common commands

```bash
cargo fetch
cargo build
cargo run
cargo check
cargo fmt
cargo test
cargo clippy --all-targets -- -D warnings
```

Run a test by substring or keep the edit loop to one target:

```bash
cargo test spawn_split_direction_follows_focused_tile_aspect
cargo test --lib
cargo test --test pane_suite pane_slide_smoke
```

Session launch examples:

```bash
cargo run -- dev                   # attach or launch the same-name profile
cargo run -- attach dev            # attach only
cargo run -- --session dev         # equivalent named target
cargo run -- --session dev --server
```

Leave the TUI with `prefix d`.

## Dependencies and release builds

After dependency changes, run both policy and vulnerability checks. Install the
tools with `cargo install cargo-deny --locked` and
`cargo install cargo-audit --locked` if needed.

```bash
cargo deny check licenses sources advisories bans
cargo audit
```

```bash
cargo build --release
```

The `rozi-launcher` binary is behind the non-default `windows-launcher` feature, because Cargo
cannot attach a target to a `[[bin]]` and it means nothing outside Windows. `cargo build --release`
therefore does not produce it; the release workflow asks for it when packaging the Windows zip, and
CI's check and lint steps pass the feature so it stays type-checked everywhere. To build it by hand:

```bash
cargo build --release --features windows-launcher --bin rozi-launcher
```

CI is defined in `.github/workflows/ci.yml`. Dependency-resolving build, check, Clippy, and test
commands use `--locked`; manifest and lockfile sources must agree. CI runs on Linux, macOS, and
Windows, on every branch push, every pull request, and every `v*` tag. The release matrix lives in
`.github/workflows/release.yml`.

Code changes belong on a short-lived branch merged through a pull request; `master` is the latest
CI-passed code and the base a release is tagged from. `CONTRIBUTING.md` has the branch conventions.

`.github/workflows/release-health.yml` runs weekly with no secret and verifies the *published*
release the way an installed rozi would: `/releases/latest` resolves to a `v`-tag, the committed
trust store passes `relswap trust-check`, `relswap verify` accepts the live assets, and more than
90 days of manifest validity remain. A signed release stops being valid on its own schedule, so
treat a failure there as release work, not as a flaky job. Signing keys, the manifest lifetime
(`MANIFEST_LIFETIME_DAYS` in `release.yml`) and the rotation order are documented in
`docs/release-process.md`; the trust anchor's shape is covered by tests in `src/release_app.rs`.
Release and nightly archives both carry `actions/attest-build-provenance` attestations, which are
independent of rozi's signing key - do not describe them as a replacement for it.

`.github/workflows/nightly.yml` publishes a disposable build of master every night to one rolling
`nightly` prerelease. It selects the newest master commit with a successful CI run, so it adds no
gate of its own, and it holds no signing secret: nightlies are unsigned and no install or update
path selects them. Its build matrix mirrors the release targets, including the manylinux
containers and the `GLIBC_2.28` ceiling, and it compiles `ROZI_NIGHTLY_COMMIT` and
`ROZI_NIGHTLY_BUILT` into the binary so `rozi --version` names the build. Keep those two lines
`key=value`: the release workflow parses `--version` that way. `docs/release-process.md` covers
the workflow, `docs/installation.md` what a user gets. CI deliberately ignores the moving
`nightly` tag.

One extra job cross-compiles `x86_64-unknown-netbsd`. NetBSD is not supported and nothing runs
there; the job exists because it is the only thing in CI that catches `#[cfg(target_os = "linux")]`
paired with a `not(linux)` fallback, which quietly gives every other Unix the macOS spelling. Write
per-OS `cfg` arms explicitly and let an unlisted target fail loudly, the way `platform::errno` does.
To run it locally you need `cross` and a working Docker, because `ring` builds C and the NetBSD
toolchain lives in the `cross` image:

```bash
cross check --locked --all-targets --target x86_64-unknown-netbsd
cross clippy --locked --all-targets --target x86_64-unknown-netbsd -- -D warnings
```

## Benchmarks and profiling

```bash
cargo check --all-targets
cargo bench
cargo bench --bench terminal_ingest -- 'sgr_heavy/200x60'
cargo build --profile release-debug
samply record ./target/release-debug/rozi profile
```

See `docs/benchmarks.md` for benchmark targets, comparison rules, stress recipes, and profiling
commands. Dated results live under `docs/performance/audits/`.
