# Render-oriented history reads

This prototype is not enabled in Rozi. See the [measurement report](../../../docs/performance/audits/2026-09-14-render.md).
It keeps the dense viewport / compact history split and changes how snapshots read it.

Live viewport rendering indexes dense rows directly. History traversal stays explicit for
scrollback. Compact rows also expose their prefix and repeated tail so snapshot construction can
copy the prefix and fill the tail instead of indexing every column. Uncompressible rows stay dense.

`ROZI_EXPERIMENT_RENDER` selects the snapshot path when constructing a populated terminal. It is an
experiment selector, not a supported Rozi setting:

| Value | Snapshot construction | `display_iter` at offset 0 |
| --- | --- | --- |
| unset or `bulk` | row parts, prefix copy, tail fill | dense viewport index |
| `dense` | existing cell iterator | dense viewport index |
| `legacy` | existing cell iterator | history-capable `read_cell` |

The conversion algorithm is unchanged. Immediate compaction remains the experimental baseline.

## Reproduce

Run from Rozi, with cached registry sources available:

```bash
python3 tools/experiments/render-history/prepare.py --output /tmp/rozi-render-review
```

The destination must be new. The script reads cached sources and patches copies without changing
the registry, Rozi, or the sibling framework. Apache-2.0 attribution for the engine is retained.

Record the released-dependency baseline first. Save the pre-experiment lockfile; restore only
changes caused by these temporary overrides afterward. Run Cargo commands sequentially.

```bash
cargo bench --bench terminal_history -- 'render_' --save-baseline render-dense --sample-size 20 --measurement-time 2 --warm-up-time 1
```

```bash
ROZI_EXPERIMENT_RENDER=legacy cargo \
  --config 'patch.crates-io.tui-lipan.path="/tmp/rozi-render-review/framework"' \
  --config 'patch.crates-io.alacritty_terminal.path="/tmp/rozi-render-review/engine"' \
  bench --bench terminal_history -- 'render_' --save-baseline render-legacy --sample-size 20 --measurement-time 2 --warm-up-time 1
```

Repeat with `ROZI_EXPERIMENT_RENDER=dense` and `ROZI_EXPERIMENT_RENDER=bulk`. `render_active` is
offset 0, `render_mix` is offset 32, and `render_deep` is offset 5,000. Fixture construction,
scroll positioning, and snapshot destruction stay outside the timer. The cases do not measure
backend drawing or an unchanged cached snapshot.

## Correctness and engine access

```bash
rozi_root="$PWD"
cd /tmp/rozi-render-review/engine
CARGO_BUILD_JOBS=8 cargo test --target-dir "$rozi_root/target"
CARGO_BUILD_JOBS=8 cargo bench --target-dir "$rozi_root/target" --bench history_boundary -- history_render_access --sample-size 20 --measurement-time 2 --warm-up-time 1
cd /tmp/rozi-render-review/framework
CARGO_BUILD_JOBS=8 cargo test --target-dir "$rozi_root/target" --features terminal --lib widgets::terminal
cd "$rozi_root"
```

The engine differential test must resolve the unchanged registry engine as its oracle, so do not
patch that dependency. Isolated framework tests use the prepared tree's `[patch.crates-io]` path
to the sibling engine. Rozi benches ignore that nested patch and need the `--config` overrides
above.
