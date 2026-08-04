# Reproducing a performance audit

This playbook reproduces the performance and resource-efficiency audit of hyprmux. It complements
[Benchmarks and profiling](../benchmarks.md): that page explains the permanent benchmark targets,
while this page combines them into a full audit covering CPU, memory, process resources, live
transport behavior, and measurement limitations.

Run timing measurements on an otherwise idle machine in release mode. Keep the exact revision,
toolchain, terminal size, power profile, and workload generator with every result. Debug-mode
measurements are useful for developer experience, but they do not represent user-facing
performance.

## 1. Record the environment and worktree

Capture the state before building:

```bash
git rev-parse HEAD
git status --short
uname -a
rustc -Vv
cargo -V
lscpu
```

Do not silently benchmark only `HEAD` when the worktree is dirty. Record whether the result includes
uncommitted changes, and rerun an affected benchmark if those files change during the audit.

Keep generated output in the ignored build directory:

```bash
mkdir -p target/perf-audit
```

Also record available profilers rather than assuming they can run:

```bash
for tool in perf samply hyperfine valgrind heaptrack smem pidstat strace; do
  command -v "$tool" || true
done
```

On Linux, Samply and `perf` may be installed but unavailable to an unprivileged process because of
`/proc/sys/kernel/perf_event_paranoid`. Report that limitation; do not change a host-wide kernel
setting as part of the audit.

## 2. Build and record artifact sizes

Build the shipping profile and the symbolized profiling profile:

```bash
cargo build
cargo build --release
cargo build --profile release-debug
```

Record file and section sizes:

```bash
stat --printf='%n %s bytes\n' target/release/hyprmux target/debug/hyprmux
size target/release/hyprmux
```

The release profile is the runtime baseline. A large debug binary mostly reflects debug information
and is not evidence of release bloat.

## 3. Run the deterministic benchmark suite

Run the complete Criterion suite:

```bash
cargo bench 2>&1 | tee target/perf-audit-criterion.log
```

At minimum, retain these representative rows:

- `app_render/view_layout/{1,8,16}`
- `sidebar_render/{hidden,panes,agents,files,git}`
- `snapshot_rebuild/{80x24,200x60,320x90}`
- `output_burst/{per_message,per_frame}/{1,8,32,128}`
- every `terminal_ingest` corpus at `200x60`, plus its large-size case
- `session_pipeline_memory/{4096,65536}`
- `session_pipeline_unix_socketpair/4096` on Unix
- `pane_output_frame_roundtrip/{4096,1048576}`
- `control_frame_serde`

Use a saved baseline when evaluating a change:

```bash
cargo bench --bench snapshot_rebuild -- --save-baseline before
# Apply the change.
cargo bench --bench snapshot_rebuild -- --baseline before
```

Criterion reports confidence intervals, not p95 latency. Do not describe its interval as a
percentile. Treat small relative changes as harmless when the absolute cost remains immaterial.

### Render-specific rerun

If view/sidebar code changes during the audit, rerun the affected cases:

```bash
cargo bench --bench app_render -- 'app_render/view_layout/(8|16)|sidebar_render'
```

`app_render` measures view expansion and layout, not backend draw/diff time. TestBackend frame
capture allocates per cell and is not a valid draw benchmark. Use the interactive devtools metrics
or a sampling profile for real draw costs.

## 4. Measure scrollback search

There is not yet a permanent `scrollback_search` benchmark. The reference audit used a temporary
release test with this deterministic fixture:

- viewport: `250x60`
- scrollback limit: 5,000 lines per pane
- pane counts: 1, 8, and 16
- line text: `line-NNNNN ordinary payload`
- every hundredth line also contains `styled-search-needle`
- queries:
  - `needle` for sparse matches
  - `line` for a match on every line
- 11 repetitions per case, reporting the median

The measured operation was the sum of `TerminalPane::search_scrollback(query)` across every pane.
Create `tests/perf_audit_temp.rs` for the duration of the audit:

```rust
use std::time::{Duration, Instant};

use hyprmux::pane::TerminalPane;
use tui_lipan::prelude::SearchItem;
use tui_lipan::rank_search_palette_indices;

fn populated_pane(lines: usize) -> TerminalPane {
    let mut pane = TerminalPane::new(lines);
    pane.apply_server_resize(250, 60);
    let mut output = String::with_capacity(lines * 48);
    for line in 0..lines {
        if line.is_multiple_of(100) {
            output.push_str(&format!(
                "line-{line:05} styled-search-needle payload\r\n"
            ));
        } else {
            output.push_str(&format!("line-{line:05} ordinary payload\r\n"));
        }
    }
    pane.process_server_output(output.as_bytes());
    pane
}

fn median(mut values: Vec<Duration>) -> Duration {
    values.sort_unstable();
    values[values.len() / 2]
}

#[test]
fn measure_scrollback_search_scaling() {
    for panes in [1usize, 8, 16] {
        let mut terminals: Vec<_> = (0..panes).map(|_| populated_pane(5_000)).collect();
        for query in ["needle", "line"] {
            let mut samples = Vec::new();
            let mut match_count = 0;
            for _ in 0..11 {
                let started = Instant::now();
                match_count = terminals
                    .iter_mut()
                    .map(|pane| pane.search_scrollback(query).len())
                    .sum();
                samples.push(started.elapsed());
            }
            eprintln!(
                "scrollback_search panes={panes} query={query} matches={match_count} median_us={}",
                median(samples).as_micros()
            );
        }
    }
}

#[test]
fn measure_picker_filter_scaling() {
    for count in [100usize, 1_000, 10_000] {
        let items: Vec<_> = (0..count)
            .map(|index| {
                SearchItem::new(
                    format!("session-{index:05}-project-alpha-worker"),
                    index,
                )
            })
            .collect();
        let mut samples = Vec::new();
        let mut matches = 0;
        for _ in 0..31 {
            let started = Instant::now();
            matches = rank_search_palette_indices(&items, "prjalph").len();
            samples.push(started.elapsed());
        }
        eprintln!(
            "picker_filter items={count} matches={matches} median_us={}",
            median(samples).as_micros()
        );
    }
}
```

Run it in both profiles:

```bash
cargo test --release --test perf_audit_temp -- --nocapture
cargo test --test perf_audit_temp -- --nocapture
```

Remove the temporary test after collecting results. Until a permanent Criterion target replaces it,
record its source or the complete fixture above with the audit so the result is not presented as a
repository benchmark.

For picker filtering, the same temporary diagnostic used `rank_search_palette_indices` over 100,
1,000, and 10,000 deterministic `SearchItem`s and 31 repetitions.

## 5. Measure process memory with PSS

Use the Linux memory matrix. It isolates `HOME`, all XDG directories, session endpoints, and
control sockets, so it cannot attach to a normal user session.

Smoke-test the harness first:

```bash
tools/memory-matrix.sh --smoke
```

Run the broad matrix when time permits:

```bash
tools/memory-matrix.sh --full --output target/perf-audit/memory-full
```

The reference audit used these focused large-viewport cases:

```bash
tools/memory-matrix.sh --case 60 250 1 1 plain 1 \
  --output target/perf-audit/memory-idle
tools/memory-matrix.sh --case 60 250 1 5000 styled 1 \
  --output target/perf-audit/memory-large-1pane
tools/memory-matrix.sh --case 60 250 8 5000 styled 1 \
  --output target/perf-audit/memory-large-8pane
tools/memory-matrix.sh --case 60 250 8 5000 styled 2 \
  --output target/perf-audit/memory-large-8pane-2client
```

Read `results.json` for the exact sample metadata and `results.md` for the table.

Report these groups separately:

- client process PSS
- session server PSS
- application PSS: clients plus server
- child shell/process PSS

Never include child processes in application memory. Prefer PSS for comparisons; RSS double-counts
shared mappings. A high-water RSS that remains above current PSS is not evidence of a leak, and
allocator-retained anonymous memory requires a steady-state or repeated-cycle test before it is
called a leak.

## 6. Measure idle CPU and process resources

For a known PID, sample process CPU directly from `/proc` rather than using a lifetime average:

```bash
PID=<client-or-server-pid>
DURATION=6
HZ=$(getconf CLK_TCK)
BEFORE=$(awk '{print $14+$15}' "/proc/$PID/stat")
sleep "$DURATION"
AFTER=$(awk '{print $14+$15}' "/proc/$PID/stat")
awk -v a="$BEFORE" -v b="$AFTER" -v hz="$HZ" -v seconds="$DURATION" \
  'BEGIN { printf "%.3f%% of one core\n", 100*(b-a)/(hz*seconds) }'
```

Measure the client and server over the same interval. Repeat at least twice and report a range.
Record whether a terminal pane is focused, whether a clock or animated widget is visible, and how
many sessions are retained in the background.

Record process resources:

```bash
awk '/^Threads:|^VmRSS:|^VmHWM:/{print}' "/proc/$PID/status"
printf 'fds=%s\n' "$(printf '%s\n' /proc/$PID/fd/* | wc -l)"
```

