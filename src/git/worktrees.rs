//! Git worktree lifecycle on the session server's host.
//!
//! Paths stay host-native here. A remote client must treat the returned strings as opaque.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::command::{self, WORKTREE_LIST_TIMEOUT, WORKTREE_MUTATION_TIMEOUT};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
    pub prunable: bool,
    pub linked: bool,
    pub lock: Option<WorktreeLock>,
}

/// A checkout Git has locked against removal and pruning.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorktreeLock {
    /// Git's lock reason, empty when the lock gave none.
    pub reason: String,
    /// The reason names a process (`pid <N>`) that is no longer running on the repository's host.
    /// A reason that names no process is never stale.
    pub stale: bool,
}

impl WorktreeLock {
    fn new(reason: &str) -> Self {
        Self {
            reason: reason.to_string(),
            stale: lock_owner(reason)
                .is_some_and(|(pid, started)| crate::platform::process::process_gone(pid, started)),
        }
    }
}

/// The process a lock reason names: `pid <N>`, optionally followed by `start <T>`, as Claude Code
/// writes `claude session <name> (pid <N> start <T>)`.
fn lock_owner(reason: &str) -> Option<(u32, Option<u64>)> {
    let words: Vec<&str> = reason
        .split(|ch: char| ch.is_whitespace() || matches!(ch, '(' | ')' | ',' | ';'))
        .filter(|word| !word.is_empty())
        .collect();
    let at = words.iter().position(|word| *word == "pid")?;
    let pid = words.get(at + 1)?.parse().ok().filter(|pid| *pid > 0)?;
    let started = (words.get(at + 2) == Some(&"start"))
        .then(|| words.get(at + 3)?.parse().ok())
        .flatten();
    Some((pid, started))
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

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorktreeBranch {
    pub name: String,
    pub checkout: Option<String>,
}

/// Local branches and their existing checkout, if any. No network access is needed.
pub fn branches(cwd: &Path) -> Result<Vec<WorktreeBranch>, String> {
    let trees = list(cwd)?;
    let refs = command::checked(
        cwd,
        &argv(&["for-each-ref", "--format=%(refname:strip=2)", "refs/heads/"]),
        WORKTREE_LIST_TIMEOUT,
    )?;
    Ok(String::from_utf8_lossy(&refs)
        .lines()
        .map(|name| WorktreeBranch {
            name: name.to_string(),
            checkout: trees
                .iter()
                .find(|tree| tree.branch.as_deref() == Some(name))
                .map(|tree| tree.path.clone()),
        })
        .collect())
}

/// Where a new checkout goes when no path is given, calculated only with the server host's path
/// rules. Without a configured `directory` it is the visible sibling `<repo>-worktrees/<branch>`
/// beside the primary checkout. An absolute `directory` holds every repository's checkouts as
/// `<directory>/<repo>/<branch>`; a relative one is inside the repository, `<repo>/<directory>/<branch>`.
pub fn default_path(
    cwd: &Path,
    branch: &str,
    directory: Option<&Path>,
) -> Result<std::path::PathBuf, String> {
    let trees = list(cwd)?;
    let primary = trees
        .first()
        .ok_or_else(|| "Git reported no primary worktree".to_string())?;
    let source = Path::new(&primary.path);
    let parent = source
        .parent()
        .ok_or_else(|| "primary worktree has no parent directory".to_string())?;
    let name = source
        .file_name()
        .ok_or_else(|| "primary worktree has no directory name".to_string())?
        .to_string_lossy();
    let slug = branch
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    if slug.is_empty() {
        return Err("worktree branch cannot be empty".to_string());
    }
    let base = match checkout_root(directory)? {
        CheckoutRoot::Beside => parent.join(format!("{name}-worktrees")),
        CheckoutRoot::External(directory) => directory.join(name.as_ref()),
        CheckoutRoot::InRepository(folder) => source.join(folder),
    };
    Ok(base.join(slug))
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

    list(cwd)?
        .into_iter()
        .find(|tree| same_path(Path::new(&tree.path), path))
        .ok_or_else(|| "created worktree was not found in Git's worktree list".to_string())
}

/// Which copies of a removed checkout's branch to delete. Both off keeps the branch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BranchDeletion {
    /// The local branch, if fully merged.
    pub local: bool,
    /// The branch on the remote it tracks, if its fetched tip is merged into the primary checkout.
    pub remote: bool,
}

impl BranchDeletion {
    pub const NONE: Self = Self {
        local: false,
        remote: false,
    };

    pub fn any(self) -> bool {
        self.local || self.remote
    }

    /// `local`, `remote`, or both, comma separated.
    pub fn parse(list: &str) -> Result<Self, String> {
        let mut selection = Self::NONE;
        for item in list.split(',').map(str::trim) {
            match item {
                "local" => selection.local = true,
                "remote" => selection.remote = true,
                other => return Err(format!("unknown branch `{other}`; use local or remote")),
            }
        }
        Ok(selection)
    }
}

