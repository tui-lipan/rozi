# Tui-lipan Agent-Diagnosable Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor hyprmux into a thin tui-lipan root shell plus focused app operation modules, and add tui-lipan framework documentation for large app diagnosis.

**Architecture:** `src/main.rs` remains the root `Component`, `Msg`, app bootstrap, and view delegation. Message handling, key routing, action execution, pane lifecycle, PTY events, focus, move/resize, search, and theme behavior move into focused modules using `pub(crate)` functions and `Context<HyprmuxApp>`. Framework work is documentation-only unless execution proves a reusable framework primitive belongs in `../tui-lipan`.

**Tech Stack:** Rust 2024, tui-lipan path dependency with `terminal` feature, cargo test/clippy/fmt, Markdown docs.

## Global Constraints

- Preserve user-visible hyprmux behavior during the structural refactor.
- Keep app-specific concepts in hyprmux: dwindle layout, workspace semantics, prefix commands, pane lifecycle policy, floating/tiled behavior, theme-to-terminal palette syncing, and geometry animation policy.
- Do not extract generic tui-lipan APIs unless a reusable framework concern is proven during implementation.
- Keep `input.rs` as the single source of truth for command bindings and action metadata.
- Keep `view.rs` as the only tui-lipan widget/callback composition boundary for this refactor.
- Do not commit unless the user explicitly grants commit permission. When a task says “commit gate,” run the listed status/diff checks and only run `git commit` if permission exists.
- Preserve unrelated dirty work in both `/home/razuer/Work/Projects/hyprmux` and `/home/razuer/Work/Projects/tui-lipan`.

---

## File Structure

### Hyprmux files

- Modify: `src/main.rs`
  - Keep module declarations, `HyprmuxApp`, `FrameworkFocus`, `Msg`, `Component` impl, transition/chrome helpers, `main`, and tests only until a task explicitly moves a helper.
  - Delegate `update` to `update::handle_msg`.
  - Delegate `on_key` to `key_routing::handle_key_routing` after `Ctrl-q` and framework focus sync.

- Create: `src/update.rs`
  - Own the `Msg` match through `pub(crate) fn handle_msg(app: &mut HyprmuxApp, msg: Msg, ctx: &mut Context<HyprmuxApp>) -> Update`.

- Create: `src/key_routing.rs`
  - Own normal/prefix/resize key routing through `pub(crate) fn handle_key_routing(ctx: &mut Context<HyprmuxApp>, key: KeyEvent, source_pane: Option<PaneId>) -> (bool, Update)`.
  - Own framework focus synchronization helpers.

- Create: `src/actions.rs`
  - Own `pub(crate) fn execute_action(ctx: &mut Context<HyprmuxApp>, action: Action) -> Update`.
  - Own command registry setup.

- Create: `src/search_ops.rs`
  - Own search opening, recomputation, navigation, and jump behavior.

- Create: `src/theme_ops.rs`
  - Own theme picker selection, theme watcher ticks, terminal palette sync, and terminal palette helpers.

- Create: `src/pty_events.rs`
  - Own PTY event handling, terminal input, pane mouse bytes, pane resize, pane scrollback, and key forwarding to PTYs.

- Create: `src/pane_lifecycle.rs`
  - Own pane spawn, close, remove/prune behavior, initial PTY command creation, and PTY spawn helpers.

- Create: `src/focus_ops.rs`
  - Own focus movement, workspace switching, fallback focus, framework focus requests, visible pane placement helpers, and active pane lookup helpers.

- Create: `src/resize_move_ops.rs`
  - Own mouse move/resize sessions, keyboard move/resize, master split adjustment, floating/tiled drop, and tiling/fullscreen/layout toggles.

- Leave unchanged unless compiler requires import cleanup: `src/input.rs`, `src/view.rs`, `src/state.rs`, `src/layout.rs`, `src/pane.rs`, `src/tiling.rs`, `src/geometry.rs`.

### Tui-lipan framework files

- Modify or create in `/home/razuer/Work/Projects/tui-lipan/docs/`:
  - Preferred create: `docs/large-app-shells.md`
  - Preferred modify: `docs/index.md`
  - Optional modify: `docs/patterns.md` if the index already points all pattern guides there.

---

### Task 1: Preflight and module skeleton

**Files:**
- Modify: `src/main.rs:1-9`
- Create: `src/update.rs`
- Create: `src/actions.rs`
- Create: `src/key_routing.rs`
- Create: `src/search_ops.rs`
- Create: `src/theme_ops.rs`
- Create: `src/pty_events.rs`
- Create: `src/pane_lifecycle.rs`
- Create: `src/focus_ops.rs`
- Create: `src/resize_move_ops.rs`

