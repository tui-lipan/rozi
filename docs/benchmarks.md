# Benchmarks and profiling

hyprmux uses Criterion 0.8 benchmarks to measure terminal parsing, snapshot rebuilding, protocol
framing, and the client session-output path. Run timing benchmarks on an otherwise idle machine;
CI compiles them through `cargo check --all-targets` but does not use shared runners for timing.

## Running benchmarks

Run the complete suite:

```bash
cargo bench
```

Run one Cargo benchmark target:

```bash
cargo bench --bench terminal_ingest
cargo bench --bench snapshot_rebuild
cargo bench --bench protocol_framing
cargo bench --bench session_pipeline
```

Arguments after `--` go to Criterion. Use a benchmark ID substring or regular expression to select
one group, corpus, size, or case:

```bash
cargo bench --bench terminal_ingest -- 'sgr_heavy/200x60'
cargo bench --bench snapshot_rebuild -- terminal_pane_process_server_output
cargo bench --bench protocol_framing -- control_frame_serde
cargo bench --bench session_pipeline -- session_pipeline_memory/4096
```

List the benchmark IDs in a target without measuring them:

```bash
cargo bench --bench terminal_ingest -- --list
```

Criterion writes reports and measurements below `target/criterion/`. Do not commit them.

## Linux process memory matrix

`tools/memory-matrix.sh` is an opt-in process benchmark for release builds on Linux. It measures
proportional set size (PSS) rather than attributing every shared mapping to every process. The quick
matrix covers 80x24 and 253x64 viewports, 1/4/8 panes, empty or 1000-line histories, and plain or
styled output. The full matrix adds 16 panes, 5000-line histories, two clients, pane closing, and a
parked named session:

```bash
tools/memory-matrix.sh --quick
tools/memory-matrix.sh --full --output target/memory-matrix/full
```

Use `--smoke` before a long run to check local PTY, control-socket, `/proc`, and shutdown support.
The runner requires `bash`, `python3`, util-linux `script`, and Linux `smaps_rollup`. It builds
`target/release/hyprmux`, creates private temporary `HOME` and XDG config/state/cache/runtime
directories per scenario, and passes every control command an explicit isolated socket. It never
discovers or connects to the user's normal sessions.

Each pane emits deterministic output and a final marker. After the marker appears, the runner waits
two seconds, takes five `/proc/<pid>/smaps_rollup` and `/proc/<pid>/status` samples 200 ms apart, and
reports the median. Results include separate client, server, and child-process groups; RSS, PSS,
anonymous, private, and file-backed memory; current/high-water RSS; thread/process counts; and
per-pane application-PSS deltas. It writes both `results.json` and `results.md` below the selected
output directory. Probe clients detach and the server receives a protocol shutdown before the
private directory is removed; a trap targets only the PIDs owned by the runner if normal shutdown
fails.

Memory numbers vary with the kernel, allocator, linked libraries, terminal dimensions, and host
load. Compare two runs made from the same build on an otherwise idle machine. Scenario PSS within
the larger of 5% or 2 MiB is considered comparable; investigate larger movement, but do not turn
that tolerance into a CI threshold. The matrix is never run by CI.

The harness changes no memory behavior by itself. Empty panes allocate history lazily, so the
scrollback cases matter only after output fills history. Queue limits likewise should not lower
normal idle RSS; their expected result is a plateau when a writer or client is stalled.

## Comparing a baseline

Criterion 0.8 can save a named baseline and compare a later run against it. Keep the toolchain,
power settings, machine load, and benchmark filter the same between both runs.

```bash
# Before the change
cargo bench -- --save-baseline before

# After the change
cargo bench -- --baseline before
```

The same options work with an individual target or filter:

```bash
cargo bench --bench terminal_ingest -- 'sgr_heavy' --save-baseline before-sgr
# Make the change.
cargo bench --bench terminal_ingest -- 'sgr_heavy' --baseline before-sgr
```

`--save-baseline` replaces an existing baseline with the same name. Use a distinct name when the
old measurement must remain available.

## Deterministic suites

All benchmark input is generated in `benches/support/mod.rs`; no terminal capture is checked in.
The generators produce the same bytes on every run:

