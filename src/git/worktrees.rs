//! Git worktree lifecycle on the session server's host.
//!
//! Paths stay host-native here. A remote client must treat the returned strings as opaque.

use std::ffi::OsString;
use std::path::Path;

use super::command::{self, WORKTREE_LIST_TIMEOUT, WORKTREE_MUTATION_TIMEOUT};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
    pub prunable: bool,
    pub linked: bool,
    pub locked: bool,
}

/// List registered checkouts. Git's porcelain format puts the primary worktree first.
pub fn list(cwd: &Path) -> Result<Vec<WorktreeInfo>, String> {
    let output = command::checked(
        cwd,
        &argv(&["worktree", "list", "--porcelain", "-z"]),
        WORKTREE_LIST_TIMEOUT,
    )?;
    parse_porcelain_z(&output)
}

/// Check out an existing local branch, or create one from `base` before checking it out.
pub fn create(cwd: &Path, branch: &str, base: &str, path: &Path) -> Result<WorktreeInfo, String> {
    absolute_path(path)?;
    if branch.is_empty() || branch.starts_with('-') {
        return Err("invalid worktree branch".to_string());
    }
    if base.is_empty() {
        return Err("worktree base cannot be empty".to_string());
    }
    command::checked(
        cwd,
        &["check-ref-format".into(), "--branch".into(), branch.into()],
        WORKTREE_LIST_TIMEOUT,
    )?;

    let refname = format!("refs/heads/{branch}");
    let exists = command::run(
        cwd,
        &[
            "show-ref".into(),
            "--verify".into(),
            "--quiet".into(),
            refname.into(),
        ],
        WORKTREE_LIST_TIMEOUT,
    )?;
    let existing_branch = match exists.status {
        Some(0) => true,
        Some(1) => false,
        _ => return Err(git_error(&exists)),
    };

    let mut args = argv(&["worktree", "add"]);
    if !existing_branch {
        // Check the revision before creating the branch, so bad input has no side effects.
        let revision = format!("{base}^{{commit}}");
        command::checked(
            cwd,
            &[
                "rev-parse".into(),
                "--verify".into(),
                "--end-of-options".into(),
                revision.into(),
            ],
            WORKTREE_LIST_TIMEOUT,
        )?;
        args.push("-b".into());
        args.push(branch.into());
    }
    args.push("--".into());
    args.push(path.as_os_str().to_os_string());
    args.push(if existing_branch { branch } else { base }.into());
    command::checked(cwd, &args, WORKTREE_MUTATION_TIMEOUT)?;

    let requested = path.canonicalize().map_err(|err| err.to_string())?;
    list(cwd)?
        .into_iter()
        .find(|tree| Path::new(&tree.path) == requested)
        .ok_or_else(|| "created worktree was not found in Git's worktree list".to_string())
}

/// Remove a linked checkout. `force` only relaxes Git's dirty-checkout guard; it never removes a
/// Rozi session or deletes a branch. Session-origin protection belongs at the RPC boundary.
pub fn remove(cwd: &Path, path: &Path, force: bool) -> Result<(), String> {
    absolute_path(path)?;
    let requested = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let worktree = list(cwd)?
        .into_iter()
        .find(|tree| {
            let listed = Path::new(&tree.path);
            listed == requested || listed.canonicalize().is_ok_and(|path| path == requested)
        })
        .ok_or_else(|| "path is not a registered Git worktree".to_string())?;
    if !worktree.linked || worktree.bare {
        return Err("cannot remove the primary worktree".to_string());
    }
    if worktree.locked {
        return Err("cannot remove a locked worktree".to_string());
    }
    let mut args = argv(&["worktree", "remove"]);
    if force {
        args.push("--force".into());
    }
    args.push("--".into());
    args.push(worktree.path.into());
    command::checked(cwd, &args, WORKTREE_MUTATION_TIMEOUT)?;
    Ok(())
}

