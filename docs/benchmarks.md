# Benchmarks and profiling

Rozi keeps repeatable benchmark definitions in `benches/` and the Linux process-memory runner in
`tools/memory-matrix.sh`. Timing results belong in dated
[performance audit reports](performance/README.md), not on this page.

CI compiles every benchmark with `cargo check --locked --all-targets`. It does not use shared
runners for timing decisions. Run timing and memory measurements on an idle machine with a stable
power profile.

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
cargo bench --bench snapshot_rebuild
cargo bench --bench protocol_framing
cargo bench --bench session_pipeline
cargo bench --bench app_render
cargo bench --bench scrollback_search
cargo bench --bench server_fairness
```

Arguments after `--` select Criterion benchmark IDs or target-specific evidence modes:

```bash
cargo bench --bench terminal_ingest -- 'sgr_heavy/200x60'
cargo bench --bench snapshot_rebuild -- terminal_pane_process_server_output
cargo bench --bench protocol_framing -- control_frame_serde
cargo bench --bench session_pipeline -- session_pipeline_memory/4096
cargo bench --bench app_render -- 'app_render/view_layout/(8|16)|sidebar_render'
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

Criterion writes generated measurements and reports below `target/criterion/`. Do not commit them.

## Target definitions

| Target | Definition |
| --- | --- |
| `terminal_ingest` | Measures `TerminalScreen::process_bytes` throughput for generated plain lines, SGR-heavy output, scroll regions and cursor movement, wide Unicode, and long sparse-escape lines at fixed viewport sizes. |
| `snapshot_rebuild` | Measures `render_snapshot()` by viewport, server-output processing by message size, and the difference between rebuilding after every message and once per output burst. |
| `protocol_framing` | Measures pane-output frame encode/decode round trips and serde for generated large control frames. |
| `session_pipeline` | Measures in-memory frame encode, decode, client terminal processing, and snapshot rebuilding. Unix also includes a socket-pair case. |
| `app_render` | Measures whole-application view expansion and layout by pane count, with empty and populated terminals. It also measures fixed sidebar states and repository-size fixtures. It does not measure backend drawing or terminal buffer diffing. |
| `scrollback_search` | Measures complete searches across fixed pane and history counts, scanner slices, and full production mapping for one cooperative slice. Cases cover sparse, dense, and absent matches. |
| `server_fairness` | Measures key acknowledgement through a real server-owned PTY under paced continuous ingress, idle-settled key latency, durable resurrection snapshot attempts, and a one-shot bounded saturation probe. |

The saturation probe is not a Criterion latency statistic. It checks the configured PTY ingress
high-water behavior under unpaced producers and reports whether the bounded downstream policy
activates. Keep its result separate from the paced key-acknowledgement benchmark.

The idle-latency probe takes 200 key round trips, allowing 50 ms of quiescence before each one, and
reports p50, p95, p99, and maximum latency. Run it on the same dedicated host before and after a
server-wait change; unlike Criterion estimate intervals, these values are request-latency
percentiles.

The resurrection cases report the server's complete durable snapshot attempt. Trigger and polling
delay stay outside the sample. The benchmark also emits server-loop blocking data, which has a
different boundary from whole-attempt duration.

Criterion estimate intervals describe uncertainty around an estimate. They are not request-latency
percentiles. Do not label an interval bound as p95.

## Deterministic corpora

Benchmark corpora must be generated from fixed inputs. Do not add captured terminal sessions,
machine-specific paths, wall-clock values, random seeds, network responses, or developer state.

Shared terminal, protocol, search, and resurrection generators live in `benches/support/mod.rs`.
`server_fairness` uses deterministic helpers in its benchmark executable because it needs live PTY
traffic and acknowledgements. A corpus generator must produce the same bytes and expected match
counts on every run.

When a generator, viewport, pane count, retained-history count, message size, or acceptance boundary
changes, treat the result as a new benchmark definition. Save a new baseline. Do not compare the
new corpus with measurements from the old definition.

## Baseline comparisons

Keep the Rust toolchain, source revision, benchmark filter, terminal dimensions, power settings,
and host load stable between runs.

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

`--save-baseline` replaces an existing baseline with the same name. Use a new name when the earlier
measurement must remain available. Record the exact revision and dirty-worktree state with any
reported result.

## Linux process-memory harness

`tools/memory-matrix.sh` is an opt-in Linux harness for release builds. It reads PSS and RSS from
`/proc` and reports client, server, application, and child-process groups separately. It isolates
`HOME`, XDG directories, session endpoints, and control sockets for every scenario.

The quick matrix covers fixed viewport, pane, history, and content combinations. The full matrix
adds larger pane and history counts, a second client, pane close, client disconnect, reconnect, and
session kill. The lifecycle matrix uses deterministic image content.

```bash
tools/memory-matrix.sh --smoke
tools/memory-matrix.sh --quick
tools/memory-matrix.sh --full --output target/memory-matrix/full
tools/memory-matrix.sh --lifecycle --output target/memory-matrix/lifecycle
```

Reproduce one scenario with:

```bash
tools/memory-matrix.sh --case ROWS COLS PANES HISTORY CONTENT CLIENTS STATE \
  --output target/memory-matrix/case
```

`CONTENT` is `plain`, `styled`, or `images`. `STATE` is `steady`, `closed`, `disconnected`,
`reconnected`, or `killed`.

The runner requires Bash, Python 3, util-linux `script`, and Linux `smaps_rollup`. It takes five
samples after a fixed settle period and reports the median in `results.json` and `results.md`.
Generated output stays below `target/` and must not be committed.

Compare PSS from the same machine and build. Keep child shell and workload processes separate from
Rozi application memory. Current RSS and PSS after quiescence can show cleanup. `VmHWM` cannot,
because it never decreases.

## Profiling

The `release-debug` profile keeps release optimization and debug symbols:

```bash
cargo build --profile release-debug
samply record ./target/release-debug/rozi profile
```

Record one controlled workload, then detach or quit to finish the profile. Attribute client,
server, and child-process samples separately. If host policy blocks profiling, report the
limitation instead of inferring percentages from source.

## Related records

- [Performance archive](performance/README.md)
- [Performance audit playbook](performance/audit-playbook.md)
