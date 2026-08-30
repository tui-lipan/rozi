use tui_lipan::prelude::*;

use crate::ops::focus::{
    request_askpass_focus, request_current_pane_focus, request_rename_focus,
    request_save_profile_focus, request_search_focus,
};
use crate::ops::identity::{apply_rename_pane, close_rename_pane as close_pane_rename};
use crate::ops::profile::{
    cancel_profile_picker, close_save_profile_prompt, profile_picker_delete_key,
    profile_picker_query_changed as change_profile_query, profile_picker_selection_changed,
    profile_picker_set_default as set_default_profile, select_profile as choose_profile,
    submit_save_profile as save_profile,
};
use crate::ops::search::{recompute_search, search_next as next_search, select_search_match};
use crate::{AppRoot, session};

pub(super) fn close_search(ctx: &mut Context<AppRoot>) -> Update {
    let from_copy_mode = ctx
        .state
        .search
        .as_ref()
        .is_some_and(|search| search.from_copy_mode);
    if from_copy_mode {
        crate::ops::search::finish_copy_mode_search(ctx, false);
        request_current_pane_focus(ctx);
        return Update::full();
    }
    ctx.state.search = None;
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
    Update::full()
}

pub(super) fn search_query_changed(ctx: &mut Context<AppRoot>, query: String) -> Update {
    if let Some(search) = ctx.state.search.as_mut() {
        let cursor = query.len();
        search.input.set_text(query);
        search.input.set_cursor(cursor);
        search.input.set_anchor(None);
    }
    recompute_search(ctx)
}

pub(super) fn search_next(ctx: &mut Context<AppRoot>, backward: bool) -> Update {
    next_search(ctx, backward)
}

pub(super) fn search_select(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    select_search_match(ctx, index)
}

pub(super) fn search_activate(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    select_search_match(ctx, index);
    if ctx
        .state
        .search
        .as_ref()
        .is_some_and(|search| search.refresh_matches.is_some())
    {
        request_search_focus(ctx);
        return Update::full();
    }
    let from_copy_mode = ctx
        .state
        .search
        .as_ref()
        .is_some_and(|search| search.from_copy_mode);
    if from_copy_mode {
        crate::ops::search::finish_copy_mode_search(ctx, true);
        request_current_pane_focus(ctx);
        return Update::full();
    }
    ctx.state.search = None;
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
    Update::full()
}

pub(super) fn search_cycle_scope(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::search::cycle_search_scope(ctx)
}

pub(super) fn close_rename_pane(ctx: &mut Context<AppRoot>) -> Update {
    let update = close_pane_rename(ctx);
    request_current_pane_focus(ctx);
    update
}

pub(super) fn rename_pane_changed(ctx: &mut Context<AppRoot>, event: InputEvent) -> Update {
    if let Some(rename) = ctx.state.rename.as_mut() {
        event.apply_to(&mut rename.input);
    }
    request_rename_focus(ctx);
    Update::full()
}

pub(super) fn submit_rename_pane(ctx: &mut Context<AppRoot>) -> Update {
    let update = apply_rename_pane(ctx);
    request_current_pane_focus(ctx);
    update
}

pub(super) fn close_rename_session(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::session::close_rename_session(ctx)
}

pub(super) fn rename_session_changed(ctx: &mut Context<AppRoot>, event: InputEvent) -> Update {
    if let Some(rename) = ctx.state.rename_session.as_mut() {
        event.apply_to(&mut rename.input);
        // Editing is a change of mind: an armed "close it" must not survive typing a name and
        // clearing it again, or the next Enter would close without ever having said so.
        if let Some(leave) = rename.leave.as_mut() {
            leave.armed = false;
        }
        // The rejection described the text that was there; once it changes, the verdict is stale.
        rename.error = None;
    }
    crate::ops::focus::request_rename_session_focus(ctx);
    Update::full()
}

pub(super) fn submit_rename_session(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::session::apply_rename_session(ctx)
}

pub(super) fn close_save_profile(ctx: &mut Context<AppRoot>) -> Update {
    close_save_profile_prompt(ctx)
}

pub(super) fn save_profile_name_changed(ctx: &mut Context<AppRoot>, event: InputEvent) -> Update {
    if let Some(prompt) = ctx.state.save_profile_prompt.as_mut() {
        event.apply_to(&mut prompt.input);
        prompt.pending_overwrite = false;
    }
    request_save_profile_focus(ctx);
    Update::full()
}

pub(super) fn submit_save_profile(ctx: &mut Context<AppRoot>) -> Update {
    save_profile(ctx)
}

pub(super) fn close_profile_picker(ctx: &mut Context<AppRoot>) -> Update {
    let update = cancel_profile_picker(ctx);
    request_current_pane_focus(ctx);
    update
}

