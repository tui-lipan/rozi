use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::config::{
    clear_default_profile, delete_profile_file, list_profiles, persist_default_profile,
    profile_path_for_name,
};
use crate::ops::focus::request_profile_picker_focus;
use crate::ops::focus::request_save_profile_focus;
use crate::profiles::{load_profile, profile_from_state, save_profile};
use crate::pty_events::{error_toast, info_toast};
use crate::state::{Mode, ProfilePickerState, SaveProfileState};

pub(crate) fn open_save_profile_prompt(ctx: &mut Context<HyprmuxApp>) -> Update {
    let initial = ctx
        .state
        .current()
        .created_from_profile
        .as_deref()
        .or_else(|| {
            ctx.state.current().session_name.as_deref().filter(|name| {
                ctx.state.current().session_attached
                    && !crate::state::is_ephemeral_session_name(name)
            })
        })
        .unwrap_or("");
    ctx.state.save_profile_prompt = Some(SaveProfileState::new(initial));
    ctx.state.show_palette = false;
    ctx.state.show_help = false;
    ctx.state.search = None;
    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
    ctx.state.mode = Mode::Normal;
    request_save_profile_focus(ctx);
    Update::full()
}

pub(crate) fn close_save_profile_prompt(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.save_profile_prompt = None;
    ctx.state.commands_dirty = true;
    Update::full()
}

pub(crate) fn submit_save_profile(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(name) = ctx
        .state
        .save_profile_prompt
        .as_ref()
        .and_then(|prompt| normalize_profile_name(prompt.input.text()))
    else {
        ctx.toast().push(error_toast(
            &ctx.state.theme,
            "Invalid profile name",
            "Use letters, numbers, _ or -",
        ));
        request_save_profile_focus(ctx);
        return Update::full();
    };

    let path = profile_path_for_name(&name);
    let existed = path.exists();
    if existed
        && !ctx
            .state
            .save_profile_prompt
            .as_ref()
            .is_some_and(|prompt| prompt.pending_overwrite)
    {
        if let Some(prompt) = ctx.state.save_profile_prompt.as_mut() {
            prompt.pending_overwrite = true;
        }
        request_save_profile_focus(ctx);
        return Update::full();
    }
    let profile = profile_from_state(&ctx.state);
    match save_profile(&path, &profile) {
        Ok(()) => {
            crate::events::emit(
                &ctx.state,
                crate::events::Event::new(
                    crate::events::EventKind::ProfileSaved,
                    vec![
                        ("profile", name.clone()),
                        ("path", path.display().to_string()),
                    ],
                ),
            );
            ctx.toast().push(info_toast(
                &ctx.state.theme,
                format!(
                    "{} profile `{name}`",
                    if existed { "Overwrote" } else { "Captured" }
                ),
            ));
        }
        Err(message) => {
            ctx.toast()
                .push(error_toast(&ctx.state.theme, "Capture failed", message));
        }
    }

    ctx.state.save_profile_prompt = None;
    ctx.state.commands_dirty = true;
    Update::full()
}

fn normalize_profile_name(name: &str) -> Option<String> {
    let name = name.trim();
    crate::session::discovery::valid_session_name(name).then(|| name.to_string())
}

pub(crate) fn open_profile_picker(ctx: &mut Context<HyprmuxApp>) -> Update {
    open_profile_picker_mode(ctx, false)
}

pub(crate) fn open_apply_profile_picker(ctx: &mut Context<HyprmuxApp>) -> Update {
    open_profile_picker_mode(ctx, true)
}

