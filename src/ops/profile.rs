use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::config::{
    clear_default_profile, delete_profile_file, list_profiles, persist_default_profile,
    profile_path_for_name,
};
use crate::ops::focus::request_profile_picker_focus;
use crate::ops::focus::request_save_profile_focus;
use crate::profiles::{load_profile, profile_from_state, save_profile};
use crate::state::{Mode, ProfilePickerState, SaveProfileState};

pub(crate) fn open_save_profile_prompt(ctx: &mut Context<AppRoot>) -> Update {
    let initial = ctx
        .state
        .current()
        .origin
        .profile
        .as_deref()
        .or_else(|| {
            ctx.state.current().session_name.as_deref().filter(|name| {
                ctx.state.current().session_attached
                    && !crate::state::is_ephemeral_session_name(name)
            })
        })
        .unwrap_or("");
    ctx.state.save_profile_prompt = Some(SaveProfileState::new(initial));
    // Raised by the picker's `new`, cancelling or saving returns to the picker (the list just
    // changed); raised standalone it leads back to the pane.
    ctx.state.overlay_return = crate::ops::overlay_return::picker_origin(&ctx.state);
    ctx.state.show_palette = false;
    ctx.state.keybindings = None;
    ctx.state.search = None;
    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
    ctx.state.mode = Mode::Normal;
    request_save_profile_focus(ctx);
    Update::full()
}

pub(crate) fn close_save_profile_prompt(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.save_profile_prompt = None;
    ctx.state.commands_dirty = true;
    crate::ops::overlay_return::finish(ctx)
}

/// Replace tears down every pane in the attached session. The launcher has nothing to replace, so
/// the picker omits the action rather than advertising a toast.
pub(crate) fn can_replace_session(state: &crate::state::State) -> bool {
    state.current().session_attached
}

/// Capture the current session's layout as a profile. The live session is left as it is: an
/// ephemeral session stays ephemeral, and a named session keeps the name the user chose.
pub(crate) fn submit_save_profile(ctx: &mut Context<AppRoot>) -> Update {
    let Some(name) = ctx
        .state
        .save_profile_prompt
        .as_ref()
        .and_then(|prompt| normalize_profile_name(prompt.input.text()))
    else {
        crate::pane::pty_events::notify_error(
            ctx,
            "Invalid profile name",
            "Use letters, numbers, _ or -",
        );
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
    // A newly captured profile appears in the picker as soon as the prompt closes. Captures from
    // elsewhere and overwrites have no visible result, so they still need the toast.
    let new_row_will_be_visible = !existed
        && matches!(
            ctx.state.overlay_return,
            Some(crate::state::OverlayOrigin::ProfilePicker { .. })
        );
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
            if !new_row_will_be_visible {
                crate::pane::pty_events::notify_info(
                    ctx,
                    format!(
                        "{} profile `{name}`",
                        if existed { "Overwrote" } else { "Captured" },
                    ),
                );
            }
        }
        Err(message) => {
            crate::pane::pty_events::notify_error(ctx, "Capture failed", message);
        }
    }

    ctx.state.save_profile_prompt = None;
    ctx.state.commands_dirty = true;
    // Saved from the picker: return to it, rebuilt so the new profile is in the list.
    crate::ops::overlay_return::finish(ctx)
}

fn normalize_profile_name(name: &str) -> Option<String> {
    let name = name.trim();
    crate::session::discovery::valid_session_name(name).then(|| name.to_string())
}

pub(crate) fn open_profile_picker(ctx: &mut Context<AppRoot>) -> Update {
    open_profile_picker_mode(ctx, false)
}

pub(crate) fn open_apply_profile_picker(ctx: &mut Context<AppRoot>) -> Update {
    if !can_replace_session(&ctx.state) {
        return Update::none();
    }
    open_profile_picker_mode(ctx, true)
}

