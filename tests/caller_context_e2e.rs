//! Verify the real CLI against detached session servers, with an unusable inherited UI socket.
mod common;

use std::process::{Command, Output};

use rozi::control::{ControlCommand, ControlRequest};
use rozi::platform::paths::PlatformEnv;
use rozi::session::headless::run_session_control;
use rozi::session::server::ServerSettings;

use crate::common::spawn_listener;

static DISCOVERY_UIS: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    // These tests share one runtime directory. A parallel sweep can hold the pane's pipe or, on
    // Windows, observe it as missing and the next `--here` then reports a stale session.
    let _discovery = DISCOVERY_UIS.lock().unwrap();
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
    let _discovery = DISCOVERY_UIS.lock().unwrap();
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
    let _discovery = DISCOVERY_UIS.lock().unwrap();
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

#[cfg(unix)]
#[test]
fn a_headless_created_pane_discovers_a_later_ui_despite_missing_or_stale_sockets() {
    use rozi::session::discovery::{DiscoveredSession, DiscoveredSessionStatus};
    use rozi::{AppRoot, Msg};
    use std::time::{Duration, Instant};
    use tui_lipan::TestBackend;

    rozi::test_support::isolate_user_dirs();
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let _discovery = DISCOVERY_UIS.lock().unwrap();
            struct Ui(TestBackend<AppRoot>);
            impl Drop for Ui {
                fn drop(&mut self) {
                    let _ = self.0.dispatch(Msg::RunAction(rozi::input::Action::Quit));
                }
            }
            let files = tempfile::tempdir().unwrap();
            let ready = files.path().join("ready");
            let go = files.path().join("go");
            let result = files.path().join("result");
            let server = spawn_listener(ServerSettings::default());
            // Give the child isolated user directories, but let the headless spawn policy supply its
            // real pane/session identity and omit ROZI_SOCKET. Its environment predates the UI.
            let mut argv = vec!["env".to_string()];
            for (key, value) in cli().get_envs() {
                if let Some(value) = value {
                    argv.push(format!(
                        "{}={}",
                        key.to_str().unwrap(),
                        value.to_str().unwrap()
                    ));
                }
            }
            argv.extend([
                "sh".into(),
                "-c".into(),
                r#"unset ROZI_CONFIG ROZI_EXTENSION ROZI_EXTENSION_CREDENTIAL
printf '%s' "${ROZI_SOCKET-unset}" > "$2"
while [ ! -f "$3" ]; do sleep 0.02; done
"$1" --here split --keep-open --argv "$1" --version > "$4.tmp" 2> "$4.err"
ROZI_SOCKET="$4.dead-socket" "$1" --here split --keep-open --argv "$1" --version > "$4.stale" 2> "$4.stale.err"
mv "$4.tmp" "$4"
"#
                .into(),
                "caller".into(),
                env!("CARGO_BIN_EXE_rozi").into(),
                ready.to_string_lossy().into_owned(),
                go.to_string_lossy().into_owned(),
                result.to_string_lossy().into_owned(),
            ]);
            let spawned = run_session_control(
                server.session(),
                request(ControlCommand::NewPane {
                    command: None,
                    argv: Some(argv),
                    local_cwd: None,
                    cwd: None,
                    title: None,
                    keep_open: true,
                    focus: false,
                    workspace: None,
                    size: None,
                }),
            )
            .unwrap();
            assert!(spawned.ok, "{spawned:?}");
            let deadline = Instant::now() + common::io_timeout();
            while !ready.exists() {
                assert!(Instant::now() < deadline, "headless caller did not start");
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(std::fs::read_to_string(&ready).unwrap(), "unset");
            let mut ui = Ui(TestBackend::new(rozi::test_support::configured_app()));
            while ui.0.state().command_link.is_none() {
                ui.0.pump().unwrap();
                assert!(Instant::now() < deadline);
            }
            ui.0.dispatch(Msg::SidebarSessionActivate(DiscoveredSession {
                name: server.session().into(),
                status: DiscoveredSessionStatus::Running {
                    panes: 1,
                    clients: 0,
                    has_layout: true,
                },
                origin: Default::default(),
                ephemeral: false,
                host: None,
                remote_target: None,
            }))
            .unwrap();
            loop {
                ui.0.pump().unwrap();
                let attachment = ui.0.state().current();
                if attachment.session_attached
                    && attachment.session_name.as_deref() == Some(server.session())
                {
                    break;
                }
                assert!(Instant::now() < deadline, "UI did not attach");
                std::thread::sleep(Duration::from_millis(10));
            }
            let direct = cli()
                .args(["--session", server.session(), "split"])
                .output()
                .unwrap();
            let reply: serde_json::Value = serde_json::from_slice(&direct.stdout).unwrap();
            assert_eq!(
                reply["code"], "not-controller",
                "the UI holds layout authority"
            );
            std::fs::write(&go, "go").unwrap();
            while !result.exists() {
                ui.0.render();
                ui.0.pump().unwrap();
                assert!(Instant::now() < deadline, "caller split did not complete");
                std::thread::sleep(Duration::from_millis(10));
            }
            let output = std::fs::read_to_string(&result).unwrap();
            let reply: serde_json::Value = serde_json::from_str(&output).expect(&output);
            assert_eq!(reply["ok"], true, "{reply}");
            let stale_output = std::fs::read_to_string(result.with_extension("stale")).unwrap();
            let stale_reply: serde_json::Value = serde_json::from_str(&stale_output).expect(&stale_output);
            assert_eq!(stale_reply["ok"], true, "{stale_reply}");
            let panes =
                run_session_control(server.session(), request(ControlCommand::ListPanes)).unwrap();
            assert_eq!(panes.data.unwrap().as_array().unwrap().len(), 3);
        })
        .unwrap()
        .join()
        .unwrap();
}

