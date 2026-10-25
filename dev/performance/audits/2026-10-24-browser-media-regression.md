# Browser performance after consuming-media support

Automatic terminal-browser transport becomes substantially more expensive after Rozi #241.
The browser probes shared memory first. Previously Rozi rejected that probe and the browser
used reusable files. Rozi now accepts it, then compresses and relays every shared-memory frame
inline, even to a local client. The client parses, hashes and inflates that inline payload.
This bypasses the file-backed path measured in #147 and the October 14 graphics audit.

`TERMINAL_BROWSER_PRESENT=full` changes presentation, not transport selection. Both presentation
modes therefore take this expensive route. A separate producer-wire recording confirmed a
successful shared-memory probe followed by `a=T,f=32,s=1768,v=901,t=s` uploads.

## Controlled comparison

Measured on Linux 7.2.8-9-omarchy, AMD Ryzen 7 9700X, Rust 1.96.1, Ghostty
1.3.1+omarchy.3 and terminal-browser 0.13.4. Both Rozi revisions use registry tui-lipan 0.29.0
and the same Cargo.lock. No source, installed browser module or personal configuration was changed.

- Before: Rozi `3a07e529`, immediately before #241. Release binary SHA-256
  `ac82c7689f06750bf8136bcc80ed44ad62cca683b2ad7fff16e1686f7902a027`.
- Current: Rozi `9f914739`, the merged #241. Release binary SHA-256
  `2e41dcabdd533459f76c022f60bcf40855660d905ea9ad009d922939e664d301`.
- A fresh, isolated, visible 1800×1000 Ghostty window per run, font size 8, monitor scale 1.
  The producer viewport was 221 columns × 53 rows, 1768×901 pixels.
- A loopback HTML page played an autoplaying, muted, looping 1600×900, 60 fps WebM test video.
  It was generated with FFmpeg `testsrc2`, eight seconds of VP9 at 3 Mbit/s. Video SHA-256
  `aa165f5e552926ca18b90a943b566ba8c2c63b4fcb6a252b9d9d48006632514c`.
- The maintained `tools/graphics-benchmark.py` and `tools/graphics-browser.sh` ran the browser
  at 60 fps with GPU rendering disabled. Eight seconds of warmup and twenty seconds of sampling.
  No compiler, capture request or interposer ran during the CPU samples.
- CPU is user+system time as a percentage of one core, sampled separately for client, server,
  browser tree and Ghostty. The table shows the ranges of two runs, except normal presentation,
  which has one run. Runs were serial: current shared, current file, before, current normal,
  current file, before, current shared. A separate diagnostic wire capture was excluded.

| Build and presentation | Frame transport | Client CPU % | Server CPU % | Client frames written/s |
| --- | --- | ---: | ---: | ---: |
| Before, full | Automatic, file | 8.20–8.50 | 4.20–4.65 | 59.8 |
| Current, full | Automatic, shared memory | 53.45–53.60 | 56.25–56.50 | 59.7–59.8 |
| Current, full | Forced file | 6.95–7.20 | 3.80–3.85 | 49.6 |
| Current, normal | Automatic, shared memory | 53.95 | 56.50 | 59.8 |

Combined Rozi CPU rises from about 12–13% to 110% of one core. Ghostty stays within
10.8–13.6%; browser-tree samples span 72.45–86.35%. Current shared-memory full presentation ends
with 93.0–103.9 MiB client RSS, compared with 29.2–29.7 MiB before. These RSS endpoints do not establish
a leak. The small maintained six-rectangle animation still ran near 60 frames/s and used only
8.45% combined Rozi CPU with automatic transport, so it understated the video cost.

The counters above measure successful client frame writes, not browser updates or monitor scanout.
A separate physical screenshot confirmed the video and toolbar rendered. This workload reproduces
the CPU regression, but does not reproduce multi-second visible lag at 60 fps. Larger or harder
to compress frames were not measured. No YouTube or network-page performance claim follows.

## Investigation follow-ups

This section records the state when the investigation ended. The causes and fix below resolve it.

Forced files restore low CPU, but consistently write only 49.6 frames/s here, against 59.8 before.
This remains a separate throughput regression. Code inspection finds that every named command now
crosses the media worker, completed work is polled rather than waking the event loop, and output
prefixes and their trailing synchronized-update markers can span loop iterations. Their individual
contributions have not been isolated.

Prepared named media also retains decoded pixels immediately and bypasses `snapshot_file()`.
The pre-existing immutable-file storage optimization is therefore lost on the server even for
local `t=f`. This is a code finding; the file-mode server CPU samples do not show a corresponding
increase in this workload.

A complete fix should preserve single server ownership of consuming sources while avoiding
compression, base64 and re-inflation for local image delivery. Server-owned immutable file
references are a candidate; they require lifetime handling through queued output and replay.
Remote readers still need a representation they can access, and each pane must keep one shared
output representation. The file throughput regression also needs a focused scheduling experiment.
These are follow-up changes, not implemented by this investigation.

For a temporary workaround on the unfixed build, restart the browser with both
`TERMINAL_BROWSER_FRAMES=file` and `TERMINAL_BROWSER_PRESENT=full`. The latter avoids the
unmerged browser #157 partial-file issue in the installed browser. This recovers CPU in the
measured case, not the previous frame rate. The fix needs neither setting.

