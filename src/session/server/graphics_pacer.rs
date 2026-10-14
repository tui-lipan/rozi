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
    sources: HashMap<(u32, u64), usize>,
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

    pub fn observe(&mut self, now: Instant, sequence: u64, class: OutputClass, bytes: usize) {
        let OutputClass::IndependentGraphics {
            image, incarnation, ..
        } = class
        else {
            self.reset();
            return;
        };
        if self
            .last_observation
            .is_some_and(|last| now.saturating_duration_since(last) > MAXIMUM_HOLD)
        {
            self.reset();
        }
        self.last_observation = Some(now);
        let (since, count) = self.candidate.get_or_insert((now, 0));
        *count = count.saturating_add(1);
        self.streaming |=
            *count >= DETECTION_FRAMES && now.saturating_duration_since(*since) >= DETECTION_WINDOW;
        self.sources.insert((image, incarnation), bytes.max(1));
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
        let interval = Duration::from_secs_f64(
            1.0 / f64::from(presenter.map_or(15, |p| p.frames_per_second.max(1))),
        );
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
            bytes: self
                .sources
                .values()
                .copied()
                .fold(0usize, usize::saturating_add)
                .clamp(1, 8192),
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
                180,
            );
        }
        pacer
    }

    #[test]
    fn ordered_and_dependent_output_immediately_remove_pacing() {
        for class in [
            OutputClass::Ordered,
            OutputClass::DependentGraphics {
                image: 7,
                incarnation: 1,
                revision: 2,
            },
        ] {
            let now = Instant::now();
            let mut pacer = streaming(now);
            pacer.observe(now + Duration::from_secs(1), 9, class, 200);
            assert_eq!(
                pacer.policy(now + Duration::from_secs(1), None),
                ReadPolicy::Open
            );
        }
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
                bytes: 180,
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
        assert!(matches!(
            pacer.policy(
                now + Duration::from_millis(1051),
                Some(Presenter {
                    presented: 8,
                    client: 1,
                    frames_per_second: 120
                })
            ),
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
