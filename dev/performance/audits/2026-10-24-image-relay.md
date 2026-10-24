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

Consuming-media slots (`t=t`, `t=s`) remain pending until server support. Real host correctness and
after measurements belong to the later implementation stage, not this baseline.
