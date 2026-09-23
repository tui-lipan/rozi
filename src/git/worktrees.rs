//! Git worktree lifecycle on the session server's host.
//!
//! Paths stay host-native here. A remote client must treat the returned strings as opaque.

use std::ffi::OsString;
use std::path::Path;

use super::command::{self, WORKTREE_LIST_TIMEOUT, WORKTREE_MUTATION_TIMEOUT};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

/// Remove a linked checkout. `force` only relaxes Git's dirty-checkout guard; it never removes a
/// Rozi session or deletes a branch. Session-origin protection belongs at the RPC boundary.
pub fn remove(cwd: &Path, path: &Path, force: bool) -> Result<(), String> {
    absolute_path(path)?;
    let worktree = list(cwd)?
        .into_iter()
        .find(|tree| same_path(Path::new(&tree.path), path))
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
