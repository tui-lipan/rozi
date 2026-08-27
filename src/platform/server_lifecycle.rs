//! Cross-platform server existence and process control (cross-platform plan Phase 5b).
//!
//! Everything the app needs in order to *start*, *stop*, and *survive the death of* a session
//! server, expressed without Unix signal APIs or Win32 handles leaking into higher-level modules:
//!
//! - [`spawn_detached_server`] - background server spawn during session bootstrap. Unix starts a
//!   new process session and closes all three stdio streams; Windows additionally passes
//!   `DETACHED_PROCESS | CREATE_NO_WINDOW` so the server never inherits (or pops up) a console.
//! - [`on_hangup`] - the *client* half of console-control handling. Unix installs a `SIGHUP`/
//!   `SIGTERM` handler; Windows installs a `SetConsoleCtrlHandler` for Ctrl+C/close/logoff/shutdown.
//!   Both map to the same thing: run a clean detach instead of dying where we stand.
//! - [`install_shutdown_handler`] / [`shutdown_requested`] - the *server* half. The authenticated
//!   `ClientMessage::Shutdown` control message remains the primary, cross-platform stop mechanism;
//!   these make a signal/console event a courtesy path onto that same graceful teardown rather than
//!   an abrupt kill that would strand PTY children.
//! - [`contain_children`] - Windows orphan containment: the server puts itself and every ConPTY
//!   child into a kill-on-close Job Object, so a killed or crashed server cannot leave orphaned
//!   shells behind. Unix has no equivalent need (a signalled server reaps its own PTYs, and the
//!   escalation below guarantees it dies).
//! - [`terminate_server`] - forced termination of an unresponsive server, the last resort after the
//!   protocol handshake itself fails. Unix escalates `SIGTERM` to `SIGKILL`; Windows opens the
//!   process and terminates it (which, thanks to [`contain_children`], takes the job's ConPTY
//!   children with it).
//!
//! The Windows half is written against documented API contracts and is compiled and linted on
//! `windows-latest` in CI, but nothing exercises it: this module's own tests are `cfg(unix)`, and
//! the situations these functions exist for - a job reaping live ConPTY children when a server is
//! killed or crashes, and the console-control events a clean detach depends on - are not states the
//! suite puts any host into. Treat a change here as unproven until it has been run by hand on a
//! Windows console.

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set by [`install_shutdown_handler`]'s handler; polled by the session server's accept loop.
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Whether a signal (Unix) or console control event (Windows) has asked this server to stop.
///
/// The server loop polls this and takes the *same* teardown path an authenticated
/// `ClientMessage::Shutdown` takes, so a `SIGTERM`ed server still snapshots, closes its PTYs, and
/// unlinks its endpoint rather than dying mid-write.
pub fn shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
}

/// Spawn a background session server for `name` (`rozi --session <name> --server`), fully
/// detached from this process's terminal so it outlives the client that started it.
///
/// On Windows, OpenSSH runs each session inside a Job Object and kills the whole job when the SSH
/// connection drops — `DETACHED_PROCESS` alone does not escape that. `CREATE_BREAKAWAY_FROM_JOB`
/// pulls the server out of the job so it survives detach, but only works when the job carries
/// `JOB_OBJECT_LIMIT_BREAKAWAY_OK`; OpenSSH's job may not permit it. So the Windows path tries the
/// breakaway flag first and, if the spawn is refused for that reason, retries without it (matching
/// the pre-existing local behavior). A local server is not in a restrictive job, so breakaway is a
/// harmless no-op there.
pub fn spawn_detached_server(
    exe: &Path,
    name: &str,
    fresh: bool,
) -> io::Result<std::process::Child> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW, DETACHED_PROCESS,
        };
        let base = DETACHED_PROCESS | CREATE_NO_WINDOW;
        match spawn_server_with_flags(exe, name, fresh, base | CREATE_BREAKAWAY_FROM_JOB) {
            Ok(child) => Ok(child),
            // A job without `JOB_OBJECT_LIMIT_BREAKAWAY_OK` refuses the flag with ACCESS_DENIED;
            // fall back to a plain detached spawn so at least the local path keeps working.
            Err(err) if err.raw_os_error() == Some(5) => {
                spawn_server_with_flags(exe, name, fresh, base)
            }
            Err(err) => Err(err),
        }
    }
    #[cfg(not(windows))]
    {
        let mut command = base_server_command(exe, name, fresh);
        configure_detached_server(&mut command);
        command.spawn()
    }
}

