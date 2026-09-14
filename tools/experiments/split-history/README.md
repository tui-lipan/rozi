# Dense viewport and compact history experiment

This prototype is not enabled in Rozi. See the [results and decision](../../../docs/performance/audits/2026-09-13.md). It keeps the original width-sized `Vec<Cell>` row
representation for the viewport and moves completed rows into a separate history deque.
History stores either a dense row or an exact prefix plus a repeated final cell. The repeated
cell retains colors, flags, hyperlinks, and Unicode data. Uncompressible rows stay dense.

Live grid indexing returns `DenseRow`. Historical reads use `read_row` or `read_cell` explicitly;
unusual historical writes use `history_row_mut`, which materializes the row. Read-only history
iteration cannot produce a mutable slice. A per-grid pool retains at most four dense buffers
and four prefix buffers. Changing width releases buffers for the old width.

Reflow moves row ownership without expanding the entire history at once. Compact rows with a
default tail can change width directly when no text needs wrapping. The framework patch adapts
history readers and skips repeated trailing blank cells during text and replay export.

The engine patch derives from Apache-2.0 Alacritty sources. Its license is included. Both patches
are experiments against published `alacritty_terminal 0.26.0` and `tui-lipan 0.9.0`.

## Reproduce

Run from Rozi, with cached registry sources available:

```bash
python3 tools/experiments/split-history/prepare.py --output /tmp/rozi-split-review
cargo bench --bench terminal_history -- --save-baseline split-dense --sample-size 30 --measurement-time 2 --warm-up-time 1
cargo bench --bench terminal_memory
```

Use a new destination if that directory exists. Save the pre-experiment lockfile and restore only
changes caused by these temporary overrides afterward. Run Cargo commands sequentially.

```bash
cargo --config 'patch.crates-io.tui-lipan.path="/tmp/rozi-split-review/framework"' \
  --config 'patch.crates-io.alacritty_terminal.path="/tmp/rozi-split-review/engine"' \
  bench --bench terminal_history -- --baseline split-dense --sample-size 30 --measurement-time 2 --warm-up-time 1
cargo --config 'patch.crates-io.tui-lipan.path="/tmp/rozi-split-review/framework"' \
  --config 'patch.crates-io.alacritty_terminal.path="/tmp/rozi-split-review/engine"' \
  bench --bench terminal_memory
```

`terminal_history` fills 5,000 history rows before timing sustained 10,000/100,000-line bursts.
It also measures text scanning, selection export, replay, and height/narrow/wide resize. The
memory probe reports retained bytes, allocation calls during full-history ingestion, and
resize peaks. These are requested heap bytes, not process RSS or combined server/client memory.

Run engine tests and conversion benchmarks from the prepared engine directory. Its differential
test must resolve the unchanged registry engine as its oracle, so do not patch that dependency.

```bash
rozi_root="$PWD"
cd /tmp/rozi-split-review/engine
CARGO_BUILD_JOBS=8 cargo test --target-dir "$rozi_root/target"
CARGO_BUILD_JOBS=8 cargo bench --target-dir "$rozi_root/target" --bench history_boundary -- --sample-size 30 --measurement-time 2 --warm-up-time 1
cd "$rozi_root"
```

The engine benchmark isolates compaction, pooled expansion, eviction, and resize with/without
reflow. Fixture construction and destruction are outside its timers. Dense-to-compact has separate
cold-pool and warm-pool cases; sustained ingestion measures continuous recycling.
