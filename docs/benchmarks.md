# Benchmarks and profiling

This page is for contributors measuring rozi's performance. It lists the benchmark targets, how to
compare runs, the Linux memory harness, and how to profile. Benchmark definitions live in
`benches/` and the memory runner in `tools/memory-matrix.sh`. Timing results belong in dated
[performance audit reports](performance/README.md), not on this page.

Before you measure:

- Use an idle machine with a stable power profile. Keep the Rust toolchain, source revision,
  benchmark filter, terminal dimensions, power settings, and host load the same between runs.
- Record the exact revision and whether the worktree was dirty with any reported result.
- Do not commit generated output. Criterion writes below `target/criterion/`, and the memory harness
  writes below `target/`.

CI compiles every benchmark target through `cargo clippy --locked --all-targets` but never uses its
shared runners for timing decisions. Benchmarks are evidence, not a merge gate.

## Criterion commands

Compile all benchmark targets:

```bash
cargo check --all-targets
```

Run the complete Criterion suite:

```bash
cargo bench
```

Run one target:

```bash
cargo bench --bench terminal_ingest
cargo bench --bench terminal_history
cargo bench --bench snapshot_rebuild
cargo bench --bench protocol_framing
cargo bench --bench session_pipeline
cargo bench --bench app_render
cargo bench --bench scrollback_search
cargo bench --bench server_fairness
cargo bench --bench terminal_memory
cargo bench --bench replay_export
```

Arguments after `--` select Criterion benchmark IDs or a target-specific evidence mode:

```bash
cargo bench --bench terminal_ingest -- 'sgr_heavy/200x60'
cargo bench --bench snapshot_rebuild -- terminal_pane_process_server_output
cargo bench --bench snapshot_rebuild -- sparse_tui_update
cargo bench --bench protocol_framing -- control_frame_serde
cargo bench --bench session_pipeline -- session_pipeline_memory/4096
cargo bench --bench app_render -- 'app_render/view_layout/(8|16)|sidebar_render|inbound_drain'
cargo bench --bench scrollback_search -- 'full_slice|sparse/(1|8|16)'
cargo bench --bench server_fairness -- continuous_pty_ingress
cargo bench --bench server_fairness -- resurrection_snapshot
cargo bench --bench server_fairness -- --idle-latency-probe
cargo bench --bench server_fairness -- --saturation-probe
```

List IDs without measuring them:

```bash
cargo bench --bench terminal_ingest -- --list
```

## Target definitions

| Target | Definition |
| --- | --- |
| `terminal_ingest` | `TerminalScreen::process_bytes` throughput for generated plain lines, SGR-heavy output, scroll regions and cursor movement, wide Unicode, and long sparse-escape lines at fixed viewport sizes. |
| `terminal_history` | Starts with 5,000 populated history rows and ingests repeated 10,000- and 100,000-line bursts into the same terminal. Separately measures snapshot construction at the live view, a mixed scroll offset, and deep scrollback, plus history text scanning, selection export, streaming replay, and height, narrow, and wide resizes. |
| `snapshot_rebuild` | `render_snapshot()` by viewport, server-output processing by message size, the difference between rebuilding after every message and once per output burst, and steady status-line updates in populated primary and alternate screens (ingest alone versus ingest plus snapshot). |
| `protocol_framing` | Pane-output frame encode and decode round trips, and serde for generated large control frames. |
| `session_pipeline` | In-memory frame encode, decode, client terminal processing, and snapshot rebuilding. Unix also includes a socket-pair case. |
| `app_render` | Whole-application view expansion and layout by pane count, with empty and populated terminals; fixed sidebar states; repository-size fixtures; per-message update overhead; and real inbound-mailbox draining for round-robin multi-pane output after pane-aware coalescing. |
| `scrollback_search` | Complete searches across fixed pane and history counts, scanner slices, and full production mapping for one cooperative slice. Cases cover sparse, dense, and absent matches. |
| `server_fairness` | Key acknowledgement through a real server-owned PTY under paced continuous ingress, idle-settled key latency, durable resurrection snapshot attempts, and a one-shot bounded saturation probe. |
| `terminal_memory` | Retained allocations for a populated production client pane at adjacent scrollback capacities. Compares production and direct constructors, collected and spooled replay, sustained-ingest allocation calls, resize peaks, and dense styled history. |
| `replay_export` | Exporting a 32×120 terminal with 1,000 history rows as replay bytes, collected into a `Vec` or written through a file spool and read back. |

