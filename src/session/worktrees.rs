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
        WorktreeRequest::List { cwd } => {
            worktrees::list(Path::new(&cwd)).map(|worktrees| WorktreeResult::Listed { worktrees })
        }
        WorktreeRequest::Preview { cwd, branch } => {
            worktrees::default_path(Path::new(&cwd), &branch, directory).map(|path| {
                WorktreeResult::Previewed {
                    path: path.to_string_lossy().into_owned(),
                }
            })
        }
        WorktreeRequest::Create {
            cwd,
            branch,
            base,
            path,
        } => create(
            Path::new(&cwd),
            &branch,
            &base,
            path.map(PathBuf::from),
            directory,
        )
        .map(|worktree| WorktreeResult::Created { worktree }),
        WorktreeRequest::Remove { cwd, path, force } => {
            remove(Path::new(&cwd), Path::new(&path), force)
                .map(|()| WorktreeResult::Removed { path })
        }
    };
    result.unwrap_or_else(|message| WorktreeResult::Failed { message })
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

fn remove(cwd: &Path, path: &Path, force: bool) -> Result<(), String> {
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
    worktrees::remove(cwd, &canonical, force)
}

/// One `rozi worktrees` call, as the host that owns the repository receives it.
///
/// Paths may be relative or start with `~`; they are resolved on that host, never by a client that
/// may run a different operating system. `cwd` defaults to the host process's working directory,
/// and for removal to the primary checkout of the worktree being removed.
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
        path: String,
        force: bool,
    },
    /// Find the checkout containing `path` and the sessions that record it as their origin.
    Resolve {
        path: String,
    },
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
    },
    Removed {
        path: String,
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
            create(&cwd, &branch, &base, path, directory.as_deref())
                .map(|worktree| HostReply::Created { worktree })
        })(),
        HostCall::Remove { path, force } => (|| {
            let (tree, trees) = containing_worktree(&host_path(&path)?)?;
            let primary = trees
                .first()
                .map(|primary| PathBuf::from(&primary.path))
                .ok_or("Git reported no primary worktree")?;
            remove(&primary, Path::new(&tree.path), force)?;
            Ok(HostReply::Removed { path: tree.path })
        })(),
        HostCall::Resolve { path } => (|| {
            let (tree, trees) = containing_worktree(&host_path(&path)?)?;
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

/// `[worktrees] directory` from this host's config, expanded here.
pub(crate) fn configured_directory() -> Option<PathBuf> {
    crate::config::load_config()
        .config
        .worktrees
        .directory
        .map(crate::config::expand_path)
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

/// The registered checkout that contains `path`, preferring the innermost when checkouts nest,
/// with every checkout of its repository (primary first).
fn containing_worktree(path: &Path) -> Result<(WorktreeInfo, Vec<WorktreeInfo>), String> {
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
    if path.is_empty() {
        return Err("path cannot be empty".to_string());
    }
    let rest = path
        .strip_prefix('~')
        .filter(|rest| rest.is_empty() || rest.starts_with(['/', std::path::MAIN_SEPARATOR]));
    let path = match rest {
        Some(rest) => {
            let home = crate::platform::paths::home_directory()
                .ok_or_else(|| "cannot expand `~`: no home directory".to_string())?;
            PathBuf::from(format!("{home}{rest}"))
        }
        None => PathBuf::from(path),
    };
    if path.is_absolute() {
        return Ok(path);
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .map_err(|err| format!("cannot resolve a relative path: {err}"))
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
        let repo_str = repo.to_string_lossy().into_owned();

        let HostReply::Created { worktree } = run_host_call(HostCall::Create {
            cwd: Some(repo_str.clone()),
            branch: "feat/cli".into(),
            base: "HEAD".into(),
            path: None,
        }) else {
            panic!("create failed");
        };
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
            path: nested.to_string_lossy().into_owned(),
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
                path: repo_str.clone(),
                force: true,
            }),
            HostReply::Failed { .. }
        ));
        assert_eq!(
            run_host_call(HostCall::Remove {
                path: nested.to_string_lossy().into_owned(),
                force: true,
            }),
            HostReply::Removed {
                path: worktree.path.clone(),
            }
        );
        assert!(!checkout.exists());
        git(&repo, &["show-ref", "--verify", "refs/heads/feat/cli"]);
    }

    #[test]
    fn host_calls_and_replies_have_a_stable_json_shape() {
        let call = HostCall::Remove {
            path: "C:\\code\\repo-worktrees\\feat".into(),
            force: true,
        };
        let json = serde_json::to_value(&call).expect("encodes");
        assert_eq!(
            json,
            serde_json::json!({"op": "remove", "path": "C:\\code\\repo-worktrees\\feat", "force": true})
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
                    locked: false,
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
