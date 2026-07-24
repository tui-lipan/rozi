use tui_lipan::prelude::*;

use crate::ops::focus::{
    request_current_pane_focus, request_rename_focus, request_save_profile_focus,
};
use crate::ops::identity::{apply_rename_pane, close_rename_pane as close_pane_rename};
use crate::ops::profile::{
    cancel_profile_picker, close_save_profile_prompt, profile_picker_delete_key,
    profile_picker_query_changed as change_profile_query, profile_picker_selection_changed,
    profile_picker_set_default as set_default_profile, select_profile as choose_profile,
    submit_save_profile as save_profile,
};
use crate::ops::search::{recompute_search, search_next as next_search, select_search_match};
use crate::{HyprmuxApp, session};

pub(super) fn close_search(ctx: &mut Context<HyprmuxApp>) -> Update {
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

pub(super) fn search_query_changed(ctx: &mut Context<HyprmuxApp>, query: String) -> Update {
    if let Some(search) = ctx.state.search.as_mut() {
        let cursor = query.len();
        search.input.set_text(query);
        search.input.set_cursor(cursor);
        search.input.set_anchor(None);
    }
    recompute_search(ctx)
}

pub(super) fn search_next(ctx: &mut Context<HyprmuxApp>, backward: bool) -> Update {
    next_search(ctx, backward)
}

pub(super) fn search_select(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    select_search_match(ctx, index)
}

pub(super) fn search_activate(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    select_search_match(ctx, index);
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

pub(super) fn search_cycle_scope(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::ops::search::cycle_search_scope(ctx)
}

pub(super) fn close_rename_pane(ctx: &mut Context<HyprmuxApp>) -> Update {
    let update = close_pane_rename(ctx);
    request_current_pane_focus(ctx);
    update
}

pub(super) fn rename_pane_changed(ctx: &mut Context<HyprmuxApp>, event: InputEvent) -> Update {
    if let Some(rename) = ctx.state.rename.as_mut() {
        event.apply_to(&mut rename.input);
    }
    request_rename_focus(ctx);
    Update::full()
}

pub(super) fn submit_rename_pane(ctx: &mut Context<HyprmuxApp>) -> Update {
    let update = apply_rename_pane(ctx);
    request_current_pane_focus(ctx);
    update
}

pub(super) fn close_rename_session(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::ops::session::close_rename_session(ctx)
}

pub(super) fn rename_session_changed(ctx: &mut Context<HyprmuxApp>, event: InputEvent) -> Update {
    if let Some(rename) = ctx.state.rename_session.as_mut() {
        event.apply_to(&mut rename.input);
    }
    crate::ops::focus::request_rename_session_focus(ctx);
    Update::full()
}

pub(super) fn submit_rename_session(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::ops::session::apply_rename_session(ctx)
}

pub(super) fn close_save_profile(ctx: &mut Context<HyprmuxApp>) -> Update {
    let update = close_save_profile_prompt(ctx);
    request_current_pane_focus(ctx);
    update
}

pub(super) fn save_profile_name_changed(
    ctx: &mut Context<HyprmuxApp>,
    event: InputEvent,
) -> Update {
    if let Some(prompt) = ctx.state.save_profile_prompt.as_mut() {
        event.apply_to(&mut prompt.input);
        prompt.pending_overwrite = false;
    }
    request_save_profile_focus(ctx);
    Update::full()
}

pub(super) fn submit_save_profile(ctx: &mut Context<HyprmuxApp>) -> Update {
    let update = save_profile(ctx);
    request_current_pane_focus(ctx);
    update
}

pub(super) fn close_profile_picker(ctx: &mut Context<HyprmuxApp>) -> Update {
    let update = cancel_profile_picker(ctx);
    request_current_pane_focus(ctx);
    update
}

pub(super) fn profile_sessions_discovered(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    rows: Vec<crate::session::discovery::DiscoveredSession>,
) -> Update {
    crate::ops::profile::apply_profile_sessions(ctx, epoch, rows)
}

pub(super) fn profile_picker_apply(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::ops::profile::apply_selected_profile_in_place(ctx)
}

pub(super) fn profile_picker_open_as(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::ops::profile::open_selected_profile_as(ctx)
}

pub(super) fn profile_picker_new(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::ops::profile::open_save_profile_prompt(ctx)
}

pub(super) fn profile_picker_query_changed(ctx: &mut Context<HyprmuxApp>, query: String) -> Update {
    change_profile_query(ctx, query)
}

pub(super) fn profile_picker_select(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    profile_picker_selection_changed(ctx, index)
}

pub(super) fn profile_picker_set_default(ctx: &mut Context<HyprmuxApp>) -> Update {
    set_default_profile(ctx)
}

pub(super) fn profile_picker_delete(ctx: &mut Context<HyprmuxApp>) -> Update {
    profile_picker_delete_key(ctx)
}

pub(super) fn select_profile(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    let update = choose_profile(ctx, index);
    request_current_pane_focus(ctx);
    update
}

pub(super) fn close_session_picker(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::ops::session::close_session_picker(ctx)
}

pub(super) fn sessions_discovered(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    rows: Vec<session::discovery::DiscoveredSession>,
) -> Update {
    crate::ops::session::apply_discovered_sessions(ctx, epoch, rows)
}

pub(super) fn session_picker_query_changed(ctx: &mut Context<HyprmuxApp>, query: String) -> Update {
    if let Some(picker) = ctx.state.session_picker.as_mut() {
        picker.input.set_text(query);
    }
    crate::ops::session::clear_pending_session_arms(ctx);
    Update::full()
}

pub(super) fn session_picker_select(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    if let Some(picker) = ctx.state.session_picker.as_mut() {
        picker.selected = index.min(picker.entries.len().saturating_sub(1));
    }
    // Moving the highlight off an armed kill row cancels its confirmation.
    let moved_off_armed = ctx.state.session_picker.as_ref().is_some_and(|picker| {
        picker
            .pending_kill
            .is_some_and(|index| index != picker.selected)
    });
    if moved_off_armed {
        crate::ops::session::clear_pending_session_arms(ctx);
    }
    Update::full()
}

pub(super) fn session_picker_activate(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    crate::ops::session::activate_selected_session(ctx, index)
}

pub(super) fn session_picker_create_from_query(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::ops::session::open_create_session(ctx)
}

pub(super) fn session_picker_detach_current(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::ops::session::detach_current_session(ctx)
}

pub(super) fn session_picker_kill_selected(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::ops::session::kill_selected_session(ctx)
}

pub(super) fn session_picker_close_attachment(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::ops::session::close_selected_attachment(ctx)
}

pub(super) fn session_picker_disconnect_host(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::ops::session::disconnect_selected_host(ctx)
}

pub(super) fn session_picker_connect_host(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::ops::session::open_connect_remote_host(ctx)
}

pub(super) fn session_picker_name_current(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::ops::session::open_rename_session(ctx)
}

pub(super) fn close_client_list(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.client_list = None;
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
    Update::full()
}

pub(super) fn client_list_select(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    let len = ctx
        .state
        .current()
        .shared
        .as_ref()
        .map_or(0, |shared| shared.clients.len());
    if let Some(list) = ctx.state.client_list.as_mut() {
        list.selected = index.min(len.saturating_sub(1));
    }
    Update::full()
}

pub(super) fn client_list_grant(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    crate::ops::session::grant_control(ctx, index)
}

pub(super) fn client_list_decline(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    crate::ops::session::decline_control(ctx, index)
}
