use tui_lipan::prelude::*;

use crate::actions::execute_action;
use crate::anim::GeometryAnimation;
use crate::focus_ops::{
    focus_pane, request_current_pane_focus, request_pane_focus, request_search_focus,
};
use crate::input::Action;
use crate::key_routing::handle_key_routing;
use crate::pane_lifecycle::{find_pane_mut, handle_prune_closed};
use crate::pty_events::{
    error_toast, handle_pane_input, handle_pane_mouse, handle_pane_resize, handle_pane_scroll,
    handle_pty_event, handle_pty_ready,
};
use crate::resize_move_ops::{begin_move, begin_resize, end_move, move_pane, resize_pane};
use crate::search_ops::{recompute_search, search_next};
use crate::state::ThemePreset;
use crate::theme_ops::{select_theme, theme_tick};
use crate::{FrameworkFocus, HyprmuxApp, Msg};

pub(crate) fn handle_msg(_app: &mut HyprmuxApp, msg: Msg, ctx: &mut Context<HyprmuxApp>) -> Update {
    match msg {
        Msg::RunAction(action) => {
            ctx.state.show_palette = false;
            let update = execute_action(ctx, action);
            match action {
                Action::OpenSearch => request_search_focus(ctx),
                Action::OpenThemePicker => {}
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
            ctx.state.show_theme_picker = false;
            request_current_pane_focus(ctx);
            Update::full()
        }
        Msg::ThemePickerSelected(index) => {
            ctx.state.theme_picker_selected = index;
            Update::full()
        }
        Msg::ThemePickerActivated(index) => {
            if let Some(preset) = ThemePreset::all().get(index).copied() {
                select_theme(ctx, preset);
                request_current_pane_focus(ctx);
            }
            Update::full()
        }
        Msg::ThemeTick => theme_tick(ctx),
        Msg::ThemeError(message) => {
            ctx.toast().push(error_toast("Theme Reload", message));
            Update::full()
        }
        Msg::CloseSearch => {
            ctx.state.search = None;
            request_current_pane_focus(ctx);
            Update::full()
        }
        Msg::SearchChanged(event) => {
            if let Some(search) = ctx.state.search.as_mut() {
                event.apply_to(&mut search.input);
            }
            recompute_search(ctx)
        }
        Msg::SearchNext(backward) => search_next(ctx, backward),
        Msg::FocusPane(id, framework_focus) => {
            focus_pane(&mut ctx.state, id);
            if framework_focus == FrameworkFocus::Request {
                request_pane_focus(ctx, id);
            }
            Update::full()
        }
        Msg::HoverPane(id) => {
            if ctx.state.focused_pane != Some(id) {
                focus_pane(&mut ctx.state, id);
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
        Msg::FinishOpen(id) => {
            if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
                pane.opening = false;
                if !pane.closing {
                    ctx.state.animation = GeometryAnimation::Spawn;
                }
            }
            Update::full()
        }
        Msg::PruneClosed(id) => handle_prune_closed(ctx, id),
        Msg::PtyReady(id, pty) => handle_pty_ready(ctx, id, pty),
        Msg::PtyEvent(id, event) => handle_pty_event(ctx, id, event),
        Msg::PaneInput(id, input) => handle_pane_input(ctx, id, input),
        Msg::PaneKey(id, key) => {
            focus_pane(&mut ctx.state, id);
            let (_handled, update) = handle_key_routing(ctx, key, Some(id));
            update
        }
        Msg::PaneMouse(id, bytes) => handle_pane_mouse(ctx, id, bytes),
        Msg::PaneResize(id, cols, rows) => handle_pane_resize(ctx, id, cols, rows),
        Msg::PaneScroll(id, offset) => handle_pane_scroll(ctx, id, offset),
    }
}
