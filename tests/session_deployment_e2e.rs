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