fn base_server_command(exe: &Path, name: &str, fresh: bool) -> std::process::Command {
    let mut command = std::process::Command::new(exe);
    command
        .arg("--session")
        .arg(name)
        .arg(if fresh { "--fresh-server" } else { "--server" })
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command
}

#[cfg(unix)]
fn configure_detached_server(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;

    unsafe {
        command.pre_exec(|| {
            if libc::setsid() >= 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        });
    }
}

#[cfg(windows)]
fn spawn_server_with_flags(
    exe: &Path,
    name: &str,
    fresh: bool,
    flags: u32,
) -> io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    let mut command = base_server_command(exe, name, fresh);
    command.creation_flags(flags);
    command.spawn()
}

#[cfg(unix)]
mod imp {
    use super::{Ordering, SHUTDOWN_REQUESTED};
    use std::io;
    use std::sync::OnceLock;
    use std::sync::atomic::AtomicI32;

    /// Write end of the self-pipe the signal handler pokes. `-1` until [`super::on_hangup`] runs.
    static HANGUP_PIPE_WRITE: AtomicI32 = AtomicI32::new(-1);

    #[cfg(target_os = "linux")]
    unsafe fn errno_slot() -> *mut libc::c_int {
        unsafe { libc::__errno_location() }
    }

    #[cfg(not(target_os = "linux"))]
    unsafe fn errno_slot() -> *mut libc::c_int {
        unsafe { libc::__error() }
    }

    /// Async-signal-safe: only `write(2)` on a pipe, with `errno` saved and restored so the
    /// interrupted thread never observes a clobbered value.
    extern "C" fn hangup_handler(_signal: libc::c_int) {
        let fd = HANGUP_PIPE_WRITE.load(Ordering::Relaxed);
        if fd < 0 {
            return;
        }
        unsafe {
            let saved = *errno_slot();
            let byte: u8 = 1;
            let _ = libc::write(fd, std::ptr::from_ref(&byte).cast::<libc::c_void>(), 1);
            *errno_slot() = saved;
        }
    }

    /// Async-signal-safe: a single relaxed atomic store.
    extern "C" fn shutdown_handler(_signal: libc::c_int) {
        SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
    }