**Interfaces:**
- Consumes: existing `HyprmuxApp`, `Msg`, `State`, `Context<HyprmuxApp>`, `Update`.
- Produces: empty modules imported by `main.rs`; no behavior changes.

- [ ] **Step 1: Record clean baseline**

Run:

```bash
git status --short
cargo test
```

Expected: `cargo test` passes. If `git status --short` shows unrelated user changes, write down the paths and do not edit them.

- [ ] **Step 2: Add module declarations**

Edit `src/main.rs` so the top module block is exactly this order:

```rust
mod actions;
mod anim;
mod config;
mod focus_ops;
mod geometry;
mod input;
mod key_routing;
mod layout;
mod pane;
mod pane_lifecycle;
mod pty_events;
mod resize_move_ops;
mod search_ops;
mod state;
mod theme_ops;
mod tiling;
mod update;
mod view;
```

- [ ] **Step 3: Create empty module files**

Create these files with a single module comment so the compiler sees them:

```rust
//! Focused operation module extracted from the root hyprmux app shell.
```

Use that exact line in:

```text
src/actions.rs
src/focus_ops.rs
src/key_routing.rs
src/pane_lifecycle.rs
src/pty_events.rs
src/resize_move_ops.rs
src/search_ops.rs
src/theme_ops.rs
src/update.rs
```

- [ ] **Step 4: Verify skeleton compiles**

Run:

```bash
cargo test
```

Expected: tests pass with no behavior changes.

- [ ] **Step 5: Commit gate**

Run:

```bash
git diff -- src/main.rs src/actions.rs src/focus_ops.rs src/key_routing.rs src/pane_lifecycle.rs src/pty_events.rs src/resize_move_ops.rs src/search_ops.rs src/theme_ops.rs src/update.rs
```

If commit permission exists:

```bash
git add src/main.rs src/actions.rs src/focus_ops.rs src/key_routing.rs src/pane_lifecycle.rs src/pty_events.rs src/resize_move_ops.rs src/search_ops.rs src/theme_ops.rs src/update.rs
git commit -m "refactor: add hyprmux operation modules"
```

If commit permission does not exist, leave the files uncommitted and continue.

---

### Task 2: Prepare the message dispatcher module

**Files:**
- Modify: `src/update.rs`

**Interfaces:**
- Consumes: `HyprmuxApp`, `Msg`, `Context<HyprmuxApp>`, `Update`.
- Produces: `pub(crate) fn handle_msg(app: &mut HyprmuxApp, msg: Msg, ctx: &mut Context<HyprmuxApp>) -> Update`.

- [ ] **Step 1: Add dispatcher interface without wiring it yet**

Replace the comment in `src/update.rs` with this compiling shell:

```rust
use tui_lipan::prelude::*;

use crate::{HyprmuxApp, Msg};

pub(crate) fn handle_msg(
    _app: &mut HyprmuxApp,
    _msg: Msg,
    _ctx: &mut Context<HyprmuxApp>,
) -> Update {
    unreachable!("update::handle_msg is wired after operation modules are extracted")
}
```

- [ ] **Step 2: Verify dispatcher interface compiles while unused**

Run:

```bash
cargo test
```

Expected: tests pass. `Component::update` still uses the existing in-place match in this task.

- [ ] **Step 3: Commit gate**

Run:

```bash
git diff -- src/update.rs
```

If commit permission exists:

```bash
git add src/update.rs
git commit -m "refactor: prepare update dispatcher"
```

If commit permission does not exist, leave the file uncommitted.

---

### Task 3: Extract key routing into `key_routing.rs`

**Files:**
- Modify: `src/main.rs:336-348,453-524,1858-1875`
- Modify: `src/key_routing.rs`

**Interfaces:**
- Consumes: `actions::execute_action`, `pty_events::forward_key_to_pane`, `focus_ops::request_current_pane_focus`, `resize_move_ops::resize_focused_in_direction`.
- Produces:
  - `pub(crate) fn handle_key_routing(ctx: &mut Context<HyprmuxApp>, key: KeyEvent, source_pane: Option<PaneId>) -> (bool, Update)`
  - `pub(crate) fn sync_focus_from_framework(ctx: &mut Context<HyprmuxApp>)`
  - `pub(crate) fn framework_focused_pane(ctx: &Context<HyprmuxApp>) -> Option<PaneId>`

