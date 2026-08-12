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