The #241 fairness benchmark excludes host rendering and client CPU. It measures unrelated-pane
latency and server CPU during a short offered batch, not sustained browser presentation cost.
Its fairness improvement remains valid, but did not establish that browser performance was preserved.

## Causes isolated

**CPU.** Confirmed by the measurements above and by the wire capture: the shared-memory relay
compresses, base64-encodes and chunks every frame on the server, and every client inflates it.

**Frame rate with files.** A completed media job did not wake the server loop. The worker handed
its result to an `mpsc` channel, and the loop, waiting on its event queue, committed it only after
an unrelated wake: the idle or graphics-pacing timeout. Each frame therefore missed its presenter
slot by a few milliseconds, which settles into a fixed 20.0 ms cadence (49.6 frames/s). A build
identical to the fix below except for the worker's wake reproduces 49.6 frames/s and the 20.0 ms
p50 interval exactly; with the wake it writes 59.8 frames/s.

| Build, forced file, full | Client CPU % | Server CPU % | Frames written/s | Interval p50 / p95 ms |
| --- | ---: | ---: | ---: | ---: |
| Fix without worker wake, `22220f82` | 7.10 | 4.80 | 49.6 | 20.0 / 21.1 |
| Fix, `0850b08d` | 8.25–8.30 | 4.60–4.85 | 59.7–59.8 | 16.7 / 17.2–17.3 |

**Lost file storage.** Confirmed in code, and visible as server RSS: prepared named media was
decoded to the heap and bypassed file snapshots, so the forced-file server ended at 34.7–35.5 MiB
against 16.8–16.9 MiB before #241. Restoring file storage returns it to 17.0–17.1 MiB.

## Fix

Framework (tui-lipan, unreleased at measurement time):

- Prepared raw out-of-band media goes into the screen's immutable file snapshots when file storage
  is enabled. `t=f` copies the producer's file; `t=t` and `t=s` reserve the snapshot first and
  stream the consumed source into it, with no intermediate pixel buffer. Failures fall back to the
  in-memory path with the same answers to the child.
- `GraphicsRelayPolicy::ConsumingAsFiles` relays a consuming transmission as a `t=f` reference to
  that snapshot. A named snapshot is never rewritten for another frame; it is deleted once neither
  the image store nor a bounded lease window (at least eight recent frames, or the last second,
  within 64 MiB per screen) keeps it. Replays still carry pixels. PNG, compressed payloads, probes
  and failed snapshots relay inline.

Rozi:

- The server selects `ConsumingAsFiles` while every attached client shares its filesystem, and
  `All` (inline pixels) otherwise, as before.
- The media worker wakes the server's event queue after each result. The wait checks a wake
  counter read before polling, so a completion between poll and wait is not lost.

The fix was measured as uncommitted changes on Rozi `9f914739` with a local path override to
tui-lipan `a9a1005` plus the framework change. Release binary SHA-256
`0850b08db3f98a3ed4417d0f51b6df8205bc541bec2519aceaf8dbef1d9cf372`. Same machine, window, fixture,
harness and settings as the controlled comparison; runs were serial. After measurement the
framework source gained only an Apple-only branch, which stores Darwin shared memory already
copied into memory in the snapshot too, plus comments; the Linux path measured here is unchanged. A fresh pre-#241 run made
after the fix builds matched the morning's baseline, so the earlier rows remain comparable.

| Build and presentation | Frame transport | Client CPU % | Server CPU % | Frames written/s | Server RSS MiB |
| --- | --- | ---: | ---: | ---: | ---: |
| Before #241, full (3 runs) | Automatic, file | 8.20–8.50 | 4.20–4.75 | 59.8 | 16.3–16.9 |
| Current, full (2 runs) | Automatic, shared memory | 53.45–53.60 | 56.25–56.50 | 59.7–59.8 | 36.4–36.6 |
| Fix, full (3 runs) | Automatic, shared memory | 7.80–8.00 | 8.60–8.90 | 59.7–59.8 | 16.7–17.5 |
| Fix, normal (2 runs) | Automatic, shared memory | 7.75–7.95 | 8.60–8.85 | 59.7 | 17.0–17.6 |
| Fix, full (2 runs) | Forced file | 8.25–8.30 | 4.60–4.85 | 59.7–59.8 | 17.0–17.1 |

Combined Rozi CPU with automatic transport falls from about 110% to 16.4–16.9% of one core.
Client RSS ends at 29.4–29.6 MiB, as before #241. Browser-tree CPU was 78.8–79.3% with shared
memory and 73.0–74.7% with forced files; the browser's own shared-memory path costs more than its
file ring. Ghostty stayed within 10.9–11.7%.

Shared-memory server CPU stays about four points above file mode. A one-run experiment that let
relayed snapshots be rewritten for later frames measured 5.55%: most of the difference is a fresh
tmpfs file per frame. Reuse would let a reader that falls behind the lease window read a file while
it is being overwritten, so the fix keeps the never-rewrite guarantee. Streaming the consumed
source into the snapshot, instead of through a heap buffer, measured 8.60–8.90% against one 9.65%
sample and lowered server RSS from 29.4 to 17 MiB.

