//! Verify the real CLI against detached session servers, with an unusable inherited UI socket.
mod common;

use std::process::{Command, Output};

use rozi::control::{ControlCommand, ControlRequest};
use rozi::platform::paths::PlatformEnv;
use rozi::session::headless::run_session_control;
use rozi::session::server::ServerSettings;

use crate::common::spawn_listener;

fn request(command: ControlCommand) -> ControlRequest {
    ControlRequest {
        command,
        source_pane: None,
        source_session: None,
        extension: None,
        caller_context: None,
        credential: None,
    }
}

fn cli() -> Command {
    let env = PlatformEnv::from_process();
    let mut cli = Command::new(env!("CARGO_BIN_EXE_rozi"));
    for (key, value) in [
        ("HOME", env.home),
        ("XDG_CONFIG_HOME", env.xdg_config_home),
        ("XDG_STATE_HOME", env.xdg_state_home),
        ("XDG_CACHE_HOME", env.xdg_cache_home),
        ("XDG_DATA_HOME", env.xdg_data_home),
        ("XDG_RUNTIME_DIR", env.xdg_runtime_dir),
        ("APPDATA", env.appdata),
        ("LOCALAPPDATA", env.local_appdata),
    ] {
        cli.env(key, value.unwrap());
    }
    for key in [
        "ROZI",
        "ROZI_PANE",
        "ROZI_SESSION_INSTANCE",
        "ROZI_CONFIG",
        "ROZI_EXTENSION",
        "ROZI_EXTENSION_CREDENTIAL",
        "ROZI_SOCKET",
    ] {
        cli.env_remove(key);
    }
    cli
}

fn data(output: Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], true, "{value}");
    value["data"].clone()
}

#[test]
fn caller_identity_and_here_work_without_a_ui_and_reject_stale_hints() {
    rozi::test_support::isolate_user_dirs();
    let outside = data(
        cli()
            .args(["context", "--format", "json"])
            .output()
            .unwrap(),
    );
    assert_eq!(outside["inside_rozi"], false);
    assert_eq!(outside["status"], "outside");
    let server = spawn_listener(ServerSettings::default());
    let spawned = data(
        cli()
            .args(["--session", server.session(), "split"])
            .output()
            .unwrap(),
    );
    let panes = run_session_control(server.session(), request(ControlCommand::ListPanes))
        .unwrap()
        .data
        .unwrap();
    let pane = &panes[0];
    let id = spawned["id"].to_string();
    let instance = pane["reference"]["session_instance"].as_str().unwrap();
    let caller = || {
        let mut command = cli();
        command
            .env("ROZI", "1")
            .env("ROZI_PANE", &id)
            .env("ROZI_SESSION_INSTANCE", instance)
            .env("ROZI_SOCKET", "nonexistent-caller-ui.sock");
        command
    };
    let context = data(
        caller()
            .args(["context", "--format", "json"])
            .output()
            .unwrap(),
    );
    assert_eq!(context["inside_rozi"], true);
    assert_eq!(context["status"], "verified");
    assert_eq!(context["pane"], pane["reference"]);
    assert_eq!(context["session"]["name"], server.session());
    assert_eq!(context["ui"]["available"], false);
    let listed = data(caller().args(["--here", "list-panes"]).output().unwrap());
    assert_eq!(listed[0]["reference"], context["pane"]);
    let absent_socket = data(
        caller()
            .env_remove("ROZI_SOCKET")
            .args(["context"])
            .output()
            .unwrap(),
    );
    assert_eq!(absent_socket["status"], "verified");
    data(caller().args(["--here", "split"]).output().unwrap());
    let own_capture = data(caller().args(["--here", "capture-pane"]).output().unwrap());
    assert_eq!(
        own_capture["id"], pane["id"],
        "an omitted pane target means the caller in a many-pane session"
    );
    let recordings = tempfile::tempdir().unwrap();
    let recording = data(
        caller()
            .current_dir(recordings.path())
            .args(["--here", "record", "start", "--output", "caller.rozirec"])
            .output()
            .unwrap(),
    );
    assert_eq!(recording["pane"], pane["id"]);
    assert!(recordings.path().join("caller.rozirec").exists());
    data(
        caller()
            .args(["--here", "record", "stop"])
            .output()
            .unwrap(),
    );
    // Another server has the same numeric pane id. Caller routing must stay in this instance.
    let other = spawn_listener(ServerSettings::default());
    data(
        cli()
            .args(["--session", other.session(), "split"])
            .output()
            .unwrap(),
    );
    let listed = data(caller().args(["--here", "list-panes"]).output().unwrap());
    assert_eq!(listed[0]["session"], server.session());
    let stale = data(
        caller()
            .env("ROZI_PANE", u32::MAX.to_string())
            .args(["context"])
            .output()
            .unwrap(),
    );
    assert_eq!(stale["status"], "stale-pane");
    assert_eq!(stale["pane"], serde_json::Value::Null);
    let refused = caller()
        .env("ROZI_PANE", u32::MAX.to_string())
        .args(["--here", "list-panes"])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    let stale = data(
        caller()
            .env("ROZI_SESSION_INSTANCE", "retired-instance")
            .args(["context"])
            .output()
            .unwrap(),
    );
    assert_eq!(stale["status"], "stale-session");
    let incomplete = data(
        cli()
            .env("ROZI_PANE", &id)
            .args(["context"])
            .output()
            .unwrap(),
    );
    assert_eq!(incomplete["status"], "incomplete");
}

