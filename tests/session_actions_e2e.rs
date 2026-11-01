//! Deployed actions called through a real session server, with real processes behind them.
#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine as _;
use common::{
    ServerGuard, connect_when_ready, private_temp_dir, read_until, subprocess_endpoint,
    unique_session_name,
};
use rozi::control::{ControlCommand, ControlErrorCode, ControlRequest, ControlResponse};
use rozi::extension_runtime::bundle::{Bundle, BundleFile};
use rozi::session::protocol::{
    ClientMessage, Frame, MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION, ServerMessage,
};
use serde_json::{Value, json};

const OPEN_SCHEMA: &str = "args_schema={}\nresult_schema={}\n";

struct Lab {
    root: PathBuf,
    session: String,
    server: ServerGuard,
}

fn environment(command: &mut Command, root: &Path) {
    command
        .env("ROZI_CONFIG", root.join("config.toml"))
        .env("XDG_RUNTIME_DIR", root)
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env_remove("ROZI_SOCKET")
        .env_remove("ROZI_PANE")
        .env_remove("ROZI_SESSION_INSTANCE")
        .env_remove("ROZI_EXTENSION_CREDENTIAL");
}

impl Lab {
    fn new() -> Self {
        let root = private_temp_dir();
        std::fs::write(root.join("config.toml"), "[session]\nresurrect = true\n").unwrap();
        let session = unique_session_name();
        let mut command = Command::new(env!("CARGO_BIN_EXE_rozi"));
        command
            .args(["--server", &session])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        environment(&mut command, &root);
        let server = ServerGuard::new(command.spawn().expect("start server"), root.clone());
        Self {
            root,
            session,
            server,
        }
    }

    /// Deploy extension `demo` from the manifest body and its scripts.
    fn deploy(&mut self, manifest: &str, scripts: &[(&str, &str)]) {
        let mut files = vec![BundleFile {
            path: "extension.toml".into(),
            executable: false,
            contents: format!(
                "[extension]\nid='demo'\ntitle='Demo'\nversion='1'\napi=1\n{manifest}"
            )
            .into_bytes(),
        }];
        files.extend(scripts.iter().map(|(path, body)| BundleFile {
            path: (*path).into(),
            executable: false,
            contents: body.as_bytes().to_vec(),
        }));
        let bundle = Bundle::from_files(files).unwrap();
        let archive = bundle.archive();
        let response = self.ask(ControlCommand::ExtensionsDeploy {
            id: "demo".into(),
            digest: bundle.digest().into(),
            size: archive.len() as u64,
            offset: 0,
            data: base64::engine::general_purpose::STANDARD.encode(&archive),
            last: true,
        });
        assert!(response.ok, "{response:?}");
    }

    fn connection(&mut self) -> common::TestConnection {
        connect_when_ready(
            &subprocess_endpoint(&self.root, &self.session),
            self.server.child_mut(),
        )
    }

