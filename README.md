# hyprmux

`hyprmux` is a single-process terminal multiplexer that ports the Hyprland-style
`window_manager` example from `tui-lipan` into a real app: panes are live PTYs,
laid out with dwindle tiling, floating windows, workspaces, and tmux-style prefix
commands.

Highlights:

- Pane identity: rename panes with the prefix command `n` or the command palette.
- Project profiles: restore named panes, workspace layout, floating geometry, and
  fresh shell/command launches from a TOML profile.
- Profiles intentionally restore layout and launch intent only, not live PTY state.
  See [Project profiles and pane identity](docs/project-profiles.md).

## Run

```bash
cargo run
```

## Keybindings

The always-works control path is the prefix key: press `Ctrl-a`, then a command.

| Prefix command | Action |
| --- | --- |
| `Enter` or `c` | spawn a new shell pane |
| `w` or `x` | close the focused pane |
| `h/j/k/l` or arrows | spatial focus |
| `1..9` | switch workspace |
| `Shift+1..9` | move pane to workspace |
| `t` | toggle floating/tiling |
| `f` | toggle fullscreen |
| `n` | rename the focused pane |
| `Space` | flip focused split |
| `[` / `]` | adjust split ratio |
| `Ctrl-a` | send a literal `Ctrl-a` to the pane |

Held-modifier bindings map to the same actions. The default modifier is `Alt`,
because `Super` is rarely delivered reliably by terminal emulators. The prefix
scheme remains the most portable path.

Mouse move/resize uses the configured modifier plus left/right drag.

## Notes

`hyprmux` intentionally does not implement tmux-style detach/reattach yet. PTYs
live inside the single UI process.

Project profiles follow the same rule: they start fresh PTYs from saved layout and
launch metadata rather than restoring previous shell processes.
