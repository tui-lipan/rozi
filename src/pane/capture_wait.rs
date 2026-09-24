//! Deciding when a pane wait has resolved: a [`ScreenWait`] watches one pane's screen for the text
//! and the quiet a [`PaneWait`] asked for.
//!
//! The session server and the UI both answer `capture-pane --wait-for` and `send-keys --wait-for`,
//! each from its own copy of the pane's screen. Both feed the same evaluator here, so the two
//! endpoints cannot drift on what "appeared" or "settled" means; all either one owns is when to look
//! and where the reply goes.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tui_lipan::prelude::*;

use crate::control::{
    CaptureRender, CaptureScrollback, ControlCommand, ControlErrorCode, ControlResponse,
    PaneCapture, PaneWait,
};
use crate::state::PaneId;

/// A checked pane wait and what its reply carries, taken from the command that asked for it.
#[derive(Clone, Debug)]
pub(crate) struct WaitPlan {
    pub(crate) wait: PaneWait,
    /// The command's own target, before any caller default is applied.
    pub(crate) target: Option<PaneId>,
    /// Whether the wait is for the answer to input the command writes first.
    pub(crate) after_input: bool,
    pub(crate) reply: WaitReply,
}

/// The capture a wait's reply carries.
#[derive(Clone, Debug)]
pub(crate) struct WaitReply {
    pub(crate) scrollback: Option<CaptureScrollback>,
    pub(crate) render: CaptureRender,
    pub(crate) scale: Option<u8>,
    pub(crate) image_pixels: bool,
    /// Whether a resolved wait returns the capture. A failed one always does, when it can, since
    /// what the pane showed instead is the first thing anyone asks.
    pub(crate) on_success: bool,
}

impl WaitPlan {
    /// The wait `command` holds its reply for, checked; `Ok(None)` for a command without one.
    ///
    /// Everything the reply will need is checked here too, so a bad render or scale is refused at
    /// once rather than after the whole wait.
    pub(crate) fn for_command(
        command: &ControlCommand,
    ) -> std::result::Result<Option<Self>, ControlResponse> {
        let plan = match command {
            ControlCommand::CapturePane {
                target,
                scrollback,
                render,
                scale,
                image_pixels,
                wait: Some(wait),
            } => {
                super::check_capture(scrollback.as_ref(), *render, *scale, *image_pixels)?;
                Self {
                    wait: wait.clone(),
                    target: *target,
                    after_input: false,
                    reply: WaitReply {
                        scrollback: scrollback.clone(),
                        render: *render,
                        scale: *scale,
                        image_pixels: *image_pixels,
                        on_success: true,
                    },
                }
            }
            ControlCommand::SendText {
                target,
                wait,
                capture,
                scale,
                ..
            }
            | ControlCommand::SendKeys {
                target,
                wait,
                capture,
                scale,
                ..
            } => {
                let checked = crate::control::send_capture(wait.as_ref(), *capture, *scale)?;
                let Some(wait) = wait else {
                    return Ok(None);
                };
                Self {
                    wait: wait.clone(),
                    target: *target,
                    after_input: true,
                    reply: WaitReply {
                        scrollback: None,
                        render: checked.map_or(CaptureRender::Text, |(render, _)| render),
                        scale: checked.map(|(_, scale)| scale).filter(|&scale| scale != 1),
                        image_pixels: false,
                        on_success: checked.is_some(),
                    },
                }
            }
            _ => return Ok(None),
        };
        plan.wait.validate()?;
        Ok(Some(plan))
    }
}

/// How a wait ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WaitEnd {
    Ready,
    TimedOut,
    /// The pane's program exited, leaving its last screen behind.
    Exited,
    /// The pane is gone, or was replaced by another program.
    Closed,
}

