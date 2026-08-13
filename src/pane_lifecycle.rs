use std::time::Duration;

use tui_lipan::prelude::*;

use crate::anim::{self, GeometryAnimation};
use crate::geometry::{clamp_float_rect, default_floating_rect};
use crate::layout::place_spawned_pane;
use crate::ops::focus::{
    choose_fallback_focus, choose_fallback_focus_near, first_visible_pane,
    focus_near_pane_in_workspace, focus_pane, reference_pane_rect, request_current_pane_focus,
    request_pane_focus, scrollable_close_neighbor,
};
use crate::ops::theme::pane_frame_background;
use crate::state::{Pane, PaneId, PaneIdentity, ScrollableRevealEdge, State};
use crate::tiling::remove_tiled_window;
use crate::{AppRoot, Msg};

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

pub(crate) fn spawn_pane_in_scratch(
    ctx: &mut Context<AppRoot>,
    previous_focused: Option<PaneId>,
    identity: PaneIdentity,
) -> (PaneId, Update) {
    let initial_pane = ctx.state.scratch.panes.is_empty();
    let tile_gap = ctx.state.tile_gap();
    let split_width_multiplier = ctx.state.config.layout.split_width_multiplier;
    let rect = crate::scratchpad::deployed_rect(&ctx.state, ctx.viewport());
    let id = ctx.state.current().next_pane_id;
    ctx.state.current_mut().next_pane_id = id.saturating_add(1);
    let generation = ctx.state.current().next_pty_generation;
    ctx.state.current_mut().next_pty_generation = generation.saturating_add(1);
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
    let env = pane_env(
        ctx.state.control_socket_path.as_deref(),
        &pane,
        ctx.state.current().remote_host.is_some(),
    );
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
    place_spawned_pane(
        &mut ctx.state.scratch,
        id,
        previous_focused,
        rect,
        0.0,
        tile_gap,
        split_width_multiplier,
    );
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SpawnFloat {
    pub width: f32,
    pub height: f32,
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

pub(crate) fn spawn_interactive_pane(
    ctx: &mut Context<AppRoot>,
    source_workspace: usize,
    source: Option<PaneId>,
    identity: PaneIdentity,
) -> (PaneId, Update) {
    spawn_interactive_pane_with_focus(ctx, source_workspace, source, identity, None)
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
) -> (PaneId, Update) {
    let (workspace_index, previous_focused, placement) = interactive_spawn_target(
        &ctx.state,
        source_workspace,
        source,
        identity.command.as_deref(),
        focus,
    );
    spawn_pane_in_workspace(ctx, workspace_index, previous_focused, identity, placement)
}

fn interactive_spawn_target(
    state: &State,
    source_workspace: usize,
    source: Option<PaneId>,
    command: Option<&str>,
    focus: Option<bool>,
) -> (usize, Option<PaneId>, SpawnPlacement) {
    let (rule_workspace, mut placement) = command
        .map(|command| crate::rules::placement_for_command(&state.config.rules, command))
        .unwrap_or_default();
    if let Some(focus) = focus {
        placement.focus = focus;
    }
    let workspace_index = rule_workspace.unwrap_or(source_workspace);
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
    if let Some(float) = placement.float {
        pane.floating = true;
        let w = bounds.w * float.width;
        let h = bounds.h * float.height;
        pane.floating_rect = clamp_float_rect(
            FloatRect {
                x: bounds.x + (bounds.w - w) / 2.0,
                y: bounds.y + (bounds.h - h) / 2.0,
                w,
                h,
            },
            bounds,
        );
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
    let command = pane.identity.command.clone();
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
    place_spawned_pane(
        workspace,
        id,
        previous_focused,
        bounds,
        top_gap,
        tile_gap,
        split_width_multiplier,
    );
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
fn workspace_has_fullscreen(state: &State, workspace_index: usize) -> bool {
    state.current().workspaces[workspace_index]
        .panes
        .iter()
        .any(|pane| pane.fullscreen && !pane.closing)
}

fn apply_spawn_focus(
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
fn set_scrollable_anchor_for_spawned(ws: &mut crate::state::Workspace, id: PaneId) {
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
    crate::update::schedule_layout_commit(ctx);
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
    let PaneIdentity {
        command,
        cwd,
        keep_open,
        replay,
        custom_title: title,
        ..
    } = identity;
    // A replay command (see `PaneIdentity::replay`) spawns a plain interactive shell and is
    // injected as type-ahead input after the spawn succeeds (see `State::pending_replay_inputs`),
    // so aliases/functions/rc-file PATH resolve and the prompt's title integration runs first.
    let command = match command {
        Some(command) if replay => {
            state
                .current_mut()
                .pending_replay_inputs
                .insert((pane_id, generation), command);
            None
        }
        command => command,
    };
    // Under `--remote` the server owns its own platform, shell, and filesystem. Our locally
    // resolved shell argv (a Linux `/usr/bin/bash` with a local rc-file path, say) and
    // shell-integration env carry local paths the remote — possibly a different OS — cannot run.
    // Send empty argv so the server resolves its own default shell (`pty_config` falls back when
    // the argv is empty), and drop the local shell-integration env.
    let (shell, command_shell, extra_env) = if state.current().remote_host.is_some() {
        (Vec::new(), Vec::new(), Vec::new())
    } else {
        resolved_launch_argv(&state.config)
    };
    // Shell-integration env (`ZDOTDIR`, `XDG_DATA_DIRS`, ...) comes first so any caller-supplied
    // override for the same key (rare, but a pane/profile could set one deliberately) wins.
    let env = extra_env.into_iter().chain(env).collect::<Vec<_>>();
    if let Some(client) = state.current().session_client.clone() {
        client.spawn_pane(
            pane_id,
            local,
            generation,
            command,
            cwd,
            cols,
            rows,
            keep_open,
            env,
            title,
            palette,
            shell,
            command_shell,
        );
    } else {
        state
            .current_mut()
            .pending_spawns
            .push(crate::state::PendingPaneSpawn {
                pane_id,
                local,
                generation,
                command,
                cwd,
                cols,
                rows,
                keep_open,
                env,
                title,
                palette,
                shell,
                command_shell,
            });
    }
}

/// Resolve this session's interactive-shell and command-runner launch policies from the live
/// config (see [`crate::platform::command`]), in wire/argv form, plus any shell-integration env
/// (Phase 8) the resolved interactive shell needs. Called at every spawn-request site (rather than
/// once at config-load time) so a hot config reload takes effect on the very next spawn without
/// needing to re-derive anything else from the reload path.
fn resolved_launch_argv(
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

/// Kill a live workspace pane and start its close animation.
///
/// The pane leaves the tiling layout immediately, so its neighbours begin expanding at once, but
/// stays in `panes` marked [`Pane::closing`] until [`Msg::PruneClosed`] drops it. It has to stay
/// described for the close animation to exist at all: the pane scales toward its centre, which
/// means the whole subtree is re-laid out every frame at a shrinking rectangle. Framework-side
/// retention (`Animated::auto_exit`) cannot do this, because it freezes the already reconciled
/// subtree and only clips it.
pub(crate) fn close_pane(ctx: &mut Context<AppRoot>, id: PaneId) -> Update {
    let scratch = crate::scratchpad::contains(&ctx.state, id);
    match close_pane_inner(ctx, id, true) {
        Some(generation) => Update::with_command(prune_closed_command(
            ctx.state.runtime_epoch,
            id,
            generation,
            if scratch && ctx.state.scratch.panes.iter().all(|pane| pane.closing) {
                anim::retained_pane_timeout(ctx.state.config.animations).max(
                    anim::scratch_transition_duration(
                        ctx.state.config.animations.geometry_duration,
                    ),
                )
            } else {
                anim::retained_pane_timeout(ctx.state.config.animations)
            },
        )),
        None => Update::full(),
    }
}

/// Start the close animation for a pane whose server-side process has already exited.
pub(crate) fn remove_pane_after_exit(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    local: bool,
) -> Update {
    let scratch = local && crate::scratchpad::contains(&ctx.state, id);
    match close_pane_inner_with_focus(ctx, id, false, true, Some(local)) {
        Some(generation) => Update::with_command(prune_closed_command(
            ctx.state.runtime_epoch,
            id,
            generation,
            if scratch && ctx.state.scratch.panes.iter().all(|pane| pane.closing) {
                anim::retained_pane_timeout(ctx.state.config.animations).max(
                    anim::scratch_transition_duration(
                        ctx.state.config.animations.geometry_duration,
                    ),
                )
            } else {
                anim::retained_pane_timeout(ctx.state.config.animations)
            },
        )),
        None => Update::full(),
    }
}

/// Mark a pane closing without scheduling its prune. Callers closing one pane wrap the returned
/// generation in [`prune_closed_command`]; callers closing several at once collect generations and
/// schedule one [`prune_closed_batch_command`], since an [`Update`] carries only one [`Command`].
/// Returns `None` when the pane is unknown or already closing.
pub(crate) fn close_pane_inner(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    kill_server_pane: bool,
) -> Option<u64> {
    close_pane_inner_with_focus(ctx, id, kill_server_pane, true, None)
}

/// Mark and kill one pane for a batch teardown without resolving focus. The caller must resolve
/// focus after all panes in the batch have been marked closing; otherwise each pane can select a
/// neighbour that the next iteration immediately closes.
pub(crate) fn close_pane_inner_without_focus(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    kill_server_pane: bool,
) -> Option<u64> {
    close_pane_inner_with_focus(ctx, id, kill_server_pane, false, None)
}

fn close_pane_inner_with_focus(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    kill_server_pane: bool,
    resolve_focus: bool,
    namespace: Option<bool>,
) -> Option<u64> {
    let in_scratch = crate::scratchpad::contains(&ctx.state, id);
    if namespace != Some(false) && in_scratch {
        let bounds = crate::scratchpad::deployed_rect(&ctx.state, ctx.viewport());
        let placements = crate::layout::workspace_target_rects(
            &ctx.state.scratch,
            bounds,
            0.0,
            ctx.state.tile_gap(),
        );
        let client = ctx.state.current().session_client.clone();
        let was_focused = ctx.state.scratch.focused_pane == Some(id);
        let scrollable_neighbor = (ctx.state.scratch.layout_kind
            == crate::state::LayoutKind::Scrollable)
            .then(|| scrollable_close_neighbor(&ctx.state.scratch, id))
            .flatten();
        let pane = ctx
            .state
            .scratch
            .panes
            .iter_mut()
            .find(|pane| pane.id == id && !pane.closing)?;
        let generation = pane.pty_generation;
        if kill_server_pane && let Some(client) = client {
            client.kill(id, generation, true);
        }
        pane.floating_rect = crate::layout::placement_for(&placements, id).unwrap_or(bounds);
        pane.opening = false;
        pane.closing = true;
        pane.terminal.kill();
        remove_tiled_window(&mut ctx.state.scratch, id);
        // Same focus resolution as a workspace: hand the keyboard to the pane nearest the one that
        // just left, not to whichever is first in the list. The frozen `floating_rect` above is the
        // reference rect, so `choose_fallback_focus_near` needs no extra geometry.
        if was_focused {
            match scrollable_neighbor {
                Some(target) => focus_pane(&mut ctx.state, target),
                None => choose_fallback_focus_near(&mut ctx.state, Some(id), None),
            }
        }
        // `focus_pane` may arm Scrollable's AxisChange while it syncs the viewport; the close
        // transition owns this frame.
        ctx.state.animation = GeometryAnimation::Close;
        if ctx.state.scratch.focused_pane.is_none() {
            crate::scratchpad::after_pane_removed(ctx);
        } else if resolve_focus {
            request_current_pane_focus(ctx);
        }
        return Some(generation);
    }
    if namespace == Some(true) {
        return None;
    }
    // Capture this before `closing` removes the pane from `tiled_ids()`: a Scrollable strip's
    // lifecycle/storage order is not its visual neighbor order after a move or swap. Batch callers
    // deliberately skip both focus and anchor resolution until the whole teardown is marked.
    let attachment = ctx.state.current();
    let active_workspace_index = attachment.active_workspace;
    let owner_workspace_index = attachment
        .workspaces
        .iter()
        .position(|workspace| workspace.panes.iter().any(|pane| pane.id == id));
    let active_global_focus = resolve_focus
        && owner_workspace_index == Some(active_workspace_index)
        && attachment.focused_pane == Some(id);
    let close_neighbor = resolve_focus
        .then(|| {
            owner_workspace_index.and_then(|workspace_index| {
                scrollable_close_neighbor(&attachment.workspaces[workspace_index], id)
            })
        })
        .flatten();
    let scrollable_anchor_remap = if resolve_focus {
        owner_workspace_index.and_then(|workspace_index| {
            let workspace = &attachment.workspaces[workspace_index];
            (workspace.layout_kind == crate::state::LayoutKind::Scrollable
                && workspace.scrollable_anchor == Some(id))
            .then_some((
                workspace_index,
                close_neighbor,
                workspace.scrollable_reveal_edge,
            ))
        })
    } else {
        None
    };
    // Freeze the pane where it currently sits. Once it is excluded from tiling its placement is
    // gone, so the close animation needs the rectangle it occupied captured up front.
    let bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());
    let top_gap = ctx.state.workspace_top_gap();
    let tile_gap = ctx.state.tile_gap();
    let placements = {
        let workspace = &ctx.state.current().workspaces[ctx.state.current().active_workspace];
        crate::layout::workspace_target_rects(workspace, bounds, top_gap, tile_gap)
    };

    let client = ctx.state.current().session_client.clone();
    let wire_local = namespace.unwrap_or_else(|| pane_is_local(&ctx.state, id));
    let mut generation = None;
    if let Some(pane) = match namespace {
        Some(false) => find_pane_in_namespace_mut(&mut ctx.state, id, false),
        _ => find_pane_mut(&mut ctx.state, id),
    } && !pane.closing
    {
        generation = Some(pane.pty_generation);
        if kill_server_pane && let Some(client) = client {
            client.kill(id, pane.pty_generation, wire_local);
        }
        pane.floating_rect =
            crate::layout::placement_for(&placements, id).unwrap_or(pane.floating_rect);
        pane.opening = false;
        pane.closing = true;
        pane.terminal.kill();
    }

    if generation.is_some() {
        ctx.state.animation = GeometryAnimation::Close;
        if resolve_focus {
            if active_global_focus {
                if let Some(target) = close_neighbor {
                    focus_pane(&mut ctx.state, target);
                } else {
                    choose_fallback_focus(&mut ctx.state);
                }
            }
            if let Some((workspace_index, anchor, edge)) = scrollable_anchor_remap
                && (!active_global_focus || anchor.is_none())
            {
                ctx.state.current_mut().workspaces[workspace_index]
                    .set_scrollable_viewport(anchor, edge);
            }
            // `focus_pane` may arm Scrollable's AxisChange animation while it synchronizes the new
            // viewport. The close transition owns this frame, however; keep its animation policy
            // (and therefore the retained pane's close scale) intact.
            ctx.state.animation = GeometryAnimation::Close;
            request_current_pane_focus(ctx);
        }
    }
    generation
}

/// Drop a pane once its close animation has run, if it is still the same closing pane.
pub(crate) fn prune_closed_pane(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    id: PaneId,
    generation: u64,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    let still_closing = find_pane(&ctx.state, id)
        .is_some_and(|pane| pane.pty_generation == generation && pane.closing);
    if !still_closing {
        return Update::none();
    }
    if ctx.state.popup.as_ref().is_some_and(|pane| pane.id == id) {
        ctx.state.popup = None;
    } else if let Some(index) = ctx
        .state
        .scratch
        .panes
        .iter()
        .position(|pane| pane.id == id)
    {
        ctx.state.scratch.panes.remove(index);
        remove_tiled_window(&mut ctx.state.scratch, id);
        crate::scratchpad::after_pane_removed(ctx);
    } else {
        let timeout = crate::anim::retained_pane_timeout(ctx.state.config.animations);
        // Take the pane out first so its terminal screen can be retired: a same-generation
        // reintroduction (a layout correction) restores its scrollback instead of starting blank.
        let removed = ctx
            .state
            .current_mut()
            .workspaces
            .iter_mut()
            .find_map(|ws| {
                ws.panes
                    .iter()
                    .position(|pane| pane.id == id)
                    .map(|index| ws.panes.remove(index))
            });
        remove_pane(&mut ctx.state, id);
        if let Some(pane) = removed {
            clear_pane_local_state(&mut ctx.state, id);
            ctx.state.current_mut().retire_pane(pane, timeout);
        }
    }
    Update::full()
}

pub(crate) fn prune_closed_command(
    epoch: u64,
    id: PaneId,
    generation: u64,
    delay: Duration,
) -> Command {
    Command::after(delay, move |link: CommandLink<Msg>| {
        link.send(Msg::PruneClosed(epoch, id, generation));
    })
}

/// Prune several panes closed in the same batch (e.g. [`crate::ops::exit::kill_workspace`]) after
/// one shared delay, since an [`Update`] can only carry a single [`Command`].
pub(crate) fn prune_closed_batch_command(
    epoch: u64,
    targets: Vec<(PaneId, u64)>,
    delay: Duration,
) -> Command {
    Command::after(delay, move |link: CommandLink<Msg>| {
        for (id, generation) in targets {
            link.send(Msg::PruneClosed(epoch, id, generation));
        }
    })
}

pub(crate) fn find_pane(state: &State, id: PaneId) -> Option<&Pane> {
    if let Some(pane) = state.popup.as_ref().filter(|pane| pane.id == id) {
        return Some(pane);
    }
    // The scratch workspace lives outside attachment workspaces; route its events here too.
    if let Some(pane) = state.scratch.panes.iter().find(|pane| pane.id == id) {
        return Some(pane);
    }
    state
        .current()
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.panes.iter())
        .find(|pane| pane.id == id)
}

pub(crate) fn find_pane_mut(state: &mut State, id: PaneId) -> Option<&mut Pane> {
    if state.popup.as_ref().is_some_and(|pane| pane.id == id) {
        return state.popup.as_mut();
    }
    if let Some(index) = state.scratch.panes.iter().position(|pane| pane.id == id) {
        return state.scratch.panes.get_mut(index);
    }
    state
        .current_mut()
        .workspaces
        .iter_mut()
        .flat_map(|workspace| workspace.panes.iter_mut())
        .find(|pane| pane.id == id)
}

/// Whether `id` currently lives in the client-local namespace (popup or scratch).
pub(crate) fn pane_is_local(state: &State, id: PaneId) -> bool {
    state.popup.as_ref().is_some_and(|pane| pane.id == id) || crate::scratchpad::contains(state, id)
}

/// Resolve a pane in the namespace named on the wire. Local events never search attachment
/// workspaces, and shared events never search scratch/popup, even when numeric ids collide.
pub(crate) fn find_pane_in_namespace(state: &State, id: PaneId, local: bool) -> Option<&Pane> {
    if local {
        if let Some(pane) = state.popup.as_ref().filter(|pane| pane.id == id) {
            return Some(pane);
        }
        return state.scratch.panes.iter().find(|pane| pane.id == id);
    }
    state
        .current()
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.panes.iter())
        .find(|pane| pane.id == id)
}

pub(crate) fn find_pane_in_namespace_mut(
    state: &mut State,
    id: PaneId,
    local: bool,
) -> Option<&mut Pane> {
    if local {
        if state.popup.as_ref().is_some_and(|pane| pane.id == id) {
            return state.popup.as_mut();
        }
        let index = state.scratch.panes.iter().position(|pane| pane.id == id)?;
        return state.scratch.panes.get_mut(index);
    }
    state
        .current_mut()
        .workspaces
        .iter_mut()
        .flat_map(|workspace| workspace.panes.iter_mut())
        .find(|pane| pane.id == id)
}

pub(crate) fn remove_pane(state: &mut State, id: PaneId) {
    let removed_rect = reference_pane_rect(
        state,
        &state.current().workspaces[state.current().active_workspace],
        id,
        None,
    );
    remove_pane_with_reference(state, id, removed_rect);
}

fn remove_pane_with_reference(
    state: &mut State,
    id: PaneId,
    removed_rect: Option<tui_lipan::prelude::FloatRect>,
) {
    if state.moving_pane.is_some_and(|session| session.id == id) {
        state.moving_pane = None;
    }
    if state
        .resizing_pane
        .as_ref()
        .is_some_and(|session| session.id == id)
    {
        state.resizing_pane = None;
    }

    let focus_updates: Vec<(usize, Option<PaneId>)> = state
        .current()
        .workspaces
        .iter()
        .enumerate()
        .filter_map(|(workspace_index, workspace)| {
            if workspace.focused_pane != Some(id) {
                return None;
            }
            Some((
                workspace_index,
                focus_near_pane_in_workspace(state, workspace, id, removed_rect)
                    .or_else(|| first_visible_pane(workspace)),
            ))
        })
        .collect();

    for workspace in &mut state.current_mut().workspaces {
        remove_tiled_window(workspace, id);
        workspace.panes.retain(|pane| pane.id != id);
    }
    clear_pane_local_state(state, id);

    for (workspace_index, focus) in focus_updates {
        state.current_mut().workspaces[workspace_index].focused_pane = focus;
        if workspace_index == state.current().active_workspace {
            state.current_mut().focused_pane = focus;
        }
    }
}

pub(crate) fn clear_pane_local_state(state: &mut State, id: PaneId) {
    if state
        .search
        .as_ref()
        .is_some_and(|search| search.target == id)
    {
        state.search = None;
        state.commands_dirty = true;
    }
    if state
        .copy_mode
        .as_ref()
        .is_some_and(|copy| copy.target == id)
    {
        state.copy_mode = None;
        state.mode = crate::state::Mode::Normal;
        state.commands_dirty = true;
    }
    if state
        .hint_mode
        .as_ref()
        .is_some_and(|hints| hints.target == id)
    {
        state.hint_mode = None;
        state.mode = crate::state::Mode::Normal;
        state.commands_dirty = true;
    }
    if state
        .rename
        .as_ref()
        .is_some_and(|rename| rename.target == id)
    {
        state.rename = None;
    }
    if state
        .copy_feedback_target
        .is_some_and(|(epoch, target)| epoch == state.runtime_epoch && target == id)
    {
        state.copy_feedback_target = None;
        state.copy_feedback_epoch = state.copy_feedback_epoch.wrapping_add(1);
    }
}

pub(crate) fn pane_env(
    control_socket_path: Option<&std::path::Path>,
    pane: &Pane,
    remote_attached: bool,
) -> Vec<(String, String)> {
    let mut env = vec![
        ("ROZI".to_string(), "1".to_string()),
        ("ROZI_PANE".to_string(), pane.id.to_string()),
    ];
    // Under `--remote`, the control socket lives on the client machine and must not be advertised
    // into remote PTYs (it may collide with an unrelated path on the remote host).
    if !remote_attached && let Some(path) = control_socket_path {
        env.push(("ROZI_SOCKET".to_string(), path.display().to_string()));
    }
    // Per-spawn additions last so a caller-supplied value wins over the standard set.
    env.extend(pane.identity.env.iter().cloned());
    env
}

pub(crate) fn open_timers_command(
    epoch: u64,
    id: PaneId,
    generation: u64,
    open_delay: Duration,
    activate_delay: Duration,
) -> Command {
    Command::after(open_delay, move |link: CommandLink<Msg>| {
        run_open_timers(epoch, id, generation, open_delay, activate_delay, link);
    })
}

fn run_open_timers(
    epoch: u64,
    id: PaneId,
    generation: u64,
    open_delay: Duration,
    activate_delay: Duration,
    link: CommandLink<Msg>,
) {
    // `open_delay` has already elapsed on the timer thread; chain the second stage there too
    // rather than sleeping, which would park an executor worker for the whole reveal.
    link.send(Msg::FinishOpen(epoch, id, generation));
    let remaining = activate_delay.saturating_sub(open_delay);
    let activate = Msg::ActivatePane(epoch, id, generation);
    if remaining.is_zero() {
        link.send(activate);
    } else {
        link.send_after(remaining, activate);
    }
}

/// Run the open/activate reveal timers for several panes at once. Panes created directly in state
/// (the initial pane, a restored profile/autosave layout, migrated panes) start with `opening =
/// true` (opacity 0) and are only spawned on the server via
/// [`crate::update`]; without these timers they would stay invisible. Interactive spawns get their
/// timers from [`spawn_pane_in_workspace`] instead.
pub(crate) fn open_timers_batch_command(
    epoch: u64,
    targets: Vec<(PaneId, u64)>,
    open_delay: Duration,
    activate_delay: Duration,
) -> Command {
    Command::after(open_delay, move |link: CommandLink<Msg>| {
        for (id, generation) in &targets {
            link.send(Msg::FinishOpen(epoch, *id, *generation));
        }
        // Second stage goes back on the timer thread; sleeping here would park an executor worker
        // for the whole reveal, and a restored layout arms one of these per pane.
        let remaining = activate_delay.saturating_sub(open_delay);
        for (id, generation) in &targets {
            let activate = Msg::ActivatePane(epoch, *id, *generation);
            if remaining.is_zero() {
                link.send(activate);
            } else {
                link.send_after(remaining, activate);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(matches: &str) -> crate::config::RuleConfig {
        crate::config::RuleConfig {
            matcher: crate::config::RuleMatcher::Substring(matches.to_string()),
            float: false,
            width: None,
            height: None,
            workspace: None,
            focus: true,
            fullscreen: false,
        }
    }

    /// A queued spawn request carrying `identity`, with the boilerplate a test does not care about.
    fn spawn_request(pane_id: PaneId, generation: u64, identity: PaneIdentity) -> PaneSpawnRequest {
        PaneSpawnRequest {
            pane_id,
            local: false,
            generation,
            identity,
            cols: 80,
            rows: 24,
            env: Vec::new(),
            palette: TerminalColorPalette::default(),
        }
    }

    pub(crate) fn in_stack<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(body)
            .expect("spawn test thread")
            .join()
            .expect("join test thread")
    }

    fn scrollable_close_backend(focus: PaneId) -> tui_lipan::TestBackend<crate::AppRoot> {
        let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
        backend.set_viewport(tui_lipan::prelude::Rect {
            x: 0,
            y: 0,
            w: 80,
            h: 24,
        });
        {
            let state = backend.state_mut();
            state.config.confirm.close_pane = false;
            let workspace = &mut state.current_mut().workspaces[0];
            workspace.layout_kind = crate::state::LayoutKind::Scrollable;
            workspace.panes.clear();
            workspace.tile_tree = crate::tiling::build_dwindle_tree(
                &[10, 30, 20],
                crate::state::SplitAxis::Horizontal,
                &[0.5, 0.5],
            );
            // Storage order intentionally differs from the tree order.
            for id in [20, 10, 30] {
                let mut pane = Pane::new(
                    id,
                    100,
                    FloatRect {
                        x: 0.0,
                        y: 0.0,
                        w: 80.0,
                        h: 24.0,
                    },
                );
                pane.opening = false;
                pane.terminal_active = true;
                // Keep the post-close strip overflowing so focus synchronization must reveal the
                // selected neighbor rather than merely preserving the first remaining column.
                pane.scrollable_width = 0.80;
                workspace.panes.push(pane);
            }
            workspace.focused_pane = Some(focus);
            workspace.scrollable_anchor = Some(focus);
            state.current_mut().focused_pane = Some(focus);
        }
        backend.render();
        backend
    }

    #[test]
    fn replay_spawn_queues_the_command_as_input_instead_of_a_wire_command() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        request_pane_spawn(
            &mut state,
            spawn_request(
                7,
                3,
                PaneIdentity {
                    command: Some("n".to_string()),
                    replay: true,
                    ..PaneIdentity::default()
                },
            ),
        );
        // No client yet: the spawn is queued, with the replay command stripped from the wire
        // request and parked for post-spawn injection instead.
        assert_eq!(state.current().pending_spawns.len(), 1);
        assert_eq!(state.current().pending_spawns[0].command, None);
        assert_eq!(
            state
                .current()
                .pending_replay_inputs
                .get(&(7, 3))
                .map(String::as_str),
            Some("n")
        );

        // A non-replay command rides the wire request as before.
        request_pane_spawn(
            &mut state,
            spawn_request(
                8,
                4,
                PaneIdentity {
                    command: Some("htop".to_string()),
                    ..PaneIdentity::default()
                },
            ),
        );
        assert_eq!(
            state.current().pending_spawns[1].command.as_deref(),
            Some("htop"),
            "deterministic command panes must keep command-shell semantics"
        );
        assert!(!state.current().pending_replay_inputs.contains_key(&(8, 4)));
    }

    /// A `--remote` pane must not carry the client's locally resolved shell argv to the server: a
    /// Linux client's `/usr/bin/bash` (with a local rc-file path) cannot run on a Windows remote.
    /// Empty argv makes the server resolve its own default shell.
    #[test]
    fn remote_spawn_sends_no_local_shell_argv() {
        // Local session: the resolved interactive shell rides the request as before.
        let mut local = State::new(crate::config::Config::default(), Theme::default());
        request_pane_spawn(&mut local, spawn_request(1, 1, PaneIdentity::default()));
        assert!(
            !local.current().pending_spawns[0].shell.is_empty(),
            "a local pane keeps its resolved interactive-shell argv"
        );

        // Remote session: shell and command_shell are emptied for server-side resolution.
        let mut remote = State::new(crate::config::Config::default(), Theme::default());
        remote.current_mut().remote_host = Some("winvm".to_string());
        request_pane_spawn(&mut remote, spawn_request(1, 1, PaneIdentity::default()));
        assert!(
            remote.current().pending_spawns[0].shell.is_empty(),
            "a --remote pane must send an empty shell argv"
        );
        assert!(
            remote.current().pending_spawns[0].command_shell.is_empty(),
            "a --remote pane must send an empty command-shell argv"
        );
    }

    #[test]
    fn replay_inputs_survive_teardown_only_while_their_spawn_is_still_queued() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        request_pane_spawn(
            &mut state,
            spawn_request(
                7,
                3,
                PaneIdentity {
                    command: Some("n".to_string()),
                    replay: true,
                    ..PaneIdentity::default()
                },
            ),
        );
        // An entry whose spawn already went out (not queued) can never complete after a
        // disconnect, and its key could be minted again once the generation counter restarts.
        state
            .current_mut()
            .pending_replay_inputs
            .insert((9, 1), "stale".to_string());

        state.prune_replay_inputs_to_pending_spawns();

        assert!(state.current().pending_replay_inputs.contains_key(&(7, 3)));
        assert!(!state.current().pending_replay_inputs.contains_key(&(9, 1)));
    }

    #[test]
    fn close_keeps_the_pane_described_while_it_animates_out() {
        in_stack(|| {
            let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
            backend.set_viewport(tui_lipan::prelude::Rect {
                x: 0,
                y: 0,
                w: 80,
                h: 24,
            });
            {
                let state = backend.state_mut();
                state.config.confirm.close_pane = false;
                let pane = &mut state.current_mut().workspaces[0].panes[0];
                pane.opening = false;
                pane.terminal_active = true;
            }
            backend.render();

            backend
                .dispatch(crate::Msg::RunAction(crate::input::Action::Close))
                .expect("close pane");
            // The pane has to stay described for the close scale to have anything to lay out.
            // It leaves the tiling layout at once so neighbours expand, but is still rendered.
            let pane = &backend.state().current().workspaces[0].panes[0];
            assert!(pane.closing, "the pane should be animating out, not gone");
            assert_eq!(backend.state().current().workspaces[0].visible_count(), 0);
            assert_eq!(backend.state().current().focused_pane, None);
            assert!(
                backend
                    .capture_ui_snapshot()
                    .widgets
                    .iter()
                    .any(|widget| widget
                        .key
                        .as_ref()
                        .is_some_and(|key| key.as_ref() == "rozi-pane-1-0")),
                "the closing pane still renders while it scales down"
            );

            // Prune drops it once the animation has run.
            backend
                .dispatch(crate::Msg::PruneClosed(
                    backend.state().runtime_epoch,
                    1,
                    backend.state().current().workspaces[0].panes[0].pty_generation,
                ))
                .expect("prune closed pane");
            assert!(backend.state().current().workspaces[0].panes.is_empty());
        });
    }

    #[test]
    fn closing_middle_scrollable_pane_focuses_next_tree_neighbor_and_prunes_cleanly() {
        in_stack(|| {
            let mut backend = scrollable_close_backend(30);
            backend
                .dispatch(crate::Msg::RunAction(crate::input::Action::Close))
                .expect("close middle pane");

            let workspace = &backend.state().current().workspaces[0];
            assert!(
                workspace
                    .panes
                    .iter()
                    .any(|pane| pane.id == 30 && pane.closing)
            );
            assert_eq!(backend.state().current().focused_pane, Some(20));
            assert_eq!(workspace.focused_pane, Some(20));
            assert_eq!(workspace.scrollable_anchor, Some(20));
            assert_eq!(backend.state().animation, GeometryAnimation::Close);
            assert_eq!(
                workspace.scrollable_reveal_edge,
                ScrollableRevealEdge::Right
            );
            assert_eq!(workspace.tiled_ids(), [10, 20]);

            let generation = workspace
                .panes
                .iter()
                .find(|pane| pane.id == 30)
                .expect("closing pane")
                .pty_generation;
            backend
                .dispatch(crate::Msg::PruneClosed(
                    backend.state().runtime_epoch,
                    30,
                    generation,
                ))
                .expect("prune closed middle pane");
            let workspace = &backend.state().current().workspaces[0];
            assert!(workspace.panes.iter().all(|pane| pane.id != 30));
            assert_eq!(backend.state().current().focused_pane, Some(20));
            assert_eq!(workspace.scrollable_anchor, Some(20));
        });
    }

    #[test]
    fn closing_final_scrollable_pane_focuses_previous_tree_neighbor() {
        in_stack(|| {
            let mut backend = scrollable_close_backend(20);
            backend
                .dispatch(crate::Msg::RunAction(crate::input::Action::Close))
                .expect("close final pane");

            let workspace = &backend.state().current().workspaces[0];
            assert!(
                workspace
                    .panes
                    .iter()
                    .any(|pane| pane.id == 20 && pane.closing)
            );
            assert_eq!(backend.state().current().focused_pane, Some(30));
            assert_eq!(workspace.focused_pane, Some(30));
            assert_eq!(workspace.scrollable_anchor, Some(30));
            assert_eq!(workspace.tiled_ids(), [10, 30]);
        });
    }

    #[test]
    fn closing_the_last_scrollable_tile_clears_its_anchor() {
        in_stack(|| {
            let mut backend = scrollable_close_backend(30);
            {
                let state = backend.state_mut();
                let workspace = &mut state.current_mut().workspaces[0];
                workspace.panes.retain(|pane| pane.id == 30);
                workspace.tile_tree = Some(crate::tiling::DwindleTree::Leaf(30));
                workspace.focused_pane = Some(30);
                workspace.scrollable_anchor = Some(30);
                workspace.scrollable_reveal_edge = ScrollableRevealEdge::Right;
                state.current_mut().focused_pane = Some(30);
            }
            backend.render();
            backend
                .dispatch(crate::Msg::RunAction(crate::input::Action::Close))
                .expect("close last Scrollable tile");

            let workspace = &backend.state().current().workspaces[0];
            assert_eq!(backend.state().current().focused_pane, None);
            assert_eq!(workspace.focused_pane, None);
            assert_eq!(workspace.scrollable_anchor, None);
            assert_eq!(workspace.scrollable_reveal_edge, ScrollableRevealEdge::Left);
            assert!(
                workspace
                    .panes
                    .iter()
                    .any(|pane| pane.id == 30 && pane.closing)
            );
        });
    }

    #[test]
    fn closing_nonfocused_scrollable_pane_preserves_focus_and_anchor() {
        in_stack(|| {
            let mut backend = scrollable_close_backend(30);
            backend.state_mut().sidebar.panels[0].active_tab =
                Some(crate::config::SidebarTabId::new("panes"));
            // Tree order is [10, 30, 20], so row 1 is pane 10; pane 30 remains focused.
            backend
                .dispatch(crate::Msg::SidebarRowClose { panel: 0, index: 1 })
                .expect("arm nonfocused close");
            backend
                .dispatch(crate::Msg::SidebarRowClose { panel: 0, index: 1 })
                .expect("close nonfocused pane");

            let workspace = &backend.state().current().workspaces[0];
            assert!(
                workspace
                    .panes
                    .iter()
                    .any(|pane| pane.id == 10 && pane.closing)
            );
            assert_eq!(backend.state().current().focused_pane, Some(30));
            assert_eq!(workspace.focused_pane, Some(30));
            assert_eq!(workspace.scrollable_anchor, Some(30));
        });
    }

    #[test]
    fn closing_a_nonfocused_scrollable_anchor_remaps_without_changing_focus() {
        in_stack(|| {
            let mut backend = scrollable_close_backend(30);
            {
                let state = backend.state_mut();
                let mut floating = Pane::new(
                    99,
                    100,
                    FloatRect {
                        x: 5.0,
                        y: 5.0,
                        w: 20.0,
                        h: 10.0,
                    },
                );
                floating.floating = true;
                floating.opening = false;
                floating.terminal_active = true;
                let workspace = &mut state.current_mut().workspaces[0];
                workspace.panes.push(floating);
                workspace.focused_pane = Some(99);
                workspace.scrollable_anchor = Some(30);
                workspace.scrollable_reveal_edge = ScrollableRevealEdge::Right;
                state.current_mut().focused_pane = Some(99);
                state.sidebar.panels[0].active_tab =
                    Some(crate::config::SidebarTabId::new("panes"));
            }
            backend.render();
            let focus_events =
                backend
                    .state()
                    .event_hub
                    .subscribe(Some(std::collections::HashSet::from([
                        crate::events::EventKind::FocusChanged,
                    ])));

            // Tree order is [10, 30, 20], so row 2 closes the anchored middle tile.
            backend
                .dispatch(crate::Msg::SidebarRowClose { panel: 0, index: 2 })
                .expect("arm anchored close");
            backend
                .dispatch(crate::Msg::SidebarRowClose { panel: 0, index: 2 })
                .expect("close anchored tile");

            let workspace = &backend.state().current().workspaces[0];
            assert!(
                workspace
                    .panes
                    .iter()
                    .any(|pane| pane.id == 30 && pane.closing)
            );
            assert_eq!(backend.state().current().focused_pane, Some(99));
            assert_eq!(workspace.focused_pane, Some(99));
            assert_eq!(workspace.scrollable_anchor, Some(20));
            assert_eq!(
                workspace.scrollable_reveal_edge,
                ScrollableRevealEdge::Right
            );
            assert!(focus_events.try_recv().is_err());
        });
    }

    #[test]
    fn closing_an_inactive_scrollable_anchor_remaps_its_workspace_only() {
        in_stack(|| {
            let mut backend = scrollable_close_backend(30);
            {
                let state = backend.state_mut();
                let rect = FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 80.0,
                    h: 24.0,
                };
                let workspace = &mut state.current_mut().workspaces[1];
                workspace.layout_kind = crate::state::LayoutKind::Scrollable;
                workspace.panes.clear();
                for id in [120, 110, 130] {
                    let mut pane = Pane::new(id, 100, rect);
                    pane.opening = false;
                    pane.terminal_active = true;
                    workspace.panes.push(pane);
                }
                workspace.tile_tree = crate::tiling::build_dwindle_tree(
                    &[110, 130, 120],
                    crate::state::SplitAxis::Horizontal,
                    &[0.5, 0.5],
                );
                workspace.focused_pane = Some(130);
                workspace.scrollable_anchor = Some(130);
                workspace.scrollable_reveal_edge = ScrollableRevealEdge::Right;
                state.sidebar.panels[0].active_tab =
                    Some(crate::config::SidebarTabId::new("panes"));
            }
            backend.render();
            let focus_events =
                backend
                    .state()
                    .event_hub
                    .subscribe(Some(std::collections::HashSet::from([
                        crate::events::EventKind::FocusChanged,
                    ])));

            // Active rows 0–3, spacer 4, inactive header 5; row 7 is pane 130 in [110, 130, 120].
            backend
                .dispatch(crate::Msg::SidebarRowClose { panel: 0, index: 7 })
                .expect("arm inactive anchored close");
            backend
                .dispatch(crate::Msg::SidebarRowClose { panel: 0, index: 7 })
                .expect("close inactive anchored tile");

            let current = backend.state().current();
            let active = &current.workspaces[0];
            let inactive = &current.workspaces[1];
            assert_eq!(current.focused_pane, Some(30));
            assert_eq!(active.focused_pane, Some(30));
            assert_eq!(active.scrollable_anchor, Some(30));
            assert!(
                inactive
                    .panes
                    .iter()
                    .any(|pane| pane.id == 130 && pane.closing)
            );
            assert_eq!(inactive.scrollable_anchor, Some(120));
            assert_eq!(inactive.scrollable_reveal_edge, ScrollableRevealEdge::Right);
            assert!(focus_events.try_recv().is_err());
        });
    }

    #[test]
    fn close_popup_keeps_the_popup_described_until_it_is_pruned() {
        in_stack(|| {
            let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
            backend.set_viewport(tui_lipan::prelude::Rect {
                x: 0,
                y: 0,
                w: 80,
                h: 24,
            });
            {
                let state = backend.state_mut();
                let mut popup = Pane::new(
                    crate::state::POPUP_PANE_ID,
                    state.config.scrollback,
                    FloatRect {
                        x: 10.0,
                        y: 5.0,
                        w: 40.0,
                        h: 12.0,
                    },
                );
                popup.opening = false;
                popup.terminal_active = true;
                state.popup = Some(popup);
            }
            backend.render();

            backend
                .dispatch(crate::Msg::ClosePopup)
                .expect("close popup");
            let popup = backend
                .state()
                .popup
                .as_ref()
                .expect("popup still described");
            assert!(popup.closing);
            let generation = popup.pty_generation;
            assert!(
                backend
                    .capture_ui_snapshot()
                    .widgets
                    .iter()
                    .any(|widget| widget
                        .key
                        .as_ref()
                        .is_some_and(|key| { key.as_ref() == "rozi-pane-4294967295-0" })),
                "the closing popup still renders while it scales down"
            );

            backend
                .dispatch(crate::Msg::PruneClosed(
                    backend.state().runtime_epoch,
                    crate::state::POPUP_PANE_ID,
                    generation,
                ))
                .expect("prune closed popup");
            assert!(backend.state().popup.is_none());
        });
    }

    #[test]
    fn disabled_close_animation_still_prunes_the_pane() {
        in_stack(|| {
            let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
            backend.set_viewport(tui_lipan::prelude::Rect {
                x: 0,
                y: 0,
                w: 80,
                h: 24,
            });
            {
                let state = backend.state_mut();
                state.config.confirm.close_pane = false;
                state.config.animations.enabled = false;
                state.current_mut().workspaces[0].panes[0].opening = false;
            }
            backend.render();
            // Read the generation before closing: with animations off the prune delay is zero, so
            // the scheduled `PruneClosed` can land on its own timer at any point after the close
            // and take the pane out from under a later read.
            let generation = backend.state().current().workspaces[0].panes[0].pty_generation;
            backend
                .dispatch(crate::Msg::RunAction(crate::input::Action::Close))
                .expect("close pane");

            // Whether the timer got there first or not, the message drives removal and the pane
            // is gone either way.
            backend
                .dispatch(crate::Msg::PruneClosed(
                    backend.state().runtime_epoch,
                    1,
                    generation,
                ))
                .expect("prune closed pane");
            assert!(backend.state().current().workspaces[0].panes.is_empty());
            assert!(backend.capture_ui_snapshot().widgets.iter().all(|widget| {
                widget
                    .key
                    .as_ref()
                    .is_none_or(|key| key.as_ref() != "rozi-pane-1-0")
            }));
        });
    }

    #[test]
    fn workspace_switch_replaces_the_canvas_host_without_retaining_old_panes() {
        in_stack(|| {
            let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
            backend.set_viewport(tui_lipan::prelude::Rect {
                x: 0,
                y: 0,
                w: 80,
                h: 24,
            });
            let mut pane = Pane::new(2, 100, FloatRect::default());
            pane.opening = false;
            backend.state_mut().current_mut().workspaces[1]
                .panes
                .push(pane);
            crate::tiling::append_tiled_window(
                &mut backend.state_mut().current_mut().workspaces[1],
                2,
            );
            backend.render();

            backend
                .dispatch(crate::Msg::RunAction(
                    crate::input::Action::SwitchWorkspace(1),
                ))
                .expect("switch workspace");
            let snapshot = backend.capture_ui_snapshot();
            assert!(snapshot.widgets.iter().any(|widget| {
                widget
                    .key
                    .as_ref()
                    .is_some_and(|key| key.as_ref() == "rozi-pane-2-0")
            }));
            assert!(snapshot.widgets.iter().all(|widget| {
                widget
                    .key
                    .as_ref()
                    .is_none_or(|key| key.as_ref() != "rozi-pane-1-0")
            }));
        });
    }

    #[test]
    fn removing_a_pane_clears_modes_that_target_it() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        state.copy_mode = Some(crate::state::CopyModeState {
            target: 1,
            navigation: TerminalCopyMode::new(0, 0, 0),
            search_matches: Vec::new(),
            search_current: 0,
            search_truncated: false,
        });
        state.hint_mode = Some(crate::state::HintModeState {
            target: 1,
            matches: Vec::new(),
            labels: Vec::new(),
            input: String::new(),
            offset: 0,
        });
        state.rename = Some(crate::state::PaneRenameState::new(1, "pane"));
        state.copy_feedback_target = Some((state.runtime_epoch, 1));

        remove_pane(&mut state, 1);

        assert!(state.copy_mode.is_none());
        assert!(state.hint_mode.is_none());
        assert!(state.rename.is_none());
        assert!(state.copy_feedback_target.is_none());
        assert_eq!(state.mode, crate::state::Mode::Normal);
    }

    #[test]
    fn spawn_focus_can_update_target_workspace_without_stealing_active_focus() {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        state.current_mut().active_workspace = 0;
        state.current_mut().focused_pane = Some(1);
        apply_spawn_focus(
            &mut state,
            2,
            7,
            SpawnPlacement {
                focus: false,
                ..Default::default()
            },
        );
        assert_eq!(state.current().workspaces[2].focused_pane, Some(7));
        assert_eq!(state.current().active_workspace, 0);
        assert_eq!(state.current().focused_pane, Some(1));
        apply_spawn_focus(&mut state, 2, 8, SpawnPlacement::default());
        assert_eq!(state.current().active_workspace, 2);
        assert_eq!(state.current().focused_pane, Some(8));
    }

    #[test]
    fn non_focusing_spawn_on_active_scrollable_keeps_viewport_anchor() {
        let bounds = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 24.0,
        };
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        {
            let workspace = &mut state.current_mut().workspaces[0];
            workspace.layout_kind = crate::state::LayoutKind::Scrollable;
            for id in [1_u32, 2] {
                workspace.panes.push(Pane::new(id, 100, bounds));
                crate::tiling::append_tiled_window(workspace, id);
            }
            workspace.focused_pane = Some(1);
            workspace.scrollable_anchor = Some(1);
            workspace.panes.push(Pane::new(3, 100, bounds));
            crate::tiling::append_tiled_window(workspace, 3);
        }
        state.current_mut().active_workspace = 0;
        state.current_mut().focused_pane = Some(1);

        apply_spawn_focus(
            &mut state,
            0,
            3,
            SpawnPlacement {
                focus: false,
                ..Default::default()
            },
        );

        assert_eq!(state.current().focused_pane, Some(1));
        assert_eq!(state.current().workspaces[0].focused_pane, Some(1));
        assert_eq!(state.current().workspaces[0].scrollable_anchor, Some(1));
        let render_focus = state.current().workspaces[0]
            .focused_pane
            .or(state.current().focused_pane);
        assert_eq!(render_focus, Some(1));

        let placements = crate::layout::workspace_target_rects(
            &state.current().workspaces[0],
            bounds,
            0.0,
            crate::state::TileGap::DEFAULT,
        );
        let anchored = placements.iter().find(|p| p.id == 1).expect("pane A");
        assert!(
            (anchored.rect.x - bounds.x).abs() < f32::EPSILON,
            "viewport must stay on A after a non-focusing spawn"
        );
    }

    #[test]
    fn non_focusing_tiled_spawn_with_floating_focus_anchors_existing_tiled() {
        let bounds = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 24.0,
        };
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        {
            let workspace = &mut state.current_mut().workspaces[0];
            workspace.layout_kind = crate::state::LayoutKind::Scrollable;
            workspace.panes.push(Pane::new(1, 100, bounds));
            crate::tiling::append_tiled_window(workspace, 1);
            let mut floating = Pane::new(2, 100, bounds);
            floating.floating = true;
            workspace.panes.push(floating);
            workspace.focused_pane = Some(2);
            workspace.scrollable_anchor = Some(99);
            workspace.panes.push(Pane::new(3, 100, bounds));
            crate::tiling::append_tiled_window(workspace, 3);
        }
        state.current_mut().active_workspace = 0;
        state.current_mut().focused_pane = Some(2);

        apply_spawn_focus(
            &mut state,
            0,
            3,
            SpawnPlacement {
                focus: false,
                ..Default::default()
            },
        );

        assert_eq!(state.current().focused_pane, Some(2));
        assert_eq!(state.current().workspaces[0].focused_pane, Some(2));
        assert_eq!(state.current().workspaces[0].scrollable_anchor, Some(1));
        assert_eq!(
            state.current().workspaces[0]
                .focused_pane
                .or(state.current().focused_pane),
            Some(2)
        );

        let placements = crate::layout::workspace_target_rects(
            &state.current().workspaces[0],
            bounds,
            0.0,
            crate::state::TileGap::DEFAULT,
        );
        let anchored = placements
            .iter()
            .find(|p| p.id == 1)
            .expect("existing tiled");
        assert!(
            (anchored.rect.x - bounds.x).abs() < f32::EPSILON,
            "stale/missing anchor must fall back to a pre-existing tiled pane, not the spawn"
        );
    }

    #[test]
    fn interactive_command_spawn_applies_configured_rule() {
        let mut config = crate::config::Config::default();
        let mut configured = rule("btop");
        configured.workspace = Some(2);
        configured.float = true;
        configured.width = Some(0.7);
        configured.height = Some(0.8);
        configured.fullscreen = true;
        configured.focus = false;
        config.rules.push(configured);
        let mut state = State::new(config, Theme::default());
        state.current_mut().workspaces[2].focused_pane = Some(7);

        let (workspace, previous_focused, placement) =
            interactive_spawn_target(&state, 0, None, Some("exec btop"), None);

        assert_eq!(workspace, 2);
        assert_eq!(previous_focused, Some(7));
        assert_eq!(
            placement,
            SpawnPlacement {
                float: Some(SpawnFloat {
                    width: 0.7,
                    height: 0.8,
                }),
                fullscreen: true,
                focus: false,
            }
        );
    }

    #[test]
    fn interactive_spawn_without_command_keeps_source_and_default_placement() {
        let mut config = crate::config::Config::default();
        config.rules.push(rule("btop"));
        let state = State::new(config, Theme::default());

        let target = interactive_spawn_target(&state, 0, Some(1), None, None);

        assert_eq!(target, (0, Some(1), SpawnPlacement::default()));
    }

    #[test]
    fn focus_override_beats_the_matched_rule_without_touching_placement() {
        let mut config = crate::config::Config::default();
        let mut configured = rule("btop");
        configured.workspace = Some(3);
        configured.fullscreen = true;
        configured.focus = true;
        config.rules.push(configured);
        let state = State::new(config, Theme::default());

        // The control endpoint's default: never move focus, but keep the rule's placement.
        let (workspace, _, placement) =
            interactive_spawn_target(&state, 0, None, Some("exec btop"), Some(false));
        assert_eq!(workspace, 3);
        assert!(!placement.focus);
        assert!(placement.fullscreen);

        // `--focus` overrides a rule that asked for no focus.
        let mut config = crate::config::Config::default();
        let mut configured = rule("btop");
        configured.focus = false;
        config.rules.push(configured);
        let state = State::new(config, Theme::default());
        let (_, _, placement) =
            interactive_spawn_target(&state, 0, None, Some("exec btop"), Some(true));
        assert!(placement.focus);
    }

    #[test]
    fn pane_env_skips_control_socket_when_remote_attached() {
        let pane = Pane::new(1, 100, FloatRect::default());
        let path = std::path::Path::new("/tmp/rozi-control.sock");
        let local = pane_env(Some(path), &pane, false);
        assert!(
            local
                .iter()
                .any(|(k, v)| k == "ROZI_SOCKET" && v.contains("rozi-control")),
            "local attach should inject ROZI_SOCKET: {local:?}"
        );
        let remote = pane_env(Some(path), &pane, true);
        assert!(
            remote.iter().all(|(k, _)| k != "ROZI_SOCKET"),
            "remote attach must not inject client ROZI_SOCKET: {remote:?}"
        );
        assert!(remote.iter().any(|(k, _)| k == "ROZI"));
        assert!(remote.iter().any(|(k, _)| k == "ROZI_PANE"));
    }

    /// A spawn parks the Scrollable viewport on the new pane. The scratch spawn skipped that, so
    /// the anchor stayed on whatever was focused first - and switching the dropdown to Scrollable
    /// later revealed that stale pane, leaving the actually-focused one off screen.
    #[test]
    fn spawning_into_the_scratchpad_parks_the_scrollable_anchor_on_the_new_pane() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                use crate::AppRoot;
                use crate::state::Pane;
                use tui_lipan::TestBackend;
                use tui_lipan::prelude::{FloatRect, Rect};

                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30,
                });
                {
                    let state = backend.state_mut();
                    let mut pane = Pane::new(1, 100, FloatRect::default());
                    pane.opening = false;
                    state.scratch.panes.push(pane);
                    crate::tiling::append_tiled_window(&mut state.scratch, 1);
                    state.scratch.focused_pane = Some(1);
                    state.scratch.scrollable_anchor = Some(1);
                    state.scratch_visible = true;
                }
                backend.render();

                backend
                    .dispatch(crate::Msg::RunAction(crate::input::Action::Spawn))
                    .expect("spawn into the scratchpad");

                let spawned = backend
                    .state()
                    .scratch
                    .focused_pane
                    .expect("the spawn takes focus");
                assert_ne!(spawned, 1, "a second pane was created");
                assert_eq!(
                    backend.state().scratch.scrollable_anchor,
                    Some(spawned),
                    "the strip must be parked on the pane that now has focus"
                );
            })
            .expect("spawn scratch anchor test thread")
            .join()
            .expect("scratch anchor test thread panicked");
    }
}