pub(super) fn profile_sessions_discovered(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    rows: Vec<crate::session::discovery::DiscoveredSession>,
) -> Update {
    crate::ops::profile::apply_profile_sessions(ctx, epoch, rows)
}

pub(super) fn profile_picker_apply(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::profile::apply_selected_profile_in_place(ctx)
}

pub(super) fn profile_picker_open_as(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::profile::open_selected_profile_as(ctx)
}

pub(super) fn profile_picker_new(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::profile::open_save_profile_prompt(ctx)
}

pub(super) fn profile_picker_query_changed(ctx: &mut Context<AppRoot>, query: String) -> Update {
    change_profile_query(ctx, query)
}

pub(super) fn profile_picker_select(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    profile_picker_selection_changed(ctx, index)
}

pub(super) fn profile_picker_set_default(ctx: &mut Context<AppRoot>) -> Update {
    set_default_profile(ctx)
}

pub(super) fn profile_picker_delete(ctx: &mut Context<AppRoot>) -> Update {
    profile_picker_delete_key(ctx)
}

pub(super) fn select_profile(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    let update = choose_profile(ctx, index);
    if !ctx.state.show_profile_picker {
        request_current_pane_focus(ctx);
    }
    update
}

pub(super) fn close_session_picker(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::session::close_session_picker(ctx)
}

pub(super) fn sessions_discovered(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    rows: Vec<session::discovery::DiscoveredSession>,
    host_status: crate::ops::session::HostProbeStatus,
) -> Update {
    crate::ops::session::apply_discovered_sessions(ctx, epoch, rows, host_status)
}

pub(super) fn session_picker_query_changed(ctx: &mut Context<AppRoot>, query: String) -> Update {
    if let Some(picker) = ctx.state.session_picker.as_mut() {
        picker.input.set_text(query);
    }
    crate::ops::session::clear_pending_session_arms(ctx);
    Update::full()
}

pub(super) fn session_picker_select(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    if let Some(picker) = ctx.state.session_picker.as_mut() {
        picker.selected = index.min(picker.entries.len().saturating_sub(1));
    }
    // Moving the highlight off an armed kill/restart row cancels its confirmation.
    let moved_off_armed = ctx.state.session_picker.as_ref().is_some_and(|picker| {
        picker
            .pending_kill
            .is_some_and(|index| index != picker.selected)
            || picker
                .pending_restart
                .is_some_and(|index| index != picker.selected)
    });
    if moved_off_armed {
        crate::ops::session::clear_pending_session_arms(ctx);
    }
    Update::full()
}

pub(super) fn session_picker_activate(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    crate::ops::session::activate_selected_session(ctx, index)
}

pub(super) fn session_picker_create_from_query(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::session::open_create_session(ctx)
}

pub(super) fn session_picker_kill_selected(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::session::kill_selected_session(ctx)
}

pub(super) fn session_picker_restart_selected(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::session::restart_selected_session(ctx)
}

pub(super) fn session_picker_disconnect_attachment(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::session::disconnect_selected_attachment(ctx)
}

pub(super) fn session_picker_disconnect_host(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::session::disconnect_selected_host(ctx)
}

pub(super) fn session_picker_remote_hosts(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::session::open_remote_hosts(ctx)
}

pub(super) fn session_picker_name_current(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::session::open_rename_session(ctx)
}

pub(super) fn close_collaboration(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.collaboration = None;
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
    Update::full()
}

pub(super) fn collaboration_query_changed(ctx: &mut Context<AppRoot>, query: String) -> Update {
    if let Some(collaboration) = ctx.state.collaboration.as_mut() {
        collaboration.query = query;
        // Re-filtering moves what is highlighted; an arming aimed at the old row must not survive.
        collaboration.pending_kick = None;
    }
    Update::full()
}

pub(super) fn collaboration_select(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    if let Some(list) = ctx.state.collaboration.as_mut() {
        // The view clamps this against its state-dependent mix of control actions and other
        // clients. Keeping the requested item here avoids duplicating that derivation in update.
        // Moving the highlight off an armed removal cancels its confirmation.
        if list.selected != index {
            list.pending_kick = None;
        }
        list.selected = index;
    }
    Update::full()
}

pub(super) fn collaboration_grant(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    crate::ops::session::grant_control(ctx, index)
}

pub(super) fn collaboration_decline(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    crate::ops::session::decline_control(ctx, index)
}

pub(super) fn collaboration_kick(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    crate::ops::session::evict_client(ctx, index)
}

