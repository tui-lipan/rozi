# Compact history prototype

This experiment is **not enabled in Rozi**. It saves substantial short-line history memory but
fails the ingest performance gate. Keep it as a reproducible starting point, not a dependency
patch to ship. See the [investigation](../../../docs/performance/audits/2026-09-10.md).

`engine.patch` applies to the published `alacritty_terminal 0.26.0` crate. It changes row storage
to an exact prefix plus a repeated final cell, compacts rows entering history, and adapts reflow.
Immutable iteration does not expand history. Mutable slice access becomes explicit. It replaces
the upstream row-size-dependent unsafe swap with `Vec::swap`, preserves the serialized grid
format, and adds row tests and a differential test against the unchanged registry engine.
This breaks Alacritty's row range-indexing API and adds a `Clone` bound to `GridCell`.

`framework.patch` contains the two tui-lipan call-site adaptations, based on released `0.9.0`.
Neither patch is a complete release candidate. In particular, a distinct dense viewport type,
bounded row recycling, sustained-ingest measurements, and broader reflow memory measurements
would need investigation before adopting an engine fork. The present shared row type makes
storage checks part of live terminal cell access too.

The engine patch derives from the Apache-2.0 Alacritty sources. The license is included here;
the prepared engine copy also retains its original license and attribution.

## Prepare disposable copies

Requirements: Python 3.11+, Cargo, and `patch`. Run `cargo fetch --locked` in Rozi first if the
two released crates are not cached. The script reads those cached sources and writes only to a
new output directory. It neither edits the Cargo cache nor changes Rozi or the sibling framework.
Use `--engine-source` and `--framework-source` to select explicit pristine source directories.

From the Rozi root:

```bash
python3 tools/experiments/compact-history/prepare.py --output /tmp/rozi-compact-history-review
```

Use a fresh destination name if that directory already exists. A failed preparation leaves its
own output directory available for inspection and does not overwrite an existing one.

## Engine correctness

Run from the prepared engine directory, so local Rozi Cargo overrides cannot replace the dense
oracle dependency. Reuse Rozi's build cache and cap jobs; do not run Cargo commands concurrently.

```bash
rozi_root="$PWD"
cd /tmp/rozi-compact-history-review/engine
CARGO_BUILD_JOBS=8 cargo test --target-dir "$rozi_root/target"
cd "$rozi_root"
```

Start this block in the Rozi root. The differential test compares 400 mixed input and
resize steps against registry Alacritty, including history cells, styles, hyperlinks, Unicode,
cursor positions, and display offset. The original 45 reference recordings remain in the copied
crate and run alongside the unit tests.

## Rozi allocation and timing

From the Rozi root, with its normal registry dependencies, record the baseline first:

```bash
cargo bench --bench terminal_memory
cargo bench --bench terminal_ingest -- --save-baseline dense --sample-size 30 --measurement-time 2 --warm-up-time 1
```

The temporary overrides below change `Cargo.lock`. Save its exact pre-experiment state first and
restore only the source changes caused by the overrides afterward. Do not discard unrelated edits.
No manifest change is needed.

```bash
cargo --config 'patch.crates-io.tui-lipan.path="/tmp/rozi-compact-history-review/framework"' \
  --config 'patch.crates-io.alacritty_terminal.path="/tmp/rozi-compact-history-review/engine"' \
  bench --bench terminal_memory
cargo --config 'patch.crates-io.tui-lipan.path="/tmp/rozi-compact-history-review/framework"' \
  --config 'patch.crates-io.alacritty_terminal.path="/tmp/rozi-compact-history-review/engine"' \
  bench --bench terminal_ingest -- --baseline dense --sample-size 30 --measurement-time 2 --warm-up-time 1
```

Compare identical workload sets and repeat before drawing conclusions. The initial full-matrix
Unicode baseline differed from the later subset baseline, although both comparisons failed the
acceptance gate. The audit reports the paired subset recheck separately. These ingest benchmarks
include consuming and dropping a fresh terminal; they do not isolate steady-state ingest after
history is full.
