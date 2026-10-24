# Named-image relay fairness

## Baseline

Measured on 2026-10-24 with Rozi `2518aa26` plus the stage-2 benchmark extension, registry
tui-lipan 0.28.5, Rust 1.96.1, Linux 7.2.8-9-omarchy, and an AMD Ryzen 7 9700X. The source SHA-256
was `e7caea7837611571b6cc756d494d8eb39724a395851c43f277b92ac7256340d8` for
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

CPU is the server's user plus system time from `/proc/<pid>/stat` at the start and end of the timed
interval, identity-checked by process start time and normalized to one core. The resolution is
0.5 percent. The table reports medians of each run's p50, p95 and CPU, and the largest maximum
across runs. It is not a pooled latency distribution. Raw 64 MiB relayed p95 varied from 4368.1 to
4395.1 ms. No CI threshold is justified by this short local sample.

| Format | MiB | Relay | p50 ms | p95 ms | Worst ms | Server CPU % |
| --- | ---: | --- | ---: | ---: | ---: | ---: |
| Text only | 0 | false | 0.023 | 0.038 | 0.980 | 0.0 |
| PNG | 1 | false | 0.022 | 0.044 | 2.129 | 2.0 |
| PNG | 1 | true | 0.025 | 0.580 | 6.748 | 3.0 |
| PNG | 8 | false | 0.026 | 9.852 | 15.333 | 15.5 |
| PNG | 8 | true | 7.744 | 39.810 | 47.761 | 29.0 |
| PNG | 32 | false | 8.823 | 54.245 | 60.498 | 59.5 |
| PNG | 32 | true | 1097.519 | 2002.634 | 2102.920 | 93.8 |
| PNG | 64 | false | 99.280 | 308.696 | 358.508 | 99.7 |
| PNG | 64 | true | 2545.925 | 3450.540 | 3576.537 | 93.3 |
| Raw RGBA | 1 | false | 0.019 | 0.037 | 2.222 | 0.5 |
| Raw RGBA | 1 | true | 0.020 | 0.734 | 8.080 | 4.5 |
| Raw RGBA | 8 | false | 0.021 | 0.039 | 1.481 | 1.0 |
| Raw RGBA | 8 | true | 9.431 | 35.605 | 47.772 | 36.0 |
| Raw RGBA | 32 | false | 0.024 | 0.045 | 5.680 | 4.0 |
| Raw RGBA | 32 | true | 1345.064 | 2249.866 | 2363.911 | 93.6 |
| Raw RGBA | 64 | false | 0.024 | 8.127 | 13.587 | 13.0 |
| Raw RGBA | 64 | true | 3483.718 | 4388.399 | 4491.207 | 94.1 |

The baseline justifies deferred media work. A full raw relay monopolizes the caller through
compression, base64 and chunk construction; PNG acquisition/decoding also stalls unrelated text
without relay. Move acquisition, validation/decoding and construction of relayed output to bounded
workers, then commit results through the original retained parser and transaction classifier.

Consuming-media slots (`t=t`, `t=s`) remain pending until server support. Real host correctness and
after measurements belong to the later implementation stage, not this baseline.
