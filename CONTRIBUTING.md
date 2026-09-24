# Contributing to rozi

This guide covers setting up a development build of `rozi`, the branch and pull request workflow,
the checks a change must pass, and the DCO sign-off every commit needs.

## Requirements and setup

rozi uses Rust 2024 and requires Rust 1.90 or newer. Install Cargo, `rustfmt`, and Clippy through
rustup, then build the repository:

```bash
git clone https://github.com/tui-lipan/rozi.git
cd rozi
rustup component add rustfmt clippy
cargo fetch
cargo build
```

Optionally enable the repository's Git hooks. They add your DCO sign-off automatically and refuse a
`Cargo.lock` that has lost its registry sources:

```bash
git config core.hooksPath .githooks
```

Start a development session with `cargo run -- dev`. Detach from the TUI with `Ctrl+A`, then `d`.

The documentation site requires Node.js 22 or newer. Install its locked dependencies, then run or
build it:

```bash
cd docs
npm ci
npm run docs:dev
npm run docs:build
```

## Branches and pull requests

`master` is the latest code that has passed CI and is expected to work. It is not the stable
release: the `v*` tags are, and each signed release is cut from a commit on `master`. Keep `master`
in a state you would be willing to tag.

Work on a short-lived branch named for the kind of change, and merge it through a pull request:

```text
feat/headless-control
fix/ssh-terminal-restore
perf/session-memory
```

- Push the branch, open a pull request, and let CI finish before merging. Squash or rebase onto
  `master`, then delete the branch.
- Send code changes through a branch even when you merge them yourself. A typo fix or a docs-only
  change can go straight to `master`.
- There is no `develop` branch. Do not create long-lived integration branches.

The required CI gate is the core matrix on Linux, macOS, and Windows: formatting, Clippy, tests, and
a release-profile check. Slower checks stay advisory unless the change touches what they measure;
benchmarks are evidence, not a merge gate.

Nightly builds are produced from `master` after CI passes, so anything merged reaches the next
nightly and whoever is testing it. See [Nightly builds](docs/installation.md#nightly-builds).

## Development workflow

Keep the edit loop focused. While changing code, run one test by name or one integration-test
target:

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

CI runs the same checks with the lockfile enforced, lints the Windows launcher, and checks the
release profile on pull requests:

```bash
cargo clippy --locked --all-targets --features windows-launcher -- -D warnings
cargo test --locked --no-fail-fast
cargo check --locked --release
```

## Tests and user-directory isolation

Unit tests usually live beside their Rust modules. Integration and smoke tests live under `tests/`.
Add a regression test for a bug fix, and test public behavior rather than copying implementation
details.

Tests must not write to the developer's config, state, cache, or runtime directories:

- An integration test that constructs `AppRoot` must call `rozi::test_support::isolate_user_dirs()`
  before creating its `TestBackend`.
- Do not use `std::env::set_var` to redirect `HOME`, `XDG_*`, `APPDATA`, or `ROZI_CONFIG` inside an
  in-process test. Passing environment variables to an isolated child process is safe.

Benchmarks are local evidence, not timing assertions. See
[Benchmarks and profiling](docs/benchmarks.md) for the targets and comparison method.

## Documentation

Update user documentation in the same pull request when behavior, CLI flags, configuration,
environment variables, installation, or supported workflows change. `docs/` is both the user
documentation and the VitePress site source. Run `npm run docs:build` from `docs/` after changing
links or site content, and keep generated site output out of the repository.

## Platform changes

CI runs native Rust checks on Linux, macOS, and Windows. New OS-specific behavior belongs under
`src/platform/`. Test on the affected platform when you can, and state in the pull request which
platforms you tested. If you could not test a supported platform, describe the remaining risk
instead of claiming cross-platform verification.

## Dependency changes

Update Rust dependencies with Cargo, and keep `Cargo.toml` and `Cargo.lock` consistent. rozi
resolves `tui-lipan` and `relswap` from crates.io; do not submit a change that depends on an
uncommitted sibling checkout.

After a dependency change, run:

```bash
cargo deny check licenses sources advisories bans
cargo audit
```

Install missing tools with `cargo install cargo-deny --locked` and
`cargo install cargo-audit --locked --version 0.22.2`.

## Pull requests

Keep each pull request to one coherent change. Explain the user-visible effect and why the change
is needed. List the exact tests you ran, documentation changes, platform coverage, and any
dependency updates. Call out checks you could not run and known follow-up work.

Before submitting, confirm:

- [ ] Focused tests cover the change.
- [ ] The repository baseline commands pass.
- [ ] User documentation is updated, or the change has no documentation effect.
- [ ] Platform-specific behavior was tested on the affected OS, or the gap is stated.
- [ ] Dependency policy and advisory checks pass when dependencies changed.
- [ ] Every commit has a DCO sign-off.

## License and DCO

rozi is licensed under [MPL-2.0](LICENSE), and contributions use inbound equals outbound: unless you
state otherwise, an intentional contribution is licensed under MPL-2.0 with no additional terms. You
keep your copyright. rozi does not require copyright assignment or a CLA.

Every commit must carry a [Developer Certificate of Origin](DCO) sign-off. Read the [DCO](DCO) before
signing. With the Git hooks enabled, the trailer is added for you from your `user.name` and
`user.email`. Otherwise, sign off with `-s`:

```bash
git commit -s -m "fix: describe the change"
```

This adds:

```text
Signed-off-by: Your Name <you@example.com>
```

Use your real name and an email that matches the commit author. To add a missing trailer to a
local, unpushed commit, run `git commit --amend -s --no-edit`. For a series of local commits, use
`git rebase --signoff <base>`.