/// Which copy of a branch an outcome is about.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BranchCopy {
    Local,
    Remote {
        /// The remote's name, unknown when the branch has no usable upstream.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote: Option<String>,
    },
}

/// What happened to one copy of a removed checkout's branch.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum BranchOutcome {
    Deleted {
        branch: String,
        copy: BranchCopy,
    },
    /// Git or a guard here kept the branch; `reason` says why.
    Kept {
        branch: String,
        copy: BranchCopy,
        reason: String,
    },
}

impl BranchOutcome {
    /// `branch feat`, `remote branch origin/feat`, or `remote branch feat` when the remote is
    /// unknown.
    pub fn subject(&self) -> String {
        let (Self::Deleted { branch, copy } | Self::Kept { branch, copy, .. }) = self;
        match copy {
            BranchCopy::Local => format!("branch {branch}"),
            BranchCopy::Remote {
                remote: Some(remote),
            } => {
                format!("remote branch {remote}/{branch}")
            }
            BranchCopy::Remote { remote: None } => format!("remote branch {branch}"),
        }
    }
}

/// Why a branch has no remote copy to delete, with the remote's name when it is known.
type NoUpstream = (Option<String>, String);

/// Remove a linked checkout. `force` only relaxes Git's dirty-checkout guard; it never removes a
/// Rozi session or overrides a lock. `unlock_stale` first lifts a lock whose owner is gone, judged
/// here rather than trusting the caller, and restores it if Git still refuses. Session-origin
/// protection belongs at the RPC boundary.
///
/// `delete_branch` then deletes the checkout's branch, never forced: the local branch with
/// `git branch -d`, the remote one only when it is merged and unchanged since it was fetched. A
/// failure there never fails the removal, which has already happened; each copy is reported as
/// [`BranchOutcome::Deleted`] or [`BranchOutcome::Kept`]. A detached checkout has no branch, so
/// nothing is reported.
pub fn remove(
    cwd: &Path,
    path: &Path,
    force: bool,
    unlock_stale: bool,
    delete_branch: BranchDeletion,
) -> Result<Vec<BranchOutcome>, String> {
    let worktree = linked_worktree(cwd, path)?;
    // `cwd` may be the checkout being removed, which Windows will not delete while a process runs
    // in it, and which is gone for the branch delete afterwards. Run from the primary checkout.
    let home = list(cwd)?
        .into_iter()
        .next()
        .map_or_else(|| cwd.to_path_buf(), |tree| PathBuf::from(tree.path));
    let cwd = home.as_path();
    if let Some(lock) = &worktree.lock {
        if !(unlock_stale && lock.stale) {
            return Err(locked_error(lock));
        }
        unlock_path(cwd, &worktree.path)?;
    }
    let mut args = argv(&["worktree", "remove"]);
    if force {
        args.push("--force".into());
    }
    args.push("--".into());
    args.push(worktree.path.clone().into());
    let removed = command::checked(cwd, &args, WORKTREE_MUTATION_TIMEOUT).map(drop);
    if removed.is_err()
        && let Some(lock) = &worktree.lock
    {
        // Best effort: Git's refusal is the error worth reporting.
        let _ = lock_path(cwd, &worktree.path, &lock.reason);
    }
    removed?;
    Ok(match worktree.branch {
        Some(branch) if delete_branch.any() => delete_branches(cwd, branch, delete_branch),
        _ => Vec::new(),
    })
}

/// Delete the selected copies of a branch whose checkout is gone, unless another checkout (the
/// primary's included) still has it checked out.
fn delete_branches(cwd: &Path, branch: String, selection: BranchDeletion) -> Vec<BranchOutcome> {
    // Read before the local branch, and its configuration, are gone.
    let upstream = selection.remote.then(|| upstream(cwd, &branch));
    let kept_all = |reason: String| {
        let mut outcomes = Vec::new();
        if selection.local {
            outcomes.push(BranchOutcome::Kept {
                branch: branch.clone(),
                copy: BranchCopy::Local,
                reason: reason.clone(),
            });
        }
        if let Some(upstream) = &upstream {
            outcomes.push(BranchOutcome::Kept {
                branch: branch.clone(),
                copy: remote_copy(upstream),
                reason,
            });
        }
        outcomes
    };
    match list(cwd) {
        Ok(trees)
            if trees
                .iter()
                .any(|tree| tree.branch.as_ref() == Some(&branch)) =>
        {
            return kept_all("it is checked out in another worktree".to_string());
        }
        Err(message) => return kept_all(message),
        Ok(_) => {}
    }
    let mut outcomes = Vec::new();
    if selection.local {
        outcomes.push(delete_merged_branch(cwd, &branch));
    }
    if let Some(upstream) = upstream {
        outcomes.push(match upstream {
            Ok(remote) => delete_remote_branch(cwd, &branch, remote),
            Err((remote, reason)) => BranchOutcome::Kept {
                branch,
                copy: BranchCopy::Remote { remote },
                reason,
            },
        });
    }
    outcomes
}

