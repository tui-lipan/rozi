use tui_lipan::prelude::*;

use crate::actions::{execute_action, execute_palette_action};
use crate::anim::GeometryAnimation;
use crate::focus_ops::{
    focus_pane, request_current_pane_focus, request_pane_focus, request_rename_focus,
    request_rename_session_focus, request_save_profile_focus, request_search_focus,
};
use crate::identity_ops::{apply_rename_pane, close_rename_pane};
use crate::input::Action;
use crate::key_routing::handle_key_routing;
use crate::pane_lifecycle::{begin_close_pane, find_pane_mut, handle_prune_closed, pane_env};
use crate::profile_ops::{
    cancel_profile_picker, close_save_profile_prompt, profile_picker_delete_key,
    profile_picker_query_changed, profile_picker_selection_changed, profile_picker_set_default,
    select_profile, submit_save_profile,
};
use crate::pty_events::{
    error_toast, handle_pane_input, handle_pane_mouse, handle_pane_resize, handle_pane_scroll,
    maybe_notify_pane_exit,
};
use crate::resize_move_ops::{begin_move, begin_resize, end_move, move_pane, resize_pane};
use crate::search_ops::{recompute_search, search_next, select_search_match};
use crate::state::State;
use crate::theme_ops::{cancel_theme_picker, preview_theme, select_theme, theme_tick};
use crate::tiling::append_tiled_window;
use crate::{HyprmuxApp, Msg};

const LAYOUT_COMMIT_DEBOUNCE_MS: u64 = 16;

fn valid_padding_text(value: &str) -> bool {
    value.is_empty()
        || (value.len() == 1
            && value.as_bytes()[0].is_ascii_digit()
            && u16::from(value.as_bytes()[0] - b'0') <= crate::config::MAX_PANE_PADDING)
}

fn padding_value(value: &str) -> Option<u16> {
    valid_padding_text(value)
        .then(|| value.parse().ok())
        .flatten()
}

fn padding_error(ctx: &mut Context<HyprmuxApp>) {
    ctx.toast().push(error_toast(
        &ctx.state.theme,
        "Padding needs a value",
        "Enter one digit.",
    ));
}

