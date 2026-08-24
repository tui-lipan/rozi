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
Windows. The release matrix lives in `.github/workflows/release.yml`.

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
