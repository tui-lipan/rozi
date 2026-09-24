//! Records explicit skill installations so an update can refresh only copies it owns.

use super::{ClaudeEntry, ClaudeInstall, SKILL_MD, SkillPaths, install_claude, replace_file};
use crate::platform::fs_security;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const REGISTRY_FILE: &str = "skill-installs.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Record {
    root: PathBuf,
    content_hash: String,
    installed_from_rozi_version: String,
    #[serde(default)]
    claude_content_hash: Option<String>,
    #[serde(default)]
    pending_content_hash: Option<String>,
    #[serde(default)]
    pending_claude_content_hash: Option<String>,
}

impl Record {
    fn owns_canonical(&self, content_hash: &str) -> bool {
        self.content_hash == content_hash
            || self.pending_content_hash.as_deref() == Some(content_hash)
    }

    fn owns_claude(&self, content_hash: &str) -> bool {
        self.claude_content_hash
            .as_deref()
            .unwrap_or(&self.content_hash)
            == content_hash
            || self.pending_claude_content_hash.as_deref() == Some(content_hash)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Registry {
    installs: Vec<Record>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedInstall {
    pub skill_file: PathBuf,
    pub claude: ClaudeInstall,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefreshReport {
    pub refreshed: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub missing: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
    pub untracked: Vec<PathBuf>,
}

fn hash(content: &[u8]) -> String {
    format!("{:x}", Sha256::digest(content))
}

fn registry_path(state_dir: &Path) -> PathBuf {
    state_dir.join(REGISTRY_FILE)
}

fn read_registry(state_dir: &Path) -> Result<Registry, String> {
    let path = registry_path(state_dir);
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Registry::default()),
        Err(error) => return Err(format!("could not inspect {}: {error}", path.display())),
    };
    if !meta.is_file() || super::is_link(&meta) {
        return Err(format!("refusing to read {}", path.display()));
    }
    let bytes =
        fs::read(&path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("invalid {}: {error}", path.display()))
}

fn write_registry(state_dir: &Path, registry: &Registry) -> Result<(), String> {
    fs_security::ensure_private_dir(state_dir)
        .map_err(|error| format!("could not secure {}: {error}", state_dir.display()))?;
    let path = registry_path(state_dir);
    if let Ok(meta) = fs::symlink_metadata(&path)
        && (!meta.is_file() || super::is_link(&meta))
    {
        return Err(format!("refusing to overwrite {}", path.display()));
    }
    let data = serde_json::to_string_pretty(registry).map_err(|error| error.to_string())?;
    replace_file(&path, &(data + "\n"))
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

/// Explicit installation uses the same ownership check as automatic refresh. `force` claims an
/// untracked or modified file, while the default only updates an unchanged managed copy.
pub fn install_managed(
    paths: &SkillPaths,
    state_dir: &Path,
    claude_available: bool,
    force: bool,
) -> Result<ManagedInstall, String> {
    install_managed_with(paths, state_dir, claude_available, force, install_claude)
}

fn install_managed_with(
    paths: &SkillPaths,
    state_dir: &Path,
    claude_available: bool,
    force: bool,
    install_compat: impl FnOnce(&SkillPaths) -> Result<ClaudeInstall, String>,
) -> Result<ManagedInstall, String> {
    let mut registry = read_registry(state_dir)?;
    let record = registry
        .installs
        .iter()
        .find(|record| record.root == paths.scope_root);
    let existing = match fs::symlink_metadata(&paths.skill_file) {
        Ok(meta) if meta.is_file() && !super::is_link(&meta) => {
            Some(fs::read(&paths.skill_file).map_err(|error| {
                format!("could not read {}: {error}", paths.skill_file.display())
            })?)
        }
        Ok(_) => {
            return Err(format!(
                "refusing to overwrite {}",
                paths.skill_file.display()
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(format!(
                "could not inspect {}: {error}",
                paths.skill_file.display()
            ));
        }
    };
    if let Some(bytes) = &existing
        && bytes != SKILL_MD.as_bytes()
        && !force
        && record.is_none_or(|record| !record.owns_canonical(&hash(bytes)))
    {
        return Err(format!(
            "{} is outdated or locally modified; run `rozi skill install --force{}` to replace it",
            paths.skill_file.display(),
            if paths.scope_root == super::resolve_home()? {
                " --global"
            } else {
                ""
            }
        ));
    }
    super::install_canonical(paths)?;
    let copy_modified = matches!(
        super::inspect_claude(paths),
        ClaudeEntry::Copy { ours: true }
    ) && fs::read(paths.claude_path.join("SKILL.md"))
        .ok()
        .is_some_and(|bytes| {
            bytes != SKILL_MD.as_bytes()
                && !force
                && record.is_none_or(|record| !record.owns_claude(&hash(&bytes)))
        });
    let claude = if claude_available && copy_modified {
        ClaudeInstall::Failed {
            path: paths.claude_path.clone(),
        }
    } else if claude_available {
        install_compat(paths).unwrap_or(ClaudeInstall::Failed {
            path: paths.claude_path.clone(),
        })
    } else {
        ClaudeInstall::Skipped
    };
    let previous_claude_hash = record.map(|record| {
        record
            .claude_content_hash
            .clone()
            .unwrap_or_else(|| record.content_hash.clone())
    });
    let new_record = Record {
        root: paths.scope_root.clone(),
        content_hash: hash(SKILL_MD.as_bytes()),
        installed_from_rozi_version: env!("CARGO_PKG_VERSION").to_string(),
        claude_content_hash: match &claude {
            ClaudeInstall::Copied { .. } => Some(hash(SKILL_MD.as_bytes())),
            ClaudeInstall::Failed { .. } | ClaudeInstall::Skipped => previous_claude_hash,
            ClaudeInstall::Linked { .. } => None,
        },
        pending_content_hash: None,
        pending_claude_content_hash: match &claude {
            ClaudeInstall::Failed { .. } | ClaudeInstall::Skipped => {
                record.and_then(|record| record.pending_claude_content_hash.clone())
            }
            ClaudeInstall::Linked { .. } | ClaudeInstall::Copied { .. } => None,
        },
    };
    registry
        .installs
        .retain(|record| record.root != paths.scope_root);
    registry.installs.push(new_record);
    write_registry(state_dir, &registry)?;
    Ok(ManagedInstall {
        skill_file: paths.skill_file.clone(),
        claude,
    })
}

pub fn forget_install(paths: &SkillPaths, state_dir: &Path) -> Result<(), String> {
    let mut registry = read_registry(state_dir)?;
    registry
        .installs
        .retain(|record| record.root != paths.scope_root);
    if registry.installs.is_empty() && !registry_path(state_dir).exists() {
        return Ok(());
    }
    write_registry(state_dir, &registry)
}

/// Refresh registered copies using the skill emitted by the newly activated binary.
pub fn refresh_managed(
    state_dir: &Path,
    new_skill: &str,
    version: &str,
    observed_roots: &[PathBuf],
) -> Result<RefreshReport, String> {
    refresh_managed_with(state_dir, new_skill, version, observed_roots, replace_file)
}

fn refresh_managed_with(
    state_dir: &Path,
    new_skill: &str,
    version: &str,
    observed_roots: &[PathBuf],
    mut replace: impl FnMut(&Path, &str) -> io::Result<()>,
) -> Result<RefreshReport, String> {
    if !new_skill.starts_with("---\nname: rozi\n") {
        return Err("updated binary returned an invalid Rozi skill".to_string());
    }
    let mut registry = read_registry(state_dir)?;
    let mut report = RefreshReport::default();
    let target_hash = hash(new_skill.as_bytes());
    for index in 0..registry.installs.len() {
        let paths = SkillPaths::at(registry.installs[index].root.clone());
        let canonical = managed_file_hash(
            &paths.skill_file,
            |value| registry.installs[index].owns_canonical(value),
            &mut report,
        );
        let copy_path = paths.claude_path.join("SKILL.md");
        let copy = if matches!(
            super::inspect_claude(&paths),
            ClaudeEntry::Copy { ours: true }
        ) {
            managed_file_hash(
                &copy_path,
                |value| registry.installs[index].owns_claude(value),
                &mut report,
            )
        } else {
            None
        };
        if canonical.is_none() && copy.is_none() {
            continue;
        }

        // Commit the intended generation before touching either file. If either file write or the
        // final registry write fails, the persisted record still recognizes both generations.
        {
            let record = &mut registry.installs[index];
            if let Some(observed) = &canonical {
                record.content_hash = observed.clone();
                record.pending_content_hash =
                    (observed != &target_hash).then(|| target_hash.clone());
            }
            if let Some(observed) = &copy {
                record.claude_content_hash = Some(observed.clone());
                record.pending_claude_content_hash =
                    (observed != &target_hash).then(|| target_hash.clone());
            }
        }
        write_registry(state_dir, &registry)?;

        let record = &mut registry.installs[index];
        if canonical
            .as_ref()
            .is_some_and(|observed| observed != &target_hash)
        {
            match replace(&paths.skill_file, new_skill) {
                Ok(()) => {
                    record.content_hash = target_hash.clone();
                    record.pending_content_hash = None;
                    report.refreshed.push(paths.skill_file);
                }
                Err(error) => report.failed.push((paths.skill_file, error.to_string())),
            }
        }
        if copy
            .as_ref()
            .is_some_and(|observed| observed != &target_hash)
        {
            match replace(&copy_path, new_skill) {
                Ok(()) => {
                    record.claude_content_hash = Some(target_hash.clone());
                    record.pending_claude_content_hash = None;
                }
                Err(error) => report.failed.push((copy_path, error.to_string())),
            }
        }
        if record.content_hash == target_hash {
            record.installed_from_rozi_version = version.to_string();
        }
        write_registry(state_dir, &registry)?;
    }
    for root in observed_roots {
        if !registry.installs.iter().any(|record| &record.root == root) {
            let skill_file = SkillPaths::at(root.clone()).skill_file;
            if skill_file.exists() {
                report.untracked.push(skill_file);
            }
        }
    }
    Ok(report)
}

fn managed_file_hash(
    path: &Path,
    owned: impl FnOnce(&str) -> bool,
    report: &mut RefreshReport,
) -> Option<String> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            report.missing.push(path.to_path_buf());
            return None;
        }
        Err(error) => {
            report.failed.push((path.to_path_buf(), error.to_string()));
            return None;
        }
    };
    if !meta.is_file() || super::is_link(&meta) {
        report.modified.push(path.to_path_buf());
        return None;
    }
    match fs::read(path) {
        Ok(bytes) => {
            let observed = hash(&bytes);
            if owned(&observed) {
                Some(observed)
            } else {
                report.modified.push(path.to_path_buf());
                None
            }
        }
        Err(error) => {
            report.failed.push((path.to_path_buf(), error.to_string()));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "rozi-managed-skill-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn refreshes_only_unchanged_recorded_copies() {
        let scratch = Scratch::new();
        let state = scratch.0.join("state");
        let clean = SkillPaths::project(scratch.0.join("clean"));
        let modified = SkillPaths::project(scratch.0.join("modified"));
        install_managed(&clean, &state, false, false).unwrap();
        install_managed(&modified, &state, false, false).unwrap();
        fs::write(&modified.skill_file, "local edits").unwrap();
        let next = "---\nname: rozi\n---\nnew instructions\n";
        let report = refresh_managed(&state, next, "9.0.0", &[]).unwrap();
        assert_eq!(report.refreshed, vec![clean.skill_file.clone()]);
        assert_eq!(report.modified, vec![modified.skill_file.clone()]);
        assert_eq!(fs::read_to_string(&clean.skill_file).unwrap(), next);
        assert_eq!(
            fs::read_to_string(&modified.skill_file).unwrap(),
            "local edits"
        );
        assert!(
            refresh_managed(&state, next, "9.0.0", &[])
                .unwrap()
                .refreshed
                .is_empty()
        );
    }

    #[test]
    fn explicit_install_requires_force_for_untracked_or_modified_content() {
        let scratch = Scratch::new();
        let state = scratch.0.join("state");
        let paths = SkillPaths::project(scratch.0.join("project"));
        fs::create_dir_all(&paths.canonical_dir).unwrap();
        fs::write(&paths.skill_file, "old or user-owned").unwrap();
        assert!(install_managed(&paths, &state, false, false).is_err());
        assert_eq!(
            fs::read_to_string(&paths.skill_file).unwrap(),
            "old or user-owned"
        );
        install_managed(&paths, &state, false, true).unwrap();
        fs::write(&paths.skill_file, "local edits").unwrap();
        assert!(install_managed(&paths, &state, false, false).is_err());
        install_managed(&paths, &state, false, true).unwrap();
        assert_eq!(fs::read_to_string(&paths.skill_file).unwrap(), SKILL_MD);
    }

    #[test]
    fn old_untracked_copy_is_reported_without_replacement() {
        let scratch = Scratch::new();
        let state = scratch.0.join("state");
        let paths = SkillPaths::project(scratch.0.join("project"));
        fs::create_dir_all(&paths.canonical_dir).unwrap();
        fs::write(&paths.skill_file, "old").unwrap();
        let report = refresh_managed(
            &state,
            "---\nname: rozi\n---\nnew\n",
            "9.0.0",
            std::slice::from_ref(&paths.scope_root),
        )
        .unwrap();
        assert_eq!(report.untracked, vec![paths.skill_file.clone()]);
        assert_eq!(fs::read_to_string(&paths.skill_file).unwrap(), "old");
    }

    #[test]
    fn failed_claude_copy_is_retried_after_canonical_refresh() {
        let scratch = Scratch::new();
        let state = scratch.0.join("state");
        let paths = SkillPaths::project(scratch.0.join("project"));
        install_managed(&paths, &state, false, false).unwrap();
        fs::create_dir_all(&paths.claude_path).unwrap();
        let copy = paths.claude_path.join("SKILL.md");
        fs::write(&copy, SKILL_MD).unwrap();
        let next = "---\nname: rozi\n---\nnew instructions\n";
        let failed = refresh_managed_with(&state, next, "9.0.0", &[], |path, contents| {
            if path == copy {
                Err(io::Error::other("injected copy failure"))
            } else {
                replace_file(path, contents)
            }
        })
        .unwrap();
        assert_eq!(failed.failed.len(), 1);
        assert_eq!(fs::read_to_string(&paths.skill_file).unwrap(), next);
        assert_eq!(fs::read_to_string(&copy).unwrap(), SKILL_MD);
        let retried = refresh_managed(&state, next, "9.0.0", &[]).unwrap();
        assert!(retried.modified.is_empty());
        assert!(retried.failed.is_empty());
        assert_eq!(fs::read_to_string(&copy).unwrap(), next);
    }

    #[test]
    fn failed_claude_copy_during_explicit_install_keeps_previous_ownership() {
        let scratch = Scratch::new();
        let state = scratch.0.join("state");
        let paths = SkillPaths::project(scratch.0.join("project"));
        install_managed(&paths, &state, false, false).unwrap();
        let old = "---\nname: rozi\n---\nold instructions\n";
        fs::write(&paths.skill_file, old).unwrap();
        fs::create_dir_all(&paths.claude_path).unwrap();
        let copy = paths.claude_path.join("SKILL.md");
        fs::write(&copy, old).unwrap();
        let mut registry = read_registry(&state).unwrap();
        registry.installs[0].content_hash = hash(old.as_bytes());
        registry.installs[0].claude_content_hash = Some(hash(old.as_bytes()));
        write_registry(&state, &registry).unwrap();

        let first = install_managed_with(&paths, &state, true, false, |_| {
            Err("injected compatibility write failure".to_string())
        })
        .unwrap();
        assert!(matches!(first.claude, ClaudeInstall::Failed { .. }));
        assert_eq!(fs::read_to_string(&paths.skill_file).unwrap(), SKILL_MD);
        assert_eq!(fs::read_to_string(&copy).unwrap(), old);

        let retry = install_managed(&paths, &state, true, false).unwrap();
        assert!(matches!(retry.claude, ClaudeInstall::Copied { .. }));
        assert_eq!(fs::read_to_string(&copy).unwrap(), SKILL_MD);
    }

    #[test]
    fn failed_final_registry_write_keeps_new_file_owned() {
        let scratch = Scratch::new();
        let state = scratch.0.join("state");
        let paths = SkillPaths::project(scratch.0.join("project"));
        install_managed(&paths, &state, false, false).unwrap();
        let blocker = state.join("SKILL.md.rozi-tmp");
        let next = "---\nname: rozi\n---\nnew instructions\n";
        assert!(
            refresh_managed_with(&state, next, "9.0.0", &[], |path, contents| {
                replace_file(path, contents)?;
                fs::create_dir(&blocker)
            })
            .is_err()
        );
        assert_eq!(fs::read_to_string(&paths.skill_file).unwrap(), next);
        fs::remove_dir(&blocker).unwrap();
        let retry = refresh_managed(&state, next, "9.0.0", &[]).unwrap();
        assert!(retry.modified.is_empty());
    }

    #[test]
    fn managed_skill_can_move_back_to_previous_version() {
        let scratch = Scratch::new();
        let state = scratch.0.join("state");
        let paths = SkillPaths::project(scratch.0.join("project"));
        install_managed(&paths, &state, false, false).unwrap();
        let newer = "---\nname: rozi\n---\nversion two\n";
        refresh_managed(&state, newer, "2.0.0", &[]).unwrap();
        let rolled_back = refresh_managed(&state, SKILL_MD, "1.0.0", &[]).unwrap();
        assert_eq!(rolled_back.refreshed, vec![paths.skill_file.clone()]);
        assert_eq!(fs::read_to_string(&paths.skill_file).unwrap(), SKILL_MD);
    }
}
