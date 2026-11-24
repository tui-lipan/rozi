use std::time::Duration;
use tui_lipan::prelude::*;

use crate::Msg;
use crate::pane::id::PaneId;

pub(crate) fn open_timers_command(
    link: &tui_lipan::Link<crate::Msg>,
    epoch: u64,
    id: PaneId,
    generation: u64,
    open_delay: Duration,
    activate_delay: Duration,
) -> Command {
    link.command_after(open_delay, move |link: CommandLink<Msg>| {
        run_open_timers(epoch, id, generation, open_delay, activate_delay, link);
    })
}

fn run_open_timers(
    epoch: u64,
    id: PaneId,
    generation: u64,
    open_delay: Duration,
    activate_delay: Duration,
    link: CommandLink<Msg>,
) {
    // `open_delay` has already elapsed on the timer thread; chain the second stage there too
    // rather than sleeping, which would park an executor worker for the whole reveal.
    link.send(Msg::FinishOpen(epoch, id, generation));
    let remaining = activate_delay.saturating_sub(open_delay);
    let activate = Msg::ActivatePane(epoch, id, generation);
    if remaining.is_zero() {
        link.send(activate);
    } else {
        link.send_after(remaining, activate);
    }
}

/// Run the open/activate reveal timers for several panes at once. Panes created directly in state
/// (the initial pane, a restored profile/autosave layout, migrated panes) start with `opening =
/// true` (opacity 0) and are only spawned on the server via
/// [`crate::update`]; without these timers they would stay invisible. Interactive spawns get their
/// timers from [`spawn_pane_in_workspace`] instead.
pub(crate) fn open_timers_batch_command(
    link: &tui_lipan::Link<crate::Msg>,
    epoch: u64,
    targets: Vec<(PaneId, u64)>,
    open_delay: Duration,
    activate_delay: Duration,
) -> Command {
    link.command_after(open_delay, move |link: CommandLink<Msg>| {
        for (id, generation) in &targets {
            link.send(Msg::FinishOpen(epoch, *id, *generation));
        }
        // Second stage goes back on the timer thread; sleeping here would park an executor worker
        // for the whole reveal, and a restored layout arms one of these per pane.
        let remaining = activate_delay.saturating_sub(open_delay);
        for (id, generation) in &targets {
            let activate = Msg::ActivatePane(epoch, *id, *generation);
            if remaining.is_zero() {
                link.send(activate);
            } else {
                link.send_after(remaining, activate);
            }
        }
    })
}

/// The epoch the open and close timers of a client-local pane (scratch or popup) carry. Scratch
/// panes outlive every session switch, reconnect, and park, so their timers must not be tied to a
/// session's epoch. A popup is closed on a switch or reconnect instead; it shares this epoch as
/// client-local state, and its client-wide generation keeps an old popup's timer from matching a
/// later one. Attachment ids are minted upward from zero and never reach it.
pub const LOCAL_PANE_EPOCH: u64 = u64::MAX;

/// The epoch pane `id`'s open and close timers are armed under and checked against: the current
/// session's for a pane in its workspaces, [`LOCAL_PANE_EPOCH`] for a scratch or popup pane.
pub fn lifecycle_epoch(state: &crate::state::State, id: PaneId) -> u64 {
    if super::pane_is_local(state, id) {
        LOCAL_PANE_EPOCH
    } else {
        state.runtime_epoch
    }
}

/// Arm again, under the current epoch, the open and close transitions of the current attachment's
/// panes that are still under way.
///
/// Each transition's timers carry the epoch they were armed under, and are dropped when they fire
/// under another: while their attachment is parked, or after a reconnect gave it a new epoch.
/// Without fresh ones an opening pane stays hidden and unfocusable, and a closing pane is never
/// pruned. Call this whenever an attachment becomes current again. Scratch and popup panes need
/// none of this: their timers carry [`LOCAL_PANE_EPOCH`].
pub(crate) fn rearm_pane_transitions(
    link: &tui_lipan::Link<crate::Msg>,
    state: &crate::state::State,
) -> Update {
    let animations = state.config.animations;
    let frame_rate = state.runtime_frame_rate();
    let epoch = state.runtime_epoch;
    let mut update = Update::none();
    let mut closing = Vec::new();
    let mut close_delay = Duration::ZERO;
    for pane in state
        .current()
        .workspaces
        .iter()
        .flat_map(|workspace| &workspace.panes)
    {
        let (id, generation) = (pane.id, pane.pty_generation);
        // Each pane finishes on the recipe it started with, not on the config as it is now.
        if pane.closing {
            closing.push((id, generation));
            close_delay = close_delay.max(crate::layout::anim::retained_pane_timeout_for_pane(
                animations, pane, frame_rate,
            ));
        } else if pane.opening {
            let (open_delay, activation_delay) = open_stages(animations, pane);
            update = update.merge(Update::full_with_command(open_timers_command(
                link,
                epoch,
                id,
                generation,
                open_delay,
                activation_delay,
            )));
        } else if crate::layout::anim::pane_opening_transition(pane) {
            // Opened but not yet activated: only the second stage is outstanding.
            let (open_delay, activation_delay) = open_stages(animations, pane);
            update = update.merge(Update::full_with_command(link.command_after(
                activation_delay.saturating_sub(open_delay),
                move |link: CommandLink<Msg>| {
                    link.send(Msg::ActivatePane(epoch, id, generation));
                },
            )));
        }
    }
    if !closing.is_empty() {
        update = update.merge(Update::full_with_command(
            super::prune_closed_batch_command(link, epoch, closing, close_delay),
        ));
    }
    update
}

/// When a pane's open finishes its first stage and when it takes input, both from when it began
/// opening: from the recipe it captured then, or the config for a pane that captured none.
fn open_stages(
    animations: crate::layout::anim::WindowAnimationConfig,
    pane: &crate::state::Pane,
) -> (Duration, Duration) {
    match pane.opening_animation {
        Some(snapshot) if snapshot.active => (
            snapshot.open_delay,
            snapshot.open_delay + snapshot.spec.open_duration,
        ),
        Some(_) => (Duration::ZERO, Duration::ZERO),
        None => (
            crate::layout::anim::open_delay(animations),
            crate::layout::anim::activation_delay(animations),
        ),
    }
}
