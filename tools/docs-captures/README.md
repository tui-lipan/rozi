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
- `nvim`, `lazygit`, `btop`, `yazi`, `eza`, and `bat`, which fill the panes

The script builds a release rozi with `--features ui-snapshot`: clips need release speed to play
smoothly. A later plain `cargo build --release` replaces that binary with one that cannot write
PNGs, so drop `SKIP_BUILD` after one.

The `image-effects` and `graphics` clips also need `terminal-browser` on `PATH` and a network
connection: they show the live rozi.tui-lipan.dev in a pane.

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
| `frame.sh` | Encodes a clip as a terminal window: an MP4 with a title bar, or a GIF that adds a shadow and glow |
| `sandbox/profiles/` | Workspaces the scenes launch: `demo` (the README and landing page demo), `dev` (the workspace shot), `agents` (coding agents), `api` (overlays), `web` (layouts), `workspaces` (workspace switching), `media` (images) |
| `sandbox/nvim`, `btop`, `lazygit` | Program configs that use ANSI colors, so each rozi theme recolors them |
| `sandbox/home/.bashrc` | A short prompt and no history file |
| `sandbox/bin/icat` | Shows a PNG in a pane with the Kitty graphics protocol: `icat FILE [COLS ROWS]` |
| `sandbox/bin/fake-agent` | Run as `claude` or `codex`: draws the screens those agents draw in each state, from `tests/fixtures/agents/`, so detection reads it like the real program |
| `sandbox/bin/ssh` | Stands in for `ssh` in the `remote` clip: every host is this machine under the sandbox's `remote/` home |
| `sandbox/demo-change.sh` | The uncommitted edit applied to the demo checkout |

## Add a scene

Call `scene NAME VIEWPORT SETTLE_MS SCRIPT [EXTRA_CONFIG]` in `scenes.sh`. `SCRIPT` uses the
`TUI_LIPAN_SNAPSHOT_SCRIPT` steps, such as `key:ctrl+a; type:?; wait:500`. `EXTRA_CONFIG` is TOML
appended to the generated `config.toml`; start it with a table header. Set `PROFILE`, `LAYOUT`,
`STARTUP`, or `SETUP` in front of the call to change the workspace; `capture.sh` documents each.

Scenes switch off the session switching animation: a still frame is taken on a frozen clock, so
the animation would never get past its first, empty frame. Clips keep it.

Use `160x45` for whole-workspace shots and `120x34` for overlays, where the text needs to stay
readable at page width. Review the PNG in `/tmp/rzc-preview` before embedding the WebP.

## Add a clip

Call `clip NAME VIEWPORT SETTLE_MS STEPS [EXTRA_CONFIG]` in `scenes.sh`. It writes `NAME.mp4` and
`NAME-poster.webp`. `STEPS` has one step per line and runs in real time, not on the virtual clock
scenes use. `mouse:down,5,0`, `mouse:drag,130,20`, and `mouse:up,130,20` send a held
left-button drag at zero-based terminal cells, including several moves before release:

| Step | Effect |
| --- | --- |
| `key:C-a` | One key in tmux notation: `Enter`, `Escape`, `C-l`, `M-l`, `Up` |
| `type:TEXT` | Types `TEXT`, one character every 45 ms |
| `sleep:MS` | Waits |
| `run:COMMAND` | Runs a shell command in the sandbox, where `rozi` is this build |
| `float:X,Y,MS` | Alt-drags the floating pane on screen so its centre lands on cell `X,Y`, eased over `MS` |

A `rozi` typed into a pane is this build too. Clips are recorded at up to 60 frames a second
and written as a constant 60 fps MP4, without the recording indicator. A variable-rate file
lets one short gap advertise thousands of frames a second, and browsers then stutter through
the motion. The recording starts after `SETTLE_MS` and stops after the last step, so the clip
holds exactly the steps. Set `POSTER` to the second shown before the clip plays; the default
is its last frame. Set `FRAME` to a window title to record the MP4 as a terminal window with that
title bar (`frame.sh`); the page that plays it draws the corners, shadow, and glow. Set `GIF` to a
repository path to also write the clip as a GIF, which carries the whole window, shadow, and glow
itself. The `hero` clip uses both: its MP4 plays on the landing page and `assets/demo.gif` is the
repository README's demo. A blinking dot may be in its off phase at a given second, so check the poster in
`/tmp/rzc-preview` after each run.

rozi runs under tmux, which answers terminal queries as a terminal emulator does. Through a bare
pseudo-terminal, typed keys reach the focused pane but never rozi's own bindings.

## Embed a screenshot

Pages show captures with the `CaptureGallery` component, which falls back to plain images on
GitHub. See [Screenshots](../../.agents/instructions/documentation.md#screenshots) in the
documentation instructions.
