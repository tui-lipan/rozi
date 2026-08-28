use std::io::Write;
use std::process::ExitCode;

/// The stack the UI runs on.
///
/// Rendering recurses through the whole view tree, deeply enough that every test in this
/// repository that renders the app has to ask for 8-16 MiB before it can - a default 2 MiB test
/// thread overflows. The process's first thread is the one place that budget was never set: it
/// gets whatever the platform hands it, which is 8 MiB on Linux and macOS but **1 MiB on Windows**,
/// where nothing passes `/STACK` to the linker. So the deepest view states - the sidebar open over
/// a remote attachment - render on a stack smaller than the app's own tests need, and Windows
/// overflows where the other platforms have room to spare.
///
/// Choosing the size here rather than inheriting it makes the budget the same on all three
/// platforms. It costs no memory: a thread stack is reserved address space, committed by the page
/// as it is used.
const UI_STACK_BYTES: usize = 32 * 1024 * 1024;

/// The budget has to clear what the app's own render tests ask for. A UI stack under that is one
/// the test suite has already shown to be too small.
const _: () = assert!(UI_STACK_BYTES >= 16 * 1024 * 1024);

fn main() -> ExitCode {
    match on_ui_thread(rozi::app::run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // A lost SSH terminal commonly makes stderr return EIO. Rust's `Result`-returning main
            // prints through an infallible stderr path and panics there, obscuring the real terminal
            // error with SIGABRT. Reporting is best-effort once the output device is already gone.
            report_runtime_error(&mut std::io::stderr().lock(), &error);
            ExitCode::FAILURE
        }
    }
}

/// Run `body` on a thread with [`UI_STACK_BYTES`] of stack and hand back what it returned.
///
/// The thread is named, so a panic - a stack overflow included - names `rozi-ui` rather than
/// `main`, which is what says which budget was exhausted. A panic is resumed here rather than
/// turned into an error: the default hook has already printed it, and finishing the unwind on this
/// thread leaves the process exiting exactly as it did when the UI ran on `main`.
/// A plain `fn` rather than a closure so it stays `Copy`: `spawn` takes the body by value, and the
/// fallback below still needs something to call when the thread was refused.
fn on_ui_thread<T: Send + 'static>(body: fn() -> T) -> T {
    let spawned = std::thread::Builder::new()
        .name("rozi-ui".to_string())
        .stack_size(UI_STACK_BYTES)
        .spawn(body);
    match spawned {
        Ok(handle) => match handle.join() {
            Ok(value) => value,
            Err(panic) => std::panic::resume_unwind(panic),
        },
        // Reserving address space is not an allocation, so this is close to unreachable - but a
        // host that refuses the thread should still get a running rozi rather than a launch error.
        Err(_) => body(),
    }
}

fn report_runtime_error(writer: &mut impl Write, error: &impl std::fmt::Debug) {
    let _ = writeln!(writer, "Error: {error:?}");
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_lost_stderr_does_not_panic_while_reporting_the_runtime_error() {
        report_runtime_error(&mut BrokenStderr, &"terminal disconnected");
    }

    #[test]
    fn the_ui_thread_hands_back_what_the_app_returned() {
        assert_eq!(on_ui_thread(|| "ran"), "ran");
    }

    fn panicking_ui() -> &'static str {
        panic!("ui panicked")
    }

    /// A panicking UI must still take the process down. Swallowing it here would turn a crash into
    /// a silent success exit, which is worse than the crash.
    #[test]
    fn a_panic_on_the_ui_thread_is_resumed_rather_than_swallowed() {
        // The default hook prints this panic; the message in the test output is expected.
        let outcome = std::panic::catch_unwind(|| on_ui_thread(panicking_ui));
        assert!(outcome.is_err());
    }
}
