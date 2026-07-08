# Tui-lipan App Structure Refactor Design

Date: 2026-06-20

## Goal

Make `hyprmux` easier for agents and humans to diagnose by reshaping the tui-lipan app layer around clear ownership boundaries, while keeping app-specific behavior in `hyprmux` and moving only reusable guidance or primitives to the sibling `tui-lipan` framework.

## Non-goals

- Do not change user-visible behavior during the structural refactor.
- Do not rename `Msg` variants or `Action` values just for aesthetics.
- Do not extract generic framework APIs unless implementation reveals a reusable framework concern.
- Do not move hyprmux-specific concepts into `tui-lipan`: dwindle layout, workspace semantics, prefix commands, pane lifecycle policy, floating/tiled behavior, theme-to-terminal palette syncing, or geometry animation policy.

## Current Problem

`src/main.rs` currently acts as the root tui-lipan component and also contains most app behavior: message dispatch, action execution, input routing, pane lifecycle, PTY handling, search, focus, move/resize, theme reload, and utility helpers. That makes diagnosis expensive because an agent must inspect one large mixed file to answer unrelated questions.

Other modules already have better seams:

- `input.rs` is the keybinding/action source of truth.
- `view.rs` owns tui-lipan widget composition and callback wiring.
- `layout.rs`, `tiling.rs`, and `geometry.rs` own app-specific placement/math.
- `pane.rs` wraps terminal screen/PTY behavior.
- `state.rs` holds the model and low-noise convenience methods.

The refactor should preserve these useful seams and reduce `main.rs` to framework glue.

## Target Architecture

Use a thin tui-lipan root shell plus focused app operation modules.

`main.rs` should keep:

- module declarations;
- `HyprmuxApp`;
- `Msg`;
- the `Component` implementation: `create_state`, `update`, `on_key`, and `view`;
- command-spawning bridge code only if it must remain close to `Component`/`Context`.

Move behavior clusters into modules:

- `update.rs` - central `Msg` dispatcher. It should answer “what happens when this message arrives?” without embedding large behavior bodies.
- `actions.rs` - converts `Action` values into app operations for prefix commands, command palette commands, and held-modifier shortcuts.
- `key_routing.rs` - owns `Mode::Normal` / `Mode::Prefix`, terminal-vs-window-manager routing, literal prefix forwarding, unknown-key forwarding, and cancel behavior.
- `pane_lifecycle.rs` - pane spawn, close, remove, prune, and PTY command helpers.
- `pty_events.rs` - PTY readiness, terminal output, terminal exit, and resize side effects.
- `focus_ops.rs` - directional focus, workspace focus/movement, hover/click focus policy, and focus cleanup helpers.
- `resize_move_ops.rs` - keyboard resize/move, mouse drag/resize session handling, floating/fullscreen transitions, and layout toggles.
- `search_ops.rs` - scrollback search state transitions, result navigation, and query lifecycle.
- `theme_ops.rs` - config/theme reload, picker selection, and terminal palette synchronization.

These modules should expose small `pub(crate)` functions that operate on `State` and tui-lipan `Context` where needed. Prefer plain modules over manager structs unless a struct removes real coupling. Plain modules are easier for agents to inspect and search.

## Module Contracts

### `main.rs`

- Defines the root app shell and cross-module message contract.
- Delegates `update` to `update::handle_msg(...)`.
- Delegates `on_key` to `key_routing::handle_key_routing(...)`.
- Delegates `view` to `view::render(...)`.
- Should read as tui-lipan integration glue, not as the whole application.

### `update.rs`

- Owns the `Msg` match.
- Stays thin by routing to operation modules.
- May contain tiny message-specific glue, but not long behavior bodies.

### `actions.rs`

- Owns `execute_action`-style behavior.
- Keeps command palette, prefix, and direct modifier actions on a shared dispatcher while allowing
  explicit palette-specific policy, such as bypassing repeated destructive-action confirmations.
