# Getting started

## Requirements

- A recent Rust toolchain (edition 2024; `rust-version = 1.88`).
- A real terminal emulator (the app drives a full-screen TUI and spawns PTYs).
- A sibling checkout of [`tui-lipan`](../../tui-lipan).

Linux and macOS use Unix-domain sockets for local control and named-session IPC. Windows named-pipe
support is planned but not yet available; do not rely on Windows-specific configuration defaults
until that work ships.

> **Dependency note.** `tui-lipan` is a **path dependency** declared in `Cargo.toml` as
> `{ path = "../tui-lipan", features = ["terminal", "theme-reload"] }`. The sibling
> `../tui-lipan` checkout must exist for `hyprmux` to build. The `terminal` feature pulls
> in `portable-pty` + `alacritty_terminal` for the PTY-backed terminal; `theme-reload`
> enables live theme hot-reload. This repo is **not** self-contained for a standalone clone.

## Build and run

```bash
cargo build       # compile
cargo run         # launch the app
```

When you start `hyprmux` it opens with a single shell pane in workspace 1. Spawn more panes,
switch workspaces, and lay them out as described in [Keybindings](keybindings.md) and
[Layouts & panes](layouts-and-panes.md).

## Quitting

- **`prefix d`** (default) **detaches**: it leaves the TUI back to your shell (tmux-style) while the
  session server keeps running for later reattach. Detaching an *anonymous* ephemeral session first
  prompts you to name it (confirm to detach durably under that name; cancel to quit and shut the
  ephemeral server down). An already-named session detaches immediately.
- **`prefix q`** / **`Alt+q`** (default) **quit**: exits the client. Quitting shuts down the current
  server only when it is an ephemeral session; named servers keep running.
- Closing the **last pane in a workspace** leaves an empty workspace panel; the app stays running.
  Use detach or quit to leave explicitly.

## Developer commands

```bash
cargo test        # run the test suite
cargo clippy      # lint
cargo fmt         # format (use rustfmt --edition 2024 if running rustfmt directly)
```

`cargo run` needs an interactive terminal; leave with **`prefix d`** (detach) or **`prefix q`** /
**`Alt+q`** (quit). For details on the module
layout and the layout/animation/input internals, see [AGENTS.md](../AGENTS.md).

## First-run configuration

`hyprmux` runs with sensible defaults and no config file. To customize the shell, keybinding
modifier, prefix, animations, theme, or to enable a project profile, create a config file at
`~/.config/hyprmux/hyprmux.toml` (or point `$HYPRMUX_CONFIG` at one). See
[Configuration](configuration.md) for the full reference.

On startup, `hyprmux` shows toast notifications reporting whether the config file, theme file,
and project profile were loaded - and any parse/read warnings - so a broken config never
silently pretends to have loaded.
## Read-only sessions

Attach to a persistent session with `hyprmux --attach dev` (or `hyprmux --session dev`). Add
`--read-only` to attach as a viewer without terminal input or layout-control authority.
