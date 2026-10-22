//! Host-side worktree lifecycle, shared by the session server's worker and `rozi worktrees`.
//!
//! Everything here runs on the machine that owns the repository. A caller on another host sends a
//! request and renders the reply; it never builds or normalizes these paths itself.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::protocol::{WorktreeRequest, WorktreeResult};
use crate::git::worktrees::{self, WorktreeInfo};

/// Run one session-protocol worktree request. `directory` is `[worktrees] directory`, expanded on
/// this host. Removal refuses a checkout that a live or restorable session records as its origin;
/// `force` never overrides that.
pub(crate) fn execute(request: WorktreeRequest, directory: Option<&Path>) -> WorktreeResult {
    let result = match request {
        WorktreeRequest::Branches { cwd } => worktrees::branches(Path::new(&cwd))
            .map(|branches| WorktreeResult::Branches { branches }),
        WorktreeRequest::Status { cwd, refresh } => Ok(WorktreeResult::Statuses {
            statuses: crate::git::pull_requests::StatusCache::default()
                .get(Path::new(&cwd), refresh),
        }),
        WorktreeRequest::List { cwd } => {
            worktrees::list(Path::new(&cwd)).map(|worktrees| WorktreeResult::Listed {
                sessions: sessions_by_checkout(&worktrees),
                worktrees,
            })
        }
        WorktreeRequest::Preview { cwd, branch } => {
            let cwd = Path::new(&cwd);
            worktrees::default_path(cwd, &branch, directory).map(|path| WorktreeResult::Previewed {
                unignored: unignored_directory(cwd, &path),
                path: path.to_string_lossy().into_owned(),
            })
        }
        WorktreeRequest::Create {
            cwd,
            branch,
            base,
            path,
        } => {
            let cwd = Path::new(&cwd);
            create(cwd, &branch, &base, path.map(PathBuf::from), directory).map(|worktree| {
                WorktreeResult::Created {
                    unignored: unignored_directory(cwd, Path::new(&worktree.path)),
                    worktree,
                }
            })
        }
        WorktreeRequest::Remove {
            cwd,
            path,
            force,
            unlock_stale,
            delete_branch,
        } => remove(
            Path::new(&cwd),
            Path::new(&path),
            force,
            unlock_stale,
            delete_branch,
        )
        .map(|branches| WorktreeResult::Removed { path, branches }),
        WorktreeRequest::Unlock { cwd, path } => {
            unlock(Path::new(&cwd), Path::new(&path)).map(|()| WorktreeResult::Unlocked { path })
        }
        WorktreeRequest::Exclude { cwd, directory } => {
            worktrees::exclude_directory(Path::new(&cwd), &directory)
                .map(|()| WorktreeResult::Excluded { directory })
        }
    };
    result.unwrap_or_else(|message| WorktreeResult::Failed { message })
}

/// Which sessions record each checkout as their origin, keyed by the checkout's listed path.
/// Display only: a session that cannot be verified is simply not listed here, unlike the removal
/// guard, which refuses on it.
fn sessions_by_checkout(
    trees: &[WorktreeInfo],
) -> std::collections::BTreeMap<String, Vec<super::protocol::WorktreeSession>> {
    let Ok(origins) = super::discovery::worktree_session_origins() else {
        return Default::default();
    };
    trees
        .iter()
        .filter_map(|tree| {
            let sessions = origins.session_refs_at(&canonical(Path::new(&tree.path)));
            (!sessions.is_empty()).then(|| (tree.path.clone(), sessions))
        })
        .collect()
}

/// The repository's top-level directory that a checkout at `path` sits in, when the checkout is
/// inside the primary checkout and Git does not ignore that directory. Such a checkout shows up as
/// untracked in `git status` and is swept up by `git add -A`. A check that fails is not a warning.
pub(crate) fn unignored_directory(cwd: &Path, path: &Path) -> Option<String> {
    let primary = worktrees::primary_checkout(cwd).ok()?;
    let below = path
        .strip_prefix(&primary)
        .ok()
        .map(Path::to_path_buf)
        .or_else(|| {
            canonical(path)
                .strip_prefix(canonical(&primary))
                .ok()
                .map(Path::to_path_buf)
        })?;
    let std::path::Component::Normal(first) = below.components().next()? else {
        return None;
    };
    let first = first.to_str()?;
    (!worktrees::directory_ignored(&primary, first).ok()?).then(|| first.to_string())
}

