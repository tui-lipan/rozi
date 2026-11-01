use std::io::{self, Write};

use tui_lipan::Result;

/// Print the control API contract implemented by this binary.
pub(crate) fn run_api_describe_cli() -> Result<()> {
    let description = crate::control::ApiDescription::current();
    println!(
        "{}",
        serde_json::to_string_pretty(&description).expect("API description serializes")
    );
    Ok(())
}

/// Print the JSON Schema of the control API this binary implements.
pub(crate) fn run_api_schema_cli() -> Result<()> {
    write_ignoring_closed_pipe(
        &mut io::stdout().lock(),
        crate::control::API_SCHEMA_DOCUMENT,
    )?;
    Ok(())
}

/// List every control command the embedded schema describes.
pub(crate) fn run_api_commands_cli() -> Result<()> {
    print_json(&serde_json::Value::Array(command_summaries(&schema())))
}

/// Describe one control command from the embedded schema.
pub(crate) fn run_api_command_cli(id: &str) -> Result<()> {
    let Some(description) = describe_command(&schema(), id) else {
        eprintln!("unknown control command `{id}`; `rozi api commands` lists them");
        std::process::exit(1);
    };
    print_json(&description)
}

fn print_json(value: &serde_json::Value) -> Result<()> {
    let text = serde_json::to_string_pretty(value).expect("a JSON value serializes") + "\n";
    write_ignoring_closed_pipe(&mut io::stdout().lock(), &text)?;
    Ok(())
}

fn schema() -> serde_json::Value {
    serde_json::from_str(crate::control::API_SCHEMA_DOCUMENT).expect("the embedded schema is JSON")
}

/// What a command's reply carries in `data`, for the commands whose reply has a named payload. The
/// schema cannot say this itself: one response envelope carries every command's answer.
const REPLY_PAYLOADS: &[(&str, &[&str])] = &[
    ("actions-list", &["ActionInfo[]"]),
    ("agent-get", &["AgentInfo"]),
    ("agent-prompt", &["AgentPromptAccepted", "AgentWaitResult"]),
    ("agent-read", &["PaneCapture"]),
    ("agent-wait", &["AgentWaitResult"]),
    ("agents-list", &["AgentListPayload"]),
    ("agents-overview", &["GlobalAgentListPayload"]),
    ("capture-pane", &["PaneCapture"]),
    ("capture-ui", &["UiCapture"]),
    ("context", &["CallerContext", "UiContextMetadata"]),
    ("extension-runtime-status", &["ExtensionRuntimeReport"]),
    ("extensions-deploy", &["ExtensionDeploymentUpload"]),
    ("extensions-status", &["ExtensionDeploymentReport"]),
    ("extensions-undeploy", &["ExtensionUndeployReport"]),
    ("invocation-cancel", &["InvocationInfo"]),
    ("invocation-list", &["InvocationInfo[]"]),
    ("invoke", &["InvocationResult"]),
    ("layout-get", &["LayoutReport"]),
    ("layout-set", &["LayoutChange"]),
    ("list-attached-panes", &["PaneListPayload"]),
    ("list-panes", &["PaneListPayload"]),
    ("metrics", &["SessionMetricsReport", "RuntimeMetrics"]),
    ("new-pane", &["NewPaneAccepted"]),
    ("notify", &["SessionNotification"]),
    ("pane-close", &["PaneClosed"]),
    ("pane-logging", &["PaneLoggingState"]),
    ("pane-move", &["LayoutChange"]),
    ("pane-reveal", &["PaneRevealed"]),
    ("pane-set", &["LayoutChange"]),
    ("pane-swap", &["LayoutChange"]),
    ("popup", &["NewPaneAccepted"]),
    ("record-list", &["RecordingListPayload"]),
    ("record-mark", &["RecordingMarked"]),
    ("record-start", &["RecordingInfo"]),
    ("record-stop", &["RecordingStopList"]),
    ("record-ui-start", &["UiRecordingInfo"]),
    ("record-ui-stop", &["UiRecordingStopped"]),
    ("send-keys", &["PaneCapture"]),
    ("send-text", &["PaneCapture"]),
    ("subscribe", &["SessionSubscribeRecord", "WireEvent"]),
];

/// Commands that answer with no named payload. Listed so that a new command has to choose.
#[cfg(test)]
const NO_REPLY_PAYLOAD: &[&str] = &[
    "agent-go",
    "agent-release",
    "agent-report",
    "command-visibility",
    "focus",
    "move-to-workspace",
    "pick",
    "prompt",
    "publish",
    "record-ui-mark",
    "run-action",
    "set-status",
    "sidebar-publish",
    "switch-workspace",
];

