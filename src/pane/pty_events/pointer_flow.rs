//! Flow control for pointer motion forwarded to a pane.
//!
//! A pixel-reporting host can produce hundreds or thousands of positions a second. Handing every
//! one to the child makes pointer motion drive the child's render loop above rozi's configured
//! frame rate: a fast hover can force 180 paints per second, and dragging a Chromium application
//! can saturate its main thread.
//!
//! Motion therefore shares the frame cadence. The first report goes immediately; until one frame
//! interval has elapsed, the newest position replaces whatever was waiting. A clock releases that
//! position, independently of child output. Output is not an acknowledgement: graphics programs
//! can write one visual frame in several PTY chunks, and treating each chunk as permission to send
//! another position defeats the cap.
//!
//! One thing is deliberately not held back:
//!
//! - **Anything that is not motion.** A press, a release and a wheel notch each mean something on
//!   their own and cannot be superseded by a later report. They go immediately, behind whatever
//!   motion was waiting, so the child still learns where the pointer was when the button moved.

use std::time::{Duration, Instant};
use tui_lipan::core::event::MouseKind;
use tui_lipan::{ForwardedMouseEvent, MouseEncoding};

/// Match `tui-lipan`'s frame interval calculation without exposing framework runner internals.
pub(crate) fn interval_for_frame_rate(frame_rate: u16) -> Duration {
    Duration::from_micros(1_000_000 / u64::from(frame_rate.max(1))).max(Duration::from_millis(1))
}

/// What a cadence wakeup found for a pane whose motion was held.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Paced {
    /// The next frame interval has begun; forward this now.
    Send(Vec<u8>),
    /// The wakeup arrived early or the cadence changed. Ask again after this long.
    Retry(Duration),
    /// Nothing is waiting; a newer report or a state change overtook the wakeup.
    Idle,
}

/// Per-pane frame-rate gate on forwarded pointer motion. See the module comment.
#[derive(Debug, Default)]
pub(crate) struct PointerFlow {
    /// When the most recent motion report left the client.
    last_sent: Option<Instant>,
    /// The newest motion report waiting for the next frame interval.
    waiting: Option<Vec<u8>>,
    /// A wakeup is already scheduled, so holding another report does not ask for a second one.
    armed: bool,
}

impl PointerFlow {
    /// What to forward now for a report the client wants to send, if anything.
    pub(crate) fn admit(
        &mut self,
        event: ForwardedMouseEvent,
        interval: Duration,
    ) -> Option<Vec<u8>> {
        // Preserve the SGR-only cadence policy. Legacy encodings continue to forward every
        // report immediately, including motion.
        let motion = matches!(event.kind, MouseKind::Moved | MouseKind::Drag(_))
            && matches!(
                event.mouse_mode.encoding,
                MouseEncoding::Sgr | MouseEncoding::SgrPixels
            );
        let bytes = event.bytes;
        if !motion {
            let mut out = self.waiting.take().unwrap_or_default();
            out.extend_from_slice(&bytes);
            // State changes begin a new gesture. Let its first motion report through immediately;
            // the press/release itself is the ordering boundary, not part of the motion budget.
            self.last_sent = None;
            return Some(out);
        }
        if self.last_sent.is_some_and(|at| at.elapsed() < interval) {
            // Replaced rather than queued: the child wants where the pointer is, not everywhere
            // it has been.
            self.waiting = Some(bytes);
            return None;
        }
        // A timer can arrive after a newer report has already opened this interval. Clear the old
        // waiting position so that stale wakeup becomes inert.
        self.waiting = None;
        self.last_sent = Some(Instant::now());
        Some(bytes)
    }

    /// How long to wait before asking this pane again, for a report that was just held.
    ///
    /// `None` when nothing is waiting or a wakeup is already outstanding, so a burst of motion
    /// asks for one timer rather than one per report.
    pub(crate) fn arm(&mut self, interval: Duration) -> Option<Duration> {
        if self.armed || self.waiting.is_none() {
            return None;
        }
        self.armed = true;
        Some(
            self.last_sent
                .map(|at| interval.saturating_sub(at.elapsed()))
                .unwrap_or_default(),
        )
    }

    /// Answer a wakeup: forward the held report when its frame interval begins.
    pub(crate) fn paced(&mut self, interval: Duration) -> Paced {
        self.armed = false;
        if self.waiting.is_none() {
            return Paced::Idle;
        }
        if let Some(at) = self.last_sent {
            let waited = at.elapsed();
            if waited < interval {
                self.armed = true;
                return Paced::Retry(interval - waited);
            }
        }
        let bytes = self.waiting.take().expect("checked above");
        self.last_sent = Some(Instant::now());
        Paced::Send(bytes)
    }