### Probe targets

These targets are process probes rather than Criterion timing benchmarks:

- `alloc_probe` counts the allocations of one view, expand, and layout pass, using the same setup
  as `app_render/view_layout`. Counts are exact and repeat between runs. Run it with
  `cargo bench --bench alloc_probe`.
- `replay_memory` holds one large terminal replay in either a `Vec` or rozi's file spool, for
  external memory accounting. Build it with `cargo build --release --bench replay_memory`, then run
  `target/release/deps/replay_memory-<hash> <vec|spool> <control-dir>`. The process writes `ready`,
  waits for `go`, exports and writes `done`, then waits for `stop`.

## Measurement boundaries

### `terminal_history` cases

The sustained history cases exclude initial filling and terminal destruction from timing. Boundary
resize cases also exclude fixture construction and destruction. Text scanning uses the framework's
line iterator with an absent literal query.

Snapshot cases render a fresh populated 253×64 viewport with 5,000 history rows: `render_active` at
offset 0, `render_mix` at offset 32, and `render_deep` at offset 5,000. Scroll positioning, fixture
setup, and snapshot destruction stay outside those timers. They do not measure backend drawing or
cached unchanged snapshots. Use `scrollback_search` for rozi's complete search path.

### `snapshot_rebuild` sparse TUI cases

`sparse_tui_update` keeps one pane alive with 5,000 lines of scrollback capacity and a populated
viewport. Setup and the first snapshot are outside timing. Each iteration alternates one spinner
character on the last row, with cursor positioning and erase-to-end-of-line, without scrolling.
`ingest` measures client parsing and output metadata handling; `snapshot` adds the snapshot read
that the live terminal widget performs before drawing. Both primary and alternate screens use the
same workload at 80×24, 200×60, and 320×90.

These cases exclude UI tree rebuilding, painting, frame diffing, transport, and host-terminal work.
They do not measure CPU usage of a running Codex session or the impact of animated window titles.

### `app_render` cases

The drain cases exercise small-frame overhead and the soft byte budget.
`inbound_fairness/hot_plus_quiet` times draining one 64 KiB hot aggregation plus a quiet pane's
frame, which is the quiet pane's extra wait from non-adjacent coalescing. `app_render` does not
measure backend drawing or terminal buffer diffing.

### `terminal_memory` cases

`terminal_memory` reports requested-heap evidence, not process RSS or timing. The lifecycle memory
probe keeps allocation tracking active across construction and all stages, so freeing pre-existing
terminal buffers cannot produce an invalid negative live-byte count.

### `server_fairness` probes

The saturation probe is not a Criterion latency statistic. It checks the configured PTY ingress
high-water behavior under two unpaced producers, with a continuously draining client and a second
client that stops reading. It verifies bounded queues and output shedding, acknowledges input on
the healthy client, then pauses the producers and resumes the slow client to verify replay recovery
without disconnecting either client. Reported recovery time is one observation, not a latency
percentile. Keep its result separate from the paced key-acknowledgement benchmark.

The idle-latency probe takes 200 key round trips, allowing 50–66 ms of deterministically
phase-jittered quiescence before each one, and reports p50, p95, p99, and maximum latency. Run it
on the same dedicated host before and after a server-wait change.

The resurrection cases report the server's complete durable snapshot attempt. Trigger and polling
delay stay outside the sample. The benchmark also emits server-loop blocking data, which has a
different boundary from whole-attempt duration. The fixture disables foreground-command restoration
so asynchronous process discovery cannot dirty a second snapshot during a terminal-only sample.
Use this setting for comparable terminal snapshot measurements.

### Estimates and percentiles

Criterion estimate intervals describe uncertainty around an estimate; they are not request-latency
percentiles. Do not label an interval bound as p95. Only the idle-latency probe reports
percentiles.