/// A server started from inside a pane inherits that pane's rozi environment. None of it may reach
/// the panes the server spawns: their identity, socket, and binary come from the spawn policy alone.
/// An exported integration marker also belongs to that starting shell, not the new pane's shell.
#[cfg(unix)]
#[test]
fn server_panes_never_inherit_the_starting_panes_rozi_environment() {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    struct Server(std::process::Child);
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    rozi::test_support::isolate_user_dirs();
    let files = tempfile::tempdir().unwrap();
    let session = format!("inherited-env-{}", std::process::id());
    let planted = [
        ("ROZI_PANE", "4242"),
        ("ROZI_SESSION_INSTANCE", "planted-instance"),
        ("ROZI_SOCKET", "/planted/ui.sock"),
        ("ROZI_BIN", "/planted/rozi"),
        ("ROZI_EXTENSION", "planted-extension"),
        ("ROZI_EXTENSION_CREDENTIAL", "planted-credential"),
        ("ROZI_SHELL_INTEGRATION_LOADED", "1"),
    ];
    let mut command = cli();
    command
        .args(["--server", &session])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for (key, value) in planted {
        command.env(key, value);
    }
    let _server = Server(command.spawn().unwrap());

    let deadline = Instant::now() + common::io_timeout();
    while run_session_control(&session, request(ControlCommand::ListPanes)).is_err() {
        assert!(Instant::now() < deadline, "session server did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    let dump = files.path().join("env");
    let spawned = run_session_control(
        &session,
        request(ControlCommand::NewPane {
            command: None,
            argv: Some(vec![
                "sh".into(),
                "-c".into(),
                r#"env > "$1.tmp" && mv "$1.tmp" "$1""#.into(),
                "pane".into(),
                dump.to_string_lossy().into_owned(),
            ]),
            local_cwd: None,
            cwd: None,
            title: None,
            keep_open: true,
            focus: false,
            workspace: None,
            size: None,
        }),
    )
    .unwrap();
    assert!(spawned.ok, "{spawned:?}");
    while !dump.exists() {
        assert!(
            Instant::now() < deadline,
            "pane did not report its environment"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let dumped = std::fs::read_to_string(&dump).unwrap();
    let env: HashMap<&str, &str> = dumped
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect();
    assert_eq!(env.get("ROZI"), Some(&"1"));
    for (key, value) in planted {
        assert_ne!(env.get(key), Some(&value), "{key} leaked into the pane");
    }
    // A headless pane has no UI, so it names none rather than the one that started its server.
    for key in [
        "ROZI_SOCKET",
        "ROZI_BIN",
        "ROZI_EXTENSION",
        "ROZI_EXTENSION_CREDENTIAL",
        "ROZI_SHELL_INTEGRATION_LOADED",
    ] {
        assert!(!env.contains_key(key), "{key} is set in a headless pane");
    }
}

/// Discoverable control endpoints accept liveness connections as well as context probes.
struct DiscoveryUi {
    endpoint: rozi::platform::ipc::IpcEndpoint,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    mutations: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl DiscoveryUi {
    fn new(caller: &rozi::session::protocol::PaneRef, displayed: bool, id: u32) -> Self {
        use std::io::{BufRead, BufReader, Write};
        use std::sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        };
        let endpoint = rozi::platform::ipc::EndpointRegistry::control_endpoint(
            &rozi::control::runtime_dir().unwrap(),
            id,
        );
        let listener = endpoint.bind().unwrap().into_listener();
        let stop = Arc::new(AtomicBool::new(false));
        let mutations = Arc::new(AtomicUsize::new(0));
        let worker_stop = stop.clone();
        let worker_mutations = mutations.clone();
        let caller = caller.clone();
        let thread = std::thread::spawn(move || {
            while !worker_stop.load(Ordering::SeqCst) {
                let mut stream = listener.accept().unwrap();
                if worker_stop.load(Ordering::SeqCst) {
                    break;
                }
                // `is_live` connects and drops. macOS refuses a receive timeout on that
                // already-closed socket (`EINVAL`) and the worker used to die, so later
                // probes never saw a UI. A closed probe is not a request.
                let _ = stream.set_read_timeout(Some(common::io_timeout()));
                let _ = stream.set_write_timeout(Some(common::io_timeout()));
                let Ok(cloned) = stream.try_clone() else {
                    continue;
                };
                let mut line = String::new();
                if BufReader::new(cloned).read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let request: ControlRequest = serde_json::from_str(&line).unwrap();
                let response = if request.command == ControlCommand::Context {
                    serde_json::json!({"ok":true,"data":{
                        "displayed_session": if displayed {caller.session_instance.as_str()} else {"another-session"},
                        "held_sessions":[caller.session_instance]
                    }})
                } else {
                    assert!(displayed, "an unrelated UI must never receive a mutation");
                    assert_eq!(request.caller_context.as_deref(), Some(&caller));
                    assert!(matches!(request.command, ControlCommand::NewPane { .. }));
                    worker_mutations.fetch_add(1, Ordering::SeqCst);
                    serde_json::json!({"ok":true,"data":{"id":999}})
                };
                writeln!(stream, "{response}").unwrap();
            }
        });
        Self {
            endpoint,
            stop,
            mutations,
            thread: Some(thread),
        }
    }

    fn mutations(&self) -> usize {
        self.mutations.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Drop for DiscoveryUi {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = self.endpoint.connect();
        self.thread.take().unwrap().join().unwrap();
        self.endpoint.remove_stale();
    }
}

#[test]
fn here_discovers_a_unique_displayed_session_among_multiple_uis() {
    rozi::test_support::isolate_user_dirs();
    let _discovery = DISCOVERY_UIS.lock().unwrap();
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
    let _controller = common::attach_client(server.endpoint(), server.session(), "controller");
    let matching = DiscoveryUi::new(&caller, true, 1000000001);
    let unrelated = DiscoveryUi::new(&caller, false, 1000000002);
    let ask = || {
        let mut command = cli();
        command
            .env("ROZI_PANE", caller.pane_id.to_string())
            .env("ROZI_SESSION_INSTANCE", caller.session_instance.as_str())
            .args(["--here", "split"]);
        command
    };
    for socket in [
        None,
        Some("dead-ui.sock".as_ref()),
        Some(unrelated.endpoint.path()),
    ] {
        let mut command = ask();
        if let Some(path) = socket {
            command.env("ROZI_SOCKET", path);
        }
        assert_eq!(data(command.output().unwrap())["id"], 999);
    }
    assert_eq!(matching.mutations(), 3);
    assert_eq!(unrelated.mutations(), 0);
    // No inherited preference and two matching UIs is ambiguous. Neither may get a mutation.
    let duplicate = DiscoveryUi::new(&caller, true, 1000000003);
    let ambiguous = ask().output().unwrap();
    assert!(!ambiguous.status.success());
    let reply: serde_json::Value = serde_json::from_slice(&ambiguous.stdout).unwrap();
    assert_eq!(reply["code"], "unsupported");
    assert!(reply["error"].as_str().unwrap().contains("no usable UI"));
    assert_eq!(matching.mutations(), 3);
    assert_eq!(duplicate.mutations(), 0);
    // An inherited UI that still matches remains the preferred attachment.
    assert_eq!(
        data(
            ask()
                .env("ROZI_SOCKET", matching.endpoint.path())
                .output()
                .unwrap()
        )["id"],
        999
    );
    assert_eq!(matching.mutations(), 4);
    assert_eq!(duplicate.mutations(), 0);
}

#[test]
fn here_explains_authority_held_by_an_attachment_without_a_local_ui() {
    use rozi::session::protocol::{ClientMessage, Frame, ServerMessage};
    rozi::test_support::isolate_user_dirs();
    let _discovery = DISCOVERY_UIS.lock().unwrap();
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
    // Model the session side of an SSH attachment: an attached controller, no shared filesystem,
    // and no control endpoint on the session host. No socket forwarding or authority delegation.
    let mut controller = common::TestConnection::connect(server.endpoint());
    let mut attach = common::attach_message(server.session(), "UI on another host");
    if let ClientMessage::Attach {
        shares_filesystem, ..
    } = &mut attach
    {
        *shares_filesystem = false;
    }
    controller.write_control(&attach);
    common::read_until(&mut controller, |frame| {
        matches!(frame, Frame::Control(ServerMessage::Attached { .. }))
    });
    let ask = || {
        let mut command = cli();
        command
            .env("ROZI_PANE", caller.pane_id.to_string())
            .env("ROZI_SESSION_INSTANCE", caller.session_instance.as_str());
        command
    };
    let assert_refusal = |output: Output, recovery: &str| {
        assert!(!output.status.success());
        let reply: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reply["code"], "unsupported", "{reply}");
        let error = reply["error"].as_str().unwrap();
        assert!(error.contains("across hosts is unsupported"), "{error}");
        assert!(error.contains(recovery), "{error}");
    };
    assert_refusal(
        ask().args(["--here", "split"]).output().unwrap(),
        "detach the controlling UI",
    );
    let named = cli()
        .args(["--session", server.session(), "split"])
        .output()
        .unwrap();
    let named: serde_json::Value = serde_json::from_slice(&named.stdout).unwrap();
    assert_eq!(
        named["code"], "not-controller",
        "ordinary named-session errors retain their meaning"
    );
    controller.write_control(&ClientMessage::SetInputLock { locked: true });
    common::read_until(&mut controller, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::ClientsChanged {
                input_locked: true,
                ..
            })
        )
    });
    assert_refusal(
        ask()
            .args(["--here", "send-text", "must not arrive"])
            .output()
            .unwrap(),
        "unlock input",
    );
    let capture = data(ask().args(["--here", "capture-pane"]).output().unwrap());
    assert_eq!(capture["id"], caller.pane_id);
    assert_eq!(
        data(ask().args(["--here", "list-panes"]).output().unwrap())
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
