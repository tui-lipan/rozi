//! A new session server started with only the environment its platform really provides.
//!
//! Every other subprocess test hands the server `HOME` and each `XDG_*` root on all platforms,
//! alongside the Windows `APPDATA` and `LOCALAPPDATA`. That keeps persistence out of the developer's
//! directories, but it also gave a Windows server a `HOME` a real Windows process normally lacks.
//! Code asking the Unix question - is `HOME` or `XDG_STATE_HOME` set? - passed under test and
//! failed for every user: a `--fresh-server` exited with "state directory unavailable" before
//! binding, and the client only saw "session server exited before it was ready (exit code: 1)".

use std::process::{Command, Stdio};

use rozi::session::protocol::{ClientMessage, Frame, ServerMessage};

use crate::common::{
    ServerGuard, attach_message, connect_when_ready, private_temp_dir, read_until,
    subprocess_endpoint, unique_session_name,
};

const UNIX_ONLY_VARS: &[&str] = &[
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "XDG_DATA_HOME",
    "XDG_RUNTIME_DIR",
];

#[test]
fn a_fresh_server_starts_with_only_the_platforms_own_user_directories() {
    let session = unique_session_name();
    let test_root = private_temp_dir();
    let runtime_base = test_root.join("runtime");
    let config_path = test_root.join("config.toml");
    std::fs::write(&config_path, "[session]\nresurrect = true\n")
        .expect("write isolated server config");
    let endpoint = subprocess_endpoint(&runtime_base, &session);

    let mut command = Command::new(env!("CARGO_BIN_EXE_rozi"));
    command.args(["--session", &session, "--fresh-server"]);
    for key in UNIX_ONLY_VARS {
        command.env_remove(key);
    }
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("ROZI_") {
            command.env_remove(key);
        }
    }
    if cfg!(windows) {
        // What a Windows login actually provides: AppData roots, no `HOME`, no `XDG_*`.
        command
            .env("APPDATA", test_root.join("AppData").join("Roaming"))
            .env("LOCALAPPDATA", test_root.join("AppData").join("Local"));
    } else {
        // The common Unix shape: `HOME` and the login runtime directory, no other `XDG_*` root.
        command
            .env("HOME", &test_root)
            .env("XDG_RUNTIME_DIR", &runtime_base)
            .env_remove("APPDATA")
            .env_remove("LOCALAPPDATA");
    }
    let child = command
        .env("ROZI_CONFIG", &config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("launch real session server");
    let mut server = ServerGuard::new(child, test_root.clone());

    // An early exit panics here with the server's stderr, which is where the state-directory
    // error surfaced.
    let mut client = connect_when_ready(&endpoint, server.child_mut());
    client.write_control(&attach_message(&session, "platform-env"));
    read_until(&mut client, |frame| {
        matches!(frame, Frame::Control(ServerMessage::Attached { .. }))
    });

    client.write_control(&ClientMessage::Shutdown);
    drop(client);
    server.wait_for_exit();
}

#[cfg(unix)]
#[test]
fn detached_start_reports_the_socket_limit_and_the_servers_startup_error() {
    let root = tempfile::tempdir_in(rozi::test_support::socket_safe_temp_dir()).unwrap();
    let root = root.path();
    let long_runtime = root.join("long".repeat(32));
    std::fs::create_dir(&long_runtime).unwrap();
    let runtime = root.join("run");
    std::fs::create_dir(&runtime).unwrap();
    let state_file = root.join("state");
    std::fs::write(&state_file, "").unwrap();
    let run = |runtime: &std::path::Path| {
        let output = Command::new(env!("CARGO_BIN_EXE_rozi"))
            .args(["sessions", "new", "test-startup", "--detach"])
            .env("XDG_RUNTIME_DIR", runtime)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_STATE_HOME", &state_file)
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("ROZI_CONFIG", root.join("config.toml"))
            .output()
            .unwrap();
        assert!(!output.status.success(), "{output:?}");
        String::from_utf8_lossy(&output.stderr).into_owned()
    };
    let message = run(&long_runtime);
    assert!(
        message.contains("socket path is") && message.contains("Use a shorter runtime directory"),
        "{message}"
    );

    let message = run(&runtime);
    assert!(
        message.contains("could not clear the saved session"),
        "{message}"
    );
    assert!(
        !subprocess_endpoint(&runtime, "test-startup")
            .path()
            .exists()
    );
}
