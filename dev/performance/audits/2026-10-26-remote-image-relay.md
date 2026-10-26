# Remote image relay for streamed frames

terminal-browser in a pane of a session on another machine was slow and lagged behind when the
client was attached over SSH. This audit finds the causes, and measures the framework and Rozi
changes that fix them against Rozi 0.0.33.

## Setup

- Client: this machine, AMD Ryzen 7 9700X, Linux 7.2.8-9-omarchy, Ghostty 1.3.1+omarchy.3.
- Server: laptop `h0p3-z`, Intel Core Ultra 5 338H (12 threads), Linux 7.2.8-9-omarchy,
  terminal-browser 0.13.4, on 5 GHz Wi-Fi with an 80 MHz channel.
- Link: ping RTT 1.7–3.4 ms, mean 2.4 ms. `ssh h0p3-z 'head -c 300000000 /dev/urandom' | wc -c`
  moved 300 MB in 2.95 s twice, about 101 MB/s including SSH encryption and the random source.
- Attachment: `rozi --remote <alias> sessions new <name>` from a lab Ghostty window of
  1800x1000 logical pixels on an otherwise empty, hidden workspace. The remote side ran through a
  wrapper that isolated all five XDG directories and `ROZI_CONFIG` under `/tmp/rzlab-delta`. The
  producer viewport was 1768x901 pixels.
- Binaries, all release builds of the same Rozi source tree apart from the stated changes:

  | Label | Rozi | Framework | SHA-256 |
  | --- | --- | --- | --- |
  | Baseline | 0.0.33 release | 0.30.0 | `9b2bb2ebea972211ead40724879c79d12ba61188597e09b232b6c78c84024d11` |
  | Elision | `088b37bc`, local override | `8748dcc` + elision and classification | `f8a29700b4afd03e15efa7623e24c71c61b9138c9381bfd5a79da2721d8d3f55` |
  | Parallel | as above | + banded deflate and bulk scanning | `00357e85f4e230b2b1211c9641eb2263513f60b7779d47a94ff59d3a7543630f` |
  | Final | this branch, local override | `4dc62a6` | `be0a262fbdda141a7682196a917fb6d854c0c9a3bb68cc6574c0893dd8fe0e98` |

  Client and server always ran the same build. Profiles used `release-debug` builds of the
  Elision source.

## Method

`tools/graphics-browser.sh BROWSER 60 auto URL` started terminal-browser in pane 1 with
`render.fps 60`, the default `auto` presenter, telemetry and update checks off, and GPU rendering
disabled. Three workloads ran:

- **Video**: the `tools/graphics-video.sh DIR plain` page, a 1600x900, 60 fps VP9 test video served
  over loopback on the laptop.
- **Generated**: `tools/graphics-page.html`, six small rectangles moving over a static background.
- **Scroll**: `https://en.wikipedia.org/wiki/Terminal_emulator`, with `Down` sent to the pane every
  100 ms through `rozi --session NAME send-keys` for the whole sample.

Each run warmed up for 15 s and sampled 20 s. The producer ran under a PTY proxy that forwarded
all bytes and window sizes and logged each Kitty command's control keys, payload length and, for
`t=s`, the shared-memory object name, with a monotonic timestamp. It never logged pixel data.
Process CPU and I/O came from `/proc/PID/stat` ticks and `/proc/PID/io` deltas over the sample,
with PID identity checked at both ends. Graphics counters came from `rozi --session NAME metrics`
before and after. Frame hand-over delay polled `/dev/shm` every 2 ms on the laptop and matched
each object's disappearance to the logged hand-over of that name. Runs were serial, one session
active at a time. Each figure is a single run unless stated.

## Findings

### Producer traffic

Under every workload, terminal-browser's `auto` presenter sent full 1768x901 RGBA frames by shared
memory, about 6.4 MB each:

```text
a=T,f=32,s=1768,v=901,t=s,i=1,p=1,C=1,z=0,q=2
```