## Harder workload

A 1920×1080 noise video (`tools/graphics-video.sh DIR noise`, scene SHA-256
`641b13601964086c91ceed9ca5615f69e08ea081590d94041ff1e00901998fdb`) is close to incompressible.
The 1900×1050 window gave a 234×56 producer viewport of 1872×952 pixels. Automatic transport,
full presentation, one run each:

| Build | Client CPU % | Server CPU % | Browser tree % | Frames written/s | Interval p50 / p95 / p99 ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Before #241 | 9.75 | 5.40 | 110.15 | 59.6 | 16.7 / 17.4 / 17.9 |
| Current | 63.95 | 64.90 | 118.85 | 34.7 | 23.6 / 73.0 / 75.5 |
| Fix | 8.95 | 9.80 | 119.55 | 59.6 | 16.7 / 17.6 / 18.3 |

This reproduces the reported visible lag on the current build: frames arrive at 34.7 per second
with 73 ms p95 gaps. The fix restores the pre-#241 frame rate and intervals.

With forced files on the same scene, the current build wrote 50.1 frames/s at a 20.1 ms p50
interval (client 8.60%, server 4.45%); the fix wrote 59.6 frames/s at 16.7 ms (client 9.60%,
server 5.35%), one run each. A second automatic fix run measured client 8.90%, server 9.95%,
59.5 frames/s. Every harness run in this audit, on every build, presented all client frames to
Ghostty natively; none were composed.

## Unrelated-pane fairness

The named-image relay fairness probe from #240 ran on master and on the fix, serially, three
repetitions each, with no other work running. Both executables were optimized bench builds with
registry-identical dependencies except the framework change; their SHA-256 values were
`35e30c0752571502c8603652fc4719da728d6293df39b2d30a19a8cdf7077c21` (master, benchmark source
`12cea372…`, as in the image relay audit) and
`faae04023aefbf8d4e04300717736b38a6051419fa3fc178eaad9d123ce20370` (fix, benchmark source
`483fdef8…`). The fix's benchmark differs only in its output check: local readers now receive a
name for raw consuming media, so the probe expects inline output only for remote readers and
consumed PNG. Method and aggregation are those of the image relay audit.

All 147 cases completed on both builds. Unrelated-pane text delay stayed below 2.4 ms p95 in every
case on both. Apart from the regular-file case noted below, remote-reader, PNG and regular-file
cases changed within run-to-run variation. Local raw consuming media no longer compress and relay
pixels:

| Raw RGBA, local reader | MiB | Medium | p50 ms | p95 ms | Worst ms | Server CPU % | Drain ms |
| --- | ---: | --- | ---: | ---: | ---: | ---: | ---: |
| Master | 8 | `t` | 0.032 | 0.169 | 2.518 | 25.5 | 2002 |
| Fix | 8 | `t` | 0.028 | 0.041 | 1.350 | 2.0 | 2002 |
| Master | 8 | `s` | 0.034 | 0.209 | 2.326 | 25.0 | 2003 |
| Fix | 8 | `s` | 0.026 | 0.044 | 1.204 | 2.0 | 2001 |
| Master | 32 | `t` | 0.051 | 2.317 | 5.951 | 43.5 | 4954 |
| Fix | 32 | `t` | 0.024 | 0.038 | 2.017 | 7.5 | 2003 |
| Master | 32 | `s` | 0.041 | 0.208 | 6.376 | 43.0 | 4978 |
| Fix | 32 | `s` | 0.025 | 0.039 | 2.095 | 8.0 | 2002 |
| Master | 64 | `t` | 0.043 | 1.293 | 10.935 | 43.5 | 9923 |
| Fix | 64 | `t` | 0.026 | 0.049 | 2.659 | 14.0 | 2002 |
| Master | 64 | `s` | 0.038 | 0.208 | 10.931 | 42.0 | 9927 |
| Fix | 64 | `s` | 0.025 | 0.043 | 2.464 | 14.5 | 2002 |

The 64 MiB local relays now finish within the two-second offer instead of draining for ten.
One case got more expensive: local 64 MiB regular-file raw images rose from 6.5% to 14.0% server
CPU (32 MiB: 3.5% to 4.0%), with p95 0.041 ms. Master decoded those prepared frames into memory;
the fix copies them into file snapshots again, which at this size exceed the spare-file budget
and are allocated afresh each time. At browser frame sizes, file mode matched the pre-#241 build
above.

## Host output

A separate diagnostic run of the fix, excluded from the CPU results, recorded producer and host
wire with the maintained interposer and `--pty-tap`. The producer sent 1,033
`a=T,…,t=s` uploads at 1768×901 plus its probes. Ghostty received 1,032 `t=t` host-link uploads,
1,032 placements and 1,033 by-id deletes, 279 KB of host traffic in all: no inline pixels.
All 598 client frames in its sample were native direct presentations. Unit and protocol tests
check the server-to-client leg: local clients receive one `t=f` reference to the server's snapshot,
never the consumed source's name or a consuming reference.