    fn install(signal: libc::c_int, handler: extern "C" fn(libc::c_int)) -> io::Result<()> {
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = handler as usize;
            action.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut action.sa_mask);
            if libc::sigaction(signal, &action, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    pub fn on_hangup(callback: Box<dyn Fn() + Send + 'static>) -> io::Result<()> {
        static INSTALLED: OnceLock<()> = OnceLock::new();
        if INSTALLED.get().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "hangup handler already installed",
            ));
        }

        let mut fds = [0 as libc::c_int; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // Neither end may survive into a spawned pane: a PTY child holding the write end open
        // would be harmless, but a child holding the *read* end open is a leak with no owner.
        for fd in fds {
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        }
        let (read_fd, write_fd) = (fds[0], fds[1]);
        HANGUP_PIPE_WRITE.store(write_fd, Ordering::SeqCst);

        install(libc::SIGHUP, hangup_handler)?;
        install(libc::SIGTERM, hangup_handler)?;
        let _ = INSTALLED.set(());

        std::thread::Builder::new()
            .name("rozi-hangup".to_string())
            .spawn(move || {
                let mut byte = [0u8; 1];
                loop {
                    let read =
                        unsafe { libc::read(read_fd, byte.as_mut_ptr().cast::<libc::c_void>(), 1) };
                    match read {
                        1 => callback(),
                        // EINTR: another signal interrupted the blocking read; go around again.
                        -1 if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted => {}
                        // 0 (write end closed) or a hard error: nothing left to watch.
                        _ => return,
                    }
                }
            })?;
        Ok(())
    }

    pub fn install_shutdown_handler() -> io::Result<()> {
        install(libc::SIGTERM, shutdown_handler)?;
        install(libc::SIGHUP, shutdown_handler)?;
        Ok(())
    }

    pub fn contain_children() -> io::Result<()> {
        // A Unix session server reaps its own PTY children on teardown, and `terminate_server`
        // guarantees it reaches teardown, so there is nothing extra to contain.
        Ok(())
    }

    /// `SIGTERM`, then `SIGKILL` if the process is still alive after the grace window.
    ///
    /// The server is not our child (it was detached at bootstrap, or predates this client
    /// entirely), so liveness is polled with `kill(pid, 0)` rather than `waitpid`.
    pub fn terminate_server(pid: u32) {
        use std::time::{Duration, Instant};

        let pid = pid as libc::pid_t;
        unsafe { libc::kill(pid, libc::SIGTERM) };

        let deadline = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < deadline {
            if !alive(pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }

    fn alive(pid: libc::pid_t) -> bool {
        // Signal 0 performs the permission/existence check without delivering anything. A live but
        // unreapable zombie is still "alive" here, which is correct: it is our caller's cue that
        // SIGTERM did not take, and SIGKILL on a zombie is harmless.
        unsafe { libc::kill(pid, 0) == 0 }
    }
}

#[cfg(windows)]
mod imp {
    use super::{Ordering, SHUTDOWN_REQUESTED};
    use std::io;
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::{CloseHandle, FALSE, HANDLE, TRUE};
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
        SetConsoleCtrlHandler,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, PROCESS_TERMINATE, TerminateProcess,
    };

    static HANGUP_CALLBACK: OnceLock<Box<dyn Fn() + Send + Sync + 'static>> = OnceLock::new();
    static HANDLE_SHUTDOWN: OnceLock<()> = OnceLock::new();

    /// Windows runs console control handlers on an OS-injected thread, not in a signal context, so
    /// this may call arbitrary code (unlike the Unix handler, which must stay async-signal-safe).
    ///
    /// Returning `TRUE` claims the event, suppressing the default "terminate the process" action so
    /// the clean detach/shutdown path gets to run. For `CTRL_CLOSE_EVENT`/`CTRL_LOGOFF_EVENT`/
    /// `CTRL_SHUTDOWN_EVENT` Windows still hard-kills us after a short timeout, which is exactly the
    /// window `on_hangup`'s detach needs.
    ///
    /// In practice the events that actually arrive in the *client* are the close/logoff/shutdown
    /// ones. The TUI puts the console in raw mode, which clears `ENABLE_PROCESSED_INPUT`, so a typed
    /// `Ctrl+C` is delivered as a key event to the focused pane rather than raised as a
    /// `CTRL_C_EVENT` here - as it should be. `CTRL_C_EVENT`/`CTRL_BREAK_EVENT` are still handled
    /// because a *server* has no raw-mode console, and because the client can receive one in the
    /// window before raw mode is entered; a clean detach is the right answer in both cases.
    unsafe extern "system" fn console_handler(event: u32) -> i32 {
        match event {
            CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT
            | CTRL_SHUTDOWN_EVENT => {
                if HANDLE_SHUTDOWN.get().is_some() {
                    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
                }
                if let Some(callback) = HANGUP_CALLBACK.get() {
                    callback();
                }
                TRUE
            }
            _ => FALSE,
        }
    }

    fn install_console_handler() -> io::Result<()> {
        // `SetConsoleCtrlHandler` is idempotent per function pointer only in the sense that adding
        // the same handler twice queues it twice; both call sites here funnel through this, and the
        // OnceLocks above make a second registration a no-op in effect, but guard it anyway.
        static INSTALLED: OnceLock<()> = OnceLock::new();
        if INSTALLED.set(()).is_err() {
            return Ok(());
        }
        if unsafe { SetConsoleCtrlHandler(Some(console_handler), TRUE) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn on_hangup(callback: Box<dyn Fn() + Send + 'static>) -> io::Result<()> {
        // The console handler runs on an OS thread, so the callback must be `Sync` to be shared
        // with it. Every caller passes a `CommandLink`-sending closure, which is; the `Send`-only
        // signature is kept so the two platform impls present one API.
        struct Shared(Box<dyn Fn() + Send + 'static>);
        // SAFETY: the handler thread is the only reader, and it only ever calls the closure; the
        // closures this module is given (`CommandLink::send`) are internally synchronized.
        unsafe impl Sync for Shared {}
        impl Shared {
            fn call(&self) {
                (self.0)();
            }
        }
        let shared = Shared(callback);
        if HANGUP_CALLBACK
            .set(Box::new(move || shared.call()))
            .is_err()
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "hangup handler already installed",
            ));
        }
        install_console_handler()
    }

    pub fn install_shutdown_handler() -> io::Result<()> {
        let _ = HANDLE_SHUTDOWN.set(());
        install_console_handler()
    }

    /// Put this process - and therefore every ConPTY child it later spawns - into a Job Object
    /// whose limits say "kill everything in the job when the last handle to it closes". The only
    /// handle is ours, so any exit path (clean, crashed, `TerminateProcess`d) closes it and takes
    /// the whole PTY tree down. Deliberately leaks the job handle: it must stay open for exactly as
    /// long as this process lives.
    pub fn contain_children() -> io::Result<()> {
        unsafe {
            let job: HANDLE = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                std::ptr::from_mut(&mut limits).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                let err = io::Error::last_os_error();
                CloseHandle(job);
                return Err(err);
            }
            if AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
                let err = io::Error::last_os_error();
                CloseHandle(job);
                return Err(err);
            }
            // `job` is a raw `HANDLE` with no `Drop`, so simply letting it fall out of scope leaks
            // it - which is the intent. Closing it would trip the kill-on-close limit and take this
            // process's own PTY children down immediately.
        }
        Ok(())
    }

    /// `TerminateProcess` on the server. Because the server called [`contain_children`] at startup,
    /// killing it closes its job handle and the job's kill-on-close limit takes every ConPTY child
    /// with it - the Windows equivalent of the Unix `SIGTERM`-then-`SIGKILL` escalation, minus the
    /// grace window (there is no signal to ask nicely with; the graceful ask was the protocol
    /// `Shutdown` message that already failed by the time we get here).
    pub fn terminate_server(pid: u32) {
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, FALSE, pid);
            if handle.is_null() {
                return;
            }
            TerminateProcess(handle, 1);
            CloseHandle(handle);
        }
    }
}