    fn send(&mut self, command: ControlCommand) -> common::TestConnection {
        let mut client = self.connection();
        client.write_control(&ClientMessage::SessionControl {
            capabilities: None,
            session: self.session.clone(),
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
        client
    }

    fn ask(&mut self, command: ControlCommand) -> ControlResponse {
        let mut client = self.send(command);
        answer(&mut client)
    }

    fn invoke(&mut self, action: &str, args: Value) -> ControlResponse {
        self.ask(invocation(action, args))
    }

    fn running(&mut self) -> Vec<Value> {
        let response = self.ask(ControlCommand::InvocationList);
        assert!(response.ok, "{response:?}");
        response.data.unwrap().as_array().unwrap().clone()
    }

    fn state_file(&self, name: &str) -> PathBuf {
        self.root.join("data/rozi/extension-state/demo").join(name)
    }
}

fn invocation(action: &str, args: Value) -> ControlCommand {
    ControlCommand::Invoke {
        action: action.into(),
        args,
        timeout_ms: None,
    }
}

fn answer(client: &mut common::TestConnection) -> ControlResponse {
    let mut response = None;
    read_until(client, |frame| match frame {
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

fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + common::io_timeout();
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn alive(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

fn pid_in(lab: &Lab, name: &str) -> String {
    let path = lab.state_file(name);
    let mut pid = String::new();
    wait_until(name, || {
        pid = std::fs::read_to_string(&path).unwrap_or_default();
        pid.ends_with('\n')
    });
    pid.trim().to_string()
}

fn code(response: &ControlResponse) -> Option<ControlErrorCode> {
    assert!(!response.ok, "{response:?}");
    response.code
}

#[test]
fn a_call_receives_validated_arguments_and_answers_a_validated_result() {
    let mut lab = Lab::new();
    lab.deploy(
        r#"
[[actions]]
id = "echo"
title = "Echo"
exec = ["sh", "./echo.sh"]
args_schema = { type = "object", required = ["name"], properties = { name = { type = "string" } } }
result_schema = { type = "object", required = ["args", "action"] }

[[actions]]
id = "wrong-shape"
title = "Wrong shape"
exec = ["sh", "-c", "echo '{}'"]
args_schema = {}
result_schema = { type = "object", required = ["missing"] }

[[actions]]
id = "not-json"
title = "Not JSON"
exec = ["sh", "-c", "echo '{} and more'"]
args_schema = {}
result_schema = {}

[[actions]]
id = "too-long"
title = "Too long"
exec = ["sh", "-c", "printf '\"'; head -c 600000 /dev/zero | tr '\\0' x; printf '\"'"]
args_schema = {}
result_schema = {}

[[actions]]
id = "fails"
title = "Fails"
exec = ["sh", "-c", "echo secret; echo why >&2; exit 3"]
args_schema = {}
result_schema = {}
"#,
        &[(
            "echo.sh",
            r#"args=$(cat)
printf '{"args":%s,"action":"%s","id":"%s"}' "$args" "$ROZI_ACTION" "$ROZI_INVOCATION_ID"
"#,
        )],
    );

    let response = lab.invoke("demo.echo", json!({"name": "rozi"}));
    assert!(response.ok, "{response:?}");
    let data = response.data.unwrap();
    assert_eq!(data["result"]["args"]["name"], "rozi");
    assert_eq!(data["result"]["action"], "demo.echo");
    assert_eq!(data["result"]["id"], data["invocation_id"]);

    for (action, args, expected) in [
        (
            "demo.echo",
            json!({"name": 7}),
            ControlErrorCode::InvalidArguments,
        ),
        ("demo.echo", json!({}), ControlErrorCode::InvalidArguments),
        (
            "demo.wrong-shape",
            json!({}),
            ControlErrorCode::InvalidResult,
        ),
        ("demo.not-json", json!({}), ControlErrorCode::InvalidResult),
        ("demo.too-long", json!({}), ControlErrorCode::InvalidResult),
        ("demo.fails", json!({}), ControlErrorCode::ActionFailed),
        ("demo.nothing", json!({}), ControlErrorCode::ActionNotFound),
    ] {
        let response = lab.invoke(action, args.clone());
        assert_eq!(
            code(&response),
            Some(expected),
            "{action} {args}: {response:?}"
        );
    }

    let oversized = "x".repeat(rozi::control::ACTION_PAYLOAD_BYTES + 1);
    let response = lab.invoke("demo.echo", json!({ "name": oversized }));
    assert_eq!(code(&response), Some(ControlErrorCode::InvalidArguments));
}

#[test]
fn concurrency_limits_refuse_rather_than_wait() {
    let mut lab = Lab::new();
    lab.deploy(
        &format!(
            r#"
[[actions]]
id = "one"
title = "One at a time"
exec = ["sleep", "30"]
concurrency = "singleton"
{OPEN_SCHEMA}
[[actions]]
id = "many"
title = "Many"
exec = ["sleep", "30"]
{OPEN_SCHEMA}"#
        ),
        &[],
    );

    let mut held = vec![lab.send(invocation("demo.one", json!({})))];
    wait_until("the singleton call", || lab.running().len() == 1);
    assert_eq!(
        code(&lab.invoke("demo.one", json!({}))),
        Some(ControlErrorCode::ActionBusy)
    );
    for _ in 0..7 {
        held.push(lab.send(invocation("demo.many", json!({}))));
    }
    wait_until("eight calls", || lab.running().len() == 8);
    assert_eq!(
        code(&lab.invoke("demo.many", json!({}))),
        Some(ControlErrorCode::InvocationLimit)
    );
    drop(held);
    wait_until("the callers' calls to end", || lab.running().is_empty());
}

#[test]
fn a_deadline_kills_the_whole_process_tree() {
    let mut lab = Lab::new();
    lab.deploy(
        &format!(
            r#"
[[actions]]
id = "slow"
title = "Slow"
exec = ["sh", "./slow.sh"]
deadline_ms = 300
{OPEN_SCHEMA}
[[actions]]
id = "patient"
title = "Patient"
exec = ["sleep", "30"]
{OPEN_SCHEMA}"#
        ),
        &[(
            "slow.sh",
            r#"sleep 30 &
echo $! > "$ROZI_EXTENSION_STATE_DIR/descendant"
wait
"#,
        )],
    );

    let response = lab.invoke("demo.slow", json!({}));
    assert_eq!(code(&response), Some(ControlErrorCode::InvocationTimeout));
    let descendant = pid_in(&lab, "descendant");
    wait_until("the descendant to die", || !alive(&descendant));

    // A caller can shorten a deadline.
    let started = Instant::now();
    let response = lab.ask(ControlCommand::Invoke {
        action: "demo.patient".into(),
        args: json!({}),
        timeout_ms: Some(100),
    });
    assert_eq!(code(&response), Some(ControlErrorCode::InvocationTimeout));
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn nested_calls_inherit_depth_and_the_remaining_deadline() {
    let mut lab = Lab::new();
    lab.deploy(
        &format!(
            r#"
[[actions]]
id = "recurse"
title = "Recurse"
exec = ["sh", "./recurse.sh"]
{OPEN_SCHEMA}
[[actions]]
id = "short-parent"
title = "Short parent"
exec = ["sh", "./short.sh"]
deadline_ms = 400
{OPEN_SCHEMA}
[[actions]]
id = "long-child"
title = "Long child"
exec = ["sleep", "30"]
deadline_ms = 600000
{OPEN_SCHEMA}"#
        ),
        &[
            (
                "recurse.sh",
                // A process may not claim a shallower depth: it can only present its credential.
                r#"cat > /dev/null
"$ROZI_BIN" invoke demo.recurse >> "$ROZI_EXTENSION_STATE_DIR/errors" 2>&1
printf '{}'
"#,
            ),
            (
                "short.sh",
                r#"cat > /dev/null
if "$ROZI_BIN" invoke demo.long-child > /dev/null 2>&1; then
  printf '"child returned"'
else
  printf '"child refused"'
fi
"#,
            ),
        ],
    );

    let response = lab.invoke("demo.recurse", json!({}));
    assert!(response.ok, "{response:?}");
    let errors = std::fs::read_to_string(lab.state_file("errors")).unwrap();
    assert!(errors.contains("nest at most 8"), "{errors}");

    // The child declares ten minutes, and the parent has 400 ms: the parent's deadline wins, and
    // the child's process goes with it.
    let started = Instant::now();
    let response = lab.invoke("demo.short-parent", json!({}));
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert!(
        !response.ok || response.data.as_ref().unwrap()["result"] == "child refused",
        "{response:?}"
    );
    wait_until("nothing left running", || lab.running().is_empty());
}

#[test]
fn a_call_ends_with_its_caller_or_a_cancel() {
    let mut lab = Lab::new();
    lab.deploy(
        &format!(
            r#"
[[actions]]
id = "wait"
title = "Wait"
exec = ["sh", "./wait.sh"]
{OPEN_SCHEMA}"#
        ),
        &[(
            "wait.sh",
            r#"echo $$ > "$ROZI_EXTENSION_STATE_DIR/$ROZI_INVOCATION_ID"
exec sleep 30
"#,
        )],
    );

    let caller = lab.send(invocation("demo.wait", json!({})));
    wait_until("the call", || lab.running().len() == 1);
    let id = lab.running()[0]["invocation_id"]
        .as_str()
        .unwrap()
        .to_string();
    let pid = pid_in(&lab, &id);
    drop(caller);
    wait_until("the process to die with its caller", || !alive(&pid));
    wait_until("the call to end", || lab.running().is_empty());

    let mut caller = lab.send(invocation("demo.wait", json!({})));
    wait_until("the call", || lab.running().len() == 1);
    let id = lab.running()[0]["invocation_id"]
        .as_str()
        .unwrap()
        .to_string();
    let pid = pid_in(&lab, &id);
    let cancelled = lab.ask(ControlCommand::InvocationCancel {
        invocation_id: id.clone(),
    });
    assert!(cancelled.ok, "{cancelled:?}");
    assert_eq!(
        code(&answer(&mut caller)),
        Some(ControlErrorCode::InvocationCancelled)
    );
    assert!(!alive(&pid));
    assert_eq!(
        code(&lab.ask(ControlCommand::InvocationCancel { invocation_id: id })),
        Some(ControlErrorCode::InvocationNotFound)
    );
}

#[test]
fn redeploying_cancels_calls_and_retires_their_credentials() {
    let mut lab = Lab::new();
    let manifest = format!(
        r#"
[[actions]]
id = "wait"
title = "Wait"
exec = ["sh", "./wait.sh"]
{OPEN_SCHEMA}"#
    );
    let script = r#"printf '%s' "$ROZI_EXTENSION_CREDENTIAL" > "$ROZI_EXTENSION_STATE_DIR/credential.tmp"
mv "$ROZI_EXTENSION_STATE_DIR/credential.tmp" "$ROZI_EXTENSION_STATE_DIR/credential"
exec sleep 30
"#;
    lab.deploy(&manifest, &[("wait.sh", script)]);
    let mut caller = lab.send(invocation("demo.wait", json!({})));
    let path = lab.state_file("credential");
    wait_until("the credential", || path.exists());
    let credential = std::fs::read_to_string(&path).unwrap();

    let list = |credential: String| ControlRequest {
        command: ControlCommand::ListPanes,
        source_pane: None,
        source_session: None,
        caller_context: None,
        extension: None,
        credential: Some(credential),
    };
    let ask_as = |lab: &mut Lab, credential: String| {
        let mut client = lab.connection();
        client.write_control(&ClientMessage::SessionControl {
            capabilities: None,
            session: lab.session.clone(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: MIN_SUPPORTED_PROTOCOL,
            request: list(credential),
        });
        answer(&mut client)
    };
    assert!(ask_as(&mut lab, credential.clone()).ok);

    lab.deploy(&manifest, &[("wait.sh", script), ("changed", "yes")]);
    assert_eq!(
        code(&answer(&mut caller)),
        Some(ControlErrorCode::InvocationCancelled)
    );
    assert_eq!(
        code(&ask_as(&mut lab, credential)),
        Some(ControlErrorCode::ExtensionInactive)
    );

    let response = lab.ask(ControlCommand::ExtensionsUndeploy { id: "demo".into() });
    assert!(response.ok, "{response:?}");
    assert_eq!(
        code(&lab.invoke("demo.wait", json!({}))),
        Some(ControlErrorCode::ActionNotFound)
    );
}

#[test]
fn a_process_that_escapes_its_group_cannot_hang_a_call() {
    if Command::new("setsid").arg("--version").output().is_err() {
        return;
    }
    let mut lab = Lab::new();
    lab.deploy(
        &format!(
            r#"
[[actions]]
id = "escape"
title = "Escape"
exec = ["sh", "-c", "setsid sleep 5 & echo '{{}}'"]
{OPEN_SCHEMA}"#
        ),
        &[],
    );
    let started = Instant::now();
    let response = lab.invoke("demo.escape", json!({}));
    assert_eq!(code(&response), Some(ControlErrorCode::ActionFailed));
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn the_command_line_calls_an_action_and_reports_failure() {
    let mut lab = Lab::new();
    lab.deploy(
        &format!(
            r#"
[[actions]]
id = "add"
title = "Add"
exec = ["sh", "-c", "cat"]
{OPEN_SCHEMA}"#
        ),
        &[],
    );
    let run = |args: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rozi"));
        command.args(["--session", &lab.session]).args(args);
        environment(&mut command, &lab.root);
        command.output().expect("rozi runs")
    };
    let output = run(&["invoke", "demo.add", "--args-json", r#"{"a":1}"#]);
    assert!(output.status.success(), "{output:?}");
    let printed: Value = serde_json::from_slice(&output.stdout).expect("JSON");
    assert_eq!(printed["data"]["result"]["a"], 1);

    let output = run(&[
        "invoke",
        "demo.add",
        "--args-json",
        r#"{"a":1}"#,
        "--format",
        "text",
    ]);
    assert!(output.status.success(), "{output:?}");
    let printed: Value = serde_json::from_slice(&output.stdout).expect("only the result");
    assert_eq!(printed, json!({"a": 1}));

    let output = run(&["invoke", "demo.none"]);
    assert!(!output.status.success());
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(printed.contains("action-not-found"), "{printed}");

    let output = run(&["invocation", "list"]);
    assert!(output.status.success(), "{output:?}");

    let output = run(&["actions", "list", "--format", "json"]);
    assert!(output.status.success(), "{output:?}");
    let printed: Value = serde_json::from_slice(&output.stdout).expect("JSON");
    assert_eq!(printed["data"][0]["id"], "demo.add");
    assert_eq!(printed["data"][0]["deadline_ms"], 30000);
    assert_eq!(printed["data"][0]["concurrency"], "parallel");
    assert_eq!(printed["data"][0]["result_schema"], json!({}));

    let output = run(&["actions", "list", "--format", "text"]);
    assert!(output.status.success(), "{output:?}");
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(
        printed.contains("demo.add") && printed.contains("Add"),
        "{printed}"
    );

    let output = run(&["invocation", "list", "--format", "text"]);
    assert!(String::from_utf8_lossy(&output.stdout).contains("No action calls running"));
}

#[test]
fn attached_clients_learn_the_palette_and_read_only_ones_cannot_call() {
    let mut lab = Lab::new();
    let endpoint = subprocess_endpoint(&lab.root, &lab.session);
    let _ = lab.connection();
    let session = lab.session.clone();
    let attach = |read_only: bool| {
        let mut client = common::TestConnection::connect(&endpoint);
        client.write_control(&ClientMessage::Attach {
            capabilities: None,
            session: session.clone(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: MIN_SUPPORTED_PROTOCOL,
            label: "test".into(),
            read_only,
            shares_filesystem: true,
            expected_server_nonce: None,
        });
        client
    };
    let actions_from = |client: &mut common::TestConnection| {
        let mut actions = None;
        read_until(client, |frame| {
            if let Frame::Control(ServerMessage::SessionActions { actions: listed }) = frame {
                actions = Some(listed.clone());
                true
            } else {
                false
            }
        });
        actions.unwrap()
    };
    let mut writable = attach(false);
    read_until(&mut writable, |frame| {
        matches!(frame, Frame::Control(ServerMessage::Attached { .. }))
    });

    lab.deploy(
        &format!(
            r#"
[[actions]]
id = "plain"
title = "Plain"
exec = ["sh", "-c", "cat"]
{OPEN_SCHEMA}
[[actions]]
id = "needs-args"
title = "Needs arguments"
exec = ["sh", "-c", "cat"]
args_schema = {{ type = "object", required = ["name"] }}
result_schema = {{}}
"#
        ),
        &[],
    );
    let offered = actions_from(&mut writable);
    assert_eq!(offered.len(), 1, "{offered:?}");
    assert_eq!(offered[0].id, "demo.plain");

    let mut read_only = attach(true);
    assert_eq!(actions_from(&mut read_only)[0].id, "demo.plain");
    let attached = |client: &mut common::TestConnection, request_id, command| {
        client.write_control(&ClientMessage::AttachedControl {
            request_id,
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
        read_until(client, |frame| match frame {
            Frame::Control(ServerMessage::AttachedControlResult {
                request_id: id,
                response: reply,
            }) if *id == request_id => {
                response = Some(reply.clone());
                true
            }
            _ => false,
        });
        response.unwrap()
    };
    assert_eq!(
        code(&attached(
            &mut read_only,
            1,
            invocation("demo.plain", json!({}))
        )),
        Some(ControlErrorCode::ReadOnly)
    );
    assert!(attached(&mut read_only, 2, ControlCommand::InvocationList).ok);
    let response = attached(&mut writable, 3, invocation("demo.plain", json!({"a": 1})));
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data.unwrap()["result"]["a"], 1);

    let response = lab.ask(ControlCommand::ExtensionsUndeploy { id: "demo".into() });
    assert!(response.ok, "{response:?}");
    assert!(actions_from(&mut writable).is_empty());
}
