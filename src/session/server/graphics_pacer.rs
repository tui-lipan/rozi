//! Presentation-paced credits for proven independent graphics streams.
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::session::protocol::OutputClass;

const MAXIMUM_HOLD: Duration = Duration::from_millis(200);
const ANSWER_WINDOW: Duration = Duration::from_millis(50);
const DETECTION_WINDOW: Duration = Duration::from_millis(33);
const DETECTION_FRAMES: u32 = 8;
const PRESENTATION_WINDOW: usize = 2;
const HISTORY_LIMIT: usize = 256;
/// Bytes one credit may drain: the framework's maximum. A hold leaves the producer's output
/// queued in the kernel oldest first, so a release takes the whole backlog and staging keeps only
/// the newest replacement. A one-frame credit would present the oldest queued frame instead.
const DRAIN_BYTES: usize = 64 * 1024;

/// How long before the presenter's next frame a release drains the producer: enough for the
/// output to reach and be drawn by the client (about 2 ms measured locally), and short enough
/// that what it drains is still current.
fn release_lead(interval: Duration) -> Duration {
    Duration::from_millis(3).min(interval)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Presenter {
    pub client: u64,
    pub frames_per_second: u16,
    pub presented: u64,
}

#[derive(Default)]
pub(super) struct GraphicsPacer {
    candidate: Option<(Instant, u32)>,
    last_observation: Option<Instant>,
    streaming: bool,
    pending: VecDeque<(u64, (u32, u64))>,
    presenter: Option<Presenter>,
    deadline: Option<Instant>,
    answer_until: Option<Instant>,
    read_in_flight: bool,
    continuation: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReadPolicy {
    Open,
    Unchanged,
    Credit { bytes: usize, available: bool },
}

impl GraphicsPacer {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn input(&mut self, now: Instant) {
        self.answer_until = Some(now + ANSWER_WINDOW);
        self.read_in_flight = false;
    }

    /// Record one output transaction. `whole_frame` says the producer replaced an image whole,
    /// which a dependent transaction can still be when it relays only what changed.
    pub fn observe(&mut self, now: Instant, sequence: u64, class: OutputClass, whole_frame: bool) {
        let (OutputClass::IndependentGraphics {
            image, incarnation, ..
        }
        | OutputClass::DependentGraphics {
            image, incarnation, ..
        }) = class
        else {
            let answer_until = self.answer_until;
            self.reset();
            self.answer_until = answer_until;
            return;
        };
        if !whole_frame {
            let answer_until = self.answer_until;
            self.reset();
            self.answer_until = answer_until;
            return;
        }
        if self
            .last_observation
            .is_some_and(|last| now.saturating_duration_since(last) > MAXIMUM_HOLD)
        {
            let answer_until = self.answer_until;
            self.reset();
            self.answer_until = answer_until;
        }
        self.last_observation = Some(now);
        let (since, count) = self.candidate.get_or_insert((now, 0));
        *count = count.saturating_add(1);
        self.streaming |=
            *count >= DETECTION_FRAMES && now.saturating_duration_since(*since) >= DETECTION_WINDOW;
        self.pending.push_back((sequence, (image, incarnation)));
        if self.pending.len() > HISTORY_LIMIT {
            self.pending.pop_front();
        }
        self.continuation = false;
    }

    pub fn read_completed(&mut self, continuation: bool) {
        self.read_in_flight = false;
        self.continuation = continuation;
    }

    pub fn policy(&mut self, now: Instant, presenter: Option<Presenter>) -> ReadPolicy {
        if !self.streaming || self.answer_until.is_some_and(|until| now < until) {
            return ReadPolicy::Open;
        }
        if self.presenter.map(|p| (p.client, p.frames_per_second))
            != presenter.map(|p| (p.client, p.frames_per_second))
        {
            // Old consumers may never ACK their last frames. A new leader must be able to
            // receive fresh independent state immediately, rather than waiting for safety credit.
            self.pending.clear();
            self.deadline = Some(now);
            self.read_in_flight = false;
        }
        let interval = Duration::from_secs_f64(
            1.0 / f64::from(presenter.map_or(15, |p| p.frames_per_second.max(1))),
        );
        if let (Some(previous), Some(current)) = (self.presenter, presenter)
            && previous.client == current.client
            && current.presented > previous.presented
        {
            // The presenter has just shown a frame, and its next one is about an interval away.
            // Releasing on that phase, rather than on an unrelated grid, hands it fresh output
            // just in time instead of up to a frame early.
            self.deadline = Some(now + interval.saturating_sub(release_lead(interval)));
        }
        self.presenter = presenter;
        if let Some(presenter) = presenter {
            while self
                .pending
                .front()
                .is_some_and(|(sequence, _)| *sequence <= presenter.presented)
            {
                self.pending.pop_front();
            }
        }
        let deadline = *self.deadline.get_or_insert(now);
        let ahead = presenter.is_some() && self.frames_ahead() >= PRESENTATION_WINDOW;
        let due = now >= deadline && !ahead;
        if self.read_in_flight {
            return ReadPolicy::Unchanged;
        }
        let available = due || self.continuation;
        if available {
            self.read_in_flight = true;
            if due {
                // Advance the existing phase, skipping missed slots rather than accumulating drift.
                let slots =
                    now.saturating_duration_since(deadline).as_nanos() / interval.as_nanos() + 1;
                self.deadline =
                    Some(deadline + interval.saturating_mul(slots.min(u32::MAX as u128) as u32));
            }
        }
        ReadPolicy::Credit {
            bytes: DRAIN_BYTES,
            available,
        }
    }

    pub fn frames_ahead(&self) -> usize {
        let mut counts = HashMap::new();
        self.pending
            .iter()
            .map(|(_, source)| {
                let count = counts.entry(source).or_insert(0);
                *count += 1;
                *count
            })
            .max()
            .unwrap_or(0)
    }

    pub fn wake_after(&self, now: Instant) -> Option<Duration> {
        if self.read_in_flight {
            return None;
        }
        self.streaming.then(|| {
            self.answer_until
                .filter(|until| *until > now)
                .or(self.deadline)
                .map_or(MAXIMUM_HOLD, |deadline| {
                    deadline.saturating_duration_since(now)
                })
                .min(MAXIMUM_HOLD)
                .max(Duration::from_millis(1))
        })
    }
}

pub(super) fn apply_policy(gate: &tui_lipan::TerminalReadGate, policy: ReadPolicy) {
    match policy {
        ReadPolicy::Open => gate.resume(),
        ReadPolicy::Unchanged => {}
        ReadPolicy::Credit { bytes, available } => gate.set_credit(bytes, available, MAXIMUM_HOLD),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_inflight_read_needs_no_polling_deadline() {
        let now = Instant::now();
        let mut pacer = streaming(now - Duration::from_secs(1));
        assert!(matches!(
            pacer.policy(now, None),
            ReadPolicy::Credit {
                available: true,
                ..
            }
        ));
        assert_eq!(pacer.wake_after(now + Duration::from_secs(1)), None);
    }

    #[test]
    fn ordered_output_preserves_the_input_answer_window() {
        let now = Instant::now();
        let mut pacer = streaming(now - Duration::from_secs(1));
        pacer.input(now);
        pacer.observe(now, 9, OutputClass::Ordered, false);
        for sequence in 10..18 {
            pacer.observe(
                now + Duration::from_millis((sequence - 10) * 5),
                sequence,
                independent(),
                true,
            );
        }
        assert_eq!(
            pacer.policy(now + Duration::from_millis(40), None),
            ReadPolicy::Open
        );
    }

    fn independent() -> OutputClass {
        OutputClass::IndependentGraphics {
            image: 7,
            incarnation: 1,
            revision: 1,
        }
    }

    fn streaming(now: Instant) -> GraphicsPacer {
        let mut pacer = GraphicsPacer::default();
        for n in 0..8 {
            pacer.observe(
                now + Duration::from_millis(n * 5),
                n + 1,
                independent(),
                true,
            );
        }
        pacer
    }

    fn dependent() -> OutputClass {
        OutputClass::DependentGraphics {
            image: 7,
            incarnation: 1,
            revision: 2,
        }
    }

    #[test]
    fn ordered_and_dependent_output_immediately_remove_pacing() {
        for class in [OutputClass::Ordered, dependent()] {
            let now = Instant::now();
            let mut pacer = streaming(now);
            pacer.observe(now + Duration::from_secs(1), 9, class, false);
            assert_eq!(
                pacer.policy(now + Duration::from_secs(1), None),
                ReadPolicy::Open
            );
        }
    }

    #[test]
    fn whole_frames_relayed_as_edits_are_paced_like_independent_ones() {
        let now = Instant::now();
        let mut pacer = GraphicsPacer::default();
        for n in 0..8 {
            pacer.observe(now + Duration::from_millis(n * 5), n + 1, dependent(), true);
        }
        assert!(matches!(
            pacer.policy(now + Duration::from_millis(40), None),
            ReadPolicy::Credit { .. }
        ));
        assert!(pacer.frames_ahead() > 0);
    }

    #[test]
    fn presentation_window_and_input_answer_window() {
        let now = Instant::now();
        let mut pacer = streaming(now);
        let presenter = Some(Presenter {
            client: 1,
            frames_per_second: 120,
            presented: 6,
        });
        pacer.presenter = presenter;
        pacer.deadline = Some(now);
        assert_eq!(
            pacer.policy(now + Duration::from_secs(1), presenter),
            ReadPolicy::Credit {
                bytes: DRAIN_BYTES,
                available: false
            }
        );
        pacer.input(now + Duration::from_secs(1));
        assert_eq!(
            pacer.policy(now + Duration::from_millis(1049), presenter),
            ReadPolicy::Open
        );
        assert!(matches!(
            pacer.policy(now + Duration::from_millis(1051), presenter),
            ReadPolicy::Credit {
                available: false,
                ..
            }
        ));
        // Catching up re-phases the next release to just before the presenter's next frame.
        let caught_up = Some(Presenter {
            presented: 8,
            client: 1,
            frames_per_second: 120,
        });
        let interval = Duration::from_secs_f64(1.0 / 120.0);
        let acknowledged = now + Duration::from_millis(1051);
        assert!(matches!(
            pacer.policy(acknowledged, caught_up),
            ReadPolicy::Credit {
                available: false,
                ..
            }
        ));
        assert!(matches!(
            pacer.policy(acknowledged + interval - release_lead(interval), caught_up),
            ReadPolicy::Credit {
                available: true,
                ..
            }
        ));
    }

    #[test]
    fn frame_rates_keep_phase_and_never_grant_repeated_inflight_credit() {
        for fps in [60, 120, 144, 240] {
            let start = Instant::now();
            let mut pacer = streaming(start - Duration::from_secs(1));
            let interval = Duration::from_secs_f64(1.0 / f64::from(fps));
            let presenter = Some(Presenter {
                client: 1,
                frames_per_second: fps,
                presented: 8,
            });
            for slot in 0..1000 {
                let now = start + interval * slot;
                assert!(matches!(
                    pacer.policy(now, presenter),
                    ReadPolicy::Credit {
                        available: true,
                        ..
                    }
                ));
                assert_eq!(pacer.policy(now, presenter), ReadPolicy::Unchanged);
                pacer.read_completed(false);
            }
        }
    }

    #[test]
    fn releases_follow_the_presenter_phase_after_each_acknowledgement() {
        let start = Instant::now();
        let mut pacer = streaming(start - Duration::from_secs(1));
        let presenter = |presented| {
            Some(Presenter {
                client: 1,
                frames_per_second: 60,
                presented,
            })
        };
        assert!(matches!(
            pacer.policy(start, presenter(8)),
            ReadPolicy::Credit {
                available: true,
                ..
            }
        ));
        pacer.read_completed(false);
        // Acknowledged 7 ms into the old grid's slot: the next release moves to that phase.
        let acknowledged = start + Duration::from_millis(7);
        let next = acknowledged + Duration::from_secs_f64(1.0 / 60.0)
            - release_lead(Duration::from_secs_f64(1.0 / 60.0));
        assert!(matches!(
            pacer.policy(acknowledged, presenter(9)),
            ReadPolicy::Credit {
                available: false,
                ..
            }
        ));
        assert!(matches!(
            pacer.policy(next - Duration::from_micros(100), presenter(9)),
            ReadPolicy::Credit {
                available: false,
                ..
            }
        ));
        assert!(matches!(
            pacer.policy(next, presenter(9)),
            ReadPolicy::Credit {
                available: true,
                ..
            }
        ));
    }

    #[test]
    fn hidden_stream_uses_background_cadence_and_partial_reads_can_finish() {
        let now = Instant::now();
        let mut pacer = streaming(now - Duration::from_secs(1));
        assert!(matches!(
            pacer.policy(now, None),
            ReadPolicy::Credit {
                available: true,
                ..
            }
        ));
        pacer.read_completed(false);
        assert!(matches!(
            pacer.policy(now + Duration::from_millis(20), None),
            ReadPolicy::Credit {
                available: false,
                ..
            }
        ));
        pacer.read_completed(true);
        assert!(matches!(
            pacer.policy(now + Duration::from_millis(20), None),
            ReadPolicy::Credit {
                available: true,
                ..
            }
        ));
        pacer.read_completed(false);
        assert!(matches!(
            pacer.policy(now + Duration::from_millis(67), None),
            ReadPolicy::Credit {
                available: true,
                ..
            }
        ));
    }
}
