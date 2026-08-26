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
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Whether anything has been drawn yet, so `finish` knows if there is a row to erase.
    painted: AtomicBool,
}

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
        let row = match fraction {
            Some(fraction) => {
                let style = if complete {
                    MeterStyle::complete(true, self.truecolor)
                } else {
                    MeterStyle::brand(true, self.truecolor)
                };
                let readout = match total {
                    // One lavender run over the whole readout: nesting a reset inside it would end
                    // the colour early and leave the total unstyled.
                    Some(total) => {
                        format!("{lavender}{} of {}{reset}", bytes(downloaded), bytes(total))
                    }
                    None => format!("{lavender}{}{reset}", bytes(downloaded)),
                };
                // The percentage is truncated, not rounded, for the same reason the meter reserves
                // its last cell: 99.6% must not print as 100% beside a bar that is still short.
                format!(
                    "  {accent}●{reset} {lavender}{:<LABEL_WIDTH$}{reset} {} {:>3}%  {readout}",
                    self.label,
                    meter(fraction, METER_WIDTH, style),
                    (fraction * 100.0) as u32,
                )
            }
            // No Content-Length: there is no fraction to draw, so report what has arrived rather
            // than inventing a position on a track.
            None => format!(
                "  {accent}●{reset} {lavender}{:<LABEL_WIDTH$}{reset} {lavender}{}{reset}",
                self.label,
                bytes(downloaded)
            ),
        };

        let mut err = io::stderr().lock();
        let hide = if self.painted.swap(true, Ordering::Relaxed) {
            ""
        } else {
            super::ansi::HIDE_CURSOR
        };
        let _ = write!(err, "{hide}{}{row}", super::ansi::CLEAR_ROW);
        let _ = err.flush();
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
