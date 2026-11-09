# Release regression check and loop alignment

A full benchmark comparison of 0.0.36 with 0.0.43 showed scrollback search 18–38% slower and
history replay about 40% slower. Neither path had changed. Both slowdowns come from where unrelated
code changes placed one tight per-cell loop relative to the CPU's 64-byte fetch blocks. Aligning
every loop to 64 bytes on x86_64 removes them: with the alignment fixed on both sides, current
master is within 3% of 0.0.36 on those cases. The remaining differences are either feature costs
or changes in machine state. Verdict: **ready**.

[Performance index](../README.md) · [Benchmark guide](../../benchmarks.md) · [Reproduction playbook](../audit-playbook.md)

## Measurement context

| Item | Value |
| --- | --- |
| Revisions | `v0.0.36` (`1da2b55d`, tui-lipan 0.33.0), `v0.0.43` (`1df1824a`), master `3f9623c6` (tui-lipan 0.41.0), clean worktrees |
| CPU | AMD Ryzen 7 9700X, 8 cores / 16 threads, governor and energy preference `performance` |
| OS | Linux 7.2.15, transparent huge pages `always` |
| Toolchain | rustc/cargo 1.96.1 |
| Profile | `bench`, inheriting `release`: thin LTO, `codegen-units = 1` |
| Tools | Criterion 0.8 targets from `benches/`, `perf stat`, `perf record`, `objdump`, `git bisect` |

Benchmark definitions are unchanged between `v0.0.36` and master apart from import paths, so the
same IDs measure the same work on both sides.

## Findings

### Scrollback search and replay: code layout, fixed

The first comparison (`v0.0.36` against `v0.0.43`, default Criterion windows) showed every
`scrollback_search` case 18–38% slower, and `terminal_history/boundary/replay/{plain,unicode}`
39–42% slower. Bisecting the search case pointed at #350, which only adds a cursor-trail flag and
cannot reach the search code. The replay case alternated between about 1.01 and 1.44 ms across
commits, out of commit order.

The cause is code layout, not code:

- One master build ran replay at 1.44 ms. Rebuilding it with only `strip = false` ran it at
  1.04 ms. That build's `.text` starts 0x50 bytes later.
- `perf stat` over 5 seconds: the slow binary retired 138 G instructions in 27.8 G cycles (IPC 4.96).
  The fast one retired 192 G in 28.2 G (IPC 6.82). Branch and cache misses per instruction are the
  same.
- `perf record` puts 99.6% of replay time in tui-lipan's
  `TerminalScreen::write_active_grid_repaint`. The hot loop is the right-to-left scan that finds
  each row's last non-blank cell. In the slow binary that loop starts 32 bytes into a 64-byte block
  and spans three fetch blocks. Search runs the same scan in `push_plain_line_text_impl`.

Rebuilding the slow configuration with `-C llvm-args=-align-loops=64` ran replay at 1.02 ms and
search at its fast speed. The release and nightly workflows now pass that flag for every x86_64
target through `CARGO_TARGET_<TRIPLE>_RUSTFLAGS`. The benchmark guide passes it for local
comparisons. A tracked `.cargo/config.toml` would collide with the ignored local override that
holds a tui-lipan path patch, and Cargo has no stable per-profile `rustflags`.

Measured in one session, with `v0.0.36` built with the same flag:

| Case | `v0.0.36` | master + alignment |
| --- | ---: | ---: |
| `terminal_history/boundary/replay/plain` | 1.016 ms | 1.019 ms |
| `terminal_history/boundary/replay/unicode` | 1.025 ms | 1.028 ms |
| `terminal_history/boundary/search_no_match/plain` | 902 µs | 895 µs |
| `terminal_history/boundary/search_no_match/dense` | 2.268 ms | 2.296 ms |
| `terminal_history/boundary/copy/plain` | 1.032 ms | 1.042 ms |
| `terminal_history/boundary/copy/dense` | 3.820 ms | 4.019 ms |

Every `scrollback_search` and `replay_export` case is within −4.3% to +3.4% of `v0.0.36`, measured
with 1-second warm-up and 2-second windows.

On master, the same source built with the alignment against a build without it:

| Effect | Cases |
| --- | --- |
| Faster | replay −30%; `inbound_drain` up to −6%; `sparse_tui_update/*/ingest` −3% to −11% |
| Slower | `program_status_ingest/progress/false` +7.5%; `copy/unicode` +7.3%; most other cases within ±3% |
| Size | `rozi` release binary 34.32 MB → 36.13 MB (+5.3%), all of it loop padding in `.text` |

Alignment fixes where loops start, not everything about code layout. Other layout effects of a
few percent remain possible.

### Copying dense history: framework trade-off

`terminal_history/boundary/copy/dense` is 5% slower in the same session and was 6.4% slower in the
first comparison. tui-lipan #419 changed copy scans to trim blank edges and sweep at one speed. The
same change made `copy/plain` and `copy/unicode` 24–29% faster and `search_no_match/{plain,unicode}`
31–34% faster in that comparison.

### Whole-application view and layout: feature cost

`app_render/view_layout/1` is 2.3% slower (31.0 → 31.7 µs). Profiles of both revisions have the
same distribution, led by `element_layout_hash` at about 7.5%, with no new hot symbol. The
allocation probe for one pane rises from 428 to 444 allocations per pass: +3 from #284 (the
workbar's draggable tab bar), +2 from #285 (clipped badge hover effects), +8 from #332 (slot labels
drawn by the tab bar) and +3 later. These are the cost of the added workbar behavior.

### `terminal_pane_process_server_output/65536`: machine state

This case measured 418 µs for `v0.0.36` and 648 µs for master in one aligned run. Bisecting it
pointed at the first commit after 0.0.43. Re-measuring showed `v0.0.43` and `v0.0.36` both at
655–658 µs as well. The same `v0.0.36` source moved from 420 to 656 µs within half an hour. The
case allocates a fresh 5,000-line pane on every iteration, so it follows the kernel's memory
state. It is not a code regression.

For the same reason, `search_no_match/dense` measured 1.99 ms before a reboot and 2.27 ms after it,
with the same binary. Compare revisions inside one session.

## Benchmark harness fixes

Two harness defects stopped the documented whole-suite commands:

- The library and `rozi` binary were benchmark targets, so `cargo bench -- --save-baseline before`
  reached their libtest harnesses, which reject Criterion options. Both are now `bench = false`.
- `replay_memory` panicked when `cargo bench` launched it with `--bench`, which stopped the run
  before `scrollback_search` and every later target. It now returns when launched that way.

## Follow-up

- The right-to-left blank scan still compares each trailing cell's character, colors, flags and
  extra data. A row-level occupied-length bound in the framework would remove the scan and the
  layout sensitivity together. alacritty keeps that count private.
- Measure the alignment on aarch64 before extending the setting beyond x86_64.
- `cargo install` from crates.io and local `cargo build --release` do not get the flag. Only the
  published archives do.
