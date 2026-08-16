use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    match rozi::app::run() {
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
}
