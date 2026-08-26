//! Process-wide ownership of the terminal cursor's visibility.
//!
//! Hiding the cursor for an animation is easy to undo on every path the program controls, and
//! impossible to undo on the one it does not: a `Ctrl+C` terminates the process without unwinding,
//! so no `Drop` runs and the cursor stays hidden in the user's shell until they type `reset`. It is
//! the one artefact of a progress meter that outlives the program, so it gets a handler rather than
//! a destructor.
//!
//! Only `SIGINT` and `SIGQUIT` are claimed. `SIGTERM` and `SIGHUP` belong to
//! [`super::server_lifecycle`], which installs its own handlers for a clean detach, and a cosmetic
//! restore must not displace a shutdown path.

use std::sync::atomic::{AtomicBool, Ordering};

/// Whether the cursor is currently hidden by us, read by the signal handler to decide whether it
/// has anything to restore.
static HIDDEN: AtomicBool = AtomicBool::new(false);

/// Hide the cursor and arm the restore that survives a kill.
///
/// Idempotent: a second call while already hidden does nothing, so a redrawing meter can call it
/// freely. Writing the escape is the caller's job - this owns the *state*, so that the handler and
/// the ordinary path cannot disagree about whether anything is hidden.
pub fn hide() {
    if !HIDDEN.swap(true, Ordering::SeqCst) {
        imp::arm();
    }
}

/// Record that the cursor is visible again, disarming the restore.
pub fn show() {
    HIDDEN.store(false, Ordering::SeqCst);
}

/// Whether we currently believe the cursor is hidden.
pub fn is_hidden() -> bool {
    HIDDEN.load(Ordering::SeqCst)
}

#[cfg(unix)]
mod imp {
    use super::{HIDDEN, Ordering};
    use std::sync::OnceLock;

    /// The show-cursor escape as raw bytes. The handler cannot format a string - no allocation is
    /// async-signal-safe - so the sequence is a constant it can hand straight to `write`.
    const SHOW_CURSOR: &[u8] = b"\x1b[?25h";

    // `errno` lives behind a different accessor per platform; same split as `server_lifecycle`.
    #[cfg(target_os = "linux")]
    unsafe fn errno_slot() -> *mut libc::c_int {
        unsafe { libc::__errno_location() }
    }

    #[cfg(not(target_os = "linux"))]
    unsafe fn errno_slot() -> *mut libc::c_int {
        unsafe { libc::__error() }
    }

    /// Async-signal-safe: a relaxed atomic load, one `write`, and re-raising with the default
    /// disposition. `errno` is saved and restored around the write because the interrupted code may
    /// be mid-syscall and inspecting it afterwards.
    extern "C" fn restore_handler(signal: libc::c_int) {
        if HIDDEN.load(Ordering::SeqCst) {
            unsafe {
                let saved = *errno_slot();
                let _ = libc::write(
                    libc::STDERR_FILENO,
                    SHOW_CURSOR.as_ptr().cast::<libc::c_void>(),
                    SHOW_CURSOR.len(),
                );
                *errno_slot() = saved;
            }
        }
        // Re-raise with the default disposition so the process still dies of the signal it was
        // sent, with the exit status a shell expects. Restoring the cursor must not turn `Ctrl+C`
        // into something the user has to press twice.
        unsafe {
            libc::signal(signal, libc::SIG_DFL);
            libc::raise(signal);
        }
    }

    /// Take the handler as a function *pointer* rather than naming the item at the cast site, which
    /// is both what `server_lifecycle::install` does and what keeps the cast off a function item.
    fn install(signal: libc::c_int, handler: extern "C" fn(libc::c_int)) {
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = handler as usize;
            // No SA_RESTART: this handler never returns to the interrupted call, so there is
            // nothing to restart, and the default is what a terminating signal wants.
            action.sa_flags = 0;
            libc::sigemptyset(&mut action.sa_mask);
            // A failure here costs a hidden cursor on an abnormal exit, which is exactly the
            // situation this is trying to improve and never a reason to fail the download.
            let _ = libc::sigaction(signal, &action, std::ptr::null_mut());
        }
    }

    pub fn arm() {
        static INSTALLED: OnceLock<()> = OnceLock::new();
        if INSTALLED.set(()).is_err() {
            return;
        }
        for signal in [libc::SIGINT, libc::SIGQUIT] {
            install(signal, restore_handler);
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::{HIDDEN, Ordering};
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::FALSE;
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, SetConsoleCtrlHandler,
    };

    const SHOW_CURSOR: &str = "\x1b[?25h";

    /// Windows runs console control handlers on an OS-injected thread rather than in a signal
    /// context, so this may call ordinary code - unlike the Unix handler.
    ///
    /// Returns `FALSE` to decline the event, so the default termination still happens: the cursor
    /// is a cosmetic repair, not a reason to keep the process alive.
    unsafe extern "system" fn console_handler(event: u32) -> i32 {
        if matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT)
            && HIDDEN.load(Ordering::SeqCst)
        {
            use std::io::Write;
            let mut err = std::io::stderr().lock();
            let _ = err.write_all(SHOW_CURSOR.as_bytes());
            let _ = err.flush();
        }
        FALSE
    }

    pub fn arm() {
        static INSTALLED: OnceLock<()> = OnceLock::new();
        if INSTALLED.set(()).is_err() {
            return;
        }
        // A failure costs a hidden cursor on an abnormal exit and nothing else.
        unsafe {
            SetConsoleCtrlHandler(Some(console_handler), windows_sys::Win32::Foundation::TRUE)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hiding_and_showing_track_one_piece_of_state() {
        // The handler reads exactly this flag to decide whether it has anything to undo, so a
        // mismatch here is a cursor left hidden after Ctrl+C.
        show();
        assert!(!is_hidden());
        hide();
        assert!(is_hidden());
        // Idempotent: a redrawing meter calls `hide` on every frame.
        hide();
        assert!(is_hidden());
        show();
        assert!(!is_hidden());
        show();
        assert!(!is_hidden());
    }
}
