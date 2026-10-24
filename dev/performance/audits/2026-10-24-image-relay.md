# Named-image relay fairness

## Baseline

Measured on 2026-10-24 with Rozi `2518aa26` plus the stage-2 benchmark extension, registry
tui-lipan 0.28.5, Rust 1.96.1, Linux 7.2.8-9-omarchy, and an AMD Ryzen 7 9700X. The source SHA-256
was `5ab7edcc21ba27275a8414308d296dbccf9f92487f72e2751bf715f62abf84e2` for
`benches/server_fairness.rs` and
`9e9a77bec8dbd64aab480912bf2d26b6d0d4f0a370339401486c50436292412b` for
`src/platform/process/linux.rs`, which supplies the CPU sample.
The shipping bench profile uses optimization level 3, thin LTO and one codegen unit.
Each pane is 250 columns by 60 rows with 5,000 scrollback lines. There is no graphical host.
The ordinary desktop remained running; no compiler was running when the matrix started.

Build once with `cargo bench --bench server_fairness --no-run`, then run:

```sh
target/release/deps/server_fairness-c3bde5a23ebcbf9b --image-relay-probe
```

Three repetitions, two seconds of text emissions per case, 5 ms between text lines, and image
submissions at 10 Hz. Before timing, each image case submits one image, waits for its
acknowledgement and for empty PTY ingress and client outboxes, then resets its counters, so the
first timed image is not cold. Latency runs from immediately before the text helper's stdout write
to the driver's frame-drain thread taking the decoded frame from the session client. Pending images
drain after the interval. It excludes host rendering and input latency. PNG and raw data contain
deterministic opaque noise. The MiB column is decoded RGBA size; PNG file sizes for 1/8/32/64 MiB
were 900,445 / 7,203,203 / 28,813,213 / 57,626,047 bytes. Raw files reach 67,108,864 bytes, the
transmit limit. Generated fixture preparation is outside timing.

CPU is the server's user plus system time from `/proc/<pid>/stat` over the two seconds of offered
load, ending when submission stops and before the STOP request and backlog drain. It is
identity-checked by process start time and normalized to one core; at 100 ticks per second the
resolution is 0.5 percent. The table reports medians of each run's p50, p95 and CPU, and the
largest maximum across runs. It is not a pooled latency distribution. Raw 64 MiB relayed p95 varied
from 4275.8 to 4300.1 ms. No CI threshold is justified by this short local sample.

| Format | MiB | Relay | p50 ms | p95 ms | Worst ms | Server CPU % |
| --- | ---: | --- | ---: | ---: | ---: | ---: |
| Text only | 0 | false | 0.020 | 0.039 | 0.940 | 0.0 |
| PNG | 1 | false | 0.020 | 0.036 | 2.080 | 2.5 |
| PNG | 1 | true | 0.026 | 0.544 | 5.645 | 3.0 |
| PNG | 8 | false | 0.021 | 9.927 | 15.268 | 15.0 |
| PNG | 8 | true | 8.403 | 40.565 | 46.713 | 28.5 |
| PNG | 32 | false | 8.785 | 54.227 | 59.447 | 59.5 |
| PNG | 32 | true | 1096.811 | 2001.391 | 2106.606 | 98.0 |
| PNG | 64 | false | 99.849 | 307.158 | 355.893 | 99.5 |
| PNG | 64 | true | 2551.568 | 3456.459 | 3562.370 | 99.5 |
| Raw RGBA | 1 | false | 0.021 | 0.034 | 2.230 | 0.0 |
| Raw RGBA | 1 | true | 0.026 | 0.703 | 5.982 | 4.0 |
| Raw RGBA | 8 | false | 0.026 | 0.043 | 1.986 | 1.0 |
| Raw RGBA | 8 | true | 11.430 | 40.546 | 48.962 | 35.0 |
| Raw RGBA | 32 | false | 0.021 | 0.044 | 2.752 | 4.0 |
| Raw RGBA | 32 | true | 1304.701 | 2209.384 | 2325.335 | 100.0 |
| Raw RGBA | 64 | false | 0.027 | 7.279 | 13.101 | 12.5 |
| Raw RGBA | 64 | true | 3383.738 | 4288.116 | 4396.121 | 100.0 |

