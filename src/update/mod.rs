mod attach;
pub(crate) use attach::spawn_state_panes_on_session;
mod overlays;
mod panes;
mod prompts;
mod session;

use tui_lipan::prelude::*;

use crate::{HyprmuxApp, Msg};

const LAYOUT_COMMIT_DEBOUNCE_MS: u64 = 16;

pub(crate) fn handle_msg(_app: &mut HyprmuxApp, msg: Msg, ctx: &mut Context<HyprmuxApp>) -> Update {
    let is_layout_flush = matches!(&msg, Msg::FlushLayoutCommit { .. });
    let mut update = match msg {
        Msg::ClosePopup => panes::close_popup(ctx),
        Msg::CommandLinkReady(link) => overlays::command_link_ready(ctx, link),
        Msg::Hangup => overlays::hangup(ctx),
        Msg::RunAction(action) => overlays::run_action(ctx, action),
        Msg::ClosePalette => overlays::close_palette(ctx),
        Msg::CloseHelp => overlays::close_help(ctx),
        Msg::CloseAppearance => overlays::close_appearance(ctx),
        Msg::AppearanceActivate(action) => overlays::appearance_activate(ctx, action),
        Msg::ClosePanePaddingEditor => overlays::close_pane_padding_editor(ctx),
        Msg::PanePaddingVerticalChanged(event) => {
            overlays::pane_padding_vertical_changed(ctx, event)
        }
        Msg::PanePaddingHorizontalChanged(event) => {
            overlays::pane_padding_horizontal_changed(ctx, event)
        }
        Msg::AdvancePanePadding => overlays::advance_pane_padding(ctx),
        Msg::SubmitPanePadding => overlays::submit_pane_padding(ctx),
        Msg::CloseThemePicker => overlays::close_theme_picker(ctx),
        Msg::PreviewTheme(index) => overlays::preview_theme(ctx, index),
        Msg::SelectTheme(index) => overlays::select_theme(ctx, index),
        Msg::ThemeTick => overlays::theme_tick(ctx),
        Msg::ConfigFileChanged => overlays::config_file_changed(ctx),
        Msg::WorkbarTick => overlays::workbar_tick(ctx),
        Msg::WorkbarCommandOutput(command, output) => {
            overlays::workbar_command_output(ctx, command, output)
        }
        Msg::ThemeError(message) => overlays::theme_error(ctx, message),
        Msg::CloseSearch => prompts::close_search(ctx),
        Msg::SearchQueryChanged(query) => prompts::search_query_changed(ctx, query),
        Msg::SearchNext(backward) => prompts::search_next(ctx, backward),
        Msg::SearchSelect(index) => prompts::search_select(ctx, index),
        Msg::SearchActivate(index) => prompts::search_activate(ctx, index),
        Msg::SearchCycleScope => prompts::search_cycle_scope(ctx),
        Msg::CloseRenamePane => prompts::close_rename_pane(ctx),
        Msg::RenamePaneChanged(event) => prompts::rename_pane_changed(ctx, event),
        Msg::SubmitRenamePane => prompts::submit_rename_pane(ctx),
        Msg::CloseRenameSession => prompts::close_rename_session(ctx),
        Msg::RenameSessionChanged(event) => prompts::rename_session_changed(ctx, event),
        Msg::SubmitRenameSession => prompts::submit_rename_session(ctx),
        Msg::CloseSaveProfile => prompts::close_save_profile(ctx),
        Msg::SaveProfileNameChanged(event) => prompts::save_profile_name_changed(ctx, event),
        Msg::SubmitSaveProfile => prompts::submit_save_profile(ctx),
        Msg::CloseProfilePicker => prompts::close_profile_picker(ctx),
        Msg::ProfilePickerQueryChanged(query) => prompts::profile_picker_query_changed(ctx, query),
        Msg::ProfilePickerSelect(index) => prompts::profile_picker_select(ctx, index),
        Msg::ProfilePickerSetDefault => prompts::profile_picker_set_default(ctx),
        Msg::ProfilePickerDelete => prompts::profile_picker_delete(ctx),
        Msg::ProfilePickerApply => prompts::profile_picker_apply(ctx),
        Msg::ProfilePickerOpenAs => prompts::profile_picker_open_as(ctx),
        Msg::SelectProfile(index) => prompts::select_profile(ctx, index),
        Msg::ProfileSessionsDiscovered { epoch, rows } => {
            prompts::profile_sessions_discovered(ctx, epoch, rows)
        }
        Msg::CloseSessionPicker => prompts::close_session_picker(ctx),
        Msg::SessionsDiscovered { epoch, rows } => prompts::sessions_discovered(ctx, epoch, rows),
        Msg::SessionPickerQueryChanged(query) => prompts::session_picker_query_changed(ctx, query),
        Msg::SessionPickerSelect(index) => prompts::session_picker_select(ctx, index),
        Msg::SessionPickerActivate(index) => prompts::session_picker_activate(ctx, index),
        Msg::SessionPickerCreateFromQuery => prompts::session_picker_create_from_query(ctx),
        Msg::SessionPickerDetachCurrent => prompts::session_picker_detach_current(ctx),
        Msg::SessionPickerKillSelected => prompts::session_picker_kill_selected(ctx),
        Msg::SessionPickerNameCurrent => prompts::session_picker_name_current(ctx),
        Msg::CloseClientList => prompts::close_client_list(ctx),
        Msg::ClientListSelect(index) => prompts::client_list_select(ctx, index),
        Msg::ClientListGrant(index) => prompts::client_list_grant(ctx, index),
        Msg::ClientListDecline(index) => prompts::client_list_decline(ctx, index),
        Msg::FocusPane(id) => panes::focus_pane(ctx, id),
        Msg::HoverPane(id) => panes::hover_pane(ctx, id),
        Msg::BeginMove(id, rect, x, y, width, height, modified) => {
            panes::begin_move(ctx, id, rect, x, y, width, height, modified)
        }
        Msg::MovePane(id, dx, dy, modified) => panes::move_pane(ctx, id, dx, dy, modified),
        Msg::EndMove(id, x, y) => panes::end_move(ctx, id, x, y),
        Msg::BeginResize(id, corner, x, y, modified) => {
            panes::begin_resize(ctx, id, corner, x, y, modified)
        }
        Msg::ResizePane(id, corner, from_x, from_y, x, y, modified) => {
            panes::resize_pane(ctx, id, corner, from_x, from_y, x, y, modified)
        }
        Msg::EndResize(id) => panes::end_resize(ctx, id),
        Msg::BeginResizeSplit(id, horizontal, x, y) => {
            panes::begin_resize_split(ctx, id, horizontal, x, y)
        }
        Msg::ResizeSplit(id, horizontal, from_x, from_y, x, y) => {
            panes::resize_split(ctx, id, horizontal, from_x, from_y, x, y)
        }
        Msg::BeginResizeSplitJunction(x, y) => panes::begin_resize_split_junction(ctx, x, y),
        Msg::ResizeSplitJunction(horizontal, vertical, from_x, from_y, x, y) => {
            panes::resize_split_junction(ctx, horizontal, vertical, from_x, from_y, x, y)
        }
        Msg::EndResizeSplit => panes::end_resize_split(ctx),
        Msg::BeginScratchResize(from_y) => panes::begin_scratch_resize(ctx, from_y),
        Msg::ScratchResize(from_y, y) => panes::scratch_resize(ctx, from_y, y),
        Msg::EndScratchResize => panes::end_scratch_resize(ctx),
        Msg::FinishOpen(epoch, id, generation) => panes::finish_open(ctx, epoch, id, generation),
        Msg::ActivatePane(epoch, id, generation) => {
            panes::activate_pane(ctx, epoch, id, generation)
        }
        Msg::PruneClosed(epoch, id, generation) => panes::prune_closed(ctx, epoch, id, generation),
        Msg::PaneInput(id, input) => panes::pane_input(ctx, id, input),
        Msg::CopyFlashExpired(id, flash_id) => panes::copy_flash_expired(ctx, id, flash_id),
        Msg::PaneKey(id, key) => panes::pane_key(ctx, id, key),
        Msg::PaneMouse(id, bytes) => panes::pane_mouse(ctx, id, bytes),
        Msg::PaneResize(id, cols, rows) => panes::pane_resize(ctx, id, cols, rows),
        Msg::PaneScroll(id, offset) => panes::pane_scroll(ctx, id, offset),
        Msg::ControlRequest(envelope) => panes::control_request(ctx, envelope),
        Msg::SessionConnected {
            epoch,
            name,
            client,
        } => session::connected(ctx, epoch, name, client),
        Msg::SessionDisconnected { epoch, name } => session::disconnected(ctx, epoch, name),
        Msg::SessionAttachFailed { epoch, message } => session::attach_failed(ctx, epoch, message),
        Msg::SessionAttached {
            epoch,
            session: name,
            client_id,
            panes,
            layout_rev,
            layout,
            controller,
            clients,
            input_locked,
            read_only,
            created_from_profile,
        } => session::attached(
            ctx,
            epoch,
            name,
            client_id,
            panes,
            layout_rev,
            layout,
            controller,
            clients,
            input_locked,
            read_only,
            created_from_profile,
        ),
        Msg::SessionOriginSet {
            epoch,
            created_from_profile,
        } => session::origin_set(ctx, epoch, created_from_profile),
        Msg::SessionLayoutCommitted {
            epoch,
            rev,
            author,
            layout,
        } => session::layout_committed(ctx, epoch, rev, author, layout),
        Msg::SessionLayoutRejected {
            epoch,
            current_rev,
            layout,
        } => session::layout_rejected(ctx, epoch, current_rev, layout),
        Msg::SessionControllerChanged {
            epoch,
            controller,
            reason,
        } => session::controller_changed(ctx, epoch, controller, reason),
        Msg::SessionClientsChanged {
            epoch,
            clients,
            input_locked,
        } => session::clients_changed(ctx, epoch, clients, input_locked),
        Msg::SessionControlRequested { epoch, from } => {
            session::control_requested(ctx, epoch, from)
        }
        Msg::SessionControlDeclined { epoch } => session::control_declined(ctx, epoch),
        Msg::SessionPing { epoch, seq } => session::ping(ctx, epoch, seq),
        Msg::FlushPaneResizes { epoch } => session::flush_pane_resizes(ctx, epoch),
        Msg::FlushLayoutCommit { epoch } => session::flush_layout_commit(ctx, epoch),
        Msg::SessionOutput {
            epoch,
            pane_id,
            generation,
            bytes,
        } => session::output(ctx, epoch, pane_id, generation, bytes),
        Msg::SessionResized {
            epoch,
            pane_id,
            generation,
            cols,
            rows,
        } => session::resized(ctx, epoch, pane_id, generation, cols, rows),
        Msg::SessionExited {
            epoch,
            pane_id,
            generation,
            code,
        } => session::exited(ctx, epoch, pane_id, generation, code),
        Msg::SessionPaneLoggingChanged {
            epoch,
            pane_id,
            generation,
            enabled,
            path,
            error,
        } => session::pane_logging_changed(ctx, epoch, pane_id, generation, enabled, path, error),
        Msg::SessionPaneRuntimeChanged {
            epoch,
            pane_id,
            generation,
            state,
        } => session::pane_runtime_changed(ctx, epoch, pane_id, generation, state),
        Msg::SessionSpawnResult {
            epoch,
            pane_id,
            generation,
            pid,
            ok,
            error,
        } => session::spawn_result(ctx, epoch, pane_id, generation, pid, ok, error),
        Msg::SessionError { epoch, message } => session::error(ctx, epoch, message),
        Msg::SessionRenamed {
            epoch,
            session: name,
        } => session::renamed(ctx, epoch, name),
    };

    if crate::ops::theme::apply_terminal_palette_to_state(&mut ctx.state) {
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