/// Run `callback` when this process is asked to go away by its controlling terminal (Unix `SIGHUP`/
/// `SIGTERM`) or its console (Windows Ctrl+C/close/logoff/shutdown).
///
/// Installed once per process; a second call is an error rather than a silent replacement. The
/// callback runs on a dedicated worker thread (Unix: draining a self-pipe the signal handler pokes,
/// which is what keeps the handler itself async-signal-safe), so it may do arbitrary work - in
/// practice it pushes one `Msg` onto the app's `CommandLink` and returns.
pub fn on_hangup(callback: impl Fn() + Send + 'static) -> io::Result<()> {
    imp::on_hangup(Box::new(callback))
}

/// Server-side: route a stop signal/console event onto the same graceful teardown the authenticated
/// protocol `Shutdown` message takes, observable via [`shutdown_requested`].
pub fn install_shutdown_handler() -> io::Result<()> {
    imp::install_shutdown_handler()
}

/// Server-side: ensure this process's PTY children cannot outlive it. No-op on Unix; a kill-on-close
/// Job Object on Windows.
pub fn contain_children() -> io::Result<()> {
    imp::contain_children()
}

/// Forcibly terminate a server that would not stop through the protocol. Best-effort and silent:
/// there is no recovery to offer a caller whose last resort just failed.
pub fn terminate_server(pid: u32) {
    imp::terminate_server(pid);
}

