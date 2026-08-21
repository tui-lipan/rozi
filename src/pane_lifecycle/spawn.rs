use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::anim::{self, GeometryAnimation};
use crate::geometry::{canvas_local_point_from_mouse, clamp_float_rect, default_floating_rect};
use crate::layout::place_spawned_pane;
use crate::ops::focus::request_pane_focus;
use crate::ops::theme::pane_frame_background;
use crate::pane_lifecycle::namespace::{find_pane, find_pane_mut, pane_env};
use crate::pane_lifecycle::timers::open_timers_command;
use crate::state::{Pane, PaneId, PaneIdentity, ScrollableRevealEdge, State};
use crate::tiling::remove_tiled_window;

/// The focused pane's local working directory, if it has a usable one.
///
/// Anything spawned *from* a pane — a split, a `[keys]`/sidebar `run` command, a popup — opens where
/// that pane is, which is almost never where the session server was started. A remote SSH path is
/// never returned: it does not name a directory on this machine. When the client is `--remote`
/// attached, every server cwd is treated as non-local even if `cwd_host` is unset.
pub(crate) fn focused_local_cwd(state: &State) -> Option<String> {
    focused_local_cwd_ref(state).map(str::to_string)
}

/// [`focused_local_cwd`] without the allocation. See [`Pane::local_cwd_ref`].
pub(crate) fn focused_local_cwd_ref(state: &State) -> Option<&str> {
    if state.current().remote_host.is_some() {
        return None;
    }
    let workspace = state.active_workspace_ref();
    workspace
        .focused_pane
        .and_then(|id| workspace.panes.iter().find(|pane| pane.id == id))
        .and_then(|pane| pane.local_cwd_ref())
}

/// The focused pane's directory as the session server sees it, without allocating.
///
/// Local attach: the same thing [`focused_local_cwd_ref`] returns. Remote attach: the
/// server-relative path, which is what the sidebar file tree roots itself at and asks the server to
/// list. See [`Pane::server_cwd_ref`].
pub(crate) fn focused_server_cwd_ref(state: &State) -> Option<&str> {
    if state.current().remote_host.is_none() {
        return focused_local_cwd_ref(state);
    }
    let workspace = state.active_workspace_ref();
    workspace
        .focused_pane
        .and_then(|id| workspace.panes.iter().find(|pane| pane.id == id))
        .and_then(|pane| pane.server_cwd_ref())
}

/// Cwd to send with a server spawn request. Under `--remote`, inherits the server-relative path.
pub(crate) fn focused_spawn_cwd(state: &State) -> Option<String> {
    if state.current().remote_host.is_some() {
        let workspace = state.active_workspace_ref();
        return workspace
            .focused_pane
            .and_then(|id| workspace.panes.iter().find(|pane| pane.id == id))
            .and_then(|pane| {
                pane.terminal
                    .working_directory()
                    .or_else(|| pane.identity.cwd.clone())
            });
    }
    focused_local_cwd(state)
}

pub(crate) fn spawn_pane(ctx: &mut Context<AppRoot>) -> Update {
    if ctx.state.scratch_visible {
        let previous_focused = ctx.state.scratch.focused_pane;
        let identity = PaneIdentity {
            cwd: focused_spawn_cwd(&ctx.state),
            ..PaneIdentity::default()
        };
        return spawn_pane_in_scratch(ctx, previous_focused, identity).1;
    }
    let previous_focused =
        ctx.state.current().workspaces[ctx.state.current().active_workspace].focused_pane;
    let identity = PaneIdentity {
        cwd: focused_spawn_cwd(&ctx.state),
        ..PaneIdentity::default()
    };

    spawn_interactive_pane(
        ctx,
        ctx.state.current().active_workspace,
        previous_focused,
        identity,
    )
    .1
}

