# Documentation screenshots

`capture.sh` regenerates the screenshots and clips in `docs/assets/captures/`. Each scene launches
the real rozi binary headlessly, runs a short key script against it, and saves one frame as WebP.
Each clip runs rozi in a real terminal, records the whole UI with `rozi record start ui` while a
script types into it, and encodes the recording as MP4 with a WebP poster.

```bash
tools/docs-captures/capture.sh                  # every scene
tools/docs-captures/capture.sh keys-record      # one or more scenes by name
SKIP_BUILD=1 tools/docs-captures/capture.sh     # reuse the last ui-snapshot build
```

A full run takes about five minutes. Full-size PNGs of the same frames, and a copy of each clip,
go to `/tmp/rzc-preview` for review; only the WebP and MP4 files belong in the repository.

## Requirements

- Linux or macOS with `bash`, `git`, and ImageMagick 7 (`magick`)
- `python3`, `tmux`, and `ffmpeg` with libx264, for clips
- `nvim`, `lazygit`, `btop`, `eza`, and `bat`, which fill the panes

The script builds rozi with `--features ui-snapshot`. A later plain `cargo build` or `cargo test`
replaces that binary with one that cannot write PNGs, so drop `SKIP_BUILD` after one.

## Isolation

Scenes run under `env -i` with `HOME` and every XDG directory inside `/tmp/rzc`, which is deleted
after the run. The panes show a shallow clone of this repository in `/tmp/rzc-base`, with a fixed
author and one uncommitted edit so lazygit has a diff to show. Your config, sessions, history, and
checkout are never read or written.

## Files

| Path | Purpose |
| --- | --- |
| `scenes.sh` | The scene and clip list: name, viewport, settle time, script, and extra config |
| `clip.py` | Plays a clip's script in real time against rozi running in a private tmux server |
| `sandbox/profiles/` | Workspaces the scenes launch: `dev` (hero), `api` (overlays), `web` (layouts) |
| `sandbox/nvim`, `btop`, `lazygit` | Program configs that use ANSI colors, so each rozi theme recolors them |
| `sandbox/home/.bashrc` | A short prompt and no history file |
| `sandbox/bin/icat` | Shows a PNG in a pane with the Kitty graphics protocol: `icat FILE [COLS ROWS]` |
| `sandbox/demo-change.sh` | The uncommitted edit applied to the demo checkout |

## Add a scene

Call `scene NAME VIEWPORT SETTLE_MS SCRIPT [EXTRA_CONFIG]` in `scenes.sh`. `SCRIPT` uses the
`TUI_LIPAN_SNAPSHOT_SCRIPT` steps, such as `key:ctrl+a; type:?; wait:500`. `EXTRA_CONFIG` is TOML
appended to the generated `config.toml`; start it with a table header. Set `PROFILE`, `LAYOUT`,
`STARTUP`, or `SETUP` in front of the call to change the workspace; `capture.sh` documents each.

Use `160x45` for whole-workspace shots and `120x34` for overlays, where the text needs to stay
readable at page width. Review the PNG in `/tmp/rzc-preview` before embedding the WebP.

## Add a clip

Call `clip NAME VIEWPORT SETTLE_MS STEPS [EXTRA_CONFIG]` in `scenes.sh`. It writes `NAME.mp4` and
`NAME-poster.webp`. `STEPS` has one step per line and runs in real time, not on the virtual clock
scenes use:

| Step | Effect |
| --- | --- |
| `key:C-a` | One key in tmux notation: `Enter`, `Escape`, `C-l`, `M-l`, `Up` |
| `type:TEXT` | Types `TEXT`, one character every 45 ms |
| `sleep:MS` | Waits |
| `run:COMMAND` | Runs a shell command in the sandbox, where `rozi` is this build |

A `rozi` typed into a pane is this build too. The recording starts after `SETTLE_MS` and stops
after the last step, so the clip holds exactly the steps. Set `POSTER` to the second shown before
the clip plays; the default is its last frame. A blinking dot may be in its off phase at a given
second, so check the poster in `/tmp/rzc-preview` after each run.

rozi runs under tmux, which answers terminal queries as a terminal emulator does. Through a bare
pseudo-terminal, typed keys reach the focused pane but never rozi's own bindings.

## Embed a screenshot

Pages show captures with the `CaptureGallery` component, which falls back to plain images on
GitHub. See [Screenshots](../../.agents/instructions/documentation.md#screenshots) in the
documentation instructions.