| Suite | What it measures |
| --- | --- |
| `terminal_ingest` | `TerminalScreen::process_bytes` throughput for plain log lines, SGR-heavy output, scroll regions and cursor movement, wide Unicode, and long sparse-escape lines at 80x24, 200x60, and 320x90. |
| `snapshot_rebuild` | `render_snapshot()` by screen size, `TerminalPane::process_server_output` at 64 B, 1 KiB, and 64 KiB message sizes, and `output_burst` rebuild-per-message vs rebuild-per-frame. |
| `protocol_framing` | Pane-output encode/decode round trips at 64 B, 4 KiB, and 1 MiB, plus serde of large `Attached` and `LayoutCommitted` control frames. |
| `session_pipeline` | In-memory frame encode, decode, client terminal processing, and snapshot rebuild; Unix also measures a 4 KiB socket-pair path. |
| `app_render` | Whole-app view + expand + layout at 1/2/4/8/16 tiled panes, with and without terminal content. This is the work `Update::full()` adds over `Update::paint()`. |

When changing a generator, treat it as a benchmark-definition change: save a fresh baseline rather
than comparing incompatible corpora.

## Live stress recipes

Microbenchmarks isolate costs; live runs expose scheduling, PTY, rendering, and broadcast behavior.
Build or run in release mode, enlarge the terminal if relevant, and watch CPU and responsiveness
while executing these commands inside a hyprmux pane. Stop unbounded output with `Ctrl-c`.

Continuous line flood:

```bash
yes 'hyprmux output flood 0123456789'
```

Generate and print a 100 MB file:

```bash
yes 'hyprmux 100 MB cat corpus' | head -c 100000000 > /tmp/hyprmux-100mb.txt
cat /tmp/hyprmux-100mb.txt
rm /tmp/hyprmux-100mb.txt
```

One million numbered lines:

```bash
seq 1 1000000
```

To measure broadcast amplification, attach two release clients to the same named session, then run
one of the output producers in a pane. Both clients receive and parse the pane output.

```bash
# Terminal 1
cargo run --release -- stress

# Terminal 2
cargo run --release -- stress
```

Use the same terminal dimensions for controlled comparisons. A follower may be read-only, but it
still receives output:

```bash
cargo run --release -- stress --read-only
```

## Profiling with Samply

The `release-debug` profile keeps release optimizations and debug symbols while disabling symbol
stripping. Install Samply separately, build once, then record the binary directly:

```bash
cargo build --profile release-debug
samply record ./target/release-debug/hyprmux profile
```

Generate a representative workload in the recorded session, then detach or quit to finish the
recording. For a self-contained ephemeral session, omit the `profile` target.

## Known hot-path shape

The current output path deliberately favors a simple authoritative model over minimum parsing:

1. The session server parses each PTY byte stream into its `TerminalScreen` for terminal state,
   metadata, replay, and resurrection behavior.
2. The server broadcasts the raw pane bytes to every attached client.
3. Every client parses those bytes again in `TerminalPane::process_server_output` and rebuilds a
   full render snapshot for each delivered message.

This creates dual parsing with one client, additional parsing for every attached client, and a
full-snapshot cost that depends on message chunking as well as screen size. Compare
`terminal_ingest`, `snapshot_rebuild`, and `session_pipeline` before optimizing this path; batching
or coalescing can improve results without changing parser throughput itself.

## Idle server cost is per pane, and agent detection drives it

An idle pane should cost nothing, so measure the *server* process (not the client) when it does not.
Sample it directly rather than trusting `ps` averages:

```bash
SRV=<server pid>
t0=$(awk '{print $14+$15}' /proc/$SRV/stat); sleep 6
t1=$(awk '{print $14+$15}' /proc/$SRV/stat)
awk -v a=$t0 -v b=$t1 'BEGIN{printf "%.2f%%\n", (b-a)/6}'
```

Add panes over the control socket (`HYPRMUX_SOCKET=… hyprmux new-pane`) and check the slope: a
cost that scales with pane count is per-pane polling, while a flat cost is the server's own loop.

Measured at idle, server process:

| Panes | Detect every poll | Detect on change | Plus shared walk |
| --- | --- | --- | --- |
| 2 | 4.33% | 1.17% | 1.33% |
| 3 | 6.33% | 1.33% | 1.33% |
| 5 | 10.17% | 2.00% | 1.50% |

Marginal cost per pane: **~1.95% → ~0.06%**.

The cause was `foreground_job`, which must examine every process on the host to find a pane's
process-group members, running once per pane at the 250 ms `RUNTIME_POLL_INTERVAL` — so every pane
independently walked the same process table four times a second. Two changes fixed it:

- Detection runs only when the cheaply known foreground program or command phase changes, with
  `AGENT_DETECT_REFRESH` as a periodic re-sweep for a wrapped process appearing inside an unchanged
  foreground.
- When a sweep is needed, [`ProcessScan`] captures the walk once and every pane in that cycle reads
  it, so cost no longer scales with pane count. Capture is lazy: a cycle where nothing is stale
  never walks at all.

