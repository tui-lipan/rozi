# Named-image relay fairness

## Baseline

Measured on 2026-10-24 with Rozi `2518aa26` plus the stage-2 benchmark extension, registry
tui-lipan 0.28.5, Rust 1.96.1, Linux 7.2.8-9-omarchy, and an AMD Ryzen 7 9700X. The benchmark
source SHA-256 was `27a4da2cfd9bc4c1de5c75f1c63491fffac5154f696edd435dff237a6ede6b0e`.
The shipping bench profile uses optimization level 3, thin LTO and one codegen unit.
Each pane is 250 columns by 60 rows with 5,000 scrollback lines. There is no graphical host.
The ordinary desktop remained running; no compiler was running when the matrix started.

Build once with `cargo bench --bench server_fairness --no-run`, then run:

```sh
ROZI_IMAGE_PROBE_CPU_SAMPLER=/path/to/kitty-rendering-validation/scripts/sample_processes.py \
  target/release/deps/server_fairness-c3bde5a23ebcbf9b --image-relay-probe
```

Three repetitions, two seconds of text emissions per case, 5 ms between text lines, and image
submissions at 10 Hz. Latency runs from immediately before the text helper's stdout write to
receipt through the session IPC, with up to 250 microseconds of driver polling delay. Pending
images drain after the interval. It excludes host rendering and input latency. PNG and raw data
contain deterministic opaque noise. The MiB column is decoded RGBA size; PNG file sizes for
1/8/32/64 MiB were 900,445 / 7,203,203 / 28,813,213 / 57,626,047 bytes. Raw files reach 67,108,864
bytes, the transmit limit. Generated fixture preparation is outside timing.

CPU is sampled from identity-checked `/proc` tick deltas at 0.5-second intervals for two seconds,
normalized to one core. Sampler startup slightly follows the latency interval. The table reports
medians of each run's p50, p95 and CPU mean, and the largest maximum across runs. It is not a pooled
latency distribution. Raw 64 MiB relayed p95 varied from 4700.1 to 5331.9 ms.
No CI threshold is justified by this short local sample.

| Format | MiB | Relay | p50 ms | p95 ms | Worst ms | Server CPU % |
| --- | ---: | --- | ---: | ---: | ---: | ---: |
| Text only | 0 | false | 0.146 | 0.263 | 3.166 | 0.0 |
| PNG | 1 | false | 0.147 | 0.265 | 1.682 | 2.0 |
| PNG | 1 | true | 0.153 | 0.534 | 6.383 | 3.0 |
| PNG | 8 | false | 0.169 | 9.898 | 15.022 | 14.5 |
| PNG | 8 | true | 8.741 | 40.601 | 52.306 | 28.5 |
| PNG | 32 | false | 9.154 | 54.199 | 61.199 | 58.5 |
| PNG | 32 | true | 1084.609 | 1983.659 | 2103.136 | 100.0 |
| PNG | 64 | false | 100.903 | 306.793 | 395.704 | 100.0 |
| PNG | 64 | true | 2618.908 | 3513.478 | 3740.931 | 100.0 |
| Raw RGBA | 1 | false | 0.152 | 0.262 | 1.126 | 0.5 |
| Raw RGBA | 1 | true | 0.160 | 0.918 | 6.772 | 4.5 |
| Raw RGBA | 8 | false | 0.155 | 0.264 | 1.087 | 1.0 |
| Raw RGBA | 8 | true | 11.504 | 39.788 | 47.997 | 35.5 |
| Raw RGBA | 32 | false | 0.184 | 0.300 | 5.570 | 5.0 |
| Raw RGBA | 32 | true | 1313.043 | 2211.973 | 2445.514 | 100.0 |
| Raw RGBA | 64 | false | 0.163 | 6.901 | 13.684 | 11.5 |
| Raw RGBA | 64 | true | 4196.272 | 5094.558 | 5427.556 | 100.0 |

The baseline justifies deferred media work. A full raw relay monopolizes the caller through
compression, base64 and chunk construction; PNG acquisition/decoding also stalls unrelated text
without relay. Move acquisition, validation/decoding and construction of relayed output to bounded
workers, then commit results through the original retained parser and transaction classifier.

The following implementation measurements fill the consuming-media slots. The baseline above
remains the stage-2 result before relay mechanics changed.


## Deferred preparation and bounded emission