fn create(
    cwd: &Path,
    branch: &str,
    base: &str,
    path: Option<PathBuf>,
    directory: Option<&Path>,
) -> Result<WorktreeInfo, String> {
    let path = match path {
        Some(path) => path,
        None => worktrees::default_path(cwd, branch, directory)?,
    };
    if !path.is_absolute() {
        return Err("worktree path must be absolute on the session host".to_string());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    worktrees::create(cwd, branch, base, &path)
}

fn remove(
    cwd: &Path,
    path: &Path,
    force: bool,
    unlock_stale: bool,
    delete_branch: worktrees::BranchDeletion,
) -> Result<Vec<worktrees::BranchOutcome>, String> {
    if !path.is_absolute() {
        return Err("worktree path must be absolute on the session host".to_string());
    }
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let users = super::discovery::sessions_using_worktree(&canonical.to_string_lossy())?;
    if !users.is_empty() {
        return Err(format!(
            "worktree is used by session {}; stop or forget it before removal",
            users.join(", ")
        ));
    }
    worktrees::remove(cwd, &canonical, force, unlock_stale, delete_branch)
}

fn unlock(cwd: &Path, path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("worktree path must be absolute on the session host".to_string());
    }
    worktrees::unlock(cwd, path)
}

/// One `rozi worktrees` call, as the host that owns the repository receives it.
///
/// Paths may be relative or start with `~`; they are resolved on that host, never by a client that
/// may run a different operating system. `cwd` defaults to the host process's working directory;
/// it locates the repository and anchors relative paths. Removal itself runs from the primary
/// checkout of the worktree being removed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum HostCall {
    List {
        cwd: Option<String>,
    },
    Create {
        cwd: Option<String>,
        branch: String,
        base: String,
        path: Option<String>,
    },
    Remove {
        cwd: Option<String>,
        checkout: CheckoutSelector,
        force: bool,
        /// `None` follows this host's `[worktrees] delete_branch`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delete_branch: Option<worktrees::BranchDeletion>,
    },
    Unlock {
        cwd: Option<String>,
        checkout: CheckoutSelector,
    },
    /// Add a top-level directory of the repository to `.git/info/exclude`: `directory`, or the
    /// repository-relative `[worktrees] directory` of this host.
    Exclude {
        cwd: Option<String>,
        directory: Option<String>,
    },
    /// Find the selected checkout and the sessions that record it as their origin.
    Resolve {
        cwd: Option<String>,
        checkout: CheckoutSelector,
    },
}

/// How a call names one checkout. Each form resolves on the host, against the call's `cwd`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckoutSelector {
    /// Any path inside the checkout.
    Path(String),
    /// The branch the checkout has checked out.
    Branch(String),
    /// An operand as the user typed it. One that reads as a path (`./`, `../`, `/`, `~`) is only a
    /// path; any other is tried as both, and fails when the two name different checkouts.
    Auto(String),
}

/// A listed checkout with the sessions that record it as their origin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedWorktree {
    #[serde(flatten)]
    pub worktree: WorktreeInfo,
    pub sessions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum HostReply {
    Listed {
        worktrees: Vec<ListedWorktree>,
    },
    Created {
        worktree: WorktreeInfo,
        /// The repository's top-level directory the checkout sits in, when Git does not ignore it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unignored: Option<String>,
    },
    Removed {
        path: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        branches: Vec<worktrees::BranchOutcome>,
    },
    Unlocked {
        path: String,
    },
    Excluded {
        directory: String,
    },
    Resolved {
        worktree: WorktreeInfo,
        sessions: Vec<String>,
        /// Every checkout of the repository, for rebasing a profile onto `worktree`.
        checkouts: Vec<String>,
    },
    Failed {
        message: String,
    },
}

