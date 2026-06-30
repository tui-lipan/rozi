# Named profiles

Named profiles are saved workspace layouts stored as TOML files in:

```
~/.config/hyprmux/profiles/<name>.toml
```

Each profile records workspaces, pane titles, layout metadata, and optional launch
identity (`cwd`, `command`, `keep_open`, floating geometry). Profiles do **not** save live
PTY state — loading a profile tears down existing panes and starts fresh shells or commands.

This is separate from **session autosave** (`[session] autosave`), which silently persists
your last layout on quit. Named profiles are explicit, shareable layouts you choose to save
and load.

## Profile fields

Each pane entry supports:

| Field | Notes |
| --- | --- |
| `name` | Pane title shown in the titlebar. |
| `cwd` | Working directory when the pane launches. `~` expands to `$HOME`. |
| `command` | Run via `shell -lc <command>` when the pane opens. |
| `keep_open` | When `true`, append `; exec <shell>` so an interactive shell remains after the command exits. |
| `floating` | Start as a floating pane instead of tiled. |
| `fullscreen` | Start fullscreen. |
| `rect` | Floating geometry `{ x, y, w, h }`. |

Omit `tree` in a workspace to let hyprmux auto-build a dwindle tree from pane order.

## Example: lazygit + nvim

```toml
version = 1
active_workspace = 0

[[workspaces]]
index = 0
layout = "dwindle"
focused_pane = 0

[[workspaces.panes]]
id = 0
name = "lazygit"
command = "lazygit"
keep_open = true

[[workspaces.panes]]
id = 1
name = "nvim"
command = "nvim"
```

A commented copy lives at [`examples/profiles/dev.toml`](../examples/profiles/dev.toml).

## Startup: CLI and default profile

Launch a named profile from the command line:

```bash
hyprmux dev
hyprmux --profile dev
hyprmux -p dev
```

Startup priority:

1. CLI profile (`dev` above)
2. `[profile] default = "dev"` in `hyprmux.toml`
3. Session autosave (when enabled)

Set a default profile in config:

```toml
[profile]
default = "dev"
```

Or use the in-app **Set default profile** command (command palette).

If a profile file is missing or fails to parse, hyprmux shows a startup warning and falls
through to the next source (or a fresh layout).

## In-app commands

| Command | Action |
| --- | --- |
| **Save profile** | Prompts for a name and writes `profiles/<name>.toml`. |
| **Open profile** | Lists saved profiles; Enter loads the selection (replacing all panes). |
| **Set default profile** | Persists `[profile] default` to `hyprmux.toml`. |

## Command lifetime

Without `keep_open = true`, a pane closes when its `command` exits (`shell -lc` semantics).
Set `keep_open = true` to drop into an interactive shell instead — hyprmux builds
`command; exec <shell>` automatically.

See also [Project profiles & pane identity](project-profiles.md) for pane titles, saving
limitations, and session autosave details.