The baseline justifies deferred media work. A full raw relay monopolizes the caller through
compression, base64 and chunk construction; PNG acquisition/decoding also stalls unrelated text
without relay. Move acquisition, validation/decoding and construction of relayed output to bounded
workers, then commit results through the original retained parser and transaction classifier.

Consuming-media results are recorded separately below. The baseline above uses the reviewed
probe from PR #240.


## After the reviewed probe was merged

Remeasured after integration of PR #240, with Rozi `a3850a5f94156b332282b4519fdbc3aa529ea263`
and the same local framework override at tui-lipan `294220b137d330916f332f794ddc883469efba5a`.
The merge carries the reviewed probe's receipt timestamp from the drain thread, event-driven
sampling, explicit image warmup and CPU ticks over the offered-load interval into the
consuming-media cases. It changes no server media behavior. PR #241 targets `master`.

Source SHA-256 values were `12cea372145d0efced09f15936fb2b9dc235b3ac830746fd8bc9e3e13fb2397c`
for `benches/server_fairness.rs` and
`9e9a77bec8dbd64aab480912bf2d26b6d0d4f0a370339401486c50436292412b` for
`src/platform/process/linux.rs`. The optimized benchmark executable, which also runs its
session server, had SHA-256
`16561f66972a64d6b73833a18be2ae31d274d7b6c2276094f403406489ca3cdb`.

```sh
env -u LD_PRELOAD -u ROZI_IMAGE_PROBE_CPU_SAMPLER \
  target/release/deps/server_fairness-ea92c1dad8642605 --image-relay-probe
```

All 147 runs completed, three repetitions of 49 cases. The baseline's reviewed timing and
aggregation apply. The server CPU sampler is built into the probe on Linux, with no Python
sampler. No agent compilation or live host validation ran during measurement; the ordinary
desktop remained running. These are short local samples with variation, not controlled
real-time guarantees. `f`, `t` and `s` mean regular file, consuming temporary file and consuming
POSIX shared memory. Consuming media is inline with either client mix. Each image case offers
20 images after the warmup image. Drain time includes the two-second offer interval and all
pending image completion. CPU covers only that offer interval, not total work per completed
image. Producer-side source creation is excluded from server CPU.