fn remote_copy(upstream: &Result<String, NoUpstream>) -> BranchCopy {
    BranchCopy::Remote {
        remote: match upstream {
            Ok(remote) => Some(remote.clone()),
            Err((remote, _)) => remote.clone(),
        },
    }
}

/// Delete a local branch unless Git finds it unmerged.
fn delete_merged_branch(cwd: &Path, branch: &str) -> BranchOutcome {
    let mut args = argv(&["branch", "-d", "--"]);
    args.push(branch.into());
    match command::checked(cwd, &args, WORKTREE_MUTATION_TIMEOUT) {
        Ok(_) => BranchOutcome::Deleted {
            branch: branch.to_string(),
            copy: BranchCopy::Local,
        },
        Err(message) => BranchOutcome::Kept {
            branch: branch.to_string(),
            copy: BranchCopy::Local,
            reason: first_line(&message),
        },
    }
}

/// The remote a branch tracks, when it tracks a same-named branch there. Deleting a differently
/// named upstream (a feature branch cut from `origin/main`) would be someone else's branch.
fn upstream(cwd: &Path, branch: &str) -> Result<String, NoUpstream> {
    let config = |key: &str| -> Option<String> {
        let args = argv(&["config", "--get", &format!("branch.{branch}.{key}")]);
        let output = command::checked(cwd, &args, WORKTREE_LIST_TIMEOUT).ok()?;
        Some(String::from_utf8_lossy(&output).trim().to_string()).filter(|value| !value.is_empty())
    };
    let (Some(remote), Some(merge)) = (config("remote"), config("merge")) else {
        return Err((None, "it has no upstream".to_string()));
    };
    if remote == "." {
        return Err((None, "it tracks a local branch".to_string()));
    }
    match merge.strip_prefix("refs/heads/") {
        Some(name) if name == branch => Ok(remote),
        _ => {
            let reason = format!(
                "its upstream {remote}/{} is named differently",
                merge.trim_start_matches("refs/heads/")
            );
            Err((Some(remote), reason))
        }
    }
}

/// Delete `branch` on `remote` when the fetched tip is merged into the primary checkout's `HEAD`.
/// The push only goes through while the remote still has that tip, so commits pushed since the
/// last fetch are never lost.
fn delete_remote_branch(cwd: &Path, branch: &str, remote: String) -> BranchOutcome {
    let tracking = format!("refs/remotes/{remote}/{branch}");
    let kept = |reason: String| BranchOutcome::Kept {
        branch: branch.to_string(),
        copy: BranchCopy::Remote {
            remote: Some(remote.clone()),
        },
        reason,
    };
    let mut args = argv(&["rev-parse", "--verify", "--quiet"]);
    args.push(format!("{tracking}^{{commit}}").into());
    let Ok(output) = command::checked(cwd, &args, WORKTREE_LIST_TIMEOUT) else {
        return kept("it has not been fetched".to_string());
    };
    let tip = String::from_utf8_lossy(&output).trim().to_string();

    let mut args = argv(&["merge-base", "--is-ancestor"]);
    args.push(tip.clone().into());
    args.push("HEAD".into());
    match command::run(cwd, &args, WORKTREE_LIST_TIMEOUT) {
        Ok(output) if output.status == Some(0) => {}
        Ok(output) if output.status == Some(1) => {
            return kept("it is not merged into the primary checkout".to_string());
        }
        Ok(_) => return kept("its merge state could not be read".to_string()),
        Err(message) => return kept(message),
    }

    let mut args = argv(&["push"]);
    args.push(format!("--force-with-lease=refs/heads/{branch}:{tip}").into());
    args.push("--".into());
    args.push(remote.clone().into());
    args.push(format!(":refs/heads/{branch}").into());
    match command::checked_remote(cwd, &args) {
        Ok(_) => BranchOutcome::Deleted {
            branch: branch.to_string(),
            copy: BranchCopy::Remote {
                remote: Some(remote),
            },
        },
        Err(message) => kept(push_reason(&message)),
    }
}

/// The line of `git push` output that says why it failed, rather than its progress chatter. A
/// lease refusal, which means the remote branch moved since it was fetched, is named as such.
fn push_reason(message: &str) -> String {
    if let Some(rejected) = message
        .lines()
        .map(str::trim)
        .find(|line| line.contains("rejected]"))
    {
        if rejected.contains("(stale info)") {
            return "it changed since it was fetched".to_string();
        }
        if let Some(reason) = rejected
            .rsplit_once('(')
            .and_then(|(_, tail)| tail.strip_suffix(')'))
        {
            return reason.to_string();
        }
    }
    let line = message
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("fatal:") || line.starts_with("error:"))
        .unwrap_or_else(|| message.lines().next().unwrap_or_default().trim());
    let line = line.strip_prefix("fatal: ").unwrap_or(line);
    line.strip_prefix("error: ").unwrap_or(line).to_string()
}

