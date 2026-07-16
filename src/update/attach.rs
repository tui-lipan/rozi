use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::pane_lifecycle::{find_pane_mut, pane_env};
use crate::state::State;
use crate::tiling::append_tiled_window;

/// Clear all window-manager structure so the shared-layout reconciler can rebuild it from scratch
/// as pure additions. Used only on attach to a session that already carries an authoritative
/// layout (the client's throwaway local panes are discarded in favor of the server's).
pub(super) fn reset_state_for_shared_seed(state: &mut State) {
    for workspace in &mut state.workspaces {
        workspace.panes.clear();
        workspace.tile_tree = None;
        workspace.focused_pane = None;
    }
    state.focused_pane = None;
    state.active_workspace = 0;
    state.next_pane_id = 1;
    state.next_pty_generation = 1;
}

/// After the reconciler has created panes from the shared layout, bind each one's server backend at
/// the authoritative size and stamp its live metadata (title, cwd, pid) from the attach frame, so
/// replay seed frames land on a correctly sized screen.
pub(super) fn bind_attached_pane_backends(
    ctx: &mut Context<HyprmuxApp>,
    panes: Vec<crate::session::protocol::PaneMeta>,
) {
    for meta in panes {
        if let Some(pane) = find_pane_mut(&mut ctx.state, meta.pane_id) {
            pane.opening = false;
            pane.terminal_active = true;
            pane.pty_generation = meta.generation;
            pane.terminal.cols = meta.cols.max(1);
            pane.terminal.rows = meta.rows.max(1);
            pane.terminal
                .bind_server_backend(meta.pane_id, meta.generation);
            pane.terminal.title = meta.title.filter(|title| !title.trim().is_empty());
            pane.terminal.cwd = meta.runtime.cwd.clone();
            pane.terminal.cwd_host = meta.runtime.cwd_host.clone();
            pane.terminal.foreground_program = meta.runtime.foreground_program.clone();
            pane.terminal.command_phase = meta.runtime.command_phase;
            pane.terminal.last_exit_status = meta.runtime.last_exit_status;
            pane.terminal.runtime_sequence = meta.runtime.sequence;
            pane.terminal.child_pid = meta.pid;
            pane.logging = meta.logging;
            pane.terminal.status = ManagedTerminalStatus::Ready;
        }
        // The popup slot's reserved id (u32::MAX) must never feed the allocator: bumping past it
        // would pin next_pane_id at MAX and collide every later spawn with the popup slot.
        if meta.pane_id != crate::state::POPUP_PANE_ID {
            ctx.state.next_pane_id = ctx.state.next_pane_id.max(meta.pane_id.saturating_add(1));
        }
        ctx.state.next_pty_generation = ctx
            .state
            .next_pty_generation
            .max(meta.generation.saturating_add(1));
    }
}

/// Defensive fallback: adopt server panes when a live session reports panes but no committed layout
/// (should not happen since the shared-layout protocol landed). Rebuilds a flat tiled workspace
/// from the pane list.
pub(super) fn apply_attached_panes(
    ctx: &mut Context<HyprmuxApp>,
    panes: Vec<crate::session::protocol::PaneMeta>,
) {
    for workspace in &mut ctx.state.workspaces {
        workspace.panes.clear();
        workspace.tile_tree = None;
        workspace.focused_pane = None;
    }
    ctx.state.focused_pane = None;

    for attached in panes {
        if find_pane_mut(&mut ctx.state, attached.pane_id).is_none() {
            let rect = FloatRect {
                x: 4.0,
                y: 3.0,
                w: 80.0,
                h: 24.0,
            };
            let pane = crate::state::Pane::new(attached.pane_id, ctx.state.config.scrollback, rect);
            ctx.state.workspaces[0].panes.push(pane);
            append_tiled_window(&mut ctx.state.workspaces[0], attached.pane_id);
        }
        if let Some(pane) = find_pane_mut(&mut ctx.state, attached.pane_id) {
            pane.opening = false;
            pane.terminal_active = true;
            pane.pty_generation = attached.generation;
            pane.terminal.cols = attached.cols.max(1);
            pane.terminal.rows = attached.rows.max(1);
            pane.terminal
                .bind_server_backend(attached.pane_id, attached.generation);
            pane.terminal.title = attached.title.filter(|title| !title.trim().is_empty());
            pane.terminal.cwd = attached.runtime.cwd.clone();
            pane.terminal.cwd_host = attached.runtime.cwd_host.clone();
            pane.terminal.foreground_program = attached.runtime.foreground_program.clone();
            pane.terminal.command_phase = attached.runtime.command_phase;
            pane.terminal.last_exit_status = attached.runtime.last_exit_status;
            pane.terminal.runtime_sequence = attached.runtime.sequence;
            pane.terminal.child_pid = attached.pid;
            pane.logging = attached.logging;
            pane.terminal.status = ManagedTerminalStatus::Ready;
        }
        if attached.pane_id != crate::state::POPUP_PANE_ID {
            ctx.state.next_pane_id = ctx
                .state
                .next_pane_id
                .max(attached.pane_id.saturating_add(1));
        }
        ctx.state.next_pty_generation = ctx
            .state
            .next_pty_generation
            .max(attached.generation.saturating_add(1));
    }

    if ctx.state.focused_pane.is_none() {
        ctx.state.focused_pane = ctx.state.workspaces[0].panes.first().map(|pane| pane.id);
        ctx.state.workspaces[0].focused_pane = ctx.state.focused_pane;
    }
}

