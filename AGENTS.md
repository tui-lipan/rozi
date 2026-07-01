# AGENTS.md

## What this is

`hyprmux` is a Hyprland-style tiling **terminal multiplexer**: panes are live PTY shells,
laid out with dwindle tiling, floating windows, workspaces, animated geometry, and
tmux-style prefix commands. It is built on the `tui-lipan` TUI framework and was ported
from that project's `window_manager` example — that example remains the reference
implementation for the tiling/interaction algorithms.

By default `hyprmux` runs in local single-process mode with PTYs in the UI process. Explicit
`--attach` / `--session` mode connects to a named session server for detach/reattach-style
workflows.

## Commands

- Build: `cargo build`
- Run: `cargo run` (then quit the app with `Ctrl-q`); launch a named profile with `cargo run -- dev` or `cargo run -- --profile dev`
- Lint: `cargo clippy`
- Test: `cargo test`; a single test by name substring, e.g.
  `cargo test spawn_split_direction_follows_focused_tile_aspect`
- Format: `cargo fmt`; if you manually run `rustfmt`, pass `--edition 2024`.

> Dependency note: `tui-lipan` is a **path dependency** (`../tui-lipan`, `features = ["terminal"]`).
> The sibling `../tui-lipan` checkout must exist to build; the `terminal` feature pulls in
> `portable-pty` + `alacritty_terminal` for the PTY-backed terminal primitives. This repo is
> not self-contained for a standalone clone.

## Working with tui-lipan

- Prefer the dedicated `tui-lipan-rag` tools when unsure about framework APIs. Good first calls:
  `tui_lipan_lookup_widget` for widgets, `tui_lipan_lookup_widget_defaults` before noisy builder
  chains, `tui_lipan_lookup_example` for runnable patterns, and `tui_lipan_search` for broad API
  questions.
- `../tui-lipan` is a separate git repo and may be dirty with unrelated user work. If you must
  change it, inspect its status and stage only the intended files. Never sweep unrelated framework
  changes into a hyprmux-motivated commit.
- If a hyprmux behavior bug comes from a framework primitive or from the original
  `examples/window_manager.rs`, consider whether the same fix belongs in `../tui-lipan` too.
- For framework terminal changes, verify both sides: run the targeted tui-lipan command with the
  relevant feature, e.g. `cargo check --features terminal` or `cargo clippy --features terminal`,
  then run `cargo test` / `cargo clippy` in hyprmux.

## Git / Commit Hygiene

- Commit only when explicitly requested. Before committing, inspect `git status --short`,
  `git diff --stat`, and `git log --oneline -10` in every repo you will commit.
- The workspace can be dirty. Preserve unrelated untracked or modified files such as local agent
  docs, generated notes, or sibling-framework work. Stage paths explicitly.
- When committing in both repos, commit `../tui-lipan` first if hyprmux depends on that framework
  change, then commit hyprmux.
- Do not amend or force-push unless explicitly requested.

## Verification Habits

- For app-only Rust changes, prefer at least `cargo test` and `cargo clippy` from this repo.
- For broad feature work, run `cargo build` too. `cargo run` needs a real terminal; quit with
  `Ctrl-q`.
- `cargo fmt` may reformat older one-line code outside your logical change. If that happens, decide
  whether to include the formatting intentionally or restore it before committing scoped work.
- `git diff --check` is cheap and should be clean before commit.

## Architecture

Elm-style app: one root `Component` (`HyprmuxApp` in `main.rs`) with `State`/`Msg` and
`create_state` / `update` / `on_key` / `view`. tui-lipan supplies the runtime plus the
primitives this app leans on: `Canvas` (absolute child placement), `Transition<FloatRect>`
(geometry animation), `MouseRegion` (drag/resize), and `TerminalPty`/`TerminalScreen`/
`TerminalRenderSnapshot` (the terminal).

Three pieces require reading across modules to understand:

**1. Layout is app-driven geometry, not framework layout.** The dwindle tree (`tiling.rs`)
computes each tiled pane's target `FloatRect`; floating panes carry an explicit rect.
`view::render` places *every* pane (tiled or floating) into one `Canvas` at an animated rect
produced by `ctx.transition(key, target, config)`. Animation is therefore entirely app-side
— there is no engine rect-override. Critically, the policy in `HyprmuxApp::transition_config_for`
animates **position and opacity but effectively snaps size changes** (it returns an instant
transition during move/resize sessions and viewport changes) to avoid spamming `pty.resize`
/ SIGWINCH and reflowing the shell. Keep that invariant when touching animations.

**2. A pane is a real shell.** Each `Pane` (`state.rs`) owns a `TerminalPane` (`pane.rs`)
wrapping a `TerminalScreen` (VT emulator) + an optional `TerminalPty`. PTYs are spawned on a
background thread via `Command::spawn` → `spawn_pty` (`main.rs`), which sends `Msg::PtyReady`
then streams `Msg::PtyEvent`. Output bytes feed `screen.process_bytes` and re-render the
snapshot; `TerminalPtyEvent::Exited` closes the pane (`remove_pane`, with dwindle-tree +
focus cleanup; quitting when the last pane closes). Pane geometry size changes call
`TerminalPane::resize`, which resizes both the screen and the PTY.

**3. Input routing / command mode (the crux).** Keys reach the WM two ways: the framework
`Component::on_key` (when no terminal consumes them) and the focused terminal's input
callback (`Msg::PaneKey`). Both funnel through `handle_key_routing(ctx, key, source_pane)`,
a `Mode::Normal`/`Mode::Prefix` state machine:
- Normal: the prefix key (`Ctrl-a`) enters Prefix mode; an explicitly configured held chord,
  or the configured WM modifier plus an active command key, triggers an `Action` directly;
  otherwise the key is forwarded to the focused pane's PTY.
- Prefix: the next key runs an `Action`, or `Ctrl-a` again sends a literal `Ctrl-a`, or `Esc`
  cancels, or an unknown key is forwarded.

`input.rs` maps keys → `Action` and owns `command_bindings()`, the **single source of truth**
for both the command palette and the help overlay (workspace digits 1-9 are handled
separately because they expand into a range). `keymap.rs` resolves user-configured triggers;
`key_routing.rs` owns `handle_key_routing` and the mode state machine. `execute_action`
(`actions.rs`) dispatches actions.
The command palette intentionally omits repetitive workspace commands; workspace digits belong in
the help overlay. Theme selection is a single `Choose theme` command that opens a `List` modal.

**4. Config, themes, and terminal colors.** Runtime config is loaded by `config.rs` from
`$HYPRMUX_CONFIG` or `~/.config/hyprmux/hyprmux.toml`. Config parse/read failures should warn via
toasts without pretending the file was loaded. App chrome uses `ThemeProvider` and `Theme`; custom
theme files can hot-reload through tui-lipan's `ThemeWatcher`. PTY content colors are not just the
widget background: `TerminalScreen` snapshots resolve ANSI/default colors through a
`TerminalColorPalette`. Keep `apply_terminal_palette_to_state` in sync with theme changes,
theme-picker selection, hot reload, initial state, and spawned panes.

**5. Master layout and scrollback search.** `Workspace.layout_kind` switches placement between
dwindle and master. Master uses the first tiled pane as the left master and the remaining tiled
panes as a right stack. Scrollback search is app-side: it scans `TerminalScreen` snapshots by moving
the screen scrollback offset, deduplicates overlapping scan windows, then restores the original
offset and jumps to selected matches. tui-lipan does not currently provide in-terminal highlight
search.

### Module map

Modules are grouped by concern. Feature behavior that reacts to a `Msg` lives in a `*_ops`
module; lifecycle and event-plumbing modules keep plain names (see Conventions).

**Core / runtime**
- `main.rs` — root `Component`, `init`/`on_key`/`view` wiring, animation/transition policy
  (`transition_config_for`, opacity/chrome configs), `main()` entry (config + theme + profile
  bootstrap), tick scheduling.
- `update.rs` — `handle_msg`: the flat `Msg` router that delegates each message to a feature
  module, then re-applies the terminal palette.
- `control.rs` / `control_ops.rs` — in-process Unix socket protocol, CLI wire types, listener
  bootstrap, and UI-thread execution of automation commands.