/// Git's explanation, without its `error:` prefix and the advice that follows it.
fn first_line(message: &str) -> String {
    let line = message.lines().next().unwrap_or_default().trim();
    line.strip_prefix("error: ").unwrap_or(line).to_string()
}

/// Lift a linked checkout's lock, whoever holds it. Whether its owner must be gone first is the
/// caller's decision; Git itself unlocks any lock.
pub fn unlock(cwd: &Path, path: &Path) -> Result<(), String> {
    let worktree = linked_worktree(cwd, path)?;
    if worktree.lock.is_none() {
        return Err("worktree is not locked".to_string());
    }
    unlock_path(cwd, &worktree.path)
}

fn linked_worktree(cwd: &Path, path: &Path) -> Result<WorktreeInfo, String> {
    absolute_path(path)?;
    let worktree = list(cwd)?
        .into_iter()
        .find(|tree| same_path(Path::new(&tree.path), path))
        .ok_or_else(|| "path is not a registered Git worktree".to_string())?;
    if !worktree.linked || worktree.bare {
        return Err("the primary worktree cannot be removed or unlocked".to_string());
    }
    Ok(worktree)
}

fn locked_error(lock: &WorktreeLock) -> String {
    let what = if lock.stale {
        "worktree is locked by a process that is no longer running"
    } else {
        "worktree is locked"
    };
    if lock.reason.is_empty() {
        what.to_string()
    } else {
        format!("{what}: {}", lock.reason)
    }
}

fn unlock_path(cwd: &Path, path: &str) -> Result<(), String> {
    let mut args = argv(&["worktree", "unlock", "--"]);
    args.push(path.into());
    command::checked(cwd, &args, WORKTREE_MUTATION_TIMEOUT).map(drop)
}

fn lock_path(cwd: &Path, path: &str, reason: &str) -> Result<(), String> {
    let mut args = argv(&["worktree", "lock"]);
    if !reason.is_empty() {
        args.push("--reason".into());
        args.push(reason.into());
    }
    args.push("--".into());
    args.push(path.into());
    command::checked(cwd, &args, WORKTREE_MUTATION_TIMEOUT).map(drop)
}

/// Where `[worktrees] directory` keeps new checkouts.
#[derive(Debug, PartialEq, Eq)]
pub enum CheckoutRoot<'a> {
    /// Unset: `<repo>-worktrees/<branch>` beside the repository.
    Beside,
    /// An absolute directory holding every repository's checkouts as `<directory>/<repo>/<branch>`.
    External(&'a Path),
    /// One top-level folder of the repository: `<repo>/<folder>/<branch>`.
    InRepository(&'a str),
}

/// Interpret `[worktrees] directory`, already `~`-expanded on the session host. A relative value
/// must be a single plain folder name such as `.worktrees`: that keeps "relative" meaning inside
/// the repository (no `../worktrees`), and names exactly the directory an ignore rule has to
/// cover (not `tools/` for `tools/.worktrees`).
pub fn checkout_root(directory: Option<&Path>) -> Result<CheckoutRoot<'_>, String> {
    let Some(directory) = directory else {
        return Ok(CheckoutRoot::Beside);
    };
    if directory.is_absolute() {
        return Ok(CheckoutRoot::External(directory));
    }
    let invalid = || {
        format!(
            "[worktrees] directory `{}` must be one folder name inside the repository, such as \
             `.worktrees`, or an absolute path",
            directory.display()
        )
    };
    let folder = directory
        .to_str()
        .ok_or_else(invalid)?
        .trim_end_matches(['/', '\\']);
    plain_directory_name(folder).map_err(|_| invalid())?;
    Ok(CheckoutRoot::InRepository(folder))
}

/// A name that is one directory at the top of a repository and can be written as a literal ignore
/// rule: no separators, no `.`/`..`, and no ignore-pattern syntax.
fn plain_directory_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || matches!(name, "." | "..")
        || name.contains(['/', '\\'])
        || name
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '*' | '?' | '[' | '!' | '#'))
    {
        return Err(format!("`{name}` is not a plain top-level directory name"));
    }
    Ok(())
}

/// The repository's primary checkout, which holds its ignore rules.
pub fn primary_checkout(cwd: &Path) -> Result<std::path::PathBuf, String> {
    list(cwd)?
        .into_iter()
        .next()
        .map(|tree| std::path::PathBuf::from(tree.path))
        .ok_or_else(|| "Git reported no primary worktree".to_string())
}

/// Whether Git ignores the top-level directory `name` of the checkout at `primary`, as
/// `.gitignore`, `.git/info/exclude`, and the user's global excludes decide. The directory need
/// not exist yet.
pub fn directory_ignored(primary: &Path, name: &str) -> Result<bool, String> {
    let output = command::run(
        primary,
        &[
            "check-ignore".into(),
            "--quiet".into(),
            "--".into(),
            format!("{name}/").into(),
        ],
        WORKTREE_LIST_TIMEOUT,
    )?;
    match output.status {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(git_error(&output)),
    }
}