It emitted 55–60 frames/s on the generated page and on video, and 38 frames/s while scrolling.
The generated page changes a few small rectangles per frame, yet still arrives whole.

### Causes on 0.0.33

1. **No pacing or coalescing for consuming media.** The framework classified only `t=d` and `t=f`
   replacements as independent, so every shared-memory frame was ordered output
   (`replaceable_transactions` stayed 0). Rozi's presentation pacer and outbox coalescing never
   engaged for terminal-browser.
2. **Every queued frame was acquired and encoded.** On video the server presented about 25 frames/s
   while the browser emitted about 59/s. The server ingress queue grew from 2,922 to 9,126 bytes,
   about 30 to 90 queued commands, over 30 s, without settling. The browser recycles objects that
   are not consumed in time, so many acquisitions failed after waiting in the queue.
3. **Inline deflate dominated the server.** A `perf record -e cpu-clock:u` of the Elision server
   during video put 63.2% of samples in `miniz_oxide::deflate::core::compress_inner` and 24.2% in
   `flush_block`. `memcpy` accounted for about 6% and base64 for under 2%.
4. **The client scanned base64 one byte at a time.** A client profile on the same build put 34.6% in
   `miniz_oxide::inflate::core::decompress_fast`, plus about 12% in other inflate functions, and 30.4%
   in `GraphicsScanner::scan_prefix`, `TerminalScreen::process_bytes` and
   `feed_grid_with_prompt_boundaries`, walking each command body byte by byte.

### Changes measured

- Elision: a burst of quiet replacements of one image is prepared as one work item. Only the
  newest readable frame is acquired, and older sources are claimed unread. Relayed consuming
  replacements are classified independent, so pacing and coalescing apply.
- Parallel: frames over 1 MiB are deflated in up to eight bands, joined into one zlib stream.
  Graphics command bodies are scanned in bulk.
- Final: a same-size replacement that readers already hold is relayed as root-frame edits of the
  changed rectangles, unless more than 70% of the frame changed. These transactions are
  `EditsPrevious` in the framework, sent as dependent output, and paced as whole frames.

### Results

Video, server presenting to one remote client:

| Build | Presented/s | Producer/s | Ingress at end | Wire MB/s | Server CPU | Client CPU | Mean ACK latency |
| --- | ---: | ---: | --- | ---: | ---: | ---: | ---: |
| Baseline | 27.7 | 59.6 | 572 B | 61.5 | 75.2% | 59.3% | 21.1 ms |
| Elision | 24.7 | 59.0 | 94–384 B, bounded | 51.8 | 81.1% | 49.3% | — |
| Parallel | 24.7 | 55.5 | 94 B | 56.6 | 181.4% | 48.5% | 20.5 ms |
| Final | 27.6 | 59.6 | 8 B | 60.9 | 110.4% | 50.7% | 20.2 ms |

Baseline queue growth during an earlier, separate video run, sampled at 10 s intervals: 2,922,
4,426, 6,400 and 9,126 bytes. With elision the same probe read 196, 384, 94 and 196 bytes. CPU is
user plus system time as a percentage of one core.

Video frame hand-over to consumption or reclamation, 20 s:

| Build | Frames | p50 | p95 | Max |
| --- | ---: | ---: | ---: | ---: |
| Baseline | 1,189 | 4.3 ms | 88.0 ms | 118.4 ms |
| Final | 1,191 | 2.5 ms | 13.4 ms | 32.2 ms |

Generated page:

| Build | Presented/s | Wire MB/s | Server CPU | Client CPU | Mean ACK latency |
| --- | ---: | ---: | ---: | ---: | ---: |
| Baseline | 60 | 4.74 | 33.9% | 12.1% | — |
| Parallel | 59.3 | 4.43 | 46.3% | 11.2% | 6.8 ms |
| Final | 59.6 | 1.19 | 29.2% | 13.2% | 5.9 ms |

Scroll:

| Build | Presented/s | Producer/s | Wire MB/s | Server CPU | Client CPU | Mean ACK latency |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Baseline | 38.0 | 38.0 | 16.81 | 31.5% | 18.2% | 13.4 ms |
| Final | 38.0 | 38.0 | 12.94 | 35.4% | 19.5% | 12.7 ms |

On the Final build, 34.9 of the 38.0 scroll frames per second travelled as edits, and 1.3 of the
27.6 video frames per second did.

Isolated deflate timing on this machine, a 1768x901 RGBA buffer of gradients with noise,
`CARGO_PROFILE_TEST_OPT_LEVEL=3`: serial `Compression::fast()` 35.1–36.3 ms for 3,460,337 bytes,
eight bands 7.3–7.9 ms for 3,460,517 bytes. Inflating the banded stream took 21.4–21.5 ms.

## Interpretation

The lag came from queueing, not from the link alone. On 0.0.33, frames that terminal-browser
handed over waited behind earlier ones, and the server spent its CPU on frames that were already
stale. With elision and pacing, the queue stays at one command and the 95th-percentile frame age
falls from 88 ms to 13 ms.

Full-motion video over this Wi-Fi link stays near 27 frames/s. Nearly the whole viewport changes
every frame, so each frame travels whole, at about 2.2 MB of base64-encoded deflate. Parallel
deflate removes the server CPU limit, but not the bytes. Raising video throughput further needs
fewer bytes per frame. Removing base64 from Rozi's own transport would save about a quarter.

Edits help whenever part of the viewport is still. The generated page needs 75% less wire
traffic, and its server CPU falls below the baseline even with parallel deflate. Scrolling a real
page changes most of the viewport, but static parts still save 23% of the bytes.

The Parallel row shows the cost of banding when frames compress easily: server CPU rose from 33.9%
to 46.3% on the generated page. Edits more than recover it there. The Final server reads the
previous frame back from its snapshot file to compare it, about 1.1 GB/s of tmpfs reads at 60
frames/s. Caching that frame is a possible follow-up.

## Verification

- `tui-lipan` `4dc62a6`: `cargo test --workspace --all-features` passed, including 3,870 library
  tests. `cargo clippy --workspace --all-targets --all-features -- -D warnings`, rustdoc with
  `-D warnings`, `cargo fmt --check`, macro formatting, `cargo deny` and the feature-table check
  passed.
- Framework tests cover burst elision, fallback to the newest readable frame, events that end a
  burst, classification of relayed consuming media, banded zlib round trips and checksums,
  equivalence of bulk and byte-wise scanning, the exact body limit, root-frame edits and refusals,
  and an end-to-end relay. That relay checks the reader's decoded pixels and placements against
  the server and against a screen fed every frame whole. Negative controls failed as expected:
  disabling elision, an off-by-one in the edit rectangle width, and an off-by-one in the body
  limit.
- Rozi pacer tests cover pacing a whole-frame stream relayed as edits, and dropping pacing for
  other dependent output.
- Review follow-up `bf3c7e9` narrows when each optimization applies, so the image budget evicts
  exactly what applying every command would. Bursts now require frames of one size separated only
  by text. Edits require the replacement to be the first graphics command of its batch. Frame-data
  continuation chunks also repeat `a=f`. These rows were not remeasured. Every measured workload
  already met the narrower conditions: after two start-up probes, the producer logs held only
  consecutive 1768x901 replacements of image 1.

## Limitations

- The lab window sat on a hidden workspace. Ghostty's presentation there may be throttled, and no
  host screenshot verified the rendered pixels. Framework tests verify the reader's decoded
  pixels, not what Ghostty drew.
- Each configuration was measured once over Wi-Fi, so link variation between runs is not bounded.
  The Elision row came from a different sampling window than the others.
- The ACK latency is the server's output-to-acknowledgement time and excludes queueing before the
  server reads. The hand-over delay covers that queueing, but stops at consumption, not display.
- macOS and Windows were not run. No YouTube or other network video was measured.
