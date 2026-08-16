# Getting started

## Requirements

- A Rust toolchain of at least **1.88** (edition 2024; this is the MSRV, and CI builds on it).
- A real terminal emulator (the app drives a full-screen TUI and spawns PTYs).

The current `Cargo.toml` uses the sibling `../tui-lipan/` checkout directly, with the `terminal`,
`terminal-images`, `terminal-serde`, `clipboard-images`, and `theme-reload` features. Clone or
place `tui-lipan` next to this repository before building. `terminal` brings in `portable-pty` +
`alacritty_terminal` for the PTY-backed terminal widget; `terminal-images` lets programs in a pane
draw pictures (see [Terminal features](terminal.md)); `theme-reload` enables live theme hot-reload.

> **Publish/lock note.** The current path dependency produces a `Cargo.lock` entry without a
> registry source or checksum. Before standalone clones, CI, or releases can build without the
> sibling checkout, publish the required `tui-lipan` version, replace the path dependency with its
> registry version requirement, and regenerate `Cargo.lock` in that registry configuration. Do not
> treat a planned registry version as the dependency currently selected by this manifest.

## Platform support

| | Linux | macOS | Windows |
| --- | --- | --- | --- |
| PTYs | Unix PTY | Unix PTY | ConPTY |
| Control + session IPC | Unix-domain sockets | Unix-domain sockets | Named pipes |
| Config directory | `$XDG_CONFIG_HOME/rozi`, else `~/.config/rozi` | same | `%APPDATA%\rozi` |
| State directory | `$XDG_STATE_HOME/rozi`, else `~/.local/state/rozi` | same | `%LOCALAPPDATA%\rozi` |
| Cache directory | `$XDG_CACHE_HOME/rozi`, else `~/.cache/rozi` | same | `%LOCALAPPDATA%\rozi\cache` |
| Runtime endpoints | `$XDG_RUNTIME_DIR/rozi`, else a private per-uid temp directory | private directory under `$TMPDIR` | `%LOCALAPPDATA%\rozi\run` |
| Shell integration | bash, zsh, fish | bash, zsh, fish | PowerShell (full), cmd.exe (prompt markers only) |
| Foreground-program detection | shell metadata, then `/proc` | shell metadata, then `libproc` | shell metadata only |

Endpoints are private to the user who created them: mode `0700`/`0600` on Unix, and a protected
current-user-SID DACL plus `PIPE_REJECT_REMOTE_CLIENTS` on Windows. Every connection additionally
completes an authenticated protocol handshake, so discovery entries are hints, never trust.

**Windows needs Windows 10 version 1809 (build 17763) or newer** — the build that introduced
ConPTY. rozi checks for it at startup and refuses with an explanation rather than failing on
every pane. Any console host from that build onwards has the VT support rozi renders through;
Windows Terminal is recommended but not required. Windows deliberately has **no process
inspection**: rozi never probes a PEB or walks a process tree, so a pane's working directory and
foreground program come from shell integration or not at all (see
[Terminal](terminal.md#smart-focus-and-cwd-inheritance)).

## Build and run

```bash
cargo build       # compile
cargo run         # launch the app
```

When you start `rozi` it opens with a single shell pane in workspace 1. Spawn more panes,
switch workspaces, and lay them out as described in [Keybindings](keybindings.md) and
[Layouts & panes](layouts-and-panes.md).

## Quitting

- **`prefix d`**, **`prefix q`**, and **`Alt+q`** (defaults) all leave the client. Named sessions
  detach and keep running. An untouched temporary session closes silently; a temporary session you
  worked in asks whether to name and keep it or close it.
- Closing the **last pane in a workspace** leaves an empty workspace panel; the app stays running.
  Use either leave binding to exit explicitly.

## Developer commands

```bash
cargo test        # run the test suite
cargo clippy      # lint
cargo fmt         # format (use rustfmt --edition 2024 if running rustfmt directly)
```

`cargo run` needs an interactive terminal; leave with **`prefix d`** (detach) or **`prefix q`** /
**`Alt+q`** (quit). For details on the module
layout and the layout/animation/input internals, see [AGENTS.md](../AGENTS.md).

`rozi --help` lists every command and option; `rozi --help --advanced` adds internals a
normal launch never needs (`--server`, endpoint resolution, `--socket`, `--skill`). It is
colored when stdout is a terminal, and follows the usual opt-outs: `NO_COLOR` (any non-empty
value) turns styling off, `CLICOLOR=0` does the same, and `CLICOLOR_FORCE` keeps it on through
a pipe. `TERM=dumb` is treated as plain. Install the built-in agent skill with
`rozi skill install` or `rozi skill install --global`; see [Agent skill](agent-skill.md).

## First-run configuration

`rozi` runs with sensible defaults and no config file. To customize the shell, keybinding
modifier, prefix, animations, theme, or to select a default launch profile, create a config file at
`~/.config/rozi/config.toml` (or point `$ROZI_CONFIG` at one). See
[Configuration](configuration.md) for the full reference.

On startup, `rozi` raises a toast for any problem reading the config file, theme file, or
launch profile, so a broken config never silently pretends to have loaded. A clean start is
quiet.

## Read-only sessions

Open a persistent session with `rozi dev` (or `rozi --session dev`). It attaches when running
or launches canonical profile `dev`; it errors if neither exists. Create one explicitly with
`rozi new dev`, or use `rozi new review --profile dev` to launch recipe `dev` under an
independent session name. Unknown targets never silently create sessions.

Use `rozi attach dev [--read-only]` when attachment must not launch anything. `--read-only`
attaches as a viewer without terminal input or layout-control authority and requires the target to
already be running. The words `attach` and `new` are reserved as subcommands; use `--session attach`
or `--session new` to address those names through positional target resolution.
