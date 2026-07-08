# Project Overview

`hyprmux` is a Hyprland-style tiling terminal multiplexer built on `tui-lipan`.
It runs real PTY-backed panes inside a TUI window manager with dwindle/master/grid/monocle
layouts, floating panes, workspaces, scrollback tools, command palettes, profiles, and optional
server-backed named sessions for detach/reattach workflows.

## Repository Structure

- `.agents/` - Local agent skills and references used by this workspace.
- `.claude/` - Local Claude/agent helper material; preserve unless explicitly asked.
- `.github/` - GitHub metadata such as funding configuration.
- `.superpowers/` - Historical planning and execution notes; do not treat as live product docs.
- `docs/` - User-facing documentation and internal design notes.
- `examples/` - Example profiles and runnable/user-copyable configuration snippets.
- `src/` - Rust application source, grouped by runtime, layout, panes, input, and features.
- `tests/` - Integration and smoke tests outside the main binary crate.
- `target/` - Cargo build output; generated and ignored.
- `Cargo.toml` / `Cargo.lock` - Rust package manifest and locked dependency graph.
- `README.md` - Public project overview and documentation index.
- `CLAUDE.md` - Legacy agent guide copy; update only when intentionally keeping it in sync.
- `AGENTS.md` - This file: operational guidance for automated coding agents.
- `LICENSE-APACHE` / `LICENSE-MIT` - Dual-license terms.

## Build & Development Commands

Install/fetch dependencies:

```bash
cargo fetch
```

Build:

```bash
cargo build
```

Run:

```bash
cargo run
```

Run a named profile:

```bash
cargo run -- dev
cargo run -- --profile dev
```

Attach to a named session:

```bash
cargo run -- --attach dev
cargo run -- --session dev
```

Run a session server directly:

```bash
cargo run -- --session dev --server
```

Leave the TUI:

```text
prefix d
```

Test:

```bash
cargo test
```

Run one test by substring:

```bash
cargo test spawn_split_direction_follows_focused_tile_aspect
```

Lint:

```bash
cargo clippy
```

Type-check:

```bash
cargo check
```

Format:

```bash
cargo fmt
```

Format manually with Rust edition:

```bash
rustfmt --edition 2024 <file>
```

Release build / deploy artifact:

```bash
cargo build --release
```

Debug locally:

```bash
cargo run
```

> TODO: Add documented debugger/profiler commands if a standard project workflow is chosen.

## Code Style & Conventions

- Rust edition is 2024; minimum supported Rust version is `1.85`.
- Use `cargo fmt`; avoid hand-formatting style debates.
- Use `cargo clippy` for linting before commits that touch Rust code.
- Prefer small, direct changes over compatibility shims; this project is pre-1.0 with no external
  compatibility obligations.
- Comments describe current invariants and non-obvious reasons, never changelog/history.
- Feature reaction modules use `*_ops` names, such as `focus_ops`, `theme_ops`, and `exit_ops`.
- Lifecycle/event/data modules use plain names, such as `pane_lifecycle`, `pty_events`, and
  `profiles`.
- Keep `input.rs` as the source of truth for command/action metadata used by help and palettes.
- Keep split direction based on the focused tile aspect ratio and `SPLIT_WIDTH_MULTIPLIER`.
- Keep geometry animations app-driven; position/opacity may animate, but terminal size changes
  should snap to avoid repeated `pty.resize` / SIGWINCH reflow.
- Commit-message style in the repo is concise conventional prefixes, for example
  `fix: improve toast confirmation behavior` or `feat: live reload config changes`.

## Architecture Notes

```text
CLI/main
  |
  v
HyprmuxApp (tui-lipan Component)
  |-- State / Msg model in state.rs + main.rs
  |-- update::handle_msg dispatches messages to *_ops modules
  |-- key_routing routes prefix/held-modifier/terminal keys
  |-- actions dispatches Action values
  |-- view renders Canvas, panes, top bar, and overlays
  |
  +--> Local mode: Pane -> TerminalPane -> TerminalScreen + TerminalPty
  |
  +--> Attached mode: session/client <-> session/server <-> server-owned PTYs
```

`hyprmux` is an Elm-style app with one root `Component` (`HyprmuxApp`), a central `State`, and
`Msg` updates. `tui-lipan` supplies runtime primitives such as `Canvas`, `Frame`, transitions,
mouse regions, overlays, and terminal widgets. The app owns window-manager policy: tiling trees,
floating geometry, focus, input routing, profiles, sessions, and terminal palette synchronization.

Local mode keeps PTYs in the UI process. Explicit `--attach` / `--session` mode connects to a
named session server; detach leaves server PTYs running for later reattach. Profiles and local
session autosave restore layout and launch intent only, while attached sessions preserve live PTY
state.

Important data flow:

1. Keys arrive via `Component::on_key` or focused terminal callbacks.
2. `key_routing.rs` decides whether to run an app `Action` or forward to the PTY.
3. `actions.rs` dispatches app actions to feature modules.
4. `update.rs` handles async/runtime messages, PTY events, session snapshots, and palette changes.
5. `view/` renders current state into a `Canvas` and overlay stack.

Major module map:

- `main.rs` - App shell, CLI parsing, startup, transition policies, and runtime wiring.
- `update.rs` - Flat message router and post-update synchronization.
- `state.rs` - Runtime model, pane/workspace/session/profile state, and constants.
- `actions.rs` - Action dispatcher, including palette-specific confirmation bypass.
- `key_routing.rs` / `keymap.rs` / `input.rs` - Input modes, bindings, actions, and command ids.
- `pane.rs` / `pane_lifecycle.rs` / `pty_events.rs` - Terminal screen, PTY, spawn, resize, exit.
- `tiling.rs` / `layout.rs` / `geometry.rs` / `resize_move_ops.rs` / `anim.rs` - Window-manager
  layout, placement, movement, resizing, and animations.
- `session/` / `session_ops.rs` - Named session protocol, server/client, discovery, attach/kill.
- `profiles.rs` / `profile_ops.rs` - Named profile serialization, restore, picker, default profile.
- `config.rs` / `config_ops.rs` / `theme_ops.rs` - Config loading/reload, themes, terminal colors.
- `view/` - Pane rendering, top bar, palettes, overlays, and callbacks.

## Testing Strategy

- Unit tests live mostly inside `src/*.rs` modules and run with `cargo test`.
- Integration/smoke tests live under `tests/`, for example `tests/border_merge_smoke.rs`.
- Prefer targeted tests for layout, geometry, key routing, profile restore, session protocol, and
  terminal behavior when changing those areas.
- For app-only Rust changes, run at least:

```bash
cargo test
cargo clippy
git diff --check
```

- For broader feature work, also run:

```bash
cargo build
```

- For framework terminal changes in `../tui-lipan`, verify both sides:

```bash
cargo check --features terminal
cargo clippy --features terminal
```

Then rerun the relevant `hyprmux` tests and lints.

> TODO: Document CI provider/check names if CI is added beyond local Cargo commands.

## Security & Compliance

- Do not commit secrets, local socket paths, personal config, generated logs, or terminal captures
  that may contain credentials.
- Runtime config comes from `$HYPRMUX_CONFIG` or `~/.config/hyprmux/hyprmux.toml`; treat user config
  as local data, not repository state.
- Control sockets are per-run Unix sockets; preserve runtime-dir safety checks and
  symlink/permission validation when editing control discovery.
- Named session sockets are scoped to hyprmux session names; keep name validation and stale-socket
  handling defensive.
- Clipboard and OSC52 behavior can expose copied data; keep `[clipboard].enable_osc52` controls
  intact.
- Dependency scanning is not configured in this repo.
- License is `MIT OR Apache-2.0`; preserve dual-license headers/files.

> TODO: Add dependency-audit command if the project standardizes on `cargo audit` or similar.

## Agent Guardrails

- Do not edit `target/` or generated build artifacts.
- Do not modify `../tui-lipan` unless the bug is in the framework or the user explicitly asks.
- If editing `../tui-lipan`, inspect its git status separately and stage only intended files.
- Do not sweep unrelated local files, `.superpowers/` reports, or `.agents/` skills into commits.
- Preserve unrelated worktree changes; never revert user work without explicit approval.
- Do not amend commits, force-push, or run destructive git commands unless explicitly requested.
- Before committing, inspect `git status --short`, `git diff --stat`, and `git log --oneline -10`.
- Stage paths explicitly; this workspace may be dirty.
- Run `git diff --check` before a commit.
- Keep docs synchronized when changing user-visible behavior, CLI flags, config keys, or workflows.
- Avoid backwards-compatibility shims unless persisted data, shipped behavior, or a user request
  requires them.

## Extensibility Hooks

- `HYPRMUX_CONFIG` selects an alternate config file.
- `HYPRMUX_SOCKET` points CLI control commands at a live UI control socket.
- `HYPRMUX=1`, `HYPRMUX_PANE`, and `HYPRMUX_SOCKET` are injected into spawned panes.
- `[keys]` can rebind built-in actions or define user commands with `run` / `send` tables.
- `[bar]` supports built-in segments, text placeholders, and timed shell command segments.
- `[theme].name` selects built-in, `system`, or custom themes from `~/.config/hyprmux/themes/`.
- `[profile] default` selects a startup profile from `~/.config/hyprmux/profiles/`.
- `[session] autosave` enables local layout autosave/restore.
- `--attach <NAME>` / `--session <NAME>` connects to persistent named session servers.
- Cargo feature flags are inherited from the path dependency `tui-lipan`; this crate currently uses
  `terminal`, `terminal-serde`, and `theme-reload`.

## Further Reading

- [README.md](README.md) - Project overview and documentation index.
- [docs/index.md](docs/index.md) - Full documentation table of contents.
- [docs/getting-started.md](docs/getting-started.md) - Build, run, quit, and dependency notes.
- [docs/configuration.md](docs/configuration.md) - Complete `hyprmux.toml` reference.
- [docs/keybindings.md](docs/keybindings.md) - Prefix mode, held modifier, mouse, and key table.
- [docs/layouts-and-panes.md](docs/layouts-and-panes.md) - Layouts, focus, movement, and animation.
- [docs/terminal.md](docs/terminal.md) - PTY, clipboard, selection, scrollback, and persistence.
- [docs/profiles.md](docs/profiles.md) - Named profiles and profile picker.
- [docs/project-profiles.md](docs/project-profiles.md) - Profile format and pane identity.
- [docs/sessions.md](docs/sessions.md) - Local vs attached sessions and detach/reattach semantics.
- [docs/control.md](docs/control.md) - Control socket CLI and JSON protocol.
- [docs/themes.md](docs/themes.md) - Themes, hot reload, and terminal color palette.
