mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use rozi::platform::paths::{self, PlatformEnv};
use rozi::session::protocol::{
    ClientMessage, Frame, FrameDecoder, FrameReadStatus, ServerMessage, write_frame,
};

fn environment(home: &Path) -> PlatformEnv {
    PlatformEnv {
        home: Some(home.into()),
        xdg_config_home: Some(home.join("config")),
        xdg_state_home: Some(home.join("state")),
        xdg_cache_home: Some(home.join("cache")),
        xdg_data_home: Some(home.join("data")),
        xdg_runtime_dir: Some(home.join("runtime")),
        appdata: Some(home.join("appdata")),
        local_appdata: Some(home.join("localappdata")),
    }
}

fn rozi(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rozi"));
    command
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_RUNTIME_DIR", home.join("runtime"))
        .env("APPDATA", home.join("appdata"))
        .env("LOCALAPPDATA", home.join("localappdata"))
        .env_remove("ROZI_CONFIG")
        .env_remove("ROZI_SOCKET")
        .env_remove("ROZI")
        .env_remove("ROZI_PANE")
        .env_remove("ROZI_SOURCE_PANE")
        .env_remove("ROZI_TERMINAL")
        .env("TERM_PROGRAM", "private-user@private-host")
        .env("TERM", "/home/private-user/terminal")
        .env("UNRELATED_SECRET", "private-token")
        .env("PATH", "");
    command
}

fn report(command: &mut Command, home: &Path) -> BTreeMap<String, String> {
    let output = command.output().expect("run doctor");
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    for private in [
        home.to_str().unwrap(),
        "private-user",
        "private-host",
        "private-token",
        "secret-session",
    ] {
        assert!(!text.contains(private), "leaked {private}: {text}");
    }
    text.lines()
        .map(|line| {
            let (key, value) = line.split_once("  ").expect("aligned field");
            (key.to_string(), value.trim().to_string())
        })
        .collect()
}

#[test]
fn doctor_reports_without_creating_user_directories() {
    let home = tempfile::tempdir().unwrap();
    let fields = report(rozi(home.path()).arg("doctor"), home.path());
    for key in [
        "rozi",
        "extension_api",
        "protocol_min",
        "protocol_max",
        "os",
        "arch",
        "install_source",
        "terminal",
        "config",
        "config_warnings",
        "sessions_running",
        "sessions_incompatible",
        "sessions_restorable",
        "extensions_installed",
        "ssh_on_path",
    ] {
        assert!(fields.contains_key(key), "missing {key}: {fields:?}");
    }
    assert_eq!(fields["config"], "valid");
    assert_eq!(fields["config_warnings"], "0");
    assert_eq!(fields["terminal"], "unknown");
    assert!(!fields.contains_key("config_errors"));
    // A pane may override `TERM_PROGRAM`; the terminal marker still decides.
    let in_pane = report(
        rozi(home.path())
            .arg("doctor")
            .env("ROZI", "1")
            .env("ROZI_PANE", "7")
            .env("ROZI_TERMINAL", "1")
            .env("TERM_PROGRAM", "kitty")
            .env("TERM", "xterm-kitty"),
        home.path(),
    );
    assert_eq!(in_pane["terminal"], "rozi (inside a pane)");
    // A command the client runs outside any pane names the focused pane in `ROZI_PANE`.
    let client_command = report(
        rozi(home.path())
            .arg("doctor")
            .env("ROZI", "1")
            .env("ROZI_PANE", "7")
            .env("ROZI_SOURCE_PANE", "7")
            .env("TERM_PROGRAM", "kitty")
            .env("TERM", "xterm-kitty"),
        home.path(),
    );
    assert_eq!(client_command["terminal"], "kitty");
    assert_eq!(fields["ssh_on_path"], "no");
    assert_eq!(fields["sessions_running"], "0");
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);

    let version = rozi(home.path()).arg("--version").output().unwrap();
    for line in String::from_utf8(version.stdout).unwrap().lines().skip(1) {
        let (key, value) = line.split_once('=').unwrap();
        assert_eq!(fields[key], value);
    }
}

