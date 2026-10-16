# Graphics resize latency

The roughly 200 ms picture-update gap after resizing comes from tui-lipan's default 180 ms image
rendering pause. Making that pause opt-in removes the gap. Screen clearing must also invalidate
native image uploads so static pictures survive resize without waiting for new producer output.

## Cause and fix

The fullscreen runner suspended image rendering after every host resize. This affected terminal
pictures as well as `Image` widgets, and continued output could not bypass the suspension.
`TUI_LIPAN_IMAGE_RESIZE_PAUSE_MS` now defaults to `0`, and zero skips suspension entirely. Setting
it to `180` restores the previous policy. Animated `Image` widgets retain their separate layout
stabilization setting.

Removing the pause exposed a correctness defect that a moving-picture benchmark missed. The
renderer clears the host screen during resize, and that erase can remove native image data. The
native scene still treated the source as uploaded and sent only a placement command. A static
picture disappeared until its producer changed it. Both backend screen-clear paths now invalidate
native upload state. The next paint reuploads the retained source using its reserved IDs.

## Builds and conditions

- rozi `a6ba03fc`, with a local Cargo override to the framework worktree. No rozi application code
  or committed dependency files changed. Subsequent integration checks updated two test fixtures
  to use complete transmit-and-place replacements, matching the framework's retransmission
  correction in `113ac1b`; those test-only changes do not affect the measured release binary.
- Framework `113ac1b` plus changes to runner resize suspension, backend clear invalidation, two
  regression tests, and the image guides. This is a dirty-worktree measurement, not a measurement
  of the unmodified commit. The complete framework diff is retained in the evidence directory;
  SHA-256 `43789d48431e93f9ba5a062bf9d0fac561b207dd7b2a10907c414f1c698e5f27`.
  The measured framework source tree was subsequently committed as `9ccd9af` in
  [tui-lipan #340](https://github.com/tui-lipan/tui-lipan/pull/340), based on merged
  [#337](https://github.com/tui-lipan/tui-lipan/pull/337).
- Release binary `/tmp/rozi-graphics-bench-bin/rozi-resize-complete`, SHA-256
  `1e4e740c569c90b723ba759e4a745518fc0aadbb1136580f06849a1c6065a4e9`.
- Rust/Cargo 1.96.1, Linux `7.2.5-4-omarchy`, Ghostty `1.3.1-arch2`, Hyprland, DP-2
  3840 × 2160 at 239.99001 Hz, scale 1. Isolated user directories and owned floating windows;
  no desktop configuration or installed application changes.
- Verified host window 980 × 720, JetBrainsMono Nerd Font size 8; resize to 820 × 600 three seconds
  into each eight-second sample. Synthetic immutable 640 × 320 RGBA pictures. Animations disabled.
- The same release binary serves every row. The `180` row restores the old delay setting while
  retaining the backend correctness fix. Builds finish before sampling.
- The wiretap observes Ghostty's terminal reads and the server's producer reads. Its SHA-256 is
  `be1d9d5740ebae2b4c058420b018bacebbc1e1c5a64672e1f47386b4642c918e`. These instrumented samples
  establish output timing, not a CPU comparison or monitor scanout timing.

## Resize timing

These are the longest gaps within each sample, including the resize. Graphics-frame gaps use
completed synchronized updates containing a graphics command. Picture-upload gaps require an
upload with the fixture's identifying pixel; a deletion-only update does not qualify.

| Path and resize pause | Client / producer Hz | Longest graphics-frame gap, ms | Longest picture-upload gap, ms |
| --- | ---: | ---: | ---: |
| Native, old `180` ms setting | 60 / 60 | 182.7 | 183.5 |
| Native, new default | 60 / 60 | 17.0 | 17.0 |
| Native, new default | 240 / 240 | 6.9 | 6.9 |
| Composed, new default | 60 / 60 | 18.7 | Not available |
| Native, explicit `0` ms setting | 60 / 240 | 18.3 | 18.3 |

Every producer reported one resize, and framework failed-frame counters stayed at zero. The
composed path does not expose the fixture marker through this media tap, so its picture-upload
gap is not inferred from the graphics-frame count.

Raw cases are under `target/rz-legacy60`, `target/rz-default60`, `target/rz-default240`,
`target/rz-composed60`, and `target/rz-zero60from240` in the original rozi checkout. Each has
`result.json`, wire records, and `resize-analysis.json` with the exact command and settings.

To repeat the default 60 Hz case from this worktree, use a fresh output directory:

```bash
python3 tools/graphics-benchmark.py \
  --binary /tmp/rozi-graphics-bench-bin/rozi-resize-complete \
  --window-lab /home/razuer/.agents/skills/ghostty-window-lab/scripts/window_lab.py \
  --output /home/razuer/Projects/rozi/target/rz-repeat60 \
  --fps 60 --producer-fps 60 --seconds 8 --resize-at 3 --workspace 2 \
  --wiretap /tmp/resize-latency-wiretap.so --tap host-read --pty-tap
```

The samples explicitly unset inherited image-mode and resize-pause overrides, except for the
settings named in the table. The harness strips SSH environment variables to select local
transport. Select the currently visible workspace when reproducing; do not switch the user's
desktop for the benchmark.

## Static pictures and clipping

`terminal_patch_layers` uses the complete framework change in a debug build, SHA-256
`834c823294984ed3cbbed6144a9f3cfbae89ba54c9dfe9362d711a64ba67cbdc`. Native and forced-composed
windows are captured at the same screen position, after the host resize badge disappears.

| State | Window size | Different pixels, native versus composed |
| --- | --- | ---: |
| Initial green picture | 980 × 720 | 0 |
| Shrunk window, producer unchanged | 220 × 180 | 0 |
| Blue replacement while shrunk | 220 × 180 | 0 |
| Restored window, producer unchanged | 980 × 720 | 0 |
| Picture clipped on the right | 180 × 150 | 0 |
| Original size restored after clipping | 980 × 720 | 0 |

The clipping case shows 17,136 green pixels instead of the full 19,584, then restores all 19,584.
Wire records show a source upload followed by placement after each resize, preserving image and
placement IDs for the unchanged source. Before backend clear invalidation, the shrunk and
restored static scenes differed by 19,584 pixels because the native picture was missing.

Curated screenshots, wire records, the source diff, and check logs are in
`target/resize-evidence` in the original rozi checkout. All owned windows and sessions were closed.

## Validation and limits

Both new regression tests pass with the fix and fail with the corresponding old behavior.
Framework macro formatting, build, strict all-target/all-feature lint, format check, strict
workspace documentation, and all-feature workspace tests pass, including 3,647 library tests.
The optimized rozi build using the framework override also passes. After updating the two
replacement fixtures, rozi's default test suite passes, including 2,625 library tests, its
integration tests, and doctests. Strict all-target/all-feature Clippy and the format check pass
against the same local framework override. The original manifest and lockfile are restored after
these checks.

Each timing case is one eight-second sample. Live graphics validation covers Ghostty on Linux;
other hosts and platforms were not measured. The separate animated-image layout pause remains
configurable and is outside this terminal-picture resize fix.