/// Add the top-level directory `name` to the repository's `.git/info/exclude`, the local ignore
/// file Git never commits. Never touches `.gitignore`. Adding an already ignored directory does
/// nothing.
pub fn exclude_directory(cwd: &Path, name: &str) -> Result<(), String> {
    plain_directory_name(name)?;
    let primary = primary_checkout(cwd)?;
    if directory_ignored(&primary, name)? {
        return Ok(());
    }
    // `--git-path` follows linked worktrees to the shared repository directory.
    let file = command::checked(
        &primary,
        &argv(&["rev-parse", "--git-path", "info/exclude"]),
        WORKTREE_LIST_TIMEOUT,
    )?;
    let file =
        String::from_utf8(file).map_err(|_| "Git returned a non-UTF-8 exclude path".to_string())?;
    let file = primary.join(file.trim_end_matches(['\n', '\r']));
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    // Appended rather than rewritten: the rule is purely additive, so a concurrent edit or an
    // interrupted write can never lose what the file already held.
    let ends_mid_line = match std::fs::read(&file) {
        Ok(contents) => contents.last().is_some_and(|byte| *byte != b'\n'),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => return Err(format!("cannot read {}: {err}", file.display())),
    };
    let rule = format!("{}/{name}/\n", if ends_mid_line { "\n" } else { "" });
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .and_then(|mut handle| std::io::Write::write_all(&mut handle, rule.as_bytes()))
        .map_err(|err| format!("cannot write {}: {err}", file.display()))
}

/// Whether two host paths name the same directory. Git on Windows reports `C:/Users/...` where
/// the rest of the system may say `C:\Users\RUNNER~1\...`, so both sides are resolved first;
/// a path that does not exist compares as written.
pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    let resolve = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    a == b || resolve(a) == resolve(b)
}

