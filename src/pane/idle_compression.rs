//! History compression for terminal screens whose output has gone quiet.
//!
//! Only the Ghostty engine compresses history (`TerminalScreen::compress_idle`); Alacritty never
//! schedules any of this. A screen becomes due [`IDLE_BEFORE_COMPRESSION`] after its last output
//! and is then stepped once per poll, every [`POLL_INTERVAL`], until the engine
//! reports nothing left. Each process shares a two-millisecond budget across all panes and resumes
//! at the next pane when it expires. New output puts it back to waiting. The session server polls
//! from its loop and the UI client from a passive timer; neither touches the output path beyond
//! noting the time. Compression changes how history is stored, never what it holds, so neither side repaints
//! or re-exports anything for it.

use std::time::{Duration, Instant};

use tui_lipan::prelude::TerminalScreen;

pub(crate) const IDLE_BEFORE_COMPRESSION: Duration = Duration::from_secs(2);
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(250);
const POLL_BUDGET: Duration = Duration::from_millis(2);

/// Whether this process compresses at all: only on the Ghostty engine.
pub(crate) fn enabled() -> bool {
    super::engine::active() == super::engine::TerminalEngine::Ghostty
}

/// Visit panes in turn under one budget for the entire poll. A native compression call is
/// incremental but cannot be interrupted, so the budget can overrun by at most that one call.
/// The cursor advances before yielding, including when a pane has nothing left to compress.
pub(crate) fn poll<T>(panes: &mut [T], cursor: &mut usize, step: impl FnMut(&mut T)) {
    poll_for(panes, cursor, POLL_BUDGET, step);
}

fn poll_for<T>(
    panes: &mut [T],
    cursor: &mut usize,
    budget: Duration,
    mut step: impl FnMut(&mut T),
) {
    if panes.is_empty() {
        // The server also calls us between rate-limited polls. Keep its place for the next one.
        return;
    }
    *cursor %= panes.len();
    let started = Instant::now();
    for _ in 0..panes.len() {
        let index = *cursor;
        *cursor = (index + 1) % panes.len();
        step(&mut panes[index]);
        if started.elapsed() >= budget {
            break;
        }
    }
}

/// A client pane's compression state.
#[derive(Default)]
pub(crate) struct ClientIdleCompression {
    last_output: Option<Instant>,
}

impl ClientIdleCompression {
    pub(crate) fn note_output(&mut self) {
        if enabled() {
            self.last_output = Some(Instant::now());
        }
    }

    pub(crate) fn poll(&mut self, screen: &mut TerminalScreen, now: Instant) {
        if self
            .last_output
            .is_some_and(|last| now.duration_since(last) >= IDLE_BEFORE_COMPRESSION)
            && !screen.compress_idle()
        {
            self.last_output = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exhausted_poll_yields_and_resumes_at_the_next_pane() {
        let mut panes = [0, 1, 2];
        let mut cursor = 0;
        let mut visited = Vec::new();
        // Zero allows one non-interruptible step, then deterministically exhausts the poll.
        // No sleeps or assumptions about how long a native compression call takes.
        for expected in [0, 1, 2, 0] {
            let before = visited.len();
            poll_for(&mut panes, &mut cursor, Duration::ZERO, |pane| {
                visited.push(*pane);
            });
            assert_eq!(&visited[before..], &[expected]);
        }
        // Rate-limited server turns supply no panes and must preserve the next poll's place.
        poll_for(&mut panes[..0], &mut cursor, Duration::ZERO, |_| {
            panic!("an empty poll has no work");
        });
        assert_eq!(cursor, 1);
        // Removing panes between ticks cannot leave the cursor outside the new list.
        poll_for(&mut panes[..1], &mut cursor, Duration::ZERO, |pane| {
            visited.push(*pane);
        });
        assert_eq!(visited.last(), Some(&0));
    }
}
