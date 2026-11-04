//! Deployment acknowledgements and crash persistence through a real session server.
mod common;

use std::path::Path;
use std::process::{Command, Stdio};

use base64::Engine as _;
use common::{
    ServerGuard, connect_when_ready, private_temp_dir, read_until, subprocess_endpoint,
    unique_session_name,
};
use rozi::control::{ControlCommand, ControlRequest, ControlResponse, ExtensionDeploymentReport};
use rozi::extension_runtime::bundle::{Bundle, BundleFile};
use rozi::session::protocol::{
    ClientMessage, Frame, MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION, ServerMessage,
};

fn rozi(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rozi"));
    command
        .env("ROZI_CONFIG", root.join("config.toml"))
        .env("XDG_RUNTIME_DIR", root)
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("APPDATA", root.join("config"))
        .env("LOCALAPPDATA", root.join("state"));
    command
}

fn spawn(session: &str, root: &Path) -> std::process::Child {
    rozi(root)
        .args(["--server", session])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start session server")
}

fn ask(
    server: &mut ServerGuard,
    root: &Path,
    session: &str,
    command: ControlCommand,
) -> ControlResponse {
    let response = ask_request(
        server,
        root,
        session,
        ControlRequest {
            command,
            source_pane: None,
            source_session: None,
            caller_context: None,
            extension: None,
            credential: None,
        },
    );
    assert!(response.ok, "{response:?}");
    response
}