- [ ] **Step 1: Move key routing imports and functions**

Replace `src/key_routing.rs` with:

```rust
use tui_lipan::prelude::*;

use crate::actions::execute_action;
use crate::focus_ops::request_current_pane_focus;
use crate::input;
use crate::pty_events::forward_key_to_pane;
use crate::resize_move_ops::resize_focused_in_direction;
use crate::state::{Direction, Mode, PaneId};
use crate::view;
use crate::HyprmuxApp;
```

Then move these functions from `src/main.rs` into `src/key_routing.rs` and mark them `pub(crate)`:

```text
handle_key_routing
handle_resize_mode_key
framework_focused_pane
sync_focus_from_framework
```

The function bodies should remain unchanged except calls to sibling-module functions must use the imported names above.

- [ ] **Step 2: Update `Component::on_key`**

Change `src/main.rs` to call the module functions explicitly:

```rust
fn on_key(&mut self, key: KeyEvent, ctx: &mut Context<Self>) -> KeyUpdate {
    if key.mods.ctrl && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q')) {
        ctx.quit();
        return KeyUpdate::handled(Update::none());
    }

    key_routing::sync_focus_from_framework(ctx);
    let (handled, update) = key_routing::handle_key_routing(ctx, key, None);
    if handled {
        KeyUpdate::handled(update)
    } else {
        KeyUpdate::unhandled(update)
    }
}
```

- [ ] **Step 3: Remove moved functions from `main.rs`**

Delete the original root definitions of:

```text
handle_key_routing
handle_resize_mode_key
framework_focused_pane
sync_focus_from_framework
```

- [ ] **Step 4: Verify after dependent modules exist**

Run after Tasks 4 through 6 are complete:

```bash
cargo test keyboard_resize_directions_grow_toward_the_nearest_split
```

Expected: the test passes.

---

### Task 4: Extract action execution and search/theme modules

**Files:**
- Modify: `src/main.rs:526-676,678-779,1345-1409`
- Modify: `src/actions.rs`
- Modify: `src/search_ops.rs`
- Modify: `src/theme_ops.rs`

**Interfaces:**
- Consumes: `input::Action`, focus operations, pane lifecycle operations, move/resize operations.
- Produces:
  - `actions::execute_action(ctx: &mut Context<HyprmuxApp>, action: Action) -> Update`
  - `actions::register_commands(ctx: &mut Context<HyprmuxApp>)`
  - `search_ops::{open_search, recompute_search, search_next, jump_to_search_match}`
  - `theme_ops::{open_theme_picker, select_theme, theme_tick, apply_terminal_palette_to_state, terminal_palette, theme_error_toast}`

- [ ] **Step 1: Move action functions**

Replace `src/actions.rs` imports with:

```rust
use tui_lipan::prelude::*;

use crate::focus_ops::{focus_in_direction, move_focused_to_workspace, request_current_pane_focus, request_pane_focus, switch_workspace};
use crate::input::{self, Action};
use crate::pane_lifecycle::{close_focused_pane, spawn_pane};
use crate::resize_move_ops::{adjust_focused_split_ratio, move_focused_in_direction, toggle_focused_split_axis, toggle_fullscreen, toggle_layout, toggle_tiling};
use crate::search_ops::open_search;
use crate::theme_ops::{open_theme_picker, select_theme};
use crate::{HyprmuxApp, Msg};
```

Move these functions from `src/main.rs` into `src/actions.rs` and mark them `pub(crate)`:

```text
execute_action
register_commands
```

Keep function bodies unchanged except imported operation calls should resolve through the imports above.

- [ ] **Step 2: Update initialization call sites**

In `src/main.rs`, change:

```rust
register_commands(ctx);
```

to:

```rust
actions::register_commands(ctx);
```

- [ ] **Step 3: Move search functions**

Replace `src/search_ops.rs` imports with:

```rust
use tui_lipan::prelude::*;

use crate::focus_ops::{find_pane_mut, request_search_focus};
use crate::state::{PaneId, ScrollbackSearchState};
use crate::HyprmuxApp;
```

Move these functions from `src/main.rs` into `src/search_ops.rs` and mark them `pub(crate)`:

```text
open_search
recompute_search
search_next
jump_to_search_match
```

Keep bodies unchanged.

- [ ] **Step 4: Move theme functions**

Replace `src/theme_ops.rs` imports with:

```rust
use tui_lipan::prelude::*;

use crate::state::{State, ThemePreset};
use crate::{HyprmuxApp, Msg};
```

