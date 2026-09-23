//! `rozi worktrees`: Git checkouts on the host that owns a repository.
//!
//! Locally the call runs in this process, which already is that host. Under `--remote` it is
//! forwarded to the far host's rozi, which resolves every path with its own rules, so nothing here
//! interprets a path beyond printing it.

use super::args::{CliArgs, ListFormat, SessionCommand, WorktreesCli, WorktreesCommand};
use super::output::{OutputStyles, OutputTone, TableCell, format_table};
use crate::session;
use crate::session::remote::RemoteTarget;
use crate::session::worktrees::{HostCall, HostReply, ListedWorktree};

/// Run a worktrees command. `Some` is a session launch the caller should continue into: `open`,
/// and `create --open`, end by attaching a UI.
pub(crate) fn run_worktrees_cli(cli: WorktreesCli) -> Result<Option<CliArgs>, String> {
    let target = resolve_target(cli.remote.as_deref())?;
    match cli.command {
        WorktreesCommand::List { cwd, format } => {
            let worktrees = match call(target.as_ref(), HostCall::List { cwd })? {
                HostReply::Listed { worktrees } => worktrees,
                other => return Err(unexpected(other)),
            };
            match format {
                ListFormat::Json => print_json(&serde_json::json!({ "worktrees": worktrees }))?,
                ListFormat::Text => print!(
                    "{}",
                    format_worktrees_text(&worktrees, OutputStyles::detect())
                ),
            }
            Ok(None)
        }
        WorktreesCommand::Create {
            branch,
            base,
            path,
            cwd,
            format,
            open,
        } => {
            let call_value = HostCall::Create {
                cwd,
                branch,
                base,
                path,
            };
            let (worktree, unignored) = match call(target.as_ref(), call_value)? {
                HostReply::Created {
                    worktree,
                    unignored,
                } => (worktree, unignored),
                other => return Err(unexpected(other)),
            };
            if let Some(directory) = unignored.as_deref() {
                // A warning, never a fix: creating a checkout does not edit Git's ignore rules.
                eprintln!("warning: {directory}/ is not ignored by Git");
                eprintln!(
                    "hint: run `rozi{} worktrees exclude {directory}` to add it to .git/info/exclude",
                    cli.remote
                        .as_deref()
                        .map(|remote| if remote.is_empty() {
                            " --remote".to_string()
                        } else {
                            format!(" --remote {remote}")
                        })
                        .unwrap_or_default()
                );
            }
            if open {
                return open_checkout(target, cli.remote, cli.config_path, worktree.path, None)
                    .map(Some);
            }
            match format {
                ListFormat::Json => print_json(&match unignored {
                    Some(directory) => {
                        serde_json::json!({ "worktree": worktree, "unignored": directory })
                    }
                    None => serde_json::json!({ "worktree": worktree }),
                })?,
                ListFormat::Text => println!("{}", worktree.path),
            }
            Ok(None)
        }
        WorktreesCommand::Open { path, name } => {
            open_checkout(target, cli.remote, cli.config_path, path, name).map(Some)
        }
        WorktreesCommand::Exclude { directory, cwd } => {
            match call(target.as_ref(), HostCall::Exclude { cwd, directory })? {
                HostReply::Excluded { directory } => {
                    println!("{directory}/ is excluded in .git/info/exclude");
                    Ok(None)
                }
                other => Err(unexpected(other)),
            }
        }
        WorktreesCommand::Remove { path, force } => {
            match call(target.as_ref(), HostCall::Remove { path, force })? {
                HostReply::Removed { .. } => Ok(None),
                other => Err(unexpected(other)),
            }
        }
    }
}