fn ask_request(
    server: &mut ServerGuard,
    root: &Path,
    session: &str,
    request: ControlRequest,
) -> ControlResponse {
    let mut client = connect_when_ready(&subprocess_endpoint(root, session), server.child_mut());
    client.write_control(&ClientMessage::SessionControl {
        capabilities: None,
        session: session.into(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: MIN_SUPPORTED_PROTOCOL,
        request,
    });
    let mut response = None;
    read_until(&mut client, |frame| match frame {
        Frame::Control(ServerMessage::SessionControlResult {
            response: reply, ..
        }) => {
            response = Some(reply.clone());
            true
        }
        _ => false,
    });
    response.expect("server replied")
}

#[test]
fn a_chunked_deployment_survives_an_immediate_server_crash() {
    let root = private_temp_dir();
    std::fs::write(root.join("config.toml"), "[session]\nresurrect = true\n").unwrap();
    let session = unique_session_name();
    let mut server = ServerGuard::new(spawn(&session, &root), root.clone());
    let bundle = Bundle::from_files(vec![
        BundleFile {
            path: "extension.toml".into(),
            executable: false,
            contents: b"[extension]\nid='demo'\ntitle='Demo'\nversion='1'\napi=1\n".to_vec(),
        },
        BundleFile {
            path: "payload".into(),
            executable: false,
            contents: vec![b'x'; 700_000],
        },
    ])
    .unwrap();
    let archive = bundle.archive();
    for (index, data) in archive.chunks(512 * 1024).enumerate() {
        let offset = index * 512 * 1024;
        let response = ask(
            &mut server,
            &root,
            &session,
            ControlCommand::ExtensionsDeploy {
                id: "demo".into(),
                digest: bundle.digest().into(),
                size: archive.len() as u64,
                offset: offset as u64,
                data: base64::engine::general_purpose::STANDARD.encode(data),
                last: offset + data.len() == archive.len(),
            },
        );
        assert_eq!(
            response.data.unwrap()["complete"],
            offset + data.len() == archive.len()
        );
    }
    server.kill_for_restart();
    server.replace_child(spawn(&session, &root));
    let report = ask(
        &mut server,
        &root,
        &session,
        ControlCommand::ExtensionsStatus,
    );
    let report: ExtensionDeploymentReport = serde_json::from_value(report.data.unwrap()).unwrap();
    assert_eq!(report.deployments.len(), 1);
    assert_eq!(report.deployments[0].digest, bundle.digest());
    assert_eq!(
        report.deployments[0].status,
        rozi::control::ExtensionDeploymentStatus::Deployed
    );
    ask(
        &mut server,
        &root,
        &session,
        ControlCommand::ExtensionsUndeploy { id: "demo".into() },
    );
    let report = ask(
        &mut server,
        &root,
        &session,
        ControlCommand::ExtensionsStatus,
    );
    assert_eq!(report.data.unwrap()["deployments"], serde_json::json!([]));
}

fn deployment_count(server: &mut ServerGuard, root: &Path, session: &str) -> usize {
    let reply = ask(server, root, session, ControlCommand::ExtensionsStatus);
    let report: ExtensionDeploymentReport = serde_json::from_value(reply.data.unwrap()).unwrap();
    report.deployments.len()
}

/// Attach, ask the server to end the session this way, and wait for it to exit.
fn end_session(server: &mut ServerGuard, root: &Path, session: &str, ending: ClientMessage) {
    let mut client = connect_when_ready(&subprocess_endpoint(root, session), server.child_mut());
    client.write_control(&common::attach_message(session, "ender"));
    read_until(&mut client, |frame| {
        matches!(frame, Frame::Control(ServerMessage::Attached { .. }))
    });
    client.write_control(&ending);
    let deadline = std::time::Instant::now() + common::io_timeout();
    while server.child_mut().try_wait().unwrap().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "the server did not exit after {ending:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// A restart ends the panes and keeps what is deployed. Only deleting the session removes that.
#[test]
fn a_restart_keeps_deployments_and_a_deletion_removes_them() {
    let root = private_temp_dir();
    std::fs::write(root.join("config.toml"), "[session]\nresurrect = true\n").unwrap();
    let session = unique_session_name();
    let mut server = ServerGuard::new(spawn(&session, &root), root.clone());
    let bundle = Bundle::from_files(vec![BundleFile {
        path: "extension.toml".into(),
        executable: false,
        contents: b"[extension]\nid='demo'\ntitle='Demo'\nversion='1'\napi=1\n".to_vec(),
    }])
    .unwrap();
    ask(&mut server, &root, &session, deploy_command(&bundle));

    end_session(
        &mut server,
        &root,
        &session,
        ClientMessage::ShutdownForRestart,
    );
    server.replace_child(spawn(&session, &root));
    assert_eq!(deployment_count(&mut server, &root, &session), 1);

    end_session(&mut server, &root, &session, ClientMessage::Shutdown);
    server.replace_child(spawn(&session, &root));
    assert_eq!(deployment_count(&mut server, &root, &session), 0);
}

#[cfg(unix)]
fn pane_count(root: &Path, session: &str) -> usize {
    let done = rozi(root)
        .args(["--session", session, "list-panes", "--format", "json"])
        .output()
        .expect("list panes");
    assert!(done.status.success(), "{done:?}");
    let reply: ControlResponse = serde_json::from_slice(&done.stdout).unwrap();
    reply.data.unwrap().as_array().unwrap().len()
}

#[cfg(unix)]
fn split_a_pane(root: &Path, session: &str) {
    let done = rozi(root)
        .args(["--session", session, "split", "--argv", "sleep", "60"])
        .output()
        .expect("split");
    assert!(done.status.success(), "{done:?}");
}

/// A stop leaves a snapshot to restore. A restart is what discards it, so the next server has no
/// panes even though it is free to restore them.
#[cfg(unix)]
#[test]
fn a_stopped_session_restores_its_panes_and_a_restarted_one_does_not() {
    let root = private_temp_dir();
    std::fs::write(root.join("config.toml"), "[session]\nresurrect = true\n").unwrap();
    let session = unique_session_name();
    let mut server = ServerGuard::new(spawn(&session, &root), root.clone());
    connect_when_ready(&subprocess_endpoint(&root, &session), server.child_mut());
    split_a_pane(&root, &session);

    let pid = server.child_mut().id().to_string();
    let signalled = Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .expect("signal the server");
    assert!(signalled.success());
    server.child_mut().wait().unwrap();
    server.replace_child(spawn(&session, &root));
    connect_when_ready(&subprocess_endpoint(&root, &session), server.child_mut());
    assert_eq!(pane_count(&root, &session), 1, "a stop keeps the snapshot");

    end_session(
        &mut server,
        &root,
        &session,
        ClientMessage::ShutdownForRestart,
    );
    server.replace_child(spawn(&session, &root));
    connect_when_ready(&subprocess_endpoint(&root, &session), server.child_mut());
    assert_eq!(pane_count(&root, &session), 0, "a restart discards it");
}

/// The command a script runs. Unix only: captured output hangs a detached start on Windows, #308.
#[cfg(unix)]
#[test]
fn sessions_restart_starts_fresh_panes_and_keeps_deployments() {
    let root = private_temp_dir();
    std::fs::write(root.join("config.toml"), "[session]\nresurrect = true\n").unwrap();
    let session = unique_session_name();
    let sessions = |args: &[&str]| {
        rozi(&root)
            .arg("sessions")
            .args(args)
            .arg(&session)
            .output()
            .expect("run a sessions command")
    };

    let missing = sessions(&["restart"]);
    assert!(!missing.status.success());
    let stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(stderr.contains("is not running"), "{stderr}");
    assert!(stderr.contains("sessions new"), "{stderr}");

    let created = rozi(&root)
        .args(["sessions", "new", &session, "--detach"])
        .output()
        .expect("sessions new");
    assert!(created.status.success(), "{created:?}");
    install_demo(&root, "demo", "payload");
    // rozi refuses a data directory others can read, and `install_demo` leaves the umask's mode.
    use std::os::unix::fs::PermissionsExt as _;
    for directory in [root.join("data"), root.join("data/rozi")] {
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let deployed = deploy_with_cli(&root, &session);
    assert!(deployed.status.success(), "{deployed:?}");
    split_a_pane(&root, &session);
    assert_eq!(pane_count(&root, &session), 1);

    let restarted = sessions(&["restart"]);
    assert!(restarted.status.success(), "{restarted:?}");
    assert_eq!(pane_count(&root, &session), 0, "fresh panes");
    assert_eq!(
        local_copy_from_cli(&root, &session),
        rozi::control::ExtensionLocalCopy::Matches,
        "the deployment is still there"
    );

    let killed = sessions(&["kill"]);
    assert!(killed.status.success(), "{killed:?}");
    let missing = sessions(&["restart"]);
    assert!(!missing.status.success(), "a killed session is not running");
}

fn listed_status(root: &Path, session: &str) -> Option<String> {
    let done = rozi(root)
        .args(["sessions", "list", "--format", "json"])
        .output()
        .expect("sessions list");
    assert!(done.status.success(), "{done:?}");
    let rows: serde_json::Value = serde_json::from_slice(&done.stdout).unwrap();
    rows.as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == session)
        .map(|row| row["status"].as_str().unwrap().to_string())
}

/// A server can crash before it has saved anything, and what is deployed to it is still on disk,
/// still trusted code. Listing it is what lets anyone find it, and deleting it has to work.
#[test]
fn a_crashed_session_with_a_deployment_stays_listed_until_it_is_deleted() {
    let root = private_temp_dir();
    std::fs::write(root.join("config.toml"), "[session]\nresurrect = true\n").unwrap();
    let session = unique_session_name();
    let mut server = ServerGuard::new(spawn(&session, &root), root.clone());
    let bundle = Bundle::from_files(vec![BundleFile {
        path: "extension.toml".into(),
        executable: false,
        contents: b"[extension]\nid='demo'\ntitle='Demo'\nversion='1'\napi=1\n".to_vec(),
    }])
    .unwrap();
    ask(&mut server, &root, &session, deploy_command(&bundle));
    server.kill_for_restart();
    assert_eq!(
        listed_status(&root, &session).as_deref(),
        Some("restorable"),
        "nothing was ever snapshotted, and it is still deployed"
    );

    let killed = rozi(&root)
        .args(["sessions", "kill", &session])
        .output()
        .expect("sessions kill");
    assert!(killed.status.success(), "{killed:?}");
    assert_eq!(listed_status(&root, &session), None, "and now it is gone");
}

/// A session with nothing deployed left has nothing to find.
#[test]
fn an_undeployed_session_that_crashed_is_not_listed() {
    let root = private_temp_dir();
    std::fs::write(root.join("config.toml"), "[session]\nresurrect = true\n").unwrap();
    let session = unique_session_name();
    let mut server = ServerGuard::new(spawn(&session, &root), root.clone());
    let bundle = Bundle::from_files(vec![BundleFile {
        path: "extension.toml".into(),
        executable: false,
        contents: b"[extension]\nid='demo'\ntitle='Demo'\nversion='1'\napi=1\n".to_vec(),
    }])
    .unwrap();
    ask(&mut server, &root, &session, deploy_command(&bundle));
    ask(
        &mut server,
        &root,
        &session,
        ControlCommand::ExtensionsUndeploy { id: "demo".into() },
    );
    server.kill_for_restart();
    assert_eq!(listed_status(&root, &session), None);
}

/// `sessions new` is the way back after a crash. It discards the panes, not what was deployed,
/// and deleting the session is still what removes those.
#[test]
fn starting_a_session_fresh_keeps_its_deployments_and_killing_it_removes_them() {
    let root = private_temp_dir();
    std::fs::write(root.join("config.toml"), "[session]\nresurrect = true\n").unwrap();
    let session = unique_session_name();
    let mut server = ServerGuard::new(spawn(&session, &root), root.clone());
    let bundle = Bundle::from_files(vec![BundleFile {
        path: "extension.toml".into(),
        executable: false,
        contents: b"[extension]\nid='demo'\ntitle='Demo'\nversion='1'\napi=1\n".to_vec(),
    }])
    .unwrap();
    ask(&mut server, &root, &session, deploy_command(&bundle));
    server.kill_for_restart();

    // What `sessions new` starts, in the foreground so this test owns the process.
    let start_fresh = |root: &Path| {
        rozi(root)
            .args(["--session", &session, "--fresh-server"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start a fresh session server")
    };
    let deployed = |server: &mut ServerGuard| deployment_count(server, &root, &session);
    server.replace_child(start_fresh(&root));
    assert_eq!(
        deployed(&mut server),
        1,
        "a fresh start keeps the deployment"
    );

    let killed = rozi(&root)
        .args(["sessions", "kill", &session])
        .output()
        .expect("sessions kill");
    assert!(killed.status.success(), "{killed:?}");
    server.kill_for_restart();
    server.replace_child(start_fresh(&root));
    assert_eq!(deployed(&mut server), 0, "deleting the session removes it");
}

#[cfg(unix)]
fn wait_for_text(path: &Path, text: &str) -> String {
    let deadline = std::time::Instant::now() + common::io_timeout();
    loop {
        let contents = std::fs::read_to_string(path).unwrap_or_default();
        if contents.contains(text) {
            return contents;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{} did not contain {text:?}: {contents}",
            path.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(unix)]
#[test]
fn session_services_subscribe_without_clients_and_retire_their_credentials() {
    use rozi::control::ControlErrorCode;
    let root = private_temp_dir();
    std::fs::write(
        root.join("config.toml"),
        "[session]\nresurrect=true\n[extensions.demo]\ngreeting='host'\n",
    )
    .unwrap();
    let session = unique_session_name();
    let mut server = ServerGuard::new(spawn(&session, &root), root.clone());
    let bundle = Bundle::from_files(vec![
        BundleFile {
            path: "extension.toml".into(),
            executable: false,
            contents: br#"
[extension]
id = "demo"
title = "Demo"
version = "1"
api = 1

[[services]]
name = "watch"
exec = ["sh", "./watch.sh"]
placement = "session"
restart = "always"

[settings]
greeting = "default"
"#
            .to_vec(),
        },
        BundleFile {
            path: "watch.sh".into(),
            executable: false,
            contents: br#"printf '%s' "$ROZI_EXTENSION_CREDENTIAL" > "$ROZI_EXTENSION_STATE_DIR/credential"
printf '%s' "$ROZI_EXTENSION_GENERATION" > "$ROZI_EXTENSION_STATE_DIR/generation"
printf '%s' "$ROZI_EXTENSION_CONFIG" > "$ROZI_EXTENSION_STATE_DIR/settings"
"$ROZI_BIN" notify "service started"
exec "$ROZI_BIN" subscribe --snapshot pane-spawned session-notification > "$ROZI_EXTENSION_STATE_DIR/events"
"#
            .to_vec(),
        },
    ])
    .unwrap();
    let archive = bundle.archive();
    let deploy = || ControlCommand::ExtensionsDeploy {
        id: "demo".into(),
        digest: bundle.digest().into(),
        size: archive.len() as u64,
        offset: 0,
        data: base64::engine::general_purpose::STANDARD.encode(&archive),
        last: true,
    };
    ask(&mut server, &root, &session, deploy());
    let state = root.join("data/rozi/extension-state/demo");
    wait_for_text(&state.join("events"), "snapshot");
    wait_for_text(&state.join("events"), "service started");
    ask(
        &mut server,
        &root,
        &session,
        ControlCommand::Notify {
            message: "headless notification".into(),
            title: None,
            level: rozi::control::NotifyLevel::Info,
        },
    );
    wait_for_text(&state.join("events"), "headless notification");
    let credential = std::fs::read_to_string(state.join("credential")).unwrap();
    assert!(credential.starts_with(rozi::extension_runtime::SESSION_CREDENTIAL_PREFIX));
    assert!(
        std::fs::read_to_string(state.join("settings"))
            .unwrap()
            .contains("host")
    );
    let first_generation: u64 = std::fs::read_to_string(state.join("generation"))
        .unwrap()
        .parse()
        .unwrap();
    let request = |credential: String| ControlRequest {
        command: ControlCommand::ListPanes,
        credential: Some(credential),
        source_pane: None,
        source_session: None,
        caller_context: None,
        extension: Some(rozi::control::ExtensionProvenance {
            id: "caller-claim-is-ignored".into(),
            generation: "client-claim".into(),
        }),
    };
    assert!(ask_request(&mut server, &root, &session, request(credential.clone())).ok);
    assert_eq!(
        ask_request(
            &mut server,
            &root,
            &session,
            request("ui-issued-token".into())
        )
        .code,
        Some(ControlErrorCode::NotPermitted)
    );
    // With no UI ever attached, the server creates a pane and the deployed service receives its event.
    ask(
        &mut server,
        &root,
        &session,
        ControlCommand::NewPane {
            command: None,
            argv: Some(vec!["sh".into(), "-c".into(), "sleep 30".into()]),
            local_cwd: None,
            cwd: None,
            title: None,
            keep_open: false,
            focus: false,
            workspace: None,
            size: None,
        },
    );
    wait_for_text(&state.join("events"), "pane-spawned");
    // A redeploy that changes something retires the old credential. Same files alone would not.
    std::fs::write(
        root.join("config.toml"),
        "[session]\nresurrect=true\n[extensions.demo]\ngreeting='changed'\n",
    )
    .unwrap();
    ask(&mut server, &root, &session, deploy());
    assert_eq!(
        ask_request(&mut server, &root, &session, request(credential)).code,
        Some(ControlErrorCode::ExtensionInactive)
    );
    let new_generation = first_generation + 1;
    wait_for_text(&state.join("generation"), &new_generation.to_string());
    let credential = std::fs::read_to_string(state.join("credential")).unwrap();
    server.kill_for_restart();
    server.replace_child(spawn(&session, &root));
    wait_for_text(&state.join("generation"), &(new_generation + 1).to_string());
    assert_eq!(
        ask_request(&mut server, &root, &session, request(credential)).code,
        Some(ControlErrorCode::ExtensionInactive)
    );
    ask(
        &mut server,
        &root,
        &session,
        ControlCommand::ExtensionsUndeploy { id: "demo".into() },
    );
}

/// A service that records what it was started with, for the lifecycle cases below.
#[cfg(unix)]
fn recording_bundle(marker: &str) -> Bundle {
    Bundle::from_files(vec![
        BundleFile {
            path: "extension.toml".into(),
            executable: false,
            contents: br#"
[extension]
id = "demo"
title = "Demo"
version = "1"
api = 1

[[services]]
name = "watch"
exec = ["sh", "./watch.sh"]
placement = "session"
restart = "always"
"#
            .to_vec(),
        },
        BundleFile {
            path: "watch.sh".into(),
            executable: false,
            contents: format!(
                r#"# {marker}
printf '%s' "$ROZI_EXTENSION_CREDENTIAL" > "$ROZI_EXTENSION_STATE_DIR/credential"
printf '%s' "$ROZI_SESSION_NAME" > "$ROZI_EXTENSION_STATE_DIR/session"
printf '%s' "$ROZI_EXTENSION_DIR" > "$ROZI_EXTENSION_STATE_DIR/dir"
printf '%s' "$$" > "$ROZI_EXTENSION_STATE_DIR/pid.tmp"
mv "$ROZI_EXTENSION_STATE_DIR/pid.tmp" "$ROZI_EXTENSION_STATE_DIR/pid"
exec sleep 300
"#
            )
            .into_bytes(),
        },
    ])
    .unwrap()
}

fn deploy_command(bundle: &Bundle) -> ControlCommand {
    let archive = bundle.archive();
    ControlCommand::ExtensionsDeploy {
        id: "demo".into(),
        digest: bundle.digest().into(),
        size: archive.len() as u64,
        offset: 0,
        data: base64::engine::general_purpose::STANDARD.encode(&archive),
        last: true,
    }
}

#[cfg(unix)]
fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + common::io_timeout();
    while !done() {
        assert!(std::time::Instant::now() < deadline, "timed out: {what}");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn service_pid(state: &Path) -> i32 {
    wait_until("the service to start", || state.join("pid").exists());
    std::fs::read_to_string(state.join("pid"))
        .unwrap()
        .parse()
        .unwrap()
}

#[cfg(target_os = "linux")]
fn process_is_running(pid: i32) -> bool {
    // A killed service stays a zombie until its supervisor reaps it, which also counts as gone.
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .is_none_or(|stat| !stat.contains(") Z "))
        && unsafe { libc::kill(pid, 0) } == 0
}

/// A bundle with one service, whose manifest and script the caller finishes.
#[cfg(target_os = "linux")]
fn service_bundle(restart: &str, settings: &str, script: &str) -> Bundle {
    Bundle::from_files(vec![
        BundleFile {
            path: "extension.toml".into(),
            executable: false,
            contents: format!(
                "[extension]\nid = 'demo'\ntitle = 'Demo'\nversion = '1'\napi = 1\n{settings}\n\
                 [[services]]\nname = 'watch'\nexec = ['sh', './watch.sh']\n\
                 placement = 'session'\nrestart = '{restart}'\n"
            )
            .into_bytes(),
        },
        BundleFile {
            path: "watch.sh".into(),
            executable: false,
            contents: script.as_bytes().to_vec(),
        },
    ])
    .unwrap()
}

#[cfg(target_os = "linux")]
const RECORDS_ITS_PID: &str = r#"printf '%s' "$$" > "$ROZI_EXTENSION_STATE_DIR/pid.tmp"
mv "$ROZI_EXTENSION_STATE_DIR/pid.tmp" "$ROZI_EXTENSION_STATE_DIR/pid"
exec sleep 300
"#;

#[cfg(target_os = "linux")]
fn service_state(server: &mut ServerGuard, root: &Path, session: &str) -> String {
    let reply = ask(server, root, session, ControlCommand::ExtensionsStatus);
    reply.data.unwrap()["deployments"][0]["services"][0]["state"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[cfg(target_os = "linux")]
fn deployed_generation(reply: &ControlResponse) -> (u64, bool) {
    let data = reply.data.as_ref().unwrap();
    (
        data["generation"].as_u64().unwrap(),
        data["unchanged"].as_bool().unwrap_or(false),
    )
}

/// Deploying the same thing twice, as a script that runs on every change does, must not disturb
/// what is running.
#[cfg(target_os = "linux")]
#[test]
fn redeploying_what_is_already_running_changes_nothing() {
    let root = private_temp_dir();
    std::fs::write(root.join("config.toml"), "[session]\nresurrect = true\n").unwrap();
    let session = unique_session_name();
    let mut server = ServerGuard::new(spawn(&session, &root), root.clone());
    let bundle = service_bundle("always", "", RECORDS_ITS_PID);
    let first = ask(&mut server, &root, &session, deploy_command(&bundle));
    assert_eq!(deployed_generation(&first), (1, false));
    let state = root.join("data/rozi/extension-state/demo");
    let pid = service_pid(&state);
    wait_until("the service to be running", || {
        service_state(&mut server, &root, &session) == "running"
    });
    std::fs::remove_file(state.join("pid")).unwrap();

    let again = ask(&mut server, &root, &session, deploy_command(&bundle));
    assert_eq!(deployed_generation(&again), (1, true));
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(process_is_running(pid), "the service was stopped");
    assert!(!state.join("pid").exists(), "the service was started again");
    assert_eq!(service_state(&mut server, &root, &session), "running");
}

/// Redeploying is how a host's new settings reach a session, so it still has to restart for them.
#[cfg(target_os = "linux")]
#[test]
fn redeploying_with_changed_settings_restarts_the_service() {
    let root = private_temp_dir();
    std::fs::write(root.join("config.toml"), "").unwrap();
    let session = unique_session_name();
    let mut server = ServerGuard::new(spawn(&session, &root), root.clone());
    let bundle = service_bundle("always", "[settings]\nmode = 'a'", RECORDS_ITS_PID);
    ask(&mut server, &root, &session, deploy_command(&bundle));
    let state = root.join("data/rozi/extension-state/demo");
    service_pid(&state);
    wait_until("the service to be running", || {
        service_state(&mut server, &root, &session) == "running"
    });
    std::fs::remove_file(state.join("pid")).unwrap();

    std::fs::write(root.join("config.toml"), "[extensions.demo]\nmode = 'b'\n").unwrap();
    let again = ask(&mut server, &root, &session, deploy_command(&bundle));
    assert_eq!(deployed_generation(&again), (2, false));
    service_pid(&state);
}

/// A service that has stopped gets another chance from a redeploy, which is how someone starts it.
#[cfg(target_os = "linux")]
#[test]
fn redeploying_restarts_a_service_that_has_stopped() {
    let root = private_temp_dir();
    std::fs::write(root.join("config.toml"), "").unwrap();
    let session = unique_session_name();
    let mut server = ServerGuard::new(spawn(&session, &root), root.clone());
    let bundle = service_bundle("never", "", "exit 0\n");
    ask(&mut server, &root, &session, deploy_command(&bundle));
    wait_until("the service to stop", || {
        service_state(&mut server, &root, &session) == "stopped"
    });
    let again = ask(&mut server, &root, &session, deploy_command(&bundle));
    assert_eq!(deployed_generation(&again), (2, false));
}

/// Different files are a different deployment, whatever is running.
#[cfg(target_os = "linux")]
#[test]
fn redeploying_different_files_restarts_the_service() {
    let root = private_temp_dir();
    std::fs::write(root.join("config.toml"), "").unwrap();
    let session = unique_session_name();
    let mut server = ServerGuard::new(spawn(&session, &root), root.clone());
    let first = service_bundle("always", "", RECORDS_ITS_PID);
    ask(&mut server, &root, &session, deploy_command(&first));
    wait_until("the service to be running", || {
        service_state(&mut server, &root, &session) == "running"
    });
    let changed = service_bundle("always", "", &format!("# changed\n{RECORDS_ITS_PID}"));
    let again = ask(&mut server, &root, &session, deploy_command(&changed));
    assert_eq!(deployed_generation(&again), (2, false));
}

#[cfg(target_os = "linux")]
#[test]
fn renaming_a_session_restarts_its_services_from_the_moved_store() {
    let root = private_temp_dir();
    std::fs::write(root.join("config.toml"), "[session]\nresurrect = true\n").unwrap();
    let session = unique_session_name();
    let renamed = unique_session_name();
    let mut server = ServerGuard::new(spawn(&session, &root), root.clone());
    let bundle = recording_bundle("rename");
    ask(&mut server, &root, &session, deploy_command(&bundle));
    let state = root.join("data/rozi/extension-state/demo");
    let first_pid = service_pid(&state);
    let first_credential = std::fs::read_to_string(state.join("credential")).unwrap();
    std::fs::remove_file(state.join("pid")).unwrap();

    let mut client = connect_when_ready(&subprocess_endpoint(&root, &session), server.child_mut());
    client.write_control(&common::attach_message(&session, "renamer"));
    read_until(&mut client, |frame| {
        matches!(frame, Frame::Control(ServerMessage::Attached { .. }))
    });
    client.write_control(&ClientMessage::Rename {
        name: renamed.clone(),
    });
    read_until(&mut client, |frame| {
        matches!(frame, Frame::Control(ServerMessage::Renamed { .. }))
    });

    // Back up, under the new name, running from where the store is now.
    let second_pid = service_pid(&state);
    assert_ne!(first_pid, second_pid);
    assert!(!process_is_running(first_pid));
    assert_eq!(
        std::fs::read_to_string(state.join("session")).unwrap(),
        renamed
    );
    let dir = std::fs::read_to_string(state.join("dir")).unwrap();
    assert!(Path::new(&dir).join("watch.sh").is_file(), "{dir}");
    assert!(dir.contains(&renamed), "{dir}");

    // The old credential went with the old process.
    let refused = ask_request(
        &mut server,
        &root,
        &renamed,
        ControlRequest {
            command: ControlCommand::ListPanes,
            credential: Some(first_credential),
            source_pane: None,
            source_session: None,
            caller_context: None,
            extension: None,
        },
    );
    assert_eq!(
        refused.code,
        Some(rozi::control::ControlErrorCode::ExtensionInactive)
    );

    // It can still be restarted: its launch does not name the store's old path.
    std::fs::remove_file(state.join("pid")).unwrap();
    unsafe { libc::kill(second_pid, libc::SIGKILL) };
    let third_pid = service_pid(&state);
    assert_ne!(second_pid, third_pid);
    drop(client);
    let report = ask(
        &mut server,
        &root,
        &renamed,
        ControlCommand::ExtensionsStatus,
    );
    let report: ExtensionDeploymentReport = serde_json::from_value(report.data.unwrap()).unwrap();
    assert_eq!(report.session, renamed);
    assert_eq!(report.deployments.len(), 1);
}

#[cfg(target_os = "linux")]
#[test]
fn undeploying_stops_services_even_when_the_hosts_config_cannot_be_read() {
    let root = private_temp_dir();
    let good = "[session]\nresurrect = true\n";
    std::fs::write(root.join("config.toml"), good).unwrap();
    let session = unique_session_name();
    let mut server = ServerGuard::new(spawn(&session, &root), root.clone());
    let first = recording_bundle("first");
    ask(&mut server, &root, &session, deploy_command(&first));
    let state = root.join("data/rozi/extension-state/demo");
    let pid = service_pid(&state);
    std::fs::remove_file(state.join("pid")).unwrap();

    std::fs::write(root.join("config.toml"), "not = = toml").unwrap();

    // A new generation replaces the old process, and does not start without its settings.
    let second = recording_bundle("second");
    ask(&mut server, &root, &session, deploy_command(&second));
    wait_until("the old generation to stop", || !process_is_running(pid));
    assert!(!state.join("pid").exists());
    let report = ask(
        &mut server,
        &root,
        &session,
        ControlCommand::ExtensionsStatus,
    );
    let report: ExtensionDeploymentReport = serde_json::from_value(report.data.unwrap()).unwrap();
    let service = &report.deployments[0].services[0];
    assert_eq!(service.state, rozi::control::SessionServiceState::Stopped);
    assert!(
        service
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("config"),
        "{service:?}"
    );

    // Back to a readable config, and a service that really is running when undeploy arrives.
    std::fs::write(root.join("config.toml"), good).unwrap();
    let third = recording_bundle("third");
    ask(&mut server, &root, &session, deploy_command(&third));
    let pid = service_pid(&state);
    std::fs::write(root.join("config.toml"), "not = = toml").unwrap();
    ask(
        &mut server,
        &root,
        &session,
        ControlCommand::ExtensionsUndeploy { id: "demo".into() },
    );
    wait_until("the undeployed service to stop", || {
        !process_is_running(pid)
    });
}

#[cfg(unix)]
#[test]
fn a_session_credential_is_gated_and_retiring_it_closes_what_it_holds() {
    use rozi::control::ControlErrorCode;

    let root = private_temp_dir();
    std::fs::write(root.join("config.toml"), "[session]\nresurrect = true\n").unwrap();
    let session = unique_session_name();
    let mut server = ServerGuard::new(spawn(&session, &root), root.clone());
    let bundle = recording_bundle("gated");
    ask(&mut server, &root, &session, deploy_command(&bundle));
    let state = root.join("data/rozi/extension-state/demo");
    service_pid(&state);
    let credential = std::fs::read_to_string(state.join("credential")).unwrap();
    let request = |command| ControlRequest {
        command,
        credential: Some(credential.clone()),
        source_pane: None,
        source_session: None,
        caller_context: None,
        extension: None,
    };

    // A session process cannot change what the session runs.
    for command in [
        deploy_command(&bundle),
        ControlCommand::ExtensionsUndeploy { id: "demo".into() },
    ] {
        assert_eq!(
            ask_request(&mut server, &root, &session, request(command)).code,
            Some(ControlErrorCode::NotPermitted)
        );
    }

    // A subscription held under the credential ends when the credential does.
    let mut held = connect_when_ready(&subprocess_endpoint(&root, &session), server.child_mut());
    held.write_control(&ClientMessage::SessionControl {
        capabilities: None,
        session: session.clone(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: MIN_SUPPORTED_PROTOCOL,
        request: request(ControlCommand::Subscribe {
            events: Vec::new(),
            snapshot: false,
            after: None,
        }),
    });
    read_until(&mut held, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::SessionControlResult { .. })
        )
    });
    ask(
        &mut server,
        &root,
        &session,
        ControlCommand::ExtensionsUndeploy { id: "demo".into() },
    );
    held.wait_closed();
}

/// Where a client started by [`rozi`] looks for installed extensions.
fn extensions_dir(root: &Path) -> std::path::PathBuf {
    let data = if cfg!(windows) { "state" } else { "data" };
    root.join(data).join("rozi").join("extensions")
}

fn install_demo(root: &Path, directory: &str, payload: &str) {
    let installation = extensions_dir(root).join(directory);
    std::fs::create_dir_all(&installation).unwrap();
    std::fs::write(
        installation.join("extension.toml"),
        "[extension]\nid = 'demo'\ntitle = 'Demo'\nversion = '1'\napi = 1\n",
    )
    .unwrap();
    std::fs::write(installation.join("payload"), payload).unwrap();
}

fn deploy_with_cli(root: &Path, session: &str) -> std::process::Output {
    rozi(root)
        .args(["extensions", "deploy", "demo", "--session", session])
        .stdin(Stdio::null())
        .output()
        .expect("run extensions deploy")
}

fn local_copy_from_cli(root: &Path, session: &str) -> rozi::control::ExtensionLocalCopy {
    let output = rozi(root)
        .args(["extensions", "status", "--session", session, "--json"])
        .stdin(Stdio::null())
        .output()
        .expect("run extensions status");
    assert!(output.status.success(), "{output:?}");
    let response: ControlResponse = serde_json::from_slice(&output.stdout).unwrap();
    let report: ExtensionDeploymentReport = serde_json::from_value(response.data.unwrap()).unwrap();
    report.deployments[0].local_copy.expect("compared locally")
}

/// Two installations declaring one ID contribute nothing to a client, because neither is the
/// extension the ID names. That has to hold where the code leaves this machine too.
#[test]
fn an_ambiguous_extension_id_is_neither_deployed_nor_compared() {
    use rozi::control::ExtensionLocalCopy;

    let root = private_temp_dir();
    let session = unique_session_name();
    let mut server = ServerGuard::new(spawn(&session, &root), root.clone());
    connect_when_ready(&subprocess_endpoint(&root, &session), server.child_mut());
    install_demo(&root, "one", "first");
    install_demo(&root, "two", "second");

    let refused = deploy_with_cli(&root, &session);
    assert!(!refused.status.success(), "{refused:?}");
    let reason = String::from_utf8_lossy(&refused.stderr);
    assert!(reason.contains("duplicate extension id"), "{reason}");
    let report = ask(
        &mut server,
        &root,
        &session,
        ControlCommand::ExtensionsStatus,
    );
    let report: ExtensionDeploymentReport = serde_json::from_value(report.data.unwrap()).unwrap();
    assert!(report.deployments.is_empty(), "{report:?}");

    // One installation is unambiguous again. `one` also sorts first, so a comparison that picked
    // whichever duplicate it reached first would call the second duplicate below a match.
    std::fs::remove_dir_all(extensions_dir(&root).join("two")).unwrap();
    let deployed = deploy_with_cli(&root, &session);
    assert!(deployed.status.success(), "{deployed:?}");
    assert_eq!(
        local_copy_from_cli(&root, &session),
        ExtensionLocalCopy::Matches
    );

    install_demo(&root, "two", "second");
    assert_eq!(
        local_copy_from_cli(&root, &session),
        ExtensionLocalCopy::Unavailable
    );
}
