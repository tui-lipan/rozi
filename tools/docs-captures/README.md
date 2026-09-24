# Documentation screenshots

`capture.sh` regenerates the screenshots in `docs/assets/captures/`. Each scene launches the real
rozi binary headlessly, runs a short key script against it, and saves one frame as WebP.

```bash
tools/docs-captures/capture.sh                  # every scene
tools/docs-captures/capture.sh keys-record      # one or more scenes by name
SKIP_BUILD=1 tools/docs-captures/capture.sh     # reuse the last ui-snapshot build
```

A full run takes about three minutes. Full-size PNGs of the same frames go to `/tmp/rzc-preview`
for review; only the WebP files belong in the repository.

## Requirements

- Linux or macOS with `bash`, `git`, and ImageMagick 7 (`magick`)
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
| `scenes.sh` | The scene list: name, viewport, settle time, key script, and extra config |
| `sandbox/profiles/` | Workspaces the scenes launch: `dev` (hero), `api` (overlays), `web` (layouts) |
| `sandbox/nvim`, `btop`, `lazygit` | Program configs that use ANSI colors, so each rozi theme recolors them |
| `sandbox/home/.bashrc` | A short prompt and no history file |
| `sandbox/demo-change.sh` | The uncommitted edit applied to the demo checkout |

## Add a scene

Call `scene NAME VIEWPORT SETTLE_MS SCRIPT [EXTRA_CONFIG]` in `scenes.sh`. `SCRIPT` uses the
`TUI_LIPAN_SNAPSHOT_SCRIPT` steps, such as `key:ctrl+a; type:?; wait:500`. `EXTRA_CONFIG` is TOML
appended to the generated `config.toml`; start it with a table header. Set `PROFILE`, `LAYOUT`,
`STARTUP`, or `SETUP` in front of the call to change the workspace; `capture.sh` documents each.

Use `160x45` for whole-workspace shots and `120x34` for overlays, where the text needs to stay
readable at page width. Review the PNG in `/tmp/rzc-preview` before embedding the WebP.

## Embed a screenshot

Pages show captures with the `CaptureGallery` component, which falls back to plain images on
GitHub. See [Screenshots](../../.agents/instructions/documentation.md#screenshots) in the
documentation instructions.
