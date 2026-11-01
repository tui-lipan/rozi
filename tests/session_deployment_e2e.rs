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

fn spawn(session: &str, root: &Path) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_rozi"))
        .args(["--server", session])
        .env("ROZI_CONFIG", root.join("config.toml"))
        .env("XDG_RUNTIME_DIR", root)
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("APPDATA", root.join("config"))
        .env("LOCALAPPDATA", root.join("state"))
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
        BundleFile { path: "extension.toml".into(), executable: false, contents: b"[extension]\nid='demo'\ntitle='Demo'\nversion='1'\napi=1\n[[services]]\nname='watch'\nexec=['sh','./watch.sh']\nplacement='session'\nrestart='always'\n[settings]\ngreeting='default'\n".to_vec() },
        BundleFile { path: "watch.sh".into(), executable: false, contents: br#"printf '%s' "$ROZI_EXTENSION_CREDENTIAL" > "$ROZI_EXTENSION_STATE_DIR/credential"
printf '%s' "$ROZI_EXTENSION_GENERATION" > "$ROZI_EXTENSION_STATE_DIR/generation"
printf '%s' "$ROZI_EXTENSION_CONFIG" > "$ROZI_EXTENSION_STATE_DIR/settings"
"$ROZI_BIN" notify "service started"
exec "$ROZI_BIN" subscribe --snapshot pane-spawned session-notification > "$ROZI_EXTENSION_STATE_DIR/events"
"#.to_vec() },
    ]).unwrap();
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
        extension: Some(rozi::config::ExtensionProvenance {
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
            contents: b"[extension]\nid='demo'\ntitle='Demo'\nversion='1'\napi=1\n[[services]]\nname='watch'\nexec=['sh','./watch.sh']\nplacement='session'\nrestart='always'\n".to_vec(),
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

#[cfg(unix)]
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
    assert_eq!(service.state, "stopped");
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