pub(crate) fn handle_msg(_app: &mut HyprmuxApp, msg: Msg, ctx: &mut Context<HyprmuxApp>) -> Update {
    let is_layout_flush = matches!(&msg, Msg::FlushLayoutCommit { .. });
    let mut update = match msg {
        Msg::CommandLinkReady(link) => {
            ctx.state.command_link = Some(link);
            Update::none()
        }
        Msg::RunAction(action) => {
            if matches!(
                action,
                Action::OpenAppearance | Action::TogglePalette | Action::ToggleHelp
            ) {
                ctx.state.pane_padding_editor = None;
            }
            let cycle_layout_in_palette =
                matches!(action, Action::ToggleLayout) && ctx.state.show_palette;
            let from_palette = ctx.state.show_palette;
            if !cycle_layout_in_palette {
                ctx.state.show_palette = false;
            }
            let update = if from_palette {
                execute_palette_action(ctx, action)
            } else {
                execute_action(ctx, action)
            };
            match action {
                Action::OpenSearch => request_search_focus(ctx),
                Action::RenamePane => request_rename_focus(ctx),
                Action::RenameWorkspace | Action::RenameSession => {
                    request_rename_session_focus(ctx)
                }
                Action::OpenAppearance => {}
                Action::OpenThemePicker => {}
                Action::SaveProfile
                | Action::OpenProfilePicker
                | Action::OpenSessionPicker
                | Action::OpenClientList => {}
                // The scratchpad manages its own focus (the scratch terminal on show, the
                // previously focused pane on hide); don't override it.
                Action::ToggleScratchpad => {}
                Action::ToggleLayout if cycle_layout_in_palette => {}
                _ => request_current_pane_focus(ctx),
            }
            update
        }
        Msg::ClosePalette => {
            ctx.state.show_palette = false;
            ctx.state.commands_dirty = true;
            request_current_pane_focus(ctx);
            Update::full()
        }
        Msg::CloseHelp => {
            ctx.state.show_help = false;
            ctx.state.commands_dirty = true;
            request_current_pane_focus(ctx);
            Update::full()
        }
        Msg::CloseAppearance => {
            ctx.state.show_appearance = false;
            ctx.state.pane_padding_editor = None;
            ctx.state.commands_dirty = true;
            request_current_pane_focus(ctx);
            Update::full()
        }
        Msg::AppearanceActivate(action) => {
            // A greyed row (its parent feature is off) is inert: keep the overlay open and focused
            // but change nothing. Otherwise dispatch the row's underlying action.
            if action.disabled_reason(&ctx.state.config.pane).is_some() {
                ctx.request_focus(crate::view::appearance_palette_key());
            } else {
                match action {
                    crate::state::AppearanceAction::Theme => {
                        execute_action(ctx, Action::OpenThemePicker);
                    }
                    crate::state::AppearanceAction::EditPadding => {
                        ctx.state.pane_padding_editor =
                            Some(crate::state::PanePaddingEditorState::new(
                                ctx.state.config.pane.padding,
                            ));
                        ctx.request_focus(crate::view::pane_padding_vertical_key());
                    }
                    crate::state::AppearanceAction::ToggleTitles => {
                        execute_action(ctx, Action::ToggleTitles);
                    }
                    crate::state::AppearanceAction::ToggleWorkbar => {
                        execute_action(ctx, Action::ToggleWorkbar);
                    }
                    crate::state::AppearanceAction::ToggleWorkbarGap => {
                        execute_action(ctx, Action::ToggleWorkbarGap);
                    }
                    crate::state::AppearanceAction::ToggleWorkbarPosition => {
                        execute_action(ctx, Action::ToggleWorkbarPosition);
                    }
                    crate::state::AppearanceAction::ToggleWorkbarPowerline => {
                        execute_action(ctx, Action::ToggleWorkbarPowerline);
                    }
                    crate::state::AppearanceAction::ToggleAnimations => {
                        execute_action(ctx, Action::ToggleAnimations);
                    }
                    crate::state::AppearanceAction::ToggleHighlightFocusedBackground => {
                        execute_action(ctx, Action::ToggleHighlightFocusedBackground);
                    }
                    crate::state::AppearanceAction::ToggleHighlightFocusedBorder => {
                        execute_action(ctx, Action::ToggleHighlightFocusedBorder);
                    }
                    crate::state::AppearanceAction::ToggleBorderMerge => {
                        execute_action(ctx, Action::ToggleBorderMerge);
                    }
                    crate::state::AppearanceAction::ToggleBackgroundFollowsTerminal => {
                        execute_action(ctx, Action::ToggleBackgroundFollowsTerminal);
                    }
                    crate::state::AppearanceAction::CycleBorderStyle => {
                        execute_action(ctx, Action::CycleBorderStyle);
                    }
                    crate::state::AppearanceAction::CycleTitleStyle => {
                        execute_action(ctx, Action::CycleTitleStyle);
                    }
                    crate::state::AppearanceAction::CycleWorkbarBadgeStyle => {
                        execute_action(ctx, Action::CycleWorkbarBadgeStyle);
                    }
                    crate::state::AppearanceAction::CycleWorkbarTabStyle => {
                        execute_action(ctx, Action::CycleWorkbarTabStyle);
                    }
                    crate::state::AppearanceAction::CycleWorkbarStyle => {
                        execute_action(ctx, Action::CycleWorkbarStyle);
                    }
                }
                if !matches!(
                    action,
                    crate::state::AppearanceAction::Theme
                        | crate::state::AppearanceAction::EditPadding
                ) {
                    ctx.state.show_appearance = true;
                    ctx.request_focus(crate::view::appearance_palette_key());
                }
            }
            Update::full()
        }
        Msg::ClosePanePaddingEditor => {
            if ctx.state.pane_padding_editor.is_none() {
                return Update::none();
            }
            ctx.state.pane_padding_editor = None;
            if ctx.state.show_appearance {
                ctx.request_focus(crate::view::appearance_palette_key());
            }
            Update::full()
        }
        Msg::PanePaddingVerticalChanged(event) => {
            let Some(editor) = ctx.state.pane_padding_editor.as_mut() else {
                return Update::none();
            };
            if valid_padding_text(&event.value) {
                event.apply_to(&mut editor.vertical);
            }
            ctx.request_focus(crate::view::pane_padding_vertical_key());
            Update::full()
        }
        Msg::PanePaddingHorizontalChanged(event) => {
            let Some(editor) = ctx.state.pane_padding_editor.as_mut() else {
                return Update::none();
            };
            if valid_padding_text(&event.value) {
                event.apply_to(&mut editor.horizontal);
            }
            ctx.request_focus(crate::view::pane_padding_horizontal_key());
            Update::full()
        }
        Msg::AdvancePanePadding => {
            let Some(editor) = ctx.state.pane_padding_editor.as_ref() else {
                return Update::none();
            };
            if padding_value(editor.vertical.text()).is_some() {
                ctx.request_focus(crate::view::pane_padding_horizontal_key());
            } else {
                padding_error(ctx);
                ctx.request_focus(crate::view::pane_padding_vertical_key());
            }
            Update::full()
        }
        Msg::SubmitPanePadding => {
            let Some(editor) = ctx.state.pane_padding_editor.as_ref() else {
                return Update::none();
            };
            let vertical = padding_value(editor.vertical.text());
            let horizontal = padding_value(editor.horizontal.text());
            let Some(vertical) = vertical else {
                padding_error(ctx);
                ctx.request_focus(crate::view::pane_padding_vertical_key());
                return Update::full();
            };
            let Some(horizontal) = horizontal else {
                padding_error(ctx);
                ctx.request_focus(crate::view::pane_padding_horizontal_key());
                return Update::full();
            };
            ctx.state.config.pane.padding = (vertical, horizontal, vertical, horizontal);
            if let Err(error) = crate::config::persist_pane_padding(vertical, horizontal) {
                ctx.toast().push(error_toast(
                    &ctx.state.theme,
                    "Could not save padding",
                    error,
                ));
            }
            ctx.state.pane_padding_editor = None;
            if ctx.state.show_appearance {
                ctx.request_focus(crate::view::appearance_palette_key());
            }
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
        Msg::ConfigFileChanged => crate::config_ops::config_file_changed(ctx),
        Msg::WorkbarTick => {
            // Repaint for the clock, then reschedule only while a clock segment is configured.
            if ctx.state.config.workbar.has_clock() {
                Update::with_command(crate::schedule_workbar_tick())
            } else {
                Update::none()
            }
        }
        Msg::WorkbarCommandOutput(command, output) => {
            ctx.state.workbar_command_output.insert(command, output);
            Update::full()
        }
        Msg::ThemeError(message) => {
            ctx.toast()
                .push(error_toast(&ctx.state.theme, "Theme Reload", message));
            Update::full()
        }
        Msg::CloseSearch => {
            ctx.state.search = None;
            ctx.state.commands_dirty = true;
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
            ctx.state.commands_dirty = true;
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
        Msg::CloseRenameSession => crate::session_ops::close_rename_session(ctx),
        Msg::RenameSessionChanged(event) => {
            if let Some(rename) = ctx.state.rename_session.as_mut() {
                event.apply_to(&mut rename.input);
                // Editing the name reconsiders the create-from-ephemeral choice, so drop any armed
                // confirmation; the next Enter re-arms it (see `apply_rename_session`).
                rename.pending_confirm = false;
            }
            crate::focus_ops::request_rename_session_focus(ctx);
            Update::full()
        }
        Msg::SubmitRenameSession => crate::session_ops::apply_rename_session(ctx),
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
        Msg::CloseSessionPicker => crate::session_ops::close_session_picker(ctx),
        Msg::SessionsDiscovered { epoch, rows } => {
            crate::session_ops::apply_discovered_sessions(ctx, epoch, rows)
        }
        Msg::SessionPickerQueryChanged(query) => {
            if let Some(picker) = ctx.state.session_picker.as_mut() {
                picker.input.set_text(query);
            }
            crate::session_ops::clear_pending_session_arms(ctx);
            Update::full()
        }
        Msg::SessionPickerSelect(index) => {
            if let Some(picker) = ctx.state.session_picker.as_mut() {
                picker.selected = index.min(picker.entries.len().saturating_sub(1));
            }
            // Moving the highlight off an armed row cancels its confirmation (and dismisses the
            // toast); staying on it keeps the arming alive for a confirming second press. Applies to
            // both the kill and the open-discards-ephemeral guards.
            let moved_off_armed = ctx.state.session_picker.as_ref().is_some_and(|picker| {
                let selected = picker.selected;
                picker.pending_kill.is_some_and(|index| index != selected)
                    || picker.pending_open.is_some_and(|index| index != selected)
            });
            if moved_off_armed {
                crate::session_ops::clear_pending_session_arms(ctx);
            }
            Update::full()
        }
        Msg::SessionPickerActivate(index) => {
            crate::session_ops::activate_selected_session(ctx, index)
        }
        Msg::SessionPickerCreateFromQuery => crate::session_ops::open_create_session(ctx),
        Msg::SessionPickerDetachCurrent => crate::session_ops::detach_current_session(ctx),
        Msg::SessionPickerKillSelected => crate::session_ops::kill_selected_session(ctx),
        Msg::SessionPickerNameCurrent => crate::session_ops::open_rename_session(ctx),
        Msg::CloseClientList => {
            ctx.state.client_list = None;
            ctx.state.commands_dirty = true;
            request_current_pane_focus(ctx);
            Update::full()
        }
        Msg::ClientListSelect(index) => {
            if let Some(list) = ctx.state.client_list.as_mut() {
                let len = ctx
                    .state
                    .shared
                    .as_ref()
                    .map_or(0, |shared| shared.clients.len());
                list.selected = index.min(len.saturating_sub(1));
            }
            Update::full()
        }
        Msg::ClientListGrant(index) => crate::session_ops::grant_control(ctx, index),
        Msg::ClientListDecline(index) => crate::session_ops::decline_control(ctx, index),
        Msg::FocusPane(id) => {
            focus_pane(&mut ctx.state, id);
            if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
                pane.activity.has_unseen_output = false;
            }
            request_pane_focus(ctx, id);
            Update::full()
        }
        Msg::HoverPane(id) => crate::focus_ops::hover_focus_pane(ctx, id),
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
        Msg::BeginResize(id, corner, x, y, modified) => {
            begin_resize(ctx, id, corner, x, y, modified)
        }
        Msg::ResizePane(id, corner, from_x, from_y, x, y, modified) => {
            resize_pane(ctx, id, corner, (from_x, from_y), (x, y), modified)
        }
        Msg::EndResize(id) => {
            if ctx
                .state
                .resizing_pane
                .as_ref()
                .is_some_and(|session| session.id == id)
            {
                ctx.state.resizing_pane = None;
            }
            Update::full()
        }
        Msg::BeginResizeSplit(id, horizontal_split, x, y) => {
            crate::resize_move_ops::begin_resize_split_drag(ctx, id, horizontal_split, x, y)
        }
        Msg::ResizeSplit(id, horizontal_split, from_x, from_y, x, y) => {
            crate::resize_move_ops::resize_split_by_drag(
                ctx,
                id,
                horizontal_split,
                from_x,
                from_y,
                x,
                y,
            )
        }
        Msg::BeginResizeSplitJunction(left_id, top_id, x, y) => {
            crate::resize_move_ops::begin_resize_split_junction_drag(ctx, left_id, top_id, x, y)
        }
        Msg::ResizeSplitJunction(left_id, top_id, from_x, from_y, x, y) => {
            crate::resize_move_ops::resize_split_junction_by_drag(
                ctx, left_id, top_id, from_x, from_y, x, y,
            )
        }
        Msg::EndResizeSplit => {
            ctx.state.split_drag = None;
            Update::full()
        }
        Msg::BeginScratchResize(_from_y) => crate::scratchpad::begin_resize(ctx),
        Msg::ScratchResize(from_y, y) => crate::scratchpad::resize(ctx, from_y, y),
        Msg::EndScratchResize => crate::scratchpad::end_resize(ctx),
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
        Msg::PaneInput(id, input) => handle_pane_input(ctx, id, input),
        Msg::CopyFlashExpired(id, flash_id) => crate::copy_mode::expire_flash(ctx, id, flash_id),
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
            // Only the current session's unexpected disconnect matters; an intentional detach or
            // attach-elsewhere has already bumped the epoch, so its stale disconnect is filtered
            // out above.
            if ctx.state.session_name.as_deref() != Some(name.as_str()) {
                return Update::none();
            }
            if ctx.state.pending_session_attach.is_some() {
                return Update::full();
            }
            ctx.state.session_attached = false;
            ctx.state.session_client = None;
            let read_only = ctx
                .state
                .shared
                .as_ref()
                .is_some_and(|shared| shared.read_only);
            // Drop shared-lease bookkeeping: while disconnected we behave as a solo controller, and
            // a successful reconnect rebuilds this from the fresh `Attached` frame.
            ctx.state.shared = None;
            for pane in ctx
                .state
                .workspaces
                .iter_mut()
                .flat_map(|workspace| workspace.panes.iter_mut())
            {
                pane.terminal.status = ManagedTerminalStatus::Error("session disconnected".into());
            }
            // Try to reconnect: an ephemeral server may still be alive (transient hiccup), so
            // reattach and re-seed. Only ephemeral sessions autostart a replacement server.
            let autostart = crate::state::is_ephemeral_session_name(&name);
            let new_epoch = ctx.state.runtime_epoch.saturating_add(1);
            ctx.state.pending_session_attach = Some(crate::state::PendingSessionAttach {
                epoch: new_epoch,
                name: name.clone(),
                client: None,
                autostart,
                read_only,
            });
            ctx.toast().push(crate::pty_events::info_toast(
                &ctx.state.theme,
                format!("Reconnecting to {name}…"),
            ));
            Update::with_command(Command::spawn(move |link| {
                std::thread::spawn(move || {
                    crate::session::bootstrap::attach_session_client(
                        new_epoch, name, autostart, read_only, link,
                    )
                });
            }))
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
                .push(error_toast(&ctx.state.theme, "Sessions", message));
            Update::full()
        }
        Msg::SessionAttached {
            epoch,
            session,
            client_id,
            panes,
            layout_rev,
            layout,
            controller,
            clients,
            input_locked,
            read_only,
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

            let mut shared = crate::state::SharedSessionState::new(client_id);
            shared.layout_rev = layout_rev;
            shared.assumed_rev = layout_rev;
            shared.controller = controller;
            shared.clients = clients;
            shared.input_locked = input_locked;
            shared.read_only = read_only;
            ctx.state.shared = Some(shared);

            // The session identity just changed (ephemeral vs named), which the "Name/Rename
            // session" palette label reflects; the lease state affects command labels too.
            ctx.state.commands_dirty = true;

            let panes: Vec<_> = panes
                .into_iter()
                .filter(|pane| pane.exited.is_none())
                .collect();
            let had_panes = !panes.is_empty();

            if let Some(layout) = layout {
                // Shared attach: seed the whole window-manager structure from the authoritative
                // layout via the one reconciler code path, then bind server backends and sizes
                // from the pane metadata before the replay seed frames arrive.
                reset_state_for_shared_seed(&mut ctx.state);
                crate::shared_layout::apply_shared_layout(ctx, &layout, layout_rev);
                bind_attached_pane_backends(ctx, panes);
                flush_pending_spawns(ctx);
                Update::full()
            } else if had_panes {
                // Defensive: a live server holding panes but no committed layout (should not occur
                // under protocol v3). Adopt the panes, then republish a layout if we control it.
                apply_attached_panes(ctx, panes);
                flush_pending_spawns(ctx);
                Update::full()
            } else {
                // An empty server (fresh ephemeral, autostarted named session, or one whose panes
                // all exited): seed it with the panes the client already holds in state; the first
                // attacher (controller) commits rev 1 on the tail chokepoint pass.
                let spawned = spawn_state_panes_on_session(ctx);
                flush_pending_spawns(ctx);
                if spawned.is_empty() {
                    Update::full()
                } else {
                    let open_delay = crate::anim::open_delay(ctx.state.config.animations);
                    let activate_delay = crate::anim::activation_delay(ctx.state.config.animations);
                    Update::with_command(crate::pane_lifecycle::open_timers_batch_command(
                        epoch,
                        spawned,
                        open_delay,
                        activate_delay,
                    ))
                }
            }
        }
        Msg::SessionLayoutCommitted {
            epoch,
            rev,
            author,
            layout,
        } => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            let my_id = ctx.state.shared.as_ref().map(|shared| shared.client_id);
            if my_id == Some(author) {
                // Echo of our own commit: confirm the revision, never re-apply our own layout.
                if let Some(shared) = ctx.state.shared.as_mut() {
                    shared.layout_rev = rev;
                }
                Update::none()
            } else {
                crate::shared_layout::apply_shared_layout(ctx, &layout, rev)
            }
        }
        Msg::SessionLayoutRejected {
            epoch,
            current_rev,
            layout,
        } => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            let update = if let Some(layout) = layout {
                crate::shared_layout::apply_shared_layout(ctx, &layout, current_rev)
            } else {
                Update::full()
            };
            if let Some(shared) = ctx.state.shared.as_mut() {
                shared.assumed_rev = current_rev;
                // Clear the dirty detector so the debounced chokepoint recommits from current state.
                shared.last_committed_layout = None;
            }
            update
        }
        Msg::SessionControllerChanged {
            epoch,
            controller,
            reason: _,
        } => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            let was_controller = ctx.state.is_controller();
            if let Some(shared) = ctx.state.shared.as_mut() {
                shared.controller = controller;
                if shared.is_controller() {
                    // Gaining control: rebase optimistic commits, and clear the dirty detector so
                    // the tail chokepoint republishes the layout with our canonical canvas.
                    shared.assumed_rev = shared.layout_rev;
                    shared.last_committed_layout = None;
                }
            }
            let now_controller = ctx.state.is_controller();
            if was_controller && !now_controller {
                ctx.state.moving_pane = None;
                ctx.state.resizing_pane = None;
                ctx.state.split_drag = None;
                let who = controller
                    .map(|id| format!("client {id}"))
                    .unwrap_or_else(|| "another client".to_string());
                ctx.toast().push(crate::pty_events::info_toast(
                    &ctx.state.theme,
                    format!("Layout control taken by {who}"),
                ));
            } else if !was_controller && now_controller {
                ctx.toast().push(crate::pty_events::info_toast(
                    &ctx.state.theme,
                    "You now control the layout",
                ));
            }
            ctx.state.commands_dirty = true;
            Update::full()
        }
        Msg::SessionClientsChanged {
            epoch,
            clients,
            input_locked,
        } => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            if let Some(shared) = ctx.state.shared.as_mut() {
                let changed = shared.input_locked != input_locked;
                shared.clients = clients;
                shared.input_locked = input_locked;
                if changed {
                    ctx.toast().push(crate::pty_events::info_toast(
                        &ctx.state.theme,
                        if input_locked {
                            "Input locked to controller"
                        } else {
                            "Input unlocked"
                        },
                    ));
                }
            }
            ctx.state.commands_dirty = true;
            Update::full()
        }
        Msg::SessionControlRequested { epoch, from } => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            // Only the controller can act on a request; ignore a stale one that arrived after we lost
            // the lease. The requester's badge lives in the roster (via ClientsChanged) regardless.
            if !ctx.state.is_controller() {
                return Update::none();
            }
            let who = ctx
                .state
                .shared
                .as_ref()
                .and_then(|shared| shared.clients.iter().find(|client| client.id == from))
                .map(|client| format!("{} #{}", client.label, client.id))
                .unwrap_or_else(|| format!("client {from}"));
            // Advertise the *live* grant binding so the hint tracks any `[keys]` override instead of a
            // hardcoded key; fall back to the session-clients view when the action is unbound.
            let how = crate::commands::command_prefix_chord(ctx, "grant-control")
                .map(|chord| format!("{chord} to grant"))
                .unwrap_or_else(|| "grant from Session clients".to_string());
            ctx.toast().push(crate::pty_events::info_toast(
                &ctx.state.theme,
                format!("{who} requests layout control\n{how}"),
            ));
            ctx.state.commands_dirty = true;
            Update::full()
        }
        Msg::SessionControlDeclined { epoch } => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            ctx.toast().push(crate::pty_events::info_toast(
                &ctx.state.theme,
                "Your control request was declined",
            ));
            Update::full()
        }
        Msg::SessionPing { epoch, seq } => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            if let Some(client) = ctx.state.session_client.as_ref() {
                client.pong(seq);
            }
            Update::none()
        }
        Msg::FlushPaneResizes { epoch } => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            crate::pty_events::flush_pending_resizes(ctx);
            Update::none()
        }
        Msg::FlushLayoutCommit { epoch } => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            if let Some(shared) = ctx.state.shared.as_mut() {
                shared.layout_commit_scheduled = false;
            }
            flush_layout_commit(ctx);
            Update::none()
        }
        Msg::SessionOutput {
            epoch,
            pane_id,
            generation,
            bytes,
        } => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            let focused = ctx.state.focused_pane;
            let matched = match find_pane_mut(&mut ctx.state, pane_id) {
                Some(pane) if pane.pty_generation == generation => {
                    pane.terminal.process_server_output(&bytes);
                    pane.activity.last_activity = Some(std::time::Instant::now());
                    if focused != Some(pane_id) {
                        pane.activity.has_unseen_output = true;
                    }
                    true
                }
                _ => false,
            };
            if !matched {
                // Output arrived before the layout commit that introduces this pane (or its new
                // generation). Buffer it so the reconciler can replay it when the pane appears;
                // dropping it would leave a follower's fresh pane blank until the next redraw.
                if let Some(shared) = ctx.state.shared.as_mut() {
                    shared.buffer_orphan_output(pane_id, generation, &bytes);
                }
            }
            Update::full()
        }
        Msg::SessionResized {
            epoch,
            pane_id,
            generation,
            cols,
            rows,
        } => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            if let Some(pane) = find_pane_mut(&mut ctx.state, pane_id)
                && pane.pty_generation == generation
                && pane.terminal.apply_server_resize(cols, rows)
            {
                return Update::full();
            }
            Update::none()
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
            if !should_close {
                return Update::full();
            }
            // The scratchpad is a local overlay (never in the shared layout), so every client that
            // owns it closes it directly.
            if crate::scratchpad::is_scratch(pane_id) {
                return crate::scratchpad::handle_scratch_exit(ctx);
            }
            // Closing a tiled/floating pane is a structural layout change: only the controller acts
            // on the exit and commits the new layout; followers close it when that commit arrives.
            if !ctx.state.is_controller() {
                return Update::full();
            }
            maybe_notify_pane_exit(&ctx.state.config, pane_id, code);
            // A clean exit closes the pane on its own; only a failure code is worth surfacing.
            if code != 0 {
                ctx.toast().push(crate::pty_events::info_toast(
                    &ctx.state.theme,
                    format!("Pane {pane_id} exited ({code})"),
                ));
            }
            begin_close_pane(ctx, pane_id, ctx.state.config.animations)
        }
        Msg::SessionSpawnResult {
            epoch,
            pane_id,
            generation,
            pid,
            ok,
            error,
        } => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            let is_controller = ctx.state.is_controller();
            let mut should_close = false;
            let mut toast_error = None;
            if let Some(pane) = find_pane_mut(&mut ctx.state, pane_id) {
                if pane.pty_generation != generation {
                    return Update::none();
                }
                // A follower may already hold this pane (bound and Ready) from the reconciler; only
                // (re)bind a fresh backend for a pane still waiting on its own spawn to complete,
                // so we never destroy a live screen that is already replaying server output.
                if !pane.terminal.is_ready() {
                    pane.terminal.bind_server_backend(pane_id, generation);
                }
                pane.terminal.child_pid = pid;
                if ok {
                    pane.terminal.status = ManagedTerminalStatus::Ready;
                } else {
                    let message = error
                        .clone()
                        .unwrap_or_else(|| "session spawn failed".to_string());
                    pane.terminal.status = ManagedTerminalStatus::Error(message.clone().into());
                    toast_error = Some(message);
                    // Only the controller structurally removes the failed pane; followers wait for
                    // the resulting layout commit.
                    should_close = !pane.closing && is_controller;
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
        Msg::SessionError { epoch, message } => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            if message.trim().is_empty() {
                return Update::none();
            }
            ctx.toast()
                .push(error_toast(&ctx.state.theme, "Session", message));
            Update::full()
        }
        Msg::SessionRenamed { epoch, session } => {
            if epoch != ctx.state.runtime_epoch {
                return Update::none();
            }
            ctx.state.session_name = Some(session.clone());
            // An ephemeral session becoming named flips the "Name/Rename session" palette label.
            ctx.state.commands_dirty = true;
            ctx.toast().push(crate::pty_events::info_toast(
                &ctx.state.theme,
                format!("Renamed session to `{session}`"),
            ));
            Update::full()
        }
    };

    if crate::theme_ops::apply_terminal_palette_to_state(&mut ctx.state) {
        let command = update.command.take();
        update = Update::with_command(command);
    }

    if ctx.state.commands_dirty {
        ctx.state.commands_dirty = false;
        crate::commands::sync(ctx);
    }

    // Layout commit chokepoint: after every message, schedule a bounded trailing-edge diff. The
    // flush message itself is excluded so an idle client does not perpetually re-arm the timer.
    if !is_layout_flush {
        schedule_layout_commit(ctx);
    }

    update
}