    /// Drop anything held for a pane that is going away or being re-bound.
    pub(crate) fn reset(&mut self) {
        self.last_sent = None;
        self.waiting = None;
        self.armed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui_lipan::core::event::MouseButton;

    const FRAME: Duration = Duration::from_millis(8);

    fn drag(y: u16) -> Vec<u8> {
        format!("opaque position {y}").into_bytes()
    }

    fn admit(
        flow: &mut PointerFlow,
        kind: MouseKind,
        bytes: Vec<u8>,
        interval: Duration,
    ) -> Option<Vec<u8>> {
        flow.admit(crate::test_support::forwarded_mouse(kind, bytes), interval)
    }

    #[test]
    fn legacy_motion_remains_immediate_and_state_changes_flush_pending_motion() {
        for encoding in [MouseEncoding::X10, MouseEncoding::Utf8] {
            let mut flow = PointerFlow::default();
            for position in 0..3 {
                let mut event =
                    crate::test_support::forwarded_mouse(MouseKind::Moved, drag(position));
                event.mouse_mode.encoding = encoding;
                assert_eq!(flow.admit(event, FRAME), Some(drag(position)));
            }
        }
        for kind in [
            MouseKind::Down(MouseButton::Right),
            MouseKind::Up(MouseButton::Middle),
            MouseKind::ScrollUp,
            MouseKind::ScrollDown,
        ] {
            let mut flow = PointerFlow::default();
            admit(&mut flow, MouseKind::Moved, drag(1), FRAME);
            assert_eq!(admit(&mut flow, MouseKind::Moved, drag(2), FRAME), None);
            let bytes = b"opaque state change".to_vec();
            let mut expected = drag(2);
            expected.extend_from_slice(&bytes);
            assert_eq!(admit(&mut flow, kind, bytes, FRAME), Some(expected));
        }
    }

    #[test]
    fn motion_is_capped_to_one_position_per_frame_and_the_newest_waits() {
        let mut flow = PointerFlow::default();
        assert_eq!(
            admit(
                &mut flow,
                MouseKind::Drag(MouseButton::Left),
                drag(1),
                FRAME
            ),
            Some(drag(1)),
            "first position is immediate"
        );
        assert_eq!(
            admit(
                &mut flow,
                MouseKind::Drag(MouseButton::Left),
                drag(2),
                FRAME
            ),
            None,
            "held until next frame"
        );
        assert_eq!(
            admit(
                &mut flow,
                MouseKind::Drag(MouseButton::Left),
                drag(3),
                FRAME
            ),
            None,
            "supersedes the held one"
        );
        flow.last_sent = Some(Instant::now() - FRAME - Duration::from_millis(1));
        assert_eq!(flow.paced(FRAME), Paced::Send(drag(3)));
        assert_eq!(flow.paced(FRAME), Paced::Idle, "nothing left waiting");
    }

    #[test]
    fn a_button_report_goes_immediately_and_carries_held_motion_with_it() {
        let mut flow = PointerFlow::default();
        admit(
            &mut flow,
            MouseKind::Drag(MouseButton::Left),
            drag(1),
            FRAME,
        );
        assert_eq!(
            admit(
                &mut flow,
                MouseKind::Drag(MouseButton::Left),
                drag(2),
                FRAME
            ),
            None
        );
        let release = b"opaque release".to_vec();
        let mut expected = drag(2);
        expected.extend_from_slice(&release);
        assert_eq!(
            admit(&mut flow, MouseKind::Up(MouseButton::Left), release, FRAME),
            Some(expected),
            "the release must not overtake the position it happened at"
        );
        assert_eq!(flow.paced(FRAME), Paced::Idle, "nothing is still waiting");
    }

    #[test]
    fn the_last_position_is_delivered_on_the_next_frame_interval() {
        let mut flow = PointerFlow::default();
        assert_eq!(
            admit(
                &mut flow,
                MouseKind::Drag(MouseButton::Left),
                drag(1),
                FRAME
            ),
            Some(drag(1))
        );
        assert_eq!(
            admit(
                &mut flow,
                MouseKind::Drag(MouseButton::Left),
                drag(2),
                FRAME
            ),
            None,
            "held behind first"
        );
        assert!(
            flow.arm(FRAME).is_some_and(|after| after <= FRAME),
            "the hold asks for a wakeup within one frame"
        );
        assert_eq!(flow.arm(FRAME), None, "one timer covers a whole burst");

        match flow.paced(FRAME) {
            Paced::Retry(after) => assert!(after <= FRAME),
            other => panic!("expected a retry, got {other:?}"),
        }
        assert_eq!(flow.arm(FRAME), None, "the retry re-armed it");

        flow.last_sent = Some(Instant::now() - FRAME - Duration::from_millis(1));
        assert_eq!(flow.paced(FRAME), Paced::Send(drag(2)));
        assert_eq!(flow.paced(FRAME), Paced::Idle, "and nothing is left over");
    }

    #[test]
    fn a_wakeup_overtaken_by_a_new_frame_interval_does_nothing() {
        let mut flow = PointerFlow::default();
        admit(
            &mut flow,
            MouseKind::Drag(MouseButton::Left),
            drag(1),
            FRAME,
        );
        assert_eq!(
            admit(
                &mut flow,
                MouseKind::Drag(MouseButton::Left),
                drag(2),
                FRAME
            ),
            None
        );
        assert!(flow.arm(FRAME).is_some());
        flow.last_sent = Some(Instant::now() - FRAME - Duration::from_millis(1));
        assert_eq!(
            admit(
                &mut flow,
                MouseKind::Drag(MouseButton::Left),
                drag(3),
                FRAME
            ),
            Some(drag(3)),
            "new arrival opens the interval first"
        );
        assert_eq!(flow.paced(FRAME), Paced::Idle);
    }

    #[test]
    fn configured_frame_rate_sets_the_motion_cap() {
        assert_eq!(interval_for_frame_rate(120), Duration::from_micros(8_333));
        assert_eq!(interval_for_frame_rate(1_500), Duration::from_millis(1));
    }
}