/// Decide which session a checkout opens in, and hand back the launch that opens it.
///
/// One associated session is reopened; none gets a new session rooted at the checkout, recorded as
/// its origin. Several are ambiguous at a prompt that cannot ask, so the caller names one.
fn open_checkout(
    target: Option<RemoteTarget>,
    remote: Option<String>,
    config_path: Option<String>,
    path: String,
    name: Option<String>,
) -> Result<CliArgs, String> {
    let (worktree, sessions, checkouts) = match call(target.as_ref(), HostCall::Resolve { path })? {
        HostReply::Resolved {
            worktree,
            sessions,
            checkouts,
        } => (worktree, sessions, checkouts),
        other => return Err(unexpected(other)),
    };
    let mut launch = CliArgs {
        config_path,
        remote,
        ..CliArgs::default()
    };
    let name = match choose_session(name, &sessions, &worktree.path)? {
        OpenChoice::Existing(name) => {
            launch.attach_session = Some(name);
            return Ok(launch);
        }
        OpenChoice::Create(Some(name)) => name,
        OpenChoice::Create(None) => {
            let taken = session_names(target.as_ref())?;
            let base =
                session::worktrees::session_name_base(worktree.branch.as_deref(), &worktree.path);
            session::worktrees::unused_session_name(&base, |name| {
                taken.iter().any(|taken| taken == name)
            })
            .ok_or("no free session name for this checkout")?
        }
    };
    launch.attach_session = Some(name);
    launch.session_command = SessionCommand::New;
    launch.cwd = Some(worktree.path);
    launch.worktree_checkouts = Some(checkouts);
    Ok(launch)
}

#[derive(Debug, PartialEq, Eq)]
enum OpenChoice {
    /// Reopen a session that records this checkout as its origin.
    Existing(String),
    /// Create a session for the checkout, under this name or a derived one.
    Create(Option<String>),
}

/// A named session that already belongs to the checkout is reopened, and any other name is created
/// alongside. Without a name, the checkout's only session is reopened, and several are ambiguous at
/// a prompt that cannot ask.
fn choose_session(
    name: Option<String>,
    sessions: &[String],
    checkout: &str,
) -> Result<OpenChoice, String> {
    match (name, sessions) {
        (Some(name), _) if sessions.contains(&name) => Ok(OpenChoice::Existing(name)),
        (Some(name), _) => Ok(OpenChoice::Create(Some(name))),
        (None, []) => Ok(OpenChoice::Create(None)),
        (None, [only]) => Ok(OpenChoice::Existing(only.clone())),
        (None, many) => Err(format!(
            "{checkout} is used by sessions {}; choose one with --name",
            many.join(", ")
        )),
    }
}

fn session_names(target: Option<&RemoteTarget>) -> Result<Vec<String>, String> {
    let rows = match target {
        Some(target) => session::discovery::discover_sessions_from(
            &session::discovery::SessionSource::Remote(target.clone()),
            &crate::config::load_config().config.remote,
        ),
        None => session::discovery::discover_sessions_with_snapshots(),
    }
    .map_err(|err| format!("could not list sessions: {err}"))?;
    Ok(rows.into_iter().map(|row| row.name).collect())
}

fn call(target: Option<&RemoteTarget>, call: HostCall) -> Result<HostReply, String> {
    let reply = match target {
        None => session::worktrees::run_host_call(call),
        Some(target) => session::remote::worktrees::forward(
            target,
            &call,
            &crate::config::load_config().config.remote,
        )?,
    };
    match reply {
        HostReply::Failed { message } => Err(message),
        reply => Ok(reply),
    }
}

/// `--remote`'s argument as a host, with a bare `--remote` meaning `[remote] default_host`.
fn resolve_target(raw: Option<&str>) -> Result<Option<RemoteTarget>, String> {
    let raw = match raw {
        None => return Ok(None),
        Some("") => crate::config::load_config()
            .config
            .remote
            .default_host
            .ok_or("--remote requires a host alias or ssh:// URL (or set [remote] default_host)")?,
        Some(raw) => raw.to_string(),
    };
    session::remote::parse_remote_target(&raw).map(Some)
}

