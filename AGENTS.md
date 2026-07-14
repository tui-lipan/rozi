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

Audit dependencies for known security vulnerabilities:

```bash
cargo install cargo-audit --locked
cargo audit
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

- Rust edition is 2024; minimum supported Rust version is `1.88` (what CI builds on).
- Targets Linux, macOS, and Windows natively. Reach OS-specific behavior through `src/platform/`,
  never `std::os::unix` / `/proc` / Win32 directly from a feature module.
- Use `cargo fmt`; avoid hand-formatting style debates.
- Use `cargo clippy` for linting before commits that touch Rust code.
- Prefer small, direct changes over compatibility shims; this project is pre-1.0 with no external
  compatibility obligations.
- Comments describe current invariants and non-obvious reasons, never changelog/history.
- Feature reaction modules live under `ops/`, such as `ops/focus.rs`, `ops/theme.rs`, and
  `ops/exit.rs`.
- Lifecycle/event/data modules use plain names, such as `pane_lifecycle`, `pty_events`, and
  `profiles`.
- Keep `input.rs` as the source of truth for command/action metadata used by help and palettes.
- Keep split direction based on the focused tile aspect ratio and the configured
  `layout.split_width_multiplier`.
- Keep geometry animations app-driven; position/opacity may animate, but terminal size changes
  should snap to avoid repeated `pty.resize` / SIGWINCH reflow.
- Commit-message style in the repo is concise conventional prefixes, for example
  `fix: improve toast confirmation behavior` or `feat: live reload config changes`.

## Architecture Notes

```text
CLI / thin main.rs
  |
  v
lib.rs -> app.rs: HyprmuxApp (tui-lipan Component)
  |-- State / Msg model in state/ + msg.rs
  |-- update/::handle_msg dispatches messages to focused update/ops modules
  |-- key_routing routes prefix/held-modifier/terminal keys
  |-- actions dispatches Action values
  |-- view renders Canvas, panes, workbar, and overlays
  |
  +--> Always-server: Pane -> TerminalPane -> client TerminalScreen (parses raw bytes)
  |
  +--> session/client <-> session/server <-> server-owned PTYs
