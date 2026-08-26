//! Whether plain stdout can render ANSI SGR styling, plus the Windows console work that makes it
//! true.
//!
//! This is only for output written *outside* the TUI - `--help` and friends. Inside the app the
//! framework owns the screen and its own capability handling.
//!
//! The decision follows the conventions cargo and the `NO_COLOR` standard settled on, so a user who
//! has already told every other tool what they want does not have to tell rozi separately.

use std::io::IsTerminal;

/// The environment inputs that decide styling, read once so the decision itself is a pure function.
#[derive(Clone, Copy, Debug)]
pub struct ColorEnv {
    /// `NO_COLOR` is set to anything non-empty (<https://no-color.org>).
    pub no_color: bool,
    /// `CLICOLOR_FORCE` is set to anything non-empty: style even when piped.
    pub clicolor_force: bool,
    /// `CLICOLOR=0`: the opt-out for callers that do not set `NO_COLOR`.
    pub clicolor_off: bool,
    /// `TERM=dumb`, which promises no escape-sequence handling at all.
    pub term_dumb: bool,
    /// Whether the stream is attached to a terminal rather than a pipe or file.
    pub is_terminal: bool,
}

impl ColorEnv {
    /// The same decision for stderr. Progress rows go there so a redirected stdout stays clean
    /// while a watching human still sees them, which means the two streams can differ: piping
    /// stdout must not silence a meter stderr can still render.
    pub fn for_stderr() -> Self {
        Self {
            is_terminal: std::io::stderr().is_terminal(),
            ..Self::for_stdout()
        }
    }

    pub fn for_stdout() -> Self {
        Self {
            no_color: non_empty("NO_COLOR"),
            clicolor_force: non_empty("CLICOLOR_FORCE"),
            clicolor_off: std::env::var_os("CLICOLOR").is_some_and(|value| value == "0"),
            term_dumb: std::env::var_os("TERM").is_some_and(|value| value == "dumb"),
            is_terminal: std::io::stdout().is_terminal(),
        }
    }
}

/// Whether to emit SGR sequences, given what the environment asked for.
///
/// `NO_COLOR` wins over everything, then `CLICOLOR_FORCE` (which deliberately outranks the terminal
/// check so a caller can colour a pipe it intends to render itself).
pub fn wants_color(env: &ColorEnv) -> bool {
    if env.no_color {
        return false;
    }
    if env.clicolor_force {
        return true;
    }
    !env.clicolor_off && !env.term_dumb && env.is_terminal
}

/// Whether stdout should carry ANSI SGR styling right now.
///
/// On Windows this also *enables* virtual-terminal processing as a side effect, because a console
/// that has not been switched into it prints the escapes as literal garbage instead.
pub fn stdout_supports_color() -> bool {
    wants_color(&ColorEnv::for_stdout()) && enable_virtual_terminal()
}

/// Whether stderr should carry ANSI SGR styling right now.
///
/// Shares the Windows virtual-terminal enablement with stdout: the mode is process-wide, so a
/// console switched into it for one stream is in it for both.
pub fn stderr_supports_color() -> bool {
    wants_color(&ColorEnv::for_stderr()) && enable_virtual_terminal()
}

/// A truecolor value from the rozi brand palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Mix `self` toward `other`, where `numerator / denominator` is how far along to land.
    ///
    /// Integer math on purpose: the gradient is sampled per cell across a bar whose width is a
    /// cell count, so there is no fractional position to preserve and rounding once per channel
    /// keeps the ramp reproducible in tests.
    pub fn mix(self, other: Self, numerator: u32, denominator: u32) -> Self {
        if denominator == 0 {
            return self;
        }
        Self(
            mix_channel(self.0, other.0, numerator, denominator),
            mix_channel(self.1, other.1, numerator, denominator),
            mix_channel(self.2, other.2, numerator, denominator),
        )
    }
}

/// One channel of [`Rgb::mix`]. Rounds to nearest rather than truncating, so a ramp reaches its
/// endpoint instead of stopping one step short.
fn mix_channel(from: u8, to: u8, numerator: u32, denominator: u32) -> u8 {
    let from = from as u32;
    let to = to as u32;
    let shift = (from.abs_diff(to) * numerator + denominator / 2) / denominator;
    if to >= from {
        (from + shift) as u8
    } else {
        (from - shift) as u8
    }
}

/// The rozi brand palette, matching `rozi_theme()` in `state::appearance` so the CLI, the
/// installers, and the running app describe themselves with one set of colours.
///
/// The identity is the rose-to-violet gradient carried by the logo and the docs site.
pub mod palette {
    use super::Rgb;