Remeasured on the same machine and profile with Rozi `62f96de5` and tui-lipan
`294220b137d330916f332f794ddc883469efba5a`. This includes the reviewed Apple-only mmap
acquisition and prepared-media identity fixes. The benchmark source SHA-256 was
`1dcc8e27ddbd7608bed7d8b103f0ffe81f01f92ffb42648ff9a90b14cf2d7d9d`.
The release Rozi binary SHA-256 was
`b29724c535b27e44c6e569621fb773b339bb17a34f07cacca93128ec52ce125a`.
No compilation or live window validation ran during this matrix. `LD_PRELOAD` was unset.

```sh
env -u LD_PRELOAD \
  ROZI_IMAGE_PROBE_CPU_SAMPLER=/path/to/kitty-rendering-validation/scripts/sample_processes.py \
  target/release/deps/server_fairness-ea92c1dad8642605 --image-relay-probe
```

All 147 runs completed, three repetitions of 49 cases. The same latency definition and
aggregation apply. `f`, `t`, and `s` mean regular file, consuming temporary file, and consuming
POSIX shared memory. A remote reader is a real draining attachment with
`shares_filesystem: false`. All attachments receive one shared pane output representation.
Consuming media is inline even without a remote reader. Each image case offers 20 images.
The shm corpus is populated before each offer; temporary-file offers hard-link the immutable
corpus and relinquish that name. Every case checks that the attachment survives and receives
image output.

The drain column is median total wall time, including the initial two-second offer interval and
completion of queued image commands. CPU still covers only the initial two-second sample,
normalized to one core. The server can exceed 100% with its worker. CPU figures do not measure
energy per completed image, nor establish better throughput. The large consuming/relayed cases
take about 9 to 10 seconds to drain. The bounded output budget favors responsiveness while the
image pane has a backlog.