- `session/` — optional named session server/client protocol used by explicit `--attach` /
  `--session` mode; local non-attached launches still keep PTYs in the UI process.
- `state.rs` — runtime data model (`State`, `Workspace`, `Pane`, `Mode`, sessions) and tuning
  constants (gaps, ratios, `SPLIT_WIDTH_MULTIPLIER`, `ThemePreset`, `LayoutKind`).
- `view.rs` — `render`: the `Canvas` of panes (each a `Frame` + terminal), top bar,
  palette/help/search/rename/theme overlays, and the per-pane mouse/drag/keyboard callbacks.

**Input**
- `input.rs` — `Action` enum, key→`Action` mapping, `command_bindings()` (single source of truth
  for palette + help overlay).
- `keymap.rs` — `Keymap`/`Trigger`: resolves user-configured key bindings.
- `key_routing.rs` — `handle_key_routing` + the `Normal`/`Prefix` mode state machine; framework
  focus sync.
- `actions.rs` — `execute_action`: dispatches each `Action` to the relevant op.

**Panes / PTY**
- `pane.rs` — `TerminalPane`: PTY + screen + snapshot lifecycle, terminal palette, scrollback
  search helper, resize.
- `pane_lifecycle.rs` — spawn/close/prune, `pty_config_for_pane`, startup `initial_command`,
  `find_pane_mut`.
- `pty_events.rs` — `PtyReady`/`PtyEvent`/`PaneInput`/`PaneMouse`/`PaneResize`/`PaneScroll`
  handlers; toast helpers.

**Layout / geometry**
- `tiling.rs` — `DwindleTree` algorithms: split/insert/remove/flip, ratio adjust, and the
  allocators (`allocate_dwindle`, `allocate_master`, grid/spiral/monocle).
- `layout.rs` — `Workspace` → placements; `place_spawned_pane` (new pane always splits the
  *focused* pane, axis from its aspect ratio — Hyprland dwindle, never the cursor).
- `geometry.rs` — `FloatRect` math: clamps, resize-from-corner, terminal-border resize gate,
  split direction, spatial-focus scoring.
- `resize_move_ops.rs` — interactive move/resize sessions, split-ratio drags, directional
  move/swap/resize, tiling/fullscreen/layout toggles.
- `anim.rs` — `GeometryAnimation`, `WindowAnimationConfig`, transition presets.

**Features**
- `focus_ops.rs` — focus changes and framework focus requests.
- `copy_mode.rs` — vi-style copy/selection mode.
- `search_ops.rs` — scrollback search scan/recompute/navigation, scope cycling.
- `scratchpad.rs` — the toggleable scratchpad pane and its focus handoff.
- `identity_ops.rs` — pane rename state apply/close.
- `profile_ops.rs` — save-by-name prompt, unified profile picker (load/set-default/delete), profile load/switch.
- `theme_ops.rs` — theme picker/preview, hot-reload tick, terminal-palette application,
  system-theme derivation.
- `profiles.rs` — profile/session (de)serialization and `State` restore/persist. Named profiles
  live in `~/.config/hyprmux/profiles/`; `[profile] default` names the startup profile.
- `config.rs` — TOML config loading, env/default path handling, theme-file loading, profile
  directory discovery, warning collection.

## Conventions

- The dwindle/geometry/animation logic is ported from tui-lipan's `examples/window_manager.rs`.
  When fixing behavior here, check whether the same fix belongs there (and vice-versa) —
  several fixes have been kept in sync across both.
- Split direction follows the focused tile's aspect ratio with `SPLIT_WIDTH_MULTIPLIER`
  correcting for terminal cells being ~2× taller than wide (Hyprland's `split_width_multiplier`).
- **Module naming:** a module that implements a *feature's reaction to messages/actions* uses the
  `*_ops` suffix (`focus_ops`, `search_ops`, `theme_ops`, `identity_ops`, `resize_move_ops`).
  Lifecycle, event-plumbing, and pure-data modules keep plain names (`pane_lifecycle`,
  `pty_events`, `key_routing`, `profiles`, `config`). New modules should follow this split rather
  than inventing a third convention.
