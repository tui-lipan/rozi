use tui_lipan::prelude::*;

use crate::actions::execute_action;
use crate::anim::GeometryAnimation;
use crate::focus_ops::{
    focus_pane, request_current_pane_focus, request_pane_focus, request_rename_focus,
    request_save_profile_focus, request_search_focus,
};
use crate::identity_ops::{apply_rename_pane, close_rename_pane};
use crate::input::Action;
use crate::key_routing::handle_key_routing;
use crate::pane_lifecycle::{begin_close_pane, find_pane_mut, handle_prune_closed};
use crate::profile_ops::{
    cancel_profile_picker, close_save_profile_prompt, profile_picker_delete_key,
    profile_picker_query_changed, profile_picker_selection_changed, profile_picker_set_default,
    select_profile, submit_save_profile,
};
use crate::pty_events::{
    error_toast, handle_pane_input, handle_pane_mouse, handle_pane_resize, handle_pane_scroll,
    handle_pty_event, handle_pty_ready,
};
use crate::resize_move_ops::{begin_move, begin_resize, end_move, move_pane, resize_pane};
use crate::search_ops::{recompute_search, search_next, select_search_match};
use crate::state::State;
use crate::theme_ops::{cancel_theme_picker, preview_theme, select_theme, theme_tick};
use crate::tiling::append_tiled_window;
use crate::{HyprmuxApp, Msg};