/// Spawn a new pane already floating, centered on the last mouse pointer.
///
/// Falls back to a centered float when no pointer has been seen this run (no mouse event yet).
/// Scratchpad-visible spawns float inside the dropdown.
pub(crate) fn spawn_floating_pane_at_cursor(ctx: &mut Context<AppRoot>) -> Update {
    let identity = PaneIdentity {
        cwd: focused_spawn_cwd(&ctx.state),
        ..PaneIdentity::default()
    };
    let float = SpawnFloat {
        width: DEFAULT_FLOAT_FRACTION,
        height: DEFAULT_FLOAT_FRACTION,
        position: crate::config::FloatPosition::Cursor,
        pointer: pointer_canvas_origin(ctx),
    };
    if ctx.state.scratch_visible {
        let previous_focused = ctx.state.scratch.focused_pane;
        let previous_anchor = ctx.state.scratch.scrollable_anchor;
        let (id, update) = spawn_pane_in_scratch(ctx, previous_focused, identity);
        let bounds = crate::scratchpad::deployed_rect(&ctx.state, ctx.viewport());
        if let Some(pane) = ctx
            .state
            .scratch
            .panes
            .iter_mut()
            .find(|pane| pane.id == id)
        {
            pane.floating = true;
            pane.floating_rect = float.rect(bounds);
        }
        remove_tiled_window(&mut ctx.state.scratch, id);
        if ctx.state.scratch.scrollable_anchor == Some(id) {
            ctx.state.scratch.scrollable_anchor = previous_anchor;
        }
        return update;
    }
    let previous_focused =
        ctx.state.current().workspaces[ctx.state.current().active_workspace].focused_pane;
    spawn_pane_in_workspace(
        ctx,
        ctx.state.current().active_workspace,
        previous_focused,
        identity,
        SpawnPlacement {
            float: Some(float),
            fullscreen: false,
            focus: true,
        },
    )
    .1
}

pub(crate) fn spawn_pane_in_scratch(
    ctx: &mut Context<AppRoot>,
    previous_focused: Option<PaneId>,
    identity: PaneIdentity,
) -> (PaneId, Update) {
    let initial_pane = ctx.state.scratch.panes.is_empty();
    let tile_gap = ctx.state.tile_gap();
    let split_width_multiplier = ctx.state.config.layout.split_width_multiplier;
    let rect = crate::scratchpad::deployed_rect(&ctx.state, ctx.viewport());
    let id = ctx.state.next_scratch_pane_id;
    ctx.state.next_scratch_pane_id = id.saturating_add(1);
    let generation = ctx.state.next_scratch_pty_generation;
    ctx.state.next_scratch_pty_generation = generation.saturating_add(1);
    let mut pane = Pane::new(id, ctx.state.config.scrollback, rect);
    pane.pty_generation = generation;
    pane.identity = identity;
    pane.terminal.bind_server_backend(id, generation);
    let palette = TerminalColorPalette::from_theme(
        &ctx.state.theme,
        pane_frame_background(
            &ctx.state.theme,
            true,
            ctx.state.config.pane.highlight_focused_background,
        ),
    );
    pane.terminal.set_palette(palette);
    // The initial pane rides the dropdown slide, preserving the original scratch animation.
    // Additional panes use the ordinary pane open transition inside the deployed workspace.
    pane.opening = !initial_pane;
    pane.terminal_active = initial_pane;
    let env = pane_env(ctx.state.control_socket_path.as_deref(), &pane, false);
    let request = PaneSpawnRequest {
        pane_id: id,
        local: true,
        generation,
        identity: pane.identity.clone(),
        cols: pane.terminal.cols,
        rows: pane.terminal.rows,
        env,
        palette,
    };
    ctx.state.scratch.panes.push(pane);
    let tiled_placement = place_spawned_pane(
        &mut ctx.state.scratch,
        id,
        previous_focused,
        rect,
        0.0,
        tile_gap,
        split_width_multiplier,
    );
    let slide_edge = tiled_placement.slide_edge(ctx.state.scratch.layout_kind);
    if let Some(pane) = ctx
        .state
        .scratch
        .panes
        .iter_mut()
        .find(|pane| pane.id == id)
    {
        pane.slide_edge = slide_edge;
    }
    ctx.state.scratch.focused_pane = Some(id);
    // A workspace spawn routes focus through `apply_spawn_focus`, which also parks the Scrollable
    // viewport on the new pane. Without the same here the anchor stays on whatever was focused
    // first, so switching the dropdown to Scrollable later reveals that pane instead of this one.
    if ctx.state.scratch.tiled_ids().contains(&id) {
        set_scrollable_anchor_for_spawned(&mut ctx.state.scratch, id);
    }
    ctx.state.animation = GeometryAnimation::Spawn;
    request_pane_spawn(&mut ctx.state, request);
    request_pane_focus(ctx, id);
    let update = if initial_pane {
        Update::full()
    } else {
        Update::with_command(open_timers_command(
            ctx.state.runtime_epoch,
            id,
            generation,
            anim::open_delay(ctx.state.config.animations),
            anim::activation_delay(ctx.state.config.animations),
        ))
    };
    (id, update)
}

