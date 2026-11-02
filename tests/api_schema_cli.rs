use std::process::{Command, Stdio};

fn rozi(home: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rozi"));
    command
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_RUNTIME_DIR", home.join("runtime"))
        .env_remove("ROZI_SOCKET");
    command
}

#[test]
fn api_schema_prints_the_contract_api_describe_names() {
    let home = tempfile::tempdir().expect("temporary home");

    let output = rozi(home.path())
        .args(["api", "schema"])
        .output()
        .expect("rozi runs");
    assert!(output.status.success(), "{output:?}");
    let schema: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the schema is JSON");

    let output = rozi(home.path())
        .args(["api", "describe"])
        .output()
        .expect("rozi runs");
    assert!(output.status.success(), "{output:?}");
    let description: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the description is JSON");

    assert_eq!(schema["x-rozi-schema-version"], description["schema"]);
    assert_eq!(schema["x-rozi-api-version"], description["api"]);
    assert!(schema["$defs"]["ControlCommand"].is_object());
}

#[test]
fn api_schema_accepts_a_reader_that_stops_early() {
    let home = tempfile::tempdir().expect("temporary home");
    let mut child = rozi(home.path())
        .args(["api", "schema"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("rozi runs");
    drop(child.stdout.take());

    let output = child.wait_with_output().expect("rozi exits");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn api_command_describes_a_command_from_the_binary_itself() {
    let home = tempfile::tempdir().expect("temporary home");
    let output = rozi(home.path())
        .args(["api", "commands"])
        .output()
        .expect("rozi runs");
    assert!(output.status.success(), "{output:?}");
    let commands: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the list is JSON");
    assert!(
        commands
            .as_array()
            .unwrap()
            .iter()
            .any(|command| command["id"] == "invoke"),
        "{commands}"
    );

    let output = rozi(home.path())
        .args(["api", "command", "invoke"])
        .output()
        .expect("rozi runs");
    assert!(output.status.success(), "{output:?}");
    let description: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the description is JSON");
    assert_eq!(description["id"], "invoke");
    assert_eq!(description["reply"][0], "InvocationResult");
    assert!(description["request"]["properties"]["args"].is_object());

    let output = rozi(home.path())
        .args(["api", "command", "nope"])
        .output()
        .expect("rozi runs");
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("rozi api commands"));
}
