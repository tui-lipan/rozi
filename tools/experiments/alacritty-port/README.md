# Compact scrollback port to Alacritty master

This experiment is not used by Rozi. See the
[measurement report](../../../docs/performance/audits/2026-09-15-alacritty-port.md).

`engine.patch` ports the dense viewport / compact history split to Alacritty `master` at
`d692748d3f61253ebe9f5094320120d22f6a046f`, the revision proposed upstream in
alacritty/alacritty#9049. Unlike the earlier engine patches it is opt-in: compaction stays off until
`Grid::set_compact_history(true)`, and negative-line `Grid` indexing keeps working for dense rows.
The patch covers both `alacritty_terminal` and the `alacritty` application. It leaves out
`Cargo.lock`; Cargo regenerates the criterion and `dense` dev-dependency entries. Applied to that
revision, it reproduces the tree of the measured commit `e8e5aee6` except for the lockfile.

`ALACRITTY_COMPACT_SCROLLBACK=1` turns compaction on in the patched Alacritty. It is a measurement
switch, not a configuration option.

Apache-2.0 attribution for Alacritty is retained in `LICENSE-APACHE`.

## Reproduce

Linux only. Build the branch from a clean checkout:

```bash
git clone https://github.com/alacritty/alacritty && cd alacritty
git checkout -b compact-scrollback d692748d3f61253ebe9f5094320120d22f6a046f
git apply /path/to/rozi/tools/experiments/alacritty-port/engine.patch
cargo test -p alacritty_terminal
cargo build --release -p alacritty
```

The baseline is a second worktree at the same revision with only
`alacritty_terminal/benches/scrollback.rs` from the patch and these `alacritty_terminal/Cargo.toml`
additions:

```toml
[dev-dependencies]
criterion = "0.8.2"

[[bench]]
name = "scrollback"
path = "benches/scrollback.rs"
harness = false

[lints.rust]
unexpected_cfgs = { level = "warn", check-cfg = ["cfg(upstream_baseline)"] }
```

Build its benchmarks with `RUSTFLAGS="--cfg upstream_baseline"`, so `scrollback.rs` measures only
the modes the baseline has.

The benchmark workspace, named by `ALACRITTY_BENCH`, holds:

- `bin/alacritty-upstream` and `bin/alacritty-branch`: the two release binaries
- `vtebench/`: a built [vtebench](https://github.com/alacritty/vtebench) checkout
- `extra/short_lines/benchmark`: `vtebench-extra/short_lines/benchmark` from this directory

`ab.sh` alternates upstream, compact, and dense runs in a fullscreen window with
`scrolling.history = 10000`, then fills history with short or full-width lines and samples PSS. It
waits for load average below 2 and no running `rustc` before each run, signals only the Alacritty
processes it starts, and skips runs whose results exist.

```bash
export ALACRITTY_BENCH=$HOME/alacritty-bench
OUT=$ALACRITTY_BENCH/results ROUNDS=3 ./ab.sh
OUT=$ALACRITTY_BENCH/dense-cells ROUNDS=5 SKIP_MEMORY=1 BENCHES=benchmarks/dense_cells ./ab.sh
python3 summary.py "$ALACRITTY_BENCH/results"
```

`crit-ab.sh` interleaves prebuilt Criterion `scrollback` bench binaries and prints medians relative
to the `upstream` label:

```bash
./crit-ab.sh 5 ingest_100k up=/path/to/baseline/scrollback-HASH branch=/path/to/branch/scrollback-HASH
```
