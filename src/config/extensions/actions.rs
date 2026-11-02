//! `[[actions]]`: operations a session runs for a caller, with JSON arguments and a JSON result.
use serde::Deserialize;

use crate::config::LaunchTemplate;
use crate::control::{ACTION_DEADLINE_MAX_MS, ActionConcurrency, ActionInfo};

const MAX_ACTIONS: usize = 128;
const MAX_SCHEMA_BYTES: usize = 64 * 1024;
const DEFAULT_DEADLINE_MS: u64 = 30_000;

#[derive(Clone, Debug)]
pub(crate) struct ExtensionAction {
    pub info: ActionInfo,
    pub launch: LaunchTemplate,
    /// Whether `{}` satisfies the arguments schema, which is what a palette can send.
    pub runs_without_arguments: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExtensionActionFile {
    id: String,
    title: String,
    exec: Vec<String>,
    args_schema: serde_json::Value,
    result_schema: serde_json::Value,
    #[serde(default = "default_deadline")]
    deadline_ms: u64,
    #[serde(default)]
    concurrency: ActionConcurrency,
    placement: Option<String>,
}

fn default_deadline() -> u64 {
    DEFAULT_DEADLINE_MS
}

pub(super) fn validate_actions(
    raw: Vec<ExtensionActionFile>,
    extension: &str,
    errors: &mut Vec<String>,
) -> Vec<ExtensionAction> {
    if raw.len() > MAX_ACTIONS {
        errors.push(format!(
            "an extension may declare at most {MAX_ACTIONS} actions"
        ));
        return Vec::new();
    }
    let mut seen = std::collections::HashSet::new();
    raw.into_iter()
        .filter_map(|action| {
            let id = format!("{extension}.{}", action.id);
            if !seen.insert(action.id.clone()) {
                errors.push(format!("duplicate action `{id}`"));
                return None;
            }
            match validate_action(action, id.clone()) {
                Ok(action) => Some(action),
                Err(error) => {
                    errors.push(format!("action `{id}`: {error}"));
                    None
                }
            }
        })
        .collect()
}

fn validate_action(action: ExtensionActionFile, id: String) -> Result<ExtensionAction, String> {
    if !crate::platform::paths::valid_extension_id(&action.id) {
        return Err("id must contain lowercase letters, digits, underscores, or hyphens".into());
    }
    if action.title.trim().is_empty() {
        return Err("title must be nonempty".into());
    }
    if !matches!(action.placement.as_deref(), None | Some("session")) {
        return Err("only session placement is supported; client placement is reserved".into());
    }
    if !(1..=ACTION_DEADLINE_MAX_MS).contains(&action.deadline_ms) {
        return Err(format!(
            "deadline_ms must be between 1 and {ACTION_DEADLINE_MAX_MS}"
        ));
    }
    if action.exec.is_empty() {
        return Err("exec must name a program".into());
    }
    let mut errors = Vec::new();
    super::placement::check_host_program(&action.exec, &id, &mut errors);
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    let args = compile_schema(&action.args_schema)?;
    compile_schema(&action.result_schema)?;
    let runs_without_arguments = args.is_valid(&serde_json::json!({}));
    Ok(ExtensionAction {
        info: ActionInfo {
            id,
            title: action.title,
            args_schema: action.args_schema,
            result_schema: action.result_schema,
            deadline_ms: action.deadline_ms,
            concurrency: action.concurrency,
        },
        launch: LaunchTemplate::Direct(action.exec),
        runs_without_arguments,
    })
}

fn compile_schema(schema: &serde_json::Value) -> Result<jsonschema::Validator, String> {
    let encoded = serde_json::to_vec(schema).map_err(|error| error.to_string())?;
    if encoded.len() > MAX_SCHEMA_BYTES {
        return Err(format!(
            "a schema may contain at most {MAX_SCHEMA_BYTES} bytes"
        ));
    }
    jsonschema::options()
        .offline()
        .build(schema)
        .map_err(|error| format!("invalid or externally referenced schema: {error}"))
}