fn command_variants(schema: &serde_json::Value) -> impl Iterator<Item = &serde_json::Value> {
    schema["$defs"]["ControlCommand"]["oneOf"]
        .as_array()
        .expect("ControlCommand is a oneOf")
        .iter()
}

fn command_id(variant: &serde_json::Value) -> &str {
    variant["properties"]["cmd"]["const"]
        .as_str()
        .expect("each command names its cmd")
}

fn command_summaries(schema: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut commands: Vec<_> = command_variants(schema)
        .map(|variant| {
            serde_json::json!({
                "id": command_id(variant),
                "description": variant.get("description"),
            })
        })
        .collect();
    commands.sort_by_key(|command| command["id"].as_str().map(str::to_owned));
    commands
}

/// A command's request schema, what its reply carries, and every definition those two refer to, so
/// a caller holds the whole contract without reading the rest of the schema.
fn describe_command(schema: &serde_json::Value, id: &str) -> Option<serde_json::Value> {
    let variant = command_variants(schema).find(|variant| command_id(variant) == id)?;
    let reply = REPLY_PAYLOADS
        .iter()
        .find(|(command, _)| *command == id)
        .map_or(&[][..], |(_, payloads)| *payloads);
    let mut definitions = serde_json::Map::new();
    let mut pending = Vec::new();
    collect_references(variant, &mut pending);
    pending.extend(
        reply
            .iter()
            .map(|payload| payload.trim_end_matches("[]").to_string()),
    );
    while let Some(name) = pending.pop() {
        if definitions.contains_key(&name) {
            continue;
        }
        let definition = &schema["$defs"][&name];
        collect_references(definition, &mut pending);
        definitions.insert(name, definition.clone());
    }
    Some(serde_json::json!({
        "id": id,
        "description": variant.get("description"),
        "request": variant,
        "reply": reply,
        "definitions": definitions,
    }))
}

fn collect_references(value: &serde_json::Value, names: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, value) in object {
                if key == "$ref"
                    && let Some(name) = value.as_str().and_then(|r| r.strip_prefix("#/$defs/"))
                {
                    names.push(name.to_string());
                }
                collect_references(value, names);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_references(item, names);
            }
        }
        _ => {}
    }
}

/// The document is far larger than a pipe's buffer, so a reader such as `head` that stops early is
/// an ordinary way for this to end rather than a failure.
fn write_ignoring_closed_pipe(out: &mut impl Write, text: &str) -> io::Result<()> {
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reply_payload_names_a_real_command_and_definition() {
        let schema = schema();
        let ids: Vec<_> = command_variants(&schema).map(command_id).collect();
        for (command, payloads) in REPLY_PAYLOADS {
            assert!(ids.contains(command), "{command} is not a command");
            for payload in *payloads {
                let name = payload.trim_end_matches("[]");
                assert!(
                    schema["$defs"][name].is_object() || schema["$defs"][name].is_boolean(),
                    "{name} is not in the schema"
                );
            }
        }
    }

    #[test]
    fn every_command_is_described_and_chooses_its_reply() {
        let schema = schema();
        for variant in command_variants(&schema) {
            let id = command_id(variant);
            assert!(
                variant["description"].is_string(),
                "{id} has no description"
            );
            let named = REPLY_PAYLOADS.iter().any(|(command, _)| *command == id);
            assert_ne!(
                named,
                NO_REPLY_PAYLOAD.contains(&id),
                "{id} must be in exactly one of REPLY_PAYLOADS and NO_REPLY_PAYLOAD"
            );
        }
    }

    #[test]
    fn a_command_description_is_self_contained() {
        let schema = schema();
        assert_eq!(
            command_summaries(&schema).len(),
            command_variants(&schema).count()
        );
        for variant in command_variants(&schema) {
            let description = describe_command(&schema, command_id(variant)).unwrap();
            let mut references = Vec::new();
            collect_references(&description["request"], &mut references);
            collect_references(&description["definitions"], &mut references);
            for name in references {
                assert!(
                    description["definitions"].get(&name).is_some(),
                    "{} refers to {name} without carrying it",
                    command_id(variant)
                );
            }
        }
        let invoke = describe_command(&schema, "invoke").unwrap();
        assert_eq!(invoke["reply"][0], "InvocationResult");
        assert!(invoke["definitions"]["InvocationResult"].is_object());
        assert!(invoke["request"]["properties"]["action"].is_object());
        assert!(describe_command(&schema, "nope").is_none());
    }
}
