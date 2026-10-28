# Gallery screenshots

The extension gallery on rozi.tui-lipan.dev shows the images an extension lists in
`[extension] screenshots`. Capture them in a real rozi UI with `scripts/screenshot.sh`, never with
a desktop screenshot of your own terminal: the script runs a throwaway rozi at an exact size and
captures it with `rozi capture-ui --render png --scale 2`. It starts from an empty environment with
its own home and XDG directories, so your config, sessions, history, and shell prompt never reach
the shot, and pane titles carry only the directory name.

It does not hide the machine itself. rozi, the extension, and anything a scene runs can still read
the real hostname, username, and system details (`hostname`, `uname`, `id`), and a remote or
host-aware view may show them. Keep scenes to the local lab, and check the image before publishing
it.

## The target

| Property | Value | Why |
| --- | --- | --- |
| Size | `160x50` cells, `--scale 2` → 2560×1600 px | rozi draws 8×16-pixel cells, so 160×50 is exactly 16:10, the card's ratio, and nothing is cropped |
| Format | `.webp` | 100–300 KB where a PNG is 0.3–1 MB; the index refuses files over 2 MiB |
| Path | `docs/screenshot.webp`, with its scene beside it as `docs/screenshot-scene.sh` | the scene makes the image reproducible after a UI change |
| Count | one, at most four | the card shows the first; the others are behind dots |

A card crops from the top left when an image is not 16:10, so put what matters there. Use
`120x34` only for a second image that zooms in on an overlay.

## Capture

```bash
ROZI_BIN=/path/to/rozi \
  .agents/skills/rozi-extension/scripts/screenshot.sh \
    --extension . --scene docs/screenshot-scene.sh --output docs/screenshot.webp
```

`ROZI_BIN` must accept every manifest key the extension uses, `screenshots` included, satisfy
`min_rozi`, and support `sessions new --detach`. The script starts its session detached and attaches
the only UI inside its own tmux server, because `capture-ui` needs one; never attach a UI from your
own terminal for a capture. Inside the lab, `rozi` is that binary for the scene and the
extension alike. `--keep` leaves the lab directory for inspection; `--config FILE` appends TOML to
the lab's `config.toml`, for a theme or extension settings.

Then list the image and check it:

```toml
[extension]
screenshots = ["docs/screenshot.webp"]
```

```bash
rozi extensions check .   # the Discover section names a missing, oversized, or misnamed image
```

## Write the scene

A scene is a Bash fragment sourced once the UI is attached, from `$DEMO` (`~/src/demo` in the lab).
It drives rozi through public commands (`rozi split`, `rozi focus`, `rozi run-action`) and the
helpers `keys` (tmux key names: `C-a`, `Enter`, `Escape`), `settle MS`, and `bin NAME BODY`, which
puts an executable stub on the lab `PATH`. `$EXTENSION` is the extension directory, `$LAB` the lab.

- **Stage everything.** Never show real conversations, accounts, repositories, paths, or hosts.
  Make Git repositories and worktrees under `$HOME/src` with `git init -b <branch>` so Activity
  groups rows by a believable repository and branch, and commit with
  `-c user.name=demo -c user.email=demo@example.invalid`.
- **Stub what the extension calls.** Replace an agent CLI with `bin claude '…'` that prints the JSON
  your service parses and, run plainly, draws a plausible screen. Answer a daemon protocol with a
  small fake server started in the background. Reuse the fakes your tests already have. The stub
  must keep the program name rozi reads: let the script itself wait (`while read -r _; do :; done`)
  rather than `exec` into another program.
- **Lay out before drawing.** Open the sidebar (`keys C-a b`) and split panes first, then start a
  program that draws at its pane's width, or it draws for the old width and rozi reflows it.
- **Run `cd` as its own command.** The pane title follows the prompt's directory report, which
  happens at the next prompt, not inside `cd dir && program`.
- **Wait for the extension.** Settle past at least two service polls, and past a network load.
- **Clean up daemons.** Define `teardown() { … }` to stop anything the scene started; the lab is
  removed after it runs.
- **Show the extension, not rozi.** Its rows, picker, pane, or effect should be the obvious subject,
  with enough context around it to say what it does.

The four tui-lipan extensions (`claude-rozi-sessions`, `codex-rozi-sessions`,
`rozi-terminal-browser`, `vim-rozi-navigator`) each keep a `docs/screenshot-scene.sh` to copy from:
stubbed agent CLIs, a fake WebSocket daemon, a live web page, and a Neovim layout.

## Review before committing

Read the PNG at full size (`--output shot.png` while iterating) and check:

- no hostname, username, real path, token, or personal content anywhere, including pane titles,
  the workbar, and sidebar rows: the lab does not mask the machine's identity;
- no error toast, half-loaded page, or empty pane where content was expected;
- text that fits its pane without wrapping where the real program would not wrap;
- the subject sits in the top-left 16:10 region.

`capture-ui` draws italics upright and shows the settled state of an animation, not a frame from
the middle of one. Re-run the capture after any visible UI change, in rozi or the extension, and
commit the image together with its scene.