| Format | MiB | Medium | Remote reader | p50 ms | p95 ms | Worst ms | Server CPU % | Drain ms |
| --- | ---: | --- | --- | ---: | ---: | ---: | ---: | ---: |
| Text only | 0 | `f` | false | 0.146 | 0.258 | 1.371 | 0.5 | 2003 |
| PNG | 1 | `f` | false | 0.147 | 0.260 | 1.140 | 2.0 | 2004 |
| PNG | 1 | `t` | false | 0.153 | 0.262 | 0.810 | 3.0 | 2003 |
| PNG | 1 | `s` | false | 0.159 | 0.271 | 0.733 | 3.0 | 2003 |
| PNG | 1 | `f` | true | 0.148 | 0.260 | 2.349 | 3.0 | 2002 |
| PNG | 1 | `t` | true | 0.157 | 0.268 | 2.165 | 3.0 | 2003 |
| PNG | 1 | `s` | true | 0.154 | 0.270 | 2.250 | 3.0 | 2003 |
| PNG | 8 | `f` | false | 0.148 | 0.261 | 1.097 | 15.0 | 2003 |
| PNG | 8 | `t` | false | 0.172 | 0.337 | 4.638 | 19.0 | 2003 |
| PNG | 8 | `s` | false | 0.170 | 0.324 | 1.346 | 18.5 | 2003 |
| PNG | 8 | `f` | true | 0.173 | 0.323 | 3.239 | 19.5 | 2004 |
| PNG | 8 | `t` | true | 0.171 | 0.332 | 3.035 | 19.5 | 2003 |
| PNG | 8 | `s` | true | 0.175 | 0.342 | 2.980 | 19.5 | 2003 |
| PNG | 32 | `f` | false | 0.148 | 0.264 | 1.329 | 61.0 | 2003 |
| PNG | 32 | `t` | false | 0.197 | 1.294 | 6.425 | 36.0 | 4451 |
| PNG | 32 | `s` | false | 0.214 | 1.364 | 4.518 | 36.0 | 4471 |
| PNG | 32 | `f` | true | 0.212 | 0.400 | 5.193 | 36.0 | 4462 |
| PNG | 32 | `t` | true | 0.214 | 0.387 | 5.638 | 36.5 | 4460 |
| PNG | 32 | `s` | true | 0.205 | 0.386 | 2.933 | 36.5 | 4473 |
| PNG | 64 | `f` | false | 0.149 | 0.266 | 2.724 | 100.5 | 2457 |
| PNG | 64 | `t` | false | 0.234 | 0.421 | 11.156 | 40.5 | 9043 |
| PNG | 64 | `s` | false | 0.229 | 0.423 | 11.837 | 41.0 | 9057 |
| PNG | 64 | `f` | true | 0.236 | 0.456 | 11.577 | 41.0 | 9050 |
| PNG | 64 | `t` | true | 0.237 | 1.478 | 10.821 | 41.0 | 9054 |
| PNG | 64 | `s` | true | 0.220 | 0.412 | 10.022 | 41.5 | 9066 |
| Raw RGBA | 1 | `f` | false | 0.146 | 0.261 | 1.181 | 0.5 | 2003 |
| Raw RGBA | 1 | `t` | false | 0.154 | 0.264 | 0.881 | 3.5 | 2003 |
| Raw RGBA | 1 | `s` | false | 0.153 | 0.265 | 0.860 | 4.0 | 2004 |
| Raw RGBA | 1 | `f` | true | 0.150 | 0.264 | 2.341 | 4.0 | 2004 |
| Raw RGBA | 1 | `t` | true | 0.155 | 0.268 | 2.185 | 4.0 | 2003 |
| Raw RGBA | 1 | `s` | true | 0.157 | 0.268 | 2.178 | 4.0 | 2003 |
| Raw RGBA | 8 | `f` | false | 0.150 | 0.261 | 1.123 | 0.5 | 2003 |
| Raw RGBA | 8 | `t` | false | 0.167 | 0.307 | 2.003 | 24.5 | 2003 |
| Raw RGBA | 8 | `s` | false | 0.174 | 0.386 | 2.080 | 25.0 | 2003 |
| Raw RGBA | 8 | `f` | true | 0.169 | 0.331 | 3.408 | 25.0 | 2003 |
| Raw RGBA | 8 | `t` | true | 0.185 | 0.415 | 4.106 | 25.0 | 2003 |
| Raw RGBA | 8 | `s` | true | 0.169 | 0.305 | 4.123 | 25.0 | 2003 |
| Raw RGBA | 32 | `f` | false | 0.150 | 0.259 | 1.333 | 3.0 | 2003 |
| Raw RGBA | 32 | `t` | false | 0.199 | 1.369 | 5.944 | 43.5 | 4958 |
| Raw RGBA | 32 | `s` | false | 0.219 | 0.417 | 6.562 | 43.5 | 4979 |
| Raw RGBA | 32 | `f` | true | 0.204 | 0.405 | 5.577 | 44.0 | 4954 |
| Raw RGBA | 32 | `t` | true | 0.200 | 0.399 | 4.994 | 44.0 | 4971 |
| Raw RGBA | 32 | `s` | true | 0.197 | 0.396 | 6.832 | 43.5 | 4980 |
| Raw RGBA | 64 | `f` | false | 0.146 | 0.260 | 1.028 | 6.5 | 2003 |
| Raw RGBA | 64 | `t` | false | 0.227 | 1.358 | 10.594 | 43.5 | 9886 |
| Raw RGBA | 64 | `s` | false | 0.194 | 0.403 | 12.982 | 43.5 | 9887 |
| Raw RGBA | 64 | `f` | true | 0.223 | 0.415 | 10.303 | 44.0 | 9883 |
| Raw RGBA | 64 | `t` | true | 0.206 | 0.396 | 12.115 | 44.0 | 9897 |
| Raw RGBA | 64 | `s` | true | 0.204 | 0.412 | 12.819 | 44.0 | 9898 |

At 64 MiB, regular-file PNG relay p95 fell from 3513.478 to 0.456 ms; raw RGBA relay fell from
5094.558 to 0.415 ms. Their worst delays fell from 3740.931 to 11.577 ms and from 5427.556 to
10.303 ms. The largest individual delay anywhere in this final matrix was 12.982 ms. Local
64 MiB PNG without relay fell from 306.793 to 0.266 ms p95. These short local repetitions
justify the implementation, but do not justify a CI timing threshold.

The framework prepares one named-media operation on a worker. The server retains ingress
accounting until commit, preserves parser transaction classification and per-pane order, and
allows other panes through while preparation is pending. Results reuse acquired pixels when
attachment policy changes. Resize and related control barriers wait for the affected media.
Prepared output also needs bounded emission. An intermediate worker-only prototype still
suffered multi-second text delays from large image output occupying client queues. The final
server emits at most 256 KiB of ordered media per tick, preserving existing atomic replacement
bounds. Late attach replay waits for the current output to finish so it cannot start with an
orphaned continuation chunk.


## Actual host rendering and lifecycle

Validated the same release binary on Linux in isolated Ghostty windows on the headless
Hyprland `FALLBACK` output, 1920 by 1080 at scale 1. The host helper used its isolated
configuration and verified geometry before capture. The primary window was 1100 by 720,
resized to 980 by 650 and restored. Actual producer sizes were 36 rows by 66 columns,
528 by 612 pixels, then 32 rows by 58 columns, 464 by 544 pixels, then the original size.
Each read-only late viewer was 760 by 720. No personal session or configuration was used.