pub(crate) fn schedule_layout_commit(ctx: &mut Context<HyprmuxApp>) {
    if !ctx.state.session_attached || !ctx.state.is_controller() {
        return;
    }
    let epoch = ctx.state.runtime_epoch;
    let Some(shared) = ctx.state.shared.as_ref() else {
        flush_layout_commit(ctx);
        return;
    };
    if shared.layout_commit_scheduled {
        return;
    }
    let Some(link) = ctx.state.command_link.clone() else {
        flush_layout_commit(ctx);
        return;
    };
    ctx.state
        .shared
        .as_mut()
        .expect("shared session checked above")
        .layout_commit_scheduled = true;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(LAYOUT_COMMIT_DEBOUNCE_MS));
        link.send(Msg::FlushLayoutCommit { epoch });
    });
}

/// If this client controls a shared session and its layout differs from the last commit, publish a
/// new [`SharedLayout`] at the optimistic base revision. The canonical canvas is this controller's
/// own pane canvas (viewport minus workbar), which followers letterbox to.
pub(crate) fn flush_layout_commit(ctx: &mut Context<HyprmuxApp>) {
    if !ctx.state.session_attached || !ctx.state.is_controller() {
        return;
    }
    let Some(client) = ctx.state.session_client.clone() else {
        return;
    };
    let bounds = ctx.state.canvas_bounds(ctx.viewport());
    let canvas = (
        bounds.w.round().max(1.0) as u16,
        bounds.h.round().max(1.0) as u16,
    );
    let layout = crate::shared_layout::shared_layout_from_state(&ctx.state, canvas);
    let Some(shared) = ctx.state.shared.as_mut() else {
        return;
    };
    if shared.last_committed_layout.as_ref() == Some(&layout) {
        return;
    }
    let base_rev = shared.assumed_rev;
    client.commit_layout(base_rev, layout.clone());
    // Optimistically advance so a rapid burst of edits pipelines onto sequential base revisions;
    // the server's echo confirms `layout_rev`, and a reject resets `assumed_rev`.
    shared.assumed_rev = shared.assumed_rev.saturating_add(1);
    shared.last_committed_layout = Some(layout);
    shared.canonical_canvas = Some(canvas);
}

