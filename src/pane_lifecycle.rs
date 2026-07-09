use std::time::Duration;

use tui_lipan::prelude::*;

use crate::anim::{self, GeometryAnimation, WindowAnimationConfig};
use crate::focus_ops::{
    choose_fallback_focus, first_visible_pane, focus_near_pane_in_workspace, reference_pane_rect,
    request_current_pane_focus, total_visible_panes,
};
use crate::geometry::default_floating_rect;
use crate::layout::{place_spawned_pane, placement_for, workspace_target_rects};
use crate::state::{Pane, PaneId, PaneIdentity, State};
use crate::theme_ops::{pane_frame_background, terminal_palette};
use crate::tiling::remove_tiled_window;
use crate::{HyprmuxApp, Msg};

pub(crate) fn spawn_pane(ctx: &mut Context<HyprmuxApp>) -> Update {
    let workspace = &ctx.state.workspaces[ctx.state.active_workspace];
    let previous_focused = workspace.focused_pane;
    // A new pane opens in the focused pane's live working directory (falling back to the
    // configured cwd when the focused pane is floating or its cwd is unknown).
    let mut identity = PaneIdentity::default();
    if let Some(cwd) = previous_focused
        .and_then(|id| workspace.panes.iter().find(|pane| pane.id == id))
        .and_then(|pane| pane.live_cwd())
    {
        identity.cwd = Some(cwd);
    }

    spawn_pane_in_workspace(ctx, ctx.state.active_workspace, previous_focused, identity).1
}