pub fn run_host_call(call: HostCall) -> HostReply {
    let reply = match call {
        HostCall::List { cwd } => {
            host_path(cwd.as_deref().unwrap_or(".")).and_then(|cwd| list_with_sessions(&cwd))
        }
        HostCall::Create {
            cwd,
            branch,
            base,
            path,
        } => (|| {
            let cwd = host_path(cwd.as_deref().unwrap_or("."))?;
            let path = path.as_deref().map(host_path).transpose()?;
            let directory = configured_directory();
            create(&cwd, &branch, &base, path, directory.as_deref()).map(|worktree| {
                HostReply::Created {
                    unignored: unignored_directory(&cwd, Path::new(&worktree.path)),
                    worktree,
                }
            })
        })(),
        HostCall::Remove {
            cwd,
            checkout,
            force,
            delete_branch,
        } => (|| {
            let (tree, trees) = resolve_checkout(cwd.as_deref(), &checkout)?;
            let primary = primary_of(&trees)?;
            let delete_branch = delete_branch.unwrap_or_else(configured_delete_branch);
            let branches = remove(&primary, Path::new(&tree.path), force, false, delete_branch)?;
            Ok(HostReply::Removed {
                path: tree.path,
                branches,
            })
        })(),
        HostCall::Unlock { cwd, checkout } => (|| {
            let (tree, trees) = resolve_checkout(cwd.as_deref(), &checkout)?;
            unlock(&primary_of(&trees)?, Path::new(&tree.path))?;
            Ok(HostReply::Unlocked { path: tree.path })
        })(),
        HostCall::Exclude { cwd, directory } => (|| {
            let cwd = host_path(cwd.as_deref().unwrap_or("."))?;
            let directory = match directory {
                Some(directory) => directory,
                None => repo_relative_directory()?,
            };
            let directory = directory.trim_end_matches(['/', '\\']).to_string();
            worktrees::exclude_directory(&cwd, &directory)?;
            Ok(HostReply::Excluded { directory })
        })(),
        HostCall::Resolve { cwd, checkout } => (|| {
            let (tree, trees) = resolve_checkout(cwd.as_deref(), &checkout)?;
            let sessions = super::discovery::worktree_session_origins()?
                .sessions_at(&canonical(Path::new(&tree.path)));
            Ok(HostReply::Resolved {
                worktree: tree,
                sessions,
                checkouts: trees.into_iter().map(|tree| tree.path).collect(),
            })
        })(),
    };
    reply.unwrap_or_else(|message| HostReply::Failed { message })
}

/// The top-level repository directory a relative `[worktrees] directory` keeps checkouts in.
fn repo_relative_directory() -> Result<String, String> {
    let directory = configured_directory();
    match worktrees::checkout_root(directory.as_deref())? {
        worktrees::CheckoutRoot::InRepository(folder) => Ok(folder.to_string()),
        _ => Err(
            "[worktrees] directory is not inside the repository; name a directory to exclude"
                .to_string(),
        ),
    }
}

/// `[worktrees] directory` from this host's config, expanded here.
pub(crate) fn configured_directory() -> Option<PathBuf> {
    crate::config::load_config()
        .config
        .worktrees
        .directory
        .map(crate::config::expand_path)
}

/// `[worktrees] delete_branch` from this host's config.
fn configured_delete_branch() -> worktrees::BranchDeletion {
    crate::config::load_config().config.worktrees.delete_branch
}

fn list_with_sessions(cwd: &Path) -> Result<HostReply, String> {
    let trees = worktrees::list(cwd)?;
    let origins = super::discovery::worktree_session_origins()?;
    let worktrees = trees
        .into_iter()
        .map(|worktree| {
            let sessions = origins.sessions_at(&canonical(Path::new(&worktree.path)));
            ListedWorktree { worktree, sessions }
        })
        .collect();
    Ok(HostReply::Listed { worktrees })
}

/// A selected checkout, with every checkout of its repository (primary first).
type ResolvedCheckout = (WorktreeInfo, Vec<WorktreeInfo>);

