# Dense span-assembly profiling

This prototype is not enabled in Rozi. See the [measurement report](../../../docs/performance/audits/2026-09-14-span.md).
It instruments released `tui-lipan 0.9.0` against released `alacritty_terminal 0.26.0`. There is no
engine fork and no compact-history path.

The patch exposes hidden stage functions used only by the bundled Criterion bench. Production
`renderable_content_lines` is unchanged. Alternative builders live beside it so they can be timed
and checked against the current snapshot.

## Reproduce

Run from Rozi, with the cached registry source available:

```bash
python3 tools/experiments/span-assembly/prepare.py --output /tmp/rozi-span-assembly
```

The destination must be new. The script copies cached `tui-lipan 0.9.0` and patches that copy.
It does not change the registry, Rozi, or the sibling framework.

```bash
rozi_root="$PWD"
cd /tmp/rozi-span-assembly/framework
CARGO_BUILD_JOBS=8 cargo test --target-dir "$rozi_root/target" --features terminal --lib widgets::terminal
CARGO_BUILD_JOBS=8 cargo bench --target-dir "$rozi_root/target" --features terminal --bench span_assembly -- --sample-size 20 --measurement-time 2 --warm-up-time 1
cd "$rozi_root"
```

Fixture construction stays outside the timer. `snapshot` marks the screen dirty and rebuilds.
The other stages are read-only on one populated 253×64 terminal with 5,000 history rows. They
measure construction, not backend drawing or a cached snapshot.

`lines_dense_rows`, `lines_attr_keyed`, and `lines_dense_attr` are probes. They must match the
current iterator snapshot in the unit tests. They are not production code.
