//! A reader that stops early, as in `rozi --version | head -1`, is an ordinary way for a command to
//! end, not a crash.
use std::process::{Command, Stdio};

/// Run `rozi` with its stdout already closed at the far end, and return its status and stderr.
fn run_with_stdout_closed(args: &[&str]) -> (std::process::ExitStatus, String) {
    let root = tempfile::tempdir().unwrap();
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let output = Command::new(env!("CARGO_BIN_EXE_rozi"))
        .args(args)
        .env("ROZI_CONFIG", root.path().join("config.toml"))
        .env("XDG_RUNTIME_DIR", root.path())
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env("APPDATA", root.path().join("config"))
        .env("LOCALAPPDATA", root.path().join("state"))
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()
        .expect("run rozi");
    (
        output.status,
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn a_reader_that_goes_away_ends_the_command_quietly() {
    for args in [
        &["--version"][..],
        &["--help"][..],
        &["sessions", "--help"][..],
        &["sessions", "list", "--format", "json"][..],
    ] {
        let (status, stderr) = run_with_stdout_closed(args);
        assert!(!stderr.contains("panicked"), "{args:?} panicked: {stderr}");
        assert!(status.success(), "{args:?} exited with {status}: {stderr}");
    }
}