impl WaitReply {
    /// The reply for a wait on pane `id` that ended as `end`. `capture` takes the pane's capture
    /// for this reply's render; it is not called for a pane that is gone.
    pub(crate) fn respond(
        &self,
        id: PaneId,
        wait: &PaneWait,
        end: WaitEnd,
        capture: impl FnOnce(&Self) -> std::result::Result<PaneCapture, ControlResponse>,
    ) -> ControlResponse {
        let (code, message) = match end {
            WaitEnd::Ready if self.on_success => {
                return capture(self).map_or_else(|response| response, ControlResponse::ok);
            }
            WaitEnd::Ready => return ControlResponse::empty(),
            WaitEnd::TimedOut => (
                ControlErrorCode::Timeout,
                format!(
                    "timed out after {}ms waiting for pane {id} {}",
                    wait.timeout_ms,
                    describe(wait)
                ),
            ),
            WaitEnd::Exited => (
                ControlErrorCode::PaneNotRunning,
                format!("pane {id} exited while waiting {}", describe(wait)),
            ),
            WaitEnd::Closed => {
                return ControlResponse::error_with(
                    ControlErrorCode::PaneNotRunning,
                    format!("pane {id} closed while waiting {}", describe(wait)),
                );
            }
        };
        let mut response = ControlResponse::error_with(code, message);
        response.data = capture(self)
            .ok()
            .and_then(|capture| serde_json::to_value(capture).ok());
        response
    }
}

/// What a wait was for, as the end of a sentence: "to show `ok`", "to settle for 300ms".
fn describe(wait: &PaneWait) -> String {
    match (&wait.text, wait.settle_ms) {
        (Some(text), Some(settle)) => format!("to show `{text}` and settle for {settle}ms"),
        (Some(text), None) => format!("to show `{text}`"),
        (None, Some(settle)) => format!("to settle for {settle}ms"),
        (None, None) => "for nothing".to_string(),
    }
}

/// Where a [`ScreenWait`] stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WaitStatus {
    Pending,
    Ready,
    TimedOut,
}

/// The rows a screen showed when input was written to it, so a match can tell the input's answer
/// from text that was already there.
///
/// Rows are keyed by absolute line - evictions plus retained history plus the row - so a row that
/// scrolls up keeps its key and still counts as old. A screen whose history was rewritten (a resize
/// that reflows it, a clear) changes `history_epoch`, and the keys stop meaning the same lines; the
/// baseline is dropped then rather than guessed at, and everything on screen counts as new.
#[derive(Debug)]
struct Baseline {
    history_epoch: u64,
    rows: HashMap<u64, String>,
}

/// One visible screen, as a wait reads it.
struct Look {
    cells: Vec<tui_lipan::CapturedCell>,
    rows: Vec<String>,
    first_line: u64,
    history_epoch: u64,
}

impl Look {
    /// The live screen, even while someone has scrolled the view into history.
    fn take(screen: &mut TerminalScreen) -> Self {
        let offset = screen.scrollback_offset();
        if offset != 0 {
            screen.set_scrollback(0);
        }
        let frame = screen.capture_frame();
        if offset != 0 {
            screen.set_scrollback(offset);
        }
        let lineage = screen.scrollback_lineage();
        let visible = usize::from(frame.height);
        let history = screen.total_text_lines().saturating_sub(visible) as u64;
        Self {
            rows: frame.to_fixed_grid_lines(),
            cells: frame.cells,
            first_line: lineage.evicted_lines.saturating_add(history),
            history_epoch: lineage.history_epoch,
        }
    }

    fn baseline(&self) -> Baseline {
        Baseline {
            history_epoch: self.history_epoch,
            rows: self
                .rows
                .iter()
                .enumerate()
                .map(|(row, text)| (self.first_line + row as u64, text.clone()))
                .collect(),
        }
    }

    /// Whether `text` is on a visible row, and not where the baseline already had it.
    fn shows(&self, text: &str, baseline: Option<&Baseline>) -> bool {
        let baseline = baseline.filter(|baseline| baseline.history_epoch == self.history_epoch);
        self.rows.iter().enumerate().any(|(row, line)| {
            let before = baseline.and_then(|b| b.rows.get(&(self.first_line + row as u64)));
            line.match_indices(text).any(|(at, _)| {
                !before.is_some_and(|before| before.get(at..).is_some_and(|b| b.starts_with(text)))
            })
        })
    }
}

