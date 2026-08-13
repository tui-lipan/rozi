# Project Overview

`rozi` is a Hyprland-style tiling terminal multiplexer built on `tui-lipan`.
It runs real PTY-backed panes inside a TUI window manager with dwindle/master/grid/columns/rows/
scrollable/monocle layouts, floating panes, workspaces, scrollback tools, command palettes, profiles, and optional
server-backed named sessions for detach/reattach workflows.

## Repository Structure

- `.agents/` - Local agent skills and references used by this workspace.
- `.claude/` - Local Claude/agent helper material; preserve unless explicitly asked.
- `.github/` - GitHub metadata such as funding configuration.
- `.superpowers/` - Historical planning and execution notes; do not treat as live product docs.
- `benches/` - Criterion benchmarks and deterministic generated terminal/protocol corpora.
- `docs/` - User-facing documentation and internal design notes. This folder is also the
  VitePress source for `rozi.tui-lipan.dev`: every `.md` here is a page on the site, and
  `docs/.vitepress/` holds the config, theme, and landing page. Adding a doc page means adding it
  to the sidebar in `docs/.vitepress/config.ts`. Links that leave `docs/` are rewritten to GitHub
  URLs at build time, so keep writing them the way GitHub wants. See `docs/.vitepress/README.md`.
- `examples/` - Example profiles and runnable/user-copyable configuration snippets.
- `src/` - Rust application source, grouped by runtime, layout, panes, input, and features.
- `tests/` - Integration and smoke tests outside the main binary crate.
- `target/` - Cargo build output; generated and ignored.
- `Cargo.toml` / `Cargo.lock` - Rust package manifest and locked dependency graph.
- `README.md` - Public project overview and documentation index.
- `CLAUDE.md` - A symlink to `AGENTS.md`, not a copy. Edit `AGENTS.md`; there is nothing to sync.
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

Resolve a named session or its canonical same-name profile:

```bash
cargo run -- dev
```

Attach to an already-running named session:

```bash
rozi attach dev
cargo run -- attach dev
```

Equivalent target spelling:

```bash
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

Benchmark:

```bash
cargo bench
cargo bench --bench terminal_ingest -- 'sgr_heavy/200x60'
```

Profile an optimized build with debug symbols:

```bash
cargo build --profile release-debug
samply record ./target/release-debug/rozi profile
```

See `docs/benchmarks.md` for targets, Criterion 0.8 baselines, stress recipes, and hot-path notes.

## Code Style & Conventions

- Rust edition is 2024; minimum supported Rust version is `1.88` (what CI builds on).
- Targets Linux, macOS, and Windows natively. Reach OS-specific behavior through `src/platform/`,
  never `std::os::unix` / `/proc` / Win32 directly from a feature module.
- Use `cargo fmt`; avoid hand-formatting style debates.
- Use `cargo clippy` for linting before commits that touch Rust code.
- Rozi is in active development with no users or external compatibility obligations. Prefer a
  clean breaking change when it improves the design; do not add migrations, deprecated aliases, or
  compatibility shims unless the user explicitly requests them or an internal protocol test
  intentionally covers version skew.
- Comments describe current invariants and non-obvious reasons, never changelog/history.
- Feature reaction modules live under `ops/`, such as `ops/focus.rs`, `ops/theme.rs`, and
  `ops/exit.rs`.
- Lifecycle/event/data modules use plain names, such as `pane_lifecycle`, `pty_events`, and
  `profiles`.
- Keep `input.rs` as the source of truth for the `Action` enum, `Action::id()` command ids, and
  `BINDABLE_ACTIONS`; keep `commands.rs` as the source of truth for the `BUILTIN_COMMANDS` registry
  entries that carry each command's label, description, group, and `default_keys`. Help and the
  palettes render from that registry, so adding an action means touching both files.
- Adding a config key means three edits: the serde model in `config/file.rs`, a row in
  `docs/configuration.md`, and a line commented at its default in `examples/config.toml`. That
  example is the copyable form of the reference - it ships inert, and a test uncomments the whole
  file and loads it, so a key renamed out from under it fails `cargo test` rather than rotting.
  Prose there is commented `# like this`, settings `#key = value`, which is what lets the test tell
  them apart. Everything in `examples/` is linked from docs and read by a test; add to the
  reference rather than leaving a new orphan snippet beside it.
