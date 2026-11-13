//! History compression for terminal screens whose output has gone quiet.
//!
//! Only the Ghostty engine compresses history (`TerminalScreen::compress_idle`); Alacritty never
//! schedules any of this. A screen becomes due [`IDLE_BEFORE_COMPRESSION`] after its last output
//! and is then stepped at most [`STEP_BUDGET`] per poll, every [`POLL_INTERVAL`], until the engine
//! reports nothing left. New output puts it back to waiting. The session server polls from its
//! loop and the UI client from a passive timer; neither touches the output path beyond noting the
//! time. Compression changes how history is stored, never what it holds, so neither side repaints
//! or re-exports anything for it.

use std::time::{Duration, Instant};

use tui_lipan::prelude::TerminalScreen;

pub(crate) const IDLE_BEFORE_COMPRESSION: Duration = Duration::from_secs(2);
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(250);
const STEP_BUDGET: Duration = Duration::from_millis(1);

/// Whether this process compresses at all: only on the Ghostty engine.
pub(crate) fn enabled() -> bool {
    super::engine::active() == super::engine::TerminalEngine::Ghostty
}

/// One bounded pass, returning whether another poll is needed.
pub(crate) fn step(screen: &mut TerminalScreen) -> bool {
    let started = Instant::now();
    let mut more = screen.compress_idle();
    while more && started.elapsed() < STEP_BUDGET {
        more = screen.compress_idle();
    }
    more
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
            && !step(screen)
        {
            self.last_output = None;
        }
    }
}
