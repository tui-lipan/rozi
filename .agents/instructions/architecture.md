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
  |-- input/routing.rs -> actions.rs or pane input
  |-- view/ -> Canvas, panes, workbar, overlays
  |
  +--> session/client <-> session/server <-> server-owned PTYs
```

## Where code goes

`src/` is organized **layer-first**. The layers are the Elm-ish decomposition every domain passes
through, and a new file belongs to whichever layer's job it does:

| Layer | Holds |
| --- | --- |
| `state/` | The `State` tree and its per-domain slices. No I/O, no rendering. |
| `update/` | `Msg` routing and post-update synchronization. |
| `ops/` | App operations an `Action` or a message performs. |
| `view/` | Everything that produces an `Element`. |
| `config/` | Parsing, schema, and persistence of user configuration. |

Beneath the layers sit the **domain cores** — the layer-independent types and policy the layers
operate on: `layout/`, `pane/`, `input/`, `session/`, `scratchpad/`, `agent_detection/`,
`platform/`, `skill/`.

The two axes cross, so the same domain name appears once per layer it touches. `pane/` is the
terminal widget and its lifecycle, `state/pane.rs` is the per-pane app state, `view/pane.rs` renders
it. That repetition is the structure working, not a naming clash — read the *directory*, not the
file name, to know which one you want.

New code lands in an existing layer or an existing domain core. Adding a new top-level module means
claiming a new domain, so say why in the change that introduces it.

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
2. `input/routing.rs` runs an app `Action` or forwards input to the session server.
3. `actions.rs` dispatches app operations.
4. `update/mod.rs` routes messages and performs post-update synchronization.
5. `view/` renders the canvas and overlays.

Start with these paths rather than a static module inventory:

- Runtime wiring: `app.rs`, `msg.rs`, `update/`, `state/`
- Input: `input/`, `actions.rs`, `commands.rs`
- Panes: `pane/` (widget, `lifecycle/`, `pty_events/`, `launch.rs`, `rules.rs`)
- Layout: `layout/` (`tiling.rs`, `geometry.rs`, `anim.rs`, `shared.rs`), `ops/resize_move/`
- Sessions: `session/`, `ops/session/`, `layout/shared.rs`
- Rendering: `view/`
- Platform boundary: `src/platform/mod.rs`

Read `docs/sessions.md`, `docs/terminal.md`, and `docs/layouts-and-panes.md` for product behavior.