#[test]
fn doctor_uses_real_validation_and_config_overrides_without_printing_errors() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("private-config.toml");
    for (text, verdict) in [
        ("invalid = [private-token", "rejected"),
        ("[recording]\nmax_fps = 0\n", "valid, 1 warning"),
        ("stale_private_key = 'private-token'\n", "valid, 1 warning"),
    ] {
        std::fs::write(&config, text).unwrap();
        for args in [
            vec!["--config", config.to_str().unwrap(), "doctor"],
            vec!["doctor", "--config", config.to_str().unwrap()],
        ] {
            let fields = report(rozi(home.path()).args(args), home.path());
            assert_eq!(fields["config"], verdict);
            assert!(fields["config_warnings"].parse::<usize>().unwrap() > 0);
            assert_eq!(std::fs::read_to_string(&config).unwrap(), text);
        }
    }
    std::fs::write(&config, "").unwrap();
    let fields = report(
        rozi(home.path()).arg("doctor").env("ROZI_CONFIG", &config),
        home.path(),
    );
    assert_eq!(fields["config"], "valid");
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 1);
}

#[test]
fn doctor_counts_installed_extensions_without_creating_runtime_state() {
    let home = tempfile::tempdir().unwrap();
    let env = environment(home.path());
    let extension = paths::extensions_dir(&env).join("secret-extension");
    std::fs::create_dir_all(&extension).unwrap();
    let manifest =
        "[extension]\nid = 'secret-extension'\ntitle = 'private-token'\nversion = '1'\napi = 1\n";
    std::fs::write(extension.join("extension.toml"), manifest).unwrap();
    let fields = report(rozi(home.path()).arg("doctor"), home.path());
    assert_eq!(fields["extensions_installed"], "1");
    assert_eq!(fields["config"], "valid");
    assert!(!fields.contains_key("extensions_running"));
    assert!(!fields.contains_key("extensions_failed"));
    assert!(!paths::runtime_dir_path(&env).exists());
    assert!(!paths::state_dir(&env).exists());
    assert!(!paths::config_dir(&env).exists());
    assert!(!paths::data_dir(&env).join("extension-state").exists());
    assert_eq!(
        std::fs::read_to_string(extension.join("extension.toml")).unwrap(),
        manifest
    );
}