pub(crate) fn spawn_pane_in_workspace(
    ctx: &mut Context<HyprmuxApp>,
    workspace_index: usize,
    previous_focused: Option<PaneId>,
    identity: PaneIdentity,
) -> (PaneId, Update) {
    let bounds = ctx.state.canvas_bounds(ctx.viewport());
    let top_gap = ctx.state.workspace_top_gap();
    let tile_gap = ctx.state.tile_gap();
    let id = ctx.state.next_pane_id;
    ctx.state.next_pane_id = ctx.state.next_pane_id.saturating_add(1);
    let generation = ctx.state.next_pty_generation;
    ctx.state.next_pty_generation = ctx.state.next_pty_generation.saturating_add(1);
    let floating_rect = default_floating_rect(bounds, id);
    let mut pane = Pane::new(id, ctx.state.config.scrollback, floating_rect);
    pane.pty_generation = generation;
    pane.terminal.bind_server_backend(id, generation);
    pane.identity = identity;
    pane.terminal.set_palette(terminal_palette(
        &ctx.state.theme,
        pane_frame_background(
            &ctx.state.theme,
            true,
            ctx.state.config.pane.highlight_focused_background,
        ),
    ));
    pane.opening = true;

    let env = pane_env(ctx.state.control_socket_path.as_deref(), &pane);
    let command = pane.identity.command.clone();
    let cwd = pane.identity.cwd.clone();
    let title = pane.identity.custom_title.clone();
    let keep_open = pane.identity.keep_open;
    let cols = pane.terminal.cols;
    let rows = pane.terminal.rows;

    let workspace = &mut ctx.state.workspaces[workspace_index];
    workspace.panes.push(pane);
    place_spawned_pane(workspace, id, previous_focused, bounds, top_gap, tile_gap);
    workspace.focused_pane = Some(id);
    ctx.state.active_workspace = workspace_index;
    ctx.state.focused_pane = Some(id);
    ctx.state.animation = GeometryAnimation::Spawn;
    let open_delay = anim::open_delay(ctx.state.config.animations);
    let activate_delay = anim::activation_delay(ctx.state.config.animations);

    request_pane_spawn(
        &mut ctx.state,
        id,
        generation,
        command,
        cwd,
        cols,
        rows,
        keep_open,
        env,
        title,
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

/// Spawn a pane on the session server, or queue it if no client is connected yet (initial attach
/// or a reconnect window). Queued spawns are flushed by [`crate::update`] once the client arrives.
#[allow(clippy::too_many_arguments)]
pub(crate) fn request_pane_spawn(
    state: &mut State,
    pane_id: PaneId,
    generation: u64,
    command: Option<String>,
    cwd: Option<String>,
    cols: u16,
    rows: u16,
    keep_open: bool,
    env: Vec<(String, String)>,
    title: Option<String>,
) {
    if let Some(client) = state.session_client.clone() {
        client.spawn_pane(
            pane_id, generation, command, cwd, cols, rows, keep_open, env, title,
        );
    } else {
        state.pending_spawns.push(crate::state::PendingPaneSpawn {
            pane_id,
            generation,
            command,
            cwd,
            cols,
            rows,
            keep_open,
            env,
            title,
        });
    }
}

pub(crate) fn begin_close_pane(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    animations: WindowAnimationConfig,
) -> Update {
    match close_pane_state(ctx, id) {
        Some(generation) => Update::with_command(prune_closed_command(
            ctx.state.runtime_epoch,
            id,
            generation,
            anim::close_delay(animations),
        )),
        None => Update::full(),
    }
}

/// Mark a pane closing, kill its terminal/PTY, and update fallback focus + the close animation,
/// without scheduling the delayed prune. Callers that close a single pane wrap the returned
/// generation in one [`prune_closed_command`]; callers that close several panes at once (e.g.
/// [`crate::exit_ops::kill_workspace`]) collect generations across multiple calls and schedule
/// one combined [`prune_closed_batch_command`], since an [`Update`] carries only one [`Command`].
/// Returns `None` (no state change) if the pane was already closing.
pub(crate) fn close_pane_state(ctx: &mut Context<HyprmuxApp>, id: PaneId) -> Option<u64> {
    let bounds = ctx.state.canvas_bounds(ctx.viewport());
    let top_gap = ctx.state.workspace_top_gap();
    let tile_gap = ctx.state.tile_gap();
    let placements = {
        let workspace = &ctx.state.workspaces[ctx.state.active_workspace];
        workspace_target_rects(workspace, bounds, top_gap, tile_gap)
    };
    let mut generation = None;
    let client = ctx.state.session_client.clone();
    if let Some(pane) = find_pane_mut(&mut ctx.state, id)
        && !pane.closing
    {
        generation = Some(pane.pty_generation);
        if let Some(client) = client {
            client.kill(id, pane.pty_generation);
        }
        pane.floating_rect = placement_for(&placements, id).unwrap_or(pane.floating_rect);
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

pub(crate) fn find_pane(state: &State, id: PaneId) -> Option<&Pane> {
    // The scratchpad lives outside the workspace lists; route its events here too.
    if let Some(pane) = state.scratch.as_ref().filter(|pane| pane.id == id) {
        return Some(pane);
    }
    state
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.panes.iter())
        .find(|pane| pane.id == id)
}

pub(crate) fn find_pane_mut(state: &mut State, id: PaneId) -> Option<&mut Pane> {
    // The scratchpad lives outside the workspace lists; route its events here too.
    if state.scratch.as_ref().is_some_and(|pane| pane.id == id) {
        return state.scratch.as_mut();
    }
    state
        .workspaces
        .iter_mut()
        .flat_map(|workspace| workspace.panes.iter_mut())
        .find(|pane| pane.id == id)
}

pub(crate) fn remove_pane(state: &mut State, id: PaneId) {
    if state.moving_pane.is_some_and(|session| session.id == id) {
        state.moving_pane = None;
    }
    if state.resizing_pane.is_some_and(|session| session.id == id) {
        state.resizing_pane = None;
    }

    let removed_rect =
        reference_pane_rect(state, &state.workspaces[state.active_workspace], id, None);
    let focus_updates: Vec<(usize, Option<PaneId>)> = state
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

    for workspace in &mut state.workspaces {
        remove_tiled_window(workspace, id);
        workspace.panes.retain(|pane| pane.id != id);
    }

    for (workspace_index, focus) in focus_updates {
        state.workspaces[workspace_index].focused_pane = focus;
        if workspace_index == state.active_workspace {
            state.focused_pane = focus;
        }
    }
}

pub(crate) fn handle_prune_closed(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    generation: u64,
) -> Update {
    if !should_prune_closed(&ctx.state, id, generation) {
        return Update::none();
    }
    remove_pane(&mut ctx.state, id);
    if ctx
        .state
        .search
        .as_ref()
        .is_some_and(|search| search.target == id)
    {
        ctx.state.search = None;
        ctx.state.commands_dirty = true;
    }
    if total_visible_panes(&ctx.state) == 0 {
        request_current_pane_focus(ctx);
        return Update::full();
    }
    request_current_pane_focus(ctx);
    Update::full()
}

fn should_prune_closed(state: &State, id: PaneId, generation: u64) -> bool {
    state
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.panes.iter())
        .any(|pane| pane.id == id && pane.pty_generation == generation && pane.closing)
}

/// Spawns one background thread per `(command, interval_secs)` pair that runs the shell
/// command, sends its output, sleeps, and repeats for the life of the app - the same
/// fire-and-forget pattern as the PTY read threads.
pub(crate) fn spawn_workbar_command_pollers(
    workbar_commands: Vec<(String, u64)>,
    link: &CommandLink<Msg>,
) {
    for (command, interval_secs) in workbar_commands {
        let poller_link = link.clone();
        std::thread::spawn(move || {
            loop {
                let output = run_workbar_command(&command);
                poller_link.send(Msg::WorkbarCommandOutput(command.clone(), output));
                std::thread::sleep(Duration::from_secs(interval_secs.max(1)));
            }
        });
    }
}

/// Runs a `command:` workbar segment's shell command through the user's shell and returns the
/// first line of stdout, trimmed. Failures (missing shell, non-zero exit, no output) collapse
/// to an empty string rather than surfacing an error in the workbar.
fn run_workbar_command(command: &str) -> String {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string());
    std::process::Command::new(shell)
        .arg("-c")
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
) -> Vec<(String, String)> {
    let mut env = vec![
        ("HYPRMUX".to_string(), "1".to_string()),
        ("HYPRMUX_PANE".to_string(), pane.id.to_string()),
    ];
    if let Some(path) = control_socket_path {
        env.push(("HYPRMUX_SOCKET".to_string(), path.display().to_string()));
    }
    env
}

pub(crate) fn open_timers_command(
    epoch: u64,
    id: PaneId,
    generation: u64,
    open_delay: Duration,
    activate_delay: Duration,
) -> Command {
    Command::spawn(move |link: CommandLink<Msg>| {
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
    if !open_delay.is_zero() {
        std::thread::sleep(open_delay);
    }
    link.send(Msg::FinishOpen(epoch, id, generation));
    let remaining = activate_delay.saturating_sub(open_delay);
    if !remaining.is_zero() {
        std::thread::sleep(remaining);
    }
    link.send(Msg::ActivatePane(epoch, id, generation));
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
    Command::spawn(move |link: CommandLink<Msg>| {
        if !open_delay.is_zero() {
            std::thread::sleep(open_delay);
        }
        for (id, generation) in &targets {
            link.send(Msg::FinishOpen(epoch, *id, *generation));
        }
        let remaining = activate_delay.saturating_sub(open_delay);
        if !remaining.is_zero() {
            std::thread::sleep(remaining);
        }
        for (id, generation) in &targets {
            link.send(Msg::ActivatePane(epoch, *id, *generation));
        }
    })
}

pub(crate) fn prune_closed_command(
    epoch: u64,
    id: PaneId,
    generation: u64,
    delay: Duration,
) -> Command {
    Command::spawn(move |link: CommandLink<Msg>| {
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        link.send(Msg::PruneClosed(epoch, id, generation));
    })
}

/// Prune several panes closed in the same batch (e.g. [`crate::exit_ops::kill_workspace`])
/// after one shared delay, since an [`Update`] can only carry a single [`Command`].
pub(crate) fn prune_closed_batch_command(
    epoch: u64,
    targets: Vec<(PaneId, u64)>,
    delay: Duration,
) -> Command {
    Command::spawn(move |link: CommandLink<Msg>| {
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        for (id, generation) in targets {
            link.send(Msg::PruneClosed(epoch, id, generation));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_prune_token_does_not_match_reused_pane_id() {
        use crate::config::HyprmuxConfig;
        let mut state = State::new(HyprmuxConfig::default(), Theme::default());
        state.workspaces[0].panes[0].pty_generation = 42;
        state.workspaces[0].panes[0].closing = true;
        assert!(should_prune_closed(&state, 1, 42));
        assert!(!should_prune_closed(&state, 1, 41));

        state.workspaces[0].panes[0].closing = false;
        assert!(!should_prune_closed(&state, 1, 42));
    }
}
