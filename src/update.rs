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
use crate::pane_lifecycle::{find_pane_mut, handle_prune_closed};
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
use crate::theme_ops::{cancel_theme_picker, preview_theme, theme_tick};
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
                Action::SaveProfile | Action::OpenProfilePicker => {}
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
        Msg::PreviewTheme(preset) => preview_theme(ctx, preset),
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
            ctx.toast().push(error_toast("Theme Reload", message));
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
        Msg::FinishOpen(id, generation) => {
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
        Msg::PruneClosed(id, generation) => handle_prune_closed(ctx, id, generation),
        Msg::PtyReady(id, generation, pty) => handle_pty_ready(ctx, id, generation, pty),
        Msg::PtyEvent(id, generation, event) => handle_pty_event(ctx, id, generation, event),
        Msg::PaneInput(id, input) => handle_pane_input(ctx, id, input),
        Msg::PaneKey(id, key) => {
            focus_pane(&mut ctx.state, id);
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
    };

    if crate::theme_ops::apply_terminal_palette_to_state(&mut ctx.state) {
        let command = update.command.take();
        update = Update::with_command(command);
    }

    update
}
