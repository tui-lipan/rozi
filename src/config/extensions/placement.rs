//! Where an extension's processes execute.
//!
//! Every contribution that starts a process runs on some machine, and that machine decides what
//! its working directory, its platform, and its executables mean. API 1 contributions run on the
//! client, beside the UI, and still do by default. A manifest may instead place a contribution on
//! the host of a session: then the process runs there, from a bundle of the files this client
//! loaded, and talks back to this client's UI through Rozi rather than through anything of its own.
//!
//! A placed process is a *worker*. Whatever machine it runs on, it holds a credential that binds it
//! to its extension, its generation, and the host (and session) it was placed for, and the UI only
//! honors it within that binding. See `crate::ops::extension_workers`.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::config::{ServiceRestart, SidebarTabId};
use crate::extension_runtime::bundle::SharedBundle;

/// Where a contribution's process runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Placement {
    /// On the client, beside the UI. The API 1 default.
    Client,
    /// On the host of the session on screen, following it when the user switches sessions.
    ActiveSession,
    /// One instance on every host this client has a session attached on.
    EachHost,
    /// One instance per attached session, on that session's host.
    EachSession,
}

impl Placement {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::ActiveSession => "active-session",
            Self::EachHost => "each-host",
            Self::EachSession => "each-session",
        }
    }

    pub fn is_client(self) -> bool {
        self == Self::Client
    }

    /// Parse a manifest value. A command or a sidebar tab runs once per invocation, so it can only
    /// follow the session on screen; the per-host and per-session forms describe long-running
    /// instances and belong to services.
    pub(super) fn parse(raw: Option<&str>, service: bool) -> Result<Self, String> {
        let value = raw.map(str::trim).unwrap_or("client");
        let placement = match value {
            "client" => Self::Client,
            "active-session" => Self::ActiveSession,
            "each-host" if service => Self::EachHost,
            "each-session" if service => Self::EachSession,
            "each-host" | "each-session" => {
                return Err(format!(
                    "placement `{value}` is only for services; a command runs once, on the \
                     client or on the active session's host"
                ));
            }
            other => {
                let allowed = if service {
                    "client, active-session, each-host, each-session"
                } else {
                    "client, active-session"
                };
                return Err(format!("unknown placement `{other}` (allowed: {allowed})"));
            }
        };
        Ok(placement)
    }
}

/// A process launch written against the extension's own files rather than a directory on any one
/// machine. `{extension_dir}` stays a placeholder and a `./` program stays relative until a host
/// resolves them against wherever the extension's files are on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchTemplate {
    /// Direct argv. The program is a bare name looked up on the executing host's `PATH`, or a
    /// `./`-relative path inside the extension.
    Direct(Vec<String>),
    /// A command line for the executing host's command shell.
    Shell(String),
}

impl LaunchTemplate {
    /// Substitute `extension_dir` for the placeholder and resolve a relative program against it.
    pub fn resolve(&self, extension_dir: &std::path::Path) -> LaunchTemplate {
        let dir = extension_dir.to_string_lossy();
        match self {
            Self::Direct(argv) => {
                let mut argv: Vec<String> = argv
                    .iter()
                    .map(|argument| argument.replace("{extension_dir}", &dir))
                    .collect();
                if let Some(program) = argv.first_mut()
                    && let Some(relative) = relative_program(program)
                {
                    let mut path = extension_dir.to_path_buf();
                    for component in relative.split(['/', '\\']).filter(|part| !part.is_empty()) {
                        if component != "." {
                            path.push(component);
                        }
                    }
                    *program = path.to_string_lossy().to_string();
                }
                Self::Direct(argv)
            }
            Self::Shell(line) => Self::Shell(line.replace("{extension_dir}", &dir)),
        }
    }

    /// The program a direct launch names, before resolution. `None` for a shell line.
    pub fn program(&self) -> Option<&str> {
        match self {
            Self::Direct(argv) => argv.first().map(String::as_str),
            Self::Shell(_) => None,
        }
    }
}

/// `./x` and `.\x` name a file inside the extension. Returns the part after the marker.
pub(crate) fn relative_program(program: &str) -> Option<&str> {
    program
        .strip_prefix("./")
        .or_else(|| program.strip_prefix(".\\"))
}

/// Where a placed service starts: a directory inside the extension's files, `.` for their root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacedService {
    /// Public name, `<extension>.<service>`.
    pub name: String,
    pub placement: Placement,
    pub launch: LaunchTemplate,
    /// Relative to the extension's files on the executing host.
    pub cwd: String,
    pub restart: ServiceRestart,
    /// Declared and Rozi-injected environment, without `ROZI_EXTENSION_DIR`, which differs per
    /// host and is set where the process starts.
    pub env: BTreeMap<String, String>,
}

/// A command that runs on the active session's host. The [`crate::config::NamedCommand`] of the
/// same id keeps everything else about it (label, key, palette visibility).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacedCommand {
    pub placement: Placement,
    pub launch: LaunchTemplate,
}

/// A command-form sidebar tab whose listing command, and `on_click` `exec`, run on the active
/// session's host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacedTab {
    pub placement: Placement,
    /// The listing command line, with `{extension_dir}` still a placeholder.
    pub command: String,
    /// The `on_click` `exec` line, with `{extension_dir}` still a placeholder. Other click actions
    /// (`run`, `popup`, `send`) already happen in a pane, on the session's host.
    pub on_click_exec: Option<String>,
}

