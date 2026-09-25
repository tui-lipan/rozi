//! When each frame of a recording is shown during playback, from any starting point.
//!
//! Kept apart from the terminal that draws it, so the timing can be tested without sleeping.

use std::collections::VecDeque;
use std::io::BufRead;

use tui_lipan::CapturedFrame;

use super::read::{Replay, ReplayStep};
use crate::control::SpanFrame;

/// Where playback starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seek {
    /// This many milliseconds into the recording.
    Time(u64),
    /// At the first mark with this label.
    Mark(String),
}

/// One thing playback does. `at` and `until` are milliseconds of recording time after the point
/// playback started from; divide by the speed for wall-clock time.
#[derive(Clone, Debug, PartialEq)]
pub enum Cue {
    /// Show `frame` from `at` on.
    Show { at: u64, frame: CapturedFrame },
    /// Keep showing the last frame until `until`, where the recording ends.
    Hold { until: u64 },
}

/// A recording played from a starting point, cue by cue.
///
/// Starting from a time or a mark shows the screen as it was at that moment, which is the last
/// frame written at or before it, and every frame after it at its own time. Playback lasts until
/// the recording's end, however long its last screen stood still.
pub struct Playback<R> {
    replay: Replay<R>,
    seek: Option<Seek>,
    /// The recording time playback started from, once reached.
    start: Option<u64>,
    /// The latest frame before the starting point, while it has not been reached.
    before: Option<SpanFrame>,
    cues: VecDeque<Cue>,
    finished: bool,
}

impl<R: BufRead> Playback<R> {
    pub fn new(replay: Replay<R>, seek: Option<Seek>) -> Self {
        Self {
            replay,
            start: match seek {
                None | Some(Seek::Time(0)) => Some(0),
                Some(_) => None,
            },
            seek,
            before: None,
            cues: VecDeque::new(),
            finished: false,
        }
    }

    /// The next cue, or `None` once the recording has been played to its end.
    pub fn next_cue(&mut self) -> Result<Option<Cue>, String> {
        loop {
            if let Some(cue) = self.cues.pop_front() {
                return Ok(Some(cue));
            }
            if self.finished {
                return Ok(None);
            }
            let Some(step) = self.replay.step()? else {
                self.finish(None)?;
                continue;
            };
            match step {
                ReplayStep::Frame { t } => self.frame(t)?,
                ReplayStep::Mark { t, label } => {
                    if self.start.is_none()
                        && matches!(&self.seek, Some(Seek::Mark(want)) if *want == label)
                    {
                        self.begin(t)?;
                    }
                }
                ReplayStep::End(end) => self.finish(Some(end.t))?,
                ReplayStep::Meta { .. } => {}
            }
        }
    }

    fn frame(&mut self, t: u64) -> Result<(), String> {
        if let Some(start) = self.start {
            let frame = self
                .replay
                .frame()
                .cloned()
                .expect("a frame step has a frame");
            let frame = self.replay.captured(&frame)?;
            self.cues.push_back(Cue::Show {
                at: t.saturating_sub(start),
                frame,
            });
            return Ok(());
        }
        if let Some(Seek::Time(at)) = self.seek
            && t > at
        {
            // The first frame after the starting point: the one before it is what showed there.
            self.begin(at)?;
            return self.frame(t);
        }
        self.before = self.replay.frame().cloned();
        Ok(())
    }

    /// Start playing from recording time `t`, showing the screen as it was then.
    fn begin(&mut self, t: u64) -> Result<(), String> {
        self.start = Some(t);
        if let Some(frame) = self.before.take() {
            let frame = self.replay.captured(&frame)?;
            self.cues.push_back(Cue::Show { at: 0, frame });
        }
        Ok(())
    }