/// A pane wait in progress: the conditions, the deadline, and what the screen has done so far.
pub(crate) struct ScreenWait {
    text: Option<String>,
    settle: Option<Duration>,
    deadline: Instant,
    baseline: Option<Baseline>,
    /// When `text` first showed up, or when the wait started if it has no text.
    matched_at: Option<Instant>,
    cells: Vec<tui_lipan::CapturedCell>,
    changed_at: Instant,
}

impl ScreenWait {
    /// Start waiting on `screen` as it is now, `now`. With `after_input`, text already on screen
    /// does not count; the wait is for what the input produces.
    ///
    /// The deadline runs from `requested`, when the caller asked. A send's baseline can come later
    /// than that, once its input is known to have landed; the quiet a settle waits for is only
    /// counted from the baseline, since nothing before it was watched.
    ///
    /// `wait` must already have passed [`PaneWait::validate`].
    pub(crate) fn start(
        wait: &PaneWait,
        screen: &mut TerminalScreen,
        after_input: bool,
        requested: Instant,
        now: Instant,
    ) -> Self {
        let look = Look::take(screen);
        let baseline = after_input.then(|| look.baseline());
        let text = wait.text.clone();
        let matched_at = match &text {
            None => Some(now),
            Some(text) => look.shows(text, baseline.as_ref()).then_some(now),
        };
        Self {
            text,
            settle: wait.settle_ms.map(Duration::from_millis),
            deadline: requested + Duration::from_millis(wait.timeout_ms),
            baseline,
            matched_at,
            cells: look.cells,
            changed_at: now,
        }
    }

    /// Look at `screen` again after it may have changed.
    pub(crate) fn observe(&mut self, screen: &mut TerminalScreen, now: Instant) {
        let look = Look::take(screen);
        if look.cells != self.cells {
            self.cells = look.cells.clone();
            self.changed_at = now;
        }
        if self.matched_at.is_none()
            && let Some(text) = &self.text
            && look.shows(text, self.baseline.as_ref())
        {
            self.matched_at = Some(now);
        }
    }

