use std::time::Duration;

use tui_lipan::prelude::*;

use crate::anim::{self, GeometryAnimation, WindowAnimationConfig};
use crate::config::HyprmuxConfig;
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

pub(crate) type OpenTimers = Option<(Duration, Duration)>;
pub(crate) type StartupSpawn = (u64, PaneId, u64, TerminalPtyConfig, OpenTimers);

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
    let id = ctx.state.next_pane_id;
    ctx.state.next_pane_id = ctx.state.next_pane_id.saturating_add(1);
    let generation = ctx.state.next_pty_generation;
    ctx.state.next_pty_generation = ctx.state.next_pty_generation.saturating_add(1);
    let floating_rect = default_floating_rect(bounds, id);
    let mut pane = Pane::new(id, ctx.state.config.scrollback, floating_rect);
    pane.pty_generation = generation;
    pane.terminal.bind_session(id, generation);
    if ctx.state.session_attached {
        pane.terminal.bind_server_backend(id, generation);
    }
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

    let pty_config = pty_config_for_pane(
        &ctx.state.config,
        ctx.state.control_socket_path.as_deref(),
        &pane,
    );
    let server_spawn = ctx
        .state
        .session_attached
        .then(|| {
            ctx.state.session_client.clone().map(|client| {
                (
                    client,
                    pane.identity.command.clone(),
                    pane.identity.cwd.clone(),
                    pane.identity.custom_title.clone(),
                    pane.terminal.cols,
                    pane.terminal.rows,
                    pane.identity.keep_open,
                )
            })
        })
        .flatten();

    let workspace = &mut ctx.state.workspaces[workspace_index];
    workspace.panes.push(pane);
    place_spawned_pane(workspace, id, previous_focused, bounds, top_gap);
    workspace.focused_pane = Some(id);
    ctx.state.active_workspace = workspace_index;
    ctx.state.focused_pane = Some(id);
    ctx.state.animation = GeometryAnimation::Spawn;
    let open_delay = anim::open_delay(ctx.state.config.animations);
    let activate_delay = anim::activation_delay(ctx.state.config.animations);

    let update = if let Some((client, command, cwd, title, cols, rows, keep_open)) = server_spawn {
        client.spawn_pane(id, generation, command, cwd, cols, rows, keep_open, title);
        Update::with_command(open_timers_command(
            ctx.state.runtime_epoch,
            id,
            generation,
            open_delay,
            activate_delay,
        ))
    } else {
        Update::with_command(spawn_pty_command(
            ctx.state.runtime_epoch,
            id,
            generation,
            pty_config,
            Some((open_delay, activate_delay)),
        ))
    };
    (id, update)
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
    let placements = {
        let workspace = &ctx.state.workspaces[ctx.state.active_workspace];
        workspace_target_rects(workspace, bounds, top_gap)
    };
    let mut generation = None;
    let client = ctx.state.session_client.clone();
    if let Some(pane) = find_pane_mut(&mut ctx.state, id)
        && !pane.closing
    {
        generation = Some(pane.pty_generation);
        if pane.terminal.is_server_backed()
            && let Some(client) = client
        {
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

pub(crate) fn initial_command(
    spawns: Vec<StartupSpawn>,
    theme_tick: bool,
    bar_tick: bool,
    bar_commands: Vec<(String, u64)>,
    control_listener: Option<std::os::unix::net::UnixListener>,
) -> Option<Command> {
    if spawns.is_empty()
        && !theme_tick
        && !bar_tick
        && bar_commands.is_empty()
        && control_listener.is_none()
    {
        return None;
    }
    Some(Command::spawn(move |link: CommandLink<Msg>| {
        if let Some(listener) = control_listener {
            let listener_link = link.clone();
            std::thread::spawn(move || crate::control::run_listener(listener, listener_link));
        }
        for (epoch, id, generation, config, open_timers) in spawns {
            spawn_pty(epoch, id, generation, config, link.clone());
            if let Some((open_delay, activate_delay)) = open_timers {
                run_open_timers(
                    epoch,
                    id,
                    generation,
                    open_delay,
                    activate_delay,
                    link.clone(),
                );
            }
        }
        if theme_tick {
            std::thread::sleep(Duration::from_millis(150));
            link.send(Msg::ThemeTick);
        }
        if bar_tick {
            link.send(Msg::BarTick);
        }
        spawn_bar_command_pollers(bar_commands, &link);
    }))
}

/// Spawns one background thread per `(command, interval_secs)` pair that runs the shell
/// command, sends its output, sleeps, and repeats for the life of the app - the same
/// fire-and-forget pattern as the PTY read threads.
pub(crate) fn spawn_bar_command_pollers(bar_commands: Vec<(String, u64)>, link: &CommandLink<Msg>) {
    for (command, interval_secs) in bar_commands {
        let poller_link = link.clone();
        std::thread::spawn(move || {
            loop {
                let output = run_bar_command(&command);
                poller_link.send(Msg::BarCommandOutput(command.clone(), output));
                std::thread::sleep(Duration::from_secs(interval_secs.max(1)));
            }
        });
    }
}

/// Runs a `command:` bar segment's shell command through the user's shell and returns the
/// first line of stdout, trimmed. Failures (missing shell, non-zero exit, no output) collapse
/// to an empty string rather than surfacing an error in the bar.
fn run_bar_command(command: &str) -> String {
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

pub(crate) fn pty_config(config: &HyprmuxConfig) -> TerminalPtyConfig {
    let mut pty_config = if let Some(shell) = shell_for_config(config) {
        TerminalPtyConfig::new(shell)
    } else {
        TerminalPtyConfig::default()
    }
    .term("xterm-256color");

    if let Some(cwd) = &config.cwd {
        pty_config = pty_config.cwd(cwd.clone());
    }

    pty_config
}

fn shell_for_config(config: &HyprmuxConfig) -> Option<String> {
    config.shell.clone()
}

fn default_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string())
}

pub(crate) fn pty_config_for_pane(
    config: &HyprmuxConfig,
    control_socket_path: Option<&std::path::Path>,
    pane: &Pane,
) -> TerminalPtyConfig {
    let mut pty_config = if let Some(command) = pane
        .identity
        .command
        .as_deref()
        .filter(|command| !command.trim().is_empty())
    {
        let shell = shell_for_config(config).unwrap_or_else(default_shell);
        let wrapped = if pane.identity.keep_open {
            format!("{command}; exec {shell}")
        } else {
            command.to_string()
        };
        TerminalPtyConfig::new(shell)
            .arg("-lc")
            .arg(wrapped)
            .term("xterm-256color")
    } else {
        pty_config(config)
    };

    if let Some(cwd) = &config.cwd {
        pty_config = pty_config.cwd(cwd.clone());
    }

    if let Some(cwd) = &pane.identity.cwd {
        pty_config = pty_config.cwd(cwd.clone());
    }

    pty_config = pty_config
        .env("HYPRMUX", "1")
        .env("HYPRMUX_PANE", pane.id.to_string());
    if let Some(path) = control_socket_path {
        pty_config = pty_config.env("HYPRMUX_SOCKET", path.display().to_string());
    }

    pty_config
}

pub(crate) fn spawn_pty_command(
    epoch: u64,
    id: PaneId,
    generation: u64,
    config: TerminalPtyConfig,
    open_timers: OpenTimers,
) -> Command {
    Command::spawn(move |link: CommandLink<Msg>| {
        spawn_pty(epoch, id, generation, config, link.clone());
        if let Some((open_delay, activate_delay)) = open_timers {
            run_open_timers(epoch, id, generation, open_delay, activate_delay, link);
        }
    })
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

pub(crate) fn spawn_pty(
    epoch: u64,
    id: PaneId,
    generation: u64,
    config: TerminalPtyConfig,
    link: CommandLink<Msg>,
) {
    let event_link = link.clone();
    match TerminalPty::spawn(config, move |event| {
        event_link.send(Msg::PtyEvent(epoch, id, generation, event));
    }) {
        Ok(pty) => link.send(Msg::PtyReady(epoch, id, generation, pty)),
        Err(err) => link.send(Msg::PtyEvent(
            epoch,
            id,
            generation,
            TerminalPtyEvent::Error(err.to_string().into()),
        )),
    }
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

    fn rect() -> FloatRect {
        FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 24.0,
        }
    }

    #[test]
    fn pane_config_prefers_pane_cwd_and_wraps_command_in_shell() {
        let config = HyprmuxConfig {
            shell: Some("/bin/bash".to_string()),
            cwd: Some("/repo".into()),
            ..HyprmuxConfig::default()
        };

        let mut pane = Pane::new(1, 100, rect());
        pane.identity.cwd = Some("/repo/backend".to_string());
        pane.identity.command = Some("cargo run".to_string());

        let debug = format!("{:?}", pty_config_for_pane(&config, None, &pane));

        assert!(debug.contains("/bin/bash"), "{debug}");
        assert!(debug.contains("-lc"), "{debug}");
        assert!(debug.contains("cargo run"), "{debug}");
        assert!(debug.contains("/repo/backend"), "{debug}");
    }

    #[test]
    fn keep_open_wraps_command_with_exec_shell() {
        let config = HyprmuxConfig {
            shell: Some("/bin/bash".to_string()),
            ..HyprmuxConfig::default()
        };

        let mut pane = Pane::new(1, 100, rect());
        pane.identity.command = Some("lazygit".to_string());
        pane.identity.keep_open = true;

        let debug = format!("{:?}", pty_config_for_pane(&config, None, &pane));

        assert!(debug.contains("lazygit; exec /bin/bash"), "{debug}");
    }

    #[test]
    fn stale_prune_token_does_not_match_reused_pane_id() {
        let mut state = State::new(HyprmuxConfig::default(), Theme::default());
        state.workspaces[0].panes[0].pty_generation = 42;
        state.workspaces[0].panes[0].closing = true;
        assert!(should_prune_closed(&state, 1, 42));
        assert!(!should_prune_closed(&state, 1, 41));

        state.workspaces[0].panes[0].closing = false;
        assert!(!should_prune_closed(&state, 1, 42));
    }
}
