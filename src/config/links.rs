use serde::Deserialize;

use super::file::UserCommandTableSpec;
use super::{LinkAction, LinkHandler};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(super) struct LinksFileConfig {
    handlers: Vec<LinkHandlerFileConfig>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LinkHandlerFileConfig {
    schemes: Vec<String>,
    files: Vec<String>,
    #[serde(flatten)]
    action: OpenActionFileConfig,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(super) struct OpenActionFileConfig {
    command: Option<String>,
    #[serde(flatten)]
    action: UserCommandTableSpec,
}

pub(super) fn parse_open_action(
    table: OpenActionFileConfig,
    context: &str,
    warnings: &mut Vec<String>,
) -> Option<LinkAction> {
    if let Some(command) = table.command {
        let command = command.trim();
        if command.is_empty()
            || table.action.run.is_some()
            || table.action.popup.is_some()
            || table.action.exec.is_some()
            || table.action.send.is_some()
            || table.action.keep_open.is_some()
        {
            warnings.push(format!(
                "{context} needs a nonempty `command` without inline action fields; skipped"
            ));
            return None;
        }
        return Some(LinkAction::Command(command.to_string()));
    }
    let table = table.action;
    if table.send.is_some() {
        warnings.push(format!(
            "{context} cannot use `send`; use `run`, `popup`, or `exec`; skipped"
        ));
        return None;
    }
    super::input::parse_user_command_action(table, context, warnings).map(LinkAction::Inline)
}

pub(super) fn build_handlers(raw: LinksFileConfig, warnings: &mut Vec<String>) -> Vec<LinkHandler> {
    raw.handlers
        .into_iter()
        .enumerate()
        .filter_map(|(index, handler)| {
            let context = format!("links.handlers[{index}]");
            let schemes: Vec<_> = handler
                .schemes
                .into_iter()
                .map(|scheme| scheme.trim().to_ascii_lowercase())
                .collect();
            if schemes.is_empty() || schemes.iter().any(|scheme| !valid_scheme(scheme)) {
                warnings.push(format!(
                    "{context} needs nonempty valid URI schemes; skipped"
                ));
                return None;
            }
            let files = match parse_files(handler.files, &schemes) {
                Ok(files) => files,
                Err(error) => {
                    warnings.push(format!("{context} {error}; skipped"));
                    return None;
                }
            };
            Some(LinkHandler {
                schemes,
                files,
                action: parse_open_action(handler.action, &context, warnings)?,
            })
        })
        .collect()
}

/// Validate a handler's `files` patterns, returning them lowercased for matching.
pub(super) fn parse_files(files: Vec<String>, schemes: &[String]) -> Result<Vec<String>, String> {
    if files.is_empty() {
        return Ok(files);
    }
    if !schemes.iter().any(|scheme| scheme == "file") {
        return Err("sets `files` without the `file` scheme".into());
    }
    files
        .into_iter()
        .map(|pattern| {
            let pattern = pattern.trim();
            if pattern.is_empty() || pattern.contains(['/', '\\']) {
                return Err(format!(
                    "needs file-name patterns in `files`, not `{pattern}`"
                ));
            }
            Ok(pattern.to_lowercase())
        })
        .collect()
}

pub(super) fn valid_scheme(scheme: &str) -> bool {
    scheme.starts_with(|ch: char| ch.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
}
