# Bounded batch compaction experiment

See the [measurement report](../../../docs/performance/audits/2026-09-14.md).

This third experiment builds on the dense viewport / compact history split. It is not enabled
in Rozi. The patches apply to the published `alacritty_terminal 0.26.0` and `tui-lipan 0.9.0`
crates and include the entire prototype, so the previous experiment is not a prerequisite.

The disposable engine reads `ROZI_EXPERIMENT_HISTORY_WINDOW` only when constructing a grid:

| Value | Recent dense rows | Compaction batch | Maximum pending rows | Buffers per pool |
| --- | ---: | ---: | ---: | ---: |
| unset or `0` | 0 | immediate | 0 | 4 |
| `32` | 32 | 16 | 47 | 16 |
| `128` | 128 | 32 | 159 | 32 |

This variable is an experiment selector, not a supported Rozi configuration setting. All compaction
runs synchronously inside the terminal engine. There is no worker thread or Rozi tick integration.
A full batch's freed dense buffers remain in the bounded pool to avoid subsequent allocation churn.
Uncompressible rows remain dense. Resize/reflow follows the split prototype's existing eager
conversion path and resets pending-row bookkeeping. Data and history limits remain authoritative.

`finish_history_compaction` explicitly compacts every pending row, including the recent window.
The settled-ingest benchmark calls it inside every timed iteration, preventing deferred work from
falling outside the measurement. The ordinary sustained benchmark keeps one full-history terminal
alive across iterations; automatic batch work remains inside ingestion there too.

## Prepare

```bash
python3 tools/experiments/batched-history/prepare.py --output /tmp/rozi-batch-review
```

Requires cached released crates, Python 3.11+, Cargo, and `patch`. The destination must be new.
The preparation script reads cached sources and patches copies without changing the registry,
Rozi, or the sibling framework. Apache-2.0 attribution for the engine is retained.

## Correctness and conversion measurements

From Rozi, run Cargo commands sequentially:

```bash
rozi_root="$PWD"
cd /tmp/rozi-batch-review/engine
CARGO_BUILD_JOBS=8 cargo test --target-dir "$rozi_root/target"
ROZI_EXPERIMENT_HISTORY_WINDOW=32 CARGO_BUILD_JOBS=8 cargo test --target-dir "$rozi_root/target"
ROZI_EXPERIMENT_HISTORY_WINDOW=128 CARGO_BUILD_JOBS=8 cargo test --target-dir "$rozi_root/target"
CARGO_BUILD_JOBS=8 cargo bench --target-dir "$rozi_root/target" --bench conversion_cost -- --sample-size 30 --measurement-time 2 --warm-up-time 1
CARGO_BUILD_JOBS=8 cargo bench --target-dir "$rozi_root/target" --bench policy_probe
ROZI_EXPERIMENT_HISTORY_WINDOW=32 CARGO_BUILD_JOBS=8 cargo bench --target-dir "$rozi_root/target" --bench policy_probe
ROZI_EXPERIMENT_HISTORY_WINDOW=128 CARGO_BUILD_JOBS=8 cargo bench --target-dir "$rozi_root/target" --bench policy_probe
cd "$rozi_root"
```

`conversion_cost` measures whole conversion and controlled component operations for 0/8/32/80/253
meaningful cells, with plain, Unicode, and styled-tail cases. Component results are not additive
cycle attribution: they include their own benchmark/ownership overhead. `policy_probe` counts
requested heap bytes and allocation calls during 100,000 further lines after warm-up. It reports
per-line p50/p99/max for regular versus batch-triggering calls and times draining the remaining
pending rows. Maximum latency is one observed maximum, not a scheduling guarantee.

## Framework and Rozi measurements

Record the released-dependency baseline first:

```bash
cargo bench --bench terminal_history -- 'sustained|render_active' --save-baseline batch-dense --sample-size 30 --measurement-time 2 --warm-up-time 1
```

Save the pre-experiment lockfile first. The temporary overrides below rewrite dependency sources;
restore only those changes afterward. Repeat with selectors `0`, `32`, and `128`, saving separate
baseline names. The selector changes behavior without recompilation.

```bash
ROZI_EXPERIMENT_HISTORY_WINDOW=32 cargo \
  --config 'patch.crates-io.tui-lipan.path="/tmp/rozi-batch-review/framework"' \
  --config 'patch.crates-io.alacritty_terminal.path="/tmp/rozi-batch-review/engine"' \
  bench --bench terminal_history -- 'sustained|render_active' --save-baseline batch-32 --sample-size 30 --measurement-time 2 --warm-up-time 1
```

For a separate comparison that drains all deferred work inside each timed iteration:

```bash
ROZI_EXPERIMENT_HISTORY_WINDOW=32 CARGO_BUILD_JOBS=8 cargo \
  --config 'patch.crates-io.alacritty_terminal.path="/tmp/rozi-batch-review/engine"' \
  bench --manifest-path /tmp/rozi-batch-review/framework/Cargo.toml \
  --target-dir "$PWD/target" --features terminal --bench policy_ingest \
  -- --save-baseline batch-32 --sample-size 30 --measurement-time 2 --warm-up-time 1
```

Repeat for `0` and `128`. This isolated framework benchmark has its own dependency resolution;
compare policies within that executable, not absolute times against Rozi's benchmark executable.
Active rendering uses an actual framework snapshot with a populated viewport and history. It
excludes fixture construction and snapshot destruction, and does not measure backend drawing.