Thread count is not CPU usage. Most hyprmux transport, PTY, watcher, and command-worker threads block
while idle.

## 7. Exercise live output and client fan-out

Build once, attach release clients at the same dimensions, and run each producer inside a pane:

```bash
timeout 10 yes 'plain output payload 0123456789'
```

```bash
timeout 10 sh -c '
  i=0
  while :; do
    printf "\033[1;38;5;196mstyled-%08d payload payload\033[0m\n" "$i"
    i=$((i+1))
  done
'
```

For fan-out, attach a second release client to the same named session, preferably read-only:

```bash
target/release/hyprmux attach perf-audit
target/release/hyprmux attach perf-audit --read-only
```

Sample the server and every client concurrently. CPU from an uncounted `yes`/shell loop is only a
stress indicator: without a byte count, do not turn it into a throughput claim. Use
`terminal_ingest` and `session_pipeline` for measured throughput.

Also test a slow client. The expected behavior is bounded backlog followed by that client's
disconnection, never an unbounded queue or a broadcast stall. The relevant deterministic tests can
be run by substring:

```bash
cargo test slow_client_is_disconnected_at_exact_backlog_boundary
cargo test two_client_broadcast_shares_one_encoded_allocation
cargo test large_paste_counts_bytes_and_overflow_fails_transport_explicitly
cargo test congested_flood_has_the_same_transcript_as_the_producer
```

## 8. Exercise lifecycle and cleanup

The full memory matrix includes pane closing and a detached/parked server case. Also exercise these
cycles manually or with an isolated diagnostic:

1. Fill scrollback, record PSS, close half the panes, settle, record PSS again.
2. Disconnect one of two clients.
3. Kill a named session and verify its server, PTYs, descriptors, and endpoint disappear.
4. Reconnect repeatedly while output continues.
5. Create and destroy sessions in a loop.
6. Leave the application idle for at least one hour; use 24 hours for leak claims.

Record both current PSS and high-water RSS. Memory need not return to its initial RSS for cleanup to
be correct; the important evidence is that live objects, processes, descriptors, and PSS reach a
stable plateau across repeated cycles.

## 9. Profile CPU when permitted

Use the symbolized optimized profile:

```bash
cargo build --profile release-debug
samply record --save-only --output target/perf-audit/profile.json.gz \
  ./target/release-debug/hyprmux perf-audit
```

Generate one controlled workload, then detach or quit. Summarize the relevant application thread;
do not attribute child shell CPU to hyprmux.

Useful profile questions:

- Is terminal parsing or snapshot rebuilding dominant?
- How much time is backend drawing and buffer diffing?
- Does server CPU land in PTY parsing, process inspection, queueing, or polling?
- Does cost grow with pane count or attached-client count?
- Are config/filesystem watchers waking while unchanged?

If profiling is denied by host policy, report that and rely on Criterion plus targeted timings. Do
not invent hotspot percentages from source inspection.

## 10. Remote, reconnect, and long-running cases

Remote testing requires a real SSH host; do not substitute local IPC numbers and call them remote.
When a host is available, repeat:

- idle and output CPU
- one and two clients
- temporary network interruption
- reconnect with queued output
- a deliberately stalled local consumer
- process and RSS cleanup after disconnect

Watch the pipe-backed transport separately from the bounded session mailbox. A memory plateau must
be demonstrated before declaring remote backpressure bounded.

## 11. Interpret results

Classify each observation:

- **Confirmed bottleneck:** repeated measurement or a specific demonstrated scaling failure.
- **Plausible risk:** a suspicious path with the exact missing experiment stated.
- **Harmless detail:** measurable but immaterial at realistic scale.
- **Deliberate trade-off:** a real cost that buys an explicit property, such as instant switching or
  independent client scrollback.

Keep these distinctions:

- throughput is not latency
- debug time is not release time
- RSS is not PSS
- allocator retention is not automatically a leak
- child memory and CPU are not application memory and CPU
- relative regressions need absolute context
- TestBackend view/layout cost is not backend draw cost

For every proposed optimization, record:

1. workload and affected scale
2. before measurement
3. relevant file and symbol
4. expected user-visible effect
5. implementation and regression risk
6. exact after-measurement that would validate it

If those fields cannot be filled, keep the item as an unverified risk rather than an optimization
task.

## 12. Cleanup and repository hygiene

Put generated reports below `target/`; never commit Criterion output, profiles, terminal captures,
private configs, socket paths, or logs. Delete temporary tests and scripts after recording enough
detail to reproduce them.

Finish by confirming that only intended source/documentation changes remain:

```bash
git status --short
git diff --check
```
