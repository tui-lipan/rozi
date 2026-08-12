//! End-to-end `--remote` transport coverage.
//!
//! Every other remote test stops at a seam — parsed targets, framed preambles, the piped transport
//! in isolation. These drive the whole path at two levels:
//!
//! - [`remote_serve_proxies_a_session_over_pipes`] spawns `--remote-serve` directly with piped
//!   stdio, which is exactly what ssh does to it. Covers autostart, the preamble's `server_started`
//!   flag, the piped transport, and a real attach — and runs everywhere.
//! - [`attaches_to_a_real_session_over_ssh_to_localhost`] adds genuine `ssh`, the one piece the
//!   first cannot approximate. Skipped unless key-based ssh to localhost already works
//!   non-interactively, since that is a machine setup this repository has no business creating.
//!
//! `binary_path` is pinned to the test binary so the probe short-circuits and **nothing is ever
//! installed on the host**.

mod common;

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;

use hyprmux::config::{RemoteConfig, RemoteHostConfig, RemoteInstallPolicy};
use hyprmux::session::client::SessionClient;
use hyprmux::session::protocol::{FILE_TREE_PROTOCOL, ServerMessage};
use hyprmux::session::remote::{connect_remote, parse_remote_target};

use common::unique_session_name;

/// Whether `ssh localhost` completes without any prompt. Anything else means "not set up here".
fn localhost_ssh_available() -> bool {
    if !hyprmux::platform::command::program_exists("ssh") {
        return false;
    }
    std::process::Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=5",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "localhost",
            "--",
            "true",
        ])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Config pinned to this test's binary, so probe/install never touches the host.
fn pinned_config() -> RemoteConfig {
    let mut hosts = HashMap::new();
    hosts.insert(
        "localhost".to_string(),
        RemoteHostConfig {
            binary_path: Some(env!("CARGO_BIN_EXE_hyprmux").to_string()),
            ..RemoteHostConfig::default()
        },
    );
    RemoteConfig {
        hosts,
        // Belt and braces: even if the pin were ignored, this refuses to mutate the host.
        install: RemoteInstallPolicy::Never,
        connection_timeout_secs: 10,
        ..RemoteConfig::default()
    }
}

/// Best-effort teardown so a real session server is not left running on the developer's machine.
fn kill_session(name: &str) {
    let _ = std::process::Command::new(env!("CARGO_BIN_EXE_hyprmux"))
        .args(["kill-session", name])
        .output();
}

#[test]
fn attaches_to_a_real_session_over_ssh_to_localhost() {
    if !localhost_ssh_available() {
        eprintln!("skipping: key-based ssh to localhost is not available");
        return;
    }

    let session = unique_session_name();
    let target = parse_remote_target("ssh://localhost").expect("parse target");
    let config = pinned_config();

    // First connect: no server for this name yet, so the proxy must autostart one and say so.
    let (stream, preamble) = match connect_remote(&target, &session, &config) {
        Ok(pair) => pair,
        Err(err) => {
            kill_session(&session);
            panic!("remote connect failed: {err}");
        }
    };
    assert!(
        preamble.server_started,
        "proxy must report that it started the server for a brand-new session"
    );
    assert!(
        preamble.protocol_max >= FILE_TREE_PROTOCOL,
        "remote is this same binary, so it must advertise our protocol range"
    );
    assert_eq!(preamble.hyprmux_version, env!("CARGO_PKG_VERSION"));

    // A real attach over the pipe: the session protocol does not know it is not a local socket.
    let (tx, _rx) = mpsc::channel();
    let attached = SessionClient::from_stream_attached(stream, session.clone(), tx, false);
    let (client, attached) = match attached {
        Ok(pair) => pair,
        Err(err) => {
            kill_session(&session);
            panic!("attach over the ssh pipe failed: {err}");
        }
    };
    match &attached {
        ServerMessage::Attached {
            session: name,
            effective_protocol,
            ..
        } => {
            assert_eq!(name, &session);
            assert!(
                *effective_protocol >= FILE_TREE_PROTOCOL,
                "same-build negotiation must reach the current version"
            );
        }
        other => {
            kill_session(&session);
            panic!("expected Attached, got {other:?}");
        }
    }
    assert_eq!(
        client.server_pid(),
        None,
        "a piped/remote connection has no meaningful local peer pid, so the forced\n\
         terminate_server fallback stays unreachable for remote sessions"
    );

    // Second connect to the *same* name must find the server already running. This is the flag
    // `create_only` keys off, replacing the local child-pid identity check that cannot work here.
    let (second, second_preamble) =
        connect_remote(&target, &session, &config).expect("second remote connect");
    assert!(
        !second_preamble.server_started,
        "an already-running remote session must not report server_started"
    );
    drop(second);

    client.detach();
    drop(client);
    // Give the proxy a moment to tear down before killing the session out from under it.
    std::thread::sleep(Duration::from_millis(200));
    kill_session(&session);
}