/// The checkout `selector` names. `cwd` locates the repository for a branch and anchors a relative
/// path; an absolute path may name a checkout of any repository.
fn resolve_checkout(
    cwd: Option<&str>,
    selector: &CheckoutSelector,
) -> Result<ResolvedCheckout, String> {
    let base = host_path(cwd.unwrap_or("."))?;
    match selector {
        CheckoutSelector::Path(path) => containing_worktree(&host_path_in(path, &base)?),
        CheckoutSelector::Auto(operand) if explicit_path(operand) => {
            containing_worktree(&host_path_in(operand, &base)?)
        }
        CheckoutSelector::Branch(branch) => {
            let (found, trees) = branch_checkouts(&base, branch)?;
            single_branch_checkout(branch, found, trees)?.ok_or_else(|| {
                format!(
                    "no checkout of {} has branch `{branch}` checked out",
                    trees_label(&base)
                )
            })
        }
        CheckoutSelector::Auto(operand) => resolve_operand(&base, operand),
    }
}

/// An operand that only a path can be: Git refuses `~`, `\`, a leading `/`, and `.` or `..` as a
/// path component in branch names, and a drive prefix is absolute only on Windows.
fn explicit_path(operand: &str) -> bool {
    ["/", "\\", "~", "./", "../", ".\\", "..\\"]
        .iter()
        .any(|prefix| operand.starts_with(prefix))
        || matches!(operand, "." | "..")
        || Path::new(operand).is_absolute()
}

/// An operand tried both as a path and as a branch. Agreement or a single match selects a
/// checkout; two different checkouts are an error rather than a choice by precedence.
fn resolve_operand(base: &Path, operand: &str) -> Result<ResolvedCheckout, String> {
    let path = host_path_in(operand, base)?;
    let by_path = path.exists().then(|| containing_worktree(&path));
    let by_branch = match branch_checkouts(base, operand) {
        Ok((found, trees)) => Ok(single_branch_checkout(operand, found, trees)?),
        Err(err) => Err(err),
    };
    match (by_path, by_branch) {
        (Some(Ok(by_path)), Ok(Some(by_branch))) => {
            if worktrees::same_path(Path::new(&by_path.0.path), Path::new(&by_branch.0.path)) {
                Ok(by_path)
            } else {
                Err(ambiguous_operand(operand, &by_path.0, &by_branch.0))
            }
        }
        (Some(Ok(by_path)), _) => Ok(by_path),
        (_, Ok(Some(by_branch))) => Ok(by_branch),
        (by_path, by_branch) => {
            let path_reason = match by_path {
                Some(Err(err)) => err,
                _ => format!("{} does not exist", path.display()),
            };
            let branch_reason = match by_branch {
                Err(err) => err,
                Ok(_) => format!(
                    "no checkout of {} has branch `{operand}` checked out",
                    trees_label(base)
                ),
            };
            Err(format!(
                "no checkout matches `{operand}`: {path_reason}, and {branch_reason}"
            ))
        }
    }
}

fn ambiguous_operand(operand: &str, by_path: &WorktreeInfo, by_branch: &WorktreeInfo) -> String {
    format!(
        "`{operand}` names two checkouts:\n  \
         as a path:   {}\n  \
         as a branch: {}\n\
         use `./{operand}` for the path or `--branch {operand}` for the branch",
        describe_checkout(by_path),
        describe_checkout(by_branch),
    )
}

/// A checkout as an error message names it: its path, then its branch and whether it is primary.
fn describe_checkout(tree: &WorktreeInfo) -> String {
    let branch = match &tree.branch {
        Some(branch) => format!("branch {branch}"),
        None => "detached".to_string(),
    };
    if tree.linked {
        format!("{} ({branch})", tree.path)
    } else {
        format!("{} ({branch}, primary checkout)", tree.path)
    }
}

/// The checkouts of the repository around `cwd` that have `branch` checked out, and all of them.
fn branch_checkouts(
    cwd: &Path,
    branch: &str,
) -> Result<(Vec<WorktreeInfo>, Vec<WorktreeInfo>), String> {
    if branch.is_empty() {
        return Err("branch name cannot be empty".to_string());
    }
    let trees = worktrees::list(cwd)?;
    let found = trees
        .iter()
        .filter(|tree| tree.branch.as_deref() == Some(branch))
        .cloned()
        .collect();
    Ok((found, trees))
}