Physical captures of an owned 1900×1050 Ghostty window running the fix confirmed the plain and
noise videos rendering and advancing between captures (the in-video clock moved by the elapsed
time), with 1,438 of 1,450 client frames native at a 16.7 ms p50 interval.

Launching a window from a shell that carries `SSH_CONNECTION` makes the client report that it
does not share the server's filesystem. The server then relays inline and the client composes
every frame: about 16 frames/s on the noise video, identically on the pre-#241 build. That is
remote behavior, not this regression. The harness removes `SSH_*` for exactly this reason; manual
local validation must do the same.

## Reproduction and retained evidence

`tools/graphics-video.sh DIRECTORY plain|noise` now generates maintained 1920×1080 fixtures; see
the benchmark guide. The investigation's original fixture was generated outside the repository:

```sh
ffmpeg -hide_banner -loglevel error -f lavfi -i testsrc2=size=1600x900:rate=60 \
  -t 8 -c:v libvpx-vp9 -deadline realtime -cpu-used 8 -b:v 3M -an scene.webm
```

Serve a page containing `<video autoplay muted loop playsinline src="scene.webm"></video>`
with its video filling the viewport. Run the existing harness, selecting each release executable:

```sh
python3 tools/graphics-benchmark.py --binary /absolute/path/to/rozi \
  --window-lab /absolute/path/to/window_lab.py --output /tmp/unique-run \
  --fps 60 --producer-fps 60 --seconds 20 --warmup 8 --workspace 11 \
  --width 1800 --height 1000 \
  --command '/absolute/path/to/graphics-browser.sh /path/to/terminal-browser 60 full http://127.0.0.1:18973/'
```

Prefix the command with `TERMINAL_BROWSER_FRAMES=file` for the file comparison. Replace `full`
with `auto` for normal presentation. Choose an owned visible workspace for the current machine.
Keep the default automatic transport environment clear for the other cases.

Raw reports are retained privately under `/tmp/rozi-browser-regression-video-*/result.json`,
with a combined copy at `/tmp/rozi-browser-regression-video/all-results.json`. The separate
`video-wire` run records producer headers and host output. The owned sessions, browser process
trees and Ghostty windows were stopped after each run. The loopback server was stopped afterward.
No runtime implementation was changed. Linux release builds and `git diff --check` passed;
macOS and Windows were not measured. Framework RAG tools were unavailable, so framework paths
were checked directly in the 0.29.0 source.

## 2026-10-25 immutable adoption and acquisition watermarks

The implementation removes the local client pixel copy and replaces the timed relay lease with
connection-scoped acquisition watermarks. Exported inodes remain immutable for their entire
lifetime. This section measures a different implementation from every earlier fix above.

The measured release executable is `/tmp/rozi-browser-regression-binaries/adoption-v1`, SHA-256
`75bcf6fd3eaa33383175c6730b49b3af7e918968298f13fb3f8d09f3cc30a590`.
It was copied from this worktree's own `target/release/rozi` after a successful release build;
the source and frozen hashes matched. Source bases are Rozi
`9f914739e4877777c11f3b6766d8be402f03cabd` and tui-lipan
`a9a1005c0fd78fd928cadb1e8ee8cfe21474eb13`, with uncommitted changes in both worktrees.
The sorted changed Rust-file SHA-256 map is retained in
`/tmp/rozi-adoption-v1-revision.json`; its aggregate digests are
`56f1567cc297130c40398d002766d8b73bbc9ce6b56417d2f8a66dd3088e86c9` for Rozi and
`7452b627df2150b7c54768aae78ee60749d0085acde454f8af61c3cf676697f1` for tui-lipan.
The reference remains the frozen pre-#241 executable and hash recorded above. No primary checkout
or its target directory was changed.

### Ownership and acquisition

Framework output transactions carry immutable owning file handles bound to complete rewritten
commands. Synchronized-update assembly and ordered merging rebase command ranges. Local raw
`t=f`, `t=t` and `t=s` snapshots relay under `GraphicsRelayPolicy::NamedAsFiles`; other consuming
payloads retain inline handling and local re-readable payloads without an eligible snapshot retain
their producer representation. Source consumption and query/error responses remain authoritative on
the server. Retained replay carries pixels.

Rozi protocol 39 carries explicit file bindings and separate acquisition acknowledgements. The
connection writer registers an obligation and assigns its acquisition sequence after queue
coalescing, before exposing bytes. Acknowledgements cannot exceed that connection's issued
sequence. Counters persist across resets and replay within a pane generation. A connection may
retain at most 128 acquired-file obligations and 128 MiB, with process-wide issued limits of 1,024
files and 256 MiB. Queued media also observes the connection limit; framework snapshots have their
own process-wide storage budget. Limit exhaustion disconnects the affected client for replay.

The client transport reader takes private hard links, validates files and exact commands, then
batches acquisition acknowledgements before UI dispatch. Its serial acquisition order proves the
acknowledged prefix independently of visibility, rendering and presentation acknowledgements.
Hard-link failure copies while the server obligation remains live. Acquired resources stay with
mailbox and orphan envelopes. Orphan overflow ends the transport for replay rather than keeping a
partial escape. Background attachments apply the same owned resources. Acquisition failure shuts
the transport down even if the UI does not dispatch its disconnect event.

