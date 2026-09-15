# Compact scrollback in Alacritty

The [ingest decision](2026-09-14-ingest.md) left compact history as a technical go whose adoption
depends on the engine. This report records the attempt to take the design upstream: an opt-in port
to Alacritty `master`, measured in Alacritty itself against the unmodified revision.

Upstream closed the proposal, alacritty/alacritty#9049, and no pull request was opened. The port is
archived here as final evidence. It is not used by Rozi.

## Candidate and baseline

- Baseline: Alacritty `d692748d`, release build.
- Candidate: the same revision with the port, archived as
  [`tools/experiments/alacritty-port`](../../../tools/experiments/alacritty-port/README.md). Its
  last commit is `e8e5aee6` on a local branch.
- `dense` is the candidate with compaction off, which is what every existing Alacritty user would
  run. `compact` sets `ALACRITTY_COMPACT_SCROLLBACK=1`.

The port keeps negative-line `Grid` indexing for dense rows and panics only on a compact row, which
must be read through `Grid::read_row`. The candidate passes 148 `alacritty_terminal` unit tests,
45 reference tests, and a differential test that compares mixed output and resizes with compaction
off, on, and toggled against `alacritty_terminal` 0.26.0.

AMD Ryzen 7 5700X3D, Linux 7.2.3, Wayland, Rust 1.98.1. Each run waited for load average below 2
and no running `rustc`.

## What was measured

- vtebench in a fullscreen 284×68 window with `scrolling.history = 10000` and `--max-secs 5`, plus
  `short_lines`, 2,000 log lines of about 40 columns. Upstream, compact, and dense alternate. The
  table gives the median of per-round medians. vtebench reports whole milliseconds, so one tick on a
  9 ms benchmark reads as 11%.
- Process PSS after 12,000 lines of short log output or full-width random text, sampled while idle.
- Criterion benchmarks of the engine alone, at 253×64 with 10,000 lines of history, interleaved over
  5 rounds against a baseline built from the same benchmark file.

## Alacritty

Three rounds, with every fix except the cursor-cell change:

| vtebench, ms per sample | upstream | dense | compact |
| --- | ---: | ---: | ---: |
| `short_lines` | 18 | 16 | 14 |
| `scrolling_fullscreen` | 9 | 9 | 10 |
| `dense_cells` | 93 | 99 | 99 |
| `cursor_motion` | 32 | 33 | 33 |
| `scrolling` | 144 | 146 | 146 |
| scroll regions, four benchmarks | 143–147 | 144–148 | 144–145 |
| `light_cells`, `medium_cells`, `sync_medium_cells`, `unicode` | 8–12 | 8–12 | 8–12, equal to upstream |

Five rounds of `dense_cells` alone after the cursor-cell change:

| `dense_cells` | upstream | dense | compact |
| --- | ---: | ---: | ---: |
| median of round medians, ms | 92 | 95 | 94 |
| round medians, ms | 92, 92, 96, 93, 92 | 95, 95, 94, 95, 94 | 94, 94, 94, 94, 95 |
| mean of all samples, ms | 93.3 | 94.7 (+1.6%) | 94.3 (+1.1%) |

| PSS after filling history, MiB | upstream | dense | compact |
| --- | ---: | ---: | ---: |
| short lines | 116.9 | 117.1 | 59.5 |
| full-width lines | 121.7 | 122.0 | 121.8 |

With short lines in history, compact storage halves Alacritty's PSS, and anonymous memory falls from
89.3 to 32.0 MiB. Full-width history is the same size in every build.

Compact mode is also the fastest build on `short_lines`. The engine benchmarks below charge it
about 30% more CPU per short line, and the application does not show that cost. Writing and
touching less memory per scrolled line is a likely reason, but this run does not isolate it.

## Engine

| Criterion case | upstream | dense | compact |
| --- | ---: | ---: | ---: |
| `ingest_100k/short` | 21.7 ms | +3.6% | +29.5% |
| `ingest_100k/full` | 182.6 ms | −1.7% | −0.3% |
| `ingest_100k/uniform` | 182.2 ms | −1.1% | −0.6% |
| `alt_screen/dense_cells` | 55.2 ms | +0.2% | +0.7% |
| `display_iter/live` | 28.2 µs | −19.0% | −17.1% |
| `display_iter/top` | 28.2 µs | −17.2% | −26.6% |
| `iterate_history` | 4.63 ms | −15.3% | −29.3% |

The first four rows are measured at `e8e5aee6`. The read rows are measured at `6eb5f322`; the later
commits do not change read paths and were not measured on them.

## Regressions found and fixed

Each was found by Alacritty or engine benchmarks, reproduced in the engine, and fixed before the
numbers above:

- `display_iter` was 55–98% slower because the iterator resolved a history row per cell. It now
  resolves a row once per line.
- Dense ingest with full history went through the row pool. A full dense history now swaps the
  oldest row with the scrolled viewport row.
- The negative-line check on `Grid` indexing stopped the per-character write path from inlining.
  History indexing moved to cold helpers.
- Rows of one repeated glyph across the full width compacted to a single cell only after a
  full-row comparison: +21% in the engine and +33% in vtebench `scrolling_fullscreen`. Such rows now
  stay dense. The engine case is at −0.6%, and Alacritty went from 12 to 10 ms against upstream's
  9 ms.
- Writing to the cursor cell went through the history check on every character: `dense_cells` was
  +3.6% dense and +4.4% compact in the engine, and 93 → 99 ms in Alacritty. Indexing the viewport
  directly brings the engine to +0.2% and +0.7%, and Alacritty to about +1–2%.

Shrinking a history row from 64 to 40 bytes did not move the remaining +3.6% dense short-line
engine cost. A 32-byte row, which needs bit-packing and a 64-bit-only layout, was not tried, since
nothing points to row size as the cause.

## Result

Compact scrollback is a credible production design. Against unmodified Alacritty it halves process
memory with short-line history and is faster on short-line output. The other vtebench workloads
stay within 3% of upstream, apart from one tick on the 9 ms `scrolling_fullscreen`. With compaction
off, the port matches upstream except for the +3.6% short-line engine case, which Alacritty does not
show, and about 1.5 ms (+1.6%) in `dense_cells`.

Upstream will not take it, so Rozi can use it only through an `alacritty_terminal` fork maintained
alongside tui-lipan. Whether the memory saving justifies that fork is a separate product decision.

Not measured: resize and reflow in Alacritty, and the port inside Rozi.
