use crate::platform::process::{ForegroundJob, ForegroundProcess};

use super::catalog::AgentCatalog;
use super::definition::{AgentDefinition, normalized_name};

/// Identify the most authoritative agent process in one foreground process group.
///
/// Process traversal and wrapper scoring are generic observation policy. Tool names and package
/// markers live in the catalog's definitions instead of leaking into this layer.
pub(super) fn identify_job<'a>(
    catalog: &'a AgentCatalog,
    job: &ForegroundJob,
) -> Option<&'a AgentDefinition> {
    for process in &job.processes {
        if let Some(definition) = process
            .agent_hint
            .as_deref()
            .and_then(|hint| catalog.by_name(hint))
        {
            return Some(definition);
        }
    }

    job.processes
        .iter()
        .filter_map(|process| {
            let (definition, wrapped) = identify_process(catalog, process)?;
            let leader = process.pid == job.process_group_id;
            Some(((leader as u8) * 2 + wrapped as u8, definition))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, definition)| definition)
}

pub(super) fn identify_process<'a>(
    catalog: &'a AgentCatalog,
    process: &ForegroundProcess,
) -> Option<(&'a AgentDefinition, bool)> {
    if let Some(definition) = process
        .executable
        .as_deref()
        .and_then(|path| catalog.by_name(path))
    {
        return Some((definition, false));
    }
    let argv0 = process
        .argv
        .first()
        .map(String::as_str)
        .unwrap_or(&process.name);
    if let Some(definition) = catalog
        .by_name(argv0)
        .or_else(|| catalog.by_name(&process.name))
    {
        return Some((definition, false));
    }
    identify_wrapped(catalog, &process.argv).map(|definition| (definition, true))
}

fn identify_wrapped<'a>(catalog: &'a AgentCatalog, argv: &[String]) -> Option<&'a AgentDefinition> {
    let runtime = argv.first().map(|value| normalized_name(value))?;
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
    candidates
        .into_iter()
        .find_map(|candidate| catalog.by_path(candidate))
}

/// Whether an executable of this name is on `PATH`, using the catalog's own name vocabulary so a
/// lookup cannot drift from what pane detection would recognize.
pub(super) fn agent_on_path(catalog: &AgentCatalog, id: &str, names: &[&str]) -> bool {
    names.iter().any(|name| {
        catalog
            .by_name(name)
            .is_some_and(|definition| definition.id() == id)
            && crate::platform::command::program_exists(name)
    })
}
