use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::anim::GeometryAnimation;
use crate::geometry::{close_rect, workspace_tile_bounds};
use crate::ops::focus::{request_current_pane_focus, request_pane_focus};
use crate::ops::theme::pane_frame_background;
use crate::pane_lifecycle::{
    PaneSpawnRequest, focused_spawn_cwd, open_timers_command, pane_env, request_pane_spawn,
};
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
    keep_open: bool,
    env: Vec<(String, String)>,
) -> std::result::Result<Update, String> {
    if ctx.state.popup_is_present() {
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
    let generation = ctx.state.current().next_pty_generation;
    ctx.state.current_mut().next_pty_generation = generation.saturating_add(1);
    let mut pane = Pane::new(POPUP_PANE_ID, ctx.state.config.scrollback, rect);
    pane.pty_generation = generation;
    pane.identity = PaneIdentity {
        command: Some(command),
        // A popup runs where the user is looking, not where the server happens to live; an explicit
        // cwd from the control socket still wins.
        cwd: cwd.or_else(|| focused_spawn_cwd(&ctx.state)),
        keep_open,
        env,
        custom_title: title,
        ..PaneIdentity::default()
    };
    pane.terminal.bind_server_backend(POPUP_PANE_ID, generation);
    let palette = TerminalColorPalette::from_theme(
        &ctx.state.theme,
        pane_frame_background(
            &ctx.state.theme,
            true,
            ctx.state.config.pane.highlight_focused_background,
        ),
    );
    pane.terminal.set_palette(palette);
    pane.opening = true;
    let env = pane_env(
        ctx.state.control_socket_path.as_deref(),
        &pane,
        ctx.state.current().remote_host.is_some(),
    );
    let identity = pane.identity.clone();
    let (cols, rows) = (pane.terminal.cols, pane.terminal.rows);
    ctx.state.popup_return_focus = ctx.state.current().focused_pane;
    ctx.state.popup = Some(pane);
    ctx.state.animation = GeometryAnimation::Spawn;
    let open_delay = crate::anim::open_delay(ctx.state.config.animations);
    let activate_delay = crate::anim::activation_delay(ctx.state.config.animations);
    request_pane_spawn(
        &mut ctx.state,
        PaneSpawnRequest {
            pane_id: POPUP_PANE_ID,
            generation,
            identity,
            cols,
            rows,
            env,
            palette,
        },
    );
    Ok(Update::with_command(open_timers_command(
        ctx.state.runtime_epoch,
        POPUP_PANE_ID,
        generation,
        open_delay,
        activate_delay,
    )))
}

pub(crate) fn close(ctx: &mut Context<HyprmuxApp>) -> Update {
    let client = ctx.state.current().session_client.clone();
    let Some(pane) = ctx.state.popup.as_mut().filter(|pane| !pane.closing) else {
        return Update::none();
    };
    let generation = pane.pty_generation;
    if let Some(client) = client {
        client.kill(POPUP_PANE_ID, generation);
    }
    pane.opening = false;
    // Stay described so the popup scales out the way it scaled in; `prune_closed_pane` drops it.
    pane.closing = true;
    pane.terminal.kill();
    ctx.state.animation = crate::anim::GeometryAnimation::Close;
    restore_focus(ctx);
    Update::with_command(crate::pane_lifecycle::prune_closed_command(
        ctx.state.runtime_epoch,
        POPUP_PANE_ID,
        generation,
        crate::anim::retained_pane_timeout(ctx.state.config.animations),
    ))
}

pub(crate) fn handle_exit(ctx: &mut Context<HyprmuxApp>) -> Update {
    if let Some(pane) = ctx.state.popup.as_mut()
        && pane.identity.keep_open
    {
        return Update::full();
    }

    close(ctx)
}

pub(crate) fn dismisses_completed(key: KeyEvent) -> bool {
    key.is(KeyCode::Enter) || key.is(KeyCode::Esc) || key.is(KeyCode::Char(' '))
}

/// Tear down an open popup before detaching or leaving the session. The popup pane is
/// client-local, so nothing else would ever kill its server-side PTY, and its reserved id must
/// not linger in the server pane map across a reattach.
pub(crate) fn kill_if_open(ctx: &mut Context<HyprmuxApp>) {
    if let Some(pane) = ctx.state.popup.take() {
        ctx.state.popup_return_focus = None;
        if let Some(client) = ctx.state.current().session_client.clone() {
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
    let target = if pane.opening || pane.closing {
        close_rect(pane.floating_rect)
    } else {
        pane.floating_rect
    };
    let rect = ctx.transition(
        format!("hyprmux-pane-rect-{}", pane.id),
        target,
        app.transition_config_for(ctx, pane, false),
    );
    Some((
        rect,
        crate::view::pane_element(
            app,
            ctx,
            pane,
            rect,
            Some(POPUP_PANE_ID),
            Some("P"),
            crate::view::PaneKind::Popup,
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
    fn completed_popup_dismiss_keys_are_plain_enter_escape_and_space() {
        let key = |code, mods| KeyEvent { code, mods };
        assert!(dismisses_completed(key(KeyCode::Enter, KeyMods::NONE)));
        assert!(dismisses_completed(key(KeyCode::Esc, KeyMods::NONE)));
        assert!(dismisses_completed(key(KeyCode::Char(' '), KeyMods::NONE)));
        assert!(!dismisses_completed(key(KeyCode::Char('x'), KeyMods::NONE)));
        assert!(!dismisses_completed(key(
            KeyCode::Enter,
            KeyMods {
                ctrl: true,
                ..KeyMods::NONE
            }
        )));
    }

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