#[cfg(unix)]
#[test]
fn here_prompt_returns_the_exact_agent_outcome_and_terminal_from_the_server() {
    use rozi::control::{AgentWaitCondition, CaptureRender};
    use rozi::session::protocol::AgentState;
    use std::time::{Duration, Instant};
    rozi::test_support::isolate_user_dirs();
    use rozi::agent_detection::{
        AgentCatalog, AgentMatchSpec, AgentOrigin, AgentSpec, build_definitions,
    };
    let mut warnings = Vec::new();
    let definitions = build_definitions(
        vec![AgentSpec {
            id: Some("turn-test".into()),
            base: Some(false),
            r#match: AgentMatchSpec {
                names: vec!["sh".into(), "dash".into(), "bash".into()],
                paths: Vec::new(),
            },
            ..AgentSpec::default()
        }],
        AgentOrigin::Config,
        &[],
        &mut warnings,
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    let server = spawn_listener(ServerSettings {
        agents: std::sync::Arc::new(AgentCatalog::with_definitions(definitions)),
        ..ServerSettings::default()
    });
    let spawned = data(
        cli()
            .args([
                "--session",
                server.session(),
                "split",
                "--argv",
                "sh",
                "-c",
                "while IFS= read -r line; do printf 'REPLY:%s\\n' \"$line\"; done",
            ])
            .output()
            .unwrap(),
    );
    let id = spawned["id"].as_u64().unwrap() as u32;
    let deadline = Instant::now() + crate::common::io_timeout();
    let detected = loop {
        let agents = run_session_control(server.session(), request(ControlCommand::AgentsList))
            .unwrap()
            .data
            .unwrap();
        if let Some(agent) = agents.as_array().unwrap().first() {
            break agent["ref"].clone();
        }
        assert!(Instant::now() < deadline, "test child was not detected");
        std::thread::sleep(Duration::from_millis(10));
    };
    let instance = detected["pane"]["session_instance"].as_str().unwrap();
    let report = |state: AgentState, reason: Option<&str>, seq: u64| {
        let mut command = cli();
        command
            .env("ROZI_PANE", id.to_string())
            .env("ROZI_SESSION_INSTANCE", instance)
            .args([
                "--here",
                "agents",
                "report",
                "--agent",
                "turn-test",
                "--integration",
                "transaction-test",
                "--state",
                state.as_str(),
                "--seq",
                &seq.to_string(),
            ]);
        if let Some(reason) = reason {
            command.args(["--reason", reason]);
        }
        data(command.output().unwrap());
    };
    report(AgentState::Idle, None, 1);
    let listed = run_session_control(server.session(), request(ControlCommand::AgentsList))
        .unwrap()
        .data
        .unwrap();
    let reference = &listed[0]["ref"];
    let mut prompt = cli();
    prompt
        .env("ROZI_PANE", id.to_string())
        .env(
            "ROZI_SESSION_INSTANCE",
            reference["pane"]["session_instance"].as_str().unwrap(),
        )
        .args([
            "--here",
            "agents",
            "prompt",
            "--ref",
            &reference.to_string(),
            "--wait",
            "turn",
            "--timeout",
            "30s",
            "--capture",
            "text",
            "hello-transaction",
        ]);
    let answer = std::thread::spawn(move || prompt.output().unwrap());
    let deadline = Instant::now() + crate::common::io_timeout();
    loop {
        let capture = run_session_control(
            server.session(),
            request(ControlCommand::CapturePane {
                target: Some(id),
                scrollback: None,
                render: CaptureRender::Text,
                scale: None,
                wait: None,
                image_pixels: false,
            }),
        )
        .unwrap()
        .data
        .unwrap();
        if capture["text"]
            .as_str()
            .unwrap()
            .contains("REPLY:hello-transaction")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "prompt did not reach the child: {capture}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    report(AgentState::Blocked, Some("Plan approval required"), 2);
    let result = data(answer.join().unwrap());
    assert_eq!(result["ref"], *reference);
    assert_eq!(
        result["condition"],
        serde_json::to_value(AgentWaitCondition::Turn).unwrap()
    );
    assert_eq!(result["agent"]["state"], "blocked");
    assert_eq!(result["agent"]["reason"], "Plan approval required");
    assert_eq!(result["result"]["kind"], "terminal");
    assert!(
        result["result"]["capture"]["text"]
            .as_str()
            .unwrap()
            .contains("REPLY:hello-transaction")
    );
    data(
        cli()
            .env("ROZI_PANE", id.to_string())
            .env("ROZI_SESSION_INSTANCE", instance)
            .args([
                "--here",
                "agents",
                "release",
                "--integration",
                "transaction-test",
                "--seq",
                "3",
            ])
            .output()
            .unwrap(),
    );
}

/// A UI transport stub verifies CLI routing; component tests exercise the real UI handler and
/// server tests cover attachment authority. Keep all IPC behind the platform transport helpers.
fn caller_ui_stub(
    caller: rozi::session::protocol::PaneRef,
    displayed: bool,
    refuse: bool,
) -> (
    rozi::platform::ipc::IpcEndpoint,
    std::thread::JoinHandle<()>,
) {
    use rozi::platform::ipc::EndpointRegistry;
    use std::io::{BufRead, BufReader, Write};
    let endpoint = EndpointRegistry::extension_bridge_endpoint(
        &rozi::control::runtime_dir().unwrap(),
        &common::unique_session_name(),
    );
    let listener = endpoint.bind().unwrap().into_listener();
    let thread = std::thread::spawn(move || {
        for probe in [true, false]
            .into_iter()
            .take(if displayed { 2 } else { 1 })
        {
            let mut stream = listener.accept().unwrap();
            stream.set_read_timeout(Some(common::io_timeout())).unwrap();
            stream
                .set_write_timeout(Some(common::io_timeout()))
                .unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let request: ControlRequest = serde_json::from_str(&line).unwrap();
            let response = if probe {
                assert_eq!(request.command, ControlCommand::Context);
                serde_json::json!({"ok": true, "data": {
                    "displayed_session": if displayed { caller.session_instance.as_str() } else { "another-session" },
                    "held_sessions": [caller.session_instance]
                }})
            } else {
                assert!(matches!(request.command, ControlCommand::NewPane { .. }));
                assert_eq!(request.caller_context.as_deref(), Some(&caller));
                assert_eq!(request.source_pane, Some(caller.pane_id));
                if refuse {
                    serde_json::json!({"ok": false, "code": "out-of-scope", "error": "UI switched sessions"})
                } else {
                    serde_json::json!({"ok": true, "data": {"id": 999, "accepted": true, "pty_ready": true}})
                }
            };
            writeln!(stream, "{response}").unwrap();
        }
    });
    (endpoint, thread)
}

#[test]
fn here_prefers_its_displayed_ui_and_falls_back_only_before_a_mutation() {
    rozi::test_support::isolate_user_dirs();
    for (displayed, refuse) in [(true, false), (true, true), (false, false)] {
        let server = spawn_listener(ServerSettings::default());
        data(
            cli()
                .args(["--session", server.session(), "split"])
                .output()
                .unwrap(),
        );
        let panes = run_session_control(server.session(), request(ControlCommand::ListPanes))
            .unwrap()
            .data
            .unwrap();
        let caller: rozi::session::protocol::PaneRef =
            serde_json::from_value(panes[0]["reference"].clone()).unwrap();
        // When displayed, the UI holds the server's controller lease. Direct splits must fail.
        let _controller = if displayed {
            let controller =
                common::attach_client(server.endpoint(), server.session(), "caller-ui");
            let direct = cli()
                .args(["--session", server.session(), "split"])
                .output()
                .unwrap();
            let reply: serde_json::Value = serde_json::from_slice(&direct.stdout).unwrap();
            assert_eq!(reply["code"], "not-controller");
            Some(controller)
        } else {
            None
        };
        let (endpoint, ui) = caller_ui_stub(caller.clone(), displayed, refuse);
        let output = cli()
            .env("ROZI_SOCKET", endpoint.path())
            .env("ROZI_PANE", caller.pane_id.to_string())
            .env("ROZI_SESSION_INSTANCE", caller.session_instance.as_str())
            .args([
                "--here",
                "split",
                "--keep-open",
                "--argv",
                env!("CARGO_BIN_EXE_rozi"),
                "--version",
            ])
            .output()
            .unwrap();
        ui.join().unwrap();
        endpoint.remove_stale();
        if refuse {
            assert!(!output.status.success());
            let reply: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                reply["code"], "out-of-scope",
                "a submitted mutation must not retry headlessly"
            );
        } else {
            let accepted = data(output);
            assert_eq!(accepted["id"] == 999, displayed);
        }
        let panes = run_session_control(server.session(), request(ControlCommand::ListPanes))
            .unwrap()
            .data
            .unwrap();
        assert_eq!(
            panes.as_array().unwrap().len(),
            if displayed { 1 } else { 2 }
        );
    }
}