/// Fraction of the pane canvas used when spawning a floating pane at the pointer.
pub(crate) const DEFAULT_FLOAT_FRACTION: f32 = 0.42;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SpawnFloat {
    pub width: f32,
    pub height: f32,
    pub position: crate::config::FloatPosition,
    /// Canvas-space pointer used as the pane's center when [`Self::position`] is
    /// [`crate::config::FloatPosition::Cursor`].
    pub pointer: Option<(f32, f32)>,
}

impl SpawnFloat {
    pub(crate) fn rect(self, bounds: FloatRect) -> FloatRect {
        let w = bounds.w * self.width;
        let h = bounds.h * self.height;
        let center = (
            bounds.x + (bounds.w - w) / 2.0,
            bounds.y + (bounds.h - h) / 2.0,
        );
        let (x, y) = match self.position {
            crate::config::FloatPosition::Center => center,
            crate::config::FloatPosition::Cursor => self
                .pointer
                .map(|(px, py)| (px - w / 2.0, py - h / 2.0))
                .unwrap_or(center),
            crate::config::FloatPosition::TopLeft => (bounds.x, bounds.y),
            crate::config::FloatPosition::Top => (bounds.x + (bounds.w - w) / 2.0, bounds.y),
            crate::config::FloatPosition::TopRight => (bounds.x + bounds.w - w, bounds.y),
            crate::config::FloatPosition::Left => (bounds.x, bounds.y + (bounds.h - h) / 2.0),
            crate::config::FloatPosition::Right => {
                (bounds.x + bounds.w - w, bounds.y + (bounds.h - h) / 2.0)
            }
            crate::config::FloatPosition::BottomLeft => (bounds.x, bounds.y + bounds.h - h),
            crate::config::FloatPosition::Bottom => {
                (bounds.x + (bounds.w - w) / 2.0, bounds.y + bounds.h - h)
            }
            crate::config::FloatPosition::BottomRight => {
                (bounds.x + bounds.w - w, bounds.y + bounds.h - h)
            }
        };
        clamp_float_rect(FloatRect { x, y, w, h }, bounds)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SpawnPlacement {
    pub float: Option<SpawnFloat>,
    pub fullscreen: bool,
    pub focus: bool,
}

impl Default for SpawnPlacement {
    fn default() -> Self {
        Self {
            float: None,
            fullscreen: false,
            focus: true,
        }
    }
}

/// Canvas-space point under the last mouse pointer, or `None` when no pointer has been seen.
pub(crate) fn pointer_canvas_origin(ctx: &Context<AppRoot>) -> Option<(f32, f32)> {
    let (x, y) = ctx.last_mouse()?;
    let bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());
    Some(canvas_local_point_from_mouse(
        x,
        y,
        bounds,
        ctx.state.terminal_content_left_offset(ctx.viewport()),
        ctx.state.content_top_offset(),
    ))
}

pub(crate) fn spawn_interactive_pane(
    ctx: &mut Context<AppRoot>,
    source_workspace: usize,
    source: Option<PaneId>,
    identity: PaneIdentity,
) -> (PaneId, Update) {
    spawn_interactive_pane_with_focus(ctx, source_workspace, source, identity, None, None)
}

/// Spawn like [`spawn_interactive_pane`], but let the caller decide whether the new pane takes
/// focus instead of the matched `[[rules]]` entry. `None` keeps the rule's answer.
///
/// The control endpoint passes `Some(false)` for an ordinary `new-pane`: a pane spawned by an
/// agent must not move focus (and the active workspace) out from under whoever is typing, which
/// would send their next keystrokes somewhere they never looked. A rule's `focus` still describes
/// what an interactive spawn of that command should do, so the override applies only to the
/// automation path and leaves workspace/float/fullscreen placement alone.
pub(crate) fn spawn_interactive_pane_with_focus(
    ctx: &mut Context<AppRoot>,
    source_workspace: usize,
    source: Option<PaneId>,
    identity: PaneIdentity,
    focus: Option<bool>,
    workspace: Option<usize>,
) -> (PaneId, Update) {
    let rule_command = identity
        .launch
        .as_ref()
        .map(crate::pane_launch::PaneLaunch::display);
    let (workspace_index, previous_focused, placement) = interactive_spawn_target(
        &ctx.state,
        source_workspace,
        source,
        rule_command.as_deref(),
        focus,
        workspace,
    );
    spawn_pane_in_workspace(ctx, workspace_index, previous_focused, identity, placement)
}

pub(crate) fn interactive_spawn_target(
    state: &State,
    source_workspace: usize,
    source: Option<PaneId>,
    command: Option<&str>,
    focus: Option<bool>,
    workspace: Option<usize>,
) -> (usize, Option<PaneId>, SpawnPlacement) {
    let (rule_workspace, mut placement) = command
        .map(|command| crate::rules::placement_for_command(&state.config.rules, command))
        .unwrap_or_default();
    if let Some(focus) = focus {
        placement.focus = focus;
    }
    // A caller that named a workspace means it: `[[rules]]` placement is a default for the panes a
    // person opens, not an override of an explicit instruction from automation.
    let workspace_index = workspace.or(rule_workspace).unwrap_or(source_workspace);
    let previous_focused = source.or(state.current().workspaces[workspace_index].focused_pane);
    (workspace_index, previous_focused, placement)
}

pub(crate) fn spawn_pane_in_workspace(
    ctx: &mut Context<AppRoot>,
    workspace_index: usize,
    previous_focused: Option<PaneId>,
    identity: PaneIdentity,
    placement: SpawnPlacement,
) -> (PaneId, Update) {
    let bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());
    let top_gap = ctx.state.workspace_top_gap();
    let tile_gap = ctx.state.tile_gap();
    let split_width_multiplier = ctx.state.config.layout.split_width_multiplier;
    let id = ctx.state.current().next_pane_id;
    ctx.state.current_mut().next_pane_id = ctx.state.current_mut().next_pane_id.saturating_add(1);
    let generation = ctx.state.current().next_pty_generation;
    ctx.state.current_mut().next_pty_generation = ctx
        .state
        .current_mut()
        .next_pty_generation
        .saturating_add(1);
    // A fullscreen pane covers the whole workspace, so a plain tiled spawn would hand the focus to
    // a pane nobody can see and leave keystrokes going somewhere invisible. Pass the fullscreen on
    // to the new pane instead, so the focused pane stays the visible one; leaving fullscreen (the
    // other option) would instead yank a layout the user deliberately set up. A spawn that does not
    // take focus must not take the screen either — it lands in the tree behind the fullscreen pane.
    let takes_over_fullscreen = placement.focus
        && placement.float.is_none()
        && workspace_has_fullscreen(&ctx.state, workspace_index);
    let floating_rect = default_floating_rect(bounds, id);
    let mut pane = Pane::new(id, ctx.state.config.scrollback, floating_rect);
    pane.pty_generation = generation;
    pane.terminal.bind_server_backend(id, generation);
    pane.identity = identity;
    pane.fullscreen = placement.fullscreen || takes_over_fullscreen;
    if let Some(mut float) = placement.float {
        if float.position == crate::config::FloatPosition::Cursor && float.pointer.is_none() {
            float.pointer = pointer_canvas_origin(ctx);
        }
        pane.floating = true;
        pane.floating_rect = float.rect(bounds);
    }
    let palette = TerminalColorPalette::from_theme(
        &ctx.state.theme,
        pane_frame_background(
            &ctx.state.theme,
            true,
            ctx.state.config.pane.highlight_focused_background,
        ),
    );
    pane.terminal.set_palette(palette);
    pane.opening = true;