## Deterministic corpora

Generate benchmark corpora from fixed inputs. Do not add captured terminal sessions,
machine-specific paths, wall-clock values, random seeds, network responses, or developer state.

Shared terminal, protocol, search, and resurrection generators live in `benches/support/mod.rs`.
`server_fairness` keeps its deterministic helpers in its own benchmark executable, because it needs
live PTY traffic and acknowledgements. A corpus generator must produce the same bytes and expected
match counts on every run.

A change to a generator, viewport, pane count, retained-history count, message size, or acceptance
boundary makes a new benchmark definition. Save a new baseline, and do not compare the new corpus
with measurements from the old definition.

## Baseline comparisons

```bash
# Before the change
cargo bench -- --save-baseline before

# After the change
cargo bench -- --baseline before
```

The same options work for one target and filter:

```bash
cargo bench --bench terminal_ingest -- 'sgr_heavy' --save-baseline before-sgr
cargo bench --bench terminal_ingest -- 'sgr_heavy' --baseline before-sgr
```

`--save-baseline` replaces an existing baseline with the same name. Use a new name when you need to
keep the earlier measurement.

## Particle preparation probe

```bash
cargo test --locked --release --lib particle_frame_cost -- --ignored --nocapture
```

This optional timing probe measures 2,000 prepared particle frames in a 200×60 effect scope,
with a 156×24 pane and a fixed seed, sweeping the close progress in 100 steps. It excludes
trajectory setup, application view/layout, cell compositing, and terminal I/O. Compare it with
the same toolchain and profile before and after a particle change; it is not whole-client CPU
usage. The ordinary particle tests also verify that paint-only frames advance the effect without
calling the component's `view()`.

## Reveal compositing probe

```bash
cargo test --locked --release --lib reveal_frame_cost -- --ignored --nocapture
```

This optional paired probe compares the original per-cell Portal and Scan calculations with
prepared frame constants. Both paths composite the same 200×60 cells against a backdrop over
100 progress samples. It includes cell construction and compositing, and excludes application
view/layout, renderer traversal, and terminal I/O. Run on the same idle machine and profile;
the result is not whole-client CPU usage. The ordinary reveal tests compare both paths cell
for cell and verify pane and session masks advance without calling the component's `view()`.

## Linux active TUI CPU probe

`tools/tui-cpu-probe.py` measures one release client and its session server separately using
Linux `/proc` CPU ticks. Every run creates private HOME/XDG directories, an owned session, and a
`script`-hosted client; it does not attach to your running sessions. The guest alternates a status
character at 10 Hz, with a fixed title, animated OSC title, or both. A dense case rewrites every
terminal row. The script repeats body-only mode to check drift, at 200×60 and 475×116.

```sh
cargo build --locked --release
python3 tools/tui-cpu-probe.py target/release/rozi
# Run the same command with a saved before binary for comparison.
```

Output is JSON lines with CPU percentages of one core, not the whole machine. The default sample
window is 15 seconds per case; `--seconds` changes it. Animations, shell integration, autosave, and
resurrection are disabled. Host output goes to a scratch file, so the probe excludes graphical
terminal rendering. Run before and after serially, without Cargo or other heavy work during the
measurements. `--keep-scratch` retains synthetic logs for diagnosis; otherwise the owned session,
clients, and scratch files are cleaned up.

## Linux process-memory harness

`tools/memory-matrix.sh` is an opt-in Linux harness for release builds. It reads PSS and RSS from
`/proc` and reports client, server, application, and child-process groups separately. Every
scenario gets its own isolated `HOME`, XDG directories, session endpoints, and control sockets.

It requires Bash, Python 3, util-linux `script`, and Linux `smaps_rollup`.

| Mode | Coverage |
| --- | --- |
| `--smoke` | A bounded image lifecycle that validates dependencies, replay, cleanup, and shutdown |
| `--quick` (default) | Fixed viewport, pane, history, and content combinations |
| `--full` | The quick matrix plus larger pane and history counts, a second client, pane close, client disconnect, reconnect, and session kill |
| `--lifecycle` | The full cleanup matrix, using deterministic image content |

