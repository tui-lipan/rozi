use std::collections::HashSet;
use std::path::Path;

use crate::config::{
    NamedCommand, ServiceConfig, ServiceLaunch, ServiceRestart, UserCommandAction,
};

use super::super::commands::valid_command_segment;
use super::manifest::{ExtensionCommandFile, ExtensionServiceFile};
use super::paths::{normalize_direct_argv, resolve_declared_path};
use super::{ExtensionInfo, RESERVED_EXTENSION_ENV, RESERVED_EXTENSION_IDS, clean_optional};

pub(super) fn validate_extension_id(id: Option<&str>, errors: &mut Vec<String>) -> bool {
    let Some(id) = id.filter(|id| !id.is_empty()) else {
        errors.push("missing required field `extension.id`".to_string());
        return false;
    };
    if !valid_command_segment(id) {
        errors.push("extension id must match [a-z0-9_-]+".to_string());
        return false;
    }
    if RESERVED_EXTENSION_IDS.contains(&id) {
        errors.push(format!("extension id `{id}` is reserved by rozi"));
        return false;
    }
    true
}

pub(crate) fn is_extension_command_id(id: &str) -> bool {
    let mut segments = id.split('.');
    let (Some(extension), Some(command), None) =
        (segments.next(), segments.next(), segments.next())
    else {
        return false;
    };
    valid_command_segment(extension)
        && !RESERVED_EXTENSION_IDS.contains(&extension)
        && valid_command_segment(command)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_command(
    raw: ExtensionCommandFile,
    extension_id: &str,
    category: &str,
    directory: &Path,
    env: &[(String, String)],
    seen: &mut HashSet<String>,
    info: &mut ExtensionInfo,
    commands: &mut Vec<NamedCommand>,
) {
    let Some(local_id) = raw
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
    else {
        info.errors
            .push("command missing required field `id`".to_string());
        return;
    };
    if !valid_command_segment(&local_id) {
        info.errors
            .push(format!("command id `{local_id}` must match [a-z0-9_-]+"));
        return;
    }
    let id = format!("{extension_id}.{local_id}");
    if !seen.insert(local_id) {
        info.errors.push(format!("duplicate command id `{id}`"));
        return;
    }
    info.commands.push(id.clone());
    let label = clean_optional(raw.label);
    let action_count = usize::from(raw.exec.is_some())
        + usize::from(raw.shell.is_some())
        + usize::from(raw.send.is_some());
    let action = match action_count {
        0 => {
            info.errors.push(format!(
                "extension command `{id}` requires exactly one of `exec`, `shell`, or `send`"
            ));
            None
        }
        1 => {
            if let Some(argv) = raw.exec {
                normalize_direct_argv(
                    argv,
                    directory,
                    &id,
                    &mut info.command_paths,
                    &mut info.errors,
                )
                .map(|argv| UserCommandAction::ExecDirect { argv })
            } else if raw.shell.is_some() {
                match clean_optional(raw.shell) {
                    Some(shell) => Some(UserCommandAction::Exec { command: shell }),
                    None => {
                        info.errors
                            .push(format!("extension command `{id}` has an empty `shell`"));
                        None
                    }
                }
            } else {
                match clean_optional(raw.send) {
                    Some(send) => Some(UserCommandAction::Send(send)),
                    None => {
                        info.errors
                            .push(format!("extension command `{id}` has an empty `send`"));
                        None
                    }
                }
            }
        }
        _ => {
            info.errors.push(format!(
                "extension command `{id}` declares conflicting actions; use exactly one of `exec`, `shell`, or `send`"
            ));
            None
        }
    };
    if let Some(action) = action {
        commands.push(NamedCommand {
            id,
            label,
            action,
            category: category.to_string(),
            env: env.to_vec(),
        });
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_service(
    mut raw: ExtensionServiceFile,
    extension_id: &str,
    directory: &Path,
    extension_dir: &str,
    seen: &mut HashSet<String>,
    info: &mut ExtensionInfo,
    services: &mut Vec<ServiceConfig>,
) {
    let Some(local_name) = raw
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
    else {
        info.errors
            .push("service missing required field `name`".to_string());
        return;
    };
    if !valid_command_segment(&local_name) {
        info.errors.push(format!(
            "service name `{local_name}` must match [a-z0-9_-]+"
        ));
        return;
    }
    let name = format!("{extension_id}.{local_name}");
    if !seen.insert(local_name) {
        info.errors.push(format!("duplicate service name `{name}`"));
        return;
    }
    info.services.push(name.clone());
    for reserved in RESERVED_EXTENSION_ENV {
        if raw.env.contains_key(*reserved) {
            info.errors.push(format!(
                "service `{name}` may not override reserved environment variable `{reserved}`"
            ));
        }
    }
    let launch_count = usize::from(raw.exec.is_some()) + usize::from(raw.shell.is_some());
    let launch = match launch_count {
        0 => {
            info.errors.push(format!(
                "extension service `{name}` requires exactly one of `exec` or `shell`"
            ));
            None
        }
        1 => {
            if let Some(argv) = raw.exec {
                normalize_direct_argv(
                    argv,
                    directory,
                    &name,
                    &mut info.service_paths,
                    &mut info.errors,
                )
                .map(ServiceLaunch::Direct)
            } else {
                match clean_optional(raw.shell) {
                    Some(shell) => Some(ServiceLaunch::Shell(shell)),
                    None => {
                        info.errors
                            .push(format!("extension service `{name}` has an empty `shell`"));
                        None
                    }
                }
            }
        }
        _ => {
            info.errors.push(format!(
                "extension service `{name}` declares both `exec` and `shell`"
            ));
            None
        }
    };
    let cwd = match clean_optional(raw.cwd) {
        Some(cwd) => {
            let path = resolve_declared_path(directory, &cwd);
            match std::fs::metadata(&path) {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => info.errors.push(format!(
                    "service `{name}` cwd is not a directory: {}",
                    path.display()
                )),
                Err(error) => info.errors.push(format!(
                    "service `{name}` cwd is unavailable at {}: {error}",
                    path.display()
                )),
            }
            Some(path.to_string_lossy().to_string())
        }
        None => Some(extension_dir.to_string()),
    };
    raw.env
        .insert("ROZI_EXTENSION".to_string(), extension_id.to_string());
    raw.env
        .insert("ROZI_EXTENSION_DIR".to_string(), extension_dir.to_string());
    let restart = match raw.restart.as_deref().map(str::trim) {
        None | Some("") | Some("on-failure") => Some(ServiceRestart::OnFailure),
        Some("always") => Some(ServiceRestart::Always),
        Some("never") => Some(ServiceRestart::Never),
        Some(other) => {
            info.errors.push(format!(
                "extension service `{name}` has unknown restart policy `{other}`"
            ));
            None
        }
    };
    if let (Some(launch), Some(restart)) = (launch, restart) {
        services.push(ServiceConfig {
            name,
            launch,
            cwd,
            restart,
            env: raw.env,
        });
    }
}