- Do not toast successful state changes that are already visible on screen. Lossless config
  normalization is also silent; reserve toasts for failures, rejections, destructive confirmations,
  and useful off-screen results. See `docs/configuration.md#in-app-toasts`.
- Overlays and modals present **structured data, not prose**. A dialog is a list of rows, badges,
  and chrome labels; it is not a place for explanatory sentences. Concretely:
  - No sentence-shaped body lines (`You: razuer #2077 · controller`, `Sides differ; applying writes
    symmetric padding.`). Put the same fact in a row, a right-aligned `ItemDescription`, a footer
    hint, or a `Frame` header label (`header_right` carries per-dialog context well).
  - Prefer compact tokens over spelled-out status: `ctrl` / `follow` / `ro`, not
    `you are currently the controller`.
  - Keep labels terse and parallel; let position and alignment carry meaning instead of words.
  - Explanation belongs in `docs/`, not on screen.
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
lib.rs -> app.rs: AppRoot (tui-lipan Component)
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

`rozi` is an Elm-style app with one root `Component` (`AppRoot`), a central `State`, and
`Msg` updates. `tui-lipan` supplies runtime primitives such as `Canvas`, `Frame`, transitions,
mouse regions, overlays, and terminal widgets. The app owns window-manager policy: tiling trees,
floating geometry, focus, input routing, profiles, sessions, and terminal palette synchronization.

`rozi` is always-server: the session server owns every PTY and the client always attaches,
parsing raw pane output into its own `TerminalScreen`. A bare launch follows `[session] startup`,
which by default picks a session without attaching anything and falls back to a disposable
ephemeral session (`eph-<pid>`); a positional target / `--session` attaches to a named session or
launches its canonical same-name profile, while `new` creates a session explicitly.
Leaving through either `detach` or `quit` preserves named servers, closes untouched ephemeral
sessions, and asks whether to keep ephemeral sessions that contain work.
Profiles restore layout and launch intent only, while a live session preserves PTY state.

The server is multi-client and layout-authoritative: several clients can attach to one session and
share a revisioned `SharedLayout` (`src/shared_layout.rs`, wire protocol negotiated in a supported
range; this build speaks 1 only). One client holds the
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
- `key_routing.rs` / `input.rs` / `commands.rs` / `config/input.rs` - Input mode routing, the
  `Action` enum and command ids, the `BUILTIN_COMMANDS` registry and its default chords, and
  `[keys]` override/user-command parsing.
- `pane.rs` / `pane_lifecycle.rs` / `pty_events.rs` - Terminal screen, PTY, spawn, resize, exit.
- `tiling.rs` / `layout.rs` / `geometry.rs` / `ops/resize_move/` / `anim.rs` - Window-manager
  layout, placement, floating and tiled movement, split dragging, keyboard resizing, and animations.
- `session/` / `ops/session.rs` - Multi-client session protocol (negotiated version range),
  server/client, discovery, bootstrap, attach/kill, layout-control lease, and `--remote` SSH proxy.
- `layout_tree_ser.rs` - Serde-stable tree shared by profile TOML and session layout documents.
- `shared_layout.rs` - Server-authoritative shared layout document, conversions, and the follower
  reconciler (`apply_shared_layout`).
- `profiles.rs` / `ops/profile.rs` - Named profile serialization, restore, picker, default profile.
- `config/` / `ops/config.rs` / `ops/theme.rs` - Serde file models and load orchestration in
  `config/file.rs`, with rules, input, workbar, appearance, persistence, schema, and theme helpers
  in flat sibling modules; runtime reload and terminal-color reactions live under `ops/`.
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
- `view/` - Pane rendering, workbar, palettes, overlays, the pending-chord which-key strip, and
  callbacks.
