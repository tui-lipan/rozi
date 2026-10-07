# Reduce CPU use for active TUI panes

This follow-up implements the optimization targets from the
[active TUI investigation](2026-10-07-active-tui.md). It measures a local rozi build with sibling
framework changes. The framework changes still need a release before rozi can consume them through
its normal registry dependency.

## Implementation

The framework keeps snapshot invalidation separate from paint damage. Sparse output reconstructs
only changed styled rows and reuses the others. Resizing, palette changes, scrollback movement,
screen switches, and resets retain full invalidation. Snapshot reads cannot consume damage that a
renderer still needs.

A shared `TextSource` lets fixed-allocation labels refresh their content during painting without
rebuilding the element tree or layout. rozi binds ordinary bar, inset, and in-frame integrated titles
to these sources. Animated OSC titles can then repaint their label and terminal rows together.
Readiness transitions, sidebar-dependent labels, modal overlays, and titles drawn directly on borders,
dividers, or merged seams retain full UI refreshes.

The partial-paint planner combines rows from multiple terminals and live labels. When more than half
the frame's rows need painting, it uses a single full paint instead of walking the tree once per row.

## Measurement context

- rozi base: `8d7c82a80e790076b3e49bd4f59aae85d8c15c03`, plus the recorded working-tree changes.
- Before: released `tui-lipan` 0.20.0. After: sibling framework base `a425f99` plus the optimization
  changes. That base also contains a terminal job-control fix after the 0.20.0 release.
- Local framework checkout: `../tui-lipan`; ignored `.cargo/config.toml` supplies the path override.
  The tracked manifest and lockfile retain the registry dependency until a framework release.
- AMD Ryzen 7 9700X, Linux `7.2.5-4-omarchy`, Rust `1.96.1`.
- CPU is a percentage of one core, calculated from process user/system ticks and monotonic time.
  The desktop remained in use. Graphical host-terminal CPU is excluded.

## Reproduce

Build and save a release binary before and after applying the changes, then run them serially:

```sh
python3 tools/tui-cpu-probe.py /absolute/path/to/before --seconds 10
python3 tools/tui-cpu-probe.py /absolute/path/to/after --seconds 10
cargo bench --bench snapshot_rebuild -- sparse_tui_update \
  --warm-up-time 1 --measurement-time 2 --sample-size 20
```

Do not run Cargo or other CPU-intensive work during measurements. The probe owns its temporary
HOME/XDG roots, session, clients, and workload. It disables animations, shell integration, autosave,
and resurrection; enters the alternate screen; and switches between idle, sparse body updates,
animated titles, their combination, and dense redraws at 10 Hz. Every sample follows one second of
settling. Body updates repeat after title updates to reveal drift. The pane fills a tiled workspace;
the test does not toggle rozi's fullscreen layout flag.

The snapshot benchmark excludes UI composition, painting, transport, and host rendering. It measures
parsing plus snapshot generation after one status-line change in a persistent populated viewport.
These workloads reproduce specific costs; they do not predict a real Codex session's total CPU use.

## Client CPU results

Each value comes from a ten-second sample. Before and after ran serially. An earlier run overlapped
another session's compilation and was discarded; no compiler processes were observed in the repeated
CPU runs. The desktop remained active, so these are directional local results rather than an idle-host
gate. With the host's tick resolution and sample length, one CPU tick is approximately 0.1 percentage
points; differences that small should not be treated as improvements or regressions.

| Viewport | Workload, 10 Hz | Before, one core | After, one core |
| --- | --- | ---: | ---: |
| 200×60 | Idle | 0.0% | 0.0% |
| 200×60 | Sparse body, fixed title | 0.3% | 0.2% |
| 200×60 | Sparse body + animated title | 0.8% | 0.3% |
| 200×60 | Animated title only | 0.9% | 0.3% |
| 200×60 | Sparse body repeat | 0.2% | 0.3% |
| 200×60 | Dense redraw | 1.4% | 0.7% |
| 475×116 | Idle | 0.1% | 0.2% |
| 475×116 | Sparse body, fixed title | 0.5% | 0.3% |
| 475×116 | Sparse body + animated title | 2.3% | 0.2% |
| 475×116 | Animated title only | 2.2% | 0.3% |
| 475×116 | Sparse body repeat | 0.5% | 0.3% |
| 475×116 | Dense redraw | 4.8% | 2.3% |

At 475×116, body-plus-title updates used about 91% less client CPU, title-only updates about 86% less,
and dense redraws about 52% less. At 200×60, body-plus-title updates used about 63% less. Server samples
ranged from 0.1–0.3% before and 0.2–0.3% after; these measurements establish no server CPU saving.
The ten-second samples do not justify predicting an exact percentage reduction in a real session.

## Sparse snapshot results

The before values are the preceding investigation's Criterion estimates. The after values below
come from a repeated run of the already-built benchmark executable with no observed compiler
activity. A first after run overlapped background compilation in its later cases and was discarded.
Both runs used one-second warmup, two-second measurement, and 20 samples per case.

| Viewport | Buffer | Parsing + snapshot before | Parsing + snapshot after | Reduction |
| --- | --- | ---: | ---: | ---: |
| 80×24 | Primary | 13.77 µs | 1.78 µs | 87% |
| 80×24 | Alternate | 13.68 µs | 1.80 µs | 87% |
| 200×60 | Primary | 72.13 µs | 4.37 µs | 94% |
| 200×60 | Alternate | 71.69 µs | 4.36 µs | 94% |
| 320×90 | Primary | 165.04 µs | 6.91 µs | 96% |
| 320×90 | Alternate | 166.75 µs | 7.04 µs | 96% |

The after estimate intervals at 200×60 were 4.34–4.40 µs for primary and 4.32–4.39 µs for alternate.
These are Criterion estimate intervals, not latency percentiles. Parsing-only after estimates were
0.163/0.161 µs at 80×24, 0.252/0.242 µs at 200×60, and 0.326/0.316 µs at 320×90
(primary/alternate). Relative to the before run, their differences range from about −2% to +5%.
The improvement comes from snapshot reuse, with much smaller differences at the parsing boundary.
Snapshot assembly still copies row/span lists and rebuilds plain text and metadata, so it is not
constant-time with respect to viewport size.

## Validation

- rozi: `cargo test` passed, 2,855 tests and two ignored; strict all-target Clippy passed.
  Formatting and diff whitespace checks passed. The optimized release build passed.
- tui-lipan: default tests passed, 2,934 tests and 21 ignored; workspace all-feature tests passed,
  3,813 tests and 25 ignored. Default build, terminal feature check, strict all-target/all-feature
  Clippy, macro formatting, Rust formatting, and diff whitespace checks passed.
- New regression coverage compares incremental snapshots against full reconstruction across
  styles, Unicode, hyperlinks, cursor modes, scrolling, screen switches, resizing, palette changes,
  and resets. Paint tests compare retained frames and host output against full paints for live
  labels, multiple terminals, and caret placement. Dense damage tests assert the full-paint fallback.
- rozi's integration test checks that a title reaches the rendered frame through the paint-only
  path, while a border title still requests full composition. Unit coverage checks sidebar fallback.
- The docs site builds successfully. Python syntax compilation and probe CLI checks passed.
- The real isolated clients produced PNG captures at both viewport sizes after measurement.
  Visual inspection confirmed the updated title, pane border, and dense terminal rows rendered
  correctly. Captures and raw measurements remain temporary artifacts outside version control.

The new public `TextSource` adds an optional source field to framework text types, which now carry
UI-thread state. The framework documentation includes struct-literal and thread-boundary migration
notes. These API changes require a framework minor release before normal registry consumption.