```bash
tools/memory-matrix.sh --smoke
tools/memory-matrix.sh --quick
tools/memory-matrix.sh --full --output target/memory-matrix/full
tools/memory-matrix.sh --lifecycle --output target/memory-matrix/lifecycle
```

Reproduce one scenario:

```bash
tools/memory-matrix.sh --case ROWS COLS PANES HISTORY CONTENT CLIENTS STATE \
  --output target/memory-matrix/case
```

`CONTENT` is `plain`, `styled`, `images`, or `image-stress`. `STATE` is `steady`, `closed`,
`disconnected`, `reconnected`, or `killed`, and defaults to `steady`. The `image-stress` content
emits enough decoded pixels to exercise the client image budget.

The runner takes five samples after a fixed settle period and reports the median in `results.json`
and `results.md`.

Compare PSS only between runs on the same machine and build. Keep child shell and workload processes
separate from rozi's own memory. Current RSS and PSS after quiescence can show cleanup; `VmHWM`
cannot, because it never decreases.

## Compare image patches in a real host

Run `python3 tools/graphics-patches.py --fps 20 --duration 60` inside an owned test pane. It generates
a retained background, a toolbar, a small opaque patch and a moving translucent patch with pixel
offsets. Resize the pane to rebuild the background. `--full` instead sends changing full frames.
The two workloads are distinct; compare each workload across rendering modes.

Every transfer uses an immutable file retained for at least `--retain-seconds` after transmission,
including backgrounds rebuilt on resize. The default is two seconds, with a 256 MiB retained-file
budget set by `--max-file-mib`. The producer waits when the budget fills instead of deleting young
files. Set a longer retention period for slow readers and record both settings and the actual
frame rate. Named-file transport has no reader acknowledgement, so readers delayed beyond that
period can miss a file. The workload also waits for the retention period before exit cleanup.

Use the same release binary, host font and pane pixel size. Launch one client normally and another
with `TUI_LIPAN_TERMINAL_IMAGES=composed`; run them serially. Sample client, server and graphical
host separately after warm-up, without compilation during sampling. Capture the actual host window
as well as rozi's PNG output: PNG captures always compose, so they cannot verify native placements.

## Profiling

The `release-debug` profile keeps release optimization and adds debug symbols:

```bash
cargo build --profile release-debug
samply record ./target/release-debug/rozi profile
```

1. Record one controlled workload.
2. Detach or quit rozi to finish the profile.
3. Attribute client, server, and child-process samples separately.

If host policy blocks profiling, report that limitation instead of inferring percentages from the
source.

## Related records

- [Performance archive](performance/README.md)
- [Performance audit playbook](performance/audit-playbook.md)

## Graphics transaction metrics

Runtime metrics distinguish parser graphics transactions, independent replacements, ordered barriers
and intentional supersession in server staging, per-client outboxes and the client inbox. These
counters are separate from overload shedding and replay resynchronization. Presentation ACK latency
measures the interval between server parsing of a completed independent replacement and receipt of
the client's successful-host-write acknowledgement. It does not measure producer CPU work before
its bytes reached the server. The timestamp window retains the newest 32 replacements; latency
samples and acknowledgements have separate counters.

DevTools exposes aggregate graphics, ACK and pacing rows. Pacing metrics include actual reader
holds, total hold duration, forced safety releases and the largest per-stream presentation gap. No per-frame logging is enabled. Report successful
host presentation counts when comparing frame rates; producer updates and received packets can
include frames intentionally superseded before presentation.

### Independent full-frame producer

Run `python3 tools/graphics-stream.py --fps 240 --mode moving --duration 20
--report /tmp/graphics-producer.json` in an owned test pane. Supported rates are 30, 60, 120 and
240 Hz; modes are `unchanged`, `moving` and `whole`. Each complete replacement uses the same image
and placement IDs. Only initial setup and optional mixed-workload barriers move the cursor.
`--mixed` adds title/text/query/delete barriers and input echo; SIGWINCH adds a resize barrier.
Use `tools/graphics-patches.py` for overlapping producer patches that must remain ordered.

