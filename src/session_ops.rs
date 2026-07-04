use tui_lipan::prelude::*;

use crate::state::SessionPickerState;
use crate::{HyprmuxApp, pane_lifecycle, pty_events::info_toast, startup_spawns};

pub(crate) fn open_session_picker(ctx: &mut Context<HyprmuxApp>) -> Update {
    match discovered_with_current(ctx) {
        Ok(rows) => ctx.state.session_picker = Some(SessionPickerState::new(rows)),
        Err(err) => {
            ctx.toast()
                .push(crate::pty_events::error_toast("Sessions", err.to_string()));
            ctx.state.session_picker = Some(SessionPickerState::new(Vec::new()));
        }
    }
    ctx.state.show_session_picker = true;
    Update::full()
}

pub(crate) fn refresh_session_picker(ctx: &mut Context<HyprmuxApp>) -> Update {
    match discovered_with_current(ctx) {
        Ok(rows) => {
            let query = ctx
                .state
                .session_picker
                .as_ref()
                .map(|p| p.input.text().to_string())
                .unwrap_or_default();
            let mut picker = SessionPickerState::new(rows);
            picker.input.set_text(query);
            ctx.state.session_picker = Some(picker);
        }
        Err(err) => {
            ctx.toast()
                .push(crate::pty_events::error_toast("Sessions", err.to_string()));
        }
    };
    Update::full()
}

fn discovered_with_current(
    ctx: &Context<HyprmuxApp>,
) -> std::io::Result<Vec<crate::session::discovery::DiscoveredSession>> {
    let current_name = ctx.state.session_name.as_deref();
    let mut rows = crate::session::discovery::discover_sessions_excluding(current_name)?;
    if let Some(name) = &ctx.state.session_name {
        rows.push(crate::session::discovery::DiscoveredSession {
            name: name.clone(),
            status: crate::session::discovery::DiscoveredSessionStatus::Running {
                panes: ctx.state.workspaces.iter().map(|w| w.panes.len()).sum(),
                has_layout: true,
            },
        });
        rows.sort_by(|a, b| a.name.cmp(&b.name));
    }
    Ok(rows)
}

pub(crate) fn detach_current_session(ctx: &mut Context<HyprmuxApp>) -> Update {
    if !ctx.state.session_attached {
        ctx.toast().push(info_toast("Not attached to a session"));
        return Update::full();
    }
    if let Some(client) = ctx.state.session_client.clone() {
        client.push_layout(
            crate::profiles::profile_from_state(&ctx.state)
                .to_toml_string()
                .unwrap_or_default(),
        );
        client.detach();
    }
    let config = ctx.state.config.clone();
    let theme = ctx.state.theme.clone();
    let theme_watcher = ctx.state.theme_watcher.take();
    let system_theme = ctx.state.system_theme.clone();
    let control_socket_path = ctx.state.control_socket_path.clone();
    let epoch = ctx
        .state
        .pending_session_attach
        .as_ref()
        .map(|pending| pending.epoch.saturating_add(1))
        .unwrap_or_else(|| ctx.state.runtime_epoch.saturating_add(1));
    let mut fresh = crate::state::State::new(config, theme);
    fresh.theme_watcher = theme_watcher;
    fresh.system_theme = system_theme;
    fresh.control_socket_path = control_socket_path;
    fresh.runtime_epoch = epoch;
    ctx.state = fresh;
    crate::theme_ops::apply_terminal_palette_to_state(&mut ctx.state);
    ctx.toast()
        .push(info_toast("Detached; remote session remains running"));
    Update::with_command(pane_lifecycle::initial_command(
        startup_spawns(&mut ctx.state),
        false,
        false,
        None,
    ))
}