- `benches/` - Criterion targets for terminal ingest, snapshot rebuilding, protocol framing, and
  the end-to-end session pipeline; `benches/support/mod.rs` generates deterministic corpora.

### Overlays and pickers - use the shared shells

Every overlay in `view/overlays/` is one of three shapes, and each already has a helper. Filtering,
fuzzy matching, keyboard navigation, scrolling, hover, and headers are `tui-lipan`'s
`SearchPalette` - do not hand-roll any of it, and do not restyle a palette per call site.

- **Searchable picker** (commands, appearance, themes, layouts, profiles, sessions, collaborators):
  `view::shared_search_palette::<T>(ctx, height, highlight_matches)` returns a fully styled
  `SearchPalette<T>`; add `.entries(...)`, `.on_select`, `.on_activate` and nothing cosmetic. Wrap it
  in `view::action_palette_modal(ctx, title)` (or `..._with_width`) plus `action_palette_frame`.
  Grouped rows come from `view::search_entries_with_groups([(category, vec![...])])`, which just
  prefixes each bucket with a `SearchEntry::header`.
- **Text prompt** (rename pane / workspace / session, save profile): `overlays::prompts`'s private
  `prompt_overlay` already does input binding, hints, Esc/Enter wiring, and the armed-confirm
  recolor. Add a thin wrapper beside `rename_overlay`; do not build a `Modal` + `Input` by hand.
- **Plain modal** (help): `view::styled_modal(ctx, title, width)`.

Wiring a new picker is mostly bookkeeping, not rendering:

1. `state/` - a `bool` + `Option<…Selected>` pair beside the appearance ones in `state/mod.rs`.
   There is no unified `PickerKind`; each overlay owns its pair.
2. `msg.rs` - `…Select` / `…Activate`, routed in `update/mod.rs`.
3. `update/overlays.rs` - the activate arm. **Settings overlays stay open**: re-assert `show_* =
   true`, restore the highlight, re-grab focus (see the appearance arm). A row greyed by
   `disabled_reason` must be inert - return without acting.
4. `input.rs` + `commands.rs` - one `Action` to open it, in both files (see the `Action` rule above).
5. `ops/overlay_return.rs` - only if the overlay can be raised *from* another one. `restore` reopens
   the parent with its query and highlighted row intact; `finish` closes to the parent or the pane.

Row conventions: right-aligned status text via `ItemDescription`; the words `"Enabled"`/`"Disabled"`
for a boolean (`enabled_status` in `overlays/appearance.rs` is private - copy the three lines rather
than exporting it); greying through a `disabled_reason` returning `Some("Needs …")`. **There are no
checkbox glyphs anywhere in this codebase - do not introduce any.** See also the structured-data rule
under Code Style.

## Testing Strategy

- Unit tests live mostly inside `src/*.rs` modules and run with `cargo test`.
- Integration/smoke tests live under `tests/`, for example `tests/border_merge_smoke.rs`.
- Session integration tests use `tests/common` and the real typed protocol/platform IPC helpers;
  never reimplement session framing or use raw Unix sockets in cross-platform tests.
- Tests never write to the developer's own config, state, or cache directories. Ordinary actions
  persist as a side effect (`[sidebar]` preferences, `session.toml` autosave, shell-integration
  scripts), and a running rozi live-reloads its config file, so an unisolated test lands in the
  UI the developer is working in. Unit tests are isolated automatically - `PlatformEnv::from_process`
  resolves into a per-process scratch root under `cfg(test)`. An integration test that builds a
  `AppRoot` must call `rozi::test_support::isolate_user_dirs()` first, from whatever helper
  constructs its `TestBackend`. Never mutate `HOME`/`XDG_*`/`APPDATA`/`ROZI_CONFIG` to redirect a
  test: `std::env::set_var` is unsound beside parallel tests, and isolation now outranks it anyway.
