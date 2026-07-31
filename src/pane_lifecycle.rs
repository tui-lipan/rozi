use std::time::Duration;

use tui_lipan::prelude::*;

use crate::anim::{self, GeometryAnimation};
use crate::geometry::{clamp_float_rect, default_floating_rect};
use crate::layout::place_spawned_pane;
use crate::ops::focus::{
    choose_fallback_focus, first_visible_pane, focus_near_pane_in_workspace, reference_pane_rect,
    request_current_pane_focus,
};
use crate::ops::theme::pane_frame_background;
use crate::state::{Pane, PaneId, PaneIdentity, State};
use crate::tiling::remove_tiled_window;
use crate::{HyprmuxApp, Msg};

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
    let workspace = &state.current().workspaces[state.current().active_workspace];
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
    let workspace = &state.current().workspaces[state.current().active_workspace];
    workspace
        .focused_pane
        .and_then(|id| workspace.panes.iter().find(|pane| pane.id == id))
        .and_then(|pane| pane.server_cwd_ref())
}

/// Cwd to send with a server spawn request. Under `--remote`, inherits the server-relative path.
pub(crate) fn focused_spawn_cwd(state: &State) -> Option<String> {
    if state.current().remote_host.is_some() {
        let workspace = &state.current().workspaces[state.current().active_workspace];
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

pub(crate) fn spawn_pane(ctx: &mut Context<HyprmuxApp>) -> Update {
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
    ctx: &mut Context<HyprmuxApp>,
    source_workspace: usize,
    source: Option<PaneId>,
    identity: PaneIdentity,
) -> (PaneId, Update) {
    let (workspace_index, previous_focused, placement) = interactive_spawn_target(
        &ctx.state,
        source_workspace,
        source,
        identity.command.as_deref(),
    );
    spawn_pane_in_workspace(ctx, workspace_index, previous_focused, identity, placement)
}

fn interactive_spawn_target(
    state: &State,
    source_workspace: usize,
    source: Option<PaneId>,
    command: Option<&str>,
) -> (usize, Option<PaneId>, SpawnPlacement) {
    let (rule_workspace, placement) = command
        .map(|command| crate::rules::placement_for_command(&state.config.rules, command))
        .unwrap_or_default();
    let workspace_index = rule_workspace.unwrap_or(source_workspace);
    let previous_focused = source.or(state.current().workspaces[workspace_index].focused_pane);
    (workspace_index, previous_focused, placement)
}

pub(crate) fn spawn_pane_in_workspace(
    ctx: &mut Context<HyprmuxApp>,
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
    state.current_mut().workspaces[workspace_index].focused_pane = Some(id);
    if placement.focus {
        state.current_mut().active_workspace = workspace_index;
        state.current_mut().focused_pane = Some(id);
    }
}

pub(crate) fn respawn_focused_pane(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(id) = ctx.state.current().focused_pane else {
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
    request_pane_spawn(
        &mut ctx.state,
        PaneSpawnRequest {
            pane_id: id,
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
    config: &crate::config::HyprmuxConfig,
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
pub(crate) fn close_pane(ctx: &mut Context<HyprmuxApp>, id: PaneId) -> Update {
    match close_pane_inner(ctx, id, true) {
        Some(generation) => Update::with_command(prune_closed_command(
            ctx.state.runtime_epoch,
            id,
            generation,
            anim::retained_pane_timeout(ctx.state.config.animations),
        )),
        None => Update::full(),
    }
}

/// Start the close animation for a pane whose server-side process has already exited.
pub(crate) fn remove_pane_after_exit(ctx: &mut Context<HyprmuxApp>, id: PaneId) -> Update {
    match close_pane_inner(ctx, id, false) {
        Some(generation) => Update::with_command(prune_closed_command(
            ctx.state.runtime_epoch,
            id,
            generation,
            anim::retained_pane_timeout(ctx.state.config.animations),
        )),
        None => Update::full(),
    }
}

/// Mark a pane closing without scheduling its prune. Callers closing one pane wrap the returned
/// generation in [`prune_closed_command`]; callers closing several at once collect generations and
/// schedule one [`prune_closed_batch_command`], since an [`Update`] carries only one [`Command`].
/// Returns `None` when the pane is unknown or already closing.
pub(crate) fn close_pane_inner(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    kill_server_pane: bool,
) -> Option<u64> {
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
    let mut generation = None;
    if let Some(pane) = find_pane_mut(&mut ctx.state, id)
        && !pane.closing
    {
        generation = Some(pane.pty_generation);
        if kill_server_pane && let Some(client) = client {
            client.kill(id, pane.pty_generation);
        }
        pane.floating_rect =
            crate::layout::placement_for(&placements, id).unwrap_or(pane.floating_rect);
        pane.opening = false;
        pane.closing = true;
        pane.terminal.kill();
    }

    if generation.is_some() {
        ctx.state.animation = GeometryAnimation::Close;
        choose_fallback_focus(&mut ctx.state);
        request_current_pane_focus(ctx);
    }
    generation
}

/// Drop a pane once its close animation has run, if it is still the same closing pane.
pub(crate) fn prune_closed_pane(
    ctx: &mut Context<HyprmuxApp>,
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
    } else if ctx.state.scratch.as_ref().is_some_and(|pane| pane.id == id) {
        ctx.state.scratch = None;
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
    // The scratchpad lives outside the workspace lists; route its events here too.
    if let Some(pane) = state.scratch.as_ref().filter(|pane| pane.id == id) {
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
    // The scratchpad lives outside the workspace lists; route its events here too.
    if state.scratch.as_ref().is_some_and(|pane| pane.id == id) {
        return state.scratch.as_mut();
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

/// Spawns one background thread per `(command, interval_secs)` pair that runs the shell
/// command, sends its output, sleeps, and repeats for the life of the app - the same
/// fire-and-forget pattern as the PTY read threads.
///
/// `command_shell` is the resolved command-runner argv (see
/// [`crate::platform::command::resolve_command_shell`]), resolved once by the caller (which has
/// the live config) rather than per-poller-thread.
pub(crate) fn spawn_workbar_command_pollers(
    workbar_commands: Vec<(String, u64)>,
    command_shell: Vec<String>,
    link: &CommandLink<Msg>,
) {
    for (command, interval_secs) in workbar_commands {
        let poller_link = link.clone();
        let command_shell = command_shell.clone();
        std::thread::spawn(move || {
            loop {
                let output = run_workbar_command(&command, &command_shell);
                poller_link.send(Msg::WorkbarCommandOutput(command.clone(), output));
                std::thread::sleep(Duration::from_secs(interval_secs.max(1)));
            }
        });
    }
}

/// Runs a `command:` workbar segment's shell command through the resolved command-runner shell
/// and returns the first line of stdout, trimmed. Failures (missing shell, non-zero exit, no
/// output) collapse to an empty string rather than surfacing an error in the workbar.
fn run_workbar_command(command: &str, command_shell: &[String]) -> String {
    let runner = crate::platform::command::ShellCommand::from_argv(command_shell)
        .unwrap_or_else(|| crate::platform::command::ShellCommand::new("/bin/sh").arg("-c"));
    std::process::Command::new(runner.program)
        .args(runner.args)
        .arg(command)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .map(str::trim)
                .map(str::to_string)
        })
        .unwrap_or_default()
}

pub(crate) fn pane_env(
    control_socket_path: Option<&std::path::Path>,
    pane: &Pane,
    remote_attached: bool,
) -> Vec<(String, String)> {
    let mut env = vec![
        ("HYPRMUX".to_string(), "1".to_string()),
        ("HYPRMUX_PANE".to_string(), pane.id.to_string()),
    ];
    // Under `--remote`, the control socket lives on the client machine and must not be advertised
    // into remote PTYs (it may collide with an unrelated path on the remote host).
    if !remote_attached && let Some(path) = control_socket_path {
        env.push(("HYPRMUX_SOCKET".to_string(), path.display().to_string()));
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

    fn rule(matches: &str) -> crate::config::HyprmuxRuleConfig {
        crate::config::HyprmuxRuleConfig {
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

    #[test]
    fn replay_spawn_queues_the_command_as_input_instead_of_a_wire_command() {
        let mut state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
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
        let mut local = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        request_pane_spawn(&mut local, spawn_request(1, 1, PaneIdentity::default()));
        assert!(
            !local.current().pending_spawns[0].shell.is_empty(),
            "a local pane keeps its resolved interactive-shell argv"
        );

        // Remote session: shell and command_shell are emptied for server-side resolution.
        let mut remote = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
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
        let mut state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
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
            let mut backend = tui_lipan::TestBackend::new(crate::HyprmuxApp::default());
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
                        .is_some_and(|key| key.as_ref() == "hyprmux-pane-1-0")),
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
    fn close_popup_keeps_the_popup_described_until_it_is_pruned() {
        in_stack(|| {
            let mut backend = tui_lipan::TestBackend::new(crate::HyprmuxApp::default());
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
                        .is_some_and(|key| { key.as_ref() == "hyprmux-pane-4294967295-0" })),
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
            let mut backend = tui_lipan::TestBackend::new(crate::HyprmuxApp::default());
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
                    .is_none_or(|key| key.as_ref() != "hyprmux-pane-1-0")
            }));
        });
    }

    #[test]
    fn workspace_switch_replaces_the_canvas_host_without_retaining_old_panes() {
        in_stack(|| {
            let mut backend = tui_lipan::TestBackend::new(crate::HyprmuxApp::default());
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
                    .is_some_and(|key| key.as_ref() == "hyprmux-pane-2-0")
            }));
            assert!(snapshot.widgets.iter().all(|widget| {
                widget
                    .key
                    .as_ref()
                    .is_none_or(|key| key.as_ref() != "hyprmux-pane-1-0")
            }));
        });
    }

    #[test]
    fn removing_a_pane_clears_modes_that_target_it() {
        let mut state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        state.copy_mode = Some(crate::state::CopyModeState {
            target: 1,
            navigation: TerminalCopyMode::new(0, 0, 0),
            search_matches: Vec::new(),
            search_current: 0,
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
        let mut state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
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
    fn interactive_command_spawn_applies_configured_rule() {
        let mut config = crate::config::HyprmuxConfig::default();
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
            interactive_spawn_target(&state, 0, None, Some("exec btop"));

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
        let mut config = crate::config::HyprmuxConfig::default();
        config.rules.push(rule("btop"));
        let state = State::new(config, Theme::default());

        let target = interactive_spawn_target(&state, 0, Some(1), None);

        assert_eq!(target, (0, Some(1), SpawnPlacement::default()));
    }

    #[test]
    fn pane_env_skips_control_socket_when_remote_attached() {
        let pane = Pane::new(1, 100, FloatRect::default());
        let path = std::path::Path::new("/tmp/hyprmux-control.sock");
        let local = pane_env(Some(path), &pane, false);
        assert!(
            local
                .iter()
                .any(|(k, v)| k == "HYPRMUX_SOCKET" && v.contains("hyprmux-control")),
            "local attach should inject HYPRMUX_SOCKET: {local:?}"
        );
        let remote = pane_env(Some(path), &pane, true);
        assert!(
            remote.iter().all(|(k, _)| k != "HYPRMUX_SOCKET"),
            "remote attach must not inject client HYPRMUX_SOCKET: {remote:?}"
        );
        assert!(remote.iter().any(|(k, _)| k == "HYPRMUX"));
        assert!(remote.iter().any(|(k, _)| k == "HYPRMUX_PANE"));
    }
}

#[cfg(test)]
mod close_animation {
    use super::tests::in_stack;
    use std::time::Duration;

    fn pane_rect(
        backend: &tui_lipan::TestBackend<crate::HyprmuxApp>,
    ) -> Option<(i16, i16, u16, u16)> {
        backend
            .capture_ui_snapshot()
            .widgets
            .iter()
            .find(|w| {
                w.key
                    .as_ref()
                    .is_some_and(|k| k.as_ref() == "hyprmux-pane-1-0")
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
                let mut backend = tui_lipan::TestBackend::new(crate::HyprmuxApp::default());
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
            let mut backend = tui_lipan::TestBackend::new(crate::HyprmuxApp::default());
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
                    generation,
                    code: 1,
                })
                .expect("exit frame");

            let text = backend.capture_frame().plain_text();
            assert!(
                !text.contains("exited (1)"),
                "closing our own pane must not toast its exit: {text}"
            );
            // `hold_on_exit` must not keep a pane the user explicitly closed.
            assert!(
                backend.state().current().workspaces[0].panes[0].closing,
                "the pane should still be closing, not held open"
            );
        });
    }
}