/// A path as Git printed it, with this host's separators. Git uses `/` even on Windows, where the
/// paths Rozi compares it with (pane directories, recorded origins) use `\`.
fn native_path(path: &str) -> String {
    if cfg!(windows) {
        path.replace('/', "\\")
    } else {
        path.to_string()
    }
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
                path: native_path(path),
                branch: None,
                detached: false,
                bare: false,
                prunable: false,
                linked: !trees.is_empty(),
                lock: None,
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
        } else if field == "locked" {
            tree.lock = Some(WorktreeLock::new(""));
        } else if let Some(reason) = field.strip_prefix("locked ") {
            tree.lock = Some(WorktreeLock::new(reason));
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
    use std::path::PathBuf;
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
        assert!(trees[2].prunable);
        assert_eq!(
            trees[2].lock,
            Some(WorktreeLock {
                reason: "maintenance".into(),
                stale: false,
            })
        );
    }

    #[test]
    fn a_lock_reason_names_its_owner_by_pid_and_start_time() {
        // Claude Code on Linux, verbatim: the start is `/proc/<pid>/stat` field 22.
        assert_eq!(
            lock_owner("claude session fix-x (pid 538630 start 34095030)"),
            Some((538_630, Some(34_095_030)))
        );
        // Claude Code on macOS writes a date, which only the PID is read from.
        assert_eq!(
            lock_owner("claude session wt-lock (pid 72411 start Tue Sep 8 13:50:41 2026)"),
            Some((72_411, None))
        );
        assert_eq!(
            lock_owner("claude session wt-lock (pid 72411)"),
            Some((72_411, None))
        );
        assert_eq!(lock_owner("held by pid 42"), Some((42, None)));
        assert_eq!(lock_owner("pid 42, start soon"), Some((42, None)));
        for reason in ["", "maintenance", "pid", "pid zero", "pid 0", "rapid 42"] {
            assert_eq!(lock_owner(reason), None, "{reason:?}");
        }
        // A lock naming no process, or a running one, is not stale.
        assert!(!WorktreeLock::new("initializing").stale);
        assert!(!WorktreeLock::new(&format!("pid {}", std::process::id())).stale);
    }

    /// The exact reason Claude Code writes on Linux, for this process: its own start time reads as
    /// live, and any other start time as a reused PID.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_claude_lock_on_linux_tells_a_reused_pid_from_its_owner() {
        let pid = std::process::id();
        let start = crate::platform::process::linux::start_time(pid).expect("own start time");
        let reason = |start: u64| format!("claude session wt-x (pid {pid} start {start})");
        assert!(!WorktreeLock::new(&reason(start)).stale);
        assert!(WorktreeLock::new(&reason(start + 1)).stale);
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

        git(&repo, &["branch", "available"]);
        let choices = branches(&repo).unwrap();
        assert!(choices.iter().any(|branch| {
            branch.name == "existing"
                && branch
                    .checkout
                    .as_deref()
                    .is_some_and(|checkout| same_path(Path::new(checkout), &existing))
        }));
        assert!(
            choices
                .iter()
                .any(|branch| branch.name == "available" && branch.checkout.is_none())
        );

        let fresh = temp.path().join("fresh checkout");
        let tree = create(&repo, "feat/new", "HEAD", &fresh).unwrap();
        assert_eq!(tree.branch.as_deref(), Some("feat/new"));
        std::fs::write(fresh.join("scratch.txt"), b"unsaved").unwrap();
        assert!(remove(&repo, &fresh, false, false, BranchDeletion::NONE).is_err());
        assert!(fresh.exists());
        assert!(remove(&repo, &repo, true, false, BranchDeletion::NONE).is_err());
        remove(&repo, &fresh, true, false, BranchDeletion::NONE).unwrap();
        assert!(!fresh.exists());
        assert!(
            list(&repo)
                .unwrap()
                .iter()
                .all(|tree| tree.path != fresh.to_string_lossy())
        );
        git(&repo, &["show-ref", "--verify", "refs/heads/feat/new"]);
        remove(&repo, &existing, false, false, BranchDeletion::NONE).unwrap();
    }

    fn has_branch(repo: &Path, name: &str) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["show-ref", "--verify", "--quiet"])
            .arg(format!("refs/heads/{name}"))
            .status()
            .unwrap()
            .success()
    }

    const LOCAL: BranchDeletion = BranchDeletion {
        local: true,
        remote: false,
    };
    const REMOTE: BranchDeletion = BranchDeletion {
        local: false,
        remote: true,
    };
    const BOTH: BranchDeletion = BranchDeletion {
        local: true,
        remote: true,
    };

    fn commit(cwd: &Path, message: &str) {
        git(
            cwd,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=t@example.com",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                message,
            ],
        );
    }

    fn deleted(branch: &str, remote: Option<&str>) -> BranchOutcome {
        BranchOutcome::Deleted {
            branch: branch.into(),
            copy: match remote {
                Some(remote) => BranchCopy::Remote {
                    remote: Some(remote.into()),
                },
                None => BranchCopy::Local,
            },
        }
    }

    fn kept_reason(outcomes: &[BranchOutcome], remote: bool) -> String {
        outcomes
            .iter()
            .find_map(|outcome| match outcome {
                BranchOutcome::Kept { copy, reason, .. }
                    if matches!(copy, BranchCopy::Remote { .. }) == remote =>
                {
                    Some(reason.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("no kept outcome in {outcomes:?}"))
    }

    #[test]
    fn removing_a_checkout_deletes_only_a_merged_local_branch_when_asked() {
        if !crate::platform::command::program_exists("git") {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        commit(&repo, "initial");

        let merged = temp.path().join("merged");
        create(&repo, "feat/merged", "HEAD", &merged).unwrap();
        assert_eq!(
            remove(&repo, &merged, false, false, LOCAL).unwrap(),
            vec![deleted("feat/merged", None)]
        );
        assert!(!has_branch(&repo, "feat/merged"));

        let unmerged = temp.path().join("unmerged");
        create(&repo, "feat/unmerged", "HEAD", &unmerged).unwrap();
        commit(&unmerged, "work");
        let outcomes = remove(&repo, &unmerged, true, false, LOCAL).unwrap();
        assert!(
            kept_reason(&outcomes, false).contains("not fully merged"),
            "{outcomes:?}"
        );
        assert!(!unmerged.exists(), "the checkout is removed regardless");
        assert!(has_branch(&repo, "feat/unmerged"));

        // The checkout being removed may be the directory the caller works in.
        let current = temp.path().join("current");
        create(&repo, "feat/current", "HEAD", &current).unwrap();
        assert_eq!(
            remove(&current, &current, false, false, LOCAL).unwrap(),
            vec![deleted("feat/current", None)]
        );
        assert!(!has_branch(&repo, "feat/current"));

        let kept = temp.path().join("kept");
        create(&repo, "feat/kept", "HEAD", &kept).unwrap();
        assert!(
            remove(&repo, &kept, false, false, BranchDeletion::NONE)
                .unwrap()
                .is_empty()
        );
        assert!(has_branch(&repo, "feat/kept"));

        let detached = temp.path().join("detached");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                detached.to_str().unwrap(),
            ],
        );
        assert!(
            remove(&repo, &detached, false, false, BOTH)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn removing_a_checkout_deletes_a_merged_unchanged_remote_branch_only_when_asked() {
        if !crate::platform::command::program_exists("git") {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let origin = temp.path().join("origin.git");
        let repo = temp.path().join("repo");
        std::fs::create_dir(&origin).unwrap();
        std::fs::create_dir(&repo).unwrap();
        git(&origin, &["init", "-q", "--bare"]);
        git(&repo, &["init", "-q"]);
        commit(&repo, "initial");
        git(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        let on_origin = |name: &str| {
            Command::new("git")
                .arg("-C")
                .arg(&origin)
                .args(["show-ref", "--verify", "--quiet"])
                .arg(format!("refs/heads/{name}"))
                .status()
                .unwrap()
                .success()
        };
        let pushed = |name: &str| {
            let checkout = temp.path().join(name.replace('/', "-"));
            create(&repo, name, "HEAD", &checkout).unwrap();
            git(&checkout, &["push", "-q", "-u", "origin", name]);
            checkout
        };

        let merged = pushed("feat/merged");
        assert_eq!(
            remove(&repo, &merged, false, false, REMOTE).unwrap(),
            vec![deleted("feat/merged", Some("origin"))]
        );
        assert!(!on_origin("feat/merged"));
        assert!(
            has_branch(&repo, "feat/merged"),
            "only the remote was asked"
        );

        let both = pushed("feat/both");
        assert_eq!(
            remove(&repo, &both, false, false, BOTH).unwrap(),
            vec![
                deleted("feat/both", None),
                deleted("feat/both", Some("origin"))
            ]
        );
        assert!(!on_origin("feat/both") && !has_branch(&repo, "feat/both"));

        let local_only = pushed("feat/local-only");
        remove(&repo, &local_only, false, false, LOCAL).unwrap();
        assert!(on_origin("feat/local-only"), "the remote is not touched");

        // Merged into its own upstream, so `-d` allows it; not merged into the primary checkout,
        // so the remote copy stays.
        let unmerged = temp.path().join("unmerged");
        create(&repo, "feat/unmerged", "HEAD", &unmerged).unwrap();
        commit(&unmerged, "work");
        git(&unmerged, &["push", "-q", "-u", "origin", "feat/unmerged"]);
        let outcomes = remove(&repo, &unmerged, false, false, BOTH).unwrap();
        assert_eq!(outcomes[0], deleted("feat/unmerged", None));
        assert!(
            kept_reason(&outcomes, true).contains("not merged"),
            "{outcomes:?}"
        );
        assert!(on_origin("feat/unmerged"));

        // Someone pushed after the last fetch: the lease refuses to drop their commit.
        let stale = pushed("feat/stale");
        let other = temp.path().join("other");
        git(
            temp.path(),
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                other.to_str().unwrap(),
            ],
        );
        git(&other, &["checkout", "-q", "feat/stale"]);
        commit(&other, "pushed meanwhile");
        git(&other, &["push", "-q", "origin", "feat/stale"]);
        let outcomes = remove(&repo, &stale, false, false, REMOTE).unwrap();
        assert_eq!(
            kept_reason(&outcomes, true),
            "it changed since it was fetched",
            "{outcomes:?}"
        );
        assert!(on_origin("feat/stale"));

        let untracked = temp.path().join("untracked");
        create(&repo, "feat/untracked", "HEAD", &untracked).unwrap();
        let outcomes = remove(&repo, &untracked, false, false, REMOTE).unwrap();
        assert_eq!(kept_reason(&outcomes, true), "it has no upstream");
        assert_eq!(outcomes[0].subject(), "remote branch feat/untracked");
        assert!(
            has_branch(&repo, "feat/untracked"),
            "only the remote was asked"
        );

        // The local copy goes and the remote one is reported as the remote one, not as the local.
        let both = temp.path().join("untracked-both");
        create(&repo, "feat/untracked-both", "HEAD", &both).unwrap();
        let outcomes = remove(&repo, &both, false, false, BOTH).unwrap();
        assert_eq!(outcomes.len(), 2, "{outcomes:?}");
        assert_eq!(outcomes[0], deleted("feat/untracked-both", None));
        assert_eq!(
            outcomes[1],
            BranchOutcome::Kept {
                branch: "feat/untracked-both".into(),
                copy: BranchCopy::Remote { remote: None },
                reason: "it has no upstream".into(),
            }
        );
    }

    #[test]
    fn locks_block_removal_until_unlocked_and_only_a_stale_one_lifts_itself() {
        if !crate::platform::command::program_exists("git") {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
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
        let lock_of = |path: &Path| {
            list(&repo)
                .unwrap()
                .into_iter()
                .find(|tree| same_path(Path::new(&tree.path), path))
                .unwrap()
                .lock
        };

        let live = temp.path().join("live");
        create(&repo, "live", "HEAD", &live).unwrap();
        let live_reason = format!("agent (pid {})", std::process::id());
        git(
            &repo,
            &[
                "worktree",
                "lock",
                "--reason",
                &live_reason,
                live.to_str().unwrap(),
            ],
        );
        assert_eq!(
            lock_of(&live),
            Some(WorktreeLock {
                reason: live_reason,
                stale: false,
            })
        );
        // Neither `force` nor `unlock_stale` gets past a running owner's lock.
        let refused = remove(&repo, &live, true, true, BranchDeletion::NONE).unwrap_err();
        assert!(
            refused.starts_with("worktree is locked: agent"),
            "{refused}"
        );
        assert!(live.exists());
        unlock(&repo, &live).unwrap();
        assert_eq!(lock_of(&live), None);
        assert!(unlock(&repo, &live).is_err(), "already unlocked");
        assert!(unlock(&repo, &repo).is_err(), "the primary checkout");
        remove(&repo, &live, false, false, BranchDeletion::NONE).unwrap();

        // The owner has exited, so the lock is stale.
        let mut child = Command::new("git").arg("--version").spawn().unwrap();
        let dead = child.id();
        child.wait().unwrap();
        let stale = temp.path().join("stale");
        create(&repo, "stale", "HEAD", &stale).unwrap();
        let stale_reason = format!("claude session stale (pid {dead} start 1)");
        git(
            &repo,
            &[
                "worktree",
                "lock",
                "--reason",
                &stale_reason,
                stale.to_str().unwrap(),
            ],
        );
        assert!(lock_of(&stale).is_some_and(|lock| lock.stale));
        let refused = remove(&repo, &stale, false, false, BranchDeletion::NONE).unwrap_err();
        assert!(refused.contains("no longer running"), "{refused}");

        // Git refuses the dirty checkout, so the lock goes back as it was.
        std::fs::write(stale.join("scratch.txt"), b"unsaved").unwrap();
        assert!(remove(&repo, &stale, false, true, BranchDeletion::NONE).is_err());
        assert_eq!(lock_of(&stale).map(|lock| lock.reason), Some(stale_reason));
        remove(&repo, &stale, true, true, BranchDeletion::NONE).unwrap();
        assert!(!stale.exists());
    }

    #[test]
    fn default_path_is_a_sibling_unless_a_directory_is_configured() {
        if !crate::platform::command::program_exists("git") {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("rozi");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        let primary = PathBuf::from(&list(&repo).unwrap()[0].path);
        let parent = primary.parent().unwrap();

        assert_eq!(
            default_path(&repo, "feat/login", None).unwrap(),
            parent.join("rozi-worktrees").join("feat-login")
        );
        let directory = temp.path().join("worktrees");
        assert_eq!(
            default_path(&repo, "feat/login", Some(&directory)).unwrap(),
            directory.join("rozi").join("feat-login")
        );
        // A relative directory is inside the repository.
        assert_eq!(
            default_path(&repo, "feat/login", Some(Path::new(".worktrees"))).unwrap(),
            primary.join(".worktrees").join("feat-login")
        );
        assert!(default_path(&repo, "feat/login", Some(Path::new("../worktrees"))).is_err());
    }

    #[test]
    fn a_relative_directory_is_one_folder_inside_the_repository() {
        assert_eq!(checkout_root(None), Ok(CheckoutRoot::Beside));
        for folder in [".worktrees", "rozi-worktrees", ".worktrees/"] {
            assert_eq!(
                checkout_root(Some(Path::new(folder))),
                Ok(CheckoutRoot::InRepository(folder.trim_end_matches('/'))),
                "{folder}"
            );
        }
        for invalid in ["../worktrees", "tools/.worktrees", ".", "..", "*"] {
            assert!(
                checkout_root(Some(Path::new(invalid))).is_err(),
                "accepted {invalid}"
            );
        }
        let absolute = std::env::temp_dir().join("worktrees");
        assert_eq!(
            checkout_root(Some(&absolute)),
            Ok(CheckoutRoot::External(absolute.as_path()))
        );
    }

    #[test]
    fn exclude_adds_a_local_ignore_rule_once_and_only_for_a_plain_directory() {
        if !crate::platform::command::program_exists("git") {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("rozi");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        let primary = primary_checkout(&repo).unwrap();

        assert!(!directory_ignored(&primary, ".worktrees").unwrap());
        exclude_directory(&repo, ".worktrees").unwrap();
        assert!(directory_ignored(&primary, ".worktrees").unwrap());
        exclude_directory(&repo, ".worktrees").unwrap();
        // Appending keeps what was there, even a last line without a newline.
        let exclude_file = repo.join(".git").join("info").join("exclude");
        let before = std::fs::read_to_string(&exclude_file).unwrap();
        std::fs::write(&exclude_file, format!("{before}/scratch")).unwrap();
        exclude_directory(&repo, "tmp").unwrap();
        let after = std::fs::read_to_string(&exclude_file).unwrap();
        assert!(after.starts_with(&before), "{after}");
        assert!(after.ends_with("/scratch\n/tmp/\n"), "{after}");
        let exclude =
            std::fs::read_to_string(repo.join(".git").join("info").join("exclude")).unwrap();
        assert_eq!(exclude.matches("/.worktrees/").count(), 1, "{exclude}");
        assert!(
            !repo.join(".gitignore").exists(),
            "the committed ignore file is never touched"
        );

        for name in ["", ".", "..", "a/b", "a\\b", "*", "#x", "!x"] {
            assert!(exclude_directory(&repo, name).is_err(), "accepted {name:?}");
        }
    }
}
