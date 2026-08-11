use tui_lipan::prelude::*;

use crate::anim::GeometryAnimation;
use crate::key_routing::handle_key_routing;
use crate::ops::focus::{acknowledge_pane_if_attended, focus_pane as focus, request_pane_focus};
use crate::pane_lifecycle::find_pane_mut;
use crate::pty_events::{
    handle_pane_input, handle_pane_mouse, handle_pane_resize, handle_pane_scroll,
};
use crate::state::{AlertMode, PaneId, ResizeCorner, State};
use crate::{HyprmuxApp, control, schedule_alert_pulse_tick};

pub(super) fn close_popup(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::popup::close(ctx)
}

pub(super) fn focus_pane(ctx: &mut Context<HyprmuxApp>, id: PaneId) -> Update {
    focus(&mut ctx.state, id);
    request_pane_focus(ctx, id);
    Update::full()
}

pub(super) fn hover_pane(ctx: &mut Context<HyprmuxApp>, id: PaneId) -> Update {
    crate::ops::focus::hover_focus_pane(ctx, id)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn begin_move(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    current_rect: FloatRect,
    from_local_x: u16,
    from_local_y: u16,
    target_w: u16,
    target_h: u16,
    modified: bool,
) -> Update {
    crate::ops::resize_move::begin_move(
        ctx,
        id,
        current_rect,
        from_local_x,
        from_local_y,
        target_w,
        target_h,
        modified,
    )
}

pub(super) fn move_pane(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    dx: i16,
    dy: i16,
    modified: bool,
) -> Update {
    crate::ops::resize_move::move_pane(ctx, id, dx, dy, modified)
}

pub(super) fn end_move(ctx: &mut Context<HyprmuxApp>, id: PaneId, x: u16, y: u16) -> Update {
    crate::ops::resize_move::end_move(ctx, id, x, y)
}

pub(super) fn begin_resize(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    corner: ResizeCorner,
    x: u16,
    y: u16,
    modified: bool,
) -> Update {
    crate::ops::resize_move::begin_resize(ctx, id, corner, x, y, modified)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn resize_pane(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    corner: ResizeCorner,
    from_x: u16,
    from_y: u16,
    x: u16,
    y: u16,
    modified: bool,
) -> Update {
    crate::ops::resize_move::resize_pane(ctx, id, corner, (from_x, from_y), (x, y), modified)
}

pub(super) fn end_resize(ctx: &mut Context<HyprmuxApp>, id: PaneId) -> Update {
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

pub(super) fn begin_resize_split(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    horizontal_split: bool,
    x: u16,
    y: u16,
) -> Update {
    crate::ops::resize_move::begin_resize_split_drag(ctx, id, horizontal_split, x, y)
}

pub(super) fn resize_split(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    horizontal_split: bool,
    from_x: u16,
    from_y: u16,
    x: u16,
    y: u16,
) -> Update {
    crate::ops::resize_move::resize_split_by_drag(ctx, id, horizontal_split, from_x, from_y, x, y)
}

pub(super) fn begin_resize_split_junction(
    ctx: &mut Context<HyprmuxApp>,
    horizontal_panes: Vec<PaneId>,
    vertical_panes: Vec<PaneId>,
    x: u16,
    y: u16,
) -> Update {
    crate::ops::resize_move::begin_resize_split_junction_drag(
        ctx,
        horizontal_panes,
        vertical_panes,
        x,
        y,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn resize_split_junction(
    ctx: &mut Context<HyprmuxApp>,
    horizontal_panes: Vec<PaneId>,
    vertical_panes: Vec<PaneId>,
    from_x: u16,
    from_y: u16,
    x: u16,
    y: u16,
) -> Update {
    crate::ops::resize_move::resize_split_junction_by_drag(
        ctx,
        horizontal_panes,
        vertical_panes,
        from_x,
        from_y,
        x,
        y,
    )
}

pub(super) fn end_resize_split(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.split_drag = None;
    Update::full()
}

pub(super) fn begin_scratch_resize(ctx: &mut Context<HyprmuxApp>, _from_y: u16) -> Update {
    crate::scratchpad::begin_resize(ctx)
}

pub(super) fn scratch_resize(ctx: &mut Context<HyprmuxApp>, from_y: u16, y: u16) -> Update {
    crate::scratchpad::resize(ctx, from_y, y)
}

pub(super) fn end_scratch_resize(ctx: &mut Context<HyprmuxApp>) -> Update {
    crate::scratchpad::end_resize(ctx)
}

pub(super) fn finish_open(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    id: PaneId,
    generation: u64,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        if pane.pty_generation != generation {
            return Update::none();
        }
        if !pane.closing {
            pane.opening = false;
            ctx.state.animation = GeometryAnimation::Spawn;
        }
    }
    Update::full()
}

pub(super) fn activate_pane(
    ctx: &mut Context<HyprmuxApp>,
    epoch: u64,
    id: PaneId,
    generation: u64,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    let focused = ctx.state.current().focused_pane == Some(id)
        || (id == crate::state::POPUP_PANE_ID && ctx.state.popup.is_some());
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        if pane.pty_generation != generation {
            return Update::none();
        }
        if !pane.closing {
            pane.terminal_active = true;
            if focused {
                request_pane_focus(ctx, id);
            }
        }
    }
    Update::full()
}

pub(super) fn copy_feedback_expired(
    ctx: &mut Context<HyprmuxApp>,
    attachment: u64,
    id: PaneId,
    epoch: u64,
) -> Update {
    crate::copy_mode::expire_copy_feedback(ctx, attachment, id, epoch)
}

pub(super) fn pane_input(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    input: TerminalInputEvent,
) -> Update {
    handle_pane_input(ctx, id, input)
}

pub(super) fn pane_key(ctx: &mut Context<HyprmuxApp>, id: PaneId, key: KeyEvent) -> Update {
    if logical_focus_pending_activation(&ctx.state).is_none_or(|pending| pending == id) {
        focus(&mut ctx.state, id);
    }
    acknowledge_pane_if_attended(&mut ctx.state, id);
    let (_handled, update) = handle_key_routing(ctx, key, Some(id));
    update
}

pub(super) fn forward_prefix(ctx: &mut Context<HyprmuxApp>, key: KeyEvent) -> Update {
    let Some(id) = ctx.state.current().focused_pane else {
        return Update::none();
    };
    crate::pty_events::forward_key_to_pane(ctx, id, key)
}

pub(super) fn pane_mouse(ctx: &mut Context<HyprmuxApp>, id: PaneId, bytes: Vec<u8>) -> Update {
    handle_pane_mouse(ctx, id, bytes)
}

pub(super) fn pane_resize(
    ctx: &mut Context<HyprmuxApp>,
    id: PaneId,
    cols: u16,
    rows: u16,
) -> Update {
    handle_pane_resize(ctx, id, cols, rows)
}

pub(super) fn pane_scroll(ctx: &mut Context<HyprmuxApp>, id: PaneId, offset: usize) -> Update {
    handle_pane_scroll(ctx, id, offset)
}

pub(super) fn control_request(
    ctx: &mut Context<HyprmuxApp>,
    envelope: control::ControlEnvelope,
) -> Update {
    crate::ops::control::handle_control_request(ctx, envelope)
}

/// One shared, self-cancelling pulse chain for visible pane frames and inactive marked tabs.
pub(crate) fn arm_alert_pulse(ctx: &mut Context<HyprmuxApp>) {
    if ctx.state.alert_pulse_armed || !alert_pulse_should_run(&ctx.state) {
        return;
    }
    let Some(link) = ctx.state.command_link.clone() else {
        return;
    };
    ctx.state.alert_pulse_armed = true;
    link.send_after(
        crate::anim::alert_pulse_half_period(ctx.state.config.animations),
        crate::Msg::AlertPulseTick,
    );
}

pub(super) fn alert_pulse_tick(ctx: &mut Context<HyprmuxApp>) -> Update {
    if !alert_pulse_should_run(&ctx.state) {
        let changed = ctx.state.alert_pulse_phase || ctx.state.alert_pulse_calm_phase;
        ctx.state.alert_pulse_armed = false;
        ctx.state.alert_pulse_phase = false;
        ctx.state.alert_pulse_calm_phase = false;
        return if changed {
            Update::full()
        } else {
            Update::none()
        };
    }
    ctx.state.alert_pulse_phase = !ctx.state.alert_pulse_phase;
    // One chain drives both rates: the calm phase turns over once per full urgent cycle, which is
    // why it needs no timer of its own and can never drift out of step with the urgent one.
    if !ctx.state.alert_pulse_phase {
        ctx.state.alert_pulse_calm_phase = !ctx.state.alert_pulse_calm_phase;
    }
    Update::with_command(schedule_alert_pulse_tick(
        crate::anim::alert_pulse_half_period(ctx.state.config.animations),
    ))
}

fn alert_pulse_should_run(state: &State) -> bool {
    let animations = state.config.animations;
    if !animations.enabled || !animations.focus_chrome {
        return false;
    }
    (state.config.pane.alert_border == AlertMode::Pulse && visible_pane_alert_can_pulse(state))
        || (state.config.workbar.alert.mode == AlertMode::Pulse
            && inactive_tab_marker_can_pulse(state))
}

fn visible_pane_alert_can_pulse(state: &State) -> bool {
    if !crate::view::has_pane_alert(state) {
        return false;
    }
    let workspace = &state.current().workspaces[state.current().active_workspace];
    let focused = workspace.focused_pane.or(state.current().focused_pane);
    workspace.panes.iter().any(|pane| {
        crate::view::pane_alert(pane, focused == Some(pane.id), &state.config.pane).is_some_and(
            |(_, color)| crate::ops::theme::pane_frame_alert_can_pulse(&state.theme, color),
        )
    })
}

fn inactive_tab_marker_can_pulse(state: &State) -> bool {
    if !state.config.pane.show_workbar
        || !state
            .config
            .workbar
            .left
            .iter()
            .chain(state.config.workbar.right.iter())
            .any(|item| matches!(item.segment, crate::config::WorkbarSegment::Workspaces))
    {
        return false;
    }
    if !crate::view::has_inactive_marked_workspace(state) {
        return false;
    }
    state
        .current()
        .workspaces
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != state.current().active_workspace)
        .filter_map(|(_, workspace)| {
            crate::view::workspace_marker(workspace, &state.config.workbar.alert)
        })
        .map(crate::view::workspace_marker_color)
        .any(|color| {
            crate::ops::theme::tab_alert_can_pulse(
                &state.theme,
                color,
                state.config.workbar.alert.paint,
            )
        })
}

fn logical_focus_pending_activation(state: &State) -> Option<PaneId> {
    let id = state.current().focused_pane?;
    let workspace = &state.current().workspaces[state.current().active_workspace];
    workspace
        .panes
        .iter()
        .any(|pane| pane.id == id && !pane.terminal_active && !pane.closing)
        .then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HyprmuxConfig;
    use crate::state::{Pane, PaneBorderMode};
    use tui_lipan::prelude::{Color, Style};

    fn blocked_pane(id: PaneId) -> Pane {
        let mut pane = Pane::new(
            id,
            100,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        );
        pane.terminal.reported_status = Some(crate::session::protocol::PaneStatus {
            value: "blocked".into(),
            reason: None,
            set_at: 0,
        });
        pane
    }

    #[test]
    fn default_pulse_sleeps_without_alerts_and_arms_for_visible_frame_alerts() {
        let mut state = State::new(HyprmuxConfig::default(), Theme::default());
        assert!(!alert_pulse_should_run(&state));
        state.current_mut().workspaces[0]
            .panes
            .push(blocked_pane(2));
        assert!(alert_pulse_should_run(&state));

        state.config.pane.border_mode = PaneBorderMode::Dividers;
        assert!(!alert_pulse_should_run(&state));
        state.config.pane.border_mode = PaneBorderMode::Separate;
        state.config.animations.focus_chrome = false;
        assert!(!alert_pulse_should_run(&state));
    }

    #[test]
    fn workbar_pulse_can_arm_for_a_background_marker_when_borders_are_off() {
        let mut state = State::new(HyprmuxConfig::default(), Theme::default());
        state.config.pane.alert_border = AlertMode::Off;
        state.config.workbar.alert.mode = AlertMode::Pulse;
        let mut pane = blocked_pane(2);
        pane.terminal.reported_status = None;
        pane.terminal.finished_unseen = true;
        state.current_mut().workspaces[1].panes.push(pane);
        assert!(alert_pulse_should_run(&state));

        // `static` keeps the marker and its color but stops the tick; `off` removes the marker
        // outright. Neither may leave a pulse chain running.
        state.config.workbar.alert.mode = AlertMode::Static;
        assert!(!alert_pulse_should_run(&state));
        state.config.workbar.alert.mode = AlertMode::Off;
        assert!(!alert_pulse_should_run(&state));
        state.config.workbar.alert.mode = AlertMode::Pulse;

        state.config.pane.show_workbar = false;
        assert!(!alert_pulse_should_run(&state));
        state.config.pane.show_workbar = true;
        state.config.workbar.left.clear();
        state.config.workbar.right.clear();
        assert!(!alert_pulse_should_run(&state));

        state.config.pane.alert_border = AlertMode::Pulse;
        state.current_mut().workspaces[0].panes.push({
            let mut pane = blocked_pane(3);
            pane.terminal.reported_status = None;
            pane.terminal.finished_unseen = true;
            pane
        });
        assert!(
            alert_pulse_should_run(&state),
            "a visible pane pulse is independent of hidden/absent workspace tabs"
        );

        state.current_mut().workspaces[1].panes.clear();
        state.current_mut().workspaces[0].panes[0]
            .terminal
            .finished_unseen = true;
        state.config.pane.alert_border = AlertMode::Off;
        assert!(!alert_pulse_should_run(&state));
    }

    #[test]
    fn pulse_surfaces_ignore_each_others_nonanimatable_roles() {
        let mut pane_state = State::new(HyprmuxConfig::default(), Theme::default());
        pane_state.config.pane.alert_border = AlertMode::Pulse;
        pane_state.config.workbar.alert.mode = AlertMode::Pulse;
        let mut finished = blocked_pane(2);
        finished.terminal.reported_status = None;
        finished.terminal.finished_unseen = true;
        pane_state.current_mut().workspaces[0].panes.push(finished);
        pane_state.current_mut().workspaces[1]
            .panes
            .push(blocked_pane(3));
        pane_state.theme.status.error = Color::Red;
        assert!(alert_pulse_should_run(&pane_state));

        let mut tab_state = State::new(HyprmuxConfig::default(), Theme::default());
        tab_state.config.pane.alert_border = AlertMode::Pulse;
        tab_state.config.workbar.alert.mode = AlertMode::Pulse;
        tab_state.current_mut().workspaces[0]
            .panes
            .push(blocked_pane(2));
        let mut finished = blocked_pane(3);
        finished.terminal.reported_status = None;
        finished.terminal.finished_unseen = true;
        tab_state.current_mut().workspaces[1].panes.push(finished);
        tab_state.theme.status.error = Color::Red;
        assert!(alert_pulse_should_run(&tab_state));
    }

    #[test]
    fn equal_or_palette_alert_endpoints_do_not_arm_a_pane_pulse() {
        let mut state = State::new(HyprmuxConfig::default(), Theme::default());
        state.config.pane.alert_border = AlertMode::Pulse;
        state.current_mut().workspaces[0]
            .panes
            .push(blocked_pane(2));
        state.theme.status.error = Color::rgb(255, 0, 1);
        state.theme.border = Style::new().fg(state.theme.status.error);
        assert!(!alert_pulse_should_run(&state));

        state.theme.border = Style::default();
        state.theme.status.error = Color::Red;
        state.theme.status.warning = Color::Yellow;
        assert!(!alert_pulse_should_run(&state));
    }
}