/// Whether this host can run rozi at all, with a user-facing reason when it cannot
/// (cross-platform plan Phase 10).
///
/// The one real gate is Windows: every pane is a PTY, and on Windows that means ConPTY, which
/// arrived in Windows 10 1809 (build 17763). On an older build there is no PTY to fall back to and
/// no partial mode worth offering - so this is checked once at startup and refused with an
/// explanation, rather than surfacing later as an inscrutable "failed to spawn pane" on every
/// single pane the user opens.
///
/// A modern VT-capable terminal host (Windows Terminal, or any conhost from that same build
/// onwards) is implied by the same check: the console-VT support rozi's rendering needs shipped
/// alongside ConPTY.
pub fn check_host_supported() -> Result<(), String> {
    #[cfg(windows)]
    {
        // `kernel32!CreatePseudoConsole` exists exactly on builds that have ConPTY. Probing for the
        // export is more honest than reading a version number, which lies under compatibility
        // shims and app-manifest quirks.
        use windows_sys::Win32::Foundation::FARPROC;
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

        let has_conpty: bool = unsafe {
            let kernel32 = GetModuleHandleA(c"kernel32.dll".as_ptr().cast());
            if kernel32.is_null() {
                false
            } else {
                let proc: FARPROC =
                    GetProcAddress(kernel32, c"CreatePseudoConsole".as_ptr().cast());
                proc.is_some()
            }
        };
        if !has_conpty {
            return Err(
                "rozi needs ConPTY, which requires Windows 10 version 1809 (build 17763) or \
                 newer. Update Windows, or run rozi under WSL."
                    .to_string(),
            );
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    /// One test rather than two: `on_hangup` is process-global and install-once, so a separate
    /// "rejects a second install" test would race the first install non-deterministically.
    #[test]
    fn sighup_reaches_the_callback_and_a_second_install_is_rejected() {
        let (tx, rx) = mpsc::channel();
        on_hangup(move || {
            let _ = tx.send(());
        })
        .expect("install hangup handler");

        unsafe { libc::raise(libc::SIGHUP) };
        rx.recv_timeout(Duration::from_secs(2))
            .expect("SIGHUP reached the callback");

        assert_eq!(
            on_hangup(|| {}).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists,
            "a second install must be rejected, not silently replace the live handler"
        );
    }

    #[test]
    fn terminate_server_escalates_to_sigkill_when_sigterm_is_ignored() {
        // `sh -c 'trap "" TERM; sleep 30'` ignores SIGTERM outright, so only the SIGKILL escalation
        // can reap it. Without the escalation this test hangs on `wait` until the 30s sleep ends.
        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("trap '' TERM; sleep 30")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn SIGTERM-ignoring child");

        terminate_server(child.id());

        let status = child.wait().expect("child reaped");
        assert!(
            !status.success(),
            "expected a signalled exit, got {status:?}"
        );
    }

    #[test]
    fn detached_server_starts_a_new_process_session() {
        let mut command = std::process::Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("sleep 30")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        configure_detached_server(&mut command);

        let mut child = command.spawn().expect("spawn detached child");
        let pid = child.id() as libc::pid_t;
        assert_eq!(unsafe { libc::getsid(pid) }, pid);

        let _ = child.kill();
        let _ = child.wait();
    }
}
