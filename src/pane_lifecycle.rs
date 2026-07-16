use std::time::Duration;

use tui_lipan::prelude::*;

use crate::anim::{self, GeometryAnimation, WindowAnimationConfig};
use crate::geometry::{clamp_float_rect, default_floating_rect};
use crate::layout::{place_spawned_pane, placement_for, workspace_target_rects};
use crate::ops::focus::{
    choose_fallback_focus, first_visible_pane, focus_near_pane_in_workspace, reference_pane_rect,
    request_current_pane_focus, total_visible_panes,
};
use crate::ops::theme::{pane_frame_background, terminal_palette};
use crate::state::{Pane, PaneId, PaneIdentity, State};
use crate::tiling::remove_tiled_window;
use crate::{HyprmuxApp, Msg};

pub(crate) fn spawn_pane(ctx: &mut Context<HyprmuxApp>) -> Update {
    let workspace = &ctx.state.workspaces[ctx.state.active_workspace];
    let previous_focused = workspace.focused_pane;
    // A new pane opens in the focused pane's local working directory, never a remote SSH path.
    let mut identity = PaneIdentity::default();
    if let Some(cwd) = previous_focused
        .and_then(|id| workspace.panes.iter().find(|pane| pane.id == id))
        .and_then(|pane| pane.local_cwd())
    {
        identity.cwd = Some(cwd);
    }

    spawn_interactive_pane(ctx, ctx.state.active_workspace, previous_focused, identity).1
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
    let previous_focused = source.or(state.workspaces[workspace_index].focused_pane);
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
    let id = ctx.state.next_pane_id;
    ctx.state.next_pane_id = ctx.state.next_pane_id.saturating_add(1);
    let generation = ctx.state.next_pty_generation;
    ctx.state.next_pty_generation = ctx.state.next_pty_generation.saturating_add(1);
    let floating_rect = default_floating_rect(bounds, id);
    let mut pane = Pane::new(id, ctx.state.config.scrollback, floating_rect);
    pane.pty_generation = generation;
    pane.terminal.bind_server_backend(id, generation);
    pane.identity = identity;
    pane.fullscreen = placement.fullscreen;
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
    let palette = terminal_palette(
        &ctx.state.theme,
        pane_frame_background(
            &ctx.state.theme,
            true,
            ctx.state.config.pane.highlight_focused_background,
        ),
    );
    pane.terminal.set_palette(palette);
    pane.opening = true;

    let env = pane_env(ctx.state.control_socket_path.as_deref(), &pane);
    let command = pane.identity.command.clone();
    let cwd = pane.identity.cwd.clone();
    let title = pane.identity.custom_title.clone();
    let keep_open = pane.identity.keep_open;
    let replay = pane.identity.replay;
    let cols = pane.terminal.cols;
    let rows = pane.terminal.rows;

    let workspace = &mut ctx.state.workspaces[workspace_index];
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
        id,
        generation,
        command.clone(),
        cwd.clone(),
        cols,
        rows,
        keep_open,
        env,
        title,
        palette,
        replay,
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

fn apply_spawn_focus(
    state: &mut State,
    workspace_index: usize,
    id: PaneId,
    placement: SpawnPlacement,
) {
    state.workspaces[workspace_index].focused_pane = Some(id);
    if placement.focus {
        state.active_workspace = workspace_index;
        state.focused_pane = Some(id);
    }
}

pub(crate) fn respawn_focused_pane(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(id) = ctx.state.focused_pane else {
        return Update::none();
    };
    if !find_pane(&ctx.state, id)
        .is_some_and(|pane| matches!(pane.terminal.status, ManagedTerminalStatus::Exited(_)))
    {
        return Update::none();
    }
    let generation = ctx.state.next_pty_generation;
    ctx.state.next_pty_generation = generation.saturating_add(1);
    let control_socket = ctx.state.control_socket_path.clone();
    let palette = terminal_palette(
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
            pane_env(control_socket.as_deref(), pane),
            pane.identity.clone(),
            pane.terminal.cols,
            pane.terminal.rows,
        )
    };
    request_pane_spawn(
        &mut ctx.state,
        id,
        generation,
        identity.command,
        identity.cwd,
        cols,
        rows,
        identity.keep_open,
        env,
        identity.custom_title,
        palette,
        identity.replay,
    );
    crate::update::schedule_layout_commit(ctx);
    Update::full()
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
    palette: TerminalColorPalette,
    replay: bool,
) {
    // A replay command (see `PaneIdentity::replay`) spawns a plain interactive shell and is
    // injected as type-ahead input after the spawn succeeds (see `State::pending_replay_inputs`),
    // so aliases/functions/rc-file PATH resolve and the prompt's title integration runs first.
    let command = match command {
        Some(command) if replay => {
            state
                .pending_replay_inputs
                .insert((pane_id, generation), command);
            None
        }
        command => command,
    };
    let (shell, command_shell, extra_env) = resolved_launch_argv(&state.config);
    // Shell-integration env (`ZDOTDIR`, `XDG_DATA_DIRS`, ...) comes first so any caller-supplied
    // override for the same key (rare, but a pane/profile could set one deliberately) wins.
    let env = extra_env.into_iter().chain(env).collect::<Vec<_>>();
    if let Some(client) = state.session_client.clone() {
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
/// [`crate::ops::exit::kill_workspace`]) collect generations across multiple calls and schedule
/// one combined [`prune_closed_batch_command`], since an [`Update`] carries only one [`Command`].
/// Returns `None` (no state change) if the pane was already closing.
pub(crate) fn close_pane_state(ctx: &mut Context<HyprmuxApp>, id: PaneId) -> Option<u64> {
    let bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());
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

/// Mark a pane closing in response to a foreign layout commit that removed it. Mirrors
/// [`close_pane_state`] but **never** sends `client.kill` - the server already dropped the pane at
/// the controller's request, so re-killing would race a reused id and there is no kill-echo loop.
/// Returns the pane's generation so the caller can schedule its delayed prune, or `None` when the
/// pane is unknown or already closing.
pub(crate) fn close_pane_remote(ctx: &mut Context<HyprmuxApp>, id: PaneId) -> Option<u64> {
    let bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());
    let top_gap = ctx.state.workspace_top_gap();
    let tile_gap = ctx.state.tile_gap();
    let placements = {
        let workspace = &ctx.state.workspaces[ctx.state.active_workspace];
        workspace_target_rects(workspace, bounds, top_gap, tile_gap)
    };
    let mut generation = None;
    if let Some(pane) = find_pane_mut(&mut ctx.state, id)
        && !pane.closing
    {
        generation = Some(pane.pty_generation);
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
    if let Some(pane) = state.popup.as_ref().filter(|pane| pane.id == id) {
        return Some(pane);
    }
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
    if state.popup.as_ref().is_some_and(|pane| pane.id == id) {
        return state.popup.as_mut();
    }
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
    if state
        .resizing_pane
        .as_ref()
        .is_some_and(|session| session.id == id)
    {
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

/// Prune several panes closed in the same batch (e.g. [`crate::ops::exit::kill_workspace`])
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

    fn rule(matches: &str) -> crate::config::HyprmuxRuleConfig {
        crate::config::HyprmuxRuleConfig {
            matches: matches.to_string(),
            float: false,
            width: None,
            height: None,
            workspace: None,
            focus: true,
            fullscreen: false,
        }
    }

    #[test]
    fn replay_spawn_queues_the_command_as_input_instead_of_a_wire_command() {
        let mut state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        request_pane_spawn(
            &mut state,
            7,
            3,
            Some("n".to_string()),
            None,
            80,
            24,
            false,
            Vec::new(),
            None,
            TerminalColorPalette::default(),
            true,
        );
        // No client yet: the spawn is queued, with the replay command stripped from the wire
        // request and parked for post-spawn injection instead.
        assert_eq!(state.pending_spawns.len(), 1);
        assert_eq!(state.pending_spawns[0].command, None);
        assert_eq!(
            state.pending_replay_inputs.get(&(7, 3)).map(String::as_str),
            Some("n")
        );

        // A non-replay command rides the wire request as before.
        request_pane_spawn(
            &mut state,
            8,
            4,
            Some("htop".to_string()),
            None,
            80,
            24,
            false,
            Vec::new(),
            None,
            TerminalColorPalette::default(),
            false,
        );
        assert_eq!(
            state.pending_spawns[1].command.as_deref(),
            Some("htop"),
            "deterministic command panes must keep command-shell semantics"
        );
        assert!(!state.pending_replay_inputs.contains_key(&(8, 4)));
    }

    #[test]
    fn replay_inputs_survive_teardown_only_while_their_spawn_is_still_queued() {
        let mut state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        request_pane_spawn(
            &mut state,
            7,
            3,
            Some("n".to_string()),
            None,
            80,
            24,
            false,
            Vec::new(),
            None,
            TerminalColorPalette::default(),
            true,
        );
        // An entry whose spawn already went out (not queued) can never complete after a
        // disconnect, and its key could be minted again once the generation counter restarts.
        state
            .pending_replay_inputs
            .insert((9, 1), "stale".to_string());

        state.prune_replay_inputs_to_pending_spawns();

        assert!(state.pending_replay_inputs.contains_key(&(7, 3)));
        assert!(!state.pending_replay_inputs.contains_key(&(9, 1)));
    }

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

    #[test]
    fn spawn_focus_can_update_target_workspace_without_stealing_active_focus() {
        let mut state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        state.active_workspace = 0;
        state.focused_pane = Some(1);
        apply_spawn_focus(
            &mut state,
            2,
            7,
            SpawnPlacement {
                focus: false,
                ..Default::default()
            },
        );
        assert_eq!(state.workspaces[2].focused_pane, Some(7));
        assert_eq!(state.active_workspace, 0);
        assert_eq!(state.focused_pane, Some(1));
        apply_spawn_focus(&mut state, 2, 8, SpawnPlacement::default());
        assert_eq!(state.active_workspace, 2);
        assert_eq!(state.focused_pane, Some(8));
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
        state.workspaces[2].focused_pane = Some(7);

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
}