    let env = pane_env(
        ctx.state.control_socket_path.as_deref(),
        &pane,
        ctx.state.current().remote_host.is_some(),
    );
    let identity = pane.identity.clone();
    let command = pane
        .identity
        .launch
        .as_ref()
        .map(crate::pane_launch::PaneLaunch::display);
    let cwd = pane.identity.cwd.clone();
    let cols = pane.terminal.cols;
    let rows = pane.terminal.rows;

    let fullscreen = pane.fullscreen;
    let workspace = &mut ctx.state.current_mut().workspaces[workspace_index];
    if fullscreen {
        // At most one pane per workspace is fullscreen: two would stack, and which one you saw
        // would come down to render order. The new pane is not in `panes` yet.
        for other in &mut workspace.panes {
            other.fullscreen = false;
        }
    }
    workspace.panes.push(pane);
    let tiled_placement = place_spawned_pane(
        workspace,
        id,
        previous_focused,
        bounds,
        top_gap,
        tile_gap,
        split_width_multiplier,
    );
    let slide_edge = tiled_placement.slide_edge(workspace.layout_kind);
    if let Some(pane) = workspace.panes.iter_mut().find(|pane| pane.id == id) {
        pane.slide_edge = slide_edge;
    }
    if placement.float.is_some() {
        remove_tiled_window(workspace, id);
    }
    apply_spawn_focus(&mut ctx.state, workspace_index, id, placement);
    ctx.state.animation = GeometryAnimation::Spawn;
    let open_delay = anim::open_delay(ctx.state.config.animations);
    let activate_delay = anim::activation_delay(ctx.state.config.animations);

