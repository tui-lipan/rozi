# Architecture

Rozi has one root `tui-lipan` component, `AppRoot`, with central `State` and `Msg` updates.
`tui-lipan` owns UI runtime primitives. Rozi owns window-manager policy, pane/session behavior,
input routing, profiles, and terminal palette synchronization.

```text
CLI / thin main.rs
  |
  v
lib.rs -> app.rs: AppRoot
  |-- state/ + msg.rs
  |-- update/::handle_msg -> update/ and ops/
  |-- key_routing.rs -> actions.rs or pane input
  |-- view/ -> Canvas, panes, workbar, overlays
  |
  +--> session/client <-> session/server <-> server-owned PTYs
```

## Runtime invariants

- Every PTY is server-owned. There is no in-process PTY mode.
- A client displaying panes parses raw server output into its own `TerminalScreen`.
- The startup launcher may have no attachment while the user chooses a session.
- Profiles restore layout and launch intent. A live session preserves PTY state.
- Named servers survive detach and quit. Untouched ephemeral sessions close; ephemeral sessions
  containing work require a keep decision.
- The server owns shared layout state. One client controls layout; followers reconcile through
  `apply_shared_layout` without replacing live screens.
- Focus, active workspace, overlays, copy/search state, scrollback, and theme remain client-local.
- Terminal size changes snap. Position and opacity may animate, but animated resizing would cause
  repeated PTY resize and SIGWINCH reflow.
- Split direction follows the focused tile's aspect ratio and
  `layout.split_width_multiplier`.

## Data flow

1. Keys arrive through `Component::on_key` or focused terminal callbacks.
2. `key_routing.rs` runs an app `Action` or forwards input to the session server.
3. `actions.rs` dispatches app operations.
4. `update/mod.rs` routes messages and performs post-update synchronization.
5. `view/` renders the canvas and overlays.

Start with these paths rather than a static module inventory:

- Runtime wiring: `app.rs`, `msg.rs`, `update/`, `state/`
- Input: `key_routing.rs`, `actions.rs`, `input.rs`, `commands.rs`
- Panes: `pane.rs`, `pane_lifecycle/`, `pty_events/`
- Layout: `tiling.rs`, `layout.rs`, `geometry.rs`, `ops/resize_move/`, `anim.rs`
- Sessions: `session/`, `ops/session/`, `shared_layout.rs`
- Rendering: `view/`
- Platform boundary: `src/platform/mod.rs`

Read `docs/sessions.md`, `docs/terminal.md`, and `docs/layouts-and-panes.md` for product behavior.