fn open_profile_picker_mode(ctx: &mut Context<AppRoot>, apply_mode: bool) -> Update {
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
    ctx.state.keybindings = None;
    ctx.state.search = None;
    ctx.state.rename = None;
    ctx.state.save_profile_prompt = None;
    ctx.state.mode = Mode::Normal;
    request_profile_picker_focus(ctx);
    Update::with_command(profile_session_watch_command(
        epoch,
        ctx.state.local_current_session_name().map(str::to_string),
    ))
}

/// The local sessions a profile's row can stand for: live ones and saved snapshots, the same rows
/// the Sessions picker lists, plus the session on screen when it is local.
fn profile_session_rows(
    ctx: &Context<AppRoot>,
) -> Vec<crate::session::discovery::DiscoveredSession> {
    let current = ctx.state.local_current_session_name();
    let mut rows =
        crate::session::discovery::discover_selectable_sessions(current).unwrap_or_default();
    push_local_current_session(ctx, &mut rows);
    rows
}

/// Discovery skips the session on screen, so it is added from what this client knows. Profiles
/// are local: a remote session on screen is not the session a same-name profile launches.
fn push_local_current_session(
    ctx: &Context<AppRoot>,
    rows: &mut Vec<crate::session::discovery::DiscoveredSession>,
) {
    let Some(name) = ctx.state.local_current_session_name() else {
        return;
    };
    let name = name.to_string();
    rows.retain(|row| row.name != name);
    rows.push(crate::session::discovery::DiscoveredSession {
        name,
        origin: ctx.state.current().origin.clone(),
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
        },
    });
}

fn profile_session_watch_command(epoch: u64, current: Option<String>) -> Command {
    // Recurring watch: `after` keeps the wait off the executor, so a picker left open does not
    // hold a worker between sweeps. The discovery itself still runs on the pool.
    Command::after(
        std::time::Duration::from_millis(1500),
        move |link: CommandLink<crate::Msg>| {
            if let Ok(rows) =
                crate::session::discovery::discover_selectable_sessions(current.as_deref())
            {
                link.send(crate::Msg::ProfileSessionsDiscovered { epoch, rows });
            }
        },
    )
}

pub(crate) fn apply_profile_sessions(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    mut rows: Vec<crate::session::discovery::DiscoveredSession>,
) -> Update {
    if !ctx.state.show_profile_picker || epoch != ctx.state.profile_picker_epoch {
        return Update::none();
    }
    push_local_current_session(ctx, &mut rows);
    if let Some(picker) = ctx.state.profile_picker.as_mut() {
        picker.running = rows.into_iter().map(|row| (row.name, row.status)).collect();
    }
    Update::with_command(profile_session_watch_command(
        epoch,
        ctx.state.local_current_session_name().map(str::to_string),
    ))
}

pub(crate) fn cancel_profile_picker(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
    ctx.state.commands_dirty = true;
    Update::full()
}

