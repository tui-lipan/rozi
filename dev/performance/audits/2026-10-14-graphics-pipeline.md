# Graphics pipeline acceptance, 2026-10-14

This audit measures the incremental Kitty graphics pipeline in rozi and tui-lipan against the
release it replaces, with synthetic graphics streams and a real browser. During the audit, five
defects were found and fixed in the measured pipeline. Every number below comes from the fixed
revisions unless a row is labelled as the baseline.

## Revisions and environment

- Baseline: rozi `64571986` (`master`) with tui-lipan 0.22.7 from crates.io. Release binary SHA-256
  `309fbb8f20c7beee…`.
- Measured: rozi `feat/graphics-presentation` at `c31f5499`, pinned to tui-lipan `49ae792` from
  [tui-lipan #337](https://github.com/tui-lipan/tui-lipan/pull/337). Release binary SHA-256
  `8d64b70ec5d3484a…`; the synthetic rows use it. The main browser table used the build of `d440eeb1`
  (SHA-256 `165ba07a6b0e79ac…`), which differs only in when paced reads are released. The pacer
  never engaged in any browser run (zero holds and zero replaceable transactions), so those rows
  also describe `c31f5499`. The browser-overspeed table uses the final build.
- Machine: AMD Ryzen 7 9700X (16 threads), Linux 7.2.5, `performance` power profile, Rust 1.96.1.
- Host: Ghostty 1.3.1 on Hyprland 0.56.2, DP-2 at 3840 × 2160, 239.99 Hz, scale 1. Each run opens a
  fresh 980 × 720 floating Ghostty window with JetBrainsMono Nerd Font at 8 pt and isolated
  configuration, data, state, cache and runtime directories. rozi animations are disabled.
- No compilation or other heavy work ran during sampling. A game running during an earlier pass was
  closed and those samples were discarded.

Every run uses `tools/graphics-benchmark.py`, described in the [benchmark guide](../../benchmarks.md).
Host traffic is taken from successful writes by the rozi client (`--tap client-write`). Rates are
presented frames, never producer updates. Each case is a single 20-second sample unless noted.

## Defects found during the audit

**Background messages did not wake the UI thread** (tui-lipan `c16a1db`). `CommandLink` sends
signalled a channel that only headless automation read. Pane output reaches the rozi client that
way, so once the loop had no frame pending each delivery waited for the 50 ms idle timeout plus one
frame slot. At 240 Hz, presentation fell into a repeating 54.2 ms stall after about 13 seconds; at
120 Hz the stall was 58.4 ms. The loop now installs a coalesced wake hook, and the unbounded channel
it replaced no longer grows with every send.

**Paced reads presented the oldest queued frame** (tui-lipan `49ae792`, rozi `bef2af7a`). While
the pacer withholds read credit, the producer's output queues in the kernel oldest first. A credit
sized to one frame then delivered the oldest frame on every release, and backpressure kept the
queue full, so staging never saw two replacements to coalesce. A 240 fps producer paced to a 60 Hz
client trailed it by about a second (p50 1022 ms, max 1959 ms). Each credit now drains the pending
backlog, up to 64 KiB, as one event, and staging keeps only the newest independent replacement.

**A stopped client kept the pacing lead** (rozi `ace8bc37`). A stopped or wedged client stays
attached, so the server kept pacing to it and every release waited out the 200 ms safety hold. A
live read-only follower then presented 16 bursty frames per second. A client whose oldest
unacknowledged output is more than 250 ms old now leads only when no responsive visible client
remains. Per-client presentation state is also pruned to live pane generations.

**Paced releases ignored the leader's phase** (rozi `c31f5499`). Releases followed a fixed grid at
the leader's frame rate, so the phase between that grid and the client's own frame clock became
latency: 2.5 to 9.9 ms between runs for a 240 fps stream at 60 Hz. Each acknowledged presentation
now schedules the next release 3 ms before the leader's next frame.

**Composed frames could reuse superseded pixels** (tui-lipan `746eaa2`). Composed terminal images
keep one host image per placement stream, and the encode cache keeps the previously sent frame as a
bridge. A producer returning to that frame's exact pixels hit the bridge entry, whose payload had
already been sent and released, so the host kept showing the newer frame. Composition is the
reference path for captures, recordings and remote clients. This defect does not affect the
measurements below, which present natively.

## Synthetic stream

`tools/graphics-stream.py` replaces one 640 × 320 RGBA image (`i=7`, `p=9`) with a moving rectangle
from 64 immutable `t=f` files. Latency runs from the producer's write returning to the host write
that ends the frame carrying that image, matched through an index stamped in each fixture's first
pixel. CPU is percent of one core (client / server / Ghostty).

| Client ← producer Hz | Build | Presented/s | Upload interval p99 / max, ms | Latency p50 / p99 / max, ms | Host KiB/s | CPU |
|---|---|---:|---|---|---:|---|
| 60 ← 60 | baseline | 58.2 | 17.41 / 17.6 | 8.70 / 16.86 / 17.0 | 145 | 2.6 / 0.7 / 5.7 |
| 60 ← 60 | measured | 59.9 | 16.82 / 33.3 | 0.27 / 15.31 / 15.3 | 12 | 1.4 / 0.9 / 3.9 |
| 120 ← 120 | baseline | 113.9 | 8.99 / 17.3 | 4.60 / 8.64 / 8.8 | 284 | 4.4 / 1.3 / 9.7 |
| 120 ← 120 | measured | 120.0 | 8.43 / 10.7 | 0.24 / 8.18 / 8.2 | 25 | 2.5 / 1.4 / 7.1 |
| 144 ← 144 | baseline | 135.3 | 7.57 / 7.8 | 3.85 / 7.23 / 7.3 | 337 | 5.4 / 1.4 / 10.6 |
| 144 ← 144 | measured | 144.0 | 7.07 / 9.3 | 1.25 / 1.36 / 2.6 | 29 | 2.7 / 1.5 / 7.1 |
| 240 ← 240 | baseline | 218.4 | 4.76 / 5.6 | 2.44 / 4.45 / 4.8 | 545 | 7.9 / 2.2 / 13.6 |
| 240 ← 240 | measured | 240.0 | 5.10 / 6.7 | 0.36 / 1.88 / 2.7 | 49 | 4.3 / 2.3 / 8.8 |
| 60 ← 240 | baseline | 57.6 | 17.64 / 18.0 | 2.51 / 4.52 / 6.4 | 143 | 3.7 / 2.1 / 5.6 |
| 60 ← 240 | measured | 60.0 | 16.72 / 17.5 | 6.04 / 6.62 / 6.6 | 12 | 1.3 / 2.1 / 3.7 |
| 120 ← 240 | baseline | 113.1 | 9.02 / 9.2 | 2.47 / 4.48 / 4.7 | 282 | 5.2 / 2.1 / 9.7 |
| 120 ← 240 | measured | 120.0 | 8.38 / 9.3 | 3.55 / 3.95 / 4.9 | 25 | 2.3 / 2.3 / 6.6 |

The baseline drops 3–9% of frames at every rate; the measured build presents every client frame
with an eleventh of the host traffic and less client and Ghostty CPU. At matched rates the latency
p99 reflects the slowly drifting phase between the producer's and the client's clocks.

When the producer outruns the client, the baseline reads everything as it arrives, so the newest
frame is presented 2.5 ms after it is written, but the producer runs unconstrained. The measured
build paces: it releases reads 3 ms before each client frame, so latency is that lead plus the
newest frame's age, 3.5–6.6 ms across samples at 60 Hz (a separate sample: p50 3.75 ms, max
4.72 ms). This is within a frame but up to 4 ms more than the unpaced baseline. The small `t=f`
commands do not fill the kernel's PTY buffer between releases, so this producer is not blocked;
the server parses all 240 commands per second at about 2% of a core and forwards only the newest.
Recorded at rozi's PTY reads, frames wait p50 8.3 ms (max 18.2 ms) in the kernel for the next
release, by design; the presented frame is the newest of them.

