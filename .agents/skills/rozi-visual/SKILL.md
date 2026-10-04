---
name: rozi-visual
description: Inspect and demonstrate live terminal applications through rozi panes, capture screenshots, and record or export visual flows. Use for a requested rozi visual review or a live preview in a verified rozi pane. This skill controls rozi's terminal presentation; it does not design tui-lipan components or use the framework's headless renderer.
---

# rozi visual

Use rozi's own CLI to inspect what a terminal application actually displays, including real
subprocess output and timers. For component design or deterministic tui-lipan rendering, use the
framework's visual skill separately.

## Establish the target

Read `rozi skill print` for endpoint, targeting, and input rules, then `rozi api describe` to
check capabilities. A UI endpoint requires `ROZI=1` and a non-empty `ROZI_SOCKET`. Do not discover
and control an arbitrary focused UI when that context is absent. An explicitly named session
can be inspected through `rozi --session NAME` without a UI.

| Task | Endpoint |
| --- | --- |
| List panes, send input, capture a pane, record a pane | Verified UI or named session |
| Reveal or focus a preview, capture/record chrome and overlays | Verified UI |
| Export a completed recording | Local file; no endpoint |

Inspect live pane IDs and layout before acting. Keep all input and captures targeted to the
returned ID; do not use focus as a target. Work on a requested pane or one you created. A requested
preview authorizes creating its pane; changes to other panes and layouts need their own scope.

## Show a running application

Build the requested executable first. Launch it directly with `--argv` rather than typing a
shell command into an existing pane:

```sh
rozi split --cwd /absolute/project --title 'Preview' --argv /absolute/project/target/debug/app
```

Save `data.id` from the JSON response. `pty_ready:false` means the pane is starting, not that
creation failed; do not split again. Wait for a recognizable screen:

```sh
rozi capture-pane --target <ID> --wait-for 'Ready' --timeout 30s --format json
rozi capture-pane --target <ID> --render png --output /absolute/scratch/preview.png
```

Inspect the saved PNG with the image viewer. Text is useful for readiness and output, but does
not prove colors, borders, clipping, or layering. `capture-ui --render png --output ...` includes
rozi's chrome and visible overlays; a pane capture shows just that terminal application.

If the user wants to see the preview, use `pane reveal --target <ID>` in a Scrollable layout to
show it without changing focus. Use `split --focus` only when activating the preview is within
the request, and only through a UI endpoint. `--session NAME split --focus` cannot work.
Leave an approval preview running and tell the user its controls. Close only your own temporary
panes when they are no longer needed.

## Drive and inspect a flow

Inspect the target before input, then wait on its response:

```sh
rozi send-keys --target <ID> m --settle 500ms --timeout 10s --capture text
rozi capture-pane --target <ID> --render png --output /absolute/scratch/open.png
```

An animation with a continuously updating clock may never settle. Use recognizable text for
readiness and a recording for motion. A settled screenshot establishes the final screen, not
whether opening or closing was smooth. Re-read IDs after a session or layout change.

## Record real motion

For a requested video or recording, record only the target needed for that task. Use absolute
scratch paths and a bounded duration. Set `--max-fps 60` for short animation reviews; the cap
limits capture frequency and cannot create frames that the application did not draw.

```sh
rozi record start --target <ID> --output /absolute/scratch/demo.rozirec --max-fps 60 --duration 30s
rozi record mark 'opening' --target <ID>
rozi send-keys --target <ID> d
rozi record stop --target <ID>
```

For chrome, pane movement, or overlays owned by rozi, use `record start ui` and `record stop --ui`
instead. A pane recording cannot show rozi's pane closing animation. Avoid recording unrelated
panes or personal screens. Verify completion with `record list`; do not stop another recording.
Detached recording paths refer to the session host; copy a remote file locally before export.

Live recording follows actual elapsed time, so delayed commands and subprocess output are
included. It is not a deterministic substitute for framework animation tests.

## Deliver a playable preview

Stop recording before export. Keep `.rozirec` as the source. Export a cast when text replay is
enough; it replaces displayed images with their terminal half-block approximations:

```sh
rozi record export /absolute/scratch/demo.rozirec --to cast /absolute/scratch/demo.cast
```

For a video, use `video-frames` to keep a constant canvas even when the terminal resized:

```sh
rozi record export /absolute/scratch/demo.rozirec --to video-frames /absolute/scratch/frames
ffmpeg -f concat -safe 0 -i /absolute/scratch/frames/frames.ffconcat \
  -vf 'pad=ceil(iw/2)*2:ceil(ih/2)*2' -c:v libx264 -bf 0 -pix_fmt yuv420p -fps_mode vfr \
  -movflags +faststart /absolute/scratch/demo.mp4
```

`video-frames` pads each frame to the maximum recorded width and height without stretching its
content. It renders the native frame before padding its pixels, preserving clipping at terminal
edges. `png-frames` preserves each frame's native dimensions. Both emit `frames.ffconcat`
with actual durations: use that listing, not a guessed `-framerate`, to preserve motion and
pauses. Check `rozi record --help` before using `video-frames` on an older binary.

Inspect frames at entry, exit, and the final state before handing over the artifact. Report
truncation or dropped changes if present. Delete temporary frame directories you created after
encoding; keep requested deliverables outside the repository. Repository documentation captures
must follow `tools/docs-captures/README.md` and use an isolated session.
