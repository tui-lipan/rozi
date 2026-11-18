use std::collections::HashSet;

use crate::platform::process::{AgentProcessSet, ForegroundProcess, ProcessTopology};

use super::catalog::AgentCatalog;
use super::definition::{AgentDefinition, normalized_name};

/// Identify the most authoritative agent process among a pane's processes, and which process it
/// was.
///
/// Process traversal and wrapper scoring are generic observation policy. Tool names and package
/// markers live in the catalog's definitions instead of leaking into this layer.
///
/// A hint decides *which* agent this is, but the process carrying it is often not that agent: it is
/// set on the set's first process, which is the shell or wrapper that launches it. So the process
/// named is the agent's own when one in the set is recognizable as it, and the hinted one only
/// stands in until then - otherwise a wrapper that outlives the launch would be the agent forever.
///
/// `shell_program` is what the pane's shell says it is running. In a descendant tree, which can
/// hold background jobs beside what holds the terminal, it narrows the choice to that agent when it
/// names one; see [`topmost_match`].
pub(super) fn identify_set<'a, 's>(
    catalog: &'a AgentCatalog,
    set: &'s AgentProcessSet,
    shell_program: Option<&str>,
) -> Option<(&'a AgentDefinition, &'s ForegroundProcess)> {
    let pick = |accept: &dyn Fn(&AgentDefinition) -> bool| match set.topology {
        ProcessTopology::ForegroundGroup => best_match(catalog, set, accept),
        ProcessTopology::DescendantTree => topmost_match(catalog, set, accept),
    };
    for observed in &set.processes {
        let process = &observed.process;
        if let Some(definition) = process
            .agent_hint
            .as_deref()
            .and_then(|hint| catalog.by_name(hint))
        {
            let itself = pick(&|candidate| candidate.identity.id == definition.identity.id);
            return Some((definition, itself.map_or(process, |(_, process)| process)));
        }
    }
    let shell_agent = (set.topology == ProcessTopology::DescendantTree)
        .then(|| shell_program.and_then(|program| catalog.by_name(program)))
        .flatten();
    match shell_agent {
        Some(definition) => pick(&|candidate| candidate.identity.id == definition.identity.id),
        // The agent a cut-off tree leaves out could be the one in use, so its prefix names only
        // an agent the shell or a hint already said which.
        None if !set.complete => None,
        None => pick(&|_| true),
    }
}

/// The group's most authoritative process recognizable by itself as an agent `accept` allows.
fn best_match<'a, 's>(
    catalog: &'a AgentCatalog,
    set: &'s AgentProcessSet,
    accept: &dyn Fn(&AgentDefinition) -> bool,
) -> Option<(&'a AgentDefinition, &'s ForegroundProcess)> {
    set.processes
        .iter()
        .map(|observed| &observed.process)
        .filter_map(|process| {
            let (definition, wrapped) = identify_process(catalog, process)?;
            accept(definition).then_some(())?;
            let leader = process.pid == set.root_pid;
            Some(((leader as u8) * 2 + wrapped as u8, (definition, process)))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, named)| named)
}

/// The one recognized process in a descendant tree that every other recognized one descends from.
///
/// An agent's own helpers, or the runtime a wrapper starts, are often recognizable as the same
/// agent; the outermost of them is the stable one to anchor on. Recognized processes in unrelated
/// branches are separate programs, such as a background job beside the one in use, and nothing in
/// the tree says which one holds the terminal. Rather than guess, that names nothing.
fn topmost_match<'a, 's>(
    catalog: &'a AgentCatalog,
    set: &'s AgentProcessSet,
    accept: &dyn Fn(&AgentDefinition) -> bool,
) -> Option<(&'a AgentDefinition, &'s ForegroundProcess)> {
    let recognized: Vec<_> = set
        .processes
        .iter()
        .filter_map(|observed| {
            let (definition, _) = identify_process(catalog, &observed.process)?;
            accept(definition).then_some((definition, observed))
        })
        .collect();
    let pids: HashSet<u32> = recognized
        .iter()
        .map(|(_, observed)| observed.process.pid)
        .collect();
    let mut tops = recognized
        .iter()
        .filter(|(_, observed)| !has_ancestor_in(set, observed.parent_pid, &pids));
    let (definition, observed) = tops.next()?;
    tops.next()
        .is_none()
        .then_some((*definition, &observed.process))
}

fn has_ancestor_in(set: &AgentProcessSet, mut parent: Option<u32>, pids: &HashSet<u32>) -> bool {
    // Bounded by the set's size, so a parent cycle the snapshot reported cannot spin forever.
    for _ in 0..set.processes.len() {
        let Some(pid) = parent else {
            return false;
        };
        if pids.contains(&pid) {
            return true;
        }
        parent = set
            .processes
            .iter()
            .find(|observed| observed.process.pid == pid)
            .and_then(|observed| observed.parent_pid);
    }
    false
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

/// Runtimes and launchers whose operands [`identify_wrapped`] reads for the program they run.
const WRAPPING_RUNTIMES: &[&str] = &[
    "node", "nodejs", "bun", "deno", "python", "python3", "ruby", "env", "npm", "npx", "pnpm",
    "pnpx", "yarn", "bunx", "uv", "uvx",
];

/// Shells whose command operands [`identify_wrapped`] splits into words for the program they run.
const WRAPPING_SHELLS: &[&str] = &["sh", "bash", "zsh", "fish", "cmd", "powershell", "pwsh"];

/// Whether a process with this executable name could be recognized as an agent once its arguments
/// are read: an agent's own name, or a runtime or shell an agent is commonly started through.
pub(super) fn may_be_agent_image(catalog: &AgentCatalog, image: &str) -> bool {
    let name = normalized_name(image);
    catalog.by_name(&name).is_some()
        || WRAPPING_RUNTIMES.contains(&name.as_str())
        || WRAPPING_SHELLS.contains(&name.as_str())
}

fn identify_wrapped<'a>(catalog: &'a AgentCatalog, argv: &[String]) -> Option<&'a AgentDefinition> {
    let runtime = argv.first().map(|value| normalized_name(value))?;
    let mut candidates: Vec<&str> = Vec::new();
    let operands = || {
        argv.iter()
            .skip(1)
            .filter(|arg| !arg.starts_with('-'))
            .map(String::as_str)
    };
    if WRAPPING_SHELLS.contains(&runtime.as_str()) {
        for command in operands() {
            candidates.extend(command.split_whitespace());
        }
    } else if WRAPPING_RUNTIMES.contains(&runtime.as_str()) {
        candidates.extend(operands());
    } else {
        candidates.extend(argv.iter().map(String::as_str));
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
