use crate::platform::process::{ForegroundJob, ForegroundProcess};
use crate::session::protocol::AgentKind;

use super::providers::catalog::{kind_from_name, kind_from_path, normalized_name, path_basename};

/// Identify the most authoritative agent process in one foreground process group.
///
/// Process traversal and wrapper scoring are generic observation policy. Tool names and package
/// markers live in the provider catalog instead of leaking into this layer.
pub(super) fn identify_job(job: &ForegroundJob) -> Option<AgentKind> {
    for process in &job.processes {
        if let Some(kind) = process.agent_hint.as_deref().and_then(kind_from_name) {
            return Some(kind);
        }
    }

    job.processes
        .iter()
        .filter_map(|process| {
            let (kind, wrapped) = identify_process(process)?;
            let leader = process.pid == job.process_group_id;
            Some(((leader as u8) * 2 + wrapped as u8, kind))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, kind)| kind)
}

pub(super) fn identify_process(process: &ForegroundProcess) -> Option<(AgentKind, bool)> {
    if let Some(kind) = process
        .executable
        .as_deref()
        .and_then(|path| kind_from_name(path_basename(path)))
    {
        return Some((kind, false));
    }
    let argv0 = process
        .argv
        .first()
        .map(String::as_str)
        .unwrap_or(&process.name);
    if let Some(kind) =
        kind_from_name(path_basename(argv0)).or_else(|| kind_from_name(&process.name))
    {
        return Some((kind, false));
    }
    identify_wrapped(&process.argv).map(|kind| (kind, true))
}

fn identify_wrapped(argv: &[String]) -> Option<AgentKind> {
    let runtime = argv
        .first()
        .map(|value| normalized_name(path_basename(value)))?;
    let mut candidates: Vec<&str> = Vec::new();
    match runtime.as_str() {
        "node" | "nodejs" | "bun" | "deno" | "python" | "python3" | "ruby" => {
            candidates.extend(
                argv.iter()
                    .skip(1)
                    .filter(|arg| !arg.starts_with('-'))
                    .map(String::as_str),
            );
        }
        "sh" | "bash" | "zsh" | "fish" | "cmd" | "powershell" | "pwsh" => {
            for command in argv.iter().skip(1).filter(|arg| !arg.starts_with('-')) {
                candidates.extend(command.split_whitespace());
            }
        }
        "env" | "npm" | "npx" | "pnpm" | "pnpx" | "yarn" | "bunx" | "uv" | "uvx" => {
            candidates.extend(
                argv.iter()
                    .skip(1)
                    .filter(|arg| !arg.starts_with('-'))
                    .map(String::as_str),
            );
        }
        _ => candidates.extend(argv.iter().map(String::as_str)),
    }
    candidates.into_iter().find_map(kind_from_path)
}
