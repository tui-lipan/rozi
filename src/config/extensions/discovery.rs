use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::super::commands::valid_command_segment;
use super::{DiscoveredExtension, ExtensionStatus};

pub(super) fn directories(root: &Path) -> (Vec<PathBuf>, Vec<String>) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (Vec::new(), Vec::new());
        }
        Err(error) => {
            return (
                Vec::new(),
                vec![format!(
                    "extensions directory read failed for {}: {error}",
                    root.display()
                )],
            );
        }
    };
    let mut directories = Vec::new();
    let mut errors = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => {
                if entry.file_name() == ".rozi" {
                    continue;
                }
                match entry.file_type() {
                    Ok(kind) if kind.is_dir() || kind.is_symlink() => {
                        directories.push(entry.path());
                    }
                    Ok(_) => {}
                    Err(error) => errors.push(format!(
                        "extension entry {} could not be inspected: {error}",
                        entry.path().display()
                    )),
                }
            }
            Err(error) => errors.push(format!(
                "extension directory entry could not be read: {error}"
            )),
        }
    }
    directories.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
    (directories, errors)
}

pub(super) fn mark_duplicate_ids(extensions: &mut [DiscoveredExtension]) {
    let mut by_id: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, extension) in extensions.iter().enumerate() {
        if let Some(id) = extension.info.id.clone()
            && valid_command_segment(&id)
        {
            by_id.entry(id).or_default().push(index);
        }
    }
    for (id, indices) in by_id.into_iter().filter(|(_, indices)| indices.len() > 1) {
        let paths = indices
            .iter()
            .map(|index| extensions[*index].info.path.clone())
            .collect::<Vec<_>>()
            .join(", ");
        for index in indices {
            let extension = &mut extensions[index];
            extension.info.status = ExtensionStatus::Duplicate;
            extension.info.enabled = false;
            extension.info.errors.insert(
                0,
                format!("duplicate extension id `{id}` is declared by: {paths}"),
            );
            extension.commands.clear();
            extension.services.clear();
            extension.agents.clear();
            extension.sidebar_tabs.clear();
            extension.navigation_targets.clear();
        }
    }
}