| Format | MiB | Medium | Remote reader | p50 ms | p95 ms | Worst ms | Server CPU % | Drain ms |
| --- | ---: | --- | --- | ---: | ---: | ---: | ---: | ---: |
| Text only | 0 | `f` | false | 0.024 | 0.040 | 0.934 | 0.5 | 2007 |
| PNG | 1 | `f` | false | 0.023 | 0.043 | 1.031 | 2.5 | 2001 |
| PNG | 1 | `t` | false | 0.026 | 0.043 | 0.954 | 3.0 | 2002 |
| PNG | 1 | `s` | false | 0.027 | 0.043 | 0.650 | 3.0 | 2002 |
| PNG | 1 | `f` | true | 0.025 | 0.043 | 2.027 | 3.0 | 2002 |
| PNG | 1 | `t` | true | 0.028 | 0.047 | 2.658 | 3.0 | 2002 |
| PNG | 1 | `s` | true | 0.026 | 0.044 | 1.293 | 3.0 | 2002 |
| PNG | 8 | `f` | false | 0.022 | 0.049 | 3.384 | 16.0 | 2002 |
| PNG | 8 | `t` | false | 0.031 | 0.179 | 2.377 | 20.0 | 2001 |
| PNG | 8 | `s` | false | 0.030 | 0.169 | 1.402 | 19.5 | 2001 |
| PNG | 8 | `f` | true | 0.032 | 0.177 | 5.208 | 20.5 | 2002 |
| PNG | 8 | `t` | true | 0.032 | 0.195 | 1.370 | 20.5 | 2002 |
| PNG | 8 | `s` | true | 0.029 | 0.170 | 4.137 | 20.5 | 2003 |
| PNG | 32 | `f` | false | 0.026 | 0.044 | 37.373 | 62.5 | 2003 |
| PNG | 32 | `t` | false | 0.043 | 0.275 | 2.568 | 36.5 | 4453 |
| PNG | 32 | `s` | false | 0.042 | 0.233 | 4.446 | 36.0 | 4467 |
| PNG | 32 | `f` | true | 0.049 | 0.400 | 3.628 | 36.5 | 4463 |
| PNG | 32 | `t` | true | 0.056 | 0.228 | 2.705 | 37.5 | 4459 |
| PNG | 32 | `s` | true | 0.048 | 0.239 | 3.473 | 37.5 | 4472 |
| PNG | 64 | `f` | false | 0.021 | 0.043 | 1.275 | 100.0 | 2465 |
| PNG | 64 | `t` | false | 0.046 | 0.246 | 11.902 | 41.5 | 9051 |
| PNG | 64 | `s` | false | 0.046 | 0.223 | 22.871 | 41.5 | 9068 |
| PNG | 64 | `f` | true | 0.147 | 1.901 | 11.085 | 41.5 | 9050 |
| PNG | 64 | `t` | true | 0.053 | 0.232 | 11.192 | 42.0 | 9062 |
| PNG | 64 | `s` | true | 0.052 | 1.255 | 11.216 | 42.5 | 9084 |
| Raw RGBA | 1 | `f` | false | 0.022 | 0.042 | 2.074 | 0.5 | 2001 |
| Raw RGBA | 1 | `t` | false | 0.026 | 0.045 | 0.882 | 3.5 | 2002 |
| Raw RGBA | 1 | `s` | false | 0.022 | 0.040 | 0.795 | 3.5 | 2001 |
| Raw RGBA | 1 | `f` | true | 0.026 | 0.044 | 0.652 | 4.0 | 2002 |
| Raw RGBA | 1 | `t` | true | 0.027 | 0.043 | 0.654 | 4.0 | 2002 |
| Raw RGBA | 1 | `s` | true | 0.024 | 0.044 | 1.037 | 4.0 | 2001 |
| Raw RGBA | 8 | `f` | false | 0.024 | 0.039 | 2.027 | 1.0 | 2002 |
| Raw RGBA | 8 | `t` | false | 0.031 | 0.518 | 3.403 | 25.0 | 2003 |
| Raw RGBA | 8 | `s` | false | 0.030 | 0.165 | 1.452 | 25.0 | 2003 |
| Raw RGBA | 8 | `f` | true | 0.031 | 0.199 | 2.676 | 25.5 | 2002 |
| Raw RGBA | 8 | `t` | true | 0.030 | 0.176 | 2.694 | 25.5 | 2002 |
| Raw RGBA | 8 | `s` | true | 0.028 | 0.169 | 2.638 | 26.5 | 2002 |
| Raw RGBA | 32 | `f` | false | 0.026 | 0.040 | 1.664 | 3.5 | 2002 |
| Raw RGBA | 32 | `t` | false | 0.044 | 0.862 | 8.416 | 43.5 | 4951 |
| Raw RGBA | 32 | `s` | false | 0.053 | 0.856 | 9.135 | 43.0 | 4979 |
| Raw RGBA | 32 | `f` | true | 0.044 | 2.348 | 7.403 | 43.5 | 4950 |
| Raw RGBA | 32 | `t` | true | 0.042 | 1.674 | 7.787 | 44.0 | 4949 |
| Raw RGBA | 32 | `s` | true | 0.043 | 1.283 | 11.411 | 44.0 | 4974 |
| Raw RGBA | 64 | `f` | false | 0.025 | 0.049 | 1.565 | 6.5 | 2002 |
| Raw RGBA | 64 | `t` | false | 0.066 | 0.216 | 10.909 | 43.5 | 9881 |
| Raw RGBA | 64 | `s` | false | 0.042 | 0.234 | 15.060 | 43.5 | 9929 |
| Raw RGBA | 64 | `f` | true | 0.046 | 0.220 | 13.338 | 43.5 | 9900 |
| Raw RGBA | 64 | `t` | true | 0.148 | 0.213 | 10.579 | 44.0 | 9902 |
| Raw RGBA | 64 | `s` | true | 0.043 | 1.115 | 14.432 | 43.5 | 9910 |

