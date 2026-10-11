# Native Kitty patch forwarding, 2026-10-11

Natural-size patches can be uploaded once and placed directly in Ghostty, leaving pixel composition
to the host. The framework retains the producer's images for captures, recordings and reconnects.
The first measurements cover generated graphics traffic; a separate Raffles follow-up measures
the placement lookup fix on a real page.

## Setup

- Machine: Intel Core Ultra 5 338H, 12 logical CPUs, Linux.
- Host: Ghostty 1.3.1-arch2, ReleaseFast, JetBrainsMono Nerd Font at 12 pt.
- Rozi: based on `60742a5a`, release build with framework revision `f1e6085b`
  from [tui-lipan #328](https://github.com/tui-lipan/tui-lipan/pull/328).
- Pane: 79 columns × 19 rows, 1501 × 798 pixels, 19 × 42 pixels per cell.
- Workload: `tools/graphics-patches.py`, 20 updates per second, reusable raw RGBA files.
  The patch scene retains a background and toolbar and changes a 37 × 23 opaque patch plus a
  moving 127 × 75 translucent patch. Full-frame mode sends a new background-sized image each tick.
- Each run has independent scratch configuration, data, state, cache and runtime directories,
  a fresh Ghostty process and an owned temporary session. Animations are disabled.
- Six seconds of warm-up precedes 20 seconds of sampling at 100 ms intervals. CPU is the change
  in `/proc/PID/stat` user + system ticks, expressed as percent of one core. Memory is median RSS.
  The producer is excluded from client, server and host measurements.
- The same executable selects composition with `TUI_LIPAN_TERMINAL_IMAGES=composed`; an unset
  variable permits native forwarding. Runs are serial. Compilation is excluded from sampling.

## Results

The patch workload runs in composed, native, native, composed order. Each value below is the
mean of the two runs; all runs sustain approximately 20 updates per second.

| Measurement | Composed | Native |
| --- | ---: | ---: |
| Client CPU, % of one core | 8.24 | 1.52 |
| Client median RSS, MiB | 43.73 | 29.83 |
| Server CPU, % of one core | 1.37 | 1.34 |
| Server median RSS, MiB | 20.62 | 20.53 |
| Ghostty CPU, % of one core | 8.91 | 4.48 |
| Ghostty median RSS, MiB | 206.47 | 199.81 |
| Client host-terminal output, KiB/s | 366.90 | 61.14 |

Client CPU falls by about 5.4× and host-terminal output by 6× in this workload. Output is measured
by a scratch `LD_PRELOAD` counter around successful `write()` calls to terminal file descriptors.
It records byte counts only, including reopened backend descriptors, without logging payloads.
The counter is active in both modes.

The separate full-frame workload has one composed run followed by one native run, with the same
warm-up, viewport and sample duration. Both sustain approximately 20 updates per second.

| Measurement | Composed | Native |
| --- | ---: | ---: |
| Client CPU, % of one core | 19.42 | 4.73 |
| Client median RSS, MiB | 45.55 | 31.88 |
| Server CPU, % of one core | 4.38 | 4.53 |
| Server median RSS, MiB | 20.82 | 20.78 |
| Ghostty CPU, % of one core | 13.00 | 8.66 |
| Ghostty median RSS, MiB | 207.27 | 217.92 |

Client CPU falls by about 4.1× in this full-frame sample. Ghostty RSS rises by about 10.7 MiB,
so the client reduction does not imply lower host memory for every workload. Host-terminal output
is not instrumented in the full-frame runs. These single samples are a check of a different input
pattern, not evidence that full frames are preferable to patches.

## Raffles follow-up

The small generated scene above did not expose a CPU regression with a retained browser patch
stack. In a separate real-page check, the native path spent most sampled client CPU time in string
searching. `prepare_placed_cell()` searched every pending placement for each outgoing image cell,
and deletion collection searched the outgoing cells again for every placement. Native forwarding
keeps more producer planes, so it amplified this existing cost.

Framework revision `8543832d` indexes pending placements by their anchor cell, parses each cell's
placement commands once into exact-match keys, and collects deletions while preparing the cells
that actually retain those commands. Hidden anchors still omit uploads
and placements, and a source whose initial anchor disappears uploads before its first surviving
row. A regression covers 64 anchors, a second plane at one anchor, hidden anchors, reversed outgoing
cell order, upload reuse and deletion on the next frame.

The follow-up uses the same retained Raffles browser and session server, with only the client
reconnected between release binaries. The anonymous page is `https://www.raffles.com`; browser
GPU rendering is disabled and `render.fps` is fixed at 30 in this owned lab.
Ghostty opens directly as a 980 × 720 floating Wayland window, with JetBrainsMono Nerd Font
at 8 pt. The pane reports 147 × 45 cells. Its retained background is 1911 × 1260 source pixels.
The counter is confined to rozi, and the browser does not inherit it. Warm-up and sampling use the same six-second and 20-second durations as above.
Compilation is excluded. Two consecutive samples are taken per binary and the table gives their
means. Native placement rates are checked as well as output bytes, so an unequal amount of work cannot masquerade as a
CPU improvement. Earlier exploratory samples with inconsistent window geometry are excluded.

| Measurement | Before `f1e6085b` | After `81392dec` |
| --- | ---: | ---: |
| Client CPU, % of one core | 31.88 | 14.37 |
| Client median RSS, MiB | 56.76 | 55.00 |
| Server CPU, % of one core | 11.13 | 11.04 |
| Server median RSS, MiB | 50.49 | 50.49 |
| Ghostty CPU, % of one core | 30.16 | 26.89 |
| Ghostty median RSS, MiB | 158.07 | 147.74 |
| Client host-terminal output, KiB/s | 647.21 | 639.88 |
| Native placements per second | 5279.56 | 5229.52 |

Client CPU falls by 55% for approximately the same placement rate and terminal traffic.
The server is unchanged, and this comparison does not establish a material RSS reduction.

The generated-workload tables above were measured at `f1e6085b`; the follow-up records the placement
lookup fix separately. Ordinary placements are still recreated each paint.

A larger-window rendering check found raw frames of 34,059,248 bytes, above the former 32 MiB
transfer limit. Framework revision `81392dec` raises the named-media and chunked-inline limit
to 64 MiB, while keeping individual APC sequences bounded to 32 MiB. A sparse file just above
32 MiB is tested through file retention and inline replay with the original payload intact,
without populating either decoded-pixel cache. Configured file-retention budgets still apply.
A fresh patched server and client render the Raffles middle correctly at the larger size in both
a PNG capture and the live Ghostty window; the lab is then restored to 980 × 720.

### Native versus composition on Raffles

A fresh patched server and the same retained browser are then compared in composed and native
modes at the same window size and producer limit of 30 updates/s. Each mode has one fresh client
and Ghostty process, six seconds of warm-up and a 20-second sample. Captures are excluded from
these client lifetimes to avoid measuring their retained heap allocations. These are single samples;
producer rate is controlled, but host presentation rate is not measured.

| Measurement | Composed | Native |
| --- | ---: | ---: |
| Client CPU, % of one core | 9.82 | 15.39 |
| Client median RSS, MiB | 86.29 | 43.69 |
| Server CPU, % of one core | 10.62 | 11.70 |
| Server median RSS, MiB | 31.79 | 31.85 |
| Ghostty CPU, % of one core | 100.42 | 27.79 |
| Ghostty median RSS, MiB | 506.46 | 149.90 |
| Client host-terminal output, KiB/s | 165.96 | 638.62 |

Native forwarding reduces Ghostty CPU and RSS in this scene, while client CPU and terminal traffic
increase. Per-row placement recreation costs more than composed tiles here. This remains a limit
of the current forwarding path; persistent placement diffs are not implemented. The lookup fix
above improves the native path without establishing that it is cheapest for the client on every page.

## Rendering and lifetime checks

Live Ghostty inspection covers pixel offsets, a translucent patch over the retained base, the
preserved toolbar, resizing, opening and closing the command palette, and client reconnect.
Patched terminal-browser is also checked on `https://www.raffles.com`, including scrolling and
palette transitions.
A reconnect is tested with the producer stopped and the new window sized before attachment, so
retained images have to render without a producer refresh. A transient size change can legitimately
move scroll-anchored images, so it is kept separate from the reconnect check.

Framework regressions cover clipping above and left of a pane, overlay holes, scaling and same-z
fallbacks, relocation of an upload whose first row was removed, multi-placement source reuse,
replacement ID isolation, cross-thread source retirement, and failed/discarded output recovery.
Closing a pane retires both ordinary placements and unreferenced native host image data.
Captures keep the existing composed path and its PNG-safe underlay semantics.

## Limits

Native placement requires an unscaled source, matching producer/host cell sizes, a Kitty host and
no intersecting backdrop effect. Large sources require file or shared-memory host transport;
without either, sources above 128 KiB use the asynchronous composed encoder to keep compression
off the UI thread. Same-z pixel overlaps compose because host image IDs cannot
preserve the producer's tie-breaking order. Other protocols retain their existing path.

Ordinary placements are recreated per visible row on each paint. Source uploads are reused, but
persistent placement diffs are not implemented. The measurements establish savings for these
workloads, not for every page, animation or host. File-backed pixels also consume filesystem cache
which is not represented by process RSS. Host memory includes GTK and GPU allocations.