#[cfg(test)]
mod close_animation {
    use super::tests::in_stack;
    use std::time::Duration;

    fn pane_rect(backend: &tui_lipan::TestBackend<crate::AppRoot>) -> Option<(i16, i16, u16, u16)> {
        backend
            .capture_ui_snapshot()
            .widgets
            .iter()
            .find(|w| {
                w.key
                    .as_ref()
                    .is_some_and(|k| k.as_ref() == "rozi-pane-1-0")
            })
            .map(|w| (w.rect.x, w.rect.y, w.rect.w, w.rect.h))
    }

    /// The close animation is the spawn animation in reverse: the pane scales toward its centre on
    /// **both** axes, so its border shrinks with it. A height-only collapse would clip the bottom
    /// border away instead. Floating panes animate exactly like tiled ones.
    #[test]
    fn a_closing_pane_scales_down_on_both_axes() {
        for floating in [false, true] {
            in_stack(move || {
                let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
                backend.set_viewport(tui_lipan::prelude::Rect {
                    x: 0,
                    y: 0,
                    w: 80,
                    h: 24,
                });
                {
                    let state = backend.state_mut();
                    state.config.confirm.close_pane = false;
                    let pane = &mut state.current_mut().workspaces[0].panes[0];
                    pane.opening = false;
                    pane.terminal_active = true;
                    pane.floating = floating;
                }
                backend.render();
                let (_, _, w0, h0) = pane_rect(&backend).expect("pane renders");

                backend
                    .dispatch(crate::Msg::RunAction(crate::input::Action::Close))
                    .expect("close");

                // Front-loaded: the shrink has to be visible before the fade hides it, so the very
                // first tick must already move. An EaseInOutCubic ramp would still be at full size.
                backend.advance(Duration::from_millis(25));
                let (x1, y1, w1, h1) = pane_rect(&backend).expect("closing pane still renders");
                assert!(
                    w1 < w0 && h1 < h0,
                    "closing={floating}: both axes should shrink on the first tick, \
                     got {w1}x{h1} from {w0}x{h0}"
                );
                assert!(
                    x1 > 0 && y1 > 0,
                    "the pane should pull in toward its centre"
                );

                // And it keeps shrinking rather than snapping.
                backend.advance(Duration::from_millis(25));
                let (_, _, w2, h2) = pane_rect(&backend).expect("still closing");
                assert!(w2 < w1 && h2 <= h1, "the scale should continue: {w2}x{h2}");
            });
        }
    }

