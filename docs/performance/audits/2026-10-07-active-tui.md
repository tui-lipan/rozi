# CPU cost of active TUI panes and animated window titles

This investigation separates terminal status-line updates from window-title updates. It finds two
concrete optimization targets: title changes bypass terminal damage rendering, and dirty snapshots
rebuild every visible row even when one character changes. No runtime optimization is shipped by
this audit.

## Measurement context

- Source: rozi `8d7c82a80e790076b3e49bd4f59aae85d8c15c03`, initially clean; this audit adds
  benchmark coverage and documentation only. Released `tui-lipan` 0.20.0, with no path override.
- Host: AMD Ryzen 7 9700X, Linux `7.2.5-4-omarchy`, Rust `1.96.1`.
- The desktop was in use. These are local directional measurements, not an idle-host performance
  gate. No before/after runtime improvement is claimed.
- CPU percentages mean a percentage of **one core**. Process CPU comes from deltas of
  `/proc/<pid>/stat` user and system ticks divided by elapsed monotonic time.
- Samply attach was blocked by `kernel.perf_event_paranoid=2`. The investigation did not change
  that machine-wide policy. Source inspection identifies paths, not sampled percentage attribution.

## Live Codex observation

The installed rozi 0.0.29 client displayed a 475×116 viewport containing an active Codex pane of
189×66 cells. The pane was floating rather than fullscreen. In 100 read-only pane-list samples
spaced approximately 100 ms apart, its OSC title changed 97 times over 10.41 seconds. The title
included an animated spinner. During the same window:

| Process | CPU, one core |
| --- | ---: |
| rozi client | 10.47% |
| rozi session server | 0.67% |
| active Codex process | 2.69% |
| Ghostty, process-wide | 24.39% |

This is an uncontrolled observation, with pane-list polling and other desktop activity. The
installed binary's exact source revision was not established. Ghostty's process includes its other
windows, so its figure cannot be attributed to this pane. The observation supports testing frequent
title changes; it does not establish that all the measured client cost comes from them.

## Controlled title experiment

A release build of the audited checkout ran in an isolated HOME/XDG root, with a separate named
server and a `script`-hosted client. The host output was drained into a temporary file, so this
measures rozi CPU without a graphical terminal's rendering cost. Animations, shell integration,
autosave, and resurrection were disabled. One Python workload entered the alternate screen,
filled 49 short text rows, hid the cursor, and then ran at 10 Hz.

Each mode settled for one second before a 15-second sample. Body mode ran again after title mode
to check drift. No Cargo commands ran during this experiment.

| Client viewport | Idle client | Body, fixed title | Body + animated title | Animated title only | Body repeat |
| --- | ---: | ---: | ---: | ---: | ---: |
| 200×60 | 0.13% | 0.33% | 0.87% | 0.87% | 0.33% |
| 475×116 | 0.07% | 0.60% | 2.47% | 2.33% | 0.67% |

The session server remained between 0.13% and 0.20% of one core across these samples. The title
spinner increases large-window client CPU by about four times relative to the same body spinner
with a fixed title. This is a workload comparison, not a shipped optimization or a prediction of
the saving in a real Codex session. The simple tiled harness also does not reproduce the live
floating pane's backdrop and other desktop activity, so it does not fully explain its 10.47%.

To reproduce using the isolation and client-launch pattern in `tools/memory-matrix.sh`:

1. Build with `cargo build --locked --release`. Create a private HOME and all XDG directories,
   clear inherited `ROZI*` control context in the child environment, and disable the features above.
2. Start an owned named server, attach a `script` client at the selected dimensions, and keep the
   wrapper's stdin open. Wait until the server lists its pane before resolving its id. Wait for a
   recognizable workload marker using the server's `capture-pane --wait-for` interface.
3. Populate the alternate screen once. Every 100 ms alternate `|` and `/`. Body mode writes
   `CSI 50;1H`, the spinner plus ` working`, and `CSI K`. Title mode writes
   `OSC 2 ; <spinner> Investigate CPU | project BEL`. Both mode writes both in one flush;
   idle mode emits nothing. Change modes through a scratch file to keep the process and pane alive.
4. Sample client and server CPU ticks over each 15-second window. Repeat body mode last. Reattach
   the client at the second viewport size and repeat the sequence.
5. Kill only the owned named session and wrapper, then remove the synthetic scratch data.

This experiment uses a pane filling the tiled workspace; it does not toggle rozi's fullscreen
layout flag. Its primary variable is the client viewport and the kind of guest output.

## Sparse status-line benchmark

The new `sparse_tui_update` cases in `benches/snapshot_rebuild.rs` keep a populated pane alive,
alternating a spinner character on its last row with cursor positioning and erase-to-end-of-line.
The pane has 5,000 lines of history capacity; no history is populated or grown. Setup is outside the
timing. The snapshot boundary includes parsing plus `TerminalPane::snapshot()`, and excludes all UI
tree work, painting, diffing, transport, and host terminal work.