- Depends on `input.rs` only for action definitions and binding metadata.

### `key_routing.rs`

- Owns the normal/prefix input state machine.
- Makes the two input sources explicit: framework `Component::on_key` and focused terminal callbacks via `Msg::PaneKey`.
- Preserves the existing routing invariant: normal terminal keys are forwarded to the focused PTY unless they are prefix keys or held window-manager modifier actions.

### Operation modules

- Own app-specific state transitions and side effects.
- Use narrow function names that match behavior: for example, `close_focused_pane`, `apply_terminal_palette_to_state`, `start_search`, `handle_pty_event`.
- Keep tui-lipan-specific side effects visible in signatures by accepting `Context` only when needed.
- Avoid hiding broad mutable access behind catch-all helper types.

### `state.rs`

- Remains data-model first.
- Keeps low-noise convenience methods such as `Workspace::tiled_ids`.
- Does not absorb large behavior methods just to reduce file count elsewhere.

### `view.rs`

- Remains the widget and callback boundary.
- Owns tui-lipan composition, `Canvas` placement, pane frames, terminal widget wiring, top bar, overlays, and callback-to-`Msg` mapping.
- Can be split later into `view/panes.rs`, `view/top_bar.rs`, and `view/overlays.rs` if growth continues, but the first refactor should not change visual behavior.

## Framework-side Work

Framework changes should be limited to reusable guidance unless implementation proves a reusable primitive belongs in `tui-lipan`.

Recommended framework documentation additions:

- A “large app shell” guide for tui-lipan apps that shows root `Component` glue, `State`/`Msg`, update dispatch, operation modules, view helpers, focus routing, and async command boundaries.
- A diagnostics guide for app authors explaining how to decide whether a bug belongs in app update logic, focus routing, widget callback wiring, terminal wiring, or framework primitives.
- Cross-links from existing docs to `examples/window_manager.rs` as the canonical multi-pane/root-routing reference.
- A decision table for app-side state machine vs widget-side props/focus/input behavior.

Framework-side API extraction is only justified if the refactor reveals a concept that is reusable across tui-lipan apps and not specific to hyprmux. Candidate areas to evaluate, but not assume, include diagnostics documentation helpers and terminal-app examples. Hyprmux’s window-manager policy remains app-side.

## Refactor Order

1. Add module shells and move functions almost verbatim.
2. Extract `Msg` dispatch into `update.rs` without redesigning messages.
3. Extract key routing into `key_routing.rs` and action execution into `actions.rs`.
4. Extract pane lifecycle, PTY events, focus, move/resize, search, and theme clusters into operation modules.
5. Add framework documentation for large app shell structure and diagnostics.
6. Consider small cleanup only after behavior-preserving extraction is complete and tests pass.

This order keeps diffs reviewable and makes each step reversible.

## Testing and Verification

For hyprmux-only code changes:

- `cargo fmt`
- `cargo test`
- `cargo clippy`
- `git diff --check`

Existing tests that must remain green include layout, tiling, geometry, and keyboard resize behavior tests.

Add targeted tests only if extraction exposes untested behavior around key routing, actions, search, or pane lifecycle. Avoid broad UI snapshot tests unless `view.rs` behavior changes.

For tui-lipan documentation-only changes, no compile is required unless examples or code snippets are modified in a way the repo verifies. If framework code changes are made, run the relevant framework checks with the `terminal` feature, then rerun hyprmux checks.

## Success Criteria

- `main.rs` becomes a short root component shell and no longer mixes unrelated operation bodies.
- Agents can locate behavior by module name without reading a large all-purpose file.
- `input.rs` remains the single source of truth for command bindings and action metadata.
- `view.rs` remains the only widget/callback composition boundary.
- App-specific behavior stays in hyprmux.
- tui-lipan receives reusable docs/pattern guidance rather than hyprmux-specific abstractions.
- Verification commands pass after implementation.
