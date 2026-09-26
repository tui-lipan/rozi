//! A recording started without a path, the way Start pane recording starts one: the session
//! server names the file itself and writes it into its own recordings directory.
//!
//! A binary of its own because that directory is in the state directory. The process's user
//! directories are isolated before the first server starts, which a suite whose other tests already
//! bound their sockets could not do.
#![cfg(unix)]

mod common;

use rozi::control::{
    ControlCommand, ControlRequest, ControlResponse, RecordingInfo, RecordingStopList,
};
use rozi::platform::command::{ShellEnv, resolve_launch_argv};
use rozi::platform::paths::{PlatformEnv, recording_dir};
use rozi::recording::EndReason;
use rozi::session::headless::run_session_control;
use rozi::session::protocol::{ClientMessage, Frame, ServerMessage};
use rozi::session::server::ServerSettings;

use common::{TestConnection, attach_client, read_until, spawn_listener};

fn request(command: ControlCommand) -> ControlRequest {
    ControlRequest {
        command,
        source_pane: None,
        source_session: None,
        extension: None,
    }
}

fn tunnel(
    client: &mut TestConnection,
    request_id: u64,
    command: ControlCommand,
) -> ControlResponse {
    client.write_control(&ClientMessage::AttachedControl {
        request_id,
        request: request(command),
    });
    let mut answer = None;
    read_until(client, |frame| match frame {
        Frame::Control(ServerMessage::AttachedControlResult {
            request_id: id,
            response,
        }) if *id == request_id => {
            answer = Some(response.clone());
            true
        }
        _ => false,
    });
    let answer = answer.expect("read_until matched without an answer");
    assert!(answer.ok, "{:?}", answer.error);
    answer
}

#[test]
fn a_recording_without_a_path_is_named_in_the_state_directory_and_stops_by_its_pane() {
    rozi::test_support::isolate_user_dirs();
    let (shell, command_shell) = resolve_launch_argv(None, None, &ShellEnv::from_process());
    let server = spawn_listener(ServerSettings {
        shell,
        command_shell,
        ..ServerSettings::default()
    });
    let session = server.session().to_string();
    let spawned = run_session_control(
        &session,
        request(ControlCommand::NewPane {
            command: None,
            argv: Some(vec![
                "sh".into(),
                "-c".into(),
                "echo ready; sleep 60".into(),
            ]),
            cwd: None,
            title: None,
            keep_open: false,
            focus: false,
            workspace: None,
        }),
    )
    .expect("the session answered");
    let pane = spawned.data.unwrap()["id"].as_u64().expect("a pane id") as u32;
    let (mut client, _) = attach_client(server.endpoint(), &session, "ui");

    let started = tunnel(
        &mut client,
        1,
        ControlCommand::RecordStart {
            target: Some(pane),
            output: None,
            max_fps: None,
            duration_ms: None,
            max_bytes: None,
            force: false,
            follow: false,
        },
    );
    let info: RecordingInfo = serde_json::from_value(started.data.unwrap()).unwrap();
    let path = std::path::PathBuf::from(&info.path);
    let expected = recording_dir(&PlatformEnv::from_process(), None).unwrap();
    assert_eq!(path.parent(), Some(expected.as_path()));
    let name = path.file_name().unwrap().to_str().unwrap();
    assert!(
        name.starts_with(&format!("{session}-pane-{pane}-")) && name.ends_with(".rozirec"),
        "{name}"
    );
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "a recording is private");
    }

    let stopped = tunnel(
        &mut client,
        2,
        ControlCommand::RecordStop {
            id: None,
            target: Some(pane),
        },
    );
    let list: RecordingStopList = serde_json::from_value(stopped.data.unwrap()).unwrap();
    let [stopped] = list.stopped.try_into().unwrap();
    assert_eq!(
        (stopped.path, stopped.reason),
        (info.path, EndReason::Stopped)
    );
    assert!(path.is_file());
    client.write_control(&ClientMessage::Detach);
}
