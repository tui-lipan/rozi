//! Flow control for pointer motion forwarded to a pane.
//!
//! Coalescing decides *how many* positions leave the client: a burst of pointer reports collapses
//! to the newest one before it is dispatched. What it cannot decide is how many the child can take.
//! Those are different questions, and the second one has no answer anywhere in the pipeline - so a
//! child is handed a position per loop iteration whether or not it has finished the last one.
//!
//! For a child that draws a frame per position that is the difference between tracking the hand and
//! trailing it. Handed 38 positions during a 150 ms drag, one costing 16 ms to answer, it spends
//! 600 ms working through them and draws 30 of its frames *after* the hand has stopped - each at a
//! position the pointer left long ago. The gap grows with the speed and the length of the drag and
//! closes only when the drag ends, which is exactly what "the grip ends up above the pointer and
//! stays there" describes.
//!
//! So motion waits its turn: one report is in flight at a time, the newest arrival replaces
//! whatever was waiting, and the child's own output releases the next one. The child's output is
//! the only honest signal that it has moved on - it announces nothing else, and asking it would be
//! a protocol it does not speak.
//!
//! Two things are deliberately not held back:
//!
//! - **Anything that is not motion.** A press, a release and a wheel notch each mean something on
//!   their own and cannot be superseded by a later report. They go immediately, behind whatever
//!   motion was waiting, so the child still learns where the pointer was when the button moved.
//! - **Motion from a child that answers nothing.** A pane that ignores mouse reports entirely would
//!   otherwise hold the first one forever and never see another. After [`STALL`] the gate opens
//!   regardless, which degrades to forwarding everything - the behavior this replaces.

use std::time::{Duration, Instant};

/// How long a forwarded report waits for an answer before motion is let through anyway.
///
/// Long enough that a child drawing an ordinary frame is never rushed, short enough that a child
/// which answers nothing is not left with a frozen pointer for a visible interval.
const STALL: Duration = Duration::from_millis(100);

/// Whether an outgoing mouse report describes pointer *motion* rather than a state change.
///
/// Only the SGR spellings are inspected, which is not the limitation it looks like: motion is
/// reported at all only under the modes that use them. In both, bit 5 of the button code marks a
/// motion report, which covers a drag (32, 33, 34) and a bare move (35) while leaving presses,
/// releases and wheel notches (64, 65) alone.
fn is_motion_report(bytes: &[u8]) -> bool {
    let Some(rest) = bytes.strip_prefix(b"\x1b[<") else {
        return false;
    };
    let digits = rest.iter().take_while(|byte| byte.is_ascii_digit());
    let mut code: u32 = 0;
    let mut any = false;
    for digit in digits {
        any = true;
        code = code.saturating_mul(10).saturating_add(u32::from(digit - b'0'));
    }
    any && code & 0x20 != 0
}

/// Per-pane gate on forwarded pointer motion. See the module comment.
#[derive(Debug, Default)]
pub(crate) struct PointerFlow {
    /// A motion report has gone to the child, which has not produced output since.
    in_flight: Option<Instant>,
    /// The newest motion report, waiting for the child to answer the one before it.
    waiting: Option<Vec<u8>>,
}

impl PointerFlow {
    /// What to forward now for a report the client wants to send, if anything.
    pub(crate) fn admit(&mut self, bytes: Vec<u8>) -> Option<Vec<u8>> {
        if !is_motion_report(&bytes) {
            let mut out = self.waiting.take().unwrap_or_default();
            out.extend_from_slice(&bytes);
            self.in_flight = None;
            return Some(out);
        }
        if self.in_flight.is_some_and(|at| at.elapsed() < STALL) {
            // Replaced rather than queued: the child wants where the pointer is, not everywhere it
            // has been. This is the position that must survive, so it is the one kept.
            self.waiting = Some(bytes);
            return None;
        }
        self.in_flight = Some(Instant::now());
        Some(bytes)
    }

    /// The child produced output, so it has moved on from whatever it was given.
    pub(crate) fn answered(&mut self) -> Option<Vec<u8>> {
        self.in_flight = None;
        let waiting = self.waiting.take()?;
        self.in_flight = Some(Instant::now());
        Some(waiting)
    }

    /// Drop anything held for a pane that is going away or being re-bound.
    pub(crate) fn reset(&mut self) {
        self.in_flight = None;
        self.waiting = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drag(y: u16) -> Vec<u8> {
        format!("\x1b[<32;10;{y}M").into_bytes()
    }

    #[test]
    fn motion_is_recognised_and_button_reports_are_not() {
        assert!(is_motion_report(b"\x1b[<32;10;20M"), "drag");
        assert!(is_motion_report(b"\x1b[<35;10;20M"), "bare move");
        assert!(!is_motion_report(b"\x1b[<0;10;20M"), "press");
        assert!(!is_motion_report(b"\x1b[<0;10;20m"), "release");
        assert!(!is_motion_report(b"\x1b[<64;10;20M"), "wheel up");
        assert!(!is_motion_report(b"\x1b[<65;10;20M"), "wheel down");
        assert!(!is_motion_report(b"\x1b[M abc"), "x10 encoding reports no motion");
        assert!(!is_motion_report(b""), "nothing at all");
    }

    #[test]
    fn one_position_is_in_flight_and_the_newest_waits() {
        let mut flow = PointerFlow::default();
        assert_eq!(flow.admit(drag(1)), Some(drag(1)), "nothing outstanding");
        assert_eq!(flow.admit(drag(2)), None, "held behind the first");
        assert_eq!(flow.admit(drag(3)), None, "supersedes the held one");
        // The newest is what the child gets next: the positions in between are where the pointer
        // was, and drawing them is the lag this exists to remove.
        assert_eq!(flow.answered(), Some(drag(3)));
        assert_eq!(flow.answered(), None, "nothing left waiting");
    }

    #[test]
    fn a_button_report_goes_immediately_and_carries_held_motion_with_it() {
        let mut flow = PointerFlow::default();
        flow.admit(drag(1));
        assert_eq!(flow.admit(drag(2)), None);
        let release = b"\x1b[<0;10;9m".to_vec();
        let mut expected = drag(2);
        expected.extend_from_slice(&release);
        assert_eq!(
            flow.admit(release),
            Some(expected),
            "the release must not overtake the position it happened at"
        );
        assert_eq!(flow.answered(), None, "nothing is still waiting");
    }

    #[test]
    fn a_child_that_answers_nothing_stops_holding_the_pointer() {
        let mut flow = PointerFlow::default();
        assert_eq!(flow.admit(drag(1)), Some(drag(1)));
        assert_eq!(flow.admit(drag(2)), None);
        flow.in_flight = Some(Instant::now() - STALL - Duration::from_millis(1));
        assert_eq!(
            flow.admit(drag(3)),
            Some(drag(3)),
            "the gate opens rather than freezing a pane that never answers"
        );
    }
}