    request_pane_spawn(
        &mut ctx.state,
        PaneSpawnRequest {
            pane_id: id,
            local: false,
            generation,
            identity,
            cols,
            rows,
            env,
            palette,
        },
    );
    crate::events::emit(
        &ctx.state,
        crate::events::Event::new(
            crate::events::EventKind::PaneSpawned,
            vec![
                ("pane", id.to_string()),
                ("workspace", (workspace_index + 1).to_string()),
                ("command", command.unwrap_or_default()),
                ("cwd", cwd.unwrap_or_default()),
            ],
        ),
    );
    let update = Update::with_command(open_timers_command(
        ctx.state.runtime_epoch,
        id,
        generation,
        open_delay,
        activate_delay,
    ));
    (id, update)
}

/// Whether a live pane in `workspace_index` currently covers the workspace.
pub(crate) fn workspace_has_fullscreen(state: &State, workspace_index: usize) -> bool {
    state.current().workspaces[workspace_index]
        .panes
        .iter()
        .any(|pane| pane.fullscreen && !pane.closing)
}

pub(crate) fn apply_spawn_focus(
    state: &mut State,
    workspace_index: usize,
    id: PaneId,
    placement: SpawnPlacement,
) {
    let active_workspace = state.current().active_workspace;
    let global_focus = state.current().focused_pane;
    let spawned_tiled = state.current().workspaces[workspace_index]
        .panes
        .iter()
        .any(|pane| pane.id == id && !pane.floating && !pane.closing);

    if placement.focus {
        state.current_mut().active_workspace = workspace_index;
        state.current_mut().focused_pane = Some(id);
        state.current_mut().workspaces[workspace_index].focused_pane = Some(id);
        if spawned_tiled {
            set_scrollable_anchor_for_spawned(
                &mut state.current_mut().workspaces[workspace_index],
                id,
            );
        }
        return;
    }

    if workspace_index != active_workspace {
        // Inactive workspace may remember the new pane without stealing the visible focus.
        state.current_mut().workspaces[workspace_index].focused_pane = Some(id);
        if spawned_tiled {
            set_scrollable_anchor_for_spawned(
                &mut state.current_mut().workspaces[workspace_index],
                id,
            );
        }
        return;
    }

    // Active workspace, focus=false: keep workspace + global focus where they are so render
    // styling/z-order and Scrollable viewport stay on the real focus.
    if !spawned_tiled {
        return;
    }
    let ws = &mut state.current_mut().workspaces[workspace_index];
    let anchor_still_valid = ws.scrollable_anchor.is_some_and(|anchor| {
        ws.panes
            .iter()
            .any(|pane| pane.id == anchor && !pane.floating && !pane.closing)
    });
    if anchor_still_valid {
        return;
    }
    let from_focused_tiled =
        [global_focus, ws.focused_pane]
            .into_iter()
            .flatten()
            .find(|&candidate| {
                candidate != id
                    && ws
                        .panes
                        .iter()
                        .any(|pane| pane.id == candidate && !pane.floating && !pane.closing)
            });
    let from_existing_tiled = ws
        .panes
        .iter()
        .find(|pane| pane.id != id && !pane.floating && !pane.closing)
        .map(|pane| pane.id);
    let anchor = from_focused_tiled
        .or(from_existing_tiled)
        .or(spawned_tiled.then_some(id));
    if let Some(anchor) = anchor {
        set_scrollable_anchor_for_spawned(ws, anchor);
    } else {
        ws.set_scrollable_viewport(None, ScrollableRevealEdge::Left);
    }
}

