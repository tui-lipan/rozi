use std::time::Duration;

use tui_lipan::prelude::*;

use crate::anim::{self, GeometryAnimation, WindowAnimationConfig};
use crate::focus_ops::{
    choose_fallback_focus, first_visible_pane, focus_near_pane_in_workspace, reference_pane_rect,
    request_current_pane_focus, request_pane_focus, total_visible_panes,
};
use crate::geometry::{canvas_bounds_from_viewport, default_floating_rect};
use crate::layout::{place_spawned_pane, placement_for, workspace_target_rects};
use crate::state::{HyprmuxConfig, Pane, PaneId, State};
use crate::theme_ops::terminal_palette;
use crate::tiling::remove_tiled_window;
use crate::{HyprmuxApp, Msg};

pub(crate) fn spawn_pane(ctx: &mut Context<HyprmuxApp>) -> Update {
    let bounds = canvas_bounds_from_viewport(ctx.viewport());
    let id = ctx.state.next_pane_id;
    ctx.state.next_pane_id = ctx.state.next_pane_id.saturating_add(1);
    let floating_rect = default_floating_rect(bounds, id);
    let mut pane = Pane::new(id, ctx.state.config.scrollback, floating_rect);
    pane.terminal
        .set_palette(terminal_palette(&ctx.state.theme));
    pane.opening = true;

    let workspace = &ctx.state.workspaces[ctx.state.active_workspace];
    let previous_focused = workspace.focused_pane;
    // A new pane opens in the focused pane's live working directory (falling back to the
    // configured cwd when the focused pane is floating or its cwd is unknown).
    if let Some(cwd) = previous_focused
        .and_then(|id| workspace.panes.iter().find(|pane| pane.id == id))
        .and_then(|pane| pane.live_cwd())
    {
        pane.identity.cwd = Some(cwd);
    }
    let pty_config = pty_config_for_pane(&ctx.state.config, &pane);

    let workspace = &mut ctx.state.workspaces[ctx.state.active_workspace];
    workspace.panes.push(pane);
    place_spawned_pane(workspace, id, previous_focused, bounds);
    workspace.focused_pane = Some(id);
    ctx.state.focused_pane = Some(id);
    request_pane_focus(ctx, id);
    ctx.state.animation = GeometryAnimation::Spawn;

    Update::with_command(spawn_pty_command(
        id,
        pty_config,
        Some(anim::open_delay(ctx.state.config.animations)),
    ))
}

pub(crate) fn begin_close_pane(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    animations: WindowAnimationConfig,
) -> Update {
    let bounds = canvas_bounds_from_viewport(ctx.viewport());
    let placements = {
        let workspace = &ctx.state.workspaces[ctx.state.active_workspace];
        workspace_target_rects(workspace, bounds)
    };
    let mut closed = false;
    if let Some(pane) = find_pane_mut(&mut ctx.state, id)
        && !pane.closing
    {
        pane.floating_rect = placement_for(&placements, id).unwrap_or(pane.floating_rect);
        pane.opening = false;
        pane.closing = true;
        pane.terminal.kill();
        closed = true;
    }

    if closed {
        ctx.state.animation = GeometryAnimation::Close;
        choose_fallback_focus(&mut ctx.state);
        request_current_pane_focus(ctx);
        Update::with_command(prune_closed_command(id, anim::close_delay(animations)))
    } else {
        Update::full()
    }
}

pub(crate) fn close_focused_pane(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(id) = ctx.state.focused_pane else {
        return Update::full();
    };
    begin_close_pane(ctx, id, ctx.state.config.animations)
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

pub(crate) fn handle_prune_closed(ctx: &mut Context<HyprmuxApp>, id: PaneId) -> Update {
    remove_pane(&mut ctx.state, id);
    if ctx
        .state
        .search
        .as_ref()
        .is_some_and(|search| search.target == id)
    {
        ctx.state.search = None;
    }
    if total_visible_panes(&ctx.state) == 0 {
        crate::profiles::persist_session_if_enabled(&ctx.state);
        ctx.quit();
        return Update::none();
    }
    request_current_pane_focus(ctx);
    Update::full()
}

pub(crate) fn initial_command(
    spawns: Vec<(PaneId, TerminalPtyConfig, Option<Duration>)>,
    theme_tick: bool,
    bar_tick: bool,
) -> Option<Command> {
    if spawns.is_empty() && !theme_tick && !bar_tick {
        return None;
    }
    Some(Command::spawn(move |link: CommandLink<Msg>| {
        for (id, config, finish_open_after) in spawns {
            spawn_pty(id, config, link.clone());
            if let Some(delay) = finish_open_after {
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                link.send(Msg::FinishOpen(id));
            }
        }
        if theme_tick {
            std::thread::sleep(Duration::from_millis(150));
            link.send(Msg::ThemeTick);
        }
        if bar_tick {
            link.send(Msg::BarTick);
        }
    }))
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

pub(crate) fn pty_config_for_pane(config: &HyprmuxConfig, pane: &Pane) -> TerminalPtyConfig {
    let mut pty_config = if let Some(command) = pane
        .identity
        .command
        .as_deref()
        .filter(|command| !command.trim().is_empty())
    {
        let shell = shell_for_config(config).unwrap_or_else(default_shell);
        TerminalPtyConfig::new(shell)
            .arg("-lc")
            .arg(command.to_string())
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

    pty_config
}

pub(crate) fn spawn_pty_command(
    id: PaneId,
    config: TerminalPtyConfig,
    finish_open_after: Option<Duration>,
) -> Command {
    Command::spawn(move |link: CommandLink<Msg>| {
        spawn_pty(id, config, link.clone());
        if let Some(delay) = finish_open_after {
            if !delay.is_zero() {
                std::thread::sleep(delay);
            }
            link.send(Msg::FinishOpen(id));
        }
    })
}

pub(crate) fn spawn_pty(id: PaneId, config: TerminalPtyConfig, link: CommandLink<Msg>) {
    let event_link = link.clone();
    match TerminalPty::spawn(config, move |event| {
        event_link.send(Msg::PtyEvent(id, event));
    }) {
        Ok(pty) => link.send(Msg::PtyReady(id, pty)),
        Err(err) => link.send(Msg::PtyEvent(
            id,
            TerminalPtyEvent::Error(err.to_string().into()),
        )),
    }
}

pub(crate) fn prune_closed_command(id: PaneId, delay: Duration) -> Command {
    Command::spawn(move |link: CommandLink<Msg>| {
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        link.send(Msg::PruneClosed(id));
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
        let mut config = HyprmuxConfig::default();
        config.shell = Some("/bin/bash".to_string());
        config.cwd = Some("/repo".into());

        let mut pane = Pane::new(1, 100, rect());
        pane.identity.cwd = Some("/repo/backend".to_string());
        pane.identity.command = Some("cargo run".to_string());

        let debug = format!("{:?}", pty_config_for_pane(&config, &pane));

        assert!(debug.contains("/bin/bash"), "{debug}");
        assert!(debug.contains("-lc"), "{debug}");
        assert!(debug.contains("cargo run"), "{debug}");
        assert!(debug.contains("/repo/backend"), "{debug}");
    }
}