fn absolute_path(path: &Path) -> Result<(), String> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err("worktree path must be absolute on the session host".to_string())
    }
}

fn argv(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn git_error(output: &crate::platform::command::CommandOutput) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        format!("git exited with status {:?}", output.status)
    } else {
        stderr
    }
}

fn parse_porcelain_z(output: &[u8]) -> Result<Vec<WorktreeInfo>, String> {
    let mut trees = Vec::new();
    let mut current: Option<WorktreeInfo> = None;
    for field in output.split(|byte| *byte == 0) {
        if field.is_empty() {
            if let Some(tree) = current.take() {
                trees.push(tree);
            }
            continue;
        }
        let field = std::str::from_utf8(field)
            .map_err(|_| "Git returned a worktree path or ref that is not UTF-8".to_string())?;
        if let Some(path) = field.strip_prefix("worktree ") {
            if path.is_empty() || current.is_some() {
                return Err("invalid Git worktree listing".to_string());
            }
            current = Some(WorktreeInfo {
                path: path.to_string(),
                branch: None,
                detached: false,
                bare: false,
                prunable: false,
                linked: !trees.is_empty(),
                locked: false,
            });
            continue;
        }
        let tree = current
            .as_mut()
            .ok_or_else(|| "invalid Git worktree listing".to_string())?;
        if let Some(branch) = field.strip_prefix("branch refs/heads/") {
            tree.branch = Some(branch.to_string());
        } else if field == "detached" {
            tree.detached = true;
        } else if field == "bare" {
            tree.bare = true;
        } else if field == "prunable" || field.starts_with("prunable ") {
            tree.prunable = true;
        } else if field == "locked" || field.starts_with("locked ") {
            tree.locked = true;
        }
    }
    if let Some(tree) = current {
        trees.push(tree);
    }
    Ok(trees)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn parses_primary_linked_detached_and_prunable_worktrees() {
        let raw = b"worktree C:\\code\\repo\0HEAD abc\0branch refs/heads/main\0\0worktree C:\\code\\feature\0HEAD def\0detached\0\0worktree C:\\code\\missing\0HEAD def\0prunable gitdir file points to non-existent location\0locked maintenance\0\0";
        let trees = parse_porcelain_z(raw).unwrap();
        assert_eq!(trees.len(), 3);
        assert_eq!(trees[0].path, "C:\\code\\repo");
        assert_eq!(trees[0].branch.as_deref(), Some("main"));
        assert!(!trees[0].linked);
        assert!(trees[1].linked && trees[1].detached);
        assert!(trees[2].prunable && trees[2].locked);
    }

    fn git(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn creates_existing_and_new_branches_and_removes_dirty_checkout_only_when_forced() {
        if !crate::platform::command::program_exists("git") {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("source repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "initial",
            ],
        );
        git(&repo, &["branch", "existing"]);

        let existing = temp.path().join("existing checkout");
        let tree = create(&repo, "existing", "HEAD", &existing).unwrap();
        assert_eq!(tree.branch.as_deref(), Some("existing"));
        assert!(tree.linked);

        let fresh = temp.path().join("fresh checkout");
        let tree = create(&repo, "feat/new", "HEAD", &fresh).unwrap();
        assert_eq!(tree.branch.as_deref(), Some("feat/new"));
        std::fs::write(fresh.join("scratch.txt"), b"unsaved").unwrap();
        assert!(remove(&repo, &fresh, false).is_err());
        assert!(fresh.exists());
        assert!(remove(&repo, &repo, true).is_err());
        remove(&repo, &fresh, true).unwrap();
        assert!(!fresh.exists());
        assert!(
            list(&repo)
                .unwrap()
                .iter()
                .all(|tree| tree.path != fresh.to_string_lossy())
        );
        git(&repo, &["show-ref", "--verify", "refs/heads/feat/new"]);
        remove(&repo, &existing, false).unwrap();
    }
}
