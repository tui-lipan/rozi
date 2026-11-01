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
    let mut client = connect_when_ready(&subprocess_endpoint(root, session), server.child_mut());
    client.write_control(&ClientMessage::SessionControl {
        capabilities: None,
        session: session.into(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: MIN_SUPPORTED_PROTOCOL,
        request: ControlRequest {
            command,
            source_pane: None,
            source_session: None,
            caller_context: None,
            extension: None,
            credential: None,
        },
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
    let response = response.expect("server replied");
    assert!(response.ok, "{response:?}");
    response
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
