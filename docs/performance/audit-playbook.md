# Reproducing a performance audit

This playbook defines a repeatable audit for CPU, latency, memory, process resources, live
transport behavior, lifecycle cleanup, and profiling. The
[benchmark guide](../benchmarks.md) defines each permanent harness.

Run release measurements on an idle machine. Keep the source revision, Rust toolchain, terminal
size, power profile, workload, and sample method fixed during a comparison.

## 1. Record the environment

Capture the source and host before building:

```bash
git rev-parse HEAD
git status --short
uname -a
rustc -Vv
cargo -V
lscpu
```

Record whether the worktree is dirty and list the changed files that affect a result. Do not report
a dirty-worktree measurement as a measurement of `HEAD`.

Record available tools:

```bash
for tool in perf samply hyperfine valgrind heaptrack smem pidstat strace; do
  command -v "$tool" || true
done
```

On Linux, `perf` and Samply may be blocked by `/proc/sys/kernel/perf_event_paranoid`. Record the
host restriction. Do not change a machine-wide policy as part of an audit.

Put generated output below the ignored build directory:

```bash
mkdir -p target/perf-audit
```

## 2. Build the measured artifacts

Build the shipping profile and the symbolized profiling profile:

```bash
cargo build --locked --release
cargo build --locked --profile release-debug
```

Record binary and section sizes when size is in scope:

```bash
stat --printf='%n %s bytes\n' target/release/rozi
size target/release/rozi
```

Use the release binary for user-facing runtime claims. Keep debug-build observations separate.

## 3. Run deterministic benchmarks

Run the full suite when the audit covers the whole application:

```bash
cargo bench 2>&1 | tee target/perf-audit/criterion.log
```

For a focused audit, select only affected targets and IDs. Record the exact command and all
Criterion output needed to identify the estimate and interval.

The broad audit set includes:

- `terminal_ingest` at the standard viewports and corpus types;
- `snapshot_rebuild`, including server-output and burst cases;
- `protocol_framing`;
- `session_pipeline`, including the Unix socket-pair case where available;
- `app_render` for populated, empty, and sidebar states;
- `scrollback_search` for complete scans and cooperative slices;
- `server_fairness` for paced ingress, saturation, and resurrection snapshots.

Run the target-specific server evidence separately:

```bash
cargo bench --bench server_fairness -- continuous_pty_ingress
cargo bench --bench server_fairness -- --saturation-probe
cargo bench --bench server_fairness -- resurrection_snapshot
```

The paced key-acknowledgement result, one-shot saturation result, whole snapshot duration, and
server-loop blocking time measure different boundaries. Report them separately.

When evaluating a change, save a baseline before editing and compare with the same filter after the
edit:

```bash
cargo bench --bench snapshot_rebuild -- --save-baseline before
cargo bench --bench snapshot_rebuild -- --baseline before
```

Do not compare results across changed generators, viewports, pane counts, history counts,
toolchains, or power settings. Criterion intervals are estimate uncertainty, not p95 latency.

## 4. Measure process memory

The Linux memory matrix isolates user directories, endpoints, and sockets. Smoke-test it before a
long run:

```bash
tools/memory-matrix.sh --smoke
```

Run the standard matrices:

```bash
tools/memory-matrix.sh --quick --output target/perf-audit/memory-quick
tools/memory-matrix.sh --full --output target/perf-audit/memory-full
tools/memory-matrix.sh --lifecycle --output target/perf-audit/memory-lifecycle
```

Use a fixed case to repeat a noisy or failed scenario:

```bash
tools/memory-matrix.sh --case 60 250 8 5000 styled 2 reconnected \
  --output target/perf-audit/memory-case
```

Read `results.json` for sample metadata and `results.md` for the summary. Report these groups
separately:

- client PSS;
- session-server PSS;
- application PSS, which is client plus server;
- child shell and workload PSS.

Prefer PSS for process comparisons because RSS counts shared mappings in each process. Use current
PSS and RSS after quiescence for cleanup. `VmHWM` cannot show cleanup. A single drop or retained
allocator arena is not enough to prove a leak. Leak claims need repeated cycles or a long soak that
shows continuing growth.

## 5. Measure idle CPU and resources

