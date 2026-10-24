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

Consuming-media slots (`t=t`, `t=s`) remain pending until server support. Real host correctness and
after measurements belong to the later implementation stage, not this baseline.