The maintained `tools/graphics-patches.py` scene was adapted only for consuming transport and
retained final output. It ran at 5 fps for 20 seconds, then stopped producing images. A separate
pane continued ordinary text every 0.2 seconds. Physical host captures showed the checkerboard
base, cyan toolbar, offset opaque patch and translucent moving patch. Both consuming transports
survived active resize and restore. Late viewers attached after the fixture's source directory
was gone and displayed the retained, correctly clipped scene. All 206 offered shared-memory
names were absent before the late attach.

Outgoing commands were counted using the skill's exact-executable `write`/`writev` interceptor
in these correctness runs only. The primary temporary-file client emitted 214 native placements
and zero composed placements; the shm client emitted 217 native placements and zero composed
placements. Each late native viewer emitted 3 native placements. Each forced-composed reference
emitted 208 composed placements and zero native placements. The native renderer currently
uploads with `a=t`, so the interceptor's `a=T` transmission count is not an upload count here.
The current native placement has no `c=`; the composed placement includes cell dimensions.
These command counts plus physical screenshots establish the selected rendering path.

For each medium, the primary image rectangle at x=14, y=74, width=528, height=612 was exactly
unchanged across two seconds of unrelated text output, zero differing pixels out of 323,136.
The late native/composed pair matched exactly in the visible image rectangle at x=6, y=74,
width=360, height=612, zero differing pixels out of 220,320. The narrower late viewers clipped
an unchanged retained scene. The comparison covers that entire visible image rectangle;
chrome and an incidental desktop notification outside it were excluded. Captures were inspected
visually as well. All owned sessions, producers and hosts were stopped after validation.
Captures, status reports and counters remain private scratch artifacts, not repository fixtures.

## Checks and release handoff

The reviewed combined framework passed `cargo build`, `cargo lint`, formatting checks,
`cargo test`, `cargo test --workspace --all-features`, strict workspace rustdoc, and
`git diff --check`. There are 359 passing terminal tests. The required Rozi-facing
`cargo check --features terminal` and `cargo clippy --features terminal` also passed;
the latter reports the existing unit-value warning in `backend/ratatui_backend/render/mod.rs`.
The framework RAG tool was unavailable, so source and export surfaces were inspected directly.

Rozi passed `cargo fmt --all -- --check`, `cargo test`,
`cargo clippy --all-targets -- -D warnings`, `cargo build`, `git diff --check`,
release application/benchmark builds, and strict all-target Windows GNU cross-target Clippy.
`cargo deny check licenses sources advisories bans` and `cargo audit` passed with the existing
allowed advisories. Server and real-protocol tests cover local/remote combinations, policy
changes during preparation, source failure, offset/size ranges, one-time consumption, retained
replay and late attachment during emitted chunks. CI on the stage-1 and stage-3 framework PRs
is green, including the macOS acquisition fixtures. No macOS runtime was available locally;
Windows was cross-checked, not run locally. NetBSD stage-4 runtime was not exercised.

Stage 1 is tui-lipan PR #369, stage 2 is Rozi PR #240, and stage 3 is tui-lipan PR #370.
Stage 4 remains the local `feat/server-consuming-media` branch in `rozi-consuming-media`,
stacked on the benchmark branch. Per the maintainer's request, none of these PRs was merged
and no framework version was released. The reviewed stage-1 platform split was kept:
descriptor acquisition outside Apple avoids a sender-truncation mmap SIGBUS hazard; Apple
uses mmap because shm descriptors cannot be read or sought there. The Apple mapping covers
only the needed page-aligned window. An opened object is unlinked once after the acquisition
attempt, including failures, so abandoned transfers do not leak.

The ignored local Cargo patch points at `../tui-lipan-image-relay`. Its lockfile rewrites are
not committed. After manually merging and releasing the framework, update Rozi's `tui-lipan`
version requirement to that published breaking release, remove the ignored override, resolve
the registry lockfile with Cargo, rerun baseline and dependency checks, then open the stage-4
PR. The branch intentionally cannot build against its old registry dependency without the
local override. The unrelated existing framework shared-frame producer's Darwin descriptor
writer limitation remains outside the acquisition change; the macOS reader fixtures use an
independent mmap writer. No CI performance threshold was added because these are short local
samples. No speculative asynchronous parser work beyond the measured named-media path was added.