    fn finish(&mut self, end: Option<u64>) -> Result<(), String> {
        self.finished = true;
        let end = end.unwrap_or_else(|| self.replay.elapsed());
        if self.start.is_none() {
            match &self.seek {
                Some(Seek::Time(at)) if *at <= end => self.begin(*at)?,
                Some(Seek::Time(at)) => {
                    return Err(format!(
                        "the recording is {:.1}s long, shorter than {:.1}s",
                        end as f64 / 1000.0,
                        *at as f64 / 1000.0
                    ));
                }
                Some(Seek::Mark(label)) => {
                    return Err(format!("the recording has no mark named {label:?}"));
                }
                None => unreachable!("playback from the start has begun"),
            }
        }
        let start = self.start.expect("begun above");
        self.cues.push_back(Cue::Hold {
            until: end.saturating_sub(start),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::{EndReason, Recorder, RecorderOptions, RecordingHeader};
    use tui_lipan::prelude::*;

    /// A recording showing `a` from 0, `b` from 1s, marked `m` at 0.6s, and ending at 10s.
    fn recording() -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("play.rozirec");
        let mut screen = TerminalScreen::new(2, 10, 0);
        let palette =
            crate::pane::spans::span_frame(&screen.capture_frame(), screen.palette(), false)
                .unwrap()
                .palette;
        let recorder = Recorder::start(RecorderOptions {
            path: path.clone(),
            overwrite: false,
            header: RecordingHeader {
                format: crate::recording::RECORDING_FORMAT.into(),
                version: crate::recording::RECORDING_VERSION,
                rozi: "test".into(),
                target: crate::recording::RecordingTarget::Pane {
                    session: "dev".into(),
                    pane: 1,
                },
                width: 10,
                height: 2,
                started_at_unix_ms: 0,
                max_fps: 30,
                keyframe_interval_ms: 10_000,
                spans_version: crate::control::SPAN_FRAME_VERSION,
                palette,
                compression: None,
            },
            max_bytes: u64::MAX,
        })
        .unwrap();
        screen.process_bytes(b"a");
        assert!(recorder.push_frame(0, screen.capture_frame(), screen.palette()));
        assert!(recorder.mark(600, "m".into()));
        screen.process_bytes(b"\rb");
        assert!(recorder.push_frame(1_000, screen.capture_frame(), screen.palette()));
        recorder.finish(10_000, EndReason::Stopped);
        recorder
            .join(std::time::Duration::from_secs(10))
            .expect("the writer finished");
        std::fs::read(path).unwrap()
    }

    fn cues(seek: Option<Seek>) -> std::result::Result<Vec<String>, String> {
        let bytes = recording();
        let mut playback = Playback::new(Replay::new(bytes.as_slice())?, seek);
        let mut out = Vec::new();
        while let Some(cue) = playback.next_cue()? {
            out.push(match cue {
                Cue::Show { at, frame } => format!("{at} {}", frame.plain_text().trim()),
                Cue::Hold { until } => format!("hold {until}"),
            });
        }
        Ok(out)
    }

    #[test]
    fn playback_shows_each_frame_at_its_time_and_holds_the_last_until_the_end() {
        assert_eq!(cues(None).unwrap(), ["0 a", "1000 b", "hold 10000"]);
    }

    #[test]
    fn a_time_starts_on_the_screen_shown_then() {
        assert_eq!(cues(Some(Seek::Time(5_000))).unwrap(), ["0 b", "hold 5000"]);
        assert_eq!(
            cues(Some(Seek::Time(500))).unwrap(),
            ["0 a", "500 b", "hold 9500"]
        );
        assert_eq!(cues(Some(Seek::Time(1_000))).unwrap(), ["0 b", "hold 9000"]);
        assert_eq!(cues(Some(Seek::Time(10_000))).unwrap(), ["0 b", "hold 0"]);
        let refused = cues(Some(Seek::Time(20_000))).unwrap_err();
        assert!(refused.contains("10.0s long"), "{refused}");
    }

    #[test]
    fn a_mark_starts_on_the_screen_shown_when_it_was_made() {
        assert_eq!(
            cues(Some(Seek::Mark("m".into()))).unwrap(),
            ["0 a", "400 b", "hold 9400"]
        );
        assert!(cues(Some(Seek::Mark("x".into()))).is_err());
    }
}