fn logical_focus_pending_activation(state: &State) -> Option<crate::state::PaneId> {
    let id = state.focused_pane?;
    state.workspaces[state.active_workspace]
        .panes
        .iter()
        .any(|pane| pane.id == id && !pane.terminal_active && !pane.closing)
        .then_some(id)
}

/// Clear all window-manager structure so the shared-layout reconciler can rebuild it from scratch
/// as pure additions. Used only on attach to a session that already carries an authoritative
/// layout (the client's throwaway local panes are discarded in favor of the server's).
fn reset_state_for_shared_seed(state: &mut State) {
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
fn bind_attached_pane_backends(
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
            pane.terminal.cwd = meta.cwd;
            pane.terminal.child_pid = meta.pid;
            pane.terminal.status = ManagedTerminalStatus::Ready;
        }
        ctx.state.next_pane_id = ctx.state.next_pane_id.max(meta.pane_id.saturating_add(1));
        ctx.state.next_pty_generation = ctx
            .state
            .next_pty_generation
            .max(meta.generation.saturating_add(1));
    }
}

/// Defensive fallback: adopt server panes when a live session reports panes but no committed layout
/// (should not happen under protocol v3). Rebuilds a flat tiled workspace from the pane list.
fn apply_attached_panes(
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
            pane.terminal.cwd = attached.cwd;
            pane.terminal.child_pid = attached.pid;
            pane.terminal.status = ManagedTerminalStatus::Ready;
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

/// Spawn every non-closing pane the client holds in state on a freshly attached (empty) session.
/// Used on initial attach and after detach when the new ephemeral server owns no panes yet.
/// Spawn the panes the client already holds in state onto the freshly attached session, returning
/// their `(pane_id, generation)` so the caller can schedule the open/activate reveal timers (these
/// panes start with `opening = true` and would otherwise stay invisible).
fn spawn_state_panes_on_session(ctx: &mut Context<HyprmuxApp>) -> Vec<(crate::state::PaneId, u64)> {
    let Some(client) = ctx.state.session_client.clone() else {
        return Vec::new();
    };
    // Fallback palette for any pane whose screen never cached one, so the server seeds a theme
    // palette before the PTY spawns and the child's startup color queries are answered correctly.
    let fallback_palette = crate::theme_ops::terminal_palette(
        &ctx.state.theme,
        crate::theme_ops::pane_frame_background(
            &ctx.state.theme,
            false,
            ctx.state.config.pane.highlight_focused_background,
        ),
    );
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
        client.spawn_pane(
            pane.id,
            generation,
            pane.identity.command.clone(),
            pane.identity.cwd.clone(),
            pane.terminal.cols,
            pane.terminal.rows,
            pane.identity.keep_open,
            pane_env(ctx.state.control_socket_path.as_deref(), pane),
            pane.identity.custom_title.clone(),
            pane.terminal.last_palette.unwrap_or(fallback_palette),
        );
        targets.push((pane.id, generation));
    }
    targets
}

/// Flush pane spawns that were queued while no client was connected (see
/// [`crate::state::State::pending_spawns`]).
fn flush_pending_spawns(ctx: &mut Context<HyprmuxApp>) {
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
        );
    }
}

#[cfg(test)]
mod padding_input_tests {
    use super::*;

    #[test]
    fn padding_input_accepts_empty_or_one_ascii_digit_in_range() {
        assert!(valid_padding_text(""));
        assert!(valid_padding_text("8"));
        assert!(!valid_padding_text("9"));
        assert!(!valid_padding_text("12"));
        assert!(!valid_padding_text("８"));
    }
}
