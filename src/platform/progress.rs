//! A single-row progress meter for plain stdout, drawn in the rozi palette.
//!
//! This is the non-TUI counterpart to the app's own rendering: `rozi update` and `rozi install`
//! run before or instead of the full screen, so they get one rewritten row rather than a frame.
//! The escapes all come from [`super::ansi`]; nothing here assembles a sequence itself.
//!
//! The bar and the track are styled separately on purpose. A meter whose remainder is painted in
//! the accent reads as one solid shape and hides where the fill actually ends, which is the whole
//! thing a progress bar exists to show.

use std::io::{self, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::ansi::{self, Rgb, palette};

/// How wide the meter's track is, in cells. Sized to sit comfortably inside an 80-column row
/// alongside its label and readout.
pub const METER_WIDTH: usize = 32;

/// The filled run. Heavy weight so it stays distinct from the track without relying on colour.
const BAR_GLYPH: char = '━';
/// The unfilled remainder. Light weight, so a monochrome terminal still shows the boundary.
const TRACK_GLYPH: char = '─';

/// How a meter should be painted.
#[derive(Clone, Copy, Debug)]
pub struct MeterStyle {
    /// The colour the filled run starts at, on the left.
    pub bar_from: Rgb,
    /// The colour the filled run reaches at the far right of the track.
    pub bar_to: Rgb,
    /// The unfilled remainder.
    pub track: Rgb,
    /// Whether to emit any escapes at all.
    pub color: bool,
    /// Whether the terminal advertised 24-bit colour.
    pub truecolor: bool,
}

impl MeterStyle {
    /// The brand meter: the logo's rose-to-violet gradient over a chrome track.
    pub fn brand(color: bool, truecolor: bool) -> Self {
        Self {
            bar_from: palette::ROSE,
            bar_to: palette::VIOLET,
            track: palette::TRACK,
            color,
            truecolor,
        }
    }

    /// A meter that has finished successfully - a flat run with no gradient left to travel.
    pub fn complete(color: bool, truecolor: bool) -> Self {
        Self {
            bar_from: palette::SUCCESS,
            bar_to: palette::SUCCESS,
            track: palette::TRACK,
            color,
            truecolor,
        }
    }
}

/// Render a meter of `width` cells that is `fraction` full, as a styled string.
///
/// The gradient is sampled against the *track*, not against the filled run, so a given cell keeps
/// its colour as the bar grows past it. Sampling across the fill instead would recolour the whole
/// bar on every update and read as a flicker rather than as progress.
pub fn meter(fraction: f64, width: usize, style: MeterStyle) -> String {
    let fraction = fraction.clamp(0.0, 1.0);
    // A full track is reserved for actual completion. Rounding to nearest would fill the last cell
    // at 99% of a 32-cell track, so the bar would read as finished while bytes were still arriving
    // - which is worse than a bar that reaches the end a moment late.
    let filled = if width == 0 {
        0
    } else if fraction >= 1.0 {
        width
    } else {
        ((fraction * width as f64) as usize).min(width - 1)
    };

    let mut out = String::new();
    if !style.color {
        out.extend(std::iter::repeat_n(BAR_GLYPH, filled));
        out.extend(std::iter::repeat_n(TRACK_GLYPH, width - filled));
        return out;
    }

    if filled > 0 {
        // One SGR per cell is what a per-cell gradient costs. The row is redrawn at most a few
        // times a second over ~32 cells, so this is far below anything a terminal notices.
        for cell in 0..filled {
            let color = sample(style.bar_from, style.bar_to, cell, width);
            out.push_str(&ansi::fg(color, style.truecolor));
            out.push(BAR_GLYPH);
        }
    }
    if filled < width {
        out.push_str(&ansi::fg(style.track, style.truecolor));
        out.extend(std::iter::repeat_n(TRACK_GLYPH, width - filled));
    }
    out.push_str(ansi::RESET);
    out
}

/// The gradient colour for `cell` of a `width`-cell track.
fn sample(from: Rgb, to: Rgb, cell: usize, width: usize) -> Rgb {
    if width <= 1 {
        return from;
    }
    from.mix(to, cell as u32, (width - 1) as u32)
}

/// Format a byte count the way a download readout should read: three significant digits at most,
/// and never a fractional byte.
pub fn bytes(count: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = count as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{count} {}", UNITS[0])
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// A one-row download meter written to stderr.
///
/// Stderr rather than stdout so `rozi update --check | jq` keeps a clean stdout while the row still
/// reaches a watching human. The row is rewritten in place and erased when the work finishes, so
/// what remains in the scrollback is the outcome, not a trail of partial bars.
pub struct StatusRow {
    label: String,
    color: bool,
    truecolor: bool,
    /// When the row was last painted, so a fast link does not spend its time formatting.
    last_paint: Mutex<Option<Instant>>,
    /// When the transfer started, for the rate and the estimate. Set on the first report rather
    /// than at construction: the row is built before the request is made, and the connection and
    /// TLS handshake would otherwise be charged to the transfer's average speed.
    started: Mutex<Option<Instant>>,
    /// Which spinner frame comes next. Advanced per redraw rather than per unit of time, which is
    /// what makes the spin rate report the redraw rate: a stalled transfer visibly stops.
    frame: AtomicUsize,
    /// Whether anything has been drawn yet, so `finish` knows if there is a row to erase.
    painted: AtomicBool,
}

/// Spinner frames, one eighth-circle apart. Braille is the densest way to show rotation in a
/// single cell, and every terminal that renders the meter's box-drawing glyphs renders these.
const SPINNER: [char; 8] = ['⠋', '⠙', '⠸', '⠼', '⠴', '⠦', '⠧', '⠏'];

/// Below this a rate is guesswork: the first chunks of a transfer arrive at whatever speed the
/// connection ramps to, and reporting that as a steady figure is worse than reporting nothing.
const RATE_SETTLES_AFTER: Duration = Duration::from_millis(750);

/// The narrowest meter worth drawing. Below this a bar carries no information a percentage does
/// not already give, so a cramped terminal gets the readout alone rather than a four-cell stub.
const MIN_METER_WIDTH: usize = 8;

/// The columns the row spends on everything except the meter: two of indent, the dot and its
/// space, the label column, a space, ` 100%`, and two before the readout.
const ROW_CHROME: usize = 2 + 2 + LABEL_WIDTH + 1 + 5 + 2;

/// What fits on one row, widest form first.
///
/// A status row must stay on one line: `\r` and the erase-line escape only clear the row the
/// cursor is on, so a wrapped row leaves its earlier fragments on screen for every redraw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowLayout {
    /// Label, meter of this width, percentage, and byte readout.
    Full(usize),
    /// Label, percentage, and byte readout. A meter this narrow would say nothing the percentage
    /// does not already say.
    Compact,
    /// The percentage alone. What is left when even the label column will not fit.
    Minimal,
}

/// The widest row that fits in `columns`.
fn layout_for(columns: Option<u16>, readout: usize) -> RowLayout {
    // No terminal to measure means the caller has already decided the row is worth drawing;
    // assume the conventional 80 rather than refusing to draw at all.
    let columns = columns.map_or(80usize, usize::from);
    let available = columns.saturating_sub(ROW_CHROME + readout);
    if available >= MIN_METER_WIDTH {
        RowLayout::Full(available.min(METER_WIDTH))
    } else if ROW_CHROME + readout <= columns {
        RowLayout::Compact
    } else {
        RowLayout::Minimal
    }
}

impl RowLayout {
    /// The columns this layout occupies, for a readout of `readout` cells.
    ///
    /// Only the fit test needs this: the renderer formats a row rather than measuring one. It
    /// exists so the one-line invariant is stated as code that can be checked, not as a comment.
    #[cfg(test)]
    fn width(self, readout: usize) -> usize {
        match self {
            Self::Full(meter) => ROW_CHROME + readout + meter,
            Self::Compact => ROW_CHROME + readout,
            // Two of indent, the dot and its space, and ` 100%`.
            Self::Minimal => MINIMAL_ROW_WIDTH,
        }
    }
}

/// Two of indent, the dot and its space, and ` 100%`.
#[cfg(test)]
const MINIMAL_ROW_WIDTH: usize = 2 + 2 + 5;

/// Bodies at or below this are not drawn. Metadata and signature files are well under a kilobyte;
/// only the release archive is worth a meter.
const MIN_REPORTABLE: u64 = 1024 * 1024;

/// Whether a body of this declared size deserves a meter.
///
/// An unknown total draws: it could be the archive arriving chunked, and showing bytes climb beats
/// showing nothing. A small known total does not - relswap reports the sub-kilobyte manifest and
/// signature too, and a bar that flashes on and off for those is noise.
fn worth_drawing(total: Option<u64>) -> bool {
    total.is_none_or(|total| total > MIN_REPORTABLE)
}

/// A duration as a person reads a wait: never more than two units, never a precision the estimate
/// does not have.
fn duration(left: Duration) -> String {
    let seconds = left.as_secs();
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

/// The readouts a row could show, richest first.
///
/// Rate and estimate are the first things dropped when columns run short: they are the least of
/// what a meter says, and a bar plus a byte count is still a complete answer. Both are omitted
/// entirely until the transfer has run long enough for an average to mean anything.
fn readouts(downloaded: u64, total: Option<u64>, elapsed: Option<Duration>) -> Vec<String> {
    let mut options = Vec::new();
    let Some(total) = total else {
        // No declared length: the byte count is the whole readout, and there is no estimate to make.
        return vec![bytes(downloaded)];
    };
    let transferred = format!("{} of {}", bytes(downloaded), bytes(total));

    // A rate needs both a settled elapsed time and bytes to divide by it.
    let rate = elapsed
        .filter(|elapsed| *elapsed >= RATE_SETTLES_AFTER && downloaded > 0)
        .map(|elapsed| downloaded as f64 / elapsed.as_secs_f64())
        .filter(|rate| *rate > 0.0);

    if let Some(rate) = rate {
        let per_second = format!("{}/s", bytes(rate as u64));
        // The estimate assumes the average speed holds, which is why it is the first thing cut:
        // it is the least trustworthy figure on the row.
        if total > downloaded {
            let left = Duration::from_secs_f64((total - downloaded) as f64 / rate);
            options.push(format!(
                "{transferred} · {per_second} · {} left",
                duration(left)
            ));
        }
        options.push(format!("{transferred} · {per_second}"));
    }
    options.push(transferred);
    options.push(bytes(downloaded));
    options
}

/// The richest readout that leaves room for a meter, and its visible width.
///
/// Chosen against the *meter's* minimum rather than against the whole row, so extra detail never
/// costs the bar: the bar is the thing the row exists to show.
fn choose_readout(columns: Option<u16>, options: &[String]) -> (String, usize) {
    let columns = columns.map_or(80usize, usize::from);
    let budget = columns.saturating_sub(ROW_CHROME + MIN_METER_WIDTH);
    options
        .iter()
        .map(|option| (option, option.chars().count()))
        .find(|(_, width)| *width <= budget)
        .map_or_else(
            // Everything is too wide: take the shortest and let the layout tiers deal with it.
            || {
                let shortest = options.last().cloned().unwrap_or_default();
                let width = shortest.chars().count();
                (shortest, width)
            },
            |(option, width)| (option.clone(), width),
        )
}

/// How far along a transfer is, or `None` when the total is unknown or nonsensical.
///
/// A chunked response carries no `Content-Length`, and a server can declare zero while sending a
/// body, so neither case may reach the renderer as a fraction.
fn fraction_of(downloaded: u64, total: Option<u64>) -> Option<f64> {
    match total {
        Some(total) if total > 0 => Some(downloaded as f64 / total as f64),
        _ => None,
    }
}

/// The label column, wide enough for the longest label a row uses ("Downloading") plus a space,
/// so the meter starts at the same column on every row rather than shifting with the verb.
const LABEL_WIDTH: usize = 12;

/// The shortest gap between redraws. 50ms is about three frames at 60Hz - fast enough to read as
/// motion, slow enough that a gigabit link does not format thousands of rows a second.
const REPAINT_INTERVAL: Duration = Duration::from_millis(50);

/// Replace the active row with one write.
///
/// Windows Terminal may render between separate console writes. Sending `CLEAR_ROW` through one
/// formatting argument and the replacement through another therefore exposes a blank frame on
/// every repaint, even though a Unix terminal commonly coalesces the same writes. Build the whole
/// frame first, like `install.ps1` does for its working progress row.
fn replace_row(output: &mut impl Write, hide_cursor: bool, row: &str) -> io::Result<()> {
    let hide = if hide_cursor { ansi::HIDE_CURSOR } else { "" };
    let mut frame = String::with_capacity(hide.len() + ansi::CLEAR_ROW.len() + row.len());
    frame.push_str(hide);
    frame.push_str(ansi::CLEAR_ROW);
    frame.push_str(row);
    output.write_all(frame.as_bytes())?;
    output.flush()
}

impl StatusRow {
    /// A row that draws only when stderr can render it. A redirected or `NO_COLOR` stream gets a
    /// silent sink, matching the install scripts, which keep a plain append-only transcript.
    pub fn new(label: impl Into<String>) -> Self {
        let color = super::ansi::stderr_supports_color();
        Self {
            label: label.into(),
            color,
            truecolor: color && super::ansi::supports_truecolor(),
            last_paint: Mutex::new(None),
            started: Mutex::new(None),
            frame: AtomicUsize::new(0),
            painted: AtomicBool::new(false),
        }
    }

    /// Whether this row will draw anything at all.
    pub fn is_visible(&self) -> bool {
        self.color
    }

    /// Erase the row, leaving the cursor where the next line can be written normally.
    pub fn finish(&self) {
        if !self.color || !self.painted.swap(false, Ordering::Relaxed) {
            return;
        }
        let mut err = io::stderr().lock();
        let _ = write!(
            err,
            "{}{}",
            super::ansi::CLEAR_ROW,
            super::ansi::SHOW_CURSOR
        );
        let _ = err.flush();
        // Release the claim only after the escape has reached the terminal: a signal arriving in
        // between must still find the cursor recorded as hidden and restore it.
        super::cursor::show();
    }

    /// How long the transfer has been running, starting the clock on the first call.
    ///
    /// `None` until a second call, so the very first frame never reports a rate computed over a
    /// zero-length interval.
    fn elapsed(&self) -> Option<Duration> {
        let mut started = self.started.lock().ok()?;
        match *started {
            Some(started) => Some(started.elapsed()),
            None => {
                *started = Some(Instant::now());
                None
            }
        }
    }

    /// The next spinner frame.
    fn spin(&self) -> char {
        let frame = self.frame.fetch_add(1, Ordering::Relaxed);
        SPINNER[frame % SPINNER.len()]
    }

    fn should_paint(&self, complete: bool) -> bool {
        let mut last = match self.last_paint.lock() {
            Ok(last) => last,
            // A poisoned lock means another thread panicked mid-paint. Drawing is cosmetic, so
            // decline rather than propagating a panic out of a download.
            Err(_) => return false,
        };
        let now = Instant::now();
        // The final frame always draws: otherwise a download that finishes inside the throttle
        // window leaves a bar stopped short of the end as the last thing on screen.
        if complete || last.is_none_or(|last| now.duration_since(last) >= REPAINT_INTERVAL) {
            *last = Some(now);
            true
        } else {
            false
        }
    }
}

impl relswap::ProgressObserver for StatusRow {
    fn advance(&self, downloaded: u64, total: Option<u64>) {
        // relswap reports every fetch, including the sub-kilobyte manifest and signature. Deciding
        // what is worth drawing is the consumer's call, not the transport's: a bar that flashes on
        // and off for a 600-byte file is noise.
        if !self.color || !worth_drawing(total) {
            return;
        }
        let fraction = fraction_of(downloaded, total);
        let complete = fraction.is_some_and(|fraction| fraction >= 1.0);
        if !self.should_paint(complete) {
            return;
        }

        let accent = ansi::fg(palette::ROSE, self.truecolor);
        let lavender = ansi::fg(palette::LAVENDER, self.truecolor);
        let reset = ansi::RESET;
        // Measured per redraw rather than cached: a terminal resized mid-download must re-fit
        // rather than keep drawing to the width it had when the transfer started.
        let columns = ansi::stderr_width();
        let (readout, readout_width) =
            choose_readout(columns, &readouts(downloaded, total, self.elapsed()));
        // One lavender run over the whole readout: nesting a reset inside it would end the colour
        // early and leave everything after it unstyled.
        let readout = format!("{lavender}{readout}{reset}");

        let row = match fraction {
            Some(fraction) => {
                let style = if complete {
                    MeterStyle::complete(true, self.truecolor)
                } else {
                    MeterStyle::brand(true, self.truecolor)
                };
                // A finished transfer shows a settled mark rather than a spinner frozen mid-turn.
                let mark = if complete { '●' } else { self.spin() };
                // The percentage is truncated, not rounded, for the same reason the meter reserves
                // its last cell: 99.6% must not print as 100% beside a bar that is still short.
                let percent = (fraction * 100.0) as u32;
                match layout_for(columns, readout_width) {
                    RowLayout::Full(width) => format!(
                        "  {accent}{mark}{reset} {lavender}{:<LABEL_WIDTH$}{reset} {} {percent:>3}%  {readout}",
                        self.label,
                        meter(fraction, width, style),
                    ),
                    // Too narrow for a bar: the percentage and the readout still say everything a
                    // meter would, and they fit.
                    RowLayout::Compact => format!(
                        "  {accent}{mark}{reset} {lavender}{:<LABEL_WIDTH$}{reset} {percent:>3}%  {readout}",
                        self.label,
                    ),
                    RowLayout::Minimal => format!("  {accent}{mark}{reset} {percent:>3}%"),
                }
            }
            // No Content-Length: there is no fraction to draw, so the spinner carries the fact that
            // something is still happening, and the byte count carries how much.
            None => format!(
                "  {accent}{}{reset} {lavender}{:<LABEL_WIDTH$}{reset} {readout}",
                self.spin(),
                self.label,
            ),
        };

        let mut err = io::stderr().lock();
        let first = !self.painted.swap(true, Ordering::Relaxed);
        if first {
            // Claim the cursor before hiding it, so the signal handler knows it has something to
            // restore even if the process is killed between this write and the next.
            super::cursor::hide();
        }
        let _ = replace_row(&mut err, first, &row);
    }
}

impl Drop for StatusRow {
    /// Restore the cursor even if the download failed or the caller forgot to finish. A hidden
    /// cursor outliving the process is the one artefact a user cannot undo without `reset`.
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> MeterStyle {
        MeterStyle {
            color: false,
            ..MeterStyle::brand(false, false)
        }
    }

    #[test]
    fn an_unstyled_meter_is_readable_without_colour() {
        // The two glyphs differ in weight, so the boundary survives NO_COLOR and a pipe.
        assert_eq!(meter(0.0, 4, plain()), "────");
        assert_eq!(meter(0.5, 4, plain()), "━━──");
        assert_eq!(meter(1.0, 4, plain()), "━━━━");
    }

    #[test]
    fn a_meter_fills_completely_only_when_the_work_is_done() {
        // Rounding must not let 99.x% render as a full bar: a meter that reads finished while the
        // process is still running is the one failure mode worth ruling out.
        let nearly = meter(0.99, 32, plain());
        assert!(nearly.contains(TRACK_GLYPH), "99% still shows track");
        assert_eq!(meter(1.0, 32, plain()).matches(BAR_GLYPH).count(), 32);
    }

    #[test]
    fn out_of_range_fractions_are_clamped_rather_than_panicking() {
        // The total comes from a Content-Length header, so a server that under-reports it must not
        // be able to drive the renderer past the end of the track.
        assert_eq!(meter(2.0, 4, plain()), "━━━━");
        assert_eq!(meter(-1.0, 4, plain()), "────");
        assert_eq!(meter(f64::NAN, 4, plain()), "────");
    }

    #[test]
    fn a_zero_width_meter_renders_nothing() {
        assert_eq!(meter(0.5, 0, plain()), "");
    }

    #[test]
    fn the_gradient_runs_from_rose_to_violet_across_the_track() {
        // Sampling against the track, not the fill, is what keeps a cell's colour stable as the
        // bar grows past it - the property that makes the bar read as filling rather than flashing.
        let width = 32;
        assert_eq!(
            sample(palette::ROSE, palette::VIOLET, 0, width),
            palette::ROSE
        );
        assert_eq!(
            sample(palette::ROSE, palette::VIOLET, width - 1, width),
            palette::VIOLET
        );
        let half = sample(palette::ROSE, palette::VIOLET, width / 2, width);
        assert_ne!(half, palette::ROSE);
        assert_ne!(half, palette::VIOLET);

        let quarter_at_half_fill = sample(palette::ROSE, palette::VIOLET, 8, width);
        let quarter_at_full_fill = sample(palette::ROSE, palette::VIOLET, 8, width);
        assert_eq!(quarter_at_half_fill, quarter_at_full_fill);
    }

    #[test]
    fn a_styled_meter_paints_bar_and_track_in_different_colours() {
        let styled = meter(0.5, 8, MeterStyle::brand(true, true));
        assert!(styled.contains(&ansi::fg(palette::ROSE, true)));
        assert!(styled.contains(&ansi::fg(palette::TRACK, true)));
        assert!(styled.ends_with(ansi::RESET));
    }

    #[test]
    fn a_full_meter_emits_no_track_colour() {
        let styled = meter(1.0, 8, MeterStyle::brand(true, true));
        assert!(!styled.contains(&ansi::fg(palette::TRACK, true)));
    }

    #[test]
    fn a_known_total_yields_a_fraction_and_an_unknown_one_does_not() {
        assert_eq!(fraction_of(5, Some(10)), Some(0.5));
        // A chunked response has no Content-Length; the renderer must not divide by it.
        assert_eq!(fraction_of(5, None), None);
        // A server declaring zero while sending a body must not produce infinity.
        assert_eq!(fraction_of(5, Some(0)), None);
    }

    #[test]
    fn only_bodies_worth_watching_are_drawn() {
        // relswap reports every fetch, the sub-kilobyte manifest and signature included.
        assert!(
            !worth_drawing(Some(600)),
            "a signature file is not a download"
        );
        assert!(worth_drawing(Some(18 * 1024 * 1024)), "the archive is");
        // Unknown means possibly the archive, arriving chunked: draw it.
        assert!(worth_drawing(None));
    }

    #[test]
    fn a_row_never_asks_for_more_columns_than_the_terminal_has() {
        // The invariant that matters, and the reason the layout has three tiers rather than two:
        // a wrapped row leaves orphaned fragments behind on every redraw, because the erase escape
        // only reaches the row the cursor is on.
        let readout = "18.0 MB of 18.0 MB".chars().count();
        for columns in MINIMAL_ROW_WIDTH as u16..=200 {
            let layout = layout_for(Some(columns), readout);
            assert!(
                layout.width(readout) <= usize::from(columns),
                "{layout:?} takes {} columns, terminal has {columns}",
                layout.width(readout)
            );
        }
    }

    #[test]
    fn the_layout_degrades_one_tier_at_a_time_as_the_terminal_narrows() {
        let readout = "18.0 MB of 18.0 MB".chars().count();
        assert_eq!(layout_for(Some(200), readout), RowLayout::Full(METER_WIDTH));
        // Narrow enough that a bar would carry nothing the percentage does not.
        assert_eq!(layout_for(Some(45), readout), RowLayout::Compact);
        // Narrower than the label column and readout together: percentage only.
        assert_eq!(layout_for(Some(20), readout), RowLayout::Minimal);
        // An unmeasurable terminal assumes 80 rather than refusing to draw.
        assert_eq!(layout_for(None, readout), layout_for(Some(80), readout));
    }

    #[test]
    fn a_shrinking_terminal_shrinks_the_meter_before_dropping_it() {
        let readout = "1.0 MB of 18.0 MB".chars().count();
        let RowLayout::Full(wide) = layout_for(Some(120), readout) else {
            panic!("a 120-column terminal fits a full row");
        };
        let RowLayout::Full(mid) = layout_for(Some(70), readout) else {
            panic!("a 70-column terminal still fits a meter");
        };
        assert!(
            mid < wide,
            "a narrower terminal gets a shorter bar, not the same one"
        );
        assert!(mid >= MIN_METER_WIDTH);
    }

    #[test]
    fn a_rate_is_withheld_until_it_means_something() {
        let total = Some(1024 * 1024 * 100);
        // No elapsed time yet: the first frame has nothing to divide by.
        let first = readouts(1024, total, None);
        assert!(
            first.iter().all(|option| !option.contains("/s")),
            "{first:?}"
        );
        // Still ramping: an average over 100ms reports the connection warming up, not the transfer.
        let early = readouts(1024, total, Some(Duration::from_millis(100)));
        assert!(
            early.iter().all(|option| !option.contains("/s")),
            "{early:?}"
        );
        // Settled.
        let settled = readouts(1024 * 1024 * 10, total, Some(Duration::from_secs(5)));
        assert!(
            settled.iter().any(|option| option.contains("/s")),
            "{settled:?}"
        );
        assert!(
            settled.iter().any(|option| option.contains("left")),
            "{settled:?}"
        );
    }

    #[test]
    fn a_finished_transfer_offers_no_estimate() {
        let total = 1024 * 1024 * 10;
        // Nothing left to wait for; an estimate of zero is noise beside a full bar.
        let done = readouts(total, Some(total), Some(Duration::from_secs(5)));
        assert!(
            done.iter().all(|option| !option.contains("left")),
            "{done:?}"
        );
    }

    #[test]
    fn an_unknown_total_offers_only_what_has_arrived() {
        let options = readouts(4096, None, Some(Duration::from_secs(5)));
        assert_eq!(options, vec![bytes(4096)]);
    }

    #[test]
    fn readouts_are_offered_richest_first_and_each_is_shorter_than_the_last() {
        let options = readouts(
            1024 * 1024 * 10,
            Some(1024 * 1024 * 100),
            Some(Duration::from_secs(5)),
        );
        assert!(options.len() >= 3, "{options:?}");
        for pair in options.windows(2) {
            assert!(
                pair[0].chars().count() > pair[1].chars().count(),
                "{:?} should be longer than {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn detail_is_dropped_before_the_meter_is() {
        let options = readouts(
            1024 * 1024 * 10,
            Some(1024 * 1024 * 100),
            Some(Duration::from_secs(5)),
        );
        // Wide: the estimate fits.
        let (wide, _) = choose_readout(Some(200), &options);
        assert!(wide.contains("left"), "{wide}");
        // Narrow enough that the full readout no longer fits beside a minimum-width bar: the
        // estimate is dropped, and the bar is still there. That ordering is the whole point - the
        // bar is what the row exists to show, so detail yields to it rather than the reverse.
        let (mid, mid_width) = choose_readout(Some(68), &options);
        assert!(!mid.contains("left"), "{mid}");
        assert!(mid.contains("/s"), "the rate outlives the estimate: {mid}");
        assert!(
            matches!(layout_for(Some(68), mid_width), RowLayout::Full(_)),
            "the meter survives losing the estimate"
        );

        // Narrower still: the rate goes too, and the bar is *still* there.
        let (tight, tight_width) = choose_readout(Some(52), &options);
        assert!(!tight.contains("/s"), "{tight}");
        assert!(
            tight.contains(" of "),
            "the byte count is the last thing kept: {tight}"
        );
        assert!(matches!(
            layout_for(Some(52), tight_width),
            RowLayout::Full(_)
        ));
    }

    #[test]
    fn a_spinner_advances_and_wraps() {
        let row = StatusRow::new("Downloading");
        let frames: Vec<char> = (0..SPINNER.len() * 2).map(|_| row.spin()).collect();
        assert_eq!(&frames[..SPINNER.len()], &SPINNER);
        // Wrapping rather than running off the end of the table.
        assert_eq!(&frames[SPINNER.len()..], &SPINNER);
    }

    #[test]
    fn a_repaint_sends_clear_and_replacement_in_one_write() {
        #[derive(Default)]
        struct WriteProbe {
            writes: Vec<Vec<u8>>,
            flushes: usize,
        }

        impl Write for WriteProbe {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.writes.push(bytes.to_vec());
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                self.flushes += 1;
                Ok(())
            }
        }

        let mut output = WriteProbe::default();
        replace_row(&mut output, true, "Downloading").expect("write replacement row");

        assert_eq!(
            output.writes,
            vec![format!("{}{}Downloading", ansi::HIDE_CURSOR, ansi::CLEAR_ROW).into_bytes()]
        );
        assert_eq!(output.flushes, 1);
    }

    #[test]
    fn durations_read_in_at_most_two_units() {
        assert_eq!(duration(Duration::from_secs(9)), "9s");
        assert_eq!(duration(Duration::from_secs(59)), "59s");
        assert_eq!(duration(Duration::from_secs(60)), "1m 00s");
        assert_eq!(duration(Duration::from_secs(125)), "2m 05s");
        assert_eq!(duration(Duration::from_secs(3600)), "1h 00m");
        assert_eq!(duration(Duration::from_secs(3725)), "1h 02m");
    }

    #[test]
    fn a_row_claims_the_cursor_so_a_signal_can_restore_it() {
        // The Ctrl+C path reads this flag from a signal handler; if the row never sets it, a killed
        // download leaves the cursor hidden until the user types `reset`.
        super::super::cursor::show();
        assert!(!super::super::cursor::is_hidden());
        super::super::cursor::hide();
        assert!(super::super::cursor::is_hidden());
        super::super::cursor::show();
        assert!(!super::super::cursor::is_hidden());
    }

    #[test]
    fn byte_counts_read_at_three_significant_digits() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1024), "1.0 KB");
        assert_eq!(bytes(1024 * 1024), "1.0 MB");
        assert_eq!(bytes(18 * 1024 * 1024), "18.0 MB");
        // Past three digits the fraction stops earning its place.
        assert_eq!(bytes(512 * 1024), "512 KB");
    }
}