fn open_profile_picker_mode(ctx: &mut Context<HyprmuxApp>, apply_mode: bool) -> Update {
    let entries = list_profiles();
    ctx.state.profile_picker_epoch = ctx.state.profile_picker_epoch.wrapping_add(1);
    let epoch = ctx.state.profile_picker_epoch;
    let rows = profile_session_rows(ctx);
    let mut picker = ProfilePickerState::new(entries);
    picker.apply_mode = apply_mode;
    picker.running = rows.into_iter().map(|row| (row.name, row.status)).collect();
    ctx.state.profile_picker = Some(picker);
    ctx.state.show_profile_picker = true;
    ctx.state.show_palette = false;
    ctx.state.show_help = false;
    ctx.state.search = None;
    ctx.state.rename = None;
    ctx.state.save_profile_prompt = None;
    ctx.state.mode = Mode::Normal;
    request_profile_picker_focus(ctx);
    Update::with_command(profile_session_watch_command(
        epoch,
        ctx.state.current().session_name.clone(),
    ))
}

fn profile_session_rows(
    ctx: &Context<HyprmuxApp>,
) -> Vec<crate::session::discovery::DiscoveredSession> {
    let current = ctx.state.current().session_name.as_deref();
    let mut rows =
        crate::session::discovery::discover_sessions_excluding(current).unwrap_or_default();
    rows.retain(|row| !row.ephemeral);
    if let Some(name) = &ctx.state.current().session_name {
        rows.push(crate::session::discovery::DiscoveredSession {
            name: name.clone(),
            ephemeral: ctx.state.is_ephemeral_session(),
            host: None,
            remote_target: None,
            status: crate::session::discovery::DiscoveredSessionStatus::Running {
                panes: ctx
                    .state
                    .current()
                    .workspaces
                    .iter()
                    .map(|workspace| workspace.panes.len())
                    .sum(),
                clients: ctx.state.attached_client_count(),
                has_layout: true,
                created_from_profile: ctx.state.current().created_from_profile.clone(),
            },
        });
    }
    rows
}

fn profile_session_watch_command(epoch: u64, current: Option<String>) -> Command {
    // Recurring watch: `after` keeps the wait off the executor, so a picker left open does not
    // hold a worker between sweeps. The discovery itself still runs on the pool.
    Command::after(
        std::time::Duration::from_millis(1500),
        move |link: CommandLink<crate::Msg>| {
            if let Ok(mut rows) =
                crate::session::discovery::discover_sessions_excluding(current.as_deref())
            {
                rows.retain(|row| !row.ephemeral);
                link.send(crate::Msg::ProfileSessionsDiscovered { epoch, rows });
            }
        },
    )
}

pub(crate) fn apply_profile_sessions(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    mut rows: Vec<crate::session::discovery::DiscoveredSession>,
) -> Update {
    if !ctx.state.show_profile_picker || epoch != ctx.state.profile_picker_epoch {
        return Update::none();
    }
    if let Some(name) = &ctx.state.current().session_name {
        rows.retain(|row| row.name != *name);
        rows.push(crate::session::discovery::DiscoveredSession {
            name: name.clone(),
            ephemeral: ctx.state.is_ephemeral_session(),
            host: None,
            remote_target: None,
            status: crate::session::discovery::DiscoveredSessionStatus::Running {
                panes: ctx
                    .state
                    .current()
                    .workspaces
                    .iter()
                    .map(|workspace| workspace.panes.len())
                    .sum(),
                clients: ctx.state.attached_client_count(),
                has_layout: true,
                created_from_profile: ctx.state.current().created_from_profile.clone(),
            },
        });
    }
    if let Some(picker) = ctx.state.profile_picker.as_mut() {
        picker.running = rows.into_iter().map(|row| (row.name, row.status)).collect();
    }
    Update::with_command(profile_session_watch_command(
        epoch,
        ctx.state.current().session_name.clone(),
    ))
}

pub(crate) fn cancel_profile_picker(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
    ctx.state.commands_dirty = true;
    Update::full()
}

pub(crate) fn profile_picker_query_changed(ctx: &mut Context<HyprmuxApp>, query: String) -> Update {
    if let Some(picker) = ctx.state.profile_picker.as_mut() {
        let cursor = query.len();
        picker.input.set_text(query);
        picker.input.set_cursor(cursor);
        picker.input.set_anchor(None);
        picker.selected = 0;
        picker.pending_delete = None;
        picker.pending_open = None;
        picker.pending_apply = None;
    }
    request_profile_picker_focus(ctx);
    Update::full()
}