Acknowledgements mark resources for retirement; a server turn reclaims at most one live obligation
and one disconnected queue entry. Disconnected queues keep their ownership and reservation charges
until actual release. Snapshot/import names live in private process-owned temporary directories.
Unix cleanup removes only current-user private directories whose owner process is confirmed dead.
Other platforms conservatively preserve stale directories when liveness cannot be established.
No acknowledgement or host unlink permits rewriting an exported inode.

### Browser measurements

The same maintained harness, original fixtures, viewports, font and visible workspace were used.
Each of the eight workload/transport/presentation cases has two serial samples per build, with
eight seconds of warmup and twenty seconds of sampling. No compilation, video encoding or
interposer ran during these CPU samples. Automatic transport is the file ring on the reference
and shared memory on the candidate. Full and Normal mean browser presentation, not transport.

| Workload | Transport | Presentation | Reference combined CPU % | Adoption combined CPU % | Reference fps | Adoption fps | Adoption p95 / p99 ms |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |
| Plain | Automatic | Full | 12.95 to 13.40 | 10.40 to 10.45 | 59.6 to 59.8 | 59.6 to 59.8 | 17.3 to 17.4 / 17.8 to 17.9 |
| Plain | Automatic | Normal | 13.15 to 13.45 | 10.45 to 10.50 | 59.7 to 59.8 | 59.7 to 59.8 | 17.3 / 17.7 to 17.8 |
| Plain | File | Full | 13.30 to 13.40 | 10.15 to 10.40 | 59.8 | 59.7 to 59.8 | 17.3 / 17.7 |
| Plain | File | Normal | 13.40 | 10.05 to 10.25 | 59.6 to 59.8 | 59.6 to 59.8 | 17.2 to 17.3 / 17.7 |
| Noise | Automatic | Full | 14.80 to 15.05 | 11.55 | 59.6 | 59.5 | 17.4 / 18.0 |
| Noise | Automatic | Normal | 14.95 to 15.15 | 11.75 to 11.80 | 59.5 to 59.6 | 59.5 to 59.6 | 17.3 to 17.5 / 17.8 to 18.0 |
| Noise | File | Full | 14.80 to 15.15 | 11.10 to 11.40 | 59.5 to 59.6 | 59.6 | 17.3 / 17.7 |
| Noise | File | Normal | 14.85 to 15.15 | 11.20 to 11.60 | 59.5 to 59.6 | 59.6 | 17.3 to 17.4 / 17.7 to 17.8 |

The p50 interval is 16.7 ms throughout. Interval percentiles are the maintained harness's final
rolling application histogram; successful host frame writes are not monitor scanout or an
end-to-end latency measurement. The sample labels `iv-` encode build `a`/`b`, plain/noise `p`/`n`,
automatic/file `a`/`f`, full/normal `f`/`a`, and repetition `1`/`2`. The 32 original `result.json`
files live under `/tmp/rozi-browser-regression-video-iv-*/`; the combined copy is
`/tmp/rozi-adoption-results.json`.

Reproduction uses the scratchpad's `bench.py` with short labels, for example:

```sh
python3 SCRATCHPAD/bench.py iv-apaf1 adoption-v1 auto full 1800 1000 http://127.0.0.1:18973/
python3 SCRATCHPAD/bench.py iv-anff1 adoption-v1 file full 1900 1050 http://127.0.0.1:18974/
```

`SCRATCHPAD` is the investigation directory named in the implementation prompt. The serial driver
and per-process probe are retained in `/tmp/run-rozi-adoption-matrix.py`,
`/tmp/run-rozi-adoption-bench.py` and `/tmp/rozi-adoption-probe.py`. Long descriptive labels first
exceeded Unix socket path limits and prevented producer startup. Those failed launches produced no
measurements; the short labels above avoid that problem.

### Memory qualification

Snapshot storage is counted by unique device/inode pairs through the measured processes' open
file descriptors, including private import links. Counting each name independently would count
shared pixels more than once. Producer ring files are excluded. `/proc` diagnostics sampled a
separate ten-second window inside each harness sample; client copied-byte estimates use its
`rchar` delta divided by the offered 60 fps. Probe records remain in
`/tmp/rozi-adoption-iv-*-probe.json`.

Strict RSS parity is not met in every forced-file case. RSS endpoints are:

| Workload | Presentation | Reference client MiB | Adoption client MiB | Reference server MiB | Adoption server MiB |
| --- | --- | ---: | ---: | ---: | ---: |
| Plain | Full | 29.4 to 29.6 | 29.8 | 16.1 to 16.4 | 17.1 |
| Plain | Normal | 29.5 to 29.8 | 29.4 to 29.6 | 16.6 to 17.0 | 17.0 to 17.1 |
| Noise | Full | 29.9 to 30.1 | 30.1 to 30.3 | 16.2 to 16.9 | 17.1 |
| Noise | Normal | 29.8 to 29.9 | 30.0 to 30.3 | 16.9 to 17.0 | 17.0 to 17.4 |

