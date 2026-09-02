use std::collections::HashSet;
use std::path::Path;
use std::str::FromStr;

use crate::config::{
    NamedCommand, ServiceConfig, ServiceLaunch, ServiceRestart, SidebarTab, SidebarTabId,
    UserCommandAction,
};

use super::super::commands::valid_command_segment;
use super::manifest::{
    ExtensionCommandFile, ExtensionNavigationTargetFile, ExtensionServiceFile,
    ExtensionSidebarTabFile,
};
use super::paths::{normalize_direct_argv, resolve_declared_path};
use super::{
    ExtensionInfo, ExtensionNavigationTargetDiagnostic, RESERVED_EXTENSION_ENV,
    RESERVED_EXTENSION_IDS, clean_optional,
};

pub(super) fn validate_requested_extension_id(id: &str) -> Result<(), String> {
    if id.trim() != id {
        return Err("extension id may not have leading or trailing whitespace".to_string());
    }
    let mut errors = Vec::new();
    validate_extension_id(Some(id), &mut errors);
    match errors.into_iter().next() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

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

/// Whether an id has the `<extension>.<local>` shape every extension contribution is namespaced
/// with. Also the test for "this id belongs to some extension, loaded or not", which is what lets a
/// sidebar panel keep a placement for a tab whose extension is not here right now.
pub(crate) fn is_extension_scoped_id(id: &str) -> bool {
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

pub(super) fn validate_navigation_target(
    raw: ExtensionNavigationTargetFile,
    extension_id: &str,
    seen: &mut HashSet<String>,
    info: &mut ExtensionInfo,
    targets: &mut Vec<crate::config::NavigationTargetContribution>,
) {
    let Some(name) = raw
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
    else {
        info.errors
            .push("navigation target missing required field `name`".to_string());
        return;
    };
    if !valid_command_segment(&name) {
        info.errors.push(format!(
            "navigation target name `{name}` must match [a-z0-9_-]+"
        ));
        return;
    }
    if !seen.insert(name.clone()) {
        info.errors.push(format!(
            "duplicate navigation target `{extension_id}.{name}`"
        ));
        return;
    }

    let target_id = format!("{extension_id}.{name}");
    let programs = validated_navigation_programs(raw.programs, &target_id, &mut info.errors);
    if programs.is_empty() {
        info.errors.push(format!(
            "navigation target `{target_id}` requires at least one program"
        ));
        return;
    }

    info.navigation_targets
        .push(ExtensionNavigationTargetDiagnostic {
            name: name.clone(),
            programs: programs.clone(),
        });
    targets.push(crate::config::NavigationTargetContribution {
        name,
        programs,
        extension_id: extension_id.to_string(),
    });
}

fn validated_navigation_programs(
    raw: Option<Vec<String>>,
    target_id: &str,
    errors: &mut Vec<String>,
) -> Vec<String> {
    let mut programs = Vec::new();
    let mut seen = HashSet::new();
    for program in raw.unwrap_or_default() {
        if let Some(program) = validated_navigation_program(program, target_id, errors)
            && seen.insert(program.to_ascii_lowercase())
        {
            programs.push(program);
        }
    }
    programs
}

fn validated_navigation_program(
    program: String,
    target_id: &str,
    errors: &mut Vec<String>,
) -> Option<String> {
    let program = program.trim();
    if program.is_empty() {
        errors.push(format!(
            "navigation target `{target_id}` contains an empty program"
        ));
        return None;
    }
    if !is_executable_basename(program) {
        errors.push(format!(
            "navigation target `{target_id}` program `{program}` must be an executable basename"
        ));
        return None;
    }
    Some(program.to_string())
}

fn is_executable_basename(program: &str) -> bool {
    program
        .chars()
        .all(|character| !character.is_control() && !matches!(character, '/' | '\\'))
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
    // A chord that cannot be parsed is the extension's mistake and is reported, but it does not
    // invalidate the extension: the command still works from the palette, which is where most
    // people reach it anyway.
    let default_key = clean_optional(raw.key).and_then(|key| {
        if key.split_whitespace().all(|step| {
            tui_lipan::input::KeyBinding::from_str(step).is_ok_and(|step| step.step_count() == 1)
        }) && !key.split_whitespace().collect::<Vec<_>>().is_empty()
        {
            Some(key)
        } else {
            info.errors.push(format!(
                "extension command `{id}` has an unparsable `key` (write the steps after the leader prefix, such as \"g b\")"
            ));
            None
        }
    });
    if let Some(action) = action {
        commands.push(NamedCommand {
            id,
            label,
            action,
            category: category.to_string(),
            env: env.to_vec(),
            default_key,
        });
    }
}

/// Build one `[[sidebar_tabs]]` entry into a namespaced tab.
///
/// Recoverable problems inside the declaration (a clamped interval, one unusable entry) are dropped
/// rather than reported: `ExtensionInfo` carries errors, and an error here would take the whole
/// extension down over a detail the author can read in the docs. Anything that stops the tab
/// existing is an error, matching how a malformed command or service is treated.
pub(super) fn validate_sidebar_tab(
    raw: ExtensionSidebarTabFile,
    extension_id: &str,
    directory: &str,
    env: &[(String, String)],
    seen: &mut HashSet<String>,
    info: &mut ExtensionInfo,
    tabs: &mut Vec<SidebarTab>,
) {
    let Some(local_name) = raw
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
    else {
        info.errors
            .push("sidebar tab missing required field `name`".to_string());
        return;
    };
    if !valid_command_segment(&local_name) {
        info.errors.push(format!(
            "sidebar tab name `{local_name}` must match [a-z0-9_-]+"
        ));
        return;
    }
    let id = format!("{extension_id}.{local_name}");
    if !seen.insert(local_name) {
        info.errors.push(format!("duplicate sidebar tab `{id}`"));
        return;
    }
    // A tab's strings are shell command lines and pane input, not argv, so `{extension_dir}` is
    // substituted here rather than resolved as a path. Without it a contributed tab has no way to
    // name its own program: the tab is not a command or a service, so it never had one.
    let expand = |value: String| value.replace("{extension_dir}", directory);
    let parts = super::super::sidebar::CustomTabParts {
        label: raw.label.unwrap_or_default(),
        entries: raw.entries.map(|entries| {
            entries
                .into_iter()
                .map(|mut entry| {
                    entry.run = entry.run.map(&expand);
                    entry.send = entry.send.map(&expand);
                    entry.popup = entry.popup.map(&expand);
                    entry
                })
                .collect()
        }),
        command: raw.command.map(&expand),
        interval: raw.interval,
        on_click: raw.on_click.map(|mut action| {
            action.run = action.run.map(&expand);
            action.send = action.send.map(&expand);
            action.popup = action.popup.map(&expand);
            action.exec = action.exec.map(&expand);
            action
        }),
        group_prefix: raw.group_prefix,
        env: env.to_vec(),
    };
    let mut advisories = Vec::new();
    match super::super::sidebar::build_custom_tab(SidebarTabId::new(&id), parts, &mut advisories) {
        Ok(tab) => {
            info.sidebar_tabs.push(id);
            tabs.push(tab);
        }
        Err(error) => info.errors.push(error),
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