The immutable RGBA pool is bounded by `--max-file-mib` (128 MiB by default) and remains on disk
at the reported `files` path after exit, so delayed clients can still open every queued filename.
Remove that directory after all clients finish. Report fixture/page-cache memory alongside process
RSS. Pool creation happens before timing; no full-frame files are rewritten during the stream.
The producer skips missed cadence slots after blocked writes rather than emitting a catch-up burst.
Its report counts completed producer writes; obtain presented FPS from successful host frames.

### Owned Ghostty measurements

`tools/graphics-benchmark.py` creates a private session and Ghostty window through the
`ghostty-window-lab` helper. Build the selected binary first, then run measurements without Cargo
or other CPU-intensive work. Use separate output directories for baseline and updated binaries:

```bash
cc -shared -fPIC -O2 -Wall -Wextra -Werror -o /tmp/graphics-wiretap.so \
  tools/graphics-wiretap.c -ldl -pthread
python3 tools/graphics-benchmark.py \
  --binary /absolute/path/to/rozi \
  --window-lab /absolute/path/to/window_lab.py \
  --output /tmp/graphics-after-120 \
  --wiretap /tmp/graphics-wiretap.so --fps 120 --producer-fps 240 --seconds 20
```

The runner waits for immutable fixture generation before timing, records binary identity,
verified window geometry, process CPU/RSS, producer write counts and runtime metric snapshots,
and stops its session before closing its owned window. It leaves results and fixtures for review.
Repeat at 60, 120, 144 and 240 Hz with the same image and window geometry, and with a 240 Hz
producer against 60 and 120 Hz clients. The window is always local, so the runner removes inherited
`SSH_*` variables; launched from an SSH shell, the client would otherwise select remote image
transport and measure the composed path. `intervals_over_two_frames` counts host frame intervals
longer than two configured frame periods. Percentiles can hide a stall that recurs in only part of
the sample. The optional Linux
interposer records only successful TTY writes from the selected executable; it also sees the
framework's separately opened TTY descriptor. Its binary records contain a native-endian u64
monotonic timestamp, u32 length and that many output bytes. This captures host traffic rather
than inferring it from uploads requested by the producer. The interposer adds measurement
overhead; use it consistently on both sides of a comparison and run an uninstrumented control.
GPU scanout is not measured; host write counters do not establish that boundary.

Producer latency is measured without changing what the pipeline transports. Pixel 0 of each
immutable fixture carries its pool index, and the producer logs each write's completion time and
index. Before a host write is forwarded, the interposer reads pixel 0 of each file that a complete
`t=t` upload names. Each write gets one timestamp, shared by its wire and media records. `latency`
in `result.json` matches each presented upload to the next producer write of that fixture. Frames
are skipped but never reordered, so the match is exact unless a whole pool cycle is skipped, which
the report flags as `ambiguous`. The boundary is producer write return to the host write that ends
the frame carrying the upload. Composed or remote presentation sends pixels directly, carries no
mark and reports no samples. A `t=t` APC split across `writev` buffers is not recognised; compare
`marked_uploads` with the upload count.

`--input-probes N` sends N keys during the sample. The producer then runs with `--echo-input`: its
terminal is in cbreak mode, and it echoes each key as `echo:<key>;` at its own blank cell run
below the image, restoring the cursor so the image never moves. `--input-via host` (the default)
delivers single letters to the owned window with Hyprland's `send_shortcut`, without moving
desktop focus, so a key takes Ghostty and the client's real input path. `--input-via control`
uses the session control socket, which bypasses the client's input answer window. `input_echo`
splits each key into send to producer read, which includes starting the injecting process,
producer echo to host write, and their total. Echoes are ordered output and reset pacing as
real echoes do.

Framework counters are available under `terminal_graphics` in client and server runtime metrics.
The client counts actual successful host frames, direct graphics presentations, native uploads and
placement changes. The server's snapshot counts its authoritative parser mutations. DevTools
includes native-scene operations, graphics command bytes, region merging and host-frame interval
percentiles. Percentiles retain the newest 1024 successful intervals; measure during sustained
output, since an idle gap is a real interval too. Composed and native host frames both participate.