/// The proxy half of `--remote`, without needing an sshd.
///
/// `ssh <host> -- hyprmux --remote-serve <NAME>` is just "run this with piped stdio", so spawning
/// `--remote-serve` directly exercises everything hyprmux owns: autostart, the preamble and its
/// `server_started` flag, the piped transport, and a real attach over pipes. Only ssh itself is
/// absent — which is what [`attaches_to_a_real_session_over_ssh_to_localhost`] covers when a
/// machine has sshd. Unlike that test, this one runs everywhere, and `XDG_RUNTIME_DIR` keeps the
/// session server it starts inside a private temp dir.
#[test]
fn remote_serve_proxies_a_session_over_pipes() {
    use std::process::{Command, Stdio};

    let session = unique_session_name();
    let runtime_base = common::private_temp_dir();

    let spawn_proxy = || {
        Command::new(env!("CARGO_BIN_EXE_hyprmux"))
            .args(["--remote-serve", &session])
            .env("XDG_RUNTIME_DIR", &runtime_base)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn --remote-serve")
    };

    // First proxy: nothing is listening for this name, so it must autostart the server.
    let mut first =
        hyprmux::platform::ipc::connection_from_child(spawn_proxy()).expect("wrap proxy stdio");
    first
        .set_read_timeout(Some(Duration::from_secs(20)))
        .expect("set read timeout");
    // A closed stream here means the proxy child died before writing anything, and its stderr is
    // owned by the connection wrapper, so report what can actually be checked from here: the
    // runtime path the child had to fit a socket into.
    let preamble = hyprmux::session::remote::read_preamble(&mut first).unwrap_or_else(|error| {
        panic!(
            "read preamble: {error}\n  runtime base: {} ({} bytes before /hyprmux/<session>.sock; \
             sun_path caps at 104 on macOS, 108 on Linux)",
            runtime_base.display(),
            runtime_base.as_os_str().len()
        )
    });
    first.set_read_timeout(None).expect("clear read timeout");

    preamble
        .validate_for_client()
        .expect("same-build preamble must validate");
    assert!(
        preamble.server_started,
        "proxy must report starting the server for a brand-new session"
    );
    assert_eq!(preamble.hyprmux_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(preamble.platform, std::env::consts::OS);

    // The session protocol runs over the pipe exactly as it would over a socket.
    let (tx, _rx) = mpsc::channel();
    let (client, attached) = SessionClient::from_stream_attached(first, session.clone(), tx, false)
        .expect("attach over the proxy pipe");
    match &attached {
        ServerMessage::Attached { session: name, .. } => assert_eq!(name, &session),
        other => panic!("expected Attached, got {other:?}"),
    }
    assert_eq!(
        client.server_pid(),
        None,
        "piped connections must not expose a peer pid to the terminate_server fallback"
    );

    // A second proxy to the same name finds the server already up. This flag is what `create_only`
    // keys off remotely, replacing the local child-pid identity check.
    let mut second = hyprmux::platform::ipc::connection_from_child(spawn_proxy())
        .expect("wrap second proxy stdio");
    second
        .set_read_timeout(Some(Duration::from_secs(20)))
        .expect("set read timeout");
    let second_preamble =
        hyprmux::session::remote::read_preamble(&mut second).expect("read second preamble");
    assert!(
        !second_preamble.server_started,
        "an already-running session must not report server_started"
    );

    client.detach();
    drop(client);
    drop(second);

    // Tear the server down inside the private runtime dir rather than the user's real one.
    let _ = Command::new(env!("CARGO_BIN_EXE_hyprmux"))
        .args(["kill-session", &session])
        .env("XDG_RUNTIME_DIR", &runtime_base)
        .output();
    let _ = std::fs::remove_dir_all(&runtime_base);
}

#[test]
fn refuses_to_install_on_the_host_when_the_policy_forbids_it() {
    if !localhost_ssh_available() {
        eprintln!("skipping: key-based ssh to localhost is not available");
        return;
    }

    // No `binary_path`, `install = "never"`, and a probe that will not find this name on PATH.
    let mut config = RemoteConfig {
        install: RemoteInstallPolicy::Never,
        connection_timeout_secs: 10,
        ..RemoteConfig::default()
    };
    let mut hosts = HashMap::new();
    hosts.insert("localhost".to_string(), RemoteHostConfig::default());
    config.hosts = hosts;

    let target = parse_remote_target("ssh://localhost").expect("parse target");
    let session = unique_session_name();

    // Whether this succeeds depends on whether a real hyprmux happens to be installed on this
    // machine's PATH. Either outcome is fine — what must never happen is an install.
    match connect_remote(&target, &session, &config) {
        Ok((stream, _)) => {
            drop(stream);
            kill_session(&session);
        }
        Err(err) => {
            let message = format!("{err}");
            assert!(
                !message.contains("installed="),
                "install = \"never\" must not have written to the host: {message}"
            );
        }
    }
}

/// Attach to an arbitrary real host, configured out-of-band.
///
/// Point `HYPRMUX_CONFIG` at a `hyprmux.toml` holding a `[remote.hosts.<alias>]` entry and set
/// `HYPRMUX_TEST_REMOTE_ALIAS` to that alias. Skipped when unset, so it costs nothing in CI while
/// giving the manual "run it against a real box" pass a repeatable form — including hosts no gated
/// localhost test can stand in for, such as a Windows session server.
///
/// Pin `binary_path` in that config: this test asserts nothing is installed on the host.
#[test]
fn attaches_to_a_configured_real_remote() {
    let Ok(alias) = std::env::var("HYPRMUX_TEST_REMOTE_ALIAS") else {
        eprintln!("skipping: set HYPRMUX_CONFIG and HYPRMUX_TEST_REMOTE_ALIAS");
        return;
    };

    let config = hyprmux::config::load_config().config.remote;
    assert!(
        config.hosts.contains_key(&alias),
        "alias `{alias}` is not in the config named by HYPRMUX_CONFIG"
    );
    assert!(
        config.hosts[&alias].binary_path.is_some(),
        "pin binary_path for `{alias}`: this test must never install onto a real host"
    );

    let target = parse_remote_target(&alias).expect("parse alias");
    let session = unique_session_name();

    let (stream, preamble) =
        connect_remote(&target, &session, &config).expect("connect to configured remote");
    eprintln!(
        "remote: platform={} version={} protocol={}-{} server_started={}",
        preamble.platform,
        preamble.hyprmux_version,
        preamble.protocol_min,
        preamble.protocol_max,
        preamble.server_started
    );
    assert!(
        preamble.server_started,
        "a fresh session name must autostart a server on the remote"
    );

    let (tx, _rx) = mpsc::channel();
    let (client, attached) =
        SessionClient::from_stream_attached(stream, session.clone(), tx, false)
            .expect("attach over ssh to the configured remote");
    let first_id = match &attached {
        ServerMessage::Attached {
            session: name,
            effective_protocol,
            client_id,
            controller,
            clients,
            ..
        } => {
            eprintln!("attached to `{name}` at protocol {effective_protocol}");
            assert_eq!(name, &session);
            assert!(*effective_protocol >= 12);
            assert_eq!(
                *controller,
                Some(*client_id),
                "the first attacher must hold the layout-control lease"
            );
            assert_eq!(clients.len(), 1, "only one client is attached so far");
            *client_id
        }
        other => panic!("expected Attached, got {other:?}"),
    };

    // A second concurrent client sharing the same session: it attaches as a *follower* (the first
    // client keeps the lease) and both appear in the roster. This is the multi-client sharing path.
    let (second_stream, second_preamble) =
        connect_remote(&target, &session, &config).expect("second concurrent connect");
    assert!(
        !second_preamble.server_started,
        "an already-running session must not report server_started to a second client"
    );
    let (second_client, second_attached) = {
        let (tx2, _rx2) = mpsc::channel();
        SessionClient::from_stream_attached(second_stream, session.clone(), tx2, false)
            .expect("second client attach")
    };
    match &second_attached {
        ServerMessage::Attached {
            client_id,
            controller,
            clients,
            ..
        } => {
            assert_ne!(*client_id, first_id, "the second client gets a distinct id");
            assert_eq!(
                *controller,
                Some(first_id),
                "the lease stays with the first client; the newcomer is a follower"
            );
            assert_eq!(clients.len(), 2, "both clients share the session roster");
        }
        other => panic!("expected Attached for the second client, got {other:?}"),
    }
    second_client.detach();
    drop(second_client);

    // Detach the first client and confirm the server survives for reattach: a fresh connect to the
    // same name must find it already running (server_started == false), then attach cleanly again.
    client.detach();
    drop(client);
    std::thread::sleep(Duration::from_millis(300));

    let (reattach_stream, reattach_preamble) =
        connect_remote(&target, &session, &config).expect("reattach after detach");
    assert!(
        !reattach_preamble.server_started,
        "detach must leave the server running, so reattach finds it up"
    );
    let (reattached_client, reattached) = {
        let (tx3, _rx3) = mpsc::channel();
        SessionClient::from_stream_attached(reattach_stream, session.clone(), tx3, false)
            .expect("reattach over ssh")
    };
    match &reattached {
        ServerMessage::Attached { session: name, .. } => assert_eq!(name, &session),
        other => panic!("expected Attached on reattach, got {other:?}"),
    }
    reattached_client.detach();
    drop(reattached_client);
    std::thread::sleep(Duration::from_millis(300));

    let _ = hyprmux::session::remote::kill_remote_session(&target, &session, &config);
}
