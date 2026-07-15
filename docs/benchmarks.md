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
| `snapshot_rebuild` | `render_snapshot()` by screen size and `TerminalPane::process_server_output` at 64 B, 1 KiB, and 64 KiB message sizes. |
| `protocol_framing` | Pane-output encode/decode round trips at 64 B, 4 KiB, and 1 MiB, plus serde of large `Attached` and `LayoutCommitted` control frames. |
| `session_pipeline` | In-memory frame encode, decode, client terminal processing, and snapshot rebuild; Unix also measures a 4 KiB socket-pair path. |

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

## Local framework dependency

`Cargo.toml` currently points directly to the sibling `../tui-lipan/` checkout, so benchmark and
profile builds require that checkout. The corresponding `Cargo.lock` entry has no registry source
or checksum, which is correct for the current path dependency.

Before a standalone clone, CI job, or release build can resolve `tui-lipan` from crates.io, publish
the required framework version, replace the path dependency with its registry version requirement,
and regenerate `Cargo.lock` without a path override. Do not describe a registry release as active
until that manifest change has landed.
