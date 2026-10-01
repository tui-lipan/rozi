//! RAII host sleep inhibition. No backend requests display or lid-switch inhibition.
use std::io;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod backend {
    use super::*;
    use std::process::{Child, Command, Stdio};

    pub struct Guard(Child);

    fn command(reason: &str) -> Command {
        #[cfg(target_os = "linux")]
        let mut command = {
            let mut command = Command::new("systemd-inhibit");
            // systemd-inhibit holds the lock while cat runs. The pipe closes even if the
            // server is killed, so cat and its waiting helper exit without an orphaned lock.
            command.args([
                "--what=sleep",
                "--mode=block",
                "--who=rozi",
                "--no-ask-password",
                "--why",
                reason,
                "--",
                "cat",
            ]);
            command
        };
        #[cfg(target_os = "macos")]
        let mut command = {
            let _ = reason;
            let mut command = Command::new("caffeinate");
            command.args(["-i", "-w", &std::process::id().to_string()]);
            command
        };
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    impl Guard {
        pub fn acquire(reason: &str) -> io::Result<Self> {
            command(reason).spawn().map(Self)
        }

        pub fn check(&mut self) -> io::Result<()> {
            match self.0.try_wait()? {
                None => Ok(()),
                Some(status) => Err(io::Error::other(format!(
                    "sleep inhibitor exited: {status}"
                ))),
            }
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            // Keep release bounded even if the helper is unresponsive; always reap it.
            drop(self.0.stdin.take());
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn command_inhibits_only_system_sleep() {
            let cmd = command("agents working");
            let args: Vec<_> = cmd.get_args().map(|arg| arg.to_str().unwrap()).collect();
            #[cfg(target_os = "linux")]
            {
                assert_eq!(cmd.get_program(), "systemd-inhibit");
                assert_eq!(
                    args,
                    [
                        "--what=sleep",
                        "--mode=block",
                        "--who=rozi",
                        "--no-ask-password",
                        "--why",
                        "agents working",
                        "--",
                        "cat"
                    ]
                );
            }
            #[cfg(target_os = "macos")]
            {
                assert_eq!(cmd.get_program(), "caffeinate");
                assert_eq!(args, ["-i", "-w", &std::process::id().to_string()]);
            }
        }

        #[test]
        fn guard_kills_and_reaps_owned_child() {
            // Exercise ownership without invoking logind or acquiring any real lock.
            let child = Command::new("cat")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()
                .unwrap();
            let pid = child.id();
            drop(Guard(child));
            let mut status = 0;
            // SAFETY: status is writable; WNOHANG never blocks. A reaped child gives ECHILD.
            assert_eq!(
                unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) },
                -1
            );
            assert_eq!(
                io::Error::last_os_error().raw_os_error(),
                Some(libc::ECHILD)
            );
        }

        #[test]
        fn guard_detects_failed_helper() {
            let mut child = Command::new("cat").stdin(Stdio::piped()).spawn().unwrap();
            child.kill().unwrap();
            child.wait().unwrap();
            assert!(Guard(child).check().is_err());
        }
    }
}

#[cfg(windows)]
mod backend {
    use super::*;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Power::{
        PowerClearRequest, PowerCreateRequest, PowerRequestSystemRequired, PowerSetRequest,
    };
    use windows_sys::Win32::System::Threading::{
        POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0,
    };

    pub struct Guard(OwnedHandle);

    impl Guard {
        pub fn acquire(reason: &str) -> io::Result<Self> {
            let mut reason: Vec<u16> = reason.encode_utf16().chain(Some(0)).collect();
            let context = REASON_CONTEXT {
                Version: 0, // POWER_REQUEST_CONTEXT_VERSION
                Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
                Reason: REASON_CONTEXT_0 {
                    SimpleReasonString: reason.as_mut_ptr(),
                },
            };
            // SAFETY: the context and terminated UTF-16 string live through the call.
            let handle = unsafe { PowerCreateRequest(&context) };
            if handle == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: PowerCreateRequest returned a valid handle owned solely by this guard.
            let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
            // SAFETY: handle is a valid owned power request object.
            if unsafe { PowerSetRequest(handle.as_raw_handle(), PowerRequestSystemRequired) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(handle))
        }

        pub fn check(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            // SAFETY: this guard owns the handle and one SystemRequired request. OwnedHandle
            // closes it after the request is cleared, also releasing it if clearing fails.
            unsafe {
                PowerClearRequest(self.0.as_raw_handle(), PowerRequestSystemRequired);
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod backend {
    use super::*;
    pub struct Guard;
    impl Guard {
        pub fn acquire(_: &str) -> io::Result<Self> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "sleep inhibition is unavailable on this host",
            ))
        }
        pub fn check(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}

/// Dropping this guard releases the host's system-sleep inhibition.
pub struct SleepInhibitor(backend::Guard);

impl SleepInhibitor {
    pub fn acquire(reason: &str) -> io::Result<Self> {
        backend::Guard::acquire(reason).map(Self)
    }

    /// Detect asynchronous helper acquisition failure or unexpected termination.
    pub(crate) fn check(&mut self) -> io::Result<()> {
        self.0.check()
    }
}
