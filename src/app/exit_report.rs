//! Messages that can only be shown once the TUI has handed the terminal back.
//!
//! While the client runs, stderr is the terminal it draws on: a write lands at the focused pane's
//! cursor and stays there until something repaints those cells, and anything written on the
//! alternate screen vanishes when the client leaves it. Work that fails on the way out (after the
//! last frame, so too late for a toast) queues its report here instead, and
//! [`flush`] prints it after the runner has restored the terminal.

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
    for message in take() {
        eprintln!("rozi: {message}");
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

    #[test]
    fn deferred_messages_are_drained_in_order() {
        take();
        defer("first".to_string());
        defer("second".to_string());
        assert_eq!(take(), ["first", "second"]);
        assert!(take().is_empty());
    }
}
