# hyprmux documentation

`hyprmux` is a Hyprland-style tiling terminal multiplexer built on the
[`tui-lipan`](../../tui-lipan) TUI framework. Each pane is a live PTY shell; panes are
arranged with dwindle (or master) tiling, with floating windows, fullscreen, 9 workspaces,
animated geometry, and tmux-style prefix commands.

Local mode keeps PTYs in the UI process. Explicit `--attach` / `--session` mode connects to a
named session server for persistent PTYs.

## Contents

| Page | What it covers |
| --- | --- |
| [Getting started](getting-started.md) | Building, running, quitting, and the `tui-lipan` path dependency. |
| [Keybindings](keybindings.md) | Prefix mode, held modifier, mouse gestures, resize mode, and the full key table. |
| [Configuration](configuration.md) | The complete `hyprmux.toml` reference: shell, input, animations, theme, profile, clipboard. |
| [Layouts & panes](layouts-and-panes.md) | Dwindle vs master tiling, floating, fullscreen, split ratios, focus and movement. |
| [Themes](themes.md) | The 10 built-in presets, the `system` theme, custom theme files, live hot-reload, and terminal ANSI colors. |
| [Terminal features](terminal.md) | The live terminal: mouse reporting, selection, clipboard (OSC52), titles, and scrollback search. |
| [Project profiles & pane identity](project-profiles.md) | Saving and restoring named workspace layouts. |
| [Sessions](sessions.md) | Local vs attached runtime, named sessions, detach/quit semantics, and limitations. |

## At a glance

- **Control paths** - a `Ctrl-a` prefix (always works) and a held `Alt`/`Super` modifier
  for active command keys. See [Keybindings](keybindings.md).
- **Layouts** - dwindle tiling by default, master/stack per workspace, plus floating and
  fullscreen panes. See [Layouts & panes](layouts-and-panes.md).
- **Detach** - `prefix d` by default; leaves attached sessions running or saves locally before exit.
- **Config file** - `$HYPRMUX_CONFIG`, else `~/.config/hyprmux/hyprmux.toml`. See
  [Configuration](configuration.md).
- **Architecture / internals** - see [AGENTS.md](../AGENTS.md) at the repo root.