#[test]
fn doctor_counts_session_replies_and_snapshots_and_keeps_stale_entries() {
    let root = common::private_temp_dir();
    let home = root.as_path();
    let env = environment(home);
    let runtime = paths::runtime_dir(&env).unwrap();
    let stale = runtime.join("session-secret-session-stale.sock");
    std::fs::write(&stale, b"stale").unwrap();
    let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut workers = Vec::new();
    for (name, reply) in [
        (
            "secret-session-running",
            ServerMessage::SessionInfo {
                capabilities: None,
                agents: vec![],
                projects: vec![],
                session: "secret-session-running".into(),
                panes: 1,
                clients: 0,
                has_layout: true,
                effective_protocol: rozi::session::protocol::PROTOCOL_VERSION,
                origin: Default::default(),
                instance: None,
                rozi_version: Some("0.0.41".into()),
            },
        ),
        (
            "secret-session-incompatible",
            ServerMessage::Error {
                code: "protocol-mismatch".into(),
                message: "private-token".into(),
                peer: None,
            },
        ),
    ] {
        let endpoint = rozi::platform::ipc::EndpointRegistry::session_endpoint(&runtime, name);
        let listener = endpoint.bind().unwrap().into_listener();
        listener.set_nonblocking(true).unwrap();
        let finished = finished.clone();
        workers.push(std::thread::spawn(move || {
            while !finished.load(std::sync::atomic::Ordering::Relaxed) {
                let mut stream = match listener.accept() {
                    Ok(stream) => stream,
                    Err(err) if rozi::platform::ipc::is_busy_error(&err) => {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                        continue;
                    }
                    Err(err) => panic!("{err}"),
                };
                // The doctor's short probe can expire before this worker is scheduled. Avoid
                // socket timeout options on a peer that may already have closed, and keep the
                // production decoder's partial frame state while polling.
                stream.set_nonblocking(true).unwrap();
                if !read_doctor_query(&mut stream) {
                    continue;
                }
                stream.set_nonblocking(false).unwrap();
                if let Err(err) = write_frame(&mut stream, &reply) {
                    assert!(
                        matches!(
                            err.kind(),
                            std::io::ErrorKind::BrokenPipe
                                | std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::ConnectionAborted
                                | std::io::ErrorKind::NotConnected
                        ),
                        "could not answer doctor: {err}"
                    );
                }
            }
        }));
    }
    for name in [
        "secret-session-running",
        "secret-session-incompatible",
        "secret-session-saved",
    ] {
        let dir = paths::state_dir(&env).join("sessions").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("meta.json"),
            serde_json::to_vec(&serde_json::json!({
                "version": 3, "session": name, "saved_at": 1, "layout_rev": 0, "panes": []
            }))
            .unwrap(),
        )
        .unwrap();
    }
    // A single 60 ms production probe can legitimately time out under CI load. Keep serving
    // queries until a complete report arrives, bounded by the harness deadline.
    let deadline = std::time::Instant::now() + common::io_timeout();
    let fields = loop {
        let fields = report(rozi(home).arg("doctor"), home);
        if (fields["sessions_running"] == "1"
            && fields["sessions_incompatible"] == "1"
            && fields["sessions_restorable"] == "1")
            || std::time::Instant::now() >= deadline
        {
            break fields;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    finished.store(true, std::sync::atomic::Ordering::Relaxed);
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(fields["sessions_running"], "1");
    assert_eq!(
        fields["sessions_versions"],
        format!(
            "0.0.41 (protocol {})",
            rozi::session::protocol::PROTOCOL_VERSION
        )
    );
    assert_eq!(fields["sessions_incompatible"], "1");
    assert_eq!(fields["sessions_restorable"], "1");
    assert_eq!(std::fs::read(&stale).unwrap(), b"stale");
    std::fs::remove_dir_all(root).unwrap();
}

/// Read the actual typed query without assuming the client stays connected long enough to set
/// socket options. EOF is an expired probe; malformed or unexpected frames still fail the test.
fn read_doctor_query(stream: &mut rozi::platform::ipc::IpcConnection) -> bool {
    let mut decoder = FrameDecoder::default();
    let deadline = std::time::Instant::now() + common::io_timeout();
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "doctor did not send its query"
        );
        if let Some(frame) = decoder.next_frame::<ClientMessage>().unwrap() {
            assert!(
                matches!(frame, Frame::Control(ClientMessage::Query { .. })),
                "{frame:?}"
            );
            return true;
        }
        match decoder.read_from_status(stream).unwrap() {
            FrameReadStatus::Eof => return false,
            FrameReadStatus::WouldBlock => std::thread::sleep(std::time::Duration::from_millis(1)),
            FrameReadStatus::Read(_) => {}
        }
    }
}

#[test]
fn doctor_is_in_help_and_rejects_launch_arguments() {
    let home = tempfile::tempdir().unwrap();
    let help = rozi(home.path()).arg("--help").output().unwrap();
    assert!(String::from_utf8(help.stdout).unwrap().contains("doctor"));
    for args in [
        vec!["doctor", "extra"],
        vec!["--session", "secret-session", "doctor"],
        vec!["doctor", "--config"],
    ] {
        let output = rozi(home.path()).args(args).output().unwrap();
        assert!(!output.status.success());
    }
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
}