Move these functions from `src/main.rs` into `src/theme_ops.rs` and mark them `pub(crate)`:

```text
handle_theme_tick as theme_tick
select_theme
apply_terminal_palette_to_state
terminal_palette
style_fg
clean_terminal_color
```

Add this wrapper in `src/theme_ops.rs`:

```rust
pub(crate) fn theme_error_toast(message: String) -> Toast {
    error_toast("Theme Reload", message)
}
```

If `error_toast` remains in another module after Task 6, import it from that module instead of using an unqualified name.

- [ ] **Step 5: Update create-state palette call**

In `src/main.rs`, change:

```rust
apply_terminal_palette_to_state(&mut state);
```

to:

```rust
theme_ops::apply_terminal_palette_to_state(&mut state);
```

- [ ] **Step 6: Verify search/theme/action extraction after dependent modules exist**

Run after Tasks 5 and 6 are complete:

```bash
cargo test
```

Expected: all tests pass.

---

### Task 5: Extract PTY events and pane lifecycle

**Files:**
- Modify: `src/main.rs:781-845,848-934,1815-1856,1895-1948`
- Modify: `src/pty_events.rs`
- Modify: `src/pane_lifecycle.rs`
- Modify: `src/update.rs`

**Interfaces:**
- Consumes: `focus_ops`, `theme_ops`, terminal types from tui-lipan, pane/state types.
- Produces:
  - `pty_events::{forward_key_to_pane, handle_pty_event, handle_pane_input, handle_pane_mouse, handle_pane_resize, handle_pane_scroll}`
  - `pane_lifecycle::{spawn_pane, begin_close_pane, close_focused_pane, remove_pane, total_visible_panes, find_pane_mut, initial_command, pty_config, spawn_pty_command, prune_closed_command}`

- [ ] **Step 1: Move toast helpers to `pty_events.rs`**

Replace `src/pty_events.rs` imports with:

```rust
use std::sync::Arc;

use tui_lipan::prelude::*;

use crate::focus_ops::find_pane_mut;
use crate::pane::PaneEventOutcome;
use crate::pane_lifecycle::{begin_close_pane, prune_closed_command};
use crate::state::PaneId;
use crate::theme_ops::apply_terminal_palette_to_state;
use crate::{HyprmuxApp, Msg};
```

Move these functions from `src/main.rs` into `src/pty_events.rs` and mark them `pub(crate)`:

```text
info_toast
error_toast
forward_key_to_pane
handle_pty_event
handle_terminal_input as handle_pane_input
```

Create these new wrappers by moving the corresponding match-arm bodies from `update.rs` into named functions:

```rust
pub(crate) fn handle_pane_mouse(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    bytes: Vec<u8>,
) -> Update {
    let mut error = None;
    if let Some(pane) = find_pane_mut(&mut ctx.state, id)
        && let Err(message) = pane.terminal.send_bytes(&bytes)
    {
        error = Some(message.clone());
        pane.terminal.status = ManagedTerminalStatus::Error(Arc::from(message));
    }
    if let Some(message) = error {
        ctx.toast().push(error_toast(format!("Pane {id}"), message));
        Update::full()
    } else {
        Update::none()
    }
}

pub(crate) fn handle_pane_resize(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    cols: u16,
    rows: u16,
) -> Update {
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        match pane.terminal.resize(cols, rows) {
            Ok(true) => Update::full(),
            Ok(false) => Update::none(),
            Err(message) => {
                let toast_message = message.clone();
                pane.terminal.status = ManagedTerminalStatus::Error(Arc::from(message));
                ctx.toast()
                    .push(error_toast(format!("Pane {id}"), toast_message));
                Update::full()
            }
        }
    } else {
        Update::none()
    }
}

pub(crate) fn handle_pane_scroll(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    offset: usize,
) -> Update {
    if let Some(pane) = find_pane_mut(&mut ctx.state, id)
        && pane.terminal.set_scrollback(offset)
    {
        return Update::full();
    }
    Update::none()
}
```

- [ ] **Step 2: Move pane lifecycle functions**

Replace `src/pane_lifecycle.rs` imports with imports needed by the moved functions. Start with this set and let rust-analyzer/cargo remove unused entries:

```rust
use std::time::Duration;

use tui_lipan::prelude::*;

use crate::anim::GeometryAnimation;
use crate::focus_ops::{choose_fallback_focus_near, first_visible_pane, focus_pane, request_current_pane_focus};
use crate::layout::place_spawned_pane;
use crate::pane::TerminalPane;
use crate::pty_events::{error_toast, info_toast};
use crate::state::{HyprmuxConfig, Pane, PaneId, State};
use crate::tiling::{append_tiled_window, remove_tiled_window};
use crate::{HyprmuxApp, Msg};
```

Move these functions from `src/main.rs` into `src/pane_lifecycle.rs` and mark them `pub(crate)`:

```text
spawn_pane
begin_close_pane
close_focused_pane
remove_pane
total_visible_panes
initial_command
spawn_pty_command
spawn_pty
prune_closed_command
pty_config
```

Move `find_pane_mut` here or in `focus_ops.rs`, but expose it from exactly one module as:

```rust
pub(crate) fn find_pane_mut(state: &mut State, id: PaneId) -> Option<&mut Pane>
```

If `find_pane_mut` is placed in `pane_lifecycle.rs`, update imports in `focus_ops`, `search_ops`, `pty_events`, and `update` to use `crate::pane_lifecycle::find_pane_mut`.

- [ ] **Step 3: Update main initialization helpers**

In `src/main.rs`, change initialization references to module-qualified names:

```rust
ctx.toast().push(pty_events::info_toast(message));
ctx.toast().push(pty_events::error_toast(
    "Theme Watcher",
    format!("Could not watch {}: {err}", path.display()),
));
let spawn = ctx
    .state
    .focused_pane
    .map(|id| (id, pane_lifecycle::pty_config(&ctx.state.config), Some(Duration::ZERO)));
pane_lifecycle::initial_command(spawn, ctx.state.theme_watcher.is_some())
```

- [ ] **Step 4: Update dispatcher imports and match arms**

In `src/update.rs`, import from `pane_lifecycle` and `pty_events` exactly where used. Keep these message arms thin:

```rust
Msg::PtyEvent(id, event) => handle_pty_event(ctx, id, event),
Msg::PaneInput(id, input) => handle_pane_input(ctx, id, input),
Msg::PaneMouse(id, bytes) => handle_pane_mouse(ctx, id, bytes),
Msg::PaneResize(id, cols, rows) => handle_pane_resize(ctx, id, cols, rows),
Msg::PaneScroll(id, offset) => handle_pane_scroll(ctx, id, offset),
```

- [ ] **Step 5: Verify PTY/lifecycle extraction**

Run:

```bash
cargo test
```

Expected: all tests pass.

---

### Task 6: Extract focus and move/resize operations

**Files:**
- Modify: `src/main.rs:936-1333,1411-1893,1955-1988`
- Modify: `src/focus_ops.rs`
- Modify: `src/resize_move_ops.rs`
- Modify: `src/update.rs`
- Modify: `src/actions.rs`
- Modify: `src/key_routing.rs`

**Interfaces:**
- Consumes: geometry/layout/tiling helpers and state types.
- Produces:
  - `focus_ops::{focus_in_direction, cycle_focus_id, switch_workspace, move_focused_to_workspace, focus_pane, choose_fallback_focus, choose_fallback_focus_near, first_visible_pane, focus_near_pane_in_workspace, visible_pane_placements, reference_pane_rect, active_pane_mut, find_pane_mut, request_pane_focus, request_current_pane_focus, request_search_focus, request_theme_picker_focus, total_visible_panes}`
  - `resize_move_ops::{begin_move, move_pane, end_move, begin_resize, resize_pane, resize_pane_state, drop_tiled_pane_at, toggle_tiling, toggle_fullscreen, toggle_focused_split_axis, adjust_focused_split_ratio, toggle_layout, move_focused_in_direction, resize_focused_in_direction}`

- [ ] **Step 1: Move focus operations**

Replace `src/focus_ops.rs` imports with the moved-function dependencies:

```rust
use tui_lipan::prelude::*;

use crate::geometry::{closest_pane_to_rect, directional_score};
use crate::layout::workspace_target_rects;
use crate::state::{Direction, Pane, PaneId, State, Workspace};
use crate::view;
use crate::HyprmuxApp;
```

Move these functions from `src/main.rs` into `src/focus_ops.rs` and mark them `pub(crate)`:

```text
directional_neighbor
split_axis_for_direction
active_pane_is_fullscreen
focus_in_direction
cycle_focus_id
switch_workspace
move_focused_to_workspace
focus_pane
choose_fallback_focus
choose_fallback_focus_near
first_visible_pane
focus_near_pane_in_workspace
visible_pane_placements
reference_pane_rect
active_pane_mut
find_pane_mut
total_visible_panes
request_pane_focus
request_current_pane_focus
request_search_focus
request_theme_picker_focus
```