- Prefer targeted tests for layout, geometry, key routing, profile restore, session protocol, and
  terminal behavior when changing those areas.
- To *look* at a screen, capture the real binary headlessly rather than writing a snapshot test with
  no assertion. The rule above forbids an in-process test mutating the environment; a child process
  is a different thing entirely, and isolating one is just its env:

```bash
cargo build --features ui-snapshot
S=/tmp/rozi-snap && mkdir -p $S/config/rozi
printf '[session]\nstartup = "ephemeral"\n' > $S/config/rozi/config.toml
env XDG_CONFIG_HOME=$S/config XDG_STATE_HOME=$S/state XDG_CACHE_HOME=$S/cache \
    XDG_RUNTIME_DIR=$S/run ROZI_CONFIG=$S/config/rozi/config.toml \
    TUI_LIPAN_SNAPSHOT=$S/out.png TUI_LIPAN_SNAPSHOT_VIEWPORTS=80x24,120x30 \
    TUI_LIPAN_SNAPSHOT_SETTLE_MS=1800 \
    TUI_LIPAN_SNAPSHOT_SCRIPT="key:ctrl+a; key:c; sleep:250" \
    TUI_LIPAN_SNAPSHOT_ADVANCE_MS=400 ./target/debug/rozi
```

  **`TUI_LIPAN_SNAPSHOT_SETTLE_MS` is not optional here.** Panes are PTY-backed through the session
  server, so without a real settle the capture is written before any pane has content *and before the
  36 ms reveal timer clears `opening`* - which renders every pane at opacity 0. The result is a
  plausible-looking image: workbar drawn, workspace tab showing a pane count, empty canvas. Nothing
  warns you. Budget ~1.5-2 s for the server handshake and the shell's own startup.

  Three kinds of waiting, and they are not interchangeable:

  | Want | Use | Why |
  | --- | --- | --- |
  | The app up, PTYs live, prompts drawn | `SETTLE_MS` (real) | A virtual clock cannot make a subprocess answer |
  | An animation or reveal timer to progress | `ADVANCE_MS` or script `wait:` (virtual) | Instant and deterministic |
  | A pane spawned *by the script* to come up | script `sleep:` (real) | `SETTLE_MS` runs before the script only |

  To catch an animation mid-flight, end the script with `sleep:` (let the new pane's PTY start) then
  `wait:` a partial duration. A trailing `ADVANCE_MS` runs last and finishes whatever is still moving,
  so use it for the settled shot, not the mid-flight one.

  Keep `$S` short. The control socket is a Unix socket, so a long scratch path exceeds `SUN_LEN`
  and the session comes up without one, which shows as a `Control socket unavailable` toast across
  the capture.

  Run the built binary, not `cargo snap`: redirecting `XDG_*` also redirects mise's trust store and
  the wrapper refuses to start. That same redirection makes `mise ERROR ... are not trusted` appear
  in the captured pane content - it is the capture's own sandbox talking, not a bug in what you are
  looking at.

  **Rebuild before every capture.** Any `cargo test` / `cargo build` / `cargo clippy` in between
  replaces `./target/debug/rozi` with a binary lacking `ui-snapshot`, after which a `.png` path
  silently receives markdown instead.

  **When a capture looks wrong, get the widget tree before theorising:**

```bash
env ... TUI_LIPAN_SNAPSHOT=$S/tree.md TUI_LIPAN_SNAPSHOT_DIAGNOSTIC=1 ./target/debug/rozi
```

  A pane listed at a sensible rect (`Animated key=rozi-pane-<id>-<gen> @ (3,3) 54x13`) with nothing
  drawn means it is present and *invisible*, not missing - so the cause is time, not geometry. The
  dump does not report opacity, which is exactly the attribute that goes wrong here, so a healthy rect
  is not evidence that rendering works. Confirming trick: capture with `[animations] enabled = false`
  and `pane_style = "slide"`, which forces opacity 1 and zero offset; if the pane appears, the
  geometry was never the problem.
- Benchmarks are local performance evidence, not timing tests: `cargo check --all-targets` compiles
  them, while `cargo bench` runs them on a stable, idle machine. Keep corpora deterministic and
  generated; never add captured terminal output. See `docs/benchmarks.md`.
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

- `Cargo.toml` depends on the published `tui-lipan = "0.2.0"`, so this repository builds, tests, and
  releases from a standalone checkout with no sibling clone. `Cargo.lock` pins it by `source` and
  `checksum`; keep it that way.
- To try an unreleased framework change, add a `[patch.crates-io]` override pointing at the sibling
  checkout **locally, without committing it**. A committed patch would make CI and a release build
  silently disagree with the lock.
- Publish the framework version first; a rozi change that needs it cannot go green in CI until the
  version it requires is on crates.io. Bump the requirement here in the same change that needs it.

- For framework terminal changes in `../tui-lipan`, verify both sides:

```bash
cargo check --features terminal
cargo clippy --features terminal
```

  Then rerun the relevant `rozi` tests and lints, and publish the framework before the rozi
  change that needs it can go green in CI.

CI (`.github/workflows/ci.yml`) runs `fmt --check`, `check --all-targets` (which compiles benches),
`clippy -D warnings`, `test`, and a release build natively on `ubuntu-latest`, `macos-latest`, and
`windows-latest`, plus `cargo audit` in a separate Linux job. The workflow checks out only this
repository, which is all it needs now that the framework comes from crates.io.

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
- Runtime config comes from `$ROZI_CONFIG` or `~/.config/rozi/config.toml`; treat user config
  as local data, not repository state.
- Control and session endpoints are per-user and per-run: Unix sockets on Linux/macOS, named pipes
  on Windows. Preserve the runtime-dir safety checks (Unix ownership/mode/symlink validation;
  Windows reparse-point rejection and the protected current-user-SID DACL) when editing endpoint
  discovery, and keep `PIPE_REJECT_REMOTE_CLIENTS` and `FILE_FLAG_FIRST_PIPE_INSTANCE` on the
  Windows backend - they are what stop remote reachability and name squatting respectively.
- Windows discovery entries under the runtime directory are hints only. Never read a pipe name out
  of one; derive it. Every endpoint must still complete the authenticated protocol handshake.
- Named session endpoints are scoped to rozi session names; keep name validation and
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
- Rozi is a case study for the sibling `../tui-lipan` framework, which is owned alongside this
  project. Framework changes are welcome when a missing capability is reusable framework behavior
  rather than rozi-specific policy; do not work around a framework deficiency in the app solely
  to avoid editing tui-lipan.
- If editing `../tui-lipan`, inspect its git status separately and stage only intended files.
- Do not sweep unrelated local files, `.superpowers/` reports, or `.agents/` skills into commits.
- Preserve unrelated worktree changes; never revert user work without explicit approval.
- Do not amend commits, force-push, or run destructive git commands unless explicitly requested.
- Before committing, inspect `git status --short`, `git diff --stat`, and `git log --oneline -10`.
- Stage paths explicitly; this workspace may be dirty.
- Run `git diff --check` before a commit.
- Keep docs synchronized when changing user-visible behavior, CLI flags, config keys, or workflows.
- Prefer intentional breaking changes over backwards-compatibility code during active development.

## Extensibility Hooks

- `ROZI_CONFIG` selects an alternate config file; `--config <PATH>` sets it for the run and reaches
  every command that loads config (launch, `--server`, remote `list-sessions` / `kill-session`).
- `ROZI_SOCKET` points CLI control commands at a live UI control socket.
- `NO_COLOR` / `CLICOLOR` / `CLICOLOR_FORCE` / `TERM=dumb` decide whether `--help` is styled, via
  `platform::ansi`. Plain-stdout styling goes through there, never raw escapes at the call site.
- `ROZI=1`, `ROZI_PANE`, and `ROZI_SOCKET` are injected into spawned panes;
  `PaneIdentity::env` adds never-persisted per-spawn variables (the file tree passes the activated
  path as `ROZI_FILE` so a `run`/`popup` command never has a filename spliced into it).
- `[[hooks]]` runs client-side commands for the 16 `events::EventKind` variants and injects
  `ROZI_EVENT`, event fields, and `ROZI_SOCKET` (plus `ROZI_REMOTE_HOST` when attached via
  `--remote`); see `docs/hooks.md`.
- `[keys]` can rebind built-in actions or define user commands with `run` / `send` tables.
- `[[rules]]` applies first-match command substring placement to interactive command-carrying pane
  spawns, including control `new-pane` and `[keys] run`.
- `[workbar]` supports built-in segments, text placeholders, and timed shell command segments; each
  segment renders as a themed badge whose color can be overridden by theme role via a segment table,
  and `[pane].workbar_powerline` toggles trailing-badge chaining.
- `[theme].name` selects built-in, `system`, or custom themes from `~/.config/rozi/themes/`.
- `[profile] default` selects a startup profile from `~/.config/rozi/profiles/`.
- `[session] startup` picks what a bare launch does: `picker` (default), `ephemeral`, `last`, or
  `profile` (the session named after `[profile] default`). One spelling per mode, no aliases.
- `[session] autosave` enables local layout autosave/restore.
- `[session] resurrect` snapshots named sessions so layout, commands, and scrollback survive a server restart.
- `<NAME>` / `--session <NAME>` attaches or launches the canonical same-name profile; `attach
  <NAME>` is attach-only and `new <NAME> [--profile <RECIPE>]` explicitly creates a session.
- `--remote <HOST|ssh://URL>` attaches over SSH via a remote-side `--remote-serve` stdio proxy; see
  `docs/remote.md`. `ROZI_REMOTE_BINARY` forces which local binary is installed on the remote.
- Cargo feature flags are inherited from the `tui-lipan` dependency; this crate uses `terminal`,
  `terminal-images`, `terminal-serde`, `clipboard-images`, `theme-reload`, and `devtools`.

## Further Reading

- [README.md](README.md) - Project overview and documentation index.
- [docs/index.md](docs/index.md) - Full documentation table of contents.
- [docs/features.md](docs/features.md) - Single-page inventory of every feature.
- [docs/getting-started.md](docs/getting-started.md) - Build, run, quit, and dependency notes.
- [docs/configuration.md](docs/configuration.md) - Complete `config.toml` reference.
- [docs/keybindings.md](docs/keybindings.md) - Prefix mode, held modifier, mouse, and key table.
- [docs/layouts-and-panes.md](docs/layouts-and-panes.md) - Layouts, focus, movement, and animation.
- [docs/terminal.md](docs/terminal.md) - PTY, clipboard, selection, scrollback, and persistence.
- [docs/profiles.md](docs/profiles.md) - Named profiles and profile picker.
- [docs/project-profiles.md](docs/project-profiles.md) - Profile format and pane identity.
- [docs/sessions.md](docs/sessions.md) - Local vs attached sessions and detach/reattach semantics.
- [docs/remote.md](docs/remote.md) - SSH `--remote` attach, bootstrap/install, and feature split.
- [docs/control.md](docs/control.md) - Control socket CLI and JSON protocol.
- [docs/hooks.md](docs/hooks.md) - Hook syntax, event fields, environment, and execution semantics.
- [docs/benchmarks.md](docs/benchmarks.md) - Benchmarks, baselines, live stress, and profiling.
- [docs/themes.md](docs/themes.md) - Themes, hot reload, and terminal color palette.