On Linux, sample a known client or server PID over a fixed interval:

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

Measure the client and server over the same interval. Repeat each sample. Record pane count,
attached-client count, focused pane, visible animated content, terminal dimensions, and retained
background sessions.

Record process resources:

```bash
awk '/^Threads:|^VmRSS:|^VmHWM:/{print}' "/proc/$PID/status"
printf 'fds=%s\n' "$(printf '%s\n' /proc/$PID/fd/* | wc -l)"
```

Thread count is not CPU usage. Identify whether each sampled process belongs to Rozi or to the
workload.

## 6. Exercise live output and fan-out

Start a named release session and attach a second client at the same terminal dimensions:

```bash
target/release/rozi perf-audit
target/release/rozi attach perf-audit --read-only
```

Run a bounded plain-output producer inside a pane:

```bash
timeout 10 yes 'plain output payload 0123456789'
```

Run a bounded styled-output producer:

```bash
timeout 10 sh -c '
  i=0
  while :; do
    printf "\033[1;38;5;196mstyled-%08d payload payload\033[0m\n" "$i"
    i=$((i+1))
  done
'
```

Sample the server and every client over the same interval. An uncounted producer is a stress case,
not a throughput measurement. Use `terminal_ingest` and `session_pipeline` for byte-throughput
claims.

Query bounded resource counters before, during, and after the workload:

```bash
ROZI_SOCKET=/path/to/control.sock target/release/rozi metrics | jq .
```

Record current bytes, high-water marks, capacities, sample age, and stale state. A high-water mark
shows a peak. Current bytes after quiescence show whether the queue drained.

## 7. Exercise lifecycle cleanup

Use the full and lifecycle memory matrices for standard states. Add longer isolated cycles when
making cleanup or leak claims:

1. Fill scrollback, close half the panes, settle, and sample again.
2. Disconnect one of two clients.
3. Reconnect while output continues.
4. Kill a named session and check its server, PTYs, descriptors, and endpoint.
5. Create and destroy sessions repeatedly.
6. Leave the isolated application idle for a declared duration.

Record live objects, processes, file descriptors, current PSS, and current RSS after every settle
period. State the duration and number of cycles.

Remote performance needs a real SSH host. Record both ends, the network conditions, disconnect and
reconnect behavior, and a deliberately slow consumer. Do not describe local IPC measurements as
remote results.

## 8. Profile CPU

Use the symbolized optimized binary:

```bash
samply record --save-only --output target/perf-audit/profile.json.gz \
  ./target/release-debug/rozi perf-audit
```

Generate one controlled workload and stop the recording cleanly. Report the sampled thread and
process. Keep client, server, and child-shell CPU separate.

Use profiles to answer a stated question, such as which function dominates a measured workload or
whether cost grows with pane or client count. If host policy blocks sampling, report that and rely
on deterministic benchmarks and direct process measurements.

## 9. Interpret and report

Classify each observation as one of:

- confirmed bottleneck, supported by repeated measurements or demonstrated scaling;
- plausible risk, with the missing experiment stated;
- harmless measured cost at the tested scale;
- deliberate trade-off tied to a documented behavior.

Keep these distinctions in the report:

- throughput is not latency;
- debug time is not release time;
- RSS is not PSS;
- allocator retention is not automatically a leak;
- child-process resources are not Rozi resources;
- relative change needs absolute context;
- view and layout time is not backend draw and diff time.

Create `audits/YYYY-MM-DD.md` and record:

1. exact revision and dirty-worktree state;
2. OS, CPU, Rust version, build profile, power settings, and unavailable tools;
3. exact commands, workloads, dimensions, durations, and sample counts;
4. statistics with their correct boundaries;
5. application and child-process resources separately;
6. confirmed findings, trade-offs, harmless details, and unverified risks;
7. follow-up measurements needed to confirm any proposed change;
8. one verdict: `ready`, `ready with minor improvements`, `needs optimization before release`, or
   `insufficient evidence`.

Link the new report from the [performance archive](README.md). Do not rewrite an older report with
new measurements.

## 10. Clean up

Keep raw reports, profiles, logs, sockets, private configuration, and captures below `target/`.
Remove temporary sessions and workload files. Finish with:

```bash
git status --short
git diff --check
```