    /// Where the wait stands at `now`. A wait that resolves on its deadline counts as resolved;
    /// one that resolves any later has timed out, however soon it is looked at.
    pub(crate) fn status(&self, now: Instant) -> WaitStatus {
        let ready_at = self.matched_at.map(|matched_at| match self.settle {
            None => matched_at,
            Some(settle) => self.changed_at.max(matched_at) + settle,
        });
        match ready_at {
            Some(ready_at) if ready_at <= self.deadline && now >= ready_at => WaitStatus::Ready,
            _ if now >= self.deadline => WaitStatus::TimedOut,
            _ => WaitStatus::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> TerminalScreen {
        TerminalScreen::new(4, 40, 100)
    }

    fn wait(text: Option<&str>, settle_ms: Option<u64>) -> PaneWait {
        PaneWait {
            text: text.map(str::to_string),
            settle_ms,
            timeout_ms: 5_000,
        }
    }

    #[test]
    fn text_resolves_once_it_is_drawn() {
        let mut screen = screen();
        let now = Instant::now();
        let mut waiting =
            ScreenWait::start(&wait(Some("done"), None), &mut screen, false, now, now);
        assert_eq!(waiting.status(now), WaitStatus::Pending);

        screen.process_bytes(b"working\r\n");
        waiting.observe(&mut screen, now);
        assert_eq!(waiting.status(now), WaitStatus::Pending);

        screen.process_bytes(b"all done\r\n");
        waiting.observe(&mut screen, now);
        assert_eq!(waiting.status(now), WaitStatus::Ready);
    }

    #[test]
    fn a_capture_wait_takes_text_that_was_already_there() {
        let mut screen = screen();
        screen.process_bytes(b"ready> ");
        let now = Instant::now();
        let waiting = ScreenWait::start(&wait(Some("ready> "), None), &mut screen, false, now, now);
        assert_eq!(waiting.status(now), WaitStatus::Ready);
    }

    #[test]
    fn an_input_wait_ignores_text_already_on_screen_even_after_it_scrolls() {
        let mut screen = screen();
        screen.process_bytes(b"$ make\r\nok\r\n$ ");
        let now = Instant::now();
        let mut waiting = ScreenWait::start(&wait(Some("$ "), None), &mut screen, true, now, now);
        assert_eq!(waiting.status(now), WaitStatus::Pending);

        // The echo lands on the old prompt row, and output scrolls the old prompts up.
        screen.process_bytes(b"make\r\nbuilding\r\nstill building\r\n");
        waiting.observe(&mut screen, now);
        assert_eq!(waiting.status(now), WaitStatus::Pending);

        screen.process_bytes(b"$ ");
        waiting.observe(&mut screen, now);
        assert_eq!(waiting.status(now), WaitStatus::Ready);
    }

    #[test]
    fn an_input_wait_matches_new_text_on_a_row_that_already_had_some() {
        let mut screen = screen();
        screen.process_bytes(b"> ");
        let now = Instant::now();
        let mut waiting = ScreenWait::start(&wait(Some("pong"), None), &mut screen, true, now, now);
        screen.process_bytes(b"ping pong");
        waiting.observe(&mut screen, now);
        assert_eq!(waiting.status(now), WaitStatus::Ready);
    }

    #[test]
    fn settle_resolves_once_the_grid_stops_changing() {
        let mut screen = screen();
        let start = Instant::now();
        let ms = Duration::from_millis;
        let mut waiting =
            ScreenWait::start(&wait(None, Some(100)), &mut screen, false, start, start);
        assert_eq!(waiting.status(start + ms(99)), WaitStatus::Pending);

        screen.process_bytes(b"tick");
        waiting.observe(&mut screen, start + ms(80));
        assert_eq!(waiting.status(start + ms(150)), WaitStatus::Pending);

        // Repainting the same cells, or only moving the cursor, is not a change.
        screen.process_bytes(b"\rtick\x1b[1;1H");
        waiting.observe(&mut screen, start + ms(150));
        assert_eq!(waiting.status(start + ms(180)), WaitStatus::Ready);
    }

    #[test]
    fn text_then_settle_starts_the_quiet_clock_at_the_match() {
        let mut screen = screen();
        let start = Instant::now();
        let ms = Duration::from_millis;
        let mut waiting = ScreenWait::start(
            &wait(Some("done"), Some(100)),
            &mut screen,
            false,
            start,
            start,
        );
        assert_eq!(waiting.status(start + ms(500)), WaitStatus::Pending);

        screen.process_bytes(b"done");
        waiting.observe(&mut screen, start + ms(500));
        assert_eq!(waiting.status(start + ms(599)), WaitStatus::Pending);
        assert_eq!(waiting.status(start + ms(600)), WaitStatus::Ready);
    }

    #[test]
    fn a_wait_times_out_at_its_deadline() {
        let mut screen = screen();
        let start = Instant::now();
        let waiting =
            ScreenWait::start(&wait(Some("never"), None), &mut screen, false, start, start);
        assert_eq!(
            waiting.status(start + Duration::from_millis(4_999)),
            WaitStatus::Pending
        );
        assert_eq!(
            waiting.status(start + Duration::from_secs(5)),
            WaitStatus::TimedOut
        );
    }

    #[test]
    fn text_that_lands_on_the_deadline_resolves_and_any_later_times_out() {
        let ms = Duration::from_millis;
        for (lands, expected) in [
            (ms(5_000), WaitStatus::Ready),
            (ms(5_001), WaitStatus::TimedOut),
        ] {
            let mut screen = screen();
            let start = Instant::now();
            let mut waiting =
                ScreenWait::start(&wait(Some("done"), None), &mut screen, false, start, start);
            screen.process_bytes(b"done");
            waiting.observe(&mut screen, start + lands);
            assert_eq!(
                waiting.status(start + lands),
                expected,
                "landing at {lands:?}"
            );
        }
    }

    #[test]
    fn a_settle_that_completes_past_the_deadline_times_out() {
        let mut screen = screen();
        let start = Instant::now();
        let ms = Duration::from_millis;
        let mut waiting = ScreenWait::start(
            &wait(Some("done"), Some(100)),
            &mut screen,
            false,
            start,
            start,
        );
        screen.process_bytes(b"done");
        waiting.observe(&mut screen, start + ms(4_901));
        assert_eq!(waiting.status(start + ms(4_999)), WaitStatus::Pending);
        // The quiet would only be complete at 5001ms, so the deadline itself ends it.
        assert_eq!(waiting.status(start + ms(5_000)), WaitStatus::TimedOut);
        assert_eq!(waiting.status(start + ms(5_001)), WaitStatus::TimedOut);

        let mut screen = self::screen();
        let mut waiting = ScreenWait::start(
            &wait(Some("done"), Some(100)),
            &mut screen,
            false,
            start,
            start,
        );
        screen.process_bytes(b"done");
        waiting.observe(&mut screen, start + ms(4_900));
        assert_eq!(waiting.status(start + ms(5_000)), WaitStatus::Ready);
    }

    #[test]
    fn a_late_baseline_counts_quiet_from_itself_and_keeps_the_request_deadline() {
        let mut screen = screen();
        let requested = Instant::now();
        let ms = Duration::from_millis;
        // The input took 800ms to be marked: none of that was watched, so none of it is quiet.
        let baseline = requested + ms(800);
        let waiting = ScreenWait::start(
            &wait(None, Some(500)),
            &mut screen,
            true,
            requested,
            baseline,
        );
        assert_eq!(waiting.status(baseline), WaitStatus::Pending);
        assert_eq!(waiting.status(baseline + ms(499)), WaitStatus::Pending);
        assert_eq!(waiting.status(baseline + ms(500)), WaitStatus::Ready);

        // The deadline is still the caller's: 5s from the request, not from the baseline.
        let waiting = ScreenWait::start(
            &wait(Some("never"), None),
            &mut screen,
            true,
            requested,
            baseline,
        );
        assert_eq!(waiting.status(requested + ms(5_000)), WaitStatus::TimedOut);
    }

    #[test]
    fn a_scrolled_view_still_waits_on_the_live_screen() {
        let mut screen = screen();
        for line in 0..20 {
            screen.process_bytes(format!("line {line}\r\n").as_bytes());
        }
        screen.set_scrollback(10);
        let now = Instant::now();
        let mut waiting =
            ScreenWait::start(&wait(Some("fresh"), None), &mut screen, false, now, now);
        screen.process_bytes(b"fresh");
        waiting.observe(&mut screen, now);
        assert_eq!(waiting.status(now), WaitStatus::Ready);
        assert_ne!(screen.scrollback_offset(), 0);
    }

    #[test]
    fn a_waited_spans_capture_keeps_its_pixels_and_other_renders_refuse_them_up_front() {
        let capture = |render, image_pixels| ControlCommand::CapturePane {
            target: Some(3),
            scrollback: None,
            render,
            scale: None,
            wait: Some(wait(Some("ready"), None)),
            image_pixels,
        };
        let plan = WaitPlan::for_command(&capture(CaptureRender::Spans, true))
            .expect("a spans capture takes pixels")
            .expect("the capture waits");
        assert!(plan.reply.image_pixels);
        assert_eq!(plan.reply.render, CaptureRender::Spans);

        let refused = WaitPlan::for_command(&capture(CaptureRender::Png, true))
            .expect_err("pixels belong to spans only");
        assert_eq!(refused.code, Some(ControlErrorCode::InvalidArgument));
    }
}