/// Spawn every non-closing pane the client holds in state on a freshly attached (empty) session.
/// Used on initial attach and after detach when the new ephemeral server owns no panes yet.
/// Spawn the panes the client already holds in state onto the freshly attached session, returning
/// their `(pane_id, generation)` so the caller can schedule the open/activate reveal timers (these
/// panes start with `opening = true` and would otherwise stay invisible).
pub(crate) fn spawn_state_panes_on_session(
    ctx: &mut Context<HyprmuxApp>,
) -> Vec<(crate::state::PaneId, u64)> {
    let Some(client) = ctx.state.session_client.clone() else {
        return Vec::new();
    };
    // Fallback palette for any pane whose screen never cached one, so the server seeds a theme
    // palette before the PTY spawns and the child's startup color queries are answered correctly.
    let fallback_palette = crate::ops::theme::terminal_palette(
        &ctx.state.theme,
        crate::ops::theme::pane_frame_background(
            &ctx.state.theme,
            false,
            ctx.state.config.pane.highlight_focused_background,
        ),
    );
    let shell_env = crate::platform::command::ShellEnv::from_process();
    let (shell, integration_env) = crate::platform::shell_integration::resolve_interactive_shell(
        ctx.state.config.shell.as_deref(),
        &shell_env,
        ctx.state.config.shell_integration.mode,
        &crate::platform::shell_integration::InjectionEnv::from_process(),
    );
    let command_shell = crate::platform::command::resolve_command_shell(
        ctx.state.config.command_shell.as_deref(),
        &shell_env,
    );
    let shell = shell.as_argv();
    let command_shell = command_shell.as_argv();
    let mut targets = Vec::new();
    for pane in ctx
        .state
        .workspaces
        .iter_mut()
        .flat_map(|workspace| workspace.panes.iter_mut())
        .filter(|pane| !pane.closing)
    {
        let generation = ctx.state.next_pty_generation;
        ctx.state.next_pty_generation = ctx.state.next_pty_generation.saturating_add(1);
        pane.pty_generation = generation;
        pane.terminal.bind_server_backend(pane.id, generation);
        let env = integration_env
            .iter()
            .cloned()
            .chain(pane_env(ctx.state.control_socket_path.as_deref(), pane))
            .collect::<Vec<_>>();
        // A replay command is not sent as the spawn command: the pane starts as a plain
        // interactive shell and the command is injected as type-ahead input once the spawn
        // succeeds (see `State::pending_replay_inputs`).
        let command = if pane.identity.replay {
            if let Some(command) = pane.identity.command.clone() {
                ctx.state
                    .pending_replay_inputs
                    .insert((pane.id, generation), command);
            }
            None
        } else {
            pane.identity.command.clone()
        };
        client.spawn_pane(
            pane.id,
            generation,
            command,
            pane.identity.cwd.clone(),
            pane.terminal.cols,
            pane.terminal.rows,
            pane.identity.keep_open,
            env,
            pane.identity.custom_title.clone(),
            pane.terminal.last_palette.unwrap_or(fallback_palette),
            shell.clone(),
            command_shell.clone(),
        );
        targets.push((pane.id, generation));
    }
    targets
}

/// Flush pane spawns that were queued while no client was connected (see
/// [`crate::state::State::pending_spawns`]).
pub(super) fn flush_pending_spawns(ctx: &mut Context<HyprmuxApp>) {
    let Some(client) = ctx.state.session_client.clone() else {
        return;
    };
    for spawn in std::mem::take(&mut ctx.state.pending_spawns) {
        client.spawn_pane(
            spawn.pane_id,
            spawn.generation,
            spawn.command,
            spawn.cwd,
            spawn.cols,
            spawn.rows,
            spawn.keep_open,
            spawn.env,
            spawn.title,
            spawn.palette,
            spawn.shell,
            spawn.command_shell,
        );
    }
}