pub(crate) fn profile_picker_selection_changed(
    ctx: &mut Context<HyprmuxApp>,
    index: usize,
) -> Update {
    if let Some(picker) = ctx.state.profile_picker.as_mut() {
        picker.selected = index;
        if picker
            .pending_delete
            .is_some_and(|pending| pending != index)
        {
            picker.pending_delete = None;
        }
        if picker.pending_open.is_some_and(|pending| pending != index) {
            picker.pending_open = None;
        }
        if picker.pending_apply.is_some_and(|pending| pending != index) {
            picker.pending_apply = None;
        }
    }
    Update::full()
}

pub(crate) fn apply_selected_profile_in_place(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(entry) = selected_profile_entry(ctx) else {
        return Update::none();
    };
    if !ctx.state.current().session_attached {
        ctx.toast().push(error_toast(
            &ctx.state.theme,
            "Replace failed",
            "Attach to a session first",
        ));
        return Update::full();
    }
    if ctx
        .state
        .current()
        .shared
        .as_ref()
        .is_some_and(|shared| shared.read_only)
    {
        ctx.toast().push(error_toast(
            &ctx.state.theme,
            "Replace failed",
            "Client is read-only",
        ));
        return Update::full();
    }
    if crate::ops::session::nudge_if_follower(ctx) {
        return Update::full();
    }
    let index = ctx
        .state
        .profile_picker
        .as_ref()
        .map_or(0, |picker| picker.selected);
    let armed = ctx
        .state
        .profile_picker
        .as_ref()
        .is_some_and(|picker| picker.pending_apply == Some(index));
    if !armed {
        if let Some(picker) = ctx.state.profile_picker.as_mut() {
            picker.pending_apply = Some(index);
        }
        return Update::full();
    }
    let profile = match load_profile(&entry.path) {
        Ok(profile) => profile,
        Err(message) => {
            ctx.toast()
                .push(error_toast(&ctx.state.theme, "Replace failed", message));
            return Update::full();
        }
    };
    crate::popup::kill_if_open(ctx);
    crate::ops::exit::clear_pending(ctx);
    ctx.state.copy_mode = None;
    ctx.state.hint_mode = None;
    ctx.state.copy_flash = None;
    ctx.state.search = None;
    ctx.state.mode = Mode::Normal;
    let Some(client) = ctx.state.current().session_client.clone() else {
        return Update::full();
    };
    if let Some(scratch) = ctx.state.scratch.take() {
        client.kill(scratch.id, scratch.pty_generation);
    }
    ctx.state.scratch_visible = false;
    ctx.state.current_mut().pending_spawns.clear();
    // Replay inputs queued for panes of the layout being replaced must never reach their
    // (killed) panes' successors; `spawn_state_panes_on_session` re-queues the new layout's own.
    ctx.state.current_mut().pending_replay_inputs.clear();
    for pane in ctx
        .state
        .current()
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.panes.iter())
        .filter(|pane| !pane.closing)
    {
        client.kill(pane.id, pane.pty_generation);
    }
    let first_pane_id = ctx.state.current().next_pane_id;
    crate::profiles::replace_layout_from_profile(&mut ctx.state, profile, first_pane_id);
    let spawned = crate::update::spawn_state_panes_on_session(ctx);
    crate::update::flush_layout_commit(ctx);
    let session = ctx.state.current().session_name.clone().unwrap_or_default();
    crate::events::emit(
        &ctx.state,
        crate::events::Event::new(
            crate::events::EventKind::ProfileApplied,
            vec![
                ("profile", entry.name.clone()),
                ("path", entry.path.display().to_string()),
                ("session", session.clone()),
            ],
        ),
    );
    ctx.toast().push(info_toast(
        &ctx.state.theme,
        format!("Replaced session `{session}` with `{}`", entry.name),
    ));
    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
    ctx.state.commands_dirty = true;
    if spawned.is_empty() {
        Update::full()
    } else {
        Update::with_command(crate::pane_lifecycle::open_timers_batch_command(
            ctx.state.runtime_epoch,
            spawned,
            crate::anim::open_delay(ctx.state.config.animations),
            crate::anim::activation_delay(ctx.state.config.animations),
        ))
    }
}

