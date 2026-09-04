# Idle CPU under a non-fullscreen agent spinner

[Performance index](../README.md) · [Benchmark guide](../../benchmarks.md) ·
[Reproduction playbook](../audit-playbook.md) ·
[Same-day render audit](2026-09-04.md)

## Verdict and scope

**Verdict: confirmed defect, since fixed. One changed character repainted the whole window.**
See [Fixed, and by how much](#fixed-and-by-how-much) for the result.

A TUI that does not take the alternate screen - a coding-agent CLI, typically - leaves its output
in the primary buffer and animates a spinner in a couple of cells. The reported symptom is a client
sitting at about 8% of one core while that spinner is the only thing moving, and the suspected
cause was the pane's large scrollback.

Scrollback is not the cause, and neither is the size of the change. Client CPU is linear in how
often the guest writes and linear in the *total* cells of the window, and does not move at all when
the writing pane is shrunk to a quarter of that window. Every tiny update rebuilds the entire
window's frame.

This audit measures one client, one writing pane, CPU from `/proc`, and a temporary timer splitting
the paint from the diff. It does not attribute cost *within* painting, which would need a sampled
profile this host is not currently configured for.

## Measurement context

| Item | Value |
| --- | --- |
| Revision | Rozi `ca5f34b`, release build |
| OS | Arch Linux, kernel 7.1.9-arch1-2 |
| CPU | AMD Ryzen 7 5700X3D, governor and EPP `performance`, boost on |
| Harness | isolated `HOME`/XDG root, server plus one `script`-hosted client, `/proc/<pid>/stat` over a 6 s window, modelled on `tools/memory-matrix.sh` |
| Guest | `printf '\r%s working'` in `/bin/sh`, one write per tick, primary screen, no scroll |
| Config | animations off, so nothing but the guest's output can drive a frame |

CPU is a percentage of one core. `host B/s` is what the client wrote to its terminal, read from the
size of the `script` wrapper's log.

## Scrollback is exonerated

`TerminalPane` at a fixed 200x60, one spinner tick plus the visible snapshot it dirties, against
history depth. Criterion medians.

| History rows | Tick + snapshot | Ingest alone |
| ---: | ---: | ---: |
| 0 | 236.17 µs | 159.02 ns |
| 1,000 | 231.85 µs | 159.50 ns |
| 5,000 | 234.00 µs | 161.24 ns |
| 20,000 | 235.10 µs | 160.57 ns |
| 50,000 | 230.69 µs | 158.08 ns |

Flat across the whole range. A spinner that ends each line, so the grid scrolls and every visible
row moves, measures the same as one redrawing in place: 216 µs against 216 µs at 0 rows, 222 µs
against 217 µs at 20,000.

"Large scrollback causes high idle CPU" is a plausible diagnosis and a wrong one. Record it as
tested.

## Client CPU against update rate

One pane, 200x60, only the spinner changing.

| Updates/s | Client % | Server % | Host B/s | Host B/update |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 0.67 | 0.67 | 222 | 222 |
| 5 | 1.33 | 0.67 | 620 | 124 |
| 10 | 2.00 | 0.67 | 1,123 | 112 |
| 20 | 3.50 | 0.67 | 2,092 | 105 |
| 30 | 5.17 | 0.83 | 3,065 | 102 |
| 60 | 9.67 | 1.00 | 5,873 | 98 |

Linear, at roughly 0.15% of a core per update per second - about 1.5 ms of client CPU for one
changed character. The reported 8% corresponds to a guest writing 45 to 50 times a second.

Two things this rules out. The server is flat near 1% throughout, so the transport and the
server-side agent read are not the cost. And host output holds at about 100 bytes per update
whatever the rate, so the frame diff is doing its job: one changed cell reaches the terminal as one
changed cell. Paints track updates one to one - there is no fragmentation to coalesce and no
scheduling storm. The waste is entirely in building the frame, not in emitting it.

## The cost is total window cells, not changed cells

Same spinner, fixed 30 updates/s, viewport varied.

| Viewport | Cells | Client % | Host B/s | CPU per update |
| --- | ---: | ---: | ---: | ---: |
| 80x24 | 1,920 | 1.17 | 3,065 | 390 µs |
| 200x60 | 12,000 | 5.17 | 3,062 | 1,723 µs |
| 320x90 | 28,800 | 10.67 | 3,065 | 3,557 µs |

Host output is identical in all three. CPU is not: it tracks the cell count of the whole window.

The discriminating case holds the window at 200x60 and splits it, so the spinner's own pane shrinks
while total cells stay the same. Idle panes run `sleep`.

| Panes | Spinner pane's share of the window | Client % | Host B/s |
| ---: | --- | ---: | ---: |
| 1 | all of it | 4.83 | 3,065 |
| 2 | half | 5.00 | 3,065 |
| 4 | a quarter | 4.50 | 3,065 |

Flat. Shrinking the changing pane to a quarter of the window changes nothing, so the repaint is not
scoped to the pane that changed, let alone to the cells that changed. **One character moving in one
pane costs a full-window frame.**

## Where an update's 1.7 ms goes

`draw_current_tree` funnels every render path, and its one `terminal.draw(...)` call does two
separable things: `render(f, &ctx)` paints the widget tree into ratatui's next `Buffer`, and the
rest of the call diffs that against the previous buffer and writes the result. Timing them
separately, at 30 updates/s:

| Viewport | Cells | `render(f, &ctx)` | diff + flush | Whole draw | Paint share |
| --- | ---: | ---: | ---: | ---: | ---: |
| 80x24 | 1,920 | 186 µs | 38 µs | 224 µs | 83% |
| 200x60 | 12,000 | 949 µs | 140 µs | 1,089 µs | 87% |
| 320x90 | 28,800 | 2,077 µs | 311 µs | 2,388 µs | 87% |

Both halves are proportional to window cells - painting at roughly 72 to 97 ns per cell, diffing at
11 to 20 - and painting is about 6.7 times the diff. The probe also counted 31.4 frames a second
against a 30 Hz guest, which is the third independent confirmation that paints track updates one to
one.

At 200x60 that accounts for 1,089 µs of the 1,723 µs an update costs. Of the remainder, about
235 µs is the visible-grid snapshot that `refresh_live_terminals` rebuilds at the top of the same
function - the figure measured independently above - leaving roughly 400 µs in the mailbox, the
event loop, and cursor handling.

## Interpretation

- Confirmed defect: per-update client cost is proportional to total window cells and independent of
  both the changed region and the changing pane's size.
- Confirmed not the cause: scrollback depth, at any size, on either the ingest or the snapshot path.
- Confirmed not the cause: the session server, flat near 1% across every rate measured.
- Confirmed working: output coalescing and the frame diff. Updates map one to one onto paints, and
  about 100 bytes reach the terminal per update at every rate and viewport.
- Confirmed target: painting the tree into the next buffer, at 87% of the draw and about 56% of
  everything an update costs. The diff is real but secondary at 8%, and the snapshot rebuild is
  14%. Optimizing either of those first would be optimizing the small half.
- Not a fix on its own: scoping the repaint to the dirty *pane*. It would explain the split-window
  result, but the reported case is a single full-window pane, where the pane rect is still every
  cell. The granularity that matters is the terminal cells that changed, which is information the
  emulator already has and the render path currently discards.

## Fixed, and by how much

The framework now carries the emulator's damage through to the draw. `alacritty_terminal` already
reports which lines changed; the render path discarded it. tui-lipan gained `Update::terminal_paint()`,
which asserts that live terminal content is the only thing that looks different this frame, and a
frame that keeps that claim repaints only the damaged viewport rows - through the ordinary tree walk
in the real render context, clipped to one row, so a patched row is by construction the cells a full
paint would have produced there. `output_frame_update` returns it where it returned `Update::paint()`;
the indicator and chrome checks beside it are exactly what would make the claim untrue.

Same harness, same host, one changed character at 30 updates a second, animations off. The
sampling window is 30 s rather than the 6 s used above: at 0.6% of a core, 6 s of `utime + stime`
is three or four clock ticks, and a one-tick quantum is a third of the reading.

| Viewport | Cells | Before | After | Change |
| --- | ---: | ---: | ---: | ---: |
| 80x24 | 1,920 | 410 µs | 205 µs | −50% |
| 200x60 | 12,000 | 1,672 µs | 580 µs | −65% |
| 320x90 | 28,800 | 3,652 µs | 1,148 µs | −69% |

And the reported case itself, 200x60 at 50 updates a second - the rate the 8% figure corresponds to:

| | Client % | Server % | CPU per update |
| --- | ---: | ---: | ---: |
| Before | 8.20 | 0.87 | 1,624 µs |
| After | 2.60 | 0.83 | 515 µs |

The shape matters more than the number. 320x90 cost 8.9 times 80x24 for the same one-row change;
it now costs 5.6, against 4.0 for the row width alone. Fitting the three points to
`a·width + b·cells + c` gives about 0.7 µs per column, 29 ns per cell, and 93 µs fixed. That
per-cell term is the visible-grid snapshot `refresh_live_terminals` rebuilds every frame - measured
independently above at about 20 ns per cell - and no longer painting. Painting has left the profile.

Host output fell slightly too, from about 104 to 89 bytes per update. It was never the problem -
the diff was already emitting one changed cell for one changed cell - so this is a side effect
worth recording rather than a result.

Two things this deliberately did not do. Column-level damage is tracked and unused: row granularity
already turns 12,000 cells into 200 at 200x60, and the numbers above say the remainder is elsewhere.
And the snapshot rebuild is now the largest per-cell term, but it is a different subsystem and a
separate decision.

## What the first cut got wrong

tui-lipan 0.7.0 shipped this with the caret unplaced. A full draw ends by telling the host where the
caret goes; the row repaint never called that, so the caret stayed where crossterm's last cell write
left it - one column past the run it had just sent. Typing at a prompt made it wander a cell at a
time, and a block caret parked on a prompt glyph read as if the glyph had been deleted.

The cells were never wrong, which is exactly why it survived: the correctness matrix compared a
patched frame against a full paint cell by cell, and a caret is not a cell. A partial repaint owes
the host two things and only one of them was being checked. tui-lipan 0.7.1 places the caret and
holds it to the same oracle, and Rozi requires that version.

The measurements above are unaffected - placing a caret is one escape sequence, and the extra row
the fix paints is a row that was usually already damaged.

## Reproducing

The harness is not committed; it is a variant of `tools/memory-matrix.sh` that replaces the pane
workload with a rate-controlled spinner and samples `utime + stime` rather than PSS. The pieces
worth keeping are the isolated root, the `script`-hosted client, and reading the wrapper's log size
for host bytes.

The paint-versus-diff split needed no profiler: a temporary timer inside `draw_current_tree`
around `render(f, &ctx)`, against one around the whole `terminal.draw(...)`, reported to a file
named by an environment variable. That instrumentation was not kept.

A finer attribution *within* painting would need a sampled profile, and that is blocked here:
`kernel.perf_event_paranoid` is 2, `perf` is not installed, and the
[2026-09-03 audit](2026-09-03.md) recorded that attaching samply to a `script`-hosted client
strands the PTY wrapper. Lowering the sysctl is an owner action.