    /// Primary brand accent - the rose the logo gradient starts from.
    pub const ROSE: Rgb = Rgb(0xFD, 0x4A, 0x80);
    /// The violet the logo gradient ends at. Pairs with `ROSE` across a filled span.
    pub const VIOLET: Rgb = Rgb(0x98, 0x2B, 0xF2);
    /// Lavender secondary text: labels, units, and anything supporting the accent.
    pub const LAVENDER: Rgb = Rgb(0x8E, 0x93, 0xB4);
    /// The unfilled remainder of a meter. Deliberately near the app's border colour so a track
    /// reads as chrome rather than as data.
    pub const TRACK: Rgb = Rgb(0x34, 0x38, 0x58);
    /// Primary foreground text.
    pub const TEXT: Rgb = Rgb(0xCC, 0xD0, 0xE6);
    pub const SUCCESS: Rgb = Rgb(0x4A, 0xDE, 0x80);
    pub const WARNING: Rgb = Rgb(0xF0, 0xA8, 0x30);
    pub const ERROR: Rgb = Rgb(0xFF, 0x5F, 0x57);
    pub const INFO: Rgb = Rgb(0x82, 0xAA, 0xFF);
}

/// Whether the terminal advertises 24-bit colour.
///
/// Truecolor has no terminfo capability worth trusting, so the de-facto signal is `COLORTERM`,
/// which every terminal that supports it sets. A terminal that does not say so still gets the
/// palette, just flattened to its nearest 256-colour cube entry rather than a gradient.
pub fn supports_truecolor() -> bool {
    std::env::var_os("COLORTERM").is_some_and(|value| value == "truecolor" || value == "24bit")
}

/// The SGR sequence setting `color` as the foreground.
///
/// Call sites ask for a palette colour and get a string; they never assemble escapes themselves.
/// When the terminal has not advertised truecolor this degrades to the 256-colour cube, which
/// every ANSI terminal since xterm-256color renders.
pub fn fg(color: Rgb, truecolor: bool) -> String {
    if truecolor {
        format!("\x1b[38;2;{};{};{}m", color.0, color.1, color.2)
    } else {
        format!("\x1b[38;5;{}m", cube_index(color))
    }
}

/// Reset every attribute. Paired with [`fg`] at the end of a styled run.
pub const RESET: &str = "\x1b[0m";

/// Move to column one and clear the line, so the next write replaces the current row.
///
/// This is the whole mechanism behind an animated status line: one row, rewritten, never a new
/// line per update. Kept here so no caller hand-rolls the escape.
pub const CLEAR_ROW: &str = "\r\x1b[2K";

/// Hide and show the cursor around an animation, so it does not strobe at the write position.
pub const HIDE_CURSOR: &str = "\x1b[?25l";
pub const SHOW_CURSOR: &str = "\x1b[?25h";

/// The nearest entry in the 256-colour cube, used when the terminal has not advertised truecolor.
fn cube_index(color: Rgb) -> u8 {
    let axis = |value: u8| -> u32 {
        // The cube's six steps are 0, 95, 135, 175, 215, 255: an unequal ramp, so the boundary
        // for each step is the midpoint between it and the next rather than a fixed division.
        const STEPS: [u32; 6] = [0, 95, 135, 175, 215, 255];
        let value = value as u32;
        let mut best = 0;
        let mut best_distance = u32::MAX;
        let mut index = 0;
        while index < STEPS.len() {
            let distance = value.abs_diff(STEPS[index]);
            if distance < best_distance {
                best_distance = distance;
                best = index as u32;
            }
            index += 1;
        }
        best
    };
    (16 + 36 * axis(color.0) + 6 * axis(color.1) + axis(color.2)) as u8
}

fn non_empty(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

#[cfg(not(windows))]
fn enable_virtual_terminal() -> bool {
    true
}

/// Switch the console into virtual-terminal mode, reporting whether it is now in it.
///
/// Modern Windows Terminal already is; a legacy conhost is not until asked, and asking can fail
/// (a redirected handle, say), which is exactly when styling must be skipped. The result is cached
/// because the mode is process-wide and only needs setting once.
#[cfg(windows)]
fn enable_virtual_terminal() -> bool {
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle, STD_OUTPUT_HANDLE,
        SetConsoleMode,
    };

    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut mode = 0;
        if GetConsoleMode(handle, &mut mode) == 0 {
            return false;
        }
        mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING != 0
            || SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tty() -> ColorEnv {
        ColorEnv {
            no_color: false,
            clicolor_force: false,
            clicolor_off: false,
            term_dumb: false,
            is_terminal: true,
        }
    }

    #[test]
    fn color_follows_the_no_color_and_clicolor_conventions() {
        assert!(wants_color(&tty()));

        // A pipe gets no styling, so `rozi --help | less` stays readable.
        assert!(!wants_color(&ColorEnv {
            is_terminal: false,
            ..tty()
        }));
        assert!(!wants_color(&ColorEnv {
            no_color: true,
            ..tty()
        }));
        assert!(!wants_color(&ColorEnv {
            clicolor_off: true,
            ..tty()
        }));
        assert!(!wants_color(&ColorEnv {
            term_dumb: true,
            ..tty()
        }));

        // CLICOLOR_FORCE outranks the terminal check, but never NO_COLOR.
        assert!(wants_color(&ColorEnv {
            clicolor_force: true,
            is_terminal: false,
            ..tty()
        }));
        assert!(!wants_color(&ColorEnv {
            clicolor_force: true,
            no_color: true,
            ..tty()
        }));
    }
}