In forced-file runs, sampled unique snapshot storage fell from 18.2 to 18.9 MiB to 6.1 to
12.2 MiB on plain video, and from 20.4 to 26.4 MiB to 6.8 to 11.5 MiB on noise. Client `rchar`
per offered frame fell from approximately 6.0 MiB and 6.8 MiB respectively to zero. Hard-link
identity tests independently verify adoption rather than a hidden copy.

Forced-file anonymous PSS increased by approximately 0.03 MiB on the client and 0.07 to 0.09 MiB
on the server. The physical snapshot and copied-byte results are reported separately from RSS;
lower shared pixel storage does not establish lower RSS. The CPU and frame-rate criteria are
satisfied by these runs, but the requested exact RSS acceptance remains a qualification.

### Additional validation

Two local clients used the maintained whole-frame fixture at 640 by 320 pixels. A single pixel
inode was simultaneously held by the server and both clients; their open snapshot names occupied
0.78125 MiB of unique storage. The leading client wrote about 60 frames/s, with a 16.8 ms p95.

A second run stopped the read-only follower two seconds into the sample and resumed it at eight
seconds. The server's sampled attached-client count fell from two to one, then returned to two
after resume and replay. The healthy leader continued at about 60 writes/s with a 16.7 ms p95.
One-second storage samples peaked at 86.71875 MiB and returned to 0.78125 MiB. This sampled peak is
not the exact limit; unit tests exercise the 128-file boundary directly. Original reports and
progress samples are `/tmp/iv-two/result.json`, `/tmp/iv-stall/result.json`,
`/tmp/iv-two-progress.json`, `/tmp/iv-stall-progress.json` and their
`/tmp/rozi-adoption-two-probe.json` / `stall-probe.json` companions.

The image relay fairness probe completed all 147 cases on each build, three repetitions per case,
serially. The master executable retained SHA-256
`35e30c0752571502c8603652fc4719da728d6293df39b2d30a19a8cdf7077c21`; the adoption probe was
`efd637f8509d24889b8732b635078e30686ce4fb746845d48b714b323ee84994`. Text-only median p95 was
0.040 ms on master and 0.039 ms on adoption. Local raw consuming-media case-median p95 stayed at
or below 0.051 ms, including 64 MiB frames, versus up to 2.317 ms on master. At 64 MiB, local
`t=t` / `t=s` server CPU fell from 43.0 / 42.5% to 13.5 / 14.0%, and drain time from about ten
seconds to two. Local `t=f` server CPU rose from 6.5% to 13.5% as expected when snapshots become
exported; this probe excludes the client-copy saving measured by the browser runs.

Large inline/PNG cases retain variable tails. The largest case-median p95 was 2.335 ms on master
and 2.387 ms on adoption; the largest individual-run p95 was 4.498 ms and 3.458 ms respectively.
Some individual case medians rose while others fell, so these results do not establish exact
non-regression for every inline case. The local named-image path improved without an unrelated-pane
latency regression. Full logs and the aggregation are `/tmp/rozi-adoption-fairness-master.log`,
`/tmp/rozi-adoption-fairness-adoption.log` and `/tmp/rozi-adoption-fairness-v1-summary.json`.

Every candidate and reference video sample had zero additional composed frames and zero failed
frames during its sample, according to the runtime counters. A separate final-candidate wire tap
and physical capture are recorded below.

### Checks and release status