Before the backlog fix, the same 60 ← 240 case presented the oldest queued frame: latency p50
1022 ms, max 1959 ms.

### Input, visibility and other clients

Keys are delivered to the owned window with Hyprland's `send_shortcut` and pass through Ghostty and
the client's input path. The producer echoes each one in cbreak mode. About 3.5 ms of each total is
starting `hyprctl` and compositor delivery.

| Case | Key to host echo p50 / p95 / max, ms | Producer echo to host, max |
|---|---|---:|
| measured, 60 ← 240 (paced) | 4.29 / 5.66 / 5.90 | 1.22 |
| measured, 60 ← 60 (not paced) | 4.75 / 5.44 / 5.76 | 3.05 |
| baseline, 60 ← 240 | 3.81 / 21.11 / 21.16 | 17.52 |

Pacing does not delay input echo.

- **Hidden pane.** With the pane on another rozi workspace for eight seconds, nothing is
  presented. On reveal, the retained frame (68 ms old) appears after 1.5 ms and the next frame is
  current. The 240 fps producer keeps writing while hidden; see the paragraph on small commands.
- **Stopped follower.** A 60 Hz writable follower is stopped for six seconds beside a 240 Hz leader.
  The leader presents 239.9 frames/s (latency p95 3.2 ms during the stop), the server's per-client
  outboxes peak at 4.2 KB while coalescing, and the follower shows a current frame 9 ms after it
  resumes.
