use tui_lipan::prelude::*;

use crate::input::routing::handle_key_routing;
use crate::layout::anim::GeometryAnimation;
use crate::ops::focus::{acknowledge_pane_input, focus_pane as focus, request_pane_focus};
use crate::pane::lifecycle::find_pane_mut;
use crate::pane::pty_events::{
    handle_pane_input, handle_pane_mouse, handle_pane_resize, handle_pane_scroll,
};
use crate::state::{AlertMode, PaneId, ResizeCorner, State};
use crate::{AppRoot, control, schedule_alert_pulse_tick};

pub(super) fn close_popup(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::popup::close(ctx)
}

pub(super) fn focus_pane(ctx: &mut Context<AppRoot>, id: PaneId) -> Update {
    // A click while hint mode is up dismisses it and stops there: the labels belong to the pane
    // that was focused when the mode was entered, so moving focus out from under them would leave
    // a pane wearing another pane's hints.
    if crate::ops::hints::cancel_for_pointer(ctx) {
        return Update::full();
    }
    focus(&mut ctx.state, id);
    request_pane_focus(ctx, id);
    Update::full()
}

pub(super) fn hover_pane(ctx: &mut Context<AppRoot>, id: PaneId, mods: KeyMods) -> Update {
    crate::ops::focus::hover_focus_pane(ctx, id, mods)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn begin_move(
    ctx: &mut Context<AppRoot>,
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
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    dx: i16,
    dy: i16,
    modified: bool,
) -> Update {
    crate::ops::resize_move::move_pane(ctx, id, dx, dy, modified)
}

pub(super) fn end_move(ctx: &mut Context<AppRoot>, id: PaneId, x: u16, y: u16) -> Update {
    crate::ops::resize_move::end_move(ctx, id, x, y)
}

pub(super) fn begin_resize(
    ctx: &mut Context<AppRoot>,
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
    ctx: &mut Context<AppRoot>,
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

pub(super) fn end_resize(ctx: &mut Context<AppRoot>, id: PaneId) -> Update {
    crate::ops::session::flush_live_layout_gesture(ctx);
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
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    horizontal_split: bool,
    x: u16,
    y: u16,
) -> Update {
    crate::ops::resize_move::begin_resize_split_drag(ctx, id, horizontal_split, x, y)
}

/// One move of a split or junction drag. The drag's begin message fixed which boundaries it moves;
/// the move only says where the pointer is now.
pub(super) fn resize_split(
    ctx: &mut Context<AppRoot>,
    from_x: u16,
    from_y: u16,
    x: u16,
    y: u16,
) -> Update {
    crate::ops::resize_move::continue_split_drag(ctx, from_x, from_y, x, y)
}

pub(super) fn begin_resize_split_junction(
    ctx: &mut Context<AppRoot>,
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

pub(super) fn end_resize_split(ctx: &mut Context<AppRoot>) -> Update {
    crate::ops::session::flush_live_layout_gesture(ctx);
    ctx.state.split_drag = None;
    Update::full()
}

pub(super) fn begin_scratch_resize(ctx: &mut Context<AppRoot>, _from_y: u16) -> Update {
    crate::scratchpad::begin_resize(ctx)
}

pub(super) fn scratch_resize(ctx: &mut Context<AppRoot>, from_y: u16, y: u16) -> Update {
    crate::scratchpad::resize(ctx, from_y, y)
}

pub(super) fn end_scratch_resize(ctx: &mut Context<AppRoot>) -> Update {
    crate::scratchpad::end_resize(ctx)
}

pub(super) fn finish_open(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    id: PaneId,
    generation: u64,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    let opened = match find_pane_mut(&mut ctx.state, id) {
        Some(pane) if pane.pty_generation != generation => return Update::none(),
        Some(pane) if !pane.closing => {
            pane.opening = false;
            true
        }
        _ => false,
    };
    if opened {
        // Re-arm through `begin_pane_event`, not a bare assignment: a close in the gap since the
        // spawn would otherwise leave the neighbours moving on the close clock.
        ctx.state.begin_pane_event(GeometryAnimation::Spawn);
    }
    Update::full()
}

pub(super) fn activate_pane(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    id: PaneId,
    generation: u64,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    let focused = ctx.state.focused_pane() == Some(id)
        || (id == crate::state::POPUP_PANE_ID && ctx.state.popup.is_some());
    if let Some(pane) = find_pane_mut(&mut ctx.state, id) {
        if pane.pty_generation != generation {
            return Update::none();
        }
        if !pane.closing {
            pane.terminal_active = true;
            pane.opening_animation = None;
            if focused {
                request_pane_focus(ctx, id);
            }
        }
    }
    Update::full()
}

pub(super) fn copy_feedback_expired(
    ctx: &mut Context<AppRoot>,
    attachment: u64,
    id: PaneId,
    epoch: u64,
) -> Update {
    crate::input::copy_mode::expire_copy_feedback(ctx, attachment, id, epoch)
}

pub(super) fn pane_input(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    input: TerminalInputEvent,
) -> Update {
    handle_pane_input(ctx, id, input)
}

pub(super) fn pane_key(ctx: &mut Context<AppRoot>, id: PaneId, key: KeyEvent) -> Update {
    if logical_focus_pending_activation(&ctx.state).is_none_or(|pending| pending == id) {
        focus(&mut ctx.state, id);
    }
    // Held here as well as in the PTY funnel: a key the app consumes as an action never reaches a
    // PTY, and typing at a pane is presence whether or not the keystroke ends up as pane input.
    acknowledge_pane_input(&mut ctx.state, id);
    let (_handled, update) = handle_key_routing(ctx, key, Some(id));
    update
}

pub(super) fn pane_link_activate(ctx: &mut Context<AppRoot>, event: TerminalLinkEvent) -> Update {
    match tui_lipan::utils::open_url(&event.uri) {
        Ok(()) => Update::none(),
        Err(error) => {
            crate::pane::pty_events::notify_error(ctx, "Could not open link", error.to_string());
            Update::full()
        }
    }
}

pub(super) fn forward_prefix(ctx: &mut Context<AppRoot>, key: KeyEvent) -> Update {
    let Some(id) = ctx.state.focused_pane() else {
        return Update::none();
    };
    crate::pane::pty_events::forward_key_to_pane(ctx, id, key)
}

pub(super) fn pane_mouse(ctx: &mut Context<AppRoot>, id: PaneId, bytes: Vec<u8>) -> Update {
    handle_pane_mouse(ctx, id, bytes)
}

pub(super) fn pane_resize(ctx: &mut Context<AppRoot>, id: PaneId, cols: u16, rows: u16) -> Update {
    handle_pane_resize(ctx, id, cols, rows)
}

pub(super) fn pane_scroll(ctx: &mut Context<AppRoot>, id: PaneId, offset: usize) -> Update {
    handle_pane_scroll(ctx, id, offset)
}

pub(super) fn control_request(
    ctx: &mut Context<AppRoot>,
    envelope: control::ControlEnvelope,
) -> Update {
    crate::ops::control::handle_control_request(ctx, envelope)
}

/// One shared, self-cancelling pulse chain for visible pane frames and inactive marked tabs.
pub(crate) fn arm_alert_pulse(ctx: &mut Context<AppRoot>) {
    if ctx.state.alert_pulse_armed || !alert_pulse_should_run(&ctx.state) {
        return;
    }
    let Some(link) = ctx.state.command_link.clone() else {
        return;
    };
    let half = crate::layout::anim::alert_pulse_half_period(ctx.state.config.animations);
    ctx.state.alert_pulse_armed = true;
    ctx.state.alert_pulse_armed_at = ctx.elapsed();
    ctx.state.alert_pulse_half = half;
    ctx.state.alert_pulse_turns = 0;
    link.send_after(half, crate::Msg::AlertPulseTick);
}

pub(super) fn alert_pulse_tick(ctx: &mut Context<AppRoot>) -> Update {
    if !alert_pulse_should_run(&ctx.state) {
        let changed = ctx.state.alert_pulse_phase || ctx.state.alert_pulse_calm_phase;
        ctx.state.alert_pulse_armed = false;
        ctx.state.alert_pulse_phase = false;
        ctx.state.alert_pulse_calm_phase = false;
        ctx.state.alert_pulse_turns = 0;
        return if changed {
            Update::full()
        } else {
            Update::none()
        };
    }
    let now = ctx.elapsed();
    let half = crate::layout::anim::alert_pulse_half_period(ctx.state.config.animations);
    let delay = turn_alert_pulse(&mut ctx.state, now, half);
    Update::with_command(schedule_alert_pulse_tick(delay))
}

/// Bring the chain's phases to where the clock says they are at `now`, and return how long until
/// its next turn falls due.
///
/// The phases are a function of `now - alert_pulse_armed_at` alone. A tick handled late does not
/// push the next one later, which is what keeps the border in step with the content tint's
/// registry pulse on the same anchor. A stall of several half periods lands on the phase the clock
/// has reached in one step rather than working through the turns it missed, and a tick that fires
/// a hair early turns nothing and just waits out the remainder.
///
/// A new half period (the animation speed changed) re-anchors the beat so the turns already taken
/// keep their count, and the phase carries on from where it is rather than jumping.
fn turn_alert_pulse(
    state: &mut State,
    now: std::time::Duration,
    half: std::time::Duration,
) -> std::time::Duration {
    let half = half.max(std::time::Duration::from_millis(1));
    if half != state.alert_pulse_half {
        state.alert_pulse_armed_at = now.saturating_sub(half * state.alert_pulse_turns);
        state.alert_pulse_half = half;
    }
    let since = now.saturating_sub(state.alert_pulse_armed_at);
    let due = u32::try_from(since.as_nanos() / half.as_nanos()).unwrap_or(u32::MAX);
    if due > state.alert_pulse_turns {
        state.alert_pulse_turns = due;
        // Urgent alerts turn every half period; calm ones every other, which keeps the two rates
        // in a fixed ratio off one beat.
        state.alert_pulse_phase = due % 2 == 1;
        state.alert_pulse_calm_phase = (due / 2) % 2 == 1;
    }
    let next = state.alert_pulse_armed_at + half * state.alert_pulse_turns.saturating_add(1);
    next.saturating_sub(now)
}

fn alert_pulse_should_run(state: &State) -> bool {
    let animations = state.config.animations;
    if !animations.enabled || !animations.focus_chrome {
        return false;
    }
    (state.config.pane.alert_border == AlertMode::Pulse && visible_pane_alert_can_pulse(state))
        || (state.config.workbar.alert.mode == AlertMode::Pulse
            && inactive_tab_marker_can_pulse(state))
        || visible_recording_dot(state)
}

/// A recording dot on screen, which blinks: in a recorded pane's own chrome on the workspace in
/// view, in the chrome of a fullscreen pane covering a recording, on a workspace tab, or in the
/// chip of a UI recording itself.
fn visible_recording_dot(state: &State) -> bool {
    if crate::ops::ui_recording::shows_indicator(state) {
        return true;
    }
    let workspace = state.active_workspace_ref();
    let chrome = crate::view::pane_chrome_shows_recording(&state.config.pane);
    // A fullscreen pane covers everything else, the workbar included, so only its chrome shows.
    if let Some(cover) = crate::view::fullscreen_pane(workspace) {
        return chrome
            && ((cover.terminal.recording && !cover.closing)
                || crate::view::covers_a_recording(state, cover));
    }
    let in_chrome = chrome
        && workspace
            .panes
            .iter()
            .any(|pane| pane.terminal.recording && !pane.closing);
    let on_a_tab = workbar_shows_workspace_tabs(state)
        && (0..state.current().workspaces.len())
            .any(|index| crate::view::workspace_tab_shows_recording(state, index));
    in_chrome || on_a_tab
}

/// Whether the workbar is on screen with a `Workspaces` segment, so a tab marker has a tab to sit on.
fn workbar_shows_workspace_tabs(state: &State) -> bool {
    state.config.pane.show_workbar
        && state
            .config
            .workbar
            .left
            .iter()
            .chain(state.config.workbar.right.iter())
            .any(|item| matches!(item.segment, crate::config::WorkbarSegment::Workspaces))
}

fn visible_pane_alert_can_pulse(state: &State) -> bool {
    if !crate::view::has_pane_alert(state) {
        return false;
    }
    let workspace = state.active_workspace_ref();
    let focused = workspace.focused_pane.or(state.current().focused_pane);
    let config = &state.config.pane;
    let frames = config.border_mode.draws_frames() && config.alert_paint.paints_border();
    workspace.panes.iter().any(|pane| {
        crate::view::pane_alert(pane, focused == Some(pane.id), config).is_some_and(|(_, color)| {
            (frames && crate::ops::theme::pane_frame_alert_can_pulse(&state.theme, color))
                || (config.alert_paint.paints_content()
                    && crate::view::pane_content_alert_can_tint(&state.theme, color))
        })
    })
}

fn inactive_tab_marker_can_pulse(state: &State) -> bool {
    if !workbar_shows_workspace_tabs(state) || !crate::view::has_inactive_marked_workspace(state) {
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
    let id = state.focused_pane()?;
    let workspace = state.active_workspace_ref();
    workspace
        .panes
        .iter()
        .any(|pane| pane.id == id && !pane.terminal_active && !pane.closing)
        .then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::state::{Pane, PaneAlertPaint, PaneBorderMode};
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
        let mut state = State::new(Config::default(), Theme::default());
        assert!(!alert_pulse_should_run(&state));
        state.current_mut().workspaces[0]
            .panes
            .push(blocked_pane(2));
        assert!(alert_pulse_should_run(&state));

        // Dividers draw no per-pane frame, so a border-only paint has nothing to breathe; the
        // content tint still does.
        state.config.pane.border_mode = PaneBorderMode::Dividers;
        assert!(alert_pulse_should_run(&state));
        state.config.pane.alert_paint = PaneAlertPaint::Border;
        assert!(!alert_pulse_should_run(&state));
        state.config.pane.border_mode = PaneBorderMode::Separate;
        state.config.animations.focus_chrome = false;
        assert!(!alert_pulse_should_run(&state));
    }

    #[test]
    fn workbar_pulse_can_arm_for_a_background_marker_when_borders_are_off() {
        let mut state = State::new(Config::default(), Theme::default());
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
    fn a_recording_in_view_runs_the_pulse_only_while_its_dot_can_blink() {
        let mut state = State::new(Config::default(), Theme::default());
        let mut pane = blocked_pane(2);
        pane.terminal.reported_status = None;
        pane.terminal.recording = true;
        state.current_mut().workspaces[0].panes.push(pane);
        assert!(alert_pulse_should_run(&state));

        state.config.animations.enabled = false;
        assert!(!alert_pulse_should_run(&state), "motion off holds the dot");
        state.config.animations.enabled = true;
        state.config.animations.focus_chrome = false;
        assert!(!alert_pulse_should_run(&state));
        state.config.animations.focus_chrome = true;

        state.config.pane.show_titles = false;
        assert!(alert_pulse_should_run(&state), "the corner dot blinks too");
        state.config.pane.border_mode = PaneBorderMode::Dividers;
        assert!(alert_pulse_should_run(&state), "the tab's dot blinks");
        state.config.workbar.left.clear();
        state.config.workbar.right.clear();
        assert!(
            !alert_pulse_should_run(&state),
            "a workbar without workspace tabs has no dot to blink"
        );
        state.config.workbar = Config::default().workbar;
        state.config.pane.show_titles = true;

        // A fullscreen pane covers the recorded one and the workbar, so its chrome carries the dot.
        let mut cover = blocked_pane(3);
        cover.terminal.reported_status = None;
        cover.fullscreen = true;
        state.current_mut().workspaces[0].panes.push(cover);
        assert!(alert_pulse_should_run(&state), "the cover's dot blinks");
        state.config.pane.show_titles = false;
        state.config.pane.border_mode = PaneBorderMode::Dividers;
        assert!(
            !alert_pulse_should_run(&state),
            "the tab's dot sits under the fullscreen pane"
        );
        state.config.pane = Config::default().pane;
        state.current_mut().workspaces[0].panes.pop();

        let pane = state.current_mut().workspaces[0].panes.pop().unwrap();
        state.current_mut().workspaces[1].panes.push(pane);
        assert!(
            alert_pulse_should_run(&state),
            "another workspace's tab blinks"
        );
        state.config.pane.show_workbar = false;
        assert!(
            !alert_pulse_should_run(&state),
            "no workbar, no tab to blink"
        );
    }

    fn ms(ms: u64) -> std::time::Duration {
        std::time::Duration::from_millis(ms)
    }

    /// A chain armed at `armed_at` on a 100 ms beat, before its first turn.
    fn armed_chain(armed_at: std::time::Duration) -> State {
        let mut state = State::new(Config::default(), Theme::default());
        state.alert_pulse_armed = true;
        state.alert_pulse_armed_at = armed_at;
        state.alert_pulse_half = ms(100);
        state
    }

    /// Each tick handled late still leaves the next due on the original beat, so lateness never
    /// adds up: forty ticks each 10 ms late leave the phase exactly where the clock puts it.
    #[test]
    fn late_ticks_never_push_the_beat_later() {
        let mut state = armed_chain(ms(1_000));
        let mut now = ms(1_000);
        let mut delay = ms(100);
        for turn in 1..=40u32 {
            now += delay + ms(10);
            delay = turn_alert_pulse(&mut state, now, ms(100));
            assert_eq!(state.alert_pulse_turns, turn);
            assert_eq!(
                delay,
                ms(90),
                "turn {turn}: the next is due on the beat, not after it"
            );
        }
        assert!(
            !state.alert_pulse_phase,
            "40 turns in: an even turn, heading up"
        );
        assert!(!state.alert_pulse_calm_phase, "20 calm turns in: even too");
    }

    /// A stall of several half periods lands on the phase the clock has reached in one step,
    /// instead of replaying the turns it missed one tick at a time.
    #[test]
    fn a_long_stall_lands_on_the_current_phase_in_one_turn() {
        let mut state = armed_chain(ms(1_000));
        assert_eq!(turn_alert_pulse(&mut state, ms(1_100), ms(100)), ms(100));
        assert_eq!(state.alert_pulse_turns, 1);

        // Stalled for 2.5 half periods past the next deadline.
        let delay = turn_alert_pulse(&mut state, ms(1_450), ms(100));
        assert_eq!(
            state.alert_pulse_turns, 4,
            "straight to the turn the clock is on"
        );
        assert!(!state.alert_pulse_phase && !state.alert_pulse_calm_phase);
        assert_eq!(delay, ms(50), "and the next turn is back on the beat");
    }

    /// A tick that fires a hair before its deadline turns nothing and waits out the remainder.
    #[test]
    fn an_early_tick_waits_for_its_deadline() {
        let mut state = armed_chain(ms(1_000));
        assert_eq!(turn_alert_pulse(&mut state, ms(1_099), ms(100)), ms(1));
        assert_eq!(state.alert_pulse_turns, 0);
        assert!(!state.alert_pulse_phase);
        turn_alert_pulse(&mut state, ms(1_100), ms(100));
        assert_eq!(state.alert_pulse_turns, 1);
        assert!(state.alert_pulse_phase);
    }

    /// A new animation speed re-anchors the beat at the turn it is on, so the phase carries on
    /// rather than jumping to wherever the new half period would put it from the old anchor.
    #[test]
    fn a_changed_half_period_carries_the_phase_on() {
        let mut state = armed_chain(ms(1_000));
        turn_alert_pulse(&mut state, ms(1_300), ms(100));
        assert_eq!(state.alert_pulse_turns, 3);
        let delay = turn_alert_pulse(&mut state, ms(1_300), ms(400));
        assert_eq!(state.alert_pulse_turns, 3, "no turn is taken or lost");
        assert_eq!(state.alert_pulse_armed_at, ms(100));
        assert_eq!(delay, ms(400), "the next turn is one new half period away");
    }

    #[test]
    fn the_pulse_chain_stops_once_the_last_recording_ends() {
        // Renders the whole app, a view tree deeper than the default test stack holds.
        on_large_stack(the_pulse_chain_stops_once_the_last_recording_ends_body);
    }

    fn the_pulse_chain_stops_once_the_last_recording_ends_body() {
        let mut backend = tui_lipan::TestBackend::new(AppRoot::default());
        let id = {
            let state = backend.state_mut();
            state.config.animations.enabled = true;
            state.config.animations.focus_chrome = true;
            let id = state.focused_pane().expect("a pane");
            crate::pane::lifecycle::find_pane_mut(state, id)
                .unwrap()
                .terminal
                .recording = true;
            state.alert_pulse_armed = true;
            state.alert_pulse_half =
                crate::layout::anim::alert_pulse_half_period(state.config.animations);
            id
        };
        let half = backend.state().alert_pulse_half;
        // Two turns: the phase follows the clock, so each tick needs its half period to pass.
        for _ in 0..2 {
            backend.advance(half);
            backend.dispatch(crate::Msg::AlertPulseTick).unwrap();
        }
        assert!(backend.state().alert_pulse_armed);
        assert!(backend.state().alert_pulse_calm_phase);

        crate::pane::lifecycle::find_pane_mut(backend.state_mut(), id)
            .unwrap()
            .terminal
            .recording = false;
        backend.dispatch(crate::Msg::AlertPulseTick).unwrap();
        let state = backend.state();
        assert!(!state.alert_pulse_armed);
        assert!(!state.alert_pulse_phase && !state.alert_pulse_calm_phase);
    }

    fn on_large_stack(test: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(test)
            .expect("spawn test thread")
            .join()
            .expect("test completes");
    }

    #[test]
    fn finish_open_keeps_its_snapshot_until_activation() {
        // Renders the whole app, a view tree deeper than the default test stack holds.
        on_large_stack(finish_open_keeps_its_snapshot_until_activation_body);
    }

    fn finish_open_keeps_its_snapshot_until_activation_body() {
        let mut backend = tui_lipan::TestBackend::new(AppRoot::default());
        let (id, generation, original) = {
            let state = backend.state_mut();
            let id = state.focused_pane().expect("fresh pane focus");
            state.config.animations.pane_open_style =
                crate::layout::anim::PaneAnimationStyle::Scale;
            state.config.animations.pane_close_style =
                crate::layout::anim::PaneAnimationStyle::Scale;
            state.config.animations.geometry_duration = std::time::Duration::from_millis(300);
            let animations = state.config.animations;
            let pane = crate::pane::lifecycle::find_pane_mut(state, id).expect("fresh pane");
            pane.opening = true;
            pane.begin_open_animation(animations);
            (
                id,
                pane.pty_generation,
                pane.opening_animation.expect("snapshot").spec,
            )
        };

        backend
            .dispatch(crate::Msg::FinishOpen(0, id, generation))
            .expect("finish open");
        {
            let state = backend.state_mut();
            state.config.animations.pane_open_style =
                crate::layout::anim::PaneAnimationStyle::Portal;
            state.config.animations.pane_close_style =
                crate::layout::anim::PaneAnimationStyle::Portal;
            state.config.animations.close_duration = std::time::Duration::from_millis(800);
            let pane = crate::pane::lifecycle::find_pane(state, id).expect("finished pane");
            assert!(!pane.opening);
            assert_eq!(
                crate::layout::anim::pane_animation_for_pane(state.config.animations, pane),
                original
            );
            assert!(crate::layout::anim::pane_opacity_animates(
                state.config.animations,
                pane
            ));
        }

        backend
            .dispatch(crate::Msg::ActivatePane(0, id, generation))
            .expect("activate pane");
        let state = backend.state();
        let pane = crate::pane::lifecycle::find_pane(state, id).expect("activated pane");
        assert!(pane.opening_animation.is_none());
        assert_eq!(
            crate::layout::anim::pane_animation_for_pane(state.config.animations, pane).kind,
            crate::layout::anim::PaneAnimationStyle::Portal
        );
    }

    #[test]
    fn pulse_surfaces_ignore_each_others_nonanimatable_roles() {
        let mut pane_state = State::new(Config::default(), Theme::default());
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

        let mut tab_state = State::new(Config::default(), Theme::default());
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
        let mut state = State::new(Config::default(), Theme::default());
        state.config.pane.alert_border = AlertMode::Pulse;
        state.config.pane.alert_paint = PaneAlertPaint::Border;
        state.current_mut().workspaces[0]
            .panes
            .push(blocked_pane(2));
        state.theme.status.error = Color::rgb(255, 0, 1);
        state.theme.border = Style::new().fg(state.theme.status.error);
        assert!(!alert_pulse_should_run(&state));
        // A border with nowhere to fade still leaves a truecolor tint to breathe over the content.
        state.config.pane.alert_paint = PaneAlertPaint::Both;
        assert!(alert_pulse_should_run(&state));

        // A palette colour neither fades on the border nor tints the content.
        state.theme.border = Style::default();
        state.theme.status.error = Color::Red;
        state.theme.status.warning = Color::Yellow;
        assert!(!alert_pulse_should_run(&state));
    }
}
