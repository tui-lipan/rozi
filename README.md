# hyprmux

`hyprmux` is a Hyprland-style tiling **terminal multiplexer**. Panes are live PTY shells laid
out with dwindle tiling, plus floating windows, workspaces, animated geometry, and tmux-style
prefix commands. It is built on the [`tui-lipan`](../tui-lipan) TUI framework and was ported
from that project's `window_manager` example.

hyprmux runs an always-server model: a background session server owns every PTY and the UI always
attaches to it. A bare launch uses a disposable per-process ephemeral session; explicit `--attach` /
`--session` connects to a persistent named session for detach/reattach-style workflows.

It supports native Linux and macOS builds. Windows named-pipe and ConPTY support remains in
development.

```bash
cargo run     # leave with prefix d (detach), or bind quit in hyprmux.toml
```

## Highlights

- **Dwindle + master tiling** - new panes split the *focused* tile along its aspect ratio
  (Hyprland dwindle), or switch to a master/stack layout per workspace.
- **Floating & fullscreen panes** - toggle any pane to floating or fullscreen; move and
  resize floats with the mouse.
- **9 workspaces** with a workbar tab strip showing live pane counts.
- **Animated geometry** - spawn, close, fullscreen, tile/float, and split-axis transitions,
  all individually configurable (size changes are snapped to avoid SIGWINCH spam).
- **tmux-style prefix + held-modifier control** - `Ctrl-a` prefix that always works, plus an
  `Alt`/`Super` direct path for active command keys.
- **Command palette & help overlay** - fuzzy command search (`p`) and a full keybinding
  reference (`?`).
- **Pane identity** - rename panes (`n`); titles also follow the program's OSC title.
- **Named workspaces** - rename a workspace to show `<number>:<name>` in the tabs, usable in the
  `{workspace}` workbar placeholder, saved with profiles and session autosave.
- **Named profiles** - save layouts to `~/.config/hyprmux/profiles/`, load via CLI (`hyprmux dev`) or the in-app picker, and set a default profile in config.
- **Named sessions** - attach to persistent server-backed sessions with `hyprmux --attach dev`
  (or add `--read-only` for a viewer),
  detach/reattach later, and shut them down explicitly when done.
- **Scrollback search** - search a pane's scrollback (`/`) and jump between matches.
- **Copy mode** - vi-style scrollback review with `hjkl`, word/WORD (`w`/`b`/`e`) and line
  (`0`/`^`/`$`) motions, and clipboard yank - the motions reuse `tui-lipan`'s vim-mode `TextArea`
  algorithms rather than a separate implementation.
- **Themes** - 10 built-in presets, a host-derived `system` theme, drop-in custom theme files,
  and live hot-reload. Terminal ANSI colors are derived from the active theme.
- **Real terminal** - mouse reporting, text selection, scroll-wheel scrollback, clipboard paste
  (`v`), and OSC52 clipboard, provided by `tui-lipan`'s terminal primitives.
- **Scriptable control socket** - a per-run Unix socket and `hyprmux` CLI (`list-panes`, `focus`,
  `send-text`, `new-pane`, `run-action`, `capture-pane`, `switch-workspace`, `move-to-workspace`)
  for external automation.
- **Extensible workbar & keybindings** - workbar segments can run a shell command on a timer, and
  `[keys]` entries can define new key-triggered commands that open a pane or send text, beyond
  rebinding built-in actions.

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
- [Vim/Neovim navigator](integrations/vim-hyprmux-navigator/) - seamless editor split and hyprmux pane navigation.
- [Sessions](docs/sessions.md) - always-server model, ephemeral vs named sessions, rename, and lifecycle.

For framework/internal architecture notes, see [AGENTS.md](AGENTS.md).

## Sponsor

If hyprmux is useful to you, consider
[sponsoring its development](https://github.com/sponsors/Razuer) ♥

## License

MIT OR Apache-2.0.