pub(crate) fn handle_msg(_app: &mut HyprmuxApp, msg: Msg, ctx: &mut Context<HyprmuxApp>) -> Update {
    let mut update = match msg {
        Msg::RunAction(action) => {
            ctx.state.show_palette = false;
            let update = execute_action(ctx, action);
            match action {
                Action::OpenSearch => request_search_focus(ctx),
                Action::RenamePane => request_rename_focus(ctx),
                Action::OpenThemePicker => {}
                Action::SaveProfile | Action::OpenProfilePicker | Action::OpenSessionPicker => {}
                // The scratchpad manages its own focus (the scratch terminal on show, the
                // previously focused pane on hide); don't override it.
                Action::ToggleScratchpad => {}
                _ => request_current_pane_focus(ctx),
            }
            update
        }
        Msg::ClosePalette => {
            ctx.state.show_palette = false;
            request_current_pane_focus(ctx);
            Update::full()
        }
        Msg::CloseHelp => {
            ctx.state.show_help = false;
            request_current_pane_focus(ctx);
            Update::full()
        }
        Msg::CloseThemePicker => {
            cancel_theme_picker(ctx);
            request_current_pane_focus(ctx);
            Update::full()
        }
        Msg::PreviewTheme(index) => preview_theme(ctx, index),
        Msg::SelectTheme(index) => select_theme(ctx, index),
        Msg::ThemeTick => theme_tick(ctx),
        Msg::BarTick => {
            // Repaint for the clock, then reschedule only while a clock segment is configured.
            if ctx.state.config.bar.has_clock() {
                Update::with_command(crate::schedule_bar_tick())
            } else {
                Update::none()
            }
        }
        Msg::ThemeError(message) => {
            ctx.toast()
                .push(error_toast(&ctx.state.theme, "Theme Reload", message));
            Update::full()
        }
        Msg::CloseSearch => {
            ctx.state.search = None;
            request_current_pane_focus(ctx);
            Update::full()
        }
        Msg::SearchQueryChanged(query) => {
            if let Some(search) = ctx.state.search.as_mut() {
                let cursor = query.len();
                search.input.set_text(query);
                search.input.set_cursor(cursor);
                search.input.set_anchor(None);
            }
            recompute_search(ctx)
        }
        Msg::SearchNext(backward) => search_next(ctx, backward),
        Msg::SearchSelect(index) => select_search_match(ctx, index),
        Msg::SearchActivate(index) => {
            select_search_match(ctx, index);
            ctx.state.search = None;
            request_current_pane_focus(ctx);
            Update::full()
        }
        Msg::SearchCycleScope => crate::search_ops::cycle_search_scope(ctx),
        Msg::CloseRenamePane => {
            let update = close_rename_pane(ctx);
            request_current_pane_focus(ctx);
            update
        }
        Msg::RenamePaneChanged(event) => {
            if let Some(rename) = ctx.state.rename.as_mut() {
                event.apply_to(&mut rename.input);
            }
            request_rename_focus(ctx);
            Update::full()
        }
        Msg::SubmitRenamePane => {
            let update = apply_rename_pane(ctx);
            request_current_pane_focus(ctx);
            update
        }
        Msg::CloseSaveProfile => {
            let update = close_save_profile_prompt(ctx);
            request_current_pane_focus(ctx);
            update
        }
        Msg::SaveProfileNameChanged(event) => {
            if let Some(prompt) = ctx.state.save_profile_prompt.as_mut() {
                event.apply_to(&mut prompt.input);
            }
            request_save_profile_focus(ctx);
            Update::full()
        }
        Msg::SubmitSaveProfile => {
            let update = submit_save_profile(ctx);
            request_current_pane_focus(ctx);
            update
        }
        Msg::CloseProfilePicker => {
            let update = cancel_profile_picker(ctx);
            request_current_pane_focus(ctx);
            update
        }
        Msg::ProfilePickerQueryChanged(query) => profile_picker_query_changed(ctx, query),
        Msg::ProfilePickerSelect(index) => profile_picker_selection_changed(ctx, index),
        Msg::ProfilePickerSetDefault => profile_picker_set_default(ctx),
        Msg::ProfilePickerDelete => profile_picker_delete_key(ctx),
        Msg::SelectProfile(index) => {
            let update = select_profile(ctx, index);
            request_current_pane_focus(ctx);
            update
        }
        Msg::CloseSessionPicker => {
            ctx.state.show_session_picker = false;
            ctx.state.session_picker = None;
            request_current_pane_focus(ctx);
            Update::full()
        }
        Msg::SessionPickerRefresh => crate::session_ops::refresh_session_picker(ctx),
        Msg::SessionPickerQueryChanged(query) => {
            if let Some(picker) = ctx.state.session_picker.as_mut() {
                picker.input.set_text(query);
                picker.pending_kill = None;
            }
            Update::full()
        }
        Msg::SessionPickerSelect(index) => {
            if let Some(picker) = ctx.state.session_picker.as_mut() {
                picker.selected = index.min(picker.entries.len().saturating_sub(1));
                if picker.pending_kill != Some(index) {
                    picker.pending_kill = None;
                }
            }
            Update::full()
        }
        Msg::SessionPickerActivate(index) => {
            crate::session_ops::activate_selected_session(ctx, index)
        }
        Msg::SessionPickerCreateFromQuery => crate::session_ops::create_from_query(ctx),
        Msg::SessionPickerDetachCurrent => crate::session_ops::detach_current_session(ctx),
        Msg::SessionPickerKillSelected => crate::session_ops::kill_selected_session(ctx),
        Msg::FocusPane(id) => {
            focus_pane(&mut ctx.state, id);
            if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
                pane.activity.has_unseen_output = false;
            }
            request_pane_focus(ctx, id);
            Update::full()
        }
        Msg::HoverPane(id) => {
            if !ctx.state.config.pane.focus_on_hover {
                return Update::none();
            }
            if ctx.state.focused_pane != Some(id) {
                focus_pane(&mut ctx.state, id);
                if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
                    pane.activity.has_unseen_output = false;
                }
                request_pane_focus(ctx, id);
                Update::full()
            } else {
                Update::none()
            }
        }
        Msg::BeginMove(
            id,
            current_rect,
            from_local_x,
            from_local_y,
            target_w,
            target_h,
            modified,
        ) => begin_move(
            ctx,
            id,
            current_rect,
            from_local_x,
            from_local_y,
            target_w,
            target_h,
            modified,
        ),
        Msg::MovePane(id, dx, dy, modified) => move_pane(ctx, id, dx, dy, modified),
        Msg::EndMove(id, x, y) => end_move(ctx, id, x, y),
        Msg::BeginResize(id, corner, modified) => begin_resize(ctx, id, corner, modified),
        Msg::ResizePane(id, corner, dx, dy, modified) => {
            resize_pane(ctx, id, corner, dx, dy, modified)
        }
        Msg::EndResize(id) => {
            if ctx
                .state
                .resizing_pane
                .is_some_and(|session| session.id == id)
            {
                ctx.state.resizing_pane = None;
            }
            Update::full()
        }
        Msg::ResizeSplit(id, horizontal_split, dx, dy) => {
            crate::resize_move_ops::resize_split_by_drag(ctx, id, horizontal_split, dx, dy)
        }
        Msg::FinishOpen(epoch, id, generation) => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
                if pane.pty_generation != generation {
                    return Update::none();
                }
                pane.opening = false;
                if !pane.closing {
                    ctx.state.animation = GeometryAnimation::Spawn;
                }
            }
            Update::full()
        }
        Msg::ActivatePane(epoch, id, generation) => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
                if pane.pty_generation != generation {
                    return Update::none();
                }
                pane.terminal_active = true;
                if !pane.closing && ctx.state.focused_pane == Some(id) {
                    request_pane_focus(ctx, id);
                }
            }
            Update::full()
        }
        Msg::PruneClosed(epoch, id, generation) => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            handle_prune_closed(ctx, id, generation)
        }
        Msg::PtyReady(epoch, id, generation, pty) => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            handle_pty_ready(ctx, id, generation, pty)
        }
        Msg::PtyEvent(epoch, id, generation, event) => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            handle_pty_event(ctx, id, generation, event)
        }
        Msg::PaneInput(id, input) => handle_pane_input(ctx, id, input),
        Msg::PaneKey(id, key) => {
            if logical_focus_pending_activation(&ctx.state).is_none_or(|pending| pending == id) {
                focus_pane(&mut ctx.state, id);
            }
            if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
                pane.activity.has_unseen_output = false;
            }
            let (_handled, update) = handle_key_routing(ctx, key, Some(id));
            update
        }
        Msg::PaneMouse(id, bytes) => handle_pane_mouse(ctx, id, bytes),
        Msg::PaneResize(id, cols, rows) => handle_pane_resize(ctx, id, cols, rows),
        Msg::PaneScroll(id, offset) => handle_pane_scroll(ctx, id, offset),
        Msg::ControlRequest(envelope) => crate::control_ops::handle_control_request(ctx, envelope),
        Msg::SessionConnected {
            epoch,
            name,
            client,
        } => {
            let Some(pending) = ctx.state.pending_session_attach.as_mut() else {
                return Update::none();
            };
            if pending.epoch != epoch || pending.name != name {
                return Update::none();
            }
            pending.client = Some(client);
            Update::full()
        }
        Msg::SessionDisconnected { epoch, name } => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            if ctx.state.session_name.as_deref() == Some(name.as_str()) {
                ctx.state.session_attached = false;
                ctx.state.session_client = None;
                for pane in ctx
                    .state
                    .workspaces
                    .iter_mut()
                    .flat_map(|workspace| workspace.panes.iter_mut())
                    .filter(|pane| pane.terminal.is_server_backed())
                {
                    pane.terminal.status =
                        ManagedTerminalStatus::Error("session disconnected".into());
                }
            }
            ctx.toast().push(error_toast(
                &ctx.state.theme,
                "Session",
                format!("{name} disconnected"),
            ));
            Update::full()
        }
        Msg::SessionAttachFailed { epoch, message } => {
            let expected_pending = ctx
                .state
                .pending_session_attach
                .as_ref()
                .is_some_and(|pending| pending.epoch == epoch);
            if !expected_pending {
                return Update::none();
            }
            ctx.state.pending_session_attach = None;
            ctx.toast()
                .push(error_toast(&ctx.state.theme, "Session Attach", message));
            Update::full()
        }
        Msg::SessionAttached {
            epoch,
            session,
            panes,
            layout_blob,
        } => {
            let Some(pending) = ctx.state.pending_session_attach.as_ref() else {
                return Update::none();
            };
            if pending.epoch != epoch || pending.name != session {
                return Update::none();
            }
            let pending = ctx
                .state
                .pending_session_attach
                .take()
                .expect("pending attach checked above");
            let Some(client) = pending.client else {
                return Update::none();
            };
            ctx.state.runtime_epoch = epoch;
            ctx.state.session_client = Some(client);
            ctx.state.session_name = Some(session);
            ctx.state.session_attached = true;
            let mut restore_layout = false;
            if let Some(blob) = layout_blob
                && let Ok(profile) = crate::profiles::HyprmuxProfile::from_toml_str(&blob)
            {
                restore_layout = true;
                let client = ctx.state.session_client.clone();
                let name = ctx.state.session_name.clone();
                let control_socket_path = ctx.state.control_socket_path.clone();
                let system_theme = ctx.state.system_theme.clone();
                let mut restored =
                    State::from_profile(ctx.state.config.clone(), ctx.state.theme.clone(), profile);
                restored.session_client = client;
                restored.session_name = name;
                restored.session_attached = true;
                restored.runtime_epoch = epoch;
                restored.last_pushed_layout = None;
                restored.control_socket_path = control_socket_path;
                restored.system_theme = system_theme;
                ctx.state = restored;
            }
            let had_panes = !panes.is_empty();
            if !had_panes && !pending.migrate_local_panes {
                replace_with_fresh_server_state(ctx, epoch);
                spawn_existing_panes_on_session(ctx);
                return Update::full();
            }
            apply_attached_panes(ctx, panes, restore_layout);
            if !had_panes && pending.migrate_local_panes {
                ctx.toast().push(crate::pty_events::info_toast(
                    "Attached; local panes moved to session",
                ));
                spawn_existing_panes_on_session(ctx);
            }
            Update::full()
        }
        Msg::SessionSnapshot {
            epoch,
            pane_id,
            generation,
            snapshot,
        } => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            if let Some(pane) = find_pane_mut(&mut ctx.state, pane_id)
                && pane.pty_generation == generation
                && pane.terminal.is_server_backed()
            {
                let title = snapshot.title.clone();
                let cwd = snapshot.cwd.clone();
                match crate::session::client::apply_wire_snapshot(snapshot) {
                    Ok(render_snapshot) => {
                        pane.terminal.apply_snapshot(render_snapshot, title, cwd)
                    }
                    Err(err) => {
                        ctx.toast()
                            .push(error_toast(&ctx.state.theme, "Session", err.to_string()));
                    }
                }
            }
            Update::full()
        }
        Msg::SessionExited {
            epoch,
            pane_id,
            generation,
            code,
        } => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            let mut should_close = false;
            if let Some(pane) = find_pane_mut(&mut ctx.state, pane_id) {
                if pane.pty_generation != generation {
                    return Update::none();
                }
                pane.terminal.status = ManagedTerminalStatus::Exited(code);
                should_close = !pane.closing;
            }
            if should_close {
                if crate::scratchpad::is_scratch(pane_id) {
                    crate::scratchpad::handle_scratch_exit(ctx)
                } else {
                    begin_close_pane(ctx, pane_id, ctx.state.config.animations)
                }
            } else {
                Update::full()
            }
        }
        Msg::SessionBell { epoch, .. } => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            Update::none()
        }
        Msg::SessionSpawnResult {
            epoch,
            pane_id,
            generation,
            ok,
            error,
        } => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            let mut should_close = false;
            let mut toast_error = None;
            if let Some(pane) = find_pane_mut(&mut ctx.state, pane_id) {
                if pane.pty_generation != generation {
                    return Update::none();
                }
                pane.terminal.bind_server_backend(pane_id, generation);
                if ok {
                    pane.terminal.status = ManagedTerminalStatus::Ready;
                } else {
                    let message = error
                        .clone()
                        .unwrap_or_else(|| "session spawn failed".to_string());
                    pane.terminal.status = ManagedTerminalStatus::Error(message.clone().into());
                    toast_error = Some(message);
                    should_close = !pane.closing;
                }
            } else if let Some(error) = error {
                toast_error = Some(error);
            }
            if let Some(error) = toast_error {
                ctx.toast()
                    .push(error_toast(&ctx.state.theme, "Session Spawn", error));
            }
            if should_close {
                begin_close_pane(ctx, pane_id, ctx.state.config.animations)
            } else {
                Update::full()
            }
        }
        Msg::SessionSearchResult {
            epoch,
            request_id,
            pane_id,
            generation,
            query,
            matches,
        } => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            let Some(pane) = find_pane_mut(&mut ctx.state, pane_id) else {
                return Update::none();
            };
            if pane.pty_generation != generation || !pane.terminal.is_server_backed() {
                return Update::none();
            }
            if let Some(search) = ctx.state.search.as_mut()
                && search.input.text().trim() == query
                && search.pending_server_requests.contains(&request_id)
            {
                search
                    .pending_server_requests
                    .retain(|id| *id != request_id);
                search.matches.extend(matches.into_iter().map(|matched| {
                    crate::state::ScrollbackMatch {
                        offset: matched.offset,
                        line: matched.line,
                        start_col: matched.start_col,
                        end_col: matched.end_col,
                        text: matched.text,
                        pane: pane_id,
                    }
                }));
                search.current = search.current.min(search.matches.len().saturating_sub(1));
                search.status = format!("{} matches", search.matches.len());
            }
            Update::full()
        }
        Msg::SessionError { epoch, message } => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            ctx.toast()
                .push(error_toast(&ctx.state.theme, "Session", message));
            Update::full()
        }
    };

    if crate::theme_ops::apply_terminal_palette_to_state(&mut ctx.state) {
        let command = update.command.take();
        update = Update::with_command(command);
    }

    if ctx.state.session_attached
        && let Some(client) = ctx.state.session_client.clone()
        && let Ok(blob) = crate::profiles::profile_from_state(&ctx.state).to_toml_string()
        && ctx.state.last_pushed_layout.as_deref() != Some(blob.as_str())
    {
        client.push_layout(blob.clone());
        ctx.state.last_pushed_layout = Some(blob);
    }

    update
}