/// Git checks a branch out once unless forced; a forced second checkout is named by path instead.
fn single_branch_checkout(
    branch: &str,
    found: Vec<WorktreeInfo>,
    trees: Vec<WorktreeInfo>,
) -> Result<Option<ResolvedCheckout>, String> {
    match <[WorktreeInfo; 1]>::try_from(found) {
        Ok([tree]) => Ok(Some((tree, trees))),
        Err(found) if found.is_empty() => Ok(None),
        Err(found) => Err(format!(
            "branch `{branch}` is checked out in several checkouts ({}); name one by path",
            found
                .iter()
                .map(|tree| tree.path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The repository around `cwd` as a message names it: its primary checkout when Git finds one.
fn trees_label(cwd: &Path) -> String {
    worktrees::primary_checkout(cwd)
        .unwrap_or_else(|_| cwd.to_path_buf())
        .display()
        .to_string()
}

fn primary_of(trees: &[WorktreeInfo]) -> Result<PathBuf, String> {
    trees
        .first()
        .map(|primary| PathBuf::from(&primary.path))
        .ok_or_else(|| "Git reported no primary worktree".to_string())
}

/// The registered checkout that contains `path`, preferring the innermost when checkouts nest,
/// with every checkout of its repository (primary first).
fn containing_worktree(path: &Path) -> Result<ResolvedCheckout, String> {
    let path = canonical(path);
    let trees = worktrees::list(&path)?;
    let tree = trees
        .iter()
        .filter(|tree| path.starts_with(canonical(Path::new(&tree.path))))
        .max_by_key(|tree| tree.path.len())
        .cloned()
        .ok_or_else(|| format!("{} is not inside a Git worktree", path.display()))?;
    Ok((tree, trees))
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// The name a new session for a checkout starts from: `wt-` and its branch, or its directory name
/// when detached. Callers add a numeric suffix when the name is taken.
pub(crate) fn session_name_base(branch: Option<&str>, path: &str) -> String {
    let tail = branch.unwrap_or_else(|| path.rsplit(['/', '\\']).next().unwrap_or("worktree"));
    let slug: String = tail
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .take(40)
        .collect();
    match slug.trim_matches('-') {
        "" => "wt-worktree".to_string(),
        slug => format!("wt-{slug}"),
    }
}

/// The first free name from [`session_name_base`], given the names already taken.
pub(crate) fn unused_session_name(base: &str, taken: impl Fn(&str) -> bool) -> Option<String> {
    (1..=9999)
        .map(|suffix| match suffix {
            1 => base.to_string(),
            suffix => format!("{base}-{suffix}"),
        })
        .find(|name| !taken(name))
}

/// Resolve a path the way a shell on this host would have: `~` is this host's home directory, and
/// a relative path is relative to this process's working directory.
pub fn host_path(path: &str) -> Result<PathBuf, String> {
    let path = expand_home(path)?;
    if path.is_absolute() {
        return Ok(path);
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .map_err(|err| format!("cannot resolve a relative path: {err}"))
}

/// [`host_path`] with a relative path anchored at `base` instead of this process's directory.
fn host_path_in(path: &str, base: &Path) -> Result<PathBuf, String> {
    Ok(base.join(expand_home(path)?))
}

fn expand_home(path: &str) -> Result<PathBuf, String> {
    if path.is_empty() {
        return Err("path cannot be empty".to_string());
    }
    let rest = path
        .strip_prefix('~')
        .filter(|rest| rest.is_empty() || rest.starts_with(['/', std::path::MAIN_SEPARATOR]));
    match rest {
        Some(rest) => {
            let home = crate::platform::paths::home_directory()
                .ok_or_else(|| "cannot expand `~`: no home directory".to_string())?;
            Ok(PathBuf::from(format!("{home}{rest}")))
        }
        None => Ok(PathBuf::from(path)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_paths_expand_home_and_anchor_relative_paths_here() {
        let cwd = std::env::current_dir().expect("cwd");
        assert_eq!(host_path("repo").expect("relative"), cwd.join("repo"));
        if let Some(home) = crate::platform::paths::home_directory() {
            assert_eq!(
                host_path("~/src/repo").expect("home"),
                PathBuf::from(format!("{home}/src/repo"))
            );
            assert_eq!(host_path("~").expect("home"), PathBuf::from(&home));
        }
        // Only a leading `~` names a home directory; `~user` is not expanded.
        assert_eq!(host_path("~other").expect("relative"), cwd.join("~other"));
        assert!(host_path("").is_err());
    }

    fn git(cwd: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
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

    /// A repository at `repo` with one commit and a linked checkout of a new `feat/cli` branch.
    fn repo_with_feature_checkout(repo: &Path) -> WorktreeInfo {
        std::fs::create_dir(repo).unwrap();
        git(repo, &["init", "-q"]);
        git(
            repo,
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
        let HostReply::Created { worktree, .. } = run_host_call(HostCall::Create {
            cwd: Some(repo.to_string_lossy().into_owned()),
            branch: "feat/cli".into(),
            base: "HEAD".into(),
            path: None,
        }) else {
            panic!("create failed");
        };
        worktree
    }

    fn resolve(cwd: &Path, checkout: CheckoutSelector) -> Result<WorktreeInfo, String> {
        match run_host_call(HostCall::Resolve {
            cwd: Some(cwd.to_string_lossy().into_owned()),
            checkout,
        }) {
            HostReply::Resolved { worktree, .. } => Ok(worktree),
            HostReply::Failed { message } => Err(message),
            other => panic!("unexpected reply {other:?}"),
        }
    }

    /// The CLI names a checkout by any path inside it and never needs the repository's own root:
    /// removal runs from the primary checkout, and resolution finds the innermost containing tree.
    #[test]
    fn host_calls_resolve_checkouts_from_any_path_inside_them() {
        if !crate::platform::command::program_exists("git") {
            return;
        }
        crate::test_support::isolate_user_dirs();
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let worktree = repo_with_feature_checkout(&repo);
        let repo_str = repo.to_string_lossy().into_owned();
        let checkout = canonical(&temp.path().join("repo-worktrees").join("feat-cli"));
        assert_eq!(canonical(Path::new(&worktree.path)), checkout);

        let HostReply::Listed { worktrees } = run_host_call(HostCall::List {
            cwd: Some(worktree.path.clone()),
        }) else {
            panic!("list failed");
        };
        assert_eq!(worktrees.len(), 2);
        assert!(worktrees.iter().all(|tree| tree.sessions.is_empty()));

        let nested = checkout.join("src");
        std::fs::create_dir(&nested).unwrap();
        let HostReply::Resolved {
            worktree: resolved,
            sessions,
            checkouts,
        } = run_host_call(HostCall::Resolve {
            cwd: None,
            checkout: CheckoutSelector::Path(nested.to_string_lossy().into_owned()),
        })
        else {
            panic!("resolve failed");
        };
        assert_eq!(resolved, worktree);
        assert!(sessions.is_empty());
        assert_eq!(checkouts.len(), 2);
        assert!(worktrees::same_path(Path::new(&checkouts[0]), &repo));
        assert_eq!(checkouts[1], worktree.path);

        assert!(matches!(
            run_host_call(HostCall::Remove {
                cwd: None,
                checkout: CheckoutSelector::Auto(repo_str.clone()),
                force: true,
                delete_branch: None,
            }),
            HostReply::Failed { .. }
        ));
        assert_eq!(
            run_host_call(HostCall::Remove {
                cwd: None,
                checkout: CheckoutSelector::Auto(nested.to_string_lossy().into_owned()),
                force: true,
                delete_branch: Some(worktrees::BranchDeletion::NONE),
            }),
            HostReply::Removed {
                path: worktree.path.clone(),
                branches: Vec::new(),
            }
        );
        assert!(!checkout.exists());
        git(&repo, &["show-ref", "--verify", "refs/heads/feat/cli"]);
    }

    /// A bare operand is tried as a path and as a branch from `cwd`. Either match alone selects
    /// its checkout, both on the same checkout agree, and two different checkouts are refused.
    #[test]
    fn operands_resolve_by_branch_or_path_and_refuse_to_guess() {
        if !crate::platform::command::program_exists("git") {
            return;
        }
        crate::test_support::isolate_user_dirs();
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let worktree = repo_with_feature_checkout(&repo);
        let is_primary = |tree: &WorktreeInfo| worktrees::same_path(Path::new(&tree.path), &repo);
        let auto = |operand: &str| CheckoutSelector::Auto(operand.into());
        let branch = || CheckoutSelector::Branch("feat/cli".into());

        assert_eq!(resolve(&repo, auto("feat/cli")), Ok(worktree.clone()));
        assert_eq!(resolve(&repo, branch()), Ok(worktree.clone()));
        let missing = resolve(&repo, auto("feat/none")).expect_err("matches nothing");
        assert!(
            missing.starts_with("no checkout matches `feat/none`:"),
            "{missing}"
        );
        // An explicit path never falls back to the branch of the same name.
        assert!(resolve(&repo, auto("./feat/cli")).is_err());

        // A `feat/cli` directory in the primary checkout makes the bare operand name two checkouts.
        std::fs::create_dir_all(repo.join("feat").join("cli")).unwrap();
        let ambiguous = resolve(&repo, auto("feat/cli")).expect_err("ambiguous");
        assert!(
            ambiguous.starts_with("`feat/cli` names two checkouts:"),
            "{ambiguous}"
        );
        for expected in [
            "primary checkout)",
            &format!("{} (branch feat/cli)", worktree.path),
            "`./feat/cli` for the path or `--branch feat/cli` for the branch",
        ] {
            assert!(ambiguous.contains(expected), "{ambiguous}");
        }
        assert!(resolve(&repo, auto("./feat/cli")).is_ok_and(|tree| is_primary(&tree)));
        assert_eq!(resolve(&repo, branch()), Ok(worktree.clone()));

        // From inside the linked checkout, its own `feat/cli` directory and branch agree.
        let checkout = PathBuf::from(&worktree.path);
        std::fs::create_dir_all(checkout.join("feat").join("cli")).unwrap();
        assert_eq!(resolve(&checkout, auto("feat/cli")), Ok(worktree.clone()));

        // `cwd` anchors a relative path but never confines an absolute one.
        assert!(resolve(&checkout, auto("../../repo")).is_ok_and(|tree| is_primary(&tree)));
        assert!(
            resolve(temp.path(), auto(&repo.to_string_lossy())).is_ok_and(|tree| is_primary(&tree))
        );

        assert_eq!(
            run_host_call(HostCall::Remove {
                cwd: Some(repo.to_string_lossy().into_owned()),
                checkout: branch(),
                force: true,
                delete_branch: Some(worktrees::BranchDeletion::NONE),
            }),
            HostReply::Removed {
                path: worktree.path.clone(),
                branches: Vec::new(),
            }
        );
    }

    #[test]
    fn operands_that_read_as_paths_are_never_branches() {
        for operand in ["./x", "../x", "/x", "~", "~/x", ".", "..", ".\\x"] {
            assert!(explicit_path(operand), "{operand}");
        }
        for operand in ["main", "feat/login", "x/./y", "fix-~"] {
            assert!(!explicit_path(operand), "{operand}");
        }
    }

    #[test]
    fn checkouts_are_described_by_path_branch_and_role() {
        let tree = |branch: Option<&str>, linked| WorktreeInfo {
            path: "/src/repo".into(),
            branch: branch.map(Into::into),
            detached: branch.is_none(),
            bare: false,
            prunable: false,
            linked,
            lock: None,
        };
        assert_eq!(
            describe_checkout(&tree(Some("main"), false)),
            "/src/repo (branch main, primary checkout)"
        );
        assert_eq!(describe_checkout(&tree(None, true)), "/src/repo (detached)");
    }

    /// A checkout created inside the repository reports that Git does not ignore its directory,
    /// until that directory is excluded; one beside the repository never does.
    #[test]
    fn in_repository_checkouts_report_an_unignored_directory() {
        if !crate::platform::command::program_exists("git") {
            return;
        }
        crate::test_support::isolate_user_dirs();
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
        let cwd = repo.to_string_lossy().into_owned();
        let create = |branch: &str, directory: Option<&Path>| {
            execute(
                WorktreeRequest::Create {
                    cwd: cwd.clone(),
                    branch: branch.into(),
                    base: "HEAD".into(),
                    path: None,
                },
                directory,
            )
        };
        let in_repo = Path::new(".worktrees");

        let WorktreeResult::Previewed { path, unignored } = execute(
            WorktreeRequest::Preview {
                cwd: cwd.clone(),
                branch: "feat/a".into(),
            },
            Some(in_repo),
        ) else {
            panic!("preview failed");
        };
        // Git and `canonicalize` spell Windows paths differently, so compare the directories.
        let path = Path::new(&path);
        assert!(
            path.ends_with(Path::new(".worktrees").join("feat-a")),
            "{path:?}"
        );
        assert!(worktrees::same_path(
            path.parent().and_then(Path::parent).unwrap(),
            &repo
        ));
        assert_eq!(unignored.as_deref(), Some(".worktrees"));

        let WorktreeResult::Created { unignored, .. } = create("feat/a", Some(in_repo)) else {
            panic!("create failed");
        };
        assert_eq!(unignored.as_deref(), Some(".worktrees"));
        assert_eq!(
            execute(
                WorktreeRequest::Exclude {
                    cwd: cwd.clone(),
                    directory: ".worktrees".into(),
                },
                None,
            ),
            WorktreeResult::Excluded {
                directory: ".worktrees".into()
            }
        );
        let WorktreeResult::Created { unignored, .. } = create("feat/b", Some(in_repo)) else {
            panic!("create failed");
        };
        assert_eq!(unignored, None);

        let WorktreeResult::Created { unignored, .. } = create("feat/c", None) else {
            panic!("create failed");
        };
        assert_eq!(
            unignored, None,
            "a sibling checkout is outside the repository"
        );
    }

    #[test]
    fn a_reply_without_the_ignore_warning_still_decodes() {
        let reply: WorktreeResult =
            serde_json::from_value(serde_json::json!({"kind": "previewed", "path": "/wt/x"}))
                .expect("decodes");
        assert_eq!(
            reply,
            WorktreeResult::Previewed {
                path: "/wt/x".into(),
                unignored: None
            }
        );
    }

    #[test]
    fn host_calls_and_replies_have_a_stable_json_shape() {
        let call = HostCall::Remove {
            cwd: None,
            checkout: CheckoutSelector::Path("C:\\code\\repo-worktrees\\feat".into()),
            force: true,
            delete_branch: Some(worktrees::BranchDeletion {
                local: true,
                remote: false,
            }),
        };
        let json = serde_json::to_value(&call).expect("encodes");
        assert_eq!(
            json,
            serde_json::json!({"op": "remove", "cwd": null, "checkout": {"path": "C:\\code\\repo-worktrees\\feat"}, "force": true, "delete_branch": {"local": true, "remote": false}})
        );
        let call = HostCall::Unlock {
            cwd: Some("~/src/repo".into()),
            checkout: CheckoutSelector::Branch("feat/login".into()),
        };
        let json = serde_json::to_value(&call).expect("encodes");
        assert_eq!(
            json,
            serde_json::json!({"op": "unlock", "cwd": "~/src/repo", "checkout": {"branch": "feat/login"}})
        );
        assert_eq!(
            serde_json::from_value::<HostCall>(json).expect("decodes"),
            call
        );
        let reply = HostReply::Listed {
            worktrees: vec![ListedWorktree {
                worktree: WorktreeInfo {
                    path: "/src/repo".into(),
                    branch: Some("main".into()),
                    detached: false,
                    bare: false,
                    prunable: false,
                    linked: false,
                    lock: None,
                },
                sessions: vec!["dev".into()],
            }],
        };
        let json = serde_json::to_value(&reply).expect("encodes");
        assert_eq!(json["kind"], "listed");
        assert_eq!(json["worktrees"][0]["path"], "/src/repo");
        assert_eq!(json["worktrees"][0]["sessions"][0], "dev");
        assert_eq!(
            serde_json::from_value::<HostReply>(json).expect("decodes"),
            reply
        );
    }
}