pub(crate) fn profile_picker_query_changed(ctx: &mut Context<AppRoot>, query: String) -> Update {
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

pub(crate) fn profile_picker_selection_changed(ctx: &mut Context<AppRoot>, index: usize) -> Update {
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

pub(crate) fn apply_selected_profile_in_place(ctx: &mut Context<AppRoot>) -> Update {
    let Some(entry) = selected_profile_entry(ctx) else {
        return Update::none();
    };
    if !can_replace_session(&ctx.state) {
        return Update::none();
    }
    if ctx
        .state
        .current()
        .shared
        .as_ref()
        .is_some_and(|shared| shared.read_only)
    {
        crate::pane::pty_events::notify_error(ctx, "Replace failed", "Client is read-only");
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
        return crate::ops::confirm::arm(ctx);
    }
    let profile = match load_profile(&entry.path) {
        Ok(profile) => profile,
        Err(message) => {
            crate::pane::pty_events::notify_error(ctx, "Replace failed", message);
            return Update::full();
        }
    };
    crate::ops::popup::kill_if_open(ctx);
    crate::ops::exit::clear_pending(ctx);
    ctx.state.copy_mode = None;
    crate::input::copy_mode::clear_copy_feedback(ctx);
    ctx.state.hint_mode = None;
    ctx.state.search = None;
    ctx.state.mode = Mode::Normal;
    let Some(client) = ctx.state.current().session_client.clone() else {
        return Update::full();
    };
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
        client.kill(pane.id, pane.pty_generation, false);
    }
    let first_pane_id = ctx.state.current().next_pane_id;
    crate::profiles::replace_layout_from_profile(&mut ctx.state, profile, first_pane_id);
    let spawned = crate::update::spawn_state_panes_on_session(ctx);
    crate::ops::session::flush_layout_commit(ctx);
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
    // Every pane on screen was just torn down and rebuilt from the profile - the layout itself is
    // the confirmation, and there is no dialog left worth returning to.
    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
    crate::ops::overlay_return::leave(ctx);
    ctx.state.commands_dirty = true;
    if spawned.is_empty() {
        Update::full()
    } else {
        Update::with_command(crate::pane::lifecycle::open_timers_batch_command(
            ctx.state.runtime_epoch,
            spawned,
            crate::layout::anim::open_delay(ctx.state.config.animations),
            crate::layout::anim::activation_delay(ctx.state.config.animations),
        ))
    }
}

/// `[session] startup = "profile"` opens the session named after `[profile] default`, so losing that
/// default leaves it pointing at nothing. Whenever *rozi itself* removes the default (unstarring,
/// deleting the starred profile) the mode goes back to `picker` in the same keystroke, which is a
/// consequence the user can see and undo. A launch that merely *finds* no default does not write
/// anything: the profile may be missing only on this machine, or only until a checkout finishes.
///
/// Returns whether the mode was actually reset, so the caller can mention it once.
fn reset_startup_mode_without_default(ctx: &mut Context<AppRoot>) -> bool {
    if ctx.state.config.profile.default.is_some()
        || ctx.state.config.session.startup != crate::config::SessionStartup::Profile
    {
        return false;
    }
    let fallback = crate::config::SessionStartup::default();
    ctx.state.config.session.startup = fallback;
    if let Err(message) = crate::config::persist_session_string("startup", fallback.id()) {
        crate::pane::pty_events::notify_error(ctx, "Startup mode not saved", message);
        return false;
    }
    true
}

pub(crate) fn profile_picker_set_default(ctx: &mut Context<AppRoot>) -> Update {
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
            // Starring is visible in the list itself, but taking the star away can invalidate the
            // startup mode, which lives on another screen. Say so where the key was pressed.
            if unset && reset_startup_mode_without_default(ctx) {
                crate::pane::pty_events::notify_info(ctx, "Startup mode reset to Picker");
            }
        }
        Err(message) => {
            crate::pane::pty_events::notify_error(ctx, "Default not set", message);
        }
    }
    Update::full()
}

pub(crate) fn profile_picker_delete_key(ctx: &mut Context<AppRoot>) -> Update {
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
        return crate::ops::confirm::arm(ctx);
    }

    let name = entry.name.clone();
    let path = entry.path.clone();
    match delete_profile_file(&path) {
        Ok(()) => {
            if ctx.state.config.profile.default.as_deref() == Some(name.as_str()) {
                match clear_default_profile(&name) {
                    Ok(Some(_)) => {
                        ctx.state.config.profile.default = None;
                        // One toast for both consequences: the deleted row is already gone from the
                        // list, so what is worth saying is what changed off screen.
                        if reset_startup_mode_without_default(ctx) {
                            crate::pane::pty_events::notify_info(
                                ctx,
                                "Startup default cleared\nStartup mode reset to Picker",
                            );
                        } else {
                            crate::pane::pty_events::notify_info(ctx, "Startup default cleared");
                        }
                    }
                    Ok(None) => {}
                    Err(message) => {
                        crate::pane::pty_events::notify_error(ctx, "Default not cleared", message);
                    }
                }
            }
            // The refreshed picker no longer lists the profile, which says it better than a toast.
            refresh_profile_picker_entries(ctx);
        }
        Err(message) => {
            crate::pane::pty_events::notify_error(ctx, "Delete failed", message);
            if let Some(picker) = ctx.state.profile_picker.as_mut() {
                picker.pending_delete = None;
            }
        }
    }
    Update::full()
}

