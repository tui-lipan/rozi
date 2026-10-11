# Native Kitty patch forwarding, 2026-10-11

Natural-size patches can be uploaded once and placed directly in Ghostty, leaving pixel composition
to the host. The framework retains the producer's images for captures, recordings and reconnects.
The measurements below cover generated graphics traffic, not a terminal-browser page benchmark.

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