pub(crate) fn profile_picker_set_default(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(entry) = selected_profile_entry(ctx) else {
        return Update::none();
    };

    let unset = ctx.state.config.profile.default.as_deref() == Some(entry.name.as_str());
    let result = if unset {
        clear_default_profile(&entry.name).map(|_| ())
    } else {
        persist_default_profile(&entry.name).map(|_| ())
    };
    match result {
        Ok(_) => {
            ctx.state.config.profile.default = (!unset).then(|| entry.name.clone());
            if let Some(picker) = ctx.state.profile_picker.as_mut() {
                picker.pending_delete = None;
            }
        }
        Err(message) => {
            ctx.toast()
                .push(error_toast(&ctx.state.theme, "Default not set", message));
        }
    }
    Update::full()
}

pub(crate) fn profile_picker_delete_key(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(entry) = selected_profile_entry(ctx) else {
        return Update::none();
    };
    let index = ctx
        .state
        .profile_picker
        .as_ref()
        .map(|picker| picker.selected);

    let Some(index) = index else {
        return Update::none();
    };

    let confirm = ctx
        .state
        .profile_picker
        .as_ref()
        .is_some_and(|picker| picker.pending_delete == Some(index));

    if !confirm {
        if let Some(picker) = ctx.state.profile_picker.as_mut() {
            picker.pending_delete = Some(index);
        }
        return Update::full();
    }

    let name = entry.name.clone();
    let path = entry.path.clone();
    match delete_profile_file(&path) {
        Ok(()) => {
            if ctx.state.config.profile.default.as_deref() == Some(name.as_str()) {
                match clear_default_profile(&name) {
                    Ok(Some(_)) => {
                        ctx.state.config.profile.default = None;
                        ctx.toast()
                            .push(info_toast(&ctx.state.theme, "Cleared startup default"));
                    }
                    Ok(None) => {}
                    Err(message) => {
                        ctx.toast().push(error_toast(
                            &ctx.state.theme,
                            "Default not cleared",
                            message,
                        ));
                    }
                }
            }
            refresh_profile_picker_entries(ctx);
            ctx.toast().push(info_toast(
                &ctx.state.theme,
                format!("Deleted profile `{name}`"),
            ));
        }
        Err(message) => {
            ctx.toast()
                .push(error_toast(&ctx.state.theme, "Delete failed", message));
            if let Some(picker) = ctx.state.profile_picker.as_mut() {
                picker.pending_delete = None;
            }
        }
    }
    Update::full()
}

pub(crate) fn select_profile(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    let Some(entry) = ctx
        .state
        .profile_picker
        .as_ref()
        .and_then(|picker| picker.entries.get(index).cloned())
    else {
        return Update::none();
    };

    let needs_confirm = ctx.state.config.confirm.load_profile
        && crate::ops::session::may_shutdown_ephemeral(&ctx.state)
        && crate::ops::exit::any_pane_live(&ctx.state);
    if needs_confirm {
        let armed = ctx
            .state
            .profile_picker
            .as_ref()
            .is_some_and(|picker| picker.pending_open == Some(index));
        if !armed {
            if let Some(picker) = ctx.state.profile_picker.as_mut() {
                picker.pending_open = Some(index);
            }
            return Update::full();
        }
    }

    if crate::session::discovery::valid_session_name(&entry.name) {
        return open_named_target(
            ctx,
            entry.name.clone(),
            OpenNamedIntent::ResolveProfile {
                profile: entry.name,
                path: entry.path,
            },
        );
    }

    load_profile_into_fresh_ephemeral(ctx, entry)
}

#[derive(Clone, Debug)]
pub(crate) enum OpenNamedIntent {
    ResolveProfile {
        profile: String,
        path: std::path::PathBuf,
    },
    CreateFresh,
    CreateFromProfile {
        profile: String,
        path: std::path::PathBuf,
    },
}