```

`hyprmux` is an Elm-style app with one root `Component` (`HyprmuxApp`), a central `State`, and
`Msg` updates. `tui-lipan` supplies runtime primitives such as `Canvas`, `Frame`, transitions,
mouse regions, overlays, and terminal widgets. The app owns window-manager policy: tiling trees,
floating geometry, focus, input routing, profiles, sessions, and terminal palette synchronization.

`hyprmux` is always-server: the session server owns every PTY and the client always attaches,
parsing raw pane output into its own `TerminalScreen`. A bare launch attaches to a disposable
ephemeral session (`eph-<pid>`); `--attach` / `--session` connects to a persistent named session.
Detach leaves the server running for later reattach; a clean quit shuts an ephemeral server down.
Profiles restore layout and launch intent only, while a live session preserves PTY state.

The server is multi-client and layout-authoritative: several clients can attach to one session and
share a revisioned `SharedLayout` (`src/shared_layout.rs`, wire protocol v8). One client holds the
layout-control lease (the *controller*) and commits layout changes; the rest are *followers* that
reconcile via `apply_shared_layout` without touching live screens, letterbox to the controller's
canonical PTY size, and take control instantly with `take-control` (`prefix g`). Local view state
(focus, active workspace, overlays, copy/search, scrollback, theme) is never shared.

Important data flow:

1. Keys arrive via `Component::on_key` or focused terminal callbacks.
2. `key_routing.rs` decides whether to run an app `Action` or forward input to the session server.
3. `actions.rs` dispatches app actions to feature modules.
4. `update/mod.rs` exhaustively routes messages to focused handlers for overlays, prompts, panes,
   attach setup, and session frames, then performs post-update synchronization.
5. `view/` renders current state into a `Canvas` and overlay stack.

Major module map:

- `main.rs` / `lib.rs` - Thin binary entry point and shared library module/re-export surface.
- `app.rs` / `msg.rs` / `cli.rs` - Root component, message model, command-line parsing, transition
  policies, startup orchestration, and runtime wiring.
- `update/` - Exhaustive message router and post-update synchronization in `mod.rs`, with focused
  overlay, prompt, pane, attach, and session handlers.
- `state/` - Central runtime `State` plus focused layout, appearance, drag, mode, search, identity,
  picker, pane, workspace, and shared-session state modules.
- `actions.rs` - Action dispatcher, including palette-specific confirmation bypass.
- `key_routing.rs` / `keymap.rs` / `input.rs` - Input modes, bindings, actions, and command ids.
- `pane.rs` / `pane_lifecycle.rs` / `pty_events.rs` - Terminal screen, PTY, spawn, resize, exit.
- `tiling.rs` / `layout.rs` / `geometry.rs` / `ops/resize_move.rs` / `anim.rs` - Window-manager
  layout, placement, movement, resizing, and animations.
- `session/` / `ops/session.rs` - Multi-client session protocol (v8), server/client, discovery,
  bootstrap, attach/kill, and layout-control lease.
- `layout_tree_ser.rs` - Serde-stable tree shared by profile TOML and session layout documents.
- `shared_layout.rs` - Server-authoritative shared layout document, conversions, and the follower
  reconciler (`apply_shared_layout`).
- `profiles.rs` / `ops/profile.rs` - Named profile serialization, restore, picker, default profile.
- `config/` / `ops/config.rs` / `ops/theme.rs` - Config loading/reload, themes, terminal colors.
- `platform/` - Cross-platform abstraction layer. Nothing above it references `std::os::unix`,
  `/proc`, `SO_PEERCRED`, XDG/AppData env vars, Unix permission bits, named-pipe APIs, or Unix
  signals directly. Submodules: `paths` (config/state/cache/runtime directories, reported-cwd
  normalization), `fs_security` (private directories; Unix mode/ownership, Windows SID DACL),
  `user` (uid/SID, `USER` vs `USERNAME`, hostname), `command` (shell and command-runner resolution,
  Windows `PATH`/`PATHEXT` lookup), `ipc/*` (Unix sockets / Windows named pipes behind one
  `IpcEndpoint`/`IpcListener`/`IpcConnection` API), `server_lifecycle` (detached spawn, hangup and
  console-control handling, protocol-first shutdown, Job Object containment, forced termination,
  ConPTY availability), `shell_integration` (per-shell injection), `process/*` (`ProcessInspector`;
  Linux/macOS only by design), and `notifications`. See `platform/mod.rs` for per-submodule status.
- `view/` - Pane rendering, workbar, palettes, overlays, and callbacks.

## Testing Strategy

- Unit tests live mostly inside `src/*.rs` modules and run with `cargo test`.
- Integration/smoke tests live under `tests/`, for example `tests/border_merge_smoke.rs`.
- Session integration tests use `tests/common` and the real typed protocol/platform IPC helpers;
  never reimplement session framing or use raw Unix sockets in cross-platform tests.
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

- To develop against a local `tui-lipan` (framework and app changing together), add a gitignored
  `.cargo/config.toml` overriding the crates.io dependency with the sibling checkout:

```toml
[patch.crates-io]
tui-lipan = { path = "../tui-lipan" }
```

  The sibling's declared version must satisfy `Cargo.toml`'s requirement.

  **A `Cargo.lock` generated with this patch active is not valid for CI.** The patched crate is
  recorded as a path package with no `source`/`checksum`, so `cargo check --locked` on a plain
  checkout rejects it. Once the framework version this depends on is published, regenerate the lock
  with the patch out of the way and commit that:

```bash
mv .cargo/config.toml .cargo/config.toml.off && cargo generate-lockfile
mv .cargo/config.toml.off .cargo/config.toml
```

- For framework terminal changes in `../tui-lipan`, verify both sides:

```bash
cargo check --features terminal
cargo clippy --features terminal
```

  Then rerun the relevant `hyprmux` tests and lints, and publish the framework before the hyprmux
  change that needs it can go green in CI.

CI (`.github/workflows/ci.yml`) runs `fmt --check`, `check`, `clippy -D warnings`, `test`, and a
release build natively on `ubuntu-latest`, `macos-latest`, and `windows-latest`, plus `cargo audit`
in a separate Linux job. It builds from a plain checkout: `tui-lipan` resolves from crates.io at the
version pinned in `Cargo.toml`, so a framework bump is an explicit `Cargo.toml`/`Cargo.lock` commit
here rather than a silent drift.

Windows code cannot be run in this workspace. Type-check it before pushing - CI is the first thing
that actually executes it:

```bash
cargo check --target x86_64-pc-windows-gnu --all-targets
cargo clippy --target x86_64-pc-windows-gnu --all-targets
```

`.github/workflows/release.yml` builds Linux x86_64/arm64, macOS x86_64/arm64, and Windows x86_64
archives on a `v*` tag, with checksums and extracted-binary smoke tests.

## Security & Compliance

- Do not commit secrets, local socket paths, personal config, generated logs, or terminal captures
  that may contain credentials.
- Runtime config comes from `$HYPRMUX_CONFIG` or `~/.config/hyprmux/hyprmux.toml`; treat user config
  as local data, not repository state.
- Control and session endpoints are per-user and per-run: Unix sockets on Linux/macOS, named pipes
  on Windows. Preserve the runtime-dir safety checks (Unix ownership/mode/symlink validation;
  Windows reparse-point rejection and the protected current-user-SID DACL) when editing endpoint
  discovery, and keep `PIPE_REJECT_REMOTE_CLIENTS` and `FILE_FLAG_FIRST_PIPE_INSTANCE` on the
  Windows backend - they are what stop remote reachability and name squatting respectively.
- Windows discovery entries under the runtime directory are hints only. Never read a pipe name out
  of one; derive it. Every endpoint must still complete the authenticated protocol handshake.
- Named session endpoints are scoped to hyprmux session names; keep name validation and
  stale-endpoint handling defensive.
- Shell integrations must emit only an executable basename, never a command line, and must never
  modify a dotfile, a `$PROFILE`, or the `AutoRun` registry key.
- Clipboard and OSC52 behavior can expose copied data; keep `[clipboard].enable_osc52` controls
  intact.
- Dependency auditing uses RustSec via `cargo audit`; run it after dependency updates and before
  release builds.
- License is `MIT OR Apache-2.0`; preserve dual-license headers/files.

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
- `[[rules]]` applies first-match command substring placement to control and `[keys] run` spawns.
- `[workbar]` supports built-in segments, text placeholders, and timed shell command segments; each
 segment renders as a themed badge whose color can be overridden by theme role via a segment table,
 and `[pane].workbar_powerline` toggles trailing-badge chaining.
- `[theme].name` selects built-in, `system`, or custom themes from `~/.config/hyprmux/themes/`.
- `[profile] default` selects a startup profile from `~/.config/hyprmux/profiles/`.
- `[session] autosave` enables local layout autosave/restore.
- `[session] resurrect` snapshots named sessions so layout, commands, and scrollback survive a server restart.
- `--attach <NAME>` / `--session <NAME>` connects to persistent named session servers.
- Cargo feature flags are inherited from the `tui-lipan` dependency (crates.io); this crate
  currently uses `terminal`, `terminal-serde`, and `theme-reload`.

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