/// Spawn/close paths without a rendered visibility classify: first tiled → Left, else Right.
pub(crate) fn set_scrollable_anchor_for_spawned(ws: &mut crate::state::Workspace, id: PaneId) {
    let edge = if ws.tiled_ids().first() == Some(&id) {
        ScrollableRevealEdge::Left
    } else {
        ScrollableRevealEdge::Right
    };
    ws.set_scrollable_viewport(Some(id), edge);
}

pub(crate) fn respawn_focused_pane(ctx: &mut Context<AppRoot>) -> Update {
    let Some(id) = ctx.state.focused_pane() else {
        return Update::none();
    };
    if !find_pane(&ctx.state, id)
        .is_some_and(|pane| matches!(pane.terminal.status, ManagedTerminalStatus::Exited(_)))
    {
        return Update::none();
    }
    let generation = ctx.state.current().next_pty_generation;
    ctx.state.current_mut().next_pty_generation = generation.saturating_add(1);
    let control_socket = ctx.state.control_socket_path.clone();
    let remote_attached = ctx.state.current().remote_host.is_some();
    let palette = TerminalColorPalette::from_theme(
        &ctx.state.theme,
        pane_frame_background(
            &ctx.state.theme,
            true,
            ctx.state.config.pane.highlight_focused_background,
        ),
    );
    let (env, identity, cols, rows) = {
        let pane = find_pane_mut(&mut ctx.state, id).expect("focused exited pane still exists");
        pane.pty_generation = generation;
        pane.terminal.bind_server_backend(id, generation);
        pane.terminal.set_palette(palette);
        pane.activity = Default::default();
        // The replacement server pane starts without a log handle; the server broadcasts the
        // stop to other clients, this clears the local badge immediately.
        pane.logging = false;
        (
            pane_env(control_socket.as_deref(), pane, remote_attached),
            pane.identity.clone(),
            pane.terminal.cols,
            pane.terminal.rows,
        )
    };
    let local = crate::scratchpad::contains(&ctx.state, id);
    request_pane_spawn(
        &mut ctx.state,
        PaneSpawnRequest {
            pane_id: id,
            local,
            generation,
            identity,
            cols,
            rows,
            env,
            palette,
        },
    );
    crate::ops::session::schedule_layout_commit(ctx);
    Update::full()
}

/// One pane spawn addressed to the session server.
///
/// The launch fields travel as the pane's own [`PaneIdentity`] rather than as loose arguments so a
/// caller cannot spawn a pane whose wire request disagrees with the identity it just stored. That
/// is not hypothetical: `keep_open` and `replay` were adjacent positional `bool`s, and the popup
/// path passed a literal `false` for `keep_open` while its identity asked to hold the pane open.
pub(crate) struct PaneSpawnRequest {
    pub pane_id: PaneId,
    pub local: bool,
    pub generation: u64,
    pub identity: PaneIdentity,
    pub cols: u16,
    pub rows: u16,
    pub env: Vec<(String, String)>,
    pub palette: TerminalColorPalette,
}