pub(crate) fn open_named_target(
    ctx: &mut Context<HyprmuxApp>,
    name: String,
    intent: OpenNamedIntent,
) -> Update {
    if !crate::session::discovery::valid_session_name(&name) {
        ctx.toast().push(error_toast(
            &ctx.state.theme,
            "Invalid name",
            "Use letters, numbers, _ or -",
        ));
        return Update::full();
    }
    let exists = crate::session::discovery::discover_session(&name)
        .ok()
        .flatten()
        .is_some();
    let explicit_create = matches!(
        intent,
        OpenNamedIntent::CreateFresh | OpenNamedIntent::CreateFromProfile { .. }
    );
    if explicit_create && exists {
        ctx.toast().push(error_toast(
            &ctx.state.theme,
            "Create failed",
            format!("Session `{name}` is already running"),
        ));
        return Update::full();
    }
    if !explicit_create && exists {
        return crate::ops::session::attach_session_by_name(ctx, name, None, None, false);
    }
    if ctx.state.current().session_attached
        && ctx.state.current().session_name.as_deref() == Some(name.as_str())
    {
        ctx.toast().push(info_toast(
            &ctx.state.theme,
            format!("Already attached to `{name}`"),
        ));
        return Update::full();
    }
    if ctx.state.current().pending_session_attach.is_some() {
        ctx.toast()
            .push(info_toast(&ctx.state.theme, "Attach already in progress"));
        return Update::full();
    }

    let seed = match intent {
        OpenNamedIntent::ResolveProfile { profile, path } => {
            if !path.exists() {
                ctx.toast().push(error_toast(
                    &ctx.state.theme,
                    "Not found",
                    format!("No session or profile `{name}`"),
                ));
                return Update::full();
            }
            Some((profile, path))
        }
        OpenNamedIntent::CreateFresh => None,
        OpenNamedIntent::CreateFromProfile { profile, path } => Some((profile, path)),
    };
    let (attachment, attach_intent) = if let Some((profile_name, path)) = seed {
        let profile = match load_profile(&path) {
            Ok(profile) => profile,
            Err(message) => {
                ctx.toast().push(error_toast(
                    &ctx.state.theme,
                    "Profile load failed",
                    message,
                ));
                return Update::full();
            }
        };
        // An empty profile still yields a working session: fall back to the default attachment.
        let attachment = crate::profiles::attachment_from_profile(&ctx.state.config, profile)
            .unwrap_or_else(|| crate::state::fresh_default_attachment(&ctx.state.config));
        (
            attachment,
            crate::state::AttachIntent::ProfileSeed {
                profile: profile_name,
                path,
            },
        )
    } else {
        (
            crate::state::fresh_default_attachment(&ctx.state.config),
            crate::state::AttachIntent::Plain,
        )
    };
    let epoch = ctx.state.mint_attachment_id();
    let (parked_epoch, left) =
        if explicit_create {
            // Creating a session parks the current one, like switching to a session or creating one on
            // a remote host: it stays live in the background, instant to return to.
            crate::ops::session::park_current_and_install(ctx, attachment, epoch)
        } else {
            // Resolving a profile *replaces* the current session — a deliberate, confirmed action (see
            // `[confirm].load_profile`) — so release it rather than park.
            let left = ctx.state.current().session_name.clone().map(|left_name| {
                crate::state::LeftSession {
                    name: left_name,
                    was_ephemeral_shutdown: crate::ops::session::may_shutdown_ephemeral(&ctx.state),
                }
            });
            crate::ops::session::release_current_session(ctx);
            crate::ops::session::install_fresh_attachment(ctx, attachment);
            (None, left)
        };
    ctx.state.current_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
        epoch,
        name: name.clone(),
        client: None,
        autostart: true,
        read_only: false,
        reconnect: false,
        remote_host: None,
        intent: attach_intent.clone(),
        left,
        parked_epoch,
    });
    ctx.state.current_mut().connection = crate::state::ConnectionState::Connecting;
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || {
            if explicit_create {
                crate::session::bootstrap::create_session_client(epoch, name, false, link)
            } else {
                crate::session::bootstrap::attach_session_client(epoch, name, true, false, link)
            }
        });
    }))
}

