# hyprmux

`hyprmux` is a Hyprland-style tiling **terminal multiplexer**. Panes are live PTY shells laid
out with dwindle tiling, plus floating windows, workspaces, animated geometry, and tmux-style
prefix commands. It is built on the [`tui-lipan`](../tui-lipan) TUI framework and was ported
from that project's `window_manager` example.

By default hyprmux runs in local single-process mode. Explicit `--attach` / `--session` mode
connects to a named always-on session server for detach/reattach-style workflows.

```bash
cargo run     # quit with Ctrl-q
```

## Highlights

- **Dwindle + master tiling** - new panes split the *focused* tile along its aspect ratio
  (Hyprland dwindle), or switch to a master/stack layout per workspace.
- **Floating & fullscreen panes** - toggle any pane to floating or fullscreen; move and
  resize floats with the mouse.
- **9 workspaces** with a top-bar tab strip showing live pane counts.
- **Animated geometry** - spawn, close, fullscreen, tile/float, and split-axis transitions,
  all individually configurable (size changes are snapped to avoid SIGWINCH spam).
- **tmux-style prefix + held-modifier control** - `Ctrl-a` prefix that always works, plus an
  `Alt`/`Super` direct path for active command keys.
- **Command palette & help overlay** - fuzzy command search (`p`) and a full keybinding
  reference (`?`).
- **Pane identity** - rename panes (`n`); titles also follow the program's OSC title.
- **Named profiles** - save layouts to `~/.config/hyprmux/profiles/`, load via CLI (`hyprmux dev`) or the in-app picker, and set a default profile in config.
- **Scrollback search** - search a pane's scrollback (`/`) and jump between matches.
- **Themes** - 10 built-in presets, a host-derived `system` theme, drop-in custom theme files,
  and live hot-reload. Terminal ANSI colors are derived from the active theme.
- **Real terminal** - mouse reporting, text selection, scroll-wheel scrollback, and OSC52
  clipboard, provided by `tui-lipan`'s terminal primitives.

## Documentation

Full docs live in [`docs/`](docs/):

- [Getting started](docs/getting-started.md) - build, run, quit, and the dependency on `tui-lipan`.
- [Keybindings](docs/keybindings.md) - prefix, held modifier, mouse, and the full key reference.
- [Configuration](docs/configuration.md) - the complete `hyprmux.toml` reference.
- [Layouts & panes](docs/layouts-and-panes.md) - dwindle, master, floating, fullscreen, resize.
- [Themes](docs/themes.md) - presets, custom theme files, hot reload, and terminal colors.
- [Terminal features](docs/terminal.md) - PTY, mouse, selection, clipboard, and scrollback search.
- [Project profiles & pane identity](docs/project-profiles.md) - save and restore layouts.
- [Named profiles](docs/profiles.md) - profiles directory, CLI launch, keep-open, picker commands.
- [Control socket](docs/control.md) - pane environment, JSON control protocol, and CLI automation.
- [Sessions](docs/sessions.md) - local vs attached runtime, named sessions, and lifecycle limits.

For framework/internal architecture notes, see [AGENTS.md](AGENTS.md).

## License

MIT OR Apache-2.0.