/// Spawn a pane on the session server, or queue it if no client is connected yet (initial attach
/// or a reconnect window). Queued spawns are flushed by [`crate::update`] once the client arrives.
pub(crate) fn request_pane_spawn(state: &mut State, request: PaneSpawnRequest) {
    let PaneSpawnRequest {
        pane_id,
        local,
        generation,
        identity,
        cols,
        rows,
        env,
        palette,
    } = request;
    let scratch = local && state.scratch.panes.iter().any(|pane| pane.id == pane_id);
    let PaneIdentity {
        launch,
        cwd,
        keep_open,
        replay,
        custom_title: title,
        ..
    } = identity;
    // A replay command (see `PaneIdentity::replay`) spawns a plain interactive shell and is
    // injected as type-ahead input after the spawn succeeds (see `State::pending_replay_inputs`),
    // so aliases/functions/rc-file PATH resolve and the prompt's title integration runs first.
    let launch = match launch {
        Some(crate::pane_launch::PaneLaunch::Shell { command }) if replay && !scratch => {
            state
                .current_mut()
                .pending_replay_inputs
                .insert((pane_id, generation), command);
            None
        }
        launch => launch,
    };
    // Under `--remote` the server owns its own platform, shell, and filesystem. Our locally
    // resolved shell argv (a Linux `/usr/bin/bash` with a local rc-file path, say) and
    // shell-integration env carry local paths the remote — possibly a different OS — cannot run.
    // Send empty argv so the server resolves its own default shell (`pty_config` falls back when
    // the argv is empty), and drop the local shell-integration env.
    let (shell, command_shell, extra_env) = if !scratch && state.current().remote_host.is_some() {
        (Vec::new(), Vec::new(), Vec::new())
    } else {
        resolved_launch_argv(&state.config)
    };
    // Shell-integration env (`ZDOTDIR`, `XDG_DATA_DIRS`, ...) comes first so any caller-supplied
    // override for the same key (rare, but a pane/profile could set one deliberately) wins.
    let env = extra_env.into_iter().chain(env).collect::<Vec<_>>();
    let request = crate::session::client::SpawnPaneRequest {
        pane_id,
        local,
        generation,
        launch,
        cwd,
        cols,
        rows,
        keep_open,
        env,
        title,
        palette,
        shell,
        command_shell,
    };
    if scratch {
        if let Some(client) = state.scratch_client() {
            client.spawn_pane(request);
        } else if let Some(pane) = state
            .scratch
            .panes
            .iter_mut()
            .find(|pane| pane.id == pane_id && pane.pty_generation == generation)
        {
            pane.terminal.status =
                ManagedTerminalStatus::Error("scratch runtime disconnected".into());
        }
    } else if let Some(client) = state.current().session_client.clone() {
        client.spawn_pane(request);
    } else {
        state.current_mut().pending_spawns.push(request);
    }
}

/// Resolve this session's interactive-shell and command-runner launch policies from the live
/// config (see [`crate::platform::command`]), in wire/argv form, plus any shell-integration env
/// (Phase 8) the resolved interactive shell needs. Called at every spawn-request site (rather than
/// once at config-load time) so a hot config reload takes effect on the very next spawn without
/// needing to re-derive anything else from the reload path.
pub(crate) fn resolved_launch_argv(
    config: &crate::config::Config,
) -> (Vec<String>, Vec<String>, Vec<(String, String)>) {
    let shell_env = crate::platform::command::ShellEnv::from_process();
    let (shell, extra_env) = crate::platform::shell_integration::resolve_interactive_shell(
        config.shell.as_deref(),
        &shell_env,
        config.shell_integration.mode,
        &crate::platform::shell_integration::InjectionEnv::from_process(),
    );
    let command_shell = crate::platform::command::resolve_command_shell(
        config.command_shell.as_deref(),
        &shell_env,
    );
    (shell.as_argv(), command_shell.as_argv(), extra_env)
}
