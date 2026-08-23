# Contributing to rozi

## Requirements and setup

Rozi uses Rust 2024 and requires Rust 1.90 or newer. Install Cargo, `rustfmt`, and Clippy through
rustup, then build the repository:

```bash
git clone https://github.com/tui-lipan/rozi.git
cd rozi
rustup component add rustfmt clippy
cargo fetch
cargo build
```

Run a development session with `cargo run -- dev`. Detach from the TUI with `prefix d`.

The documentation site requires Node.js 22 or newer. Install its locked dependencies before
running or building it:

```bash
cd docs
npm ci
npm run docs:dev
npm run docs:build
```

## Development workflow

Keep the edit loop focused. Run one test by name or one integration-test target while changing
code:

```bash
cargo test spawn_split_direction_follows_focused_tile_aspect
cargo test --lib
cargo test --test pane_suite pane_slide_smoke
```

Use `cargo check` for a quick compile check. Run `cargo build --release` when changing packaging,
startup, platform integration, or other behavior that differs in a release build.

Before opening a pull request for a Rust application change, run the repository baseline from the
repository root:

```bash
cargo fmt --all -- --check
cargo test
cargo clippy --all-targets -- -D warnings
git diff --check
```

CI also compiles all targets and builds a release binary with the lockfile:

```bash
cargo check --locked --all-targets
cargo build --locked --release
```

## Tests and user-directory isolation

Unit tests usually live beside their Rust modules. Integration and smoke tests live under `tests/`.
Add a regression test for a bug fix and test public behavior rather than copying implementation
details.

Tests must not write to the developer's config, state, cache, or runtime directories. An
integration test that constructs `AppRoot` must call
`rozi::test_support::isolate_user_dirs()` before creating its `TestBackend`. Do not use
`std::env::set_var` to redirect `HOME`, `XDG_*`, `APPDATA`, or `ROZI_CONFIG` inside an in-process
test. Environment variables passed to an isolated child process are safe.

Benchmarks are local evidence, not timing assertions. See
[Benchmarks and profiling](docs/benchmarks.md) for the permanent targets and comparison method.

## Documentation

Update user documentation in the same pull request when behavior, CLI flags, configuration,
environment variables, installation, or supported workflows change. `docs/` is both the user
documentation and the VitePress site source. Run `npm run docs:build` from `docs/` after changing
links or site content. Keep generated site output out of the repository.

## Platform changes

CI runs native Rust checks on Linux, macOS, and Windows. New OS-specific behavior belongs under
`src/platform/`. Exercise the affected platform when possible and state which platforms you tested
in the pull request. If you could not test a supported platform, describe the remaining risk
instead of claiming cross-platform verification.

## Dependency changes

Use Cargo to update Rust dependencies and keep `Cargo.toml` and `Cargo.lock` consistent. Rozi
normally resolves `tui-lipan` and `relswap` from crates.io. Do not submit a change that depends on
an uncommitted sibling checkout.

After a dependency change, run:

```bash
cargo deny check licenses sources advisories bans
cargo audit
```

Install missing tools with `cargo install cargo-deny --locked` and
`cargo install cargo-audit --locked`.

## Pull requests

Keep a pull request limited to one coherent change. Explain the user-visible effect and why the
change is needed. Include the exact tests run, documentation changes, platform coverage, and any
dependency updates. Call out checks you could not run and known follow-up work.

Before submission, confirm:

- [ ] Focused tests cover the change.
- [ ] The repository baseline commands pass.
- [ ] User documentation is updated, or the change has no documentation effect.
- [ ] Platform-specific behavior was tested on the affected OS, or the gap is stated.
- [ ] Dependency policy and advisory checks pass when dependencies changed.
- [ ] Every commit has a DCO sign-off.

## License and DCO

Rozi is licensed under [MPL-2.0](LICENSE). Contributions use inbound equals outbound. Unless you
state otherwise, an intentional contribution is licensed under MPL-2.0 with no additional terms.
You retain copyright. Rozi does not require copyright assignment or a CLA.

Every commit must include the [Developer Certificate of Origin](DCO) sign-off:

```bash
git commit -s -m "fix: describe the change"
```

This adds:

```text
Signed-off-by: Your Name <you@example.com>
```

Use your real name and an email that matches the commit author. If a local, unpushed commit is
missing the trailer, add it with `git commit --amend -s --no-edit`. For a series of local commits,
use `git rebase --signoff <base>`. Read the repository's [DCO](DCO) before signing.