If `find_pane_mut` was placed in `pane_lifecycle.rs` in Task 5, do not duplicate it here; import it from `pane_lifecycle` where needed.

- [ ] **Step 2: Move move/resize operations**

Replace `src/resize_move_ops.rs` imports with the moved-function dependencies:

```rust
use tui_lipan::prelude::*;

use crate::anim::GeometryAnimation;
use crate::focus_ops::{active_pane_is_fullscreen, choose_fallback_focus_near, find_pane_mut, focus_pane, reference_pane_rect, request_current_pane_focus};
use crate::geometry::{canvas_bounds_from_viewport, canvas_local_point_from_mouse, clamp_float_rect, clamp_floating_rect, default_floating_rect, grabbed_edge_on_outer_border, lift_off_float_rect, resize_float_rect_from_corner, tiled_drag_preview_rect};
use crate::layout::{insert_tiled_pane_at_point, target_tiled_pane_for_drop, workspace_target_rects, workspace_target_rects_excluding};
use crate::state::{Direction, LayoutKind, MoveSession, OUTER_GAP, PaneId, RATIO_STEP, ResizeCorner, ResizeSession, State, TILE_GAP};
use crate::tiling::{adjust_ratio_value, adjust_tree_split_for_focused, allocate_dwindle, flip_tree_split_for_focused, focused_is_first_in_nearest_axis_split, move_tiled_window_around_target, nearest_split_available, ratio_at, resize_tiled_split};
use crate::HyprmuxApp;
```

Move these functions from `src/main.rs` into `src/resize_move_ops.rs` and mark them `pub(crate)`:

```text
begin_move
move_pane
end_move
begin_resize
resize_pane
resize_pane_state
drop_tiled_pane_at
toggle_tiling
toggle_fullscreen
toggle_focused_split_axis
adjust_focused_split_ratio
toggle_layout
adjust_master_split_for_focused
resize_master_split_by_pixels
master_available_width
move_focused_in_direction
resize_focused_in_direction
keyboard_resize_pixels
```

- [ ] **Step 3: Move the keyboard resize test**

Move the `#[cfg(test)] mod tests` block from `src/main.rs:1955-1988` to the bottom of `src/resize_move_ops.rs`. Keep the test name unchanged:

```rust
keyboard_resize_directions_grow_toward_the_nearest_split
```

- [ ] **Step 4: Clean root imports**

Remove imports from `src/main.rs` that are no longer used after extraction. Keep these categories only if still used by `main.rs`:

```rust
use std::time::Duration;
use tui_lipan::prelude::*;
use crate::anim::{GeometryAnimation, WindowAnimationConfig};
use crate::state::{HyprmuxConfig, Pane, ResizeCorner, State};
```

Let `cargo check` identify the exact unused imports and remove them one by one.

- [ ] **Step 5: Verify full hyprmux extraction**

Run:

```bash
cargo fmt
cargo test
cargo clippy
git diff --check
```

Expected: formatting completes, tests pass, clippy exits successfully, and `git diff --check` prints no output.

- [ ] **Step 6: Commit gate**

Run:

```bash
git status --short
git diff --stat
```

If commit permission exists:

```bash
git add src/main.rs src/actions.rs src/focus_ops.rs src/key_routing.rs src/pane_lifecycle.rs src/pty_events.rs src/resize_move_ops.rs src/search_ops.rs src/theme_ops.rs src/update.rs
git commit -m "refactor: split hyprmux app operations"
```

If commit permission does not exist, leave the files uncommitted.

---

### Task 7: Wire message dispatch through `update.rs`

**Files:**
- Modify: `src/main.rs:142-333`
- Modify: `src/update.rs`

**Interfaces:**
- Consumes: operation functions extracted in Tasks 3 through 6.
- Produces: `main.rs` root `Component::update` delegates to `update::handle_msg`.

- [ ] **Step 1: Replace dispatcher shell imports**

Replace `src/update.rs` imports with:

```rust
use std::sync::Arc;

use tui_lipan::prelude::*;

use crate::actions::execute_action;
use crate::anim::GeometryAnimation;
use crate::focus_ops::{focus_pane, request_current_pane_focus, request_pane_focus, total_visible_panes};
use crate::key_routing::handle_key_routing;
use crate::pane_lifecycle::{find_pane_mut, remove_pane};
use crate::pty_events::{error_toast, handle_pane_input, handle_pane_mouse, handle_pane_resize, handle_pane_scroll, handle_pty_event};
use crate::resize_move_ops::{begin_move, begin_resize, end_move, move_pane, resize_pane};
use crate::search_ops::{recompute_search, search_next};
use crate::state::ThemePreset;
use crate::theme_ops::{select_theme, theme_tick};
use crate::{FrameworkFocus, HyprmuxApp, Msg};
```

- [ ] **Step 2: Move the `Msg` match body**

Move the full existing `match msg { ... }` body from `Component::update` in `src/main.rs:142-333` into `update::handle_msg`. Preserve every match arm body exactly except for these helper-name changes:

```rust
Msg::ThemeTick => theme_tick(ctx),
Msg::ThemeError(message) => {
    ctx.toast().push(error_toast("Theme Reload", message));
    Update::full()
}
Msg::PaneInput(id, input) => handle_pane_input(ctx, id, input),
Msg::PaneMouse(id, bytes) => handle_pane_mouse(ctx, id, bytes),
Msg::PaneResize(id, cols, rows) => handle_pane_resize(ctx, id, cols, rows),
Msg::PaneScroll(id, offset) => handle_pane_scroll(ctx, id, offset),
```

Keep all other logic from the original arms unchanged.

- [ ] **Step 3: Replace `Component::update`**

Change `src/main.rs` update method to exactly:

```rust
fn update(&mut self, msg: Self::Message, ctx: &mut Context<Self>) -> Update {
    update::handle_msg(self, msg, ctx)
}
```

- [ ] **Step 4: Verify root shell still behaves**

Run:

```bash
cargo fmt
cargo test
cargo clippy
git diff --check
```

Expected: formatting completes, tests pass, clippy exits successfully, and diff check prints no output.

- [ ] **Step 5: Commit gate**

Run:

```bash
git diff -- src/main.rs src/update.rs
```

If commit permission exists:

```bash
git add src/main.rs src/update.rs
git commit -m "refactor: delegate message dispatch"
```

If commit permission does not exist, leave the files uncommitted.

---

### Task 8: Add tui-lipan large-app diagnostics docs

**Files:**
- Create: `/home/razuer/Work/Projects/tui-lipan/docs/large-app-shells.md`
- Modify: `/home/razuer/Work/Projects/tui-lipan/docs/index.md`
- Optional modify: `/home/razuer/Work/Projects/tui-lipan/docs/patterns.md`

**Interfaces:**
- Consumes: existing tui-lipan docs for components, tutorial, focus, patterns, terminal widget, and `examples/window_manager.rs`.
- Produces: reusable framework documentation only. No hyprmux-specific framework API.

- [ ] **Step 1: Inspect framework repo status**

Run:

```bash
git status --short
```

from `/home/razuer/Work/Projects/tui-lipan`.

Expected: note any unrelated changes and do not edit those files unless this task names them.

- [ ] **Step 2: Create large app shell guide**

Create `/home/razuer/Work/Projects/tui-lipan/docs/large-app-shells.md` with this content:

```markdown
# Large App Shells

Large tui-lipan apps are easiest to diagnose when the root `Component` stays thin and app behavior lives in named operation modules.

## Recommended shape

- Keep the root app type, `Message` enum, `Component` implementation, and bootstrap code together.
- Put the `Message` match in a dispatcher module when it grows beyond a short screen of code.
- Put app-owned behavior in operation modules named after behavior: `actions`, `key_routing`, `search`, `theme`, `focus`, or domain-specific names.
- Keep view code as the widget/callback boundary. It should build elements and emit messages, not mutate app policy directly.
- Keep reusable chrome as helper functions or composite widgets. Use nested `Component`s only when the child owns state, lifecycle, keyboard handling, or async work.

## Message dispatch

The root component can delegate update logic without hiding the tui-lipan lifecycle:

```rust
impl Component for App {
    type Message = Msg;
    type Properties = ();
    type State = State;

    fn update(&mut self, msg: Self::Message, ctx: &mut Context<Self>) -> Update {
        update::handle_msg(self, msg, ctx)
    }