pub(crate) fn open_selected_profile_as(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(entry) = selected_profile_entry(ctx) else {
        return Update::none();
    };
    ctx.state.rename_session = Some(crate::state::SessionRenameState::new_open_profile_as(
        entry.name, entry.path,
    ));
    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
    ctx.state.mode = Mode::Normal;
    crate::ops::focus::request_rename_session_focus(ctx);
    Update::full()
}

pub(crate) fn load_profile_into_fresh_ephemeral(
    ctx: &mut Context<HyprmuxApp>,
    entry: crate::config::ProfileEntry,
) -> Update {
    let profile = match load_profile(&entry.path) {
        Ok(profile) => profile,
        Err(message) => {
            ctx.toast().push(error_toast(
                &ctx.state.theme,
                "Profile load failed",
                message,
            ));
            ctx.state.show_profile_picker = false;
            ctx.state.profile_picker = None;
            ctx.state.commands_dirty = true;
            return Update::full();
        }
    };

    // Loading a profile *replaces* the current session — a deliberate, confirmed action (see
    // `[confirm].load_profile`), distinct from creating a session alongside it. So release the
    // current session (an ephemeral one is disposable and shut down; a named one is detached and
    // left running) and start the profile in a fresh ephemeral.
    crate::ops::session::release_current_session(ctx);

    let epoch = ctx.state.mint_attachment_id();
    let name = crate::state::fresh_ephemeral_session_name(epoch);

    // An empty profile still yields a working session: fall back to the default attachment.
    let attachment = crate::profiles::attachment_from_profile(&ctx.state.config, profile)
        .unwrap_or_else(|| crate::state::fresh_default_attachment(&ctx.state.config));
    crate::ops::session::install_fresh_attachment(ctx, attachment);
    ctx.state.current_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
        epoch,
        name: name.clone(),
        client: None,
        autostart: true,
        read_only: false,
        reconnect: false,
        remote_host: None,
        intent: crate::state::AttachIntent::ProfileSeed {
            profile: entry.name.clone(),
            path: entry.path.clone(),
        },
        left: None,
        parked_epoch: None,
    });
    ctx.state.current_mut().connection = crate::state::ConnectionState::Connecting;
    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
    // The theme-tick, workbar-tick, and workbar-command loops started at app launch are
    // self-sustaining and survive the state swap, so don't restart them here.
    Update::with_command(Command::spawn(move |link| {
        std::thread::spawn(move || {
            crate::session::bootstrap::attach_session_client(epoch, name, true, false, link)
        });
    }))
}

fn selected_profile_entry(ctx: &Context<HyprmuxApp>) -> Option<crate::config::ProfileEntry> {
    ctx.state
        .profile_picker
        .as_ref()
        .and_then(|picker| picker.entries.get(picker.selected).cloned())
}

