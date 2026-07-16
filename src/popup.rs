use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::geometry::workspace_tile_bounds;
use crate::ops::focus::{request_current_pane_focus, request_pane_focus};
use crate::ops::theme::{pane_frame_background, terminal_palette};
use crate::pane_lifecycle::{pane_env, request_pane_spawn};
use crate::state::{POPUP_PANE_ID, Pane, PaneIdentity};

pub(crate) fn popup_rect(bounds: FloatRect, top_gap: f32, width: f32, height: f32) -> FloatRect {
    let bounds = workspace_tile_bounds(bounds, top_gap);
    let width = width.clamp(0.2, 0.95);
    let height = height.clamp(0.2, 0.95);
    let w = (bounds.w * width).round().max(1.0);
    let h = (bounds.h * height).round().max(1.0);
    FloatRect {
        x: bounds.x + (bounds.w - w) / 2.0,
        y: bounds.y + (bounds.h - h) / 2.0,
        w,
        h,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn open(
    ctx: &mut Context<HyprmuxApp>,
    command: String,
    cwd: Option<String>,
    width: Option<f32>,
    height: Option<f32>,
    title: Option<String>,
) -> std::result::Result<Update, String> {
    if ctx.state.popup.is_some() {
        return Err("a popup is already open".to_string());
    }
    if command.trim().is_empty() {
        return Err("popup command must not be empty".to_string());
    }
    let rect = popup_rect(
        ctx.state
            .canvas_bounds_from_terminal_viewport(ctx.viewport()),
        ctx.state.workspace_top_gap(),
        width.unwrap_or(0.6),
        height.unwrap_or(0.6),
    );
    let generation = ctx.state.next_pty_generation;
    ctx.state.next_pty_generation = generation.saturating_add(1);
    let mut pane = Pane::new(POPUP_PANE_ID, ctx.state.config.scrollback, rect);
    pane.pty_generation = generation;
    pane.identity = PaneIdentity {
        command: Some(command),
        cwd,
        keep_open: false,
        custom_title: title,
        ..PaneIdentity::default()
    };
    pane.terminal.bind_server_backend(POPUP_PANE_ID, generation);
    let palette = terminal_palette(
        &ctx.state.theme,
        pane_frame_background(
            &ctx.state.theme,
            true,
            ctx.state.config.pane.highlight_focused_background,
        ),
    );
    pane.terminal.set_palette(palette);
    pane.opening = false;
    pane.terminal_active = true;
    let env = pane_env(ctx.state.control_socket_path.as_deref(), &pane);
    let identity = pane.identity.clone();
    let (cols, rows) = (pane.terminal.cols, pane.terminal.rows);
    ctx.state.popup_return_focus = ctx.state.focused_pane;
    ctx.state.popup = Some(pane);
    request_pane_spawn(
        &mut ctx.state,
        POPUP_PANE_ID,
        generation,
        identity.command,
        identity.cwd,
        cols,
        rows,
        false,
        env,
        identity.custom_title,
        palette,
        false,
    );
    request_pane_focus(ctx, POPUP_PANE_ID);
    Ok(Update::full())
}

pub(crate) fn close(ctx: &mut Context<HyprmuxApp>) -> Update {
    if let Some(pane) = ctx.state.popup.take()
        && let Some(client) = ctx.state.session_client.clone()
    {
        client.kill(POPUP_PANE_ID, pane.pty_generation);
    }
    restore_focus(ctx)
}

pub(crate) fn handle_exit(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.popup = None;
    restore_focus(ctx)
}

/// Tear down an open popup before detaching or leaving the session. The popup pane is
/// client-local, so nothing else would ever kill its server-side PTY, and its reserved id must
/// not linger in the server pane map across a reattach.
pub(crate) fn kill_if_open(ctx: &mut Context<HyprmuxApp>) {
    if let Some(pane) = ctx.state.popup.take() {
        ctx.state.popup_return_focus = None;
        if let Some(client) = ctx.state.session_client.clone() {
            client.kill(POPUP_PANE_ID, pane.pty_generation);
        }
    }
}

fn restore_focus(ctx: &mut Context<HyprmuxApp>) -> Update {
    if let Some(previous) = ctx.state.popup_return_focus.take() {
        crate::ops::focus::focus_pane(&mut ctx.state, previous);
        request_pane_focus(ctx, previous);
    } else {
        request_current_pane_focus(ctx);
    }
    Update::full()
}

pub(crate) fn placement(
    app: &HyprmuxApp,
    ctx: &Context<HyprmuxApp>,
) -> Option<(FloatRect, Element)> {
    let pane = ctx.state.popup.as_ref()?;
    let rect = pane.floating_rect;
    Some((
        rect,
        crate::view::pane_element(
            app,
            ctx,
            pane,
            rect,
            Some(POPUP_PANE_ID),
            "P",
            crate::view::PaneMerge::default(),
        ),
    ))
}

pub(crate) fn backdrop(ctx: &Context<HyprmuxApp>) -> Option<(FloatRect, Element)> {
    ctx.state.popup.as_ref()?;
    let region: Element = MouseRegion::new()
        .capture_click(true)
        .on_mouse_down(ctx.link().callback(|_| crate::Msg::ClosePopup))
        .child(Text::new("").width(Length::Flex(1)).height(Length::Flex(1)))
        .into();
    Some((
        ctx.state
            .canvas_bounds_from_terminal_viewport(ctx.viewport()),
        region.key("hyprmux-popup-scrim"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn popup_rect_is_centered_and_clamped() {
        let rect = popup_rect(
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 40.0,
            },
            0.0,
            0.6,
            0.5,
        );
        assert_eq!((rect.w, rect.h), (60.0, 20.0));
        assert_eq!((rect.x, rect.y), (20.0, 10.0));
        assert!(
            popup_rect(
                FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 100.0,
                    h: 40.0
                },
                0.0,
                2.0,
                2.0
            )
            .w <= 95.0
        );
    }
}