#[allow(dead_code)]
pub(crate) fn attach_session_by_name(ctx: &mut Context<HyprmuxApp>, name: String) -> Update {
    if !crate::session::discovery::valid_session_name(&name) {
        ctx.toast().push(crate::pty_events::error_toast(
            "Sessions",
            "Invalid session name",
        ));
        return Update::full();
    }
    if ctx.state.session_attached && ctx.state.session_name.as_deref() == Some(name.as_str()) {
        ctx.toast()
            .push(info_toast(format!("Already attached to `{name}`")));
        return Update::full();
    }
    if ctx.state.pending_session_attach.is_some() {
        ctx.toast()
            .push(info_toast("A session attach is already in progress"));
        return Update::full();
    }
    if let Some(client) = ctx.state.session_client.clone() {
        client.push_layout(
            crate::profiles::profile_from_state(&ctx.state)
                .to_toml_string()
                .unwrap_or_default(),
        );
    }
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
    let epoch = ctx.state.runtime_epoch.saturating_add(1);
    ctx.state.pending_session_attach = Some(crate::state::PendingSessionAttach {
        epoch,
        name: name.clone(),
        client: None,
        migrate_local_panes: !ctx.state.session_attached,
    });
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || crate::attach_session_client(epoch, name, link));
    }))
}

pub(crate) fn activate_selected_session(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    let Some(name) = ctx
        .state
        .session_picker
        .as_ref()
        .and_then(|picker| picker.entries.get(index).map(|entry| entry.name.clone()))
    else {
        return Update::full();
    };
    attach_session_by_name(ctx, name)
}

pub(crate) fn create_from_query(ctx: &mut Context<HyprmuxApp>) -> Update {
    let name = ctx
        .state
        .session_picker
        .as_ref()
        .map(|picker| picker.input.text().trim().to_string())
        .unwrap_or_default();
    if name.is_empty() || !crate::session::discovery::valid_session_name(&name) {
        ctx.toast().push(crate::pty_events::error_toast(
            "Sessions",
            "Use letters, numbers, _ or - for session names",
        ));
        return Update::full();
    }
    attach_session_by_name(ctx, name)
}

pub(crate) fn kill_selected_session(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(picker) = ctx.state.session_picker.as_mut() else {
        return Update::full();
    };
    let index = picker.selected.min(picker.entries.len().saturating_sub(1));
    let Some(entry) = picker.entries.get(index).cloned() else {
        return Update::full();
    };
    if picker.pending_kill != Some(index) {
        picker.pending_kill = Some(index);
        ctx.toast().push(info_toast(format!(
            "Press Ctrl+K again to kill `{}`",
            entry.name
        )));
        return Update::full();
    }
    picker.pending_kill = None;
    if ctx.state.session_attached && ctx.state.session_name.as_deref() == Some(entry.name.as_str())
    {
        ctx.toast().push(crate::pty_events::error_toast(
            "Sessions",
            "Detach before killing the current session",
        ));
        return Update::full();
    }
    match shutdown_session(&entry.name) {
        Ok(()) => {
            ctx.toast()
                .push(info_toast(format!("Killed session `{}`", entry.name)));
            refresh_session_picker(ctx)
        }
        Err(err) => {
            ctx.toast()
                .push(crate::pty_events::error_toast("Sessions", err.to_string()));
            Update::full()
        }
    }
}

fn shutdown_session(name: &str) -> std::io::Result<()> {
    use crate::session::protocol::{ClientMessage, PROTOCOL_VERSION};
    let path = crate::session::server::session_socket_path(name)?;
    let mut stream = std::os::unix::net::UnixStream::connect(&path)?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(1)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(1)))?;
    crate::session::protocol::write_frame(
        &mut stream,
        &ClientMessage::Attach {
            session: name.to_string(),
            protocol_version: PROTOCOL_VERSION,
        },
    )?;
    let _ = crate::session::protocol::read_frame::<_, crate::session::protocol::ServerMessage>(
        &mut stream,
    )?;
    crate::session::protocol::write_frame(&mut stream, &ClientMessage::Shutdown)?;
    // The server unlinks its socket only once it finishes tearing down, which races the refresh
    // that follows a kill. Drop the path now so the dead session leaves the list immediately.
    let _ = std::fs::remove_file(&path);
    Ok(())
}