fn refresh_profile_picker_entries(ctx: &mut Context<HyprmuxApp>) {
    let Some(picker) = ctx.state.profile_picker.as_mut() else {
        return;
    };
    picker.entries = list_profiles();
    picker.pending_delete = None;
    if picker.entries.is_empty() {
        picker.selected = 0;
        return;
    }
    picker.selected = picker.selected.min(picker.entries.len() - 1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Msg;
    use crate::config::ProfileEntry;
    use crate::profiles::{HyprmuxProfile, save_profile};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tui_lipan::TestBackend;

    fn entry(name: &str, path: PathBuf) -> ProfileEntry {
        ProfileEntry {
            name: name.to_string(),
            path,
        }
    }

    fn temp_profile_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "hyprmux-profile-ops-{}-{}.toml",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ))
    }

    fn on_large_stack(test: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(test)
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    #[test]
    fn profile_names_are_trimmed_and_reject_paths() {
        assert_eq!(normalize_profile_name("  dev  ").as_deref(), Some("dev"));
        assert_eq!(normalize_profile_name("   "), None);
        assert_eq!(normalize_profile_name("team/dev"), None);
        assert_eq!(normalize_profile_name("team\\dev"), None);
        assert_eq!(normalize_profile_name("team dev"), None);
        assert_eq!(normalize_profile_name("eph-123"), None);
    }

    #[test]
    fn save_prompt_prefills_named_session_but_not_ephemeral_session() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.state_mut().current_mut().session_attached = true;
            backend.state_mut().current_mut().session_name = Some("dev".to_string());
            backend
                .dispatch(Msg::RunAction(crate::input::Action::SaveProfile))
                .expect("open save prompt");
            assert_eq!(
                backend
                    .state()
                    .save_profile_prompt
                    .as_ref()
                    .unwrap()
                    .input
                    .text(),
                "dev"
            );

            backend.state_mut().save_profile_prompt = None;
            backend.state_mut().current_mut().created_from_profile = Some("rust-dev".to_string());
            backend
                .dispatch(Msg::RunAction(crate::input::Action::SaveProfile))
                .expect("reopen save prompt with origin");
            assert_eq!(
                backend
                    .state()
                    .save_profile_prompt
                    .as_ref()
                    .unwrap()
                    .input
                    .text(),
                "rust-dev"
            );

            backend.state_mut().save_profile_prompt = None;
            backend.state_mut().current_mut().created_from_profile = None;
            backend.state_mut().current_mut().session_name = Some("eph-123".to_string());
            backend
                .dispatch(Msg::RunAction(crate::input::Action::SaveProfile))
                .expect("reopen save prompt");
            assert_eq!(
                backend
                    .state()
                    .save_profile_prompt
                    .as_ref()
                    .unwrap()
                    .input
                    .text(),
                ""
            );
        });
    }

    #[test]
    fn picker_query_and_selection_dispatch_reset_transient_state() {
        on_large_stack(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.state_mut().profile_picker = Some(ProfilePickerState::new(vec![
                entry("one", PathBuf::from("one.toml")),
                entry("two", PathBuf::from("two.toml")),
            ]));
            {
                let picker = backend.state_mut().profile_picker.as_mut().unwrap();
                picker.selected = 1;
                picker.pending_delete = Some(1);
            }

            backend
                .dispatch(Msg::ProfilePickerQueryChanged("tw".to_string()))
                .expect("dispatch query");
            let picker = backend.state().profile_picker.as_ref().unwrap();
            assert_eq!(picker.input.text(), "tw");
            assert_eq!(picker.selected, 0);
            assert_eq!(picker.pending_delete, None);

            backend
                .dispatch(Msg::ProfilePickerSelect(1))
                .expect("dispatch selection");
            assert_eq!(backend.state().profile_picker.as_ref().unwrap().selected, 1);
        });
    }

    #[test]
    fn selecting_profile_dispatches_named_profile_seed_attach() {
        on_large_stack(|| {
            let path = temp_profile_path();
            save_profile(&path, &HyprmuxProfile::default()).expect("write profile");

            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.state_mut().profile_picker =
                Some(ProfilePickerState::new(vec![entry("empty", path.clone())]));
            backend.state_mut().show_profile_picker = true;
            backend.state_mut().current_mut().pending_session_attach = None;
            let old_epoch = backend.state().runtime_epoch;

            backend
                .dispatch(Msg::SelectProfile(0))
                .expect("dispatch profile restore");

            let state = backend.state();
            assert!(!state.show_profile_picker);
            assert!(state.profile_picker.is_none());
            let pending = state
                .current()
                .pending_session_attach
                .as_ref()
                .expect("fresh session attach queued");
            assert_eq!(pending.epoch, old_epoch.saturating_add(1));
            assert!(pending.autostart);
            assert_eq!(pending.name, "empty");
            assert_eq!(
                pending.intent,
                crate::state::AttachIntent::ProfileSeed {
                    profile: "empty".to_string(),
                    path: path.clone(),
                }
            );

            std::fs::remove_file(path).expect("remove profile");
        });
    }

    #[test]
    fn open_as_queues_entered_session_with_selected_profile_seed() {
        on_large_stack(|| {
            let path = temp_profile_path();
            save_profile(&path, &HyprmuxProfile::default()).expect("write profile");
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.state_mut().profile_picker = Some(ProfilePickerState::new(vec![entry(
                "rust-dev",
                path.clone(),
            )]));
            backend.state_mut().show_profile_picker = true;
            backend.state_mut().current_mut().pending_session_attach = None;

            backend
                .dispatch(Msg::ProfilePickerOpenAs)
                .expect("open session-name prompt");
            let rename = backend.state_mut().rename_session.as_mut().unwrap();
            rename.input.set_text("work-copy");
            backend
                .dispatch(Msg::SubmitRenameSession)
                .expect("queue seeded attach");

            let pending = backend
                .state()
                .current()
                .pending_session_attach
                .as_ref()
                .unwrap();
            assert_eq!(pending.name, "work-copy");
            assert_eq!(
                pending.intent,
                crate::state::AttachIntent::ProfileSeed {
                    profile: "rust-dev".to_string(),
                    path: path.clone(),
                }
            );
            std::fs::remove_file(path).expect("remove profile");
        });
    }

    #[test]
    fn open_as_without_name_queues_ephemeral_profile_seed() {
        on_large_stack(|| {
            let path = temp_profile_path();
            save_profile(&path, &HyprmuxProfile::default()).expect("write profile");
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.state_mut().profile_picker = Some(ProfilePickerState::new(vec![entry(
                "rust-dev",
                path.clone(),
            )]));
            backend.state_mut().show_profile_picker = true;
            backend.state_mut().current_mut().pending_session_attach = None;

            backend
                .dispatch(Msg::ProfilePickerOpenAs)
                .expect("open session-name prompt");
            backend
                .dispatch(Msg::SubmitRenameSession)
                .expect("queue ephemeral profile attach");

            let pending = backend
                .state()
                .current()
                .pending_session_attach
                .as_ref()
                .unwrap();
            assert!(pending.name.starts_with("eph-"));
            assert_eq!(
                pending.intent,
                crate::state::AttachIntent::ProfileSeed {
                    profile: "rust-dev".to_string(),
                    path: path.clone(),
                }
            );
            std::fs::remove_file(path).expect("remove profile");
        });
    }

    #[test]
    fn replacing_session_emits_profile_applied_not_profile_loaded() {
        on_large_stack(|| {
            let path = temp_profile_path();
            save_profile(&path, &HyprmuxProfile::default()).expect("write profile");
            let mut backend = TestBackend::new(HyprmuxApp::default());
            let (client, _rx) = crate::session::client::SessionClient::test_channel();
            {
                let state = backend.state_mut();
                state.current_mut().session_attached = true;
                state.current_mut().session_name = Some("work".to_string());
                state.current_mut().session_client = Some(client);
                let mut shared = crate::state::SharedSessionState::new(1);
                shared.controller = Some(1);
                state.current_mut().shared = Some(shared);
                state.profile_picker = Some(ProfilePickerState::new(vec![entry(
                    "rust-dev",
                    path.clone(),
                )]));
                state.show_profile_picker = true;
            }
            let events = backend.state().event_hub.subscribe(None);

            backend
                .dispatch(Msg::ProfilePickerApply)
                .expect("arm replacement");
            backend
                .dispatch(Msg::ProfilePickerApply)
                .expect("replace session");

            let event: serde_json::Value =
                serde_json::from_str(&events.try_recv().expect("profile-applied event")).unwrap();
            assert_eq!(event["event"], "profile-applied");
            assert_eq!(event["data"]["profile"], "rust-dev");
            assert_eq!(event["data"]["session"], "work");
            assert!(events.try_recv().is_err());
            std::fs::remove_file(path).expect("remove profile");
        });
    }
}