fn logical_focus_pending_activation(state: &State) -> Option<crate::state::PaneId> {
    let id = state.focused_pane?;
    state.workspaces[state.active_workspace]
        .panes
        .iter()
        .any(|pane| pane.id == id && !pane.terminal_active && !pane.closing)
        .then_some(id)
}

fn apply_attached_panes(
    ctx: &mut Context<HyprmuxApp>,
    panes: Vec<crate::session::protocol::AttachedPane>,
    restored_layout: bool,
) {
    let panes: Vec<_> = panes
        .into_iter()
        .filter(|pane| pane.exited.is_none())
        .collect();
    let attached_ids: std::collections::HashSet<_> =
        panes.iter().map(|pane| pane.pane_id).collect();
    if restored_layout {
        for workspace in &mut ctx.state.workspaces {
            workspace
                .panes
                .retain(|pane| attached_ids.contains(&pane.id));
            if workspace
                .focused_pane
                .is_some_and(|id| !attached_ids.contains(&id))
            {
                workspace.focused_pane = None;
            }
        }
        if ctx
            .state
            .focused_pane
            .is_some_and(|id| !attached_ids.contains(&id))
        {
            ctx.state.focused_pane = None;
        }
    } else if !panes.is_empty() {
        for workspace in &mut ctx.state.workspaces {
            workspace.panes.clear();
            workspace.tile_tree = None;
            workspace.focused_pane = None;
        }
        ctx.state.focused_pane = None;
    }

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
            pane.terminal
                .bind_server_backend(attached.pane_id, attached.generation);
            let title = attached.snapshot.title.clone();
            let cwd = attached.snapshot.cwd.clone();
            match crate::session::client::apply_wire_snapshot(attached.snapshot) {
                Ok(snapshot) => pane.terminal.apply_snapshot(snapshot, title, cwd),
                Err(err) => {
                    ctx.toast()
                        .push(error_toast(&ctx.state.theme, "Session", err.to_string()));
                }
            }
        }
        ctx.state.next_pane_id = ctx
            .state
            .next_pane_id
            .max(attached.pane_id.saturating_add(1));
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