    /// A pane the user closed exits by definition. Reporting that exit is noise, and worse, the
    /// `[exited]` title and failure toast appear over the pane while it is still animating out.
    #[test]
    fn a_user_closed_pane_does_not_report_its_own_exit() {
        in_stack(|| {
            let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
            backend.set_viewport(tui_lipan::prelude::Rect {
                x: 0,
                y: 0,
                w: 80,
                h: 24,
            });
            let generation = {
                let state = backend.state_mut();
                state.config.confirm.close_pane = false;
                state.config.pane.hold_on_exit = true;
                let pane = &mut state.current_mut().workspaces[0].panes[0];
                pane.opening = false;
                pane.terminal_active = true;
                pane.pty_generation
            };
            backend.render();

            backend
                .dispatch(crate::Msg::RunAction(crate::input::Action::Close))
                .expect("close");
            let epoch = backend.state().runtime_epoch;

            // The server reports the kill we asked for.
            backend
                .dispatch(crate::Msg::SessionExited {
                    epoch,
                    pane_id: 1,
                    local: false,
                    generation,
                    code: 1,
                })
                .expect("exit frame");

            let text = backend.capture_frame().plain_text();
            assert!(
                !text.contains("exited (1)"),
                "closing our own pane must not toast its exit: {text}"
            );
            // The other half of the same noise: the marker would rewrite the titlebar while the
            // pane is animating out from under it.
            assert!(
                !text.contains("[exited"),
                "a closing pane must not wear its exit code: {text}"
            );
            // `hold_on_exit` must not keep a pane the user explicitly closed.
            assert!(
                backend.state().current().workspaces[0].panes[0].closing,
                "the pane should still be closing, not held open"
            );
        });
    }

