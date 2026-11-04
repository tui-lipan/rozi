# Architecture

Rozi has one root `tui-lipan` component, `AppRoot`, with central `State` and `Msg` updates.
`tui-lipan` owns UI runtime primitives. Rozi owns window-manager policy, pane/session behavior,
input routing, profiles, and terminal palette synchronization.

```text
CLI / thin main.rs
  |
  v
lib.rs -> app/: AppRoot
  |-- app/startup.rs  session-policy / StartupPlan
  |-- app/entry.rs    run() / CLI dispatch
  |-- state/ + msg.rs
  |-- update/::handle_msg -> update/ and ops/
  |-- input/routing.rs -> actions.rs or pane input
  |-- view/ -> Canvas, workbar, workspace, overlays
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

### Domain floor

Some files in the domain cores are cut free of the application layers: they must not import
`crate::state`, `config`, `view`, `app`, `ops` or `update`, and must not name `AppRoot`. They are
the plain vocabulary and documents the layers share: `layout/model.rs`, `pane/id.rs`,
`pane/chrome.rs`, the shared layout document (`layout/shared/mod.rs`, `validate.rs`, `edit.rs`,
`placement.rs`), the wire vocabulary (`control.rs`, `control/`, `session/protocol/`, and the
`runtime_metrics.rs`, `extension_runtime/protocol.rs` and `recording/format.rs` types it carries)
and `session/naming.rs`. `tests/suites/contracts/domain_floor.rs` holds the list and enforces it.
Unit-test modules are skipped, and only them: production code after a `#[cfg(test)] mod` is still
scanned. A file joins the list once it stops importing the layers; it never leaves the list to make
a change compile. Plain vocabulary that both a core and a layer need belongs in the core, not in
`state/`, and a conversion between a floor type and a layer type lives on the layer's side.

The guard covers *direct* imports only. It does not make these files separable from `state`: the
floor still calls into `layout/tiling.rs` (`DwindleTree`) and `layout/mod.rs`
(`effective_tile_tree`, `placement_for`), and both import `Workspace` from `state`. Moving the tree
and placement code into state-free modules, with `Workspace` adapters left behind, is what would make
the floor hold transitively.

The code that reads or writes `State` for these documents sits next to them but outside the list:
`layout/shared/project.rs` projects `State` into the document, `layout/shared/reconcile.rs`
reconciles `State` toward it, and `ops/control_listener.rs` turns control requests into `Msg`.

### Growing `Msg` and `State`

- **Nesting `Msg`.** When a change reworks a subsystem that has its own group of `Msg` variants, it
  nests that group as `Msg::Sidebar(sidebar::Msg)` with a dispatcher in `update/<domain>`, in the
  same change. The largest groups are Session, Sidebar, Extensions, Worktree, Keybinding, Remote,
  Settings and Pick. Sidebar goes first because it already has `update/sidebar/`.
- **Grouping `State` fields.** Group fields into a struct only when the struct enforces a rule,
  with methods that keep the fields consistent. Field count alone is not a reason.

New code lands in an existing layer or an existing domain core. Adding a new top-level module means
claiming a new domain, so say why in the change that introduces it.

## Runtime invariants

- The session server is the sole authority for session event ordering, deployed automation
  generations and session action execution. A client may present or request those things, but
  must never become their durable owner.
- Every PTY is server-owned. There is no in-process PTY mode.
- A client displaying panes parses raw server output into its own `TerminalScreen`.
- The startup launcher may have no attachment while the user chooses a session.
- Profiles restore layout and launch intent. A live session preserves PTY state.
- Named servers survive detach and quit. Untouched ephemeral sessions close; ephemeral sessions
  containing work require a keep decision.
- The server owns shared layout state. One client controls layout; followers reconcile through
  `apply_shared_layout` without replacing live screens.
- Shared layout mutations use `SharedLayout` semantics. Controllers reconcile placement into live
  workspaces independently of follower lifecycle reconciliation.
- Every successful shared spawn commits its resulting layout in the same server transaction.
- Each server pane owns its history capacity. Metadata precedes layout and replay; reload changes
  future panes only, and snapshots preserve each pane's capacity.
- Focus, active workspace, overlays, copy/search state, scrollback position, and theme remain client-local.
- Terminal size changes snap. Position and opacity may animate, but animated resizing would cause
  repeated PTY resize and SIGWINCH reflow.
- Split direction follows the focused tile's aspect ratio and
  `layout.split_width_multiplier`.

## Data flow

1. Keys arrive through `Component::on_key` or focused terminal callbacks.
2. `input/routing.rs` runs an app `Action` or forwards input to the session server.
3. `actions.rs` dispatches app operations. User-command execution lives in `ops/user_command.rs`.
   Persisted appearance/workbar/animation preferences live in `ops/preferences.rs`.
4. `update/mod.rs` routes messages and performs post-update synchronization.
5. `view/` renders the canvas and overlays.

Start with these paths rather than a static module inventory:

- Runtime wiring: `app/` (`AppRoot`, `startup.rs`, `entry.rs`), `msg.rs`, `update/`, `state/`
- Input: `input/`, `actions.rs`, `commands/` (`catalog.rs` + `registry.rs`)
- Panes: `pane/` (widget, `lifecycle/`, `pty_events/`, `launch.rs`, `rules.rs`)
- Layout: `layout/` (`tiling.rs`, `geometry.rs`, `anim.rs`, `shared.rs`), `ops/resize_move/`
- Sessions: `session/`, `ops/session/`, `layout/shared.rs`
- Rendering: `view/` (`animation.rs` for keyed chrome/transitions; `workspace.rs` for tiled/float layers; overlays are real modules)
- Focus ops: `ops/focus/` (spatial algorithm, `requests.rs`, `workspace.rs`)
- Remote runtime: `state::RemoteRuntimeState` (`state.remote`)
- Platform boundary: `src/platform/mod.rs`

Read `docs/sessions.md`, `docs/terminal.md`, and `docs/layouts-and-panes.md` for product behavior.

See `dev/shared-layout-authority.md` for transaction and reconciliation boundaries.