fn replace_with_fresh_server_state(ctx: &mut Context<HyprmuxApp>, epoch: u64) {
    let config = ctx.state.config.clone();
    let theme = ctx.state.theme.clone();
    let theme_watcher = ctx.state.theme_watcher.take();
    let system_theme = ctx.state.system_theme.clone();
    let control_socket_path = ctx.state.control_socket_path.clone();
    let session_client = ctx.state.session_client.clone();
    let session_name = ctx.state.session_name.clone();
    let next_pty_generation = ctx.state.next_pty_generation;

    let mut fresh = State::new(config, theme);
    fresh.theme_watcher = theme_watcher;
    fresh.system_theme = system_theme;
    fresh.control_socket_path = control_socket_path;
    fresh.session_client = session_client;
    fresh.session_name = session_name;
    fresh.session_attached = true;
    fresh.runtime_epoch = epoch;
    fresh.next_pty_generation = next_pty_generation;
    ctx.state = fresh;
    crate::theme_ops::apply_terminal_palette_to_state(&mut ctx.state);
    ctx.toast().push(crate::pty_events::info_toast(
        "Attached; opened a fresh pane",
    ));
}

fn spawn_existing_panes_on_session(ctx: &mut Context<HyprmuxApp>) {
    let Some(client) = ctx.state.session_client.clone() else {
        return;
    };
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
        client.spawn_pane(
            pane.id,
            generation,
            pane.identity.command.clone(),
            pane.identity.cwd.clone(),
            pane.terminal.cols,
            pane.terminal.rows,
            pane.identity.keep_open,
            pane.identity.custom_title.clone(),
        );
    }
}
