//! Messages that can only be shown once the TUI has handed the terminal back.
//!
//! While the client runs, stderr is the terminal it draws on: a write lands at the focused pane's
//! cursor and stays there until something repaints those cells, and anything written on the
//! alternate screen vanishes when the client leaves it. Work that fails on the way out (after the
//! last frame, so too late for a toast) queues its report here instead, and
//! [`flush`] prints it after the runner has restored the terminal.

use std::io::Write;
use std::sync::Mutex;

static PENDING: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Queue `message` to be printed on stderr once the TUI has exited.
pub(crate) fn defer(message: String) {
    PENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(message);
}

/// Print and clear every queued message. Call only after the TUI has released the terminal.
pub(crate) fn flush() {
    flush_to(&mut std::io::stderr().lock());
}

/// Best-effort, like `main`'s runtime-error report: after a terminal hangup stderr commonly
/// returns EIO, and `eprintln!` would panic there instead of letting the exit finish.
fn flush_to(writer: &mut impl Write) {
    for message in take() {
        let _ = writeln!(writer, "rozi: {message}");
    }
}

fn take() -> Vec<String> {
    std::mem::take(
        &mut *PENDING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The queue is process-wide, so tests that drain it must not interleave.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn deferred_messages_are_drained_in_order() {
        let _serial = serial();
        take();
        defer("first".to_string());
        defer("second".to_string());
        assert_eq!(take(), ["first", "second"]);
        assert!(take().is_empty());
    }

    struct BrokenStderr;

    impl Write for BrokenStderr {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from_raw_os_error(5))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::from_raw_os_error(5))
        }
    }

    #[test]
    fn a_lost_stderr_does_not_panic_while_flushing() {
        let _serial = serial();
        take();
        defer("session autosave failed: disk full".to_string());
        flush_to(&mut BrokenStderr);
        assert!(take().is_empty());
    }
}