/// Show an ssh prompt, or queue it behind the one already on screen.
///
/// Raised from the askpass broker's worker thread while some other overlay may be open. The picker
/// the user was driving stays dimmed underneath; only focus moves, so answering a prompt returns
/// them where they were.
///
/// A prompt from a connection the user already refused is declined for them: `ssh` re-asks
/// whatever the helper says, so showing it again would make Esc mean "ask me a fourth time".
pub(super) fn askpass_prompt(
    ctx: &mut Context<AppRoot>,
    id: u64,
    session: String,
    kind: crate::session::remote::AskpassKind,
    prompt: String,
) -> Update {
    if ctx.state.askpass_history.refuses(&session) {
        crate::session::remote::askpass::cancel(id);
        return Update::none();
    }
    let entry = crate::state::AskpassPrompt {
        error: ctx
            .state
            .askpass_history
            .is_retry_of(&session, &prompt)
            .then(|| "Rejected - try again".to_string()),
        id,
        session,
        kind,
        prompt,
    };
    match ctx.state.askpass.as_mut() {
        Some(askpass) => askpass.queued.push_back(entry),
        None => {
            ctx.state.askpass = Some(crate::state::AskpassState::new(entry));
            ctx.state.commands_dirty = true;
            request_askpass_focus(ctx);
        }
    }
    Update::full()
}

/// The helper stopped waiting. Only the prompt it names goes away; a queued one behind it is a
/// different ssh still waiting on an answer.
pub(super) fn askpass_expired(ctx: &mut Context<AppRoot>, id: u64) -> Update {
    let Some(askpass) = ctx.state.askpass.as_mut() else {
        return Update::none();
    };
    if askpass.discard(id) {
        return close_or_advance_askpass(ctx);
    }
    Update::full()
}

pub(super) fn askpass_changed(ctx: &mut Context<AppRoot>, event: InputEvent) -> Update {
    if let Some(askpass) = ctx.state.askpass.as_mut() {
        event.apply_to(&mut askpass.input);
    }
    request_askpass_focus(ctx);
    Update::full()
}

pub(super) fn submit_askpass(ctx: &mut Context<AppRoot>) -> Update {
    let Some(askpass) = ctx.state.askpass.as_ref() else {
        return Update::none();
    };
    let session = askpass.current.session.clone();
    let (id, prompt) = (askpass.current.id, askpass.current.prompt.clone());
    let answer = askpass.input.text().to_string();
    crate::session::remote::askpass::answer(id, answer);
    ctx.state.askpass_history.answered(&session, &prompt);
    close_or_advance_askpass(ctx)
}

/// Refusing the prompt fails the ssh behind it, which is the only way out of a host that keeps
/// asking for something the user cannot supply.
///
/// The refusal is the end of the whole attempt, not of one dialog: it silences the prompts `ssh`
/// will keep raising, and gives up on the host probe that raised them, so the picker stops
/// spinning on a connection the user has just called off. The ssh itself still runs to its own
/// conclusion; its answer no longer matches the picker's epoch, so it lands nowhere.
pub(super) fn cancel_askpass(ctx: &mut Context<AppRoot>) -> Update {
    let Some(askpass) = ctx.state.askpass.as_ref() else {
        return Update::none();
    };
    let id = askpass.current.id;
    let session = askpass.current.session.clone();
    crate::session::remote::askpass::cancel(id);
    ctx.state.askpass_history.refused(&session);
    // Anything queued from the same connection is that connection's retries arriving early, so it
    // goes with the refusal. A prompt from some other connection is a different question and stays.
    if let Some(askpass) = ctx.state.askpass.as_mut() {
        let queued = std::mem::take(&mut askpass.queued);
        for prompt in queued {
            if prompt.session == session {
                crate::session::remote::askpass::cancel(prompt.id);
            } else {
                askpass.queued.push_back(prompt);
            }
        }
    }
    crate::ops::session::remotes::cancel_host_probe(ctx);
    close_or_advance_askpass(ctx)
}

/// Retire the prompt on screen: show the next one waiting, or take the modal down and hand focus
/// back to whatever the prompt interrupted.
fn close_or_advance_askpass(ctx: &mut Context<AppRoot>) -> Update {
    let more = ctx
        .state
        .askpass
        .as_mut()
        .is_some_and(crate::state::AskpassState::advance);
    if more {
        request_askpass_focus(ctx);
        return Update::full();
    }
    ctx.state.askpass = None;
    ctx.state.commands_dirty = true;
    if ctx.state.remote_picker.is_some() {
        crate::ops::focus::request_remote_picker_focus(ctx);
    } else if ctx.state.show_session_picker {
        crate::ops::focus::request_session_picker_focus(ctx);
    } else {
        request_current_pane_focus(ctx);
    }
    Update::full()
}

pub(super) fn follow_prompt_select(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    if let Some(prompt) = ctx.state.follow_prompt.as_mut() {
        prompt.selected = index.min(crate::state::FollowChoice::ALL.len() - 1);
    }
    Update::full()
}

pub(super) fn follow_prompt_choose(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    let Some(choice) = crate::state::FollowChoice::ALL.get(index).copied() else {
        return Update::full();
    };
    crate::ops::session::resolve_follow_prompt(ctx, choice)
}
