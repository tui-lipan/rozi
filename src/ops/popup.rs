use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::layout::anim::GeometryAnimation;
use crate::layout::geometry::{close_rect, workspace_tile_bounds};
use crate::ops::focus::{request_current_pane_focus, request_pane_focus};
use crate::ops::theme::pane_frame_background;
use crate::pane::lifecycle::{
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
    ctx: &mut Context<AppRoot>,
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
        launch: Some(crate::pane::launch::PaneLaunch::shell(command)),
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
    pane.begin_open_animation(ctx.state.config.animations);
    let env = pane_env(
        ctx.state.control_socket_path.as_deref(),
        &pane,
        ctx.state.current().remote_host.is_some(),
        &ctx.state.config.environment.forward,
    );
    let identity = pane.identity.clone();
    let (cols, rows) = (pane.terminal.cols, pane.terminal.rows);
    ctx.state.popup_return_focus = ctx.state.focused_pane();
    ctx.state.popup = Some(pane);
    ctx.state.begin_pane_event(GeometryAnimation::Spawn);
    let open_delay = crate::layout::anim::open_delay(ctx.state.config.animations);
    let activate_delay = crate::layout::anim::activation_delay(ctx.state.config.animations);
    request_pane_spawn(
        &mut ctx.state,
        PaneSpawnRequest {
            pane_id: POPUP_PANE_ID,
            local: true,
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

pub(crate) fn close(ctx: &mut Context<AppRoot>) -> Update {
    let animations = ctx.state.config.animations;
    let client = ctx.state.current().session_client.clone();
    let Some(pane) = ctx.state.popup.as_mut().filter(|pane| !pane.closing) else {
        return Update::none();
    };
    let generation = pane.pty_generation;
    if let Some(client) = client {
        client.kill(POPUP_PANE_ID, generation, true);
    }
    pane.opening = false;
    pane.opening_animation = None;
    // Stay described so the popup scales out the way it scaled in; `prune_closed_pane` drops it.
    pane.closing = true;
    pane.begin_close_animation(animations);
    pane.terminal.kill();
    let timeout = crate::layout::anim::retained_pane_timeout_for_pane(animations, pane);
    ctx.state
        .begin_pane_event(crate::layout::anim::GeometryAnimation::Close);
    restore_focus(ctx);
    Update::with_command(crate::pane::lifecycle::prune_closed_command(
        ctx.state.runtime_epoch,
        POPUP_PANE_ID,
        generation,
        timeout,
    ))
}

pub(crate) fn handle_exit(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::update_check::popup_exited(ctx);
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
pub(crate) fn kill_if_open(ctx: &mut Context<AppRoot>) {
    if let Some(pane) = ctx.state.popup.take() {
        ctx.state.popup_return_focus = None;
        if let Some(client) = ctx.state.current().session_client.clone() {
            client.kill(POPUP_PANE_ID, pane.pty_generation, true);
        }
    }
}

fn restore_focus(ctx: &mut Context<AppRoot>) -> Update {
    if let Some(previous) = ctx.state.popup_return_focus.take() {
        crate::ops::focus::focus_pane(&mut ctx.state, previous);
        request_pane_focus(ctx, previous);
    } else {
        request_current_pane_focus(ctx);
    }
    Update::full()
}

pub(crate) fn placement(ctx: &Context<AppRoot>) -> Option<(FloatRect, Element)> {
    let pane = ctx.state.popup.as_ref()?;
    // Portal and Scan reveal in place. Off has no collapsed geometry. The kind is the lifecycle
    // snapshot, so a pane_style reload does not resize a transition already running.
    let spec = crate::layout::anim::pane_animation_for_pane(ctx.state.config.animations, pane);
    let full_size = matches!(
        spec.kind,
        crate::layout::anim::PaneAnimationStyle::Portal
            | crate::layout::anim::PaneAnimationStyle::Scan
            | crate::layout::anim::PaneAnimationStyle::Off
    );
    let target = if (pane.opening || pane.closing) && !full_size {
        close_rect(pane.floating_rect)
    } else {
        pane.floating_rect
    };
    let rect = ctx.transition(
        format!("rozi-pane-rect-{}", pane.id),
        target,
        crate::view::animation::transition_config_for(ctx, pane, false, target),
    );
    Some((
        rect,
        crate::view::pane_element(
            ctx,
            pane,
            rect,
            Some(POPUP_PANE_ID),
            Some("P"),
            crate::view::PaneKind::Popup,
            crate::view::PaneMerge::default(),
            crate::view::animation::pane_reveal_progress(
                ctx,
                pane,
                format!("rozi-popup-pane-reveal-{}", pane.id),
            ),
            false,
        ),
    ))
}

pub(crate) fn backdrop(ctx: &Context<AppRoot>) -> Option<(FloatRect, Element)> {
    ctx.state.popup.as_ref()?;
    let region: Element = MouseRegion::new()
        .capture_click(true)
        .on_mouse_down(ctx.link().callback(|_| crate::Msg::ClosePopup))
        .child(Text::new("").width(Length::Flex(1)).height(Length::Flex(1)))
        .into();
    Some((
        ctx.state
            .canvas_bounds_from_terminal_viewport(ctx.viewport()),
        region.key("rozi-popup-scrim"),
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

    /// Off is not a Scale inset. An opening popup is full size and already readable.
    #[test]
    fn an_off_opening_popup_is_full_size_and_visible() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                crate::test_support::isolate_user_dirs();
                let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
                let viewport = Rect {
                    x: 0,
                    y: 0,
                    w: 40,
                    h: 10,
                };
                backend.set_viewport(viewport);
                let full = FloatRect {
                    x: 8.0,
                    y: 2.0,
                    w: 24.0,
                    h: 6.0,
                };
                {
                    let state = backend.state_mut();
                    state.config.animations.pane_style =
                        crate::layout::anim::PaneAnimationStyle::Off;
                    state.config.animations.open_delay = std::time::Duration::ZERO;
                    state.config.pane.show_titles = false;
                    state.config.pane.show_workbar = false;
                    let mut popup = crate::state::Pane::new(POPUP_PANE_ID, 5_000, full);
                    popup.opening = true;
                    popup.terminal_active = true;
                    popup.begin_open_animation(state.config.animations);
                    let _ = popup.terminal.process_server_output(b"OFF-POPUP-LIVE\r\n");
                    state.popup = Some(popup);
                }
                backend.render();

                let popup = backend.state().popup.as_ref().expect("opening popup");
                assert!(popup.opening);
                let collapsed = close_rect(full);
                let rect = backend
                    .rect_of_key(&crate::view::pane_window_key(POPUP_PANE_ID, 0).into())
                    .expect("opening Off popup");
                assert!(
                    (f32::from(rect.w) - full.w).abs() < 1.0
                        && (f32::from(rect.h) - full.h).abs() < 1.0,
                    "Off popup opened at {rect:?}; full {full:?}, collapsed {collapsed:?}"
                );
                assert!(
                    (f32::from(rect.w) - collapsed.w).abs() > 1.0,
                    "Off popup used the Scale close rect {rect:?} vs {collapsed:?}"
                );
                let frame = backend.capture_frame();
                let visible = (0..viewport.h).any(|y| {
                    let row = frame.row(y);
                    let line: String = row.iter().map(|cell| cell.symbol.as_str()).collect();
                    let Some(start) = line.find("OFF-POPUP-LIVE") else {
                        return false;
                    };
                    row.get(start..start + "OFF-POPUP-LIVE".len())
                        .is_some_and(|cells| cells.iter().any(|cell| cell.fg != cell.bg))
                });
                assert!(
                    visible,
                    "an opening Off popup should already show its content"
                );
            })
            .expect("spawn off popup test thread")
            .join()
            .expect("off popup test thread panicked");
    }

    /// A popup keeps the geometry of the snapshot it opened with. Reloading `pane_style` to Scale
    /// must not pull an in-flight Portal reveal down to the close inset.
    #[test]
    fn a_portal_popup_keeps_its_full_rect_when_pane_style_changes() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                crate::test_support::isolate_user_dirs();
                let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 40,
                    h: 10,
                });
                let full = FloatRect {
                    x: 8.0,
                    y: 2.0,
                    w: 24.0,
                    h: 6.0,
                };
                {
                    let state = backend.state_mut();
                    state.config.animations.pane_style =
                        crate::layout::anim::PaneAnimationStyle::Portal;
                    state.config.pane.show_titles = false;
                    state.config.pane.show_workbar = false;
                    let mut popup = crate::state::Pane::new(POPUP_PANE_ID, 5_000, full);
                    popup.opening = true;
                    popup.terminal_active = true;
                    popup.begin_open_animation(state.config.animations);
                    state.config.animations.pane_style =
                        crate::layout::anim::PaneAnimationStyle::Scale;
                    state.popup = Some(popup);
                }
                backend.render();

                let collapsed = close_rect(full);
                let rect = backend
                    .rect_of_key(&crate::view::pane_window_key(POPUP_PANE_ID, 0).into())
                    .expect("opening Portal popup");
                assert!(
                    (f32::from(rect.w) - full.w).abs() < 1.0
                        && (f32::from(rect.h) - full.h).abs() < 1.0,
                    "Portal popup followed the reloaded style: {rect:?}, full {full:?}, collapsed {collapsed:?}"
                );
            })
            .expect("spawn portal popup reload test thread")
            .join()
            .expect("portal popup reload test thread panicked");
    }

    #[test]
    fn popup_return_focus_uses_visible_scratch_focus() {
        let mut state =
            crate::state::State::new(crate::config::Config::default(), Theme::default());
        state.scratch_visible = true;
        state.scratch.focused_pane = Some(42);
        state.current_mut().focused_pane = Some(1);
        assert_eq!(state.focused_pane(), Some(42));
    }
}