pub(crate) fn select_profile(ctx: &mut Context<AppRoot>, index: usize) -> Update {
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
            return crate::ops::confirm::arm(ctx);
        }
    }

    if crate::session::discovery::valid_session_name(&entry.name) {
        return crate::ops::session::open::open_named_target(
            ctx,
            entry.name.clone(),
            crate::ops::session::open::OpenNamedIntent::ResolveProfile {
                profile: entry.name,
                path: entry.path,
            },
            None,
        );
    }

    load_profile_into_fresh_ephemeral(ctx, entry)
}

pub(crate) fn open_selected_profile_as(ctx: &mut Context<AppRoot>) -> Update {
    let Some(entry) = selected_profile_entry(ctx) else {
        return Update::none();
    };
    ctx.state.rename_session = Some(crate::state::SessionRenameState::new_open_profile_as(
        entry.name, entry.path,
    ));
    // Cancelling the name prompt goes back to the picker; submitting attaches, which retires it.
    ctx.state.overlay_return = crate::ops::overlay_return::picker_origin(&ctx.state);
    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
    ctx.state.mode = Mode::Normal;
    crate::ops::focus::request_rename_session_focus(ctx);
    Update::full()
}

pub(crate) fn load_profile_into_fresh_ephemeral(
    ctx: &mut Context<AppRoot>,
    entry: crate::config::ProfileEntry,
) -> Update {
    let profile = match load_profile(&entry.path) {
        Ok(profile) => profile,
        Err(message) => {
            crate::pane::pty_events::notify_error(ctx, "Profile load failed", message);
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
    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
    crate::ops::overlay_return::leave(ctx);
    let request = crate::state::AttachRequest::new(name, crate::state::AttachMode::JoinOrStart)
        .with_intent(crate::state::AttachIntent::ProfileSeed {
            profile: entry.name,
            path: entry.path,
        });
    // The theme-tick, workbar-tick, and workbar-command loops started at app launch are
    // self-sustaining and survive the state swap, so don't restart them here.
    crate::ops::session::start_attach(ctx, crate::state::PendingSessionAttach::new(epoch, request))
}

fn selected_profile_entry(ctx: &Context<AppRoot>) -> Option<crate::config::ProfileEntry> {
    ctx.state
        .profile_picker
        .as_ref()
        .and_then(|picker| picker.entries.get(picker.selected).cloned())
}

fn refresh_profile_picker_entries(ctx: &mut Context<AppRoot>) {
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
    use crate::profiles::{Profile, save_profile};
    use std::path::PathBuf;
    use tui_lipan::TestBackend;

    fn entry(name: &str, path: PathBuf) -> ProfileEntry {
        ProfileEntry {
            name: name.to_string(),
            path,
        }
    }

    /// A profile path no other test shares. The time tells runs apart; tests in one run are in
    /// parallel and the clock can read the same for two of them (macOS gives microseconds), so a
    /// counter tells those apart.
    fn temp_profile_path() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "rozi-profile-ops-{}-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
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

    /// Unstarring the default profile strands `startup = "profile"`, so rozi undoes that mode in the
    /// same keystroke rather than leaving a launch to discover it. Starring, and unstarring under any
    /// other mode, must leave the startup mode alone.
    #[test]
    fn unstarring_the_default_profile_resets_a_profile_startup_mode() {
        crate::test_support::on_large_stack(|| {
            let path = temp_profile_path();
            save_profile(&path, &Profile::default()).expect("write profile");
            let open_on_default = |backend: &mut TestBackend<AppRoot>, startup| {
                let mut picker = ProfilePickerState::new(vec![entry("dev", path.clone())]);
                picker.selected = 0;
                let state = backend.state_mut();
                state.profile_picker = Some(picker);
                state.show_profile_picker = true;
                state.config.profile.default = Some("dev".to_string());
                state.config.session.startup = startup;
            };

            let mut backend = TestBackend::new(AppRoot::default());
            open_on_default(&mut backend, crate::config::SessionStartup::Profile);
            backend
                .dispatch(Msg::ProfilePickerSetDefault)
                .expect("unstar the default profile");
            assert!(backend.state().config.profile.default.is_none());
            assert_eq!(
                backend.state().config.session.startup,
                crate::config::SessionStartup::Picker,
                "unstarring must not leave `profile` mode pointing at nothing"
            );

            // Any other startup mode is none of unstarring's business.
            open_on_default(&mut backend, crate::config::SessionStartup::Last);
            backend
                .dispatch(Msg::ProfilePickerSetDefault)
                .expect("unstar the default profile");
            assert!(backend.state().config.profile.default.is_none());
            assert_eq!(
                backend.state().config.session.startup,
                crate::config::SessionStartup::Last
            );

            // Starring one back leaves the mode alone too; only removal invalidates anything.
            backend
                .dispatch(Msg::ProfilePickerSetDefault)
                .expect("star it again");
            assert_eq!(
                backend.state().config.profile.default.as_deref(),
                Some("dev")
            );
            assert_eq!(
                backend.state().config.session.startup,
                crate::config::SessionStartup::Last
            );

            let _ = std::fs::remove_file(&path);
        });
    }

    /// Capture writes a recipe and leaves the live session as it is. An ephemeral session stays
    /// ephemeral; a named session keeps the name the user chose.
    #[test]
    fn capturing_an_ephemeral_session_does_not_rename_it() {
        crate::test_support::on_large_stack(|| {
            let (client, rx) = crate::session::client::SessionClient::test_channel();
            let mut backend = TestBackend::new(AppRoot::default());
            {
                let state = backend.state_mut();
                state.current_mut().session_attached = true;
                state.current_mut().session_name = Some("eph-123".to_string());
                state.current_mut().session_client = Some(client);
            }
            backend
                .dispatch(Msg::RunAction(crate::input::Action::SaveProfile))
                .expect("open save prompt");
            let name = format!("capture-eph-{}", std::process::id());
            backend
                .state_mut()
                .save_profile_prompt
                .as_mut()
                .unwrap()
                .input
                .set_text(&name);
            backend
                .dispatch(Msg::SubmitSaveProfile)
                .expect("capture profile");

            assert_eq!(
                backend.state().current().session_name.as_deref(),
                Some("eph-123")
            );
            // Only a `Rename` would be wrong here. Unrelated outbound traffic is not: the pane
            // widget reports its viewport during setup, and that debounced resize flush lands
            // whenever its timer happens to fire, so asserting the channel is *empty* would make
            // this test a race against a timer thread.
            let renamed = rx.try_iter().any(|message| {
                matches!(
                    message,
                    crate::session::client::ClientOutbound::Control(
                        crate::session::protocol::ClientMessage::Rename { .. }
                    )
                )
            });
            assert!(!renamed, "capture must not send Rename to the live session");
            let path = crate::config::profile_path_for_name(&name);
            assert!(path.exists(), "profile file should be written");
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn capture_prompt_is_a_plain_capture_from_ephemeral_and_named_sessions() {
        crate::test_support::on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().config.animations.picker =
                crate::layout::anim::PickerAnimationStyle::Off;
            backend.state_mut().current_mut().session_attached = true;
            backend.state_mut().current_mut().session_name = Some("eph-123".to_string());
            backend
                .dispatch(Msg::RunAction(crate::input::Action::SaveProfile))
                .expect("open save prompt");
            backend.render();
            let hints = backend.capture_frame().to_fixed_grid_lines().join("\n");
            assert!(
                hints.contains("save Enter") && !hints.contains("name session"),
                "capturing a temporary session must not also name it\n{hints}"
            );

            backend.state_mut().current_mut().session_name = Some("dev".to_string());
            backend.render();
            let hints = backend.capture_frame().to_fixed_grid_lines().join("\n");
            assert!(
                hints.contains("save Enter") && !hints.contains("name session"),
                "a named session keeps its name, so the commit is a plain capture\n{hints}"
            );
        });
    }

    /// Replace arms like every other destructive gesture, so it has to disarm on the same clock.
    /// It previously returned a bare update, leaving a confirmation armed indefinitely.
    #[test]
    fn replace_arms_on_the_shared_confirm_clock_and_expires_with_it() {
        crate::test_support::on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().current_mut().session_attached = true;
            let path = temp_profile_path();
            save_profile(&path, &Profile::default()).expect("write profile");
            let mut picker = ProfilePickerState::new(vec![entry("dev", path.clone())]);
            picker.apply_mode = true;
            backend.state_mut().profile_picker = Some(picker);
            backend.state_mut().show_profile_picker = true;

            let before = backend.state().confirm_epoch;
            backend
                .dispatch(Msg::ProfilePickerApply)
                .expect("arm replace");
            assert_eq!(
                backend
                    .state()
                    .profile_picker
                    .as_ref()
                    .unwrap()
                    .pending_apply,
                Some(0)
            );
            let armed = backend.state().confirm_epoch;
            assert_ne!(
                armed, before,
                "arming must advance the shared confirm token"
            );

            backend
                .dispatch(Msg::ConfirmationExpired(armed))
                .expect("expire replace");
            assert!(
                backend
                    .state()
                    .profile_picker
                    .as_ref()
                    .unwrap()
                    .pending_apply
                    .is_none(),
                "the window lapsing must disarm replace"
            );
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn save_prompt_prefills_named_session_but_not_ephemeral_session() {
        crate::test_support::on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
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
            backend.state_mut().current_mut().origin.profile = Some("rust-dev".to_string());
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
            backend.state_mut().current_mut().origin.profile = None;
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
        crate::test_support::on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
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
        crate::test_support::on_large_stack(|| {
            let path = temp_profile_path();
            save_profile(&path, &Profile::default()).expect("write profile");

            let mut backend = TestBackend::new(AppRoot::default());
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
            assert_eq!(pending.request.mode, crate::state::AttachMode::JoinOrStart);
            assert_eq!(pending.request.name, "empty");
            assert_eq!(
                pending.request.intent,
                crate::state::AttachIntent::ProfileSeed {
                    profile: "empty".to_string(),
                    path: path.clone(),
                }
            );

            std::fs::remove_file(path).expect("remove profile");
        });
    }

    /// A profile whose session this client holds opens that session, even when its server is gone
    /// and the attachment is offline: the retained attachment comes forward and reconnects, rather
    /// than a fresh profile seed starting a second attachment under the same name.
    #[test]
    fn selecting_a_profile_whose_session_is_held_offline_reconnects_it() {
        crate::test_support::on_large_stack(|| {
            let path = temp_profile_path();
            save_profile(&path, &Profile::default()).expect("write profile");

            let mut backend = TestBackend::new(AppRoot::default());
            let mut held = crate::state::Attachment::new();
            held.session_name = Some("empty".into());
            held.connection = crate::state::ConnectionState::Disconnected;
            backend.state_mut().background.insert(1, held);
            backend.state_mut().profile_picker =
                Some(ProfilePickerState::new(vec![entry("empty", path.clone())]));
            backend.state_mut().show_profile_picker = true;
            backend.state_mut().current_mut().pending_session_attach = None;

            // `update_level`, not `dispatch`: the assertion is about where selecting lands and what
            // it queues, not the reconnect that follows, which fails at once with no server here.
            backend
                .update_level(Msg::SelectProfile(0))
                .expect("dispatch profile select");

            let state = backend.state();
            assert_eq!(state.current().session_name.as_deref(), Some("empty"));
            assert!(
                !state
                    .background
                    .values()
                    .any(|attachment| attachment.session_name.as_deref() == Some("empty")),
                "the held attachment came forward instead of staying parked beside a new one"
            );
            assert!(
                !state
                    .current()
                    .pending_session_attach
                    .as_ref()
                    .is_some_and(|pending| matches!(
                        pending.request.intent,
                        crate::state::AttachIntent::ProfileSeed { .. }
                    )),
                "no fresh profile seed replaces the held session"
            );

            std::fs::remove_file(path).expect("remove profile");
        });
    }

    /// A saved session outranks its same-name profile: selecting the profile restores the session
    /// without reading the profile, so one that no longer parses cannot stand in the way.
    #[test]
    fn selecting_a_profile_with_a_saved_session_restores_it_without_reading_the_profile() {
        crate::test_support::on_large_stack(|| {
            const NAME: &str = "restore-over-broken-profile";
            let path = temp_profile_path();
            std::fs::write(&path, "this is [not toml").expect("write broken profile");
            crate::session::server::write_empty_snapshot_for_test(NAME);

            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().profile_picker =
                Some(ProfilePickerState::new(vec![entry(NAME, path.clone())]));
            backend.state_mut().show_profile_picker = true;
            backend.state_mut().current_mut().pending_session_attach = None;

            // `update_level`, not `dispatch`: the assertion is about the attach this queues.
            backend
                .update_level(Msg::SelectProfile(0))
                .expect("dispatch profile select");

            let queued = backend
                .state()
                .current()
                .pending_session_attach
                .as_ref()
                .map(|pending| {
                    (
                        pending.request.name.clone(),
                        (pending.request.mode == crate::state::AttachMode::JoinOrStart),
                        pending.request.intent.clone(),
                    )
                });
            crate::session::server::delete_snapshot(NAME).expect("remove snapshot");
            std::fs::remove_file(path).expect("remove profile");
            let (name, autostart, intent) = queued.expect("a restoring attach is queued");
            assert_eq!(name, NAME);
            assert!(
                autostart,
                "the server starts so resurrection can restore it"
            );
            assert_eq!(intent, crate::state::AttachIntent::Plain);
        });
    }

    /// Profiles are local. A remote session on screen that shares a profile's name is not that
    /// profile's session, so it must not mark the profile as running; a local one does.
    #[test]
    fn only_a_local_session_on_screen_stands_for_a_profile() {
        crate::test_support::on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().current_mut().session_name = Some("dev".into());
            backend.state_mut().current_mut().remote_target = Some(
                crate::session::remote::RemoteTarget::Alias("workbox".into()),
            );
            backend.state_mut().profile_picker = Some(ProfilePickerState::new(vec![entry(
                "dev",
                temp_profile_path(),
            )]));
            backend.state_mut().show_profile_picker = true;
            let epoch = backend.state().profile_picker_epoch;
            let discovered = |backend: &mut TestBackend<AppRoot>| {
                backend
                    .update_level(Msg::ProfileSessionsDiscovered {
                        epoch,
                        rows: Vec::new(),
                    })
                    .expect("apply discovered sessions");
                backend
                    .state()
                    .profile_picker
                    .as_ref()
                    .unwrap()
                    .running
                    .contains_key("dev")
            };
            assert!(
                !discovered(&mut backend),
                "a remote `dev` is not the local profile's"
            );

            backend.state_mut().current_mut().remote_target = None;
            assert!(discovered(&mut backend), "a local `dev` is");
        });
    }

    #[test]
    fn open_as_queues_entered_session_with_selected_profile_seed() {
        crate::test_support::on_large_stack(|| {
            let path = temp_profile_path();
            save_profile(&path, &Profile::default()).expect("write profile");
            let mut backend = TestBackend::new(AppRoot::default());
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
            // `update_level`, not `dispatch`: the assertion is about the attach this queues, and
            // `dispatch` drains until idle - long enough for the attach thread to fail against a
            // session that does not exist and clear `pending_session_attach` again.
            backend
                .update_level(Msg::SubmitRenameSession)
                .expect("queue seeded attach");

            let pending = backend
                .state()
                .current()
                .pending_session_attach
                .as_ref()
                .unwrap();
            assert_eq!(pending.request.name, "work-copy");
            assert_eq!(
                pending.request.intent,
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
        crate::test_support::on_large_stack(|| {
            let path = temp_profile_path();
            save_profile(&path, &Profile::default()).expect("write profile");
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().profile_picker = Some(ProfilePickerState::new(vec![entry(
                "rust-dev",
                path.clone(),
            )]));
            backend.state_mut().show_profile_picker = true;
            backend.state_mut().current_mut().pending_session_attach = None;

            backend
                .dispatch(Msg::ProfilePickerOpenAs)
                .expect("open session-name prompt");
            // Synchronous outcome only - see
            // `open_as_queues_entered_session_with_selected_profile_seed` above.
            backend
                .update_level(Msg::SubmitRenameSession)
                .expect("queue ephemeral profile attach");

            let pending = backend
                .state()
                .current()
                .pending_session_attach
                .as_ref()
                .unwrap();
            assert!(pending.request.name.starts_with("eph-"));
            assert_eq!(
                pending.request.intent,
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
        crate::test_support::on_large_stack(|| {
            let path = temp_profile_path();
            save_profile(&path, &Profile::default()).expect("write profile");
            let mut backend = TestBackend::new(AppRoot::default());
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

    #[test]
    fn replace_from_the_launcher_is_inert() {
        crate::test_support::on_large_stack(|| {
            let path = temp_profile_path();
            save_profile(&path, &Profile::default()).expect("write profile");
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().profile_picker =
                Some(ProfilePickerState::new(vec![entry("dev", path.clone())]));
            backend.state_mut().show_profile_picker = true;
            assert!(
                !backend.state().current().session_attached,
                "default TestBackend is the launcher"
            );

            backend
                .dispatch(Msg::ProfilePickerApply)
                .expect("replace from launcher");
            assert!(
                backend
                    .state()
                    .profile_picker
                    .as_ref()
                    .unwrap()
                    .pending_apply
                    .is_none(),
                "replace must not arm when nothing is attached"
            );
            assert!(backend.state().show_profile_picker);
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn apply_profile_command_does_not_open_from_the_launcher() {
        crate::test_support::on_large_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend
                .dispatch(Msg::RunAction(crate::input::Action::ApplyProfile))
                .expect("apply-profile from launcher");
            assert!(!backend.state().show_profile_picker);
            assert!(backend.state().profile_picker.is_none());
        });
    }

    #[test]
    fn selecting_the_attached_profile_is_inert() {
        crate::test_support::on_large_stack(|| {
            let path = temp_profile_path();
            save_profile(&path, &Profile::default()).expect("write profile");
            let mut backend = TestBackend::new(AppRoot::default());
            backend.state_mut().profile_picker =
                Some(ProfilePickerState::new(vec![entry("dev", path.clone())]));
            backend.state_mut().show_profile_picker = true;
            backend.state_mut().current_mut().session_name = Some("dev".into());
            backend.state_mut().current_mut().session_attached = true;
            backend.state_mut().current_mut().pending_session_attach = None;

            backend
                .update_level(Msg::SelectProfile(0))
                .expect("select the attached profile");

            assert!(
                backend.state().show_profile_picker,
                "Enter on the current profile must leave the picker open"
            );
            assert!(
                backend.state().replaceable_toasts.is_empty(),
                "already being there must not toast"
            );
            assert!(backend.state().current().pending_session_attach.is_none());
            let _ = std::fs::remove_file(path);
        });
    }
}