- **Stopped leader.** With the 240 Hz leader stopped for six seconds, a read-only 60 Hz follower keeps
  presenting 58 frames/s at p50 4.0 ms. Before the stalled-leader fix it got 16 frames/s in bursts.
- **Mixed workload and resize.** Title, text, query and image delete/recreate barriers once a
  second, plus a window resize, at 60 ← 240: 59.5 frames/s, latency max 10.2 ms. The only gap is
  192 ms after the resize; the baseline shows the same gap (about 200 ms), so it predates this work.
- **Exit while paced.** When the producer exits during pacing, the pane closes and Ghostty receives
  the image and placement deletes 9.9 ms after the last frame.

## Browser producer

terminal-browser 0.13.4 runs with the native module from
[terminal-browser #157](https://github.com/zenbu-labs/terminal-browser/pull/157), built locally
because that PR is not merged. The installed application is copied, and only the copy's module is
replaced. GPU rendering is disabled, `render.fps` matches the client frame rate, and telemetry and
update checks are off. The generated page (`tools/graphics-page.html`) animates six rectangles and a
counter on a canvas. Raffles (`https://www.raffles.com`) runs from a copy of an existing profile that
had dismissed the cookie notice; the copy is deleted after each run. Each run warms up for eight
seconds. `auto` is the browser's normal patch presentation; `full` sends whole frames.
CPU is percent of one core; "browser" sums the browser's process tree.

| Page | Presenter | Hz | Build | Graphics frames/s | Interval p50 / p99 / max, ms | Host KiB/s | Placements/s | Client | Ghostty | Browser |
|---|---|---:|---|---:|---|---:|---:|---:|---:|---:|
| generated | auto | 60 | baseline | 42.4 | 24.13 / 26.95 / 30.7 | 6126 | 52203 | 9.9 | 76.3 | 17.1 |
| generated | auto | 60 | measured | 59.7 | 16.71 / 19.06 / 21.7 | 46 | 181 | 5.1 | 8.5 | 16.6 |
| generated | auto | 120 | baseline | 64.0 | 16.39 / 19.00 / 20.9 | 9536 | 80604 | 16.2 | 115.4 | 32.5 |
| generated | auto | 120 | measured | 118.9 | 8.37 / 9.80 / 13.3 | 90 | 352 | 9.7 | 15.9 | 32.0 |
| generated | auto | 240 | baseline | 85.8 | 12.52 / 15.58 / 17.9 | 12201 | 102690 | 23.9 | 133.4 | 62.3 |
| generated | auto | 240 | measured | 233.2 | 4.21 / 6.04 / 10.0 | 180 | 696 | 18.8 | 22.5 | 61.3 |
| generated | full | 60 | baseline | 56.9 | 17.53 / 18.05 / 18.4 | 258 | 2048 | 4.9 | 10.2 | 18.4 |
| generated | full | 60 | measured | 59.8 | 16.72 / 17.78 / 19.4 | 14 | 60 | 3.5 | 6.2 | 17.9 |
| generated | full | 120 | baseline | 107.7 | 9.21 / 9.80 / 18.1 | 489 | 3879 | 9.5 | 17.1 | 35.3 |
| generated | full | 120 | measured | 119.0 | 8.38 / 9.38 / 11.3 | 29 | 119 | 6.8 | 11.8 | 36.3 |
| generated | full | 240 | baseline | 194.2 | 5.06 / 5.82 / 9.7 | 890 | 6993 | 17.8 | 26.7 | 70.1 |
| generated | full | 240 | measured | 233.4 | 4.22 / 5.68 / 12.3 | 57 | 233 | 13.4 | 19.5 | 70.6 |
| Raffles | auto | 60 | baseline | 28.1 | 35.25 / 102.20 / 337.5 | 1118 | 9404 | 3.2 | 13.4 | 18.2 |
| Raffles | auto | 60 | measured | 22.7 | 33.34 / 400.15 / 968.0 | 16 | 64 | 1.9 | 2.9 | 18.0 |
| Raffles | auto | 120 | baseline | 23.2 | 35.51 / 404.61 / 967.6 | 1028 | 8635 | 2.9 | 12.0 | 20.6 |
| Raffles | auto | 120 | measured | 23.1 | 33.31 / 400.32 / 968.9 | 16 | 64 | 1.8 | 3.0 | 18.3 |
| Raffles | auto | 240 | baseline | 23.8 | 31.75 / 399.27 / 970.0 | 1149 | 9713 | 3.1 | 12.9 | 19.3 |
| Raffles | auto | 240 | measured | 24.5 | 33.28 / 400.71 / 968.4 | 16 | 65 | 1.9 | 3.1 | 19.1 |
| Raffles | full | 60 | baseline | 21.6 | 34.29 / 569.54 / 1373.1 | 98 | 779 | 2.0 | 3.9 | 18.0 |
| Raffles | full | 60 | measured | 21.7 | 33.30 / 566.99 / 1366.5 | 5 | 22 | 1.3 | 2.5 | 18.1 |
| Raffles | full | 120 | baseline | 22.5 | 34.39 / 572.15 / 1369.9 | 102 | 810 | 2.1 | 4.1 | 18.6 |
| Raffles | full | 120 | measured | 22.7 | 33.30 / 566.79 / 1366.5 | 5 | 23 | 1.5 | 2.6 | 18.6 |
| Raffles | full | 240 | baseline | 23.7 | 34.53 / 499.59 / 1365.1 | 108 | 855 | 2.1 | 4.1 | 19.3 |
| Raffles | full | 240 | measured | 24.1 | 33.28 / 566.84 / 1366.9 | 6 | 24 | 1.5 | 2.7 | 19.4 |

On the generated page the baseline recreated every placement each paint and could not keep up:
normal patch presentation reached 42, 64 and 86 frames/s at 60, 120 and 240 Hz. The measured build
presents at the configured rate. Host traffic falls by a factor of 66–133 and Ghostty CPU by a
factor of 6–9, while the browser's own CPU is unchanged. Whole-frame presentation also improves,
most at 240 Hz (194 to 233 frames/s).

Raffles is limited by the page, not by either build. Its video advances at about 22–25 frames/s with
recurring pauses of 0.4 to 1.4 seconds, so the frame rate and interval tails are the same for both
builds within sample variation. Two further samples of the 60 Hz pair gave 22.3 and 22.1 frames/s
for the baseline and 22.2 and 22.0 for the measured build, so the first baseline sample (28.1) was
an outlier. The measured build sends about 1/70 of the host
traffic and spends about a quarter of the baseline's Ghostty CPU on the same page.

### Browser faster than the client

With the browser rendering at 240 fps for a 60 Hz client, `--pty-tap` also records the producer's
own output as rozi's server reads it.

| Presenter | Build | Presented/s | Interval p99 / max, ms | Producer frames/s | Host KiB/s | Client | Server | Ghostty | Browser |
|---|---|---:|---|---:|---:|---:|---:|---:|---:|
| full | baseline | 54.2 | 19.1 / 19.7 | 233.6 | 246 | 9.3 | 7.0 | 6.3 | 68.1 |
| full | measured | 60.0 | 17.4 / 17.7 | 233.5 | 15 | 8.2 | 6.9 | 3.8 | 67.4 |
| auto | baseline | 38.3 | 30.4 / 36.0 | 234.1 | 5757 | 15.1 | 7.2 | 37.2 | 62.5 |
| auto | measured | 60.0 | 17.6 / 17.7 | 233.9 | 124 | 10.4 | 7.0 | 7.8 | 58.9 |

The measured build presents every client frame, but the browser keeps rendering 234 frames/s and
the client and server process all of them. terminal-browser is not paced or coalesced: each
whole frame is a synchronized update containing a cursor move, a placement delete and a
transmit-and-place of the same image and placement (`a=d,d=i,i=1,p=1` then `a=T,…,t=f,i=1,p=1`).
Synchronization, cursor movement and deletion are ordered operations, so no single command is a
parser-proven independent replacement, as the design requires. Patched presentation is a
dependency chain by design. Recognising a synchronized update whose net effect is one replacement
would let such streams coalesce and backpressure; it is not implemented. Running the browser at
the client's frame rate, as in the main table, avoids the wasted work.

## Comparison with TuiOS

TuiOS 0.9.2 (`937cc693`) passes Kitty graphics through to the host. It is written in Go and does not
write through libc, so both multiplexers are measured at the same point: what Ghostty reads from
its PTY (`--tap host-read`). The producer's window is TuiOS's only window. TuiOS has no client frame
rate; rozi's client runs at the stated rate. Latency ends at Ghostty's read of the frame. PSS is the
client and server together.

| Case | Multiplexer | Graphics frames/s | Interval p99 / max, ms | Latency p50 / p99 / max, ms | Host KiB/s | Client | Server | Ghostty | PSS, MiB |
|---|---|---:|---|---|---:|---:|---:|---:|---:|
| synthetic 60 ← 60 | rozi | 60.0 | 16.79 / 19.0 | 0.27 / 14.79 / 14.8 | 12 | 1.4 | 0.9 | 3.8 | 29.1 |
| synthetic 60 ← 60 | TuiOS | 60.0 | 16.86 / 17.2 | 0.28 / 0.47 / 0.8 | 15 | 2.1 | 0.5 | 4.0 | 71.6 |
| synthetic 120 ← 120 | rozi | 120.0 | 8.41 / 10.7 | 0.25 / 6.05 / 6.1 | 25 | 2.4 | 1.3 | 6.9 | 28.8 |
| synthetic 120 ← 120 | TuiOS | 60.3 | 19.75 / 20.8 | 5.60 / 13.83 / 14.7 | 26 | 2.3 | 0.6 | 4.2 | 74.0 |
| synthetic 240 ← 240 | rozi | 240.0 | 5.10 / 6.5 | 1.31 / 2.87 / 4.0 | 49 | 4.1 | 2.2 | 9.2 | 28.8 |
| synthetic 240 ← 240 | TuiOS | 60.6 | 17.68 / 17.9 | 6.64 / 14.92 / 15.9 | 48 | 2.9 | 0.8 | 4.8 | 75.9 |
| synthetic 60 ← 240 | rozi | 60.0 | 16.72 / 17.6 | 6.11 / 6.75 / 6.8 | 12 | 1.3 | 2.0 | 3.8 | 29.0 |
| synthetic 240 producer, repeat | TuiOS | 60.8 | 17.66 / 17.7 | 6.66 / 14.94 / 15.9 | 48 | 2.8 | 0.8 | 4.7 | 75.7 |
| browser generated, auto, 240 | rozi | 233.6 | 5.89 / 9.8 | | 179 | 18.1 | 6.6 | 22.6 | 28.0 |
| browser generated, auto, 240 | TuiOS | 60.0 | 17.25 / 17.7 | | 25 | 7.7 | 1.4 | 11.2 | 77.0 |
| browser generated, full, 240 | rozi | 234.2 | 5.43 / 7.3 | | 57 | 12.8 | 6.7 | 19.9 | 26.1 |
| browser generated, full, 240 | TuiOS | 60.0 | 17.40 / 17.7 | | 25 | 7.8 | 1.4 | 10.8 | 77.4 |
| browser Raffles, auto, 60 | rozi | 22.5 | 400.36 / 968.4 | | 16 | 1.9 | 1.1 | 3.0 | 26.8 |

TuiOS forwards graphics within its own screen updates, which in this setup reach about 60 per
second whatever the producer's rate. At 60 Hz it matches rozi's frame rate with a tighter latency
tail; rozi's 60 Hz p99 reflects the phase between its frame clock and the producer's. Above 60 Hz,
rozi presents 120 and 240 frames per second with lower latency, at a higher client and Ghostty CPU
cost for the extra frames. rozi's client and server use about 29 MiB PSS against TuiOS's 72–77 MiB.

Raffles could not be measured under TuiOS: in two attempts, Ghostty received about 25 KB from
TuiOS before sampling began and nothing during the 20-second sample, while the browser stayed busy.
The cause was not investigated.

## Native and composed pixels

`examples/terminal_patch_layers.rs` (tui-lipan `746eaa2`, debug build) runs in two fresh 980 × 720
Ghostty windows at the same screen position, one native and one with
`TUI_LIPAN_TERMINAL_IMAGES=composed`. Each state is captured with `grim` after keys delivered by
Hyprland. A modal traps keyboard focus, so the modal and portal states start their own windows.

| State | Differing pixels (whole window) | Native Kitty commands for the step |
|---|---:|---|
| image | 0 | none (already uploaded at start) |
| opaque text overlay | 0 | composition takes over: 2 deletes, 1 composed upload |
| opaque block overlay | 0 | none |
| image again | 0 | 1 source upload, 1 placement |
| source replaced (blue) | 0 | 1 source upload, 1 placement |
| source replaced (green) | 0 | 1 source upload, 1 placement |
| text overlay again | 0 | 2 deletes |
| image final | 0 | 1 source upload, 1 placement |
| three unchanged actions | not captured | none |
| transparent modal | 0 | none |
| terminal in a portal | 0 | none |

Before `746eaa2`, the second replacement differed in 19,584 pixels: the composed window still
showed the first replacement's colour.

## Verdict

The measured build presents every client frame at 60, 120, 144 and 240 Hz where the baseline
dropped 3–9% of synthetic frames and, for a browser's normal patch presentation, up to 64% of them.
Host traffic falls by an order of magnitude or more, and client and Ghostty CPU fall with it. Input
echo is unaffected by pacing, a stopped follower or leader no longer slows the other client, and
native output matches composition pixel for pixel in every scene checked.

Limitations and follow-ups:

- When a producer outruns the client, the newest frame is presented 3.5–6.6 ms after it is
  written, against 2.5 ms for the unpaced baseline, which presented fewer frames.
- A small-command producer such as this synthetic stream is not blocked by pacing; the server parses
  every frame (about 2% of a core at 240 frames/s) and forwards only the newest.
- terminal-browser output is never paced or coalesced, because its whole frames combine ordered
  operations. Recognising a synchronized update with the net effect of one replacement is not
  implemented.
- A window resize takes about 200 ms to present the next upload, the same as the baseline.
- Raffles is producer-bound at about 22–25 frames/s, and could not be measured under TuiOS.
- Each case is one 20-second sample unless noted. GPU scanout and Kitty hosts other than Ghostty
  were not measured.
