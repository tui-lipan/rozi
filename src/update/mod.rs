mod attach;
pub(crate) use attach::spawn_state_panes_on_session;
mod overlays;
mod panes;
mod prompts;
mod session;
pub(crate) mod sidebar;
pub(crate) mod workbar;

use tui_lipan::prelude::*;

use crate::{AppRoot, Msg};

const LAYOUT_COMMIT_DEBOUNCE_MS: u64 = 16;

pub(crate) fn handle_msg(_app: &mut AppRoot, msg: Msg, ctx: &mut Context<AppRoot>) -> Update {
    let is_layout_flush = matches!(&msg, Msg::FlushLayoutCommit { .. });
    let mut update = match msg {
        Msg::ClosePopup => panes::close_popup(ctx),
        Msg::CommandLinkReady(link) => overlays::command_link_ready(ctx, link),
        Msg::Hangup => overlays::hangup(ctx),
        Msg::RunAction(action) => overlays::run_action(ctx, action),
        Msg::ClosePalette => overlays::close_palette(ctx),
        Msg::CommandPaletteQueryChanged(query) => {
            overlays::command_palette_query_changed(ctx, query)
        }
        Msg::CloseHelp => overlays::close_help(ctx),
        Msg::CloseSettings => overlays::close_settings(ctx),
        Msg::SettingsSelect(action) => overlays::settings_select(ctx, action),
        Msg::SettingsActivate(action) => overlays::settings_activate(ctx, action),
        Msg::SettingsStep { reverse } => overlays::settings_step(ctx, reverse),
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
        Msg::AgentTick => sidebar::agent_tick(ctx),
        Msg::AlertPulseTick => panes::alert_pulse_tick(ctx),
        Msg::WorkbarCommandPoll { epoch, command } => workbar::poll_command(ctx, epoch, command),
        Msg::WorkbarCommandOutput {
            epoch,
            command,
            output,
        } => workbar::command_output(ctx, epoch, command, output),
        Msg::SidebarTabSelected { panel, index } => sidebar::tab_selected(ctx, panel, index),
        Msg::SidebarTabReordered { panel, event } => sidebar::tab_reordered(ctx, panel, event),
        Msg::SidebarTabTransferred(event) => sidebar::tab_transferred(ctx, event),
        Msg::SidebarPanelsResized(event) => sidebar::panels_resized(ctx, event),
        Msg::SidebarViewportChanged {
            panel,
            tab_id,
            event,
        } => sidebar::viewport_changed(ctx, panel, tab_id, event),
        Msg::SidebarWidthResizing(event) => sidebar::width_resizing(ctx, event),
        Msg::SidebarWidthResized(event) => sidebar::width_resized(ctx, event),
        Msg::SidebarPointerMoved(panel) => sidebar::pointer_moved(ctx, panel),
        Msg::SidebarRowActivate { panel, index } => sidebar::row_activate(ctx, panel, index),
        Msg::SidebarRowHover {
            panel,
            index,
            hovered,
        } => sidebar::row_hover(ctx, panel, index, hovered),
        Msg::SidebarRowClose { panel, index } => sidebar::row_close(ctx, panel, index),
        Msg::ConfirmationExpired(epoch) => crate::ops::confirm::expired(ctx, epoch),
        Msg::SidebarBlur => sidebar::blur_body(ctx),
        Msg::SidebarTreeFocused => sidebar::tree_focused(ctx),
        Msg::SidebarExplorerFocus(origin) => sidebar::explorer_focus(ctx, origin),
        Msg::SidebarCycleTab(forward) => sidebar::cycle_tab(ctx, forward),
        Msg::SidebarFocusPane(id) => sidebar::focus_pane(ctx, id),
        Msg::SidebarLauncherActivate {
            config_epoch,
            tab_id,
            entry_index,
        } => sidebar::launcher_activate(ctx, config_epoch, tab_id, entry_index),
        Msg::SidebarTreeActivate {
            config_epoch,
            tab_id,
            path,
            is_dir,
        } => sidebar::tree_activate(ctx, config_epoch, tab_id, path, is_dir),
        Msg::SidebarTreeEntryRequest { path } => sidebar::tree_entry_request(ctx, path),
        Msg::SessionDirectoryListing {
            epoch,
            path,
            entries,
            error,
        } => sidebar::tree_directory_listed(ctx, epoch, path, entries, error),
        Msg::SessionChangeListing {
            epoch,
            root,
            changes,
            error,
        } => sidebar::tree_changes_listed(ctx, epoch, root, changes, error),
        Msg::SidebarCommandPoll { epoch, tab_id } => sidebar::poll_command(ctx, epoch, tab_id),
        Msg::SidebarCommandOutput {
            epoch,
            tab_id,
            rows,
        } => sidebar::command_output(ctx, epoch, tab_id, rows),
        Msg::SidebarCommandRowActivate {
            config_epoch,
            tab_id,
            output_epoch,
            line,
        } => sidebar::command_row_activate(ctx, config_epoch, tab_id, output_epoch, line),
        Msg::SidebarSessionsRefresh { epoch } => sidebar::refresh_sessions(ctx, epoch),
        Msg::SidebarSessionsDiscovered {
            epoch,
            rows,
            host_status,
        } => sidebar::sessions_discovered(ctx, epoch, rows, host_status),
        Msg::SidebarSessionActivate(entry) => sidebar::activate_session(ctx, entry),
        Msg::ThemeError(message) => overlays::theme_error(ctx, message),
        Msg::CloseSearch => prompts::close_search(ctx),
        Msg::SearchQueryChanged(query) => prompts::search_query_changed(ctx, query),
        Msg::SearchScanChunk { epoch } => crate::ops::search::search_scan_chunk(ctx, epoch),
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
        Msg::ProfilePickerNew => prompts::profile_picker_new(ctx),
        Msg::SelectProfile(index) => prompts::select_profile(ctx, index),
        Msg::CloseLayoutPicker => crate::ops::layout_picker::cancel_layout_picker(ctx),
        Msg::LayoutPickerSelect(index) => {
            crate::ops::layout_picker::layout_picker_selection_changed(ctx, index)
        }
        Msg::SelectLayout(index) => crate::ops::layout_picker::select_layout(ctx, index),
        Msg::LayoutPickerSetDefault => crate::ops::layout_picker::layout_picker_set_default(ctx),
        Msg::ProfileSessionsDiscovered { epoch, rows } => {
            prompts::profile_sessions_discovered(ctx, epoch, rows)
        }
        Msg::CloseSessionPicker => prompts::close_session_picker(ctx),
        Msg::SessionsDiscovered {
            epoch,
            rows,
            host_status,
        } => prompts::sessions_discovered(ctx, epoch, rows, host_status),
        Msg::SessionPickerQueryChanged(query) => prompts::session_picker_query_changed(ctx, query),
        Msg::SessionPickerSelect(index) => prompts::session_picker_select(ctx, index),
        Msg::SessionPickerActivate(index) => prompts::session_picker_activate(ctx, index),
        Msg::SessionPickerEphemeral => crate::ops::session::open_ephemeral_session(ctx),
        Msg::SessionPickerCreateFromQuery => prompts::session_picker_create_from_query(ctx),
        Msg::SessionPickerKillSelected => prompts::session_picker_kill_selected(ctx),
        Msg::SessionPickerRestartSelected => prompts::session_picker_restart_selected(ctx),
        Msg::SessionPickerDisconnectAttachment => {
            prompts::session_picker_disconnect_attachment(ctx)
        }
        Msg::SessionPickerDisconnectHost => prompts::session_picker_disconnect_host(ctx),
        Msg::SessionPickerConnectHost => prompts::session_picker_connect_host(ctx),
        Msg::SessionPickerNameCurrent => prompts::session_picker_name_current(ctx),
        Msg::CloseCollaboration => prompts::close_collaboration(ctx),
        Msg::CollaborationQueryChanged(query) => prompts::collaboration_query_changed(ctx, query),
        Msg::CollaborationSelect(index) => prompts::collaboration_select(ctx, index),
        Msg::CollaborationGrant(index) => prompts::collaboration_grant(ctx, index),
        Msg::CollaborationDecline(index) => prompts::collaboration_decline(ctx, index),
        Msg::CollaborationKick(index) => prompts::collaboration_kick(ctx, index),
        Msg::FollowPromptSelect(index) => prompts::follow_prompt_select(ctx, index),
        Msg::FollowPromptChoose(index) => prompts::follow_prompt_choose(ctx, index),
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
        Msg::BeginResizeSplitJunction(horizontal, vertical, x, y) => {
            panes::begin_resize_split_junction(ctx, horizontal, vertical, x, y)
        }
        Msg::ResizeSplitJunction(horizontal, vertical, from_x, from_y, x, y) => {
            panes::resize_split_junction(ctx, horizontal, vertical, from_x, from_y, x, y)
        }
        Msg::EndResizeSplit => panes::end_resize_split(ctx),
        Msg::BeginScratchResize(from_y) => panes::begin_scratch_resize(ctx, from_y),
        Msg::ScratchResize(from_y, y) => panes::scratch_resize(ctx, from_y, y),
        Msg::EndScratchResize => panes::end_scratch_resize(ctx),
        Msg::FinishOpen(epoch, id, generation) => panes::finish_open(ctx, epoch, id, generation),
        Msg::PruneClosed(epoch, id, generation) => {
            crate::pane_lifecycle::prune_closed_pane(ctx, epoch, id, generation)
        }
        Msg::ActivatePane(epoch, id, generation) => {
            panes::activate_pane(ctx, epoch, id, generation)
        }
        Msg::CopyFeedbackExpired(attachment, id, epoch) => {
            panes::copy_feedback_expired(ctx, attachment, id, epoch)
        }
        Msg::PaneInput(id, input) => panes::pane_input(ctx, id, input),
        Msg::PaneKey(id, key) => panes::pane_key(ctx, id, key),
        Msg::ForwardPrefix(key) => panes::forward_prefix(ctx, key),
        Msg::PaneMouse(id, bytes) => panes::pane_mouse(ctx, id, bytes),
        Msg::PaneResize(id, cols, rows) => panes::pane_resize(ctx, id, cols, rows),
        Msg::PaneScroll(id, offset) => panes::pane_scroll(ctx, id, offset),
        Msg::ControlRequest(envelope) => panes::control_request(ctx, envelope),
        Msg::AgentSlotStreamOpen { pane_id, sender } => {
            crate::ops::agent_slots::stream_opened(ctx, pane_id, sender)
        }
        Msg::AgentSlotsReported { pane_id, slots } => {
            crate::ops::agent_slots::slots_reported(ctx, pane_id, slots)
        }
        Msg::AgentSlotStreamClosed { pane_id } => {
            crate::ops::agent_slots::stream_closed(ctx, pane_id)
        }
        Msg::SessionConnected {
            epoch,
            name,
            client,
        } => session::connected(ctx, epoch, name, client),
        Msg::SessionDisconnected { epoch, name } => session::disconnected(ctx, epoch, name),
        Msg::DrainSessionFrames { epoch, mailbox } => {
            let event = mailbox.pop();
            mailbox.finish_drain();
            match event {
                Some(crate::session::client::InboundEvent::Frame(frame)) => {
                    return handle_msg(
                        _app,
                        crate::session::bootstrap::server_message_to_msg(epoch, *frame),
                        ctx,
                    );
                }
                Some(crate::session::client::InboundEvent::Disconnected) => {
                    session::disconnected(ctx, epoch, mailbox.session_name())
                }
                Some(crate::session::client::InboundEvent::Failed(message)) => {
                    session::transport_failed(ctx, epoch, mailbox.session_name(), message)
                }
                None => Update::none(),
            }
        }
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
            allow_takeover,
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
            allow_takeover,
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
            allow_takeover,
        } => session::clients_changed(ctx, epoch, clients, input_locked, allow_takeover),
        Msg::SessionControlRequested { epoch, from } => {
            session::control_requested(ctx, epoch, from)
        }
        Msg::SessionControlDeclined { epoch } => session::control_declined(ctx, epoch),
        Msg::SessionPing { epoch, seq } => session::ping(ctx, epoch, seq),
        Msg::SessionRuntimeMetrics { epoch, metrics: _ } => {
            runtime_metrics_update(epoch, ctx.state.runtime_epoch, ctx.devtools_visible())
        }
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
        Msg::ReplayInputDeadline {
            epoch,
            pane_id,
            generation,
        } => session::replay_input_deadline(ctx, epoch, pane_id, generation),
        Msg::SpawnReplyDeadline {
            epoch,
            pane_id,
            generation,
        } => session::spawn_reply_deadline(ctx, epoch, pane_id, generation),
        Msg::SessionError { epoch, message } => session::error(ctx, epoch, message),
        Msg::SessionEvicted { epoch, message } => session::evicted(ctx, epoch, message),
        Msg::SessionRenamed {
            epoch,
            session: name,
        } => session::renamed(ctx, epoch, name),
    };

    ctx.state.current_mut().retired_panes.expire();
    for attachment in ctx.state.background.values_mut() {
        attachment.retired_panes.expire();
    }
    let stale_search = ctx.state.search.as_ref().is_some_and(|search| {
        crate::pane_lifecycle::find_pane(&ctx.state, search.target).is_none()
    });
    if stale_search {
        ctx.state.search = None;
        ctx.state.commands_dirty = true;
    }
    if stale_search {
        let command = update.command.take();
        update = Update::with_command(command);
    }
    acknowledge_attended_pane(ctx);
    panes::arm_alert_pulse(ctx);
    sidebar::sync_tree_roots(ctx);
    // Keep the Sessions tab's auto-refresh loop alive across session switches, creates, and reopens,
    // which bump the sessions epoch and would otherwise leave the tab frozen until it is reopened.
    sidebar::ensure_sessions_refresh_armed(ctx);

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