/// Everything about one extension that runs away from the client, or that has to know where it
/// runs. Present only for an extension with at least one placed contribution.
#[derive(Clone, Debug)]
pub struct ExtensionPlacements {
    pub id: String,
    /// `[extension] platforms`, checked against each host a contribution executes on rather than
    /// against the client alone.
    pub platforms: Vec<String>,
    /// The installation directory on this client, for a placed contribution running here.
    pub local_dir: String,
    /// The files this load read, carried to any other host that runs a placed contribution.
    pub bundle: SharedBundle,
    pub commands: BTreeMap<String, PlacedCommand>,
    pub tabs: BTreeMap<SidebarTabId, PlacedTab>,
    pub services: Vec<PlacedService>,
    /// The client cannot run this extension's client contributions (its `platforms` excludes this
    /// machine), so only the placed ones were contributed.
    pub client_unsupported: Option<String>,
}

impl PartialEq for ExtensionPlacements {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.platforms == other.platforms
            && self.local_dir == other.local_dir
            && self.bundle.digest() == other.bundle.digest()
            && self.commands == other.commands
            && self.tabs == other.tabs
            && self.services == other.services
            && self.client_unsupported == other.client_unsupported
    }
}

impl Eq for ExtensionPlacements {}

impl ExtensionPlacements {
    /// Whether the extension can execute on a host running `os`, named as Rust names it.
    pub fn supports(&self, os: &str) -> bool {
        self.platforms.is_empty() || self.platforms.iter().any(|platform| platform == os)
    }

    pub fn bundle_digest(&self) -> &str {
        self.bundle.digest()
    }
}

pub type SharedPlacements = Arc<ExtensionPlacements>;

/// Placed contributions gathered while one manifest is validated.
#[derive(Debug, Default)]
pub(super) struct PlacedContributions {
    pub(super) commands: BTreeMap<String, PlacedCommand>,
    pub(super) tabs: BTreeMap<SidebarTabId, PlacedTab>,
    pub(super) services: Vec<PlacedService>,
}

impl PlacedContributions {
    pub(super) fn is_empty(&self) -> bool {
        self.commands.is_empty() && self.tabs.is_empty() && self.services.is_empty()
    }
}

/// Refuse a placed direct launch whose program names a file on the client.
///
/// A placed process runs on another machine, from the extension's own files. The only programs
/// that mean the same thing there are a name looked up on that host's `PATH` and a `./` path inside
/// the extension; an absolute path, `~`, or a `../` climb out of the extension all name something
/// on whichever machine wrote the manifest.
pub(super) fn check_host_program(argv: &[String], public_id: &str, errors: &mut Vec<String>) {
    let Some(program) = argv.first() else {
        return;
    };
    let portable = match relative_program(program) {
        Some(relative) => crate::extension_runtime::bundle::is_contained(relative),
        None => {
            !program.contains(['/', '\\'])
                && !program.starts_with('~')
                && !std::path::Path::new(program).is_absolute()
        }
    };
    if !portable {
        errors.push(format!(
            "`{public_id}` is placed on a session host, so its program must be a name on that \
             host's PATH or a `./` path inside the extension, not `{program}`"
        ));
    }
}

/// Validate a placed service's `cwd`: a directory inside the extension, as a relative path.
pub(super) fn host_relative_cwd(
    cwd: Option<&str>,
    public_id: &str,
    errors: &mut Vec<String>,
) -> String {
    let Some(cwd) = cwd.map(str::trim).filter(|cwd| !cwd.is_empty()) else {
        return ".".to_string();
    };
    if cwd.starts_with('~')
        || std::path::Path::new(cwd).is_absolute()
        || !crate::extension_runtime::bundle::is_contained(cwd)
    {
        errors.push(format!(
            "`{public_id}` is placed on a session host, so its `cwd` must be a directory inside \
             the extension, not `{cwd}`"
        ));
    }
    cwd.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_only_choose_between_the_client_and_the_active_session() {
        assert_eq!(Placement::parse(None, false).unwrap(), Placement::Client);
        assert_eq!(
            Placement::parse(Some("active-session"), false).unwrap(),
            Placement::ActiveSession
        );
        assert!(Placement::parse(Some("each-host"), false).is_err());
        assert_eq!(
            Placement::parse(Some("each-host"), true).unwrap(),
            Placement::EachHost
        );
        assert_eq!(
            Placement::parse(Some("each-session"), true).unwrap(),
            Placement::EachSession
        );
        assert!(Placement::parse(Some("everywhere"), true).is_err());
    }

    #[test]
    fn a_template_resolves_against_the_directory_of_the_host_running_it() {
        let launch = LaunchTemplate::Direct(vec![
            "./bin/run".to_string(),
            "{extension_dir}/data".to_string(),
        ]);
        let LaunchTemplate::Direct(argv) =
            launch.resolve(std::path::Path::new("/cache/bundles/abc"))
        else {
            unreachable!()
        };
        assert_eq!(
            argv,
            [
                std::path::Path::new("/cache/bundles/abc")
                    .join("bin")
                    .join("run")
                    .to_string_lossy()
                    .to_string(),
                "/cache/bundles/abc/data".to_string()
            ]
        );
        let bare = LaunchTemplate::Direct(vec!["python3".to_string()]);
        assert_eq!(bare.resolve(std::path::Path::new("/x")), bare);
    }
}