    fn view(&self, ctx: &Context<Self>) -> Element {
        view::render(self, ctx)
    }
}
```

The dispatcher should route messages to narrow functions. Long behavior bodies belong in app modules, not in the root component.

## Input routing

Apps with global shortcuts and focusable children should make the input sources explicit:

- `Component::on_key` receives keys that bubble to the root.
- Widget callbacks receive keys consumed by focused widgets.
- Both paths can call the same app-owned routing function when they share policy.

Use `ctx.request_focus(...)` and stable element keys for focus handoff. Return `KeyUpdate::handled(...)` only when the app consumed the key.

## Terminal apps

Terminal widgets expose low-level control. A terminal-heavy app should keep these concerns visible:

- PTY readiness and output event handling.
- Terminal input forwarding.
- Terminal resize side effects.
- Scrollback synchronization.
- App theme to terminal palette mapping.

These policies are usually app-owned. Move them to focused app modules before considering a framework abstraction.

## Diagnostics checklist

When debugging a large app, locate the bug by boundary:

| Symptom | First place to inspect |
| --- | --- |
| Message has no effect | Message dispatcher and operation module |
| Shortcut works only in some widgets | Root `on_key`, widget callback, and focus bubbling |
| Focus jumps to the wrong element | Stable keys and `ctx.request_focus(...)` call site |
| Widget renders correctly but app state is stale | Callback-to-message wiring in the view boundary |
| Terminal receives wrong bytes | App terminal input forwarding |
| Terminal layout or resize is wrong | App geometry policy before framework layout primitives |
| Async result applies out of order | Command key and `TaskPolicy` choice |

## Reference example

`examples/window_manager.rs` demonstrates a large root app with canvas composition, focus routing, drag/resize behavior, workspace switching, and terminal composition. Treat it as a reference for boundaries and event flow rather than as a generic framework abstraction.
```

- [ ] **Step 3: Link the guide from docs index**

Edit `/home/razuer/Work/Projects/tui-lipan/docs/index.md` and add a bullet in the docs list:

```markdown
- [Large app shells](large-app-shells.md) - structure and diagnostics for multi-pane, root-routed apps.
```

Place it near existing component/tutorial/pattern links.

- [ ] **Step 4: Link from patterns if the file has guide links**

If `/home/razuer/Work/Projects/tui-lipan/docs/patterns.md` has a guide list or related-docs section, add:

```markdown
For larger root-routed applications, see [Large app shells](large-app-shells.md).
```

If there is no related-docs section, skip this step without creating one.

- [ ] **Step 5: Verify docs diff**

Run from `/home/razuer/Work/Projects/tui-lipan`:

```bash
git diff --check
git diff -- docs/large-app-shells.md docs/index.md docs/patterns.md
```

Expected: no whitespace errors; diff contains only the guide and links.

- [ ] **Step 6: Framework commit gate**

Run:

```bash
git status --short
```

If commit permission exists:

```bash
git add docs/large-app-shells.md docs/index.md docs/patterns.md
git commit -m "docs: add large app shell guidance"
```

If `docs/patterns.md` was not changed, omit it from `git add`. If commit permission does not exist, leave the docs uncommitted.

---

### Task 9: Final verification and handoff summary

**Files:**
- Modify only if verification reveals import cleanup is needed: hyprmux source files from earlier tasks.
- No planned new files.

**Interfaces:**
- Consumes: completed hyprmux refactor and tui-lipan docs.
- Produces: final verified working tree state and concise handoff.

- [ ] **Step 1: Verify hyprmux**

Run from `/home/razuer/Work/Projects/hyprmux`:

```bash
cargo fmt
cargo test
cargo clippy
git diff --check
git status --short
```

Expected: format completes, tests pass, clippy passes, diff check has no output, and status lists only intended files.

- [ ] **Step 2: Verify tui-lipan docs**

Run from `/home/razuer/Work/Projects/tui-lipan`:

```bash
git diff --check
git status --short
```

Expected: diff check has no output, and status lists only intended docs files plus any unrelated pre-existing files noted in Task 7 Step 1.

- [ ] **Step 3: Inspect final diffs**

Run:

```bash
git diff --stat
```

from both repositories.

Expected hyprmux diff: `src/main.rs` shrinks; new operation modules contain moved behavior; no unrelated app files changed except formatting caused by `cargo fmt` on touched files.

Expected tui-lipan diff: documentation-only changes.

- [ ] **Step 4: Final response**

Report:

```text
Implemented the app-structure refactor and framework docs.

Verification:
- hyprmux: cargo fmt, cargo test, cargo clippy, git diff --check
- tui-lipan: git diff --check

Changed:
- hyprmux: root Component shell plus operation modules
- tui-lipan: large app shell diagnostics docs
```

If a command failed, report the failed command and exact failure instead of saying the work is complete.
