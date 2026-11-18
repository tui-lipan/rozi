use std::time::Duration;
use tui_lipan::prelude::*;

use crate::Msg;
use crate::pane::id::PaneId;

pub(crate) fn open_timers_command(
    epoch: u64,
    id: PaneId,
    generation: u64,
    open_delay: Duration,
    activate_delay: Duration,
) -> Command {
    Command::after(open_delay, move |link: CommandLink<Msg>| {
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
    epoch: u64,
    targets: Vec<(PaneId, u64)>,
    open_delay: Duration,
    activate_delay: Duration,
) -> Command {
    Command::after(open_delay, move |link: CommandLink<Msg>| {
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

/// Arm again, under the current epoch, the open and close transitions of the current attachment's
/// panes that are still under way.
///
/// Each transition's timers carry the epoch they were armed under, and are dropped when they fire
/// under another: while their attachment is parked, or after a reconnect gave it a new epoch.
/// Without fresh ones an opening pane stays hidden and unfocusable, and a closing pane is never
/// pruned. Call this whenever an attachment becomes current again.
pub(crate) fn rearm_pane_transitions(state: &crate::state::State) -> Update {
    let animations = state.config.animations;
    let mut opening = Vec::new();
    let mut activating = Vec::new();
    let mut closing = Vec::new();
    for pane in state
        .current()
        .workspaces
        .iter()
        .flat_map(|workspace| &workspace.panes)
    {
        let target = (pane.id, pane.pty_generation);
        if pane.closing {
            closing.push(target);
        } else if pane.opening {
            opening.push(target);
        } else if crate::layout::anim::pane_opening_transition(pane) {
            // Opened but not yet activated: only the second stage is outstanding.
            activating.push(target);
        }
    }
    let epoch = state.runtime_epoch;
    let mut update = Update::none();
    if !opening.is_empty() {
        update = update.merge(Update::with_command(open_timers_batch_command(
            epoch,
            opening,
            crate::layout::anim::open_delay(animations),
            crate::layout::anim::activation_delay(animations),
        )));
    }
    if !activating.is_empty() {
        let delay = crate::layout::anim::activation_delay(animations);
        update = update.merge(Update::with_command(Command::after(
            delay,
            move |link: CommandLink<Msg>| {
                for (id, generation) in activating {
                    link.send(Msg::ActivatePane(epoch, id, generation));
                }
            },
        )));
    }
    if !closing.is_empty() {
        let delay =
            crate::layout::anim::retained_pane_timeout(animations, state.runtime_frame_rate());
        update = update.merge(Update::with_command(super::prune_closed_batch_command(
            epoch, closing, delay,
        )));
    }
    update
}