fn runtime_metrics_update(epoch: u64, current_epoch: u64, devtools_visible: bool) -> Update {
    if epoch == current_epoch && devtools_visible {
        Update::full()
    } else {
        Update::none()
    }
}

/// Attention chokepoint after every message: acknowledge the current pane only while the host window
/// and pane are both focused. This keeps unseen output, BEL, and finished-run attention on one rule
/// across clicks, keyboard focus, workspace switches, and layout reconciliation.
fn acknowledge_attended_pane(ctx: &mut Context<AppRoot>) {
    let Some(focused) = ctx.state.current().focused_pane else {
        return;
    };
    crate::ops::focus::acknowledge_pane_if_attended(&mut ctx.state, focused);
}

pub(crate) fn schedule_layout_commit(ctx: &mut Context<AppRoot>) {
    if !ctx.state.current().session_attached || !ctx.state.is_controller() {
        return;
    }
    let epoch = ctx.state.runtime_epoch;
    let Some(shared) = ctx.state.current().shared.as_ref() else {
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
        .current_mut()
        .shared
        .as_mut()
        .expect("shared session checked above")
        .layout_commit_scheduled = true;
    // Re-armed after every message, so during sustained output this fires ~60 times a second.
    // `send_after` parks it on the shared timer thread instead of spawning one per window.
    link.send_after(
        std::time::Duration::from_millis(LAYOUT_COMMIT_DEBOUNCE_MS),
        Msg::FlushLayoutCommit { epoch },
    );
}

/// If this client controls a shared session and its layout differs from the last commit, publish a
/// new [`SharedLayout`] at the optimistic base revision. The canonical canvas is this controller's
/// own pane canvas (viewport minus workbar), which followers letterbox to.
pub(crate) fn flush_layout_commit(ctx: &mut Context<AppRoot>) {
    if !ctx.state.current().session_attached || !ctx.state.is_controller() {
        return;
    }
    let Some(client) = ctx.state.current().session_client.clone() else {
        return;
    };
    let bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());
    let canvas = (
        bounds.w.round().max(1.0) as u16,
        bounds.h.round().max(1.0) as u16,
    );
    let layout = crate::shared_layout::shared_layout_from_state(&ctx.state, canvas);
    let Some(shared) = ctx.state.current_mut().shared.as_mut() else {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tui_lipan::UpdateLevel;

    #[test]
    fn runtime_metrics_redraw_current_but_not_parked_visible_attachment() {
        let current_epoch = 7;
        let parked_epoch = 6;
        assert_eq!(
            runtime_metrics_update(current_epoch, current_epoch, true).level(),
            UpdateLevel::Full
        );
        assert_eq!(
            runtime_metrics_update(parked_epoch, current_epoch, true).level(),
            UpdateLevel::None
        );
        assert_eq!(
            runtime_metrics_update(current_epoch, current_epoch, false).level(),
            UpdateLevel::None
        );
    }
}