```sh
cargo bench --locked --bench snapshot_rebuild -- sparse_tui_update \
  --warm-up-time 1 --measurement-time 2 --sample-size 20
```

Criterion point estimates from this run:

| Viewport | Primary ingest | Alternate ingest | Primary ingest + snapshot | Alternate ingest + snapshot |
| --- | ---: | ---: | ---: | ---: |
| 80×24 | 0.164 µs | 0.161 µs | 13.77 µs | 13.68 µs |
| 200×60 | 0.241 µs | 0.240 µs | 72.13 µs | 71.69 µs |
| 320×90 | 0.321 µs | 0.323 µs | 165.04 µs | 166.75 µs |

The 200×60 primary snapshot interval was 72.03–72.24 µs; the corresponding alternate interval
was 71.61–71.80 µs. These are Criterion estimate intervals, not request-latency percentiles.
Primary and alternate buffers have essentially the same cost in this workload. The snapshot cost
tracks visible grid area. At 60 updates/s, a 72 µs snapshot alone accounts for about 0.43% of one
core, before drawing or any other work; it cannot explain a 10% client reading by itself.

## Why these paths cost more

`TerminalPane::process_server_output` in `src/pane/mod.rs` compares the parsed title with the
previous title and returns `OutputFrame::Rebuild` for any change. `output_frame_update` in
`src/update/session/pane_events.rs` maps that to `Update::full()`. Thus a title spinner forces a new
root view and ordinary drawing, even if the terminal body is unchanged or only one cell moves.
The title must still be updated correctly: replacing this with `Update::terminal_paint()` would
leave the titlebar stale.

With a stable title, visible terminal-only output already uses `Update::terminal_paint()`.
`tui-lipan` can repaint damaged viewport rows. However, before choosing those rows,
`refresh_from_live_screen` pulls a snapshot, and `TerminalScreen::render_snapshot` rebuilds the
entire visible text, styled spans, wrap flags, and hyperlink data whenever its cache is dirty.
Row damage currently saves painting work without saving this snapshot construction.

The framework's damage planner also falls back to ordinary painting when several terminals change
in one frame, when damage is full, or for supported compositing restrictions such as overlays and
images. Those are additional candidates, established by source inspection here rather than by a
multi-pane CPU experiment. A TUI that clears and redraws the entire screen creates different damage
from this one-row workload and needs its own measurement.

## Recommended implementation order

1. **Make title and other local chrome changes cheap.** Add framework support for rebuilding and
   repainting the affected component/region while retaining the rest of the frame, then use it for
   rozi's pane title. Preserve terminal damage and repaint both the title and changed body rows.
   Verify overlays, merged title seams, clipping, cursor placement, and transitions against a full
   render. This addresses the title-spinner path without dropping title updates.
2. **Reuse unchanged snapshot rows in tui-lipan.** Keep snapshot invalidation separate from the
   damage consumed by painting; reconstruct only affected rows and reuse their styled data. Define
   full invalidation for resize/reflow, screen swaps, scrolling, palette changes, images, and
   hyperlinks. Preserve the public snapshot's text and selection semantics. Compare with the new
   benchmark and validate against full snapshots for Unicode, wraps, and styling.
3. **Support damage from multiple terminals in one frame.** Union affected physical rows and use
   the existing production rendering context, retaining safe compositing fallbacks. Measure two
   simultaneous spinners separately before claiming a CPU saving.

Reusable rendering and snapshot changes belong in `tui-lipan`. Rozi should consume a released
framework version, as required by the repository's framework guide. This audit leaves both the
framework checkout and runtime behavior unchanged.

Reducing scrollback is not supported by this evidence as a solution: the measured sparse workload
never walks history. Lowering the frame-rate ceiling only helps when updates exceed that ceiling;
it does not address a title changing around 10 times/s with a 120 FPS ceiling. Hiding or overriding
the visible pane title alone also does not remove the raw-title comparison in the current output
handler.

## Validation

Passed:

- `cargo fmt --all -- --check`
- `cargo build --locked --release`
- The 12 new `sparse_tui_update` Criterion cases with the command above
- `cargo clippy --locked --release --bench snapshot_rebuild -- -D warnings`
- `git diff --check`
- Isolated title/body CPU experiment; owned clients and session server shut down afterward

Build, benchmark, and lint commands used
`CARGO_TARGET_DIR=/home/razuer/Projects/rozi/target` to reuse local build artifacts; there was no
concurrent Cargo invocation. Full application tests and all-target Clippy were not run: the only
Rust change adds benchmark cases, with no application or framework source changes. Sampled
profiling was unavailable under the host policy described above.