Framework macro-aware formatting, `cargo fmt --all -- --check`,
`cargo test --workspace --all-features`, strict all-target/all-feature Clippy,
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps`, `cargo build`, and
`git diff --check` passed. Rozi formatting, `cargo test`, strict all-target Clippy, `cargo build`,
Windows cross-target strict Clippy, release build, fairness probe build and `git diff --check`
passed. Windows Clippy emitted 21 existing framework dependency warnings outside the changed
paths. Logs are retained as `/tmp/rozi-adoption-framework-final-*.log` and
`/tmp/rozi-adoption-verified-*.log`.

The hard-link negative control forced the copying fallback and failed the inode equality
assertion. Restoring adoption passed. The disconnect negative control kept the UI transport owner
alive without dispatch and timed out waiting for EOF; explicitly shutting down the failed reader's
connection made it pass. Logs are `/tmp/rozi-immutable-adoption-negative-control.log` and
`/tmp/rozi-adoption-disconnect-negative-control.log`.

A manual complexity review split parser/import validation without changing the contracts. Counts
include explicit branches, loops, match arms and boolean decision operators:

| Function | Before extraction | After extraction |
| --- | ---: | ---: |
| `decode_output_frame` | 23 | 4 |
| `prepare_immutable_media_input` | 12 | 5 |

Apple targets are not installed. The Unix cleanup and Apple shared-memory branches were reviewed
in source; no Apple compile or macOS/Windows interactive result is claimed. Linux hard-link
adoption is the measured performance target. Copy fallback is tested, without a performance claim.
The framework RAG tools were unavailable, so the actual checkout source was used.

The framework API change requires a 0.30.0 release. No commit, push, pull request, tag or publish
was performed. Local iteration used the pre-existing ignored path override; a released framework
and registry-source Rozi dependency remain the release integration step.

### Final build: adoption-v2

A final framework regression test found that immutable-input preparation consumed the parsed
command's wire bytes during validation. Applying that prepared input worked, but rebuilding its
relay policy could then suppress the acquired image. Validation now borrows those bytes. The test
removes the source name before rebuilding inline relay, so it cannot pass by reopening the source.
The negative-control failure is retained in
`/tmp/rozi-adoption-relay-rebuild-negative-control.log`.

This correction was followed by the complete framework and Rozi check sets listed above, including
Windows cross-target Clippy, strict framework rustdoc, release and fairness builds. All passed;
logs are `/tmp/rozi-adoption-v2-check-0.log` through `-14.log`. The existing Windows dependency
warnings and Apple/interactive-platform limitations still apply.

The final frozen executable is `/tmp/rozi-browser-regression-binaries/adoption-v2`, SHA-256
`d17c21d6395559f90647483c76c32dca07495c64c9f3a51f98bfa5cab991b621`.
Its fairness executable has SHA-256
`b9436cd6d35eb0eb2c440f88c60acd71c5e70829eff39992a706148b53f051de`.
Source heads remain Rozi `9f914739e4877777c11f3b6766d8be402f03cabd` and framework
`a9a1005c0fd78fd928cadb1e8ee8cfe21474eb13`. The changed Rust-file digest remains
`56f1567cc297130c40398d002766d8b73bbc9ce6b56417d2f8a66dd3088e86c9` for Rozi and is now
`de5737898033bccb4bcd069f45a357c7645cf645a5031a21d023d0ffe990ea94` for the framework.
The per-file maps are `/tmp/rozi-adoption-v2-revision.json`. The earlier `adoption-v1` results
above retain their original identity.

All sixteen browser cases were repeated on this final build, serially, with the same fixtures and
harness settings. The comparison below uses the sixteen original reference cases. Two additional
fresh reference controls for plain/automatic/full measured 11.30 and 12.70% combined CPU, versus
9.90 and 10.20% on the final candidate; the CPU conclusion also holds against those controls.

| Workload | Transport | Presentation | Reference combined CPU % | Final combined CPU % | Reference fps | Final fps | Final p95 / p99 ms |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |
| Plain | Automatic | Full | 12.95–13.40 | 9.90–10.20 | 59.6–59.8 | 59.7–59.8 | 17.3–17.4 / 17.8 |
| Plain | Automatic | Normal | 13.15–13.45 | 10.15–10.40 | 59.7–59.8 | 59.7–59.8 | 17.3–17.4 / 17.9 |
| Plain | File | Full | 13.30–13.40 | 9.90–10.20 | 59.8 | 59.8 | 17.3 / 17.6–17.7 |
| Plain | File | Normal | 13.40 | 9.90 | 59.6–59.8 | 59.8 | 17.3 / 17.7 |
| Noise | Automatic | Full | 14.80–15.05 | 11.60 | 59.6 | 59.5 | 17.3–17.5 / 17.9 |
| Noise | Automatic | Normal | 14.95–15.15 | 11.75 | 59.5–59.6 | 59.5–59.6 | 17.4 / 18.0 |
| Noise | File | Full | 14.80–15.15 | 11.10–11.25 | 59.5–59.6 | 59.6 | 17.3 / 17.7–17.8 |
| Noise | File | Normal | 14.85–15.15 | 11.55–11.60 | 59.5–59.6 | 59.6 | 17.4 / 17.8–17.9 |

The final samples add zero composed frames and zero failed frames. Final p50 remains 16.7 ms.
Short sample labels use build `c`; raw reports and probes retain the same paths and encoding
specified above. `/tmp/run-rozi-adoption-v2-matrix.py` is the serial driver and
`/tmp/rozi-adoption-final-results.json` combines the sixteen reference and sixteen final reports.
The fresh reference controls have labels `iv-bpafR1` and `iv-bpafR2`.

Forced-file unique snapshot storage was 6.078–11.223 MiB for plain and 6.801–13.602 MiB for noise,
versus 18.234–18.859 and 20.402–26.363 MiB respectively on the reference. Client `rchar` per offered
frame fell from about 6.0 / 6.75 MiB to zero. These are sampled open-inode measurements, excluding
producer files and files held only by the host terminal; they do not establish a bound on all
system tmpfs allocations.

Strict forced-file RSS parity still fails in some cases:

| Workload | Presentation | Reference client MiB | Final client MiB | Reference server MiB | Final server MiB |
| --- | --- | ---: | ---: | ---: | ---: |
| Plain | Full | 29.4–29.6 | 29.4–29.8 | 16.1–16.4 | 16.7–17.3 |
| Plain | Normal | 29.5–29.8 | 29.7–29.9 | 16.6–17.0 | 16.9–17.1 |
| Noise | Full | 29.9–30.1 | 29.8–30.1 | 16.2–16.9 | 16.9–17.0 |
| Noise | Normal | 29.8–29.9 | 30.1–30.2 | 16.9–17.0 | 17.0–17.1 |

The final two-local-client run again shared one 0.78125 MiB inode between the server and both
clients, with about 60 writes/s and a 16.8 ms p95. In the stopped-client run, sampled attached
clients fell from two to one at about five seconds and returned to two at about 8.8 seconds after
resume. The healthy client maintained about 60 writes/s and a 16.7 ms p95. Sampled physical storage
peaked at 86.71875 MiB and returned to 0.78125 MiB. Reports, progress samples and probes use
`/tmp/iv-two2/`, `/tmp/iv-stall2/`, `/tmp/iv-two2-progress.json`,
`/tmp/iv-stall2-progress.json` and `/tmp/rozi-adoption-two2-probe.json` / `stall2-probe.json`.

A separate final-build twelve-second noise-video wire-tap sample recorded 712 producer `t=s`
uploads and 712 native host `t=t` uploads, with zero composed placements. Runtime composed/failed
frame deltas were also zero. Both physical window captures were inspected: the fixture time moved
from 00:00:03.100 to 00:00:05.263 and the moving shapes changed position. Private artifacts remain
in `/tmp/iv-wire2/`, including `frame-a.png`, `frame-b.png`, host/PTY wire records and `result.json`;
`/tmp/iv-wire2-summary.json` records counts. Captures contain an unrelated desktop notification and
are not repository artifacts. No wire-tap sample is used for a CPU conclusion.

The final fairness executable completed all 147 samples, three repetitions of each of 49 cases,
in 613 seconds without failures. The command was:

```sh
ROZI_IMAGE_PROBE_REPEATS=3 /tmp/rozi-browser-regression-binaries/fairness-adoption-v2 --image-relay-probe
```

The serial reference run and its frozen executable remain those recorded above. Text-only median
p95 was 0.040 ms on the reference and 0.041 ms on the final build. Local raw-media case-median p95
was at most 0.048 ms, versus up to 2.317 ms on the reference. At 64 MiB, local consuming `t=t` /
`t=s` server CPU fell from 43.0 / 42.5% to 13.5 / 14.0%, and drain time fell from about ten seconds
to two. Local `t=f` server CPU rose from 6.5% to 13.0%; the browser matrix includes the client-copy
saving that this server-only probe excludes.

Across all cases, the largest case-median p95 fell from 2.335 to 1.244 ms and the largest
individual-run p95 from 4.498 to 2.689 ms. Some inline/PNG case medians still rose: remote 8 MiB
PNG shared memory measured 0.176 to 1.243 ms; local consuming 64 MiB PNG temporary files measured
0.221 to 1.223 ms; remote 64 MiB raw files measured 0.229 to 0.833 ms. These tails varied between
the first and final candidate runs. This supports the local named-media improvement and improved
overall worst case, but does not establish strict non-regression for every inline case. The log
is `/tmp/rozi-adoption-fairness-v2.log`; paired per-case medians and CPU/drain figures are retained
in `/tmp/rozi-adoption-fairness-v2-summary.json`.

Final source hashes were checked against the frozen executable's revision metadata. Both primary
checkouts remain clean. Rozi's original registry-source `Cargo.lock` was restored after local
validation; the pre-existing ignored path override remains for iteration. Owned lab windows,
isolated sessions and the temporary noise fixture HTTP server were stopped. The pre-existing
plain fixture server and user's live session were left running.

The implementation is ready for review with the RSS and fairness qualifications stated here.
This is not an assertion that every literal acceptance criterion passed. The framework 0.30.0
release and subsequent registry dependency integration remain gated by the implementation prompt's
explicit approval requirement. No release, commit, push or PR was performed.

### Review follow-up and pull requests

Review accepted the measured RSS increase and variable inline/PNG tails as recorded qualifications.
Both suites were independently rerun successfully, and the ownership, acknowledgement, import and
remote-attach paths were reviewed without blocking findings.

The follow-up computes the acquisition sequence offset from `PANE_FRAME_OVERHEAD` and the shared
semantic metadata length. Metadata length and decoding use the same constants; the encoder checks
its semantic header length. The wire layout is unchanged. The 57 protocol tests pass. The browser
measurements above remain results of the frozen `adoption-v2` executable and were not relabelled
as measurements of this follow-up.

The framework change is committed as `c2ba4d204dfa5ae902a4d30a3c94ab215f1056d4` in
[tui-lipan #371](https://github.com/tui-lipan/tui-lipan/pull/371). Its existing macOS media CI job now
also runs immutable-file import/lifetime and prepared-media tests. These test filters and the full
framework suite and strict Clippy passed locally; macOS execution is checked through that PR rather than claimed from Linux.

For independent Rozi PR builds, the manifest and lockfile pin that pushed framework commit. The
local path override is disabled, and the temporary git source is explicitly allowed by dependency
policy. This supersedes the earlier registry-lockfile handoff state. After framework 0.30.0 is
published, replace the git dependency with the registry requirement, regenerate registry lockfile
entries, and remove the temporary source allowlist before publishing Rozi. No release is made by
this follow-up.

With the ignored path override disabled, the PR dependency passed `cargo fmt --all -- --check`,
`cargo test --locked`, strict all-target Clippy, `cargo build --locked`, Windows cross-target
strict Clippy, `cargo deny check licenses sources advisories bans`, `cargo audit`, and
`git diff --check`. Logs are `/tmp/rozi-review-root-check-0.log` through `-7.log`.
A generated framework documentation source-line reference was refreshed in follow-up commit
`0b641fe`; the Rozi dependency remains on the identical reviewed implementation at `c2ba4d2`.
