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
    loop {
        let agents = run_session_control(server.session(), request(ControlCommand::AgentsList))
            .unwrap()
            .data
            .unwrap();
        if !agents.as_array().unwrap().is_empty() {
            break;
        }
        assert!(Instant::now() < deadline, "test child was not detected");
        std::thread::sleep(Duration::from_millis(10));
    }
    let report = |state, reason: Option<&str>, seq| {
        let response = run_session_control(
            server.session(),
            request(ControlCommand::AgentReport {
                target: Some(id),
                agent: "turn-test".into(),
                integration: "transaction-test".into(),
                state,
                reason: reason.map(str::to_string),
                native_session: None,
                seq,
            }),
        )
        .unwrap();
        assert!(response.ok, "{response:?}");
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
}