Compared with the reviewed baseline above, 64 MiB regular-file PNG relay changed from
2551.568 / 3456.459 / 3562.370 ms p50 / p95 / worst to
0.147 / 1.901 / 11.085 ms. Raw RGBA relay changed from
3383.738 / 4288.116 / 4396.121 ms to 0.046 / 0.220 / 13.338 ms.
Median offered-window server CPU changed from 99.5% to 41.5% for PNG and from 100.0% to
43.5% for raw RGBA. Large relays still take about 9 to 10 seconds to drain; these CPU figures
do not establish better throughput or energy per completed image.

The three 64 MiB PNG relay p95 values were 1.901, 2.351 and 0.228 ms; raw relay values were
0.197, 0.220 and 2.365 ms. The largest individual delay in the matrix was 37.373 ms in the
third 32 MiB local PNG regular-file case. No performance threshold was added. The improved
method changes the reported numbers but still justifies off-thread named-media preparation
and bounded server emission. The historical tables below retain their original source labels
and must not be compared directly with the reviewed baseline.

After the merge, `cargo fmt --all -- --check`, `cargo test`,
`cargo clippy --all-targets -- -D warnings`, `cargo build`, the optimized benchmark build and
`git diff --check` passed with the framework override. This does not replace validation against
the published dependency. Subsequent registry verification is recorded below. No local path or
source-less lockfile entry is committed.

## Published dependency verification

After the matrix completed, the maintainer published tui-lipan v0.29.0. Rozi's manifest and
Cargo-generated registry lockfile now select 0.29.0 for both framework crates, with registry
sources and checksums. The ignored local override was removed. The release tag is
`a9a1005c0fd78fd928cadb1e8ee8cfe21474eb13`. Relative to the measured framework revision,
the only Rust source difference is `src/test_backend.rs`; terminal acquisition, deferred
preparation, parser and renderer code are unchanged. The measurements above retain their
original revision labels and were not rerun or relabeled as release measurements.

Without a local override, all of these passed against the registry dependency:

- `cargo fmt --all -- --check`
- `cargo test --locked`
- `cargo clippy --locked --all-targets -- -D warnings`
- `cargo build --locked`
- `cargo clippy --locked --target x86_64-pc-windows-gnu --all-targets -- -D warnings`
- `cargo deny check licenses sources advisories bans`
- `cargo audit`, with its two allowed warnings
- `git diff --check`

PR #241 remains a draft for review. The framework release and registry dependency integration
are complete. macOS and Windows runtime were not available locally; previous framework macOS
CI acquisition results and Linux live rendering evidence retain their original revision labels.

## Historical pre-review baseline

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


## Historical pre-review preparation and emission measurements

These historical measurements used the original, pre-review probe. They must be compared only
with the historical baseline below, not the reviewed baseline above. The drain timestamps, warmup
and CPU boundaries differ. They are retained with their original source labels.

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

All 147 runs completed, three repetitions of 49 cases. The historical baseline latency definition and aggregation apply. `f`, `t`, and `s` mean regular file, consuming temporary file, and consuming
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
