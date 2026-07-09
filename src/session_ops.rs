use tui_lipan::prelude::*;

use crate::focus_ops::request_session_picker_focus;
use crate::state::SessionPickerState;
use crate::{HyprmuxApp, pty_events::info_toast};

pub(crate) fn open_session_picker(ctx: &mut Context<HyprmuxApp>) -> Update {
    match discovered_with_current(ctx) {
        Ok(rows) => ctx.state.session_picker = Some(SessionPickerState::new(rows)),
        Err(err) => {
            ctx.toast().push(crate::pty_events::error_toast(
                &ctx.state.theme,
                "Sessions",
                err.to_string(),
            ));
            ctx.state.session_picker = Some(SessionPickerState::new(Vec::new()));
        }
    }
    ctx.state.show_session_picker = true;
    request_session_picker_focus(ctx);
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
            ctx.toast().push(crate::pty_events::error_toast(
                &ctx.state.theme,
                "Sessions",
                err.to_string(),
            ));
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
            ephemeral: ctx.state.is_ephemeral_session(),
            status: crate::session::discovery::DiscoveredSessionStatus::Running {
                panes: ctx.state.workspaces.iter().map(|w| w.panes.len()).sum(),
                has_layout: true,
            },
        });
        rows.sort_by(|a, b| a.name.cmp(&b.name));
    }
    Ok(rows)
}

/// Detach the current session (leaving its server running) and switch the UI to a fresh ephemeral
/// session, so the user keeps a working terminal while the old session stays parked for reattach.
pub(crate) fn detach_current_session(ctx: &mut Context<HyprmuxApp>) -> Update {
    if !ctx.state.session_attached {
        ctx.toast()
            .push(info_toast(&ctx.state.theme, "Not attached to a session"));
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
    let old_epoch = ctx.state.runtime_epoch;
    let epoch = old_epoch.saturating_add(1);
    let name = crate::state::fresh_ephemeral_session_name(epoch);
    let mut fresh = crate::state::State::new(config, theme);
    fresh.theme_watcher = theme_watcher;
    fresh.system_theme = system_theme;
    fresh.control_socket_path = control_socket_path;
    // Keep the pre-attach epoch so stale messages from the just-detached connection are filtered
    // out; `Msg::SessionAttached` advances it to `epoch` once the fresh ephemeral is live.
    fresh.runtime_epoch = old_epoch;
    fresh.pending_session_attach = Some(crate::state::PendingSessionAttach {
        epoch,
        name: name.clone(),
        client: None,
        autostart: true,
    });
    ctx.state = fresh;
    ctx.state.commands_dirty = true;
    crate::theme_ops::apply_terminal_palette_to_state(&mut ctx.state);
    ctx.toast().push(info_toast(
        &ctx.state.theme,
        "Detached (session still running)",
    ));
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || crate::attach_session_client(epoch, name, true, link));
    }))
}

pub(crate) fn attach_session_by_name(
    ctx: &mut Context<HyprmuxApp>,
    name: String,
    autostart: bool,
) -> Update {
    if !crate::session::discovery::valid_session_name(&name) {
        ctx.toast().push(crate::pty_events::error_toast(
            &ctx.state.theme,
            "Sessions",
            "Invalid session name",
        ));
        return Update::full();
    }
    if ctx.state.session_attached && ctx.state.session_name.as_deref() == Some(name.as_str()) {
        ctx.toast().push(info_toast(
            &ctx.state.theme,
            format!("Already attached to `{name}`"),
        ));
        return Update::full();
    }
    if ctx.state.pending_session_attach.is_some() {
        ctx.toast()
            .push(info_toast(&ctx.state.theme, "Attach already in progress"));
        return Update::full();
    }
    // Attach-elsewhere: park the current session (push its layout, detach) and leave its server
    // running so it can be reattached later. Leaving an ephemeral is allowed but noted, since a
    // clean quit is the only thing that shuts an ephemeral down.
    if let Some(client) = ctx.state.session_client.clone() {
        client.push_layout(
            crate::profiles::profile_from_state(&ctx.state)
                .to_toml_string()
                .unwrap_or_default(),
        );
        client.detach();
        if ctx.state.is_ephemeral_session() {
            ctx.toast().push(info_toast(
                &ctx.state.theme,
                "Left ephemeral session running — reattach from the session picker",
            ));
        }
    }
    ctx.state.show_session_picker = false;
    ctx.state.session_picker = None;
    ctx.state.commands_dirty = true;
    let epoch = ctx.state.runtime_epoch.saturating_add(1);
    ctx.state.pending_session_attach = Some(crate::state::PendingSessionAttach {
        epoch,
        name: name.clone(),
        client: None,
        autostart,
    });
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || crate::attach_session_client(epoch, name, autostart, link));
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
    // A session shown in the picker is already running, so don't autostart a replacement if it
    // died between discovery and attach.
    attach_session_by_name(ctx, name, false)
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
            &ctx.state.theme,
            "Sessions",
            "Use letters, numbers, _ or - for session names",
        ));
        return Update::full();
    }
    // From an ephemeral session, "create named" is a rename in place: the same server keeps its
    // live panes and simply becomes discoverable under the new name — zero pane movement. The
    // `Renamed` reply updates `session_name` and toasts (a collision surfaces as an error).
    if ctx.state.session_attached && ctx.state.is_ephemeral_session() {
        if let Some(client) = ctx.state.session_client.clone() {
            client.rename(name);
            ctx.state.show_session_picker = false;
            ctx.state.session_picker = None;
            return Update::full();
        }
    }
    // From a named session (or when not attached), start/attach a separate named server.
    attach_session_by_name(ctx, name, true)
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
        ctx.toast().push(info_toast(
            &ctx.state.theme,
            format!("Press Ctrl+K again to kill `{}`", entry.name),
        ));
        return Update::full();
    }
    picker.pending_kill = None;
    if ctx.state.session_attached && ctx.state.session_name.as_deref() == Some(entry.name.as_str())
    {
        ctx.toast().push(crate::pty_events::error_toast(
            &ctx.state.theme,
            "Sessions",
            "Detach before killing the current session",
        ));
        return Update::full();
    }
    match shutdown_session(&entry.name) {
        Ok(()) => {
            ctx.toast().push(info_toast(
                &ctx.state.theme,
                format!("Killed session `{}`", entry.name),
            ));
            refresh_session_picker(ctx)
        }
        Err(err) => {
            ctx.toast().push(crate::pty_events::error_toast(
                &ctx.state.theme,
                "Sessions",
                err.to_string(),
            ));
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
