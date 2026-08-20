# rozi documentation

`rozi` is a Hyprland-style tiling terminal multiplexer built on the
[`tui-lipan`](../../tui-lipan) TUI framework. Each pane is a live PTY shell; panes are
arranged with dwindle, master, grid, columns, rows, scrollable, or monocle tiling, with floating
windows, fullscreen, 9 workspaces,
animated geometry, and tmux-style prefix commands.

`rozi` runs an always-server model: a background session server owns every PTY and the UI
attaches to it. A bare launch follows `[session] startup` — by default it opens the session picker;
a named target or `--session` attaches or launches from a same-named profile; use `new` to create
explicitly.

## Contents

| Page | What it covers |
| --- | --- |
| [Feature overview](features.md) | Single-page inventory of every feature, with links into the reference docs. |
| [Getting started](getting-started.md) | Requirements, platform support, building, running, and quitting. |
| [Installation & releases](installation.md) | Bootstrap installers, signed updates, rollback, managed layout, and release signing. |
| [Keybindings](keybindings.md) | Prefix mode, held modifier, mouse gestures, resize mode, and the full key table. |
| [Configuration](configuration.md) | The complete `config.toml` reference: shell, input, animations, theme, profile, clipboard. |
| [Agent definitions](agents.md) | Teaching rozi to recognize a coding-agent CLI and read its state, in `config.toml` or an extension. |
| [Layouts & panes](layouts-and-panes.md) | Tiled layout kinds, floating, fullscreen, split ratios, focus and movement. |
| [Sidebar](sidebar.md) | Docked sidebar configuration, built-in tabs, navigation, and shared-session sizing. |
| [Themes](themes.md) | The 29 selectable presets, the `system` theme and ANSI fallback, custom theme files, live hot-reload, and terminal colors. |
| [Terminal features](terminal.md) | The live terminal: mouse reporting, selection, clipboard (OSC52), titles, and scrollback search. |
| [Project profiles & pane identity](project-profiles.md) | Saving and restoring named workspace layouts. |
| [Named profiles](profiles.md) | Profile files, CLI launch/default profile priority, and in-app profile management. |
| [Control socket](control.md) | Per-run automation socket, pane environment, CLI commands, and JSON protocol. |
| [Agent skill](agent-skill.md) | Install the built-in Agent Skill (`rozi skill install`); `rozi --skill` still prints it. |
| [Hooks](hooks.md) | Event-triggered commands, event fields, environment variables, and control-socket callbacks. |
| [Extensions](extensions.md) | Stable identity/API, manifests, diagnostics, lifecycle, examples, and author workflow. |
| [Extension test lab](extension-testing.md) | Canonical extension setup, real-tool workflows, and adversarial lifecycle checks. |
| [Extension recipes](recipes.md) | Worked examples combining pickers, published rows, services, and hooks. |
| [Benchmarks & profiling](benchmarks.md) | Criterion targets, baseline comparisons, live stress recipes, and Samply profiling. |
| [Performance records](performance/README.md) | Current assessment, dated audit reports, and conventions for recording future results. |
| [Performance audit playbook](performance/audit-playbook.md) | Reproduce a full CPU, memory, latency, scaling, lifecycle, and profiling audit. |
| [Vim/Neovim navigator](../integrations/vim-rozi-navigator/) | Seamless navigation between editor splits and rozi panes. |
| [Sessions](sessions.md) | Ephemeral and named sessions, leave behavior, detach/reattach, and limitations. |
| [Remote SSH sessions](remote.md) | `--remote` attach over SSH, bootstrap/install, protocol negotiation, and feature split. |

## At a glance

- **Control paths** - a `Ctrl-a` prefix (always works) and a held `Alt`/`Super` modifier
  for active command keys. See [Keybindings](keybindings.md).
- **Layouts** - dwindle by default; master, grid, columns, rows, scrollable, and monocle per
  workspace; plus floating and fullscreen panes. See [Layouts & panes](layouts-and-panes.md).
- **Leave client** - `prefix q` or `prefix d` by default; named sessions keep running, while
  temporary sessions are closed or kept according to whether they contain work.
- **Named sessions** - `rozi <name>` creates/connects to a persistent PTY server.
- **Config file** - `$ROZI_CONFIG`, else `~/.config/rozi/config.toml`. See
  [Configuration](configuration.md).
- **Architecture / internals** - see [AGENTS.md](../AGENTS.md) at the repo root.
