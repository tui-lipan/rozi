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
| `snapshot_rebuild` | `render_snapshot()` by viewport, server-output processing by message size, and the difference between rebuilding after every message and once per output burst. |
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
high-water behavior under unpaced producers and reports whether the bounded downstream policy
activates. Keep its result separate from the paced key-acknowledgement benchmark.

The idle-latency probe takes 200 key round trips, allowing 50–66 ms of deterministically
phase-jittered quiescence before each one, and reports p50, p95, p99, and maximum latency. Run it
on the same dedicated host before and after a server-wait change.

The resurrection cases report the server's complete durable snapshot attempt. Trigger and polling
delay stay outside the sample. The benchmark also emits server-loop blocking data, which has a
different boundary from whole-attempt duration.

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