Keep both properties when touching this path. The unit tests assert the gate via
`last_agent_detect` rather than via `LazyProcessScan::captured`, because the test pane has no PTY
and the walk is unreachable there either way — a `captured()` assertion would pass vacuously.

[`ProcessScan`]: ../src/platform/process/mod.rs

## Snapshot rebuilds dominate output cost

The expensive part of receiving output is not rendering it — it is rebuilding the render snapshot.
`snapshot_rebuild` puts one rebuild at ~58 µs at 80x24, ~343 µs at 200x60, and ~809 µs at 320x90.
For comparison, the whole app's view and layout for eight tiled panes is ~395 µs.

`terminal_pane_process_server_output` shows the shape of the problem: 64 B and 1 KiB messages cost
almost the same, because the cost is per *message* (one full rebuild) rather than per byte.

`TerminalPane` therefore rebuilds on read rather than on write (`TerminalPane::snapshot`), leaning
on `TerminalScreen`'s own dirty flag. Since the runtime coalesces a burst of server messages into a
single frame, only the last snapshot is ever rendered, and `output_burst` measures what that saves:
`per_message` rebuilds after every message (the old behavior), `per_frame` rebuilds once at the end
(the current one).

| Messages in burst | Rebuild per message | Rebuild per frame | Saved |
| --- | --- | --- | --- |
| 1 | 130 µs | 127 µs | — |
| 8 | 977 µs | 130 µs | 7.5x |
| 32 | 3.92 ms | 143 µs | 27x |
| 128 | 15.9 ms | 417 µs | 38x |

The single-message row matters as much as the others: it shows the work was genuinely removed
rather than relocated. One message has no redundant rebuild to drop, so the two shapes agree.

Two consequences worth preserving:

- Do not reintroduce an eager `render_snapshot()` call on a write path. It reads as harmless
  bookkeeping and silently reinstates one full rebuild per message.
- Prefer `scrollback_offset()` / `total_scrollback_rows()` over reading the same fields off
  `snapshot()`. `process_bytes` keeps those current on its own, so going through the snapshot
  forces a rebuild to read one integer.

## What a full render actually costs

`HyprmuxApp` is the only `Component` in the crate, so every `Update::full()` re-runs `view()` for
every pane, the workbar, and the overlays — there is no smaller subtree to refresh. `app_render`
measures that, on a 200x60 viewport with dwindle-tiled panes (Ryzen, release build):

| Panes | With terminal content | Empty screens |
| --- | --- | --- |
| 1 | 58 µs | 55 µs |
| 2 | 103 µs | 100 µs |
| 4 | 202 µs | 195 µs |
| 8 | 395 µs | 381 µs |
| 16 | 756 µs | 720 µs |

Two things follow, and both argue against scoping renders:

- Cost is linear at roughly **47 µs per pane**, and styled terminal content accounts for only about
  5% of it. The rest is per-pane chrome and tiling structure. Memoizing terminal snapshots — the
  intuitive target — would therefore buy almost nothing.
- Even the whole of it is small. At 8 panes, an `Update::full()` costs ~395 µs of view/layout more
  than an `Update::paint()`. Sustained at the runtime's 60fps ceiling that is ~2.4% of one core.

So splitting panes into child `Component`s with `memo_key()` has a hard ceiling of a few percent of
a core, against a large refactor of `view/pane.rs` and real visual-regression risk. Prefer
eliminating whole frames instead: a frame that never runs saves view, layout, draw, and terminal
I/O, rather than just the view/layout slice measured here.

Note also that a `ctx.transition`-driven color (pane focus chrome) **cannot** be animated by
`Update::paint()`. Property transitions mark the frame full precisely because the interpolated
value only reaches the screen through the next `view()`
(`tui-lipan/src/app/runner/animation_ticker.rs`), while paint-only redraws the existing realized
tree. Focus-chrome animation frames are therefore inherently full frames.

Draw cost is deliberately not benchmarked here: `TestBackend::capture_frame()` allocates a heap
`String` per cell, so it measures the harness, not the real buffer write and frame diff. Read the
devtools metrics panel's `Draw` row for that.

## Local framework dependency

`Cargo.toml` currently points directly to the sibling `../tui-lipan/` checkout, so benchmark and
profile builds require that checkout. The corresponding `Cargo.lock` entry has no registry source
or checksum, which is correct for the current path dependency.

Before a standalone clone, CI job, or release build can resolve `tui-lipan` from crates.io, publish
the required framework version, replace the path dependency with its registry version requirement,
and regenerate `Cargo.lock` without a path override. Do not describe a registry release as active
until that manifest change has landed.
