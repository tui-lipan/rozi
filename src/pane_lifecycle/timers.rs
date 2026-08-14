use std::time::Duration;
use tui_lipan::prelude::*;

use crate::state::PaneId;
use crate::Msg;

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