    /// The other side of the marker rule: a pane `hold_on_exit` keeps in the layout is staying, so
    /// its exit code is the only thing saying why it is inert and respawnable.
    #[test]
    fn a_held_exited_pane_still_wears_its_exit_code() {
        in_stack(|| {
            let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
            backend.set_viewport(tui_lipan::prelude::Rect {
                x: 0,
                y: 0,
                w: 80,
                h: 24,
            });
            let generation = {
                let state = backend.state_mut();
                state.config.pane.hold_on_exit = true;
                let pane = &mut state.current_mut().workspaces[0].panes[0];
                pane.opening = false;
                pane.terminal_active = true;
                pane.pty_generation
            };
            backend.render();
            let epoch = backend.state().runtime_epoch;

            // The shell exits on its own - nobody closed this pane.
            backend
                .dispatch(crate::Msg::SessionExited {
                    epoch,
                    pane_id: 1,
                    local: false,
                    generation,
                    code: 3,
                })
                .expect("exit frame");

            assert!(
                !backend.state().current().workspaces[0].panes[0].closing,
                "hold_on_exit should keep the pane in the layout"
            );
            let text = backend.capture_frame().plain_text();
            assert!(
                text.contains("[exited 3]"),
                "a held pane must say why it is inert: {text}"
            );
        });
    }

    #[test]
    fn find_pane_in_namespace_does_not_cross_local_and_shared_ids() {
        in_stack(|| {
            let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
            let rect = tui_lipan::prelude::FloatRect::default();
            let mut shared = crate::state::Pane::new(7, 100, rect);
            shared.title = "shared".into();
            backend.state_mut().current_mut().workspaces[0]
                .panes
                .push(shared);
            let mut local = crate::state::Pane::new(7, 100, rect);
            local.title = "local".into();
            backend.state_mut().scratch.panes.push(local);
            backend.state_mut().scratch_visible = true;

            assert_eq!(
                super::find_pane_in_namespace(backend.state(), 7, false)
                    .unwrap()
                    .title,
                "shared"
            );
            assert_eq!(
                super::find_pane_in_namespace(backend.state(), 7, true)
                    .unwrap()
                    .title,
                "local"
            );
            assert_eq!(
                super::find_pane(backend.state(), 7).unwrap().title,
                "local",
                "stacking lookup still prefers scratch; session events must not use it"
            );
        });
    }
}