fn unexpected(reply: HostReply) -> String {
    format!("unexpected worktree reply: {reply:?}")
}

fn print_json(value: &serde_json::Value) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string(value).map_err(|err| err.to_string())?
    );
    Ok(())
}

pub(super) fn format_worktrees_text(rows: &[ListedWorktree], styles: OutputStyles) -> String {
    if rows.is_empty() {
        return format!(
            "{}\n",
            styles.paint("No worktrees found.", OutputTone::Muted)
        );
    }
    let table_rows = rows
        .iter()
        .map(|row| {
            let tree = &row.worktree;
            let branch = match (&tree.branch, tree.bare) {
                (_, true) => "(bare)".to_string(),
                (Some(branch), false) => branch.clone(),
                (None, false) => "(detached)".to_string(),
            };
            let mut state = vec![if tree.linked { "linked" } else { "primary" }];
            if tree.locked {
                state.push("locked");
            }
            if tree.prunable {
                state.push("prunable");
            }
            let tone = if tree.prunable {
                OutputTone::Warning
            } else if tree.linked {
                OutputTone::Plain
            } else {
                OutputTone::Accent
            };
            vec![
                TableCell::new(branch, OutputTone::Key),
                TableCell::new(state.join(","), tone),
                TableCell::new(
                    if row.sessions.is_empty() {
                        "—".to_string()
                    } else {
                        row.sessions.join(",")
                    },
                    OutputTone::Muted,
                ),
                TableCell::plain(tree.path.clone()),
            ]
        })
        .collect::<Vec<_>>();
    format_table(
        &["BRANCH", "STATE", "SESSIONS", "PATH"],
        &table_rows,
        styles,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::worktrees::WorktreeInfo;

    fn tree(path: &str, branch: Option<&str>, linked: bool) -> WorktreeInfo {
        WorktreeInfo {
            path: path.into(),
            branch: branch.map(Into::into),
            detached: branch.is_none(),
            bare: false,
            prunable: false,
            linked,
            locked: false,
        }
    }

    #[test]
    fn open_reuses_the_checkouts_session_and_asks_when_it_cannot_choose() {
        let sessions = vec!["wt-feat".to_string(), "review".to_string()];
        assert_eq!(
            choose_session(None, &[], "/wt/feat"),
            Ok(OpenChoice::Create(None))
        );
        assert_eq!(
            choose_session(None, &sessions[..1], "/wt/feat"),
            Ok(OpenChoice::Existing("wt-feat".into()))
        );
        assert_eq!(
            choose_session(Some("review".into()), &sessions, "/wt/feat"),
            Ok(OpenChoice::Existing("review".into()))
        );
        // A checkout may have several sessions; naming a new one adds it rather than refusing.
        assert_eq!(
            choose_session(Some("third".into()), &sessions, "/wt/feat"),
            Ok(OpenChoice::Create(Some("third".into())))
        );
        assert_eq!(
            choose_session(None, &sessions, "/wt/feat"),
            Err("/wt/feat is used by sessions wt-feat, review; choose one with --name".into())
        );
    }

    #[test]
    fn worktree_report_lists_state_and_sessions_with_paths_verbatim() {
        let rows = vec![
            ListedWorktree {
                worktree: tree("/src/rozi", Some("main"), false),
                sessions: vec!["dev".into()],
            },
            ListedWorktree {
                worktree: WorktreeInfo {
                    locked: true,
                    ..tree("C:\\wt\\feat", None, true)
                },
                sessions: Vec::new(),
            },
        ];
        assert_eq!(
            format_worktrees_text(&rows, OutputStyles::plain()),
            "BRANCH      STATE          SESSIONS  PATH\n\
             main        primary        dev       /src/rozi\n\
             (detached)  linked,locked  —         C:\\wt\\feat\n"
        );
        assert_eq!(
            format_worktrees_text(&[], OutputStyles::plain()),
            "No worktrees found.\n"
        );
    }
}
