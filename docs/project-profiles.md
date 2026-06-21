# Project profiles and pane identity

Project profiles let `hyprmux` restore a workspace layout for a project. A profile is a TOML file that records workspaces, pane names, layout metadata, and optional launch identity for each pane.

Profiles do **not** save or restore live PTY state. Restoring a profile starts fresh shells or commands in new PTYs; it does not resurrect the shell processes, scrollback, environment, or running programs from an earlier `hyprmux` process.

## Pane identity

Each pane carries an *identity* — the information `hyprmux` knows about it beyond its live
shell:

- **Custom title** — a name you set with the `n` keybinding (or *Rename pane* in the command
  palette). It overrides the program's terminal title. Submitting an empty name clears it.
- **Profile name** — the name a pane was restored with from a profile.
- **cwd / command** — the working directory and launch command, when known (e.g. for panes
  restored from a profile). These appear as the pane's subtitle and are what a `Save project
  profile` writes back.

The titlebar shows the custom title if set, otherwise the program's terminal title, otherwise
the default label `shell`. See [Layouts & panes › Titlebars](layouts-and-panes.md#titlebars)
for the full precedence, and [Keybindings](keybindings.md) for the rename flow.

## Enable a profile

Set a profile path in your `hyprmux.toml`:

```toml
[profile]
path = "~/code/my-app/hyprmux-profile.toml"
```

On startup, `hyprmux` loads that file when it exists and shows a startup message for success or failure. The command palette action `Save project profile` writes the current profile back to this path.

## Profile shape

Each profile has a version, the active workspace, and workspace entries. Pane entries can use these fields:

- `name`: pane title shown by `hyprmux` and restored on startup.
- `cwd`: directory used when launching that pane's fresh shell or command. `~` and `~/...` expand to `HOME`.
- `command`: shell command string passed to the configured shell as `shell -lc <command>` when launching the pane.
- `floating`: whether the pane is floating instead of tiled.
- `fullscreen`: whether the pane starts fullscreen.
- `rect`: floating geometry as `{ x, y, w, h }`; used for floating panes.

Example:

```toml
version = 1
active_workspace = 0

[[workspaces]]
index = 0
layout = "dwindle"
focused_pane = 0

[[workspaces.panes]]
id = 0
name = "server"
cwd = "~/code/my-app"
command = "cargo run; exec ${SHELL:-/bin/sh}"

[[workspaces.panes]]
id = 1
name = "logs"
cwd = "~/code/my-app"

[[workspaces]]
index = 1
layout = "master"

[[workspaces.panes]]
id = 0
name = "scratch"
floating = true
rect = { x = 8.0, y = 4.0, w = 80.0, h = 24.0 }
```

## Command lifetime

Profile commands run as `shell -lc <command>`. If the command exits, the shell exits and `hyprmux` closes that pane. To keep a pane open after a command finishes, replace the shell process at the end of the command:

```toml
command = "cargo run; exec ${SHELL:-/bin/sh}"
```

## Saving limitations in v1

Saving a hand-built session preserves pane names, workspace layout, split tree/ratios, floating state, fullscreen state, and floating geometry.

It does not inspect running shells to discover their current working directory or original command. `cwd` and `command` are saved only for panes whose identity already knows those values, such as panes restored from a profile. Rename panes explicitly when you want stable profile titles.
