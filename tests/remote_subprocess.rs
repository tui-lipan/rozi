//! Fake SSH boundary tests in isolated child processes, without global PATH mutation.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn noisy_ssh_completes_the_remote_protocol() {
    run_case("noisy");
}

#[test]
fn hung_ssh_is_killed_and_reaped() {
    run_case("hang");
}

#[test]
fn hung_remote_session_list_is_killed_and_reaped() {
    run_case("list_hang");
}

fn run_case(case: &str) {
    let root = tempfile::tempdir().unwrap();
    let ssh = root.path().join("ssh");
    std::fs::write(
        &ssh,
        r#"#!/bin/sh
while [ "$#" -gt 0 ] && [ "$1" != -- ]; do shift; done
shift
shift
case "$ROZI_SSH_CASE:$*" in
  "hang:"*"'sessions' 'kill'"* | "list_hang:"*"'sessions' 'list'"*)
    echo $$ > "$ROZI_SSH_PID"
    exec sleep 120 ;;
  *"--remote-serve"*)
    # More than the collector cap and a normal pipe buffer. Broken stderr must fail SSH.
    head -c 262144 /dev/zero >&2 || exit 91 ;;
esac
exec /bin/sh -c "$*"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "remote_subprocess_child", "--nocapture"])
        .env("ROZI_SSH_CASE", case)
        .env("ROZI_SSH_PID", root.path().join("pid"))
        .env("ROZI_TEST_BINARY", env!("CARGO_BIN_EXE_rozi"))
        .env(
            "ROZI_TEST_SESSION",
            format!("ssh-fixture-{}", std::process::id()),
        )
        .env("PATH", format!("{}:/usr/bin:/bin", root.path().display()))
        .env("HOME", root.path())
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_RUNTIME_DIR", root.path())
        .env_remove("ROZI_CONFIG")
        .env_remove("ROZI_REMOTE_BINARY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{case}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn remote_subprocess_child() {
    let Ok(case) = std::env::var("ROZI_SSH_CASE") else {
        return;
    };
    use rozi::config::{RemoteConfig, RemoteHostConfig};
    use rozi::session::remote::{connect_remote, kill_remote_session, parse_remote_target};
    let config = RemoteConfig {
        connection_timeout_secs: if case == "noisy" { 10 } else { 1 },
        hosts: [(
            "fixture".into(),
            RemoteHostConfig {
                binary_path: Some(env!("CARGO_BIN_EXE_rozi").into()),
                ..RemoteHostConfig::default()
            },
        )]
        .into(),
        ..RemoteConfig::default().unattended()
    };
    let target = parse_remote_target("fixture").unwrap();
    let session = std::env::var("ROZI_TEST_SESSION").unwrap();
    if case == "hang" || case == "list_hang" {
        let started = Instant::now();
        let error = if case == "hang" {
            kill_remote_session(&target, &session, &config).unwrap_err()
        } else {
            let source = rozi::session::discovery::SessionSource::Remote(target.clone());
            rozi::session::discovery::discover_sessions_from(&source, &config)
                .unwrap_err()
                .to_string()
        };
        assert!(error.contains("timed out"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(40));
        let pid: i32 = std::fs::read_to_string(std::env::var_os("ROZI_SSH_PID").unwrap())
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // This process spawned SSH: ECHILD proves it has already reaped it, ESRCH that it is gone.
        assert_eq!(
            unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    } else {
        let result =
            connect_remote(&target, &session, &config).and_then(|(connection, preamble)| {
                preamble.validate_for_client().unwrap();
                assert!(preamble.server_started);
                let (tx, _rx) = std::sync::mpsc::channel();
                let (client, attached) =
                    rozi::session::client::SessionClient::from_stream_attached_with_server_nonce(
                        connection,
                        session.clone(),
                        tx,
                        false,
                        preamble.server_nonce,
                    )?;
                assert!(matches!(
                    attached,
                    rozi::session::protocol::ServerMessage::Attached { .. }
                ));
                client.detach();
                Ok(())
            });
        let _ = Command::new(env!("CARGO_BIN_EXE_rozi"))
            .args(["sessions", "kill", &session])
            .output();
        result.unwrap();
    }
}
