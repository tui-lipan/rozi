//! Parsing `rozi`'s argv into the [`ParsedCli`] shape `main.rs` dispatches on.
//!
//! Nothing here performs work: every variant is a decision already made, so a bad command line
//! fails before any socket, config file, or session is touched.

use std::path::PathBuf;

use crate::{control, session};

mod agents;
mod extensions;
mod layout;
mod sessions;
mod skill;
mod worktrees;

#[cfg(test)]
pub(super) use agents::HELP_SECTIONS as AGENTS_HELP_SECTIONS;
pub(crate) use agents::print_help as print_agents_help;
#[cfg(test)]
pub(super) use extensions::HELP_SECTIONS as EXTENSIONS_HELP_SECTIONS;
pub(crate) use extensions::print_check_help as print_extensions_check_help;
pub(crate) use extensions::print_help as print_extensions_help;
pub(crate) use extensions::print_install_help as print_extensions_install_help;
pub(crate) use extensions::print_remove_help as print_extensions_remove_help;
pub(crate) use extensions::print_update_help as print_extensions_update_help;
#[cfg(test)]
pub(super) use sessions::HELP_SECTIONS as SESSIONS_HELP_SECTIONS;
pub(crate) use sessions::print_help as print_sessions_help;
#[cfg(test)]
pub(super) use worktrees::HELP_SECTIONS as WORKTREES_HELP_SECTIONS;
pub(crate) use worktrees::print_help as print_worktrees_help;
pub(crate) use worktrees::{WorktreesCli, WorktreesCommand};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CliArgs {
    pub(crate) session_command: SessionCommand,
    pub(crate) profile: Option<String>,
    pub(crate) config_path: Option<String>,
    pub(crate) attach_session: Option<String>,
    /// Whether [`Self::attach_session`] was spelled `--session <NAME>` rather than positionally.
    ///
    /// The two are interchangeable for launching, and deliberately not for control: `rozi dev`
    /// starts a UI, so reading `rozi dev list-panes` as "talk to the session server called dev"
    /// would let one trailing word change what the command *is*. `--session` says it outright.
    pub(crate) session_flag_target: bool,
    /// Force the startup session picker, overriding whatever `[session] startup` selects. A target
    /// wins over it, and `--remote` skips it: the session lives on the far host, which local
    /// discovery does not describe.
    pub(crate) pick: bool,
    pub(crate) read_only: bool,
    /// SSH remote host alias or `ssh://` URL (`--remote`).
    pub(crate) remote: Option<String>,
    /// First-pane directory for a `sessions new` session, on the session's host (`--cwd`).
    pub(crate) cwd: Option<String>,
    /// Record [`Self::cwd`] as the session's worktree origin, with its repository's checkouts on the
    /// session host. Set by `worktrees open`, never by argv.
    pub(crate) worktree_checkouts: Option<Vec<String>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SessionCommand {
    #[default]
    Dwim,
    Attach,
    New,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ControlCli {
    pub(super) endpoint: ControlEndpoint,
    pub(super) request: control::ControlRequest,
    /// Explicit report format. Without one, a terminal gets human output and a pipe gets JSON.
    pub(super) output_format: Option<ListFormat>,
}

/// Which rozi a control command talks to.
///
/// Two endpoints answer the same commands. A UI endpoint belongs to a running rozi and can serve
/// everything, including the parts that only mean something on a screen. A session endpoint
/// belongs to a named session server and serves the subset that does not need one — which is what
/// makes a detached session scriptable at all.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ControlEndpoint {
    /// The local UI endpoint named by `--socket`, `ROZI_SOCKET`, or discovery.
    Ui(Option<PathBuf>),
    /// The named session server, reached with no UI in the picture (`--session <NAME>`).
    Session(String),
    /// A named session server on another host, reached over the SSH transport `--remote` attach
    /// already uses (`--remote <HOST> --session <NAME>`).
    Remote { target: String, session: String },
}

impl ControlEndpoint {
    /// Whether this endpoint is a session server, wherever it runs.
    ///
    /// The commands a session owns - waits, prompts, integration reports - are owned by it just as
    /// much when it is a hop away, so the checks that gate them cannot be a test for `Session`
    /// alone.
    pub(crate) fn is_session(&self) -> bool {
        matches!(self, Self::Session(_) | Self::Remote { .. })
    }
}

/// `rozi publish`: the stdio bridge a program uses to publish the activity rows running inside its
/// own pane.
///
/// This exists so a publisher needs no IPC code of its own. On Windows `ROZI_SOCKET` names a
/// discovery entry rather than the pipe itself, and the pipe name must be derived rather than read
/// out of it, so a program cannot portably open the endpoint directly. Bridging through the binary
/// that already owns endpoint discovery and its security checks keeps every publisher to plain
/// line-delimited JSON on stdin and stdout.
#[derive(Debug)]
pub(crate) struct PublishCli {
    pub(super) socket: Option<PathBuf>,
}

#[derive(Debug)]
pub(crate) struct SubscribeCli {
    pub(super) socket: Option<PathBuf>,
    pub(super) events: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct PickCli {
    pub title: Option<String>,
    pub placeholder: Option<String>,
    pub socket: Option<PathBuf>,
    /// Speak the wire format on both sides instead of plain lines. Plain mode exists so a shell
    /// pipeline needs no `jq` on either end; `--json` is for a caller that wants groups, badges,
    /// disabled rows, or to replace the row set while the palette is open.
    pub json: bool,
}

#[derive(Debug)]
pub(crate) enum ParsedCli {
    Help {
        /// Also show the plumbing a normal run never touches (`--help --advanced`).
        advanced: bool,
    },
    Version,
    ApiDescribe,
    Skill(SkillCommand),
    SkillHelp,
    AgentsHelp,
    Sessions(SessionsCommand),
    Extensions(ExtensionsCommand),
    SessionsHelp,
    Worktrees(WorktreesCli),
    WorktreesHelp,
    ExtensionsHelp,
    ExtensionsCheckHelp,
    ExtensionsInstallHelp,
    ExtensionsRemoveHelp,
    ExtensionsUpdateHelp,
    Install,
    Update(UpdateCommand),
    Run(CliArgs),
    Control(ControlCli),
    Publish(PublishCli),
    Subscribe(SubscribeCli),
    Pick(PickCli),
    Server {
        name: String,
        fresh: bool,
        config_path: Option<String>,
        startup_nonce: Option<String>,
    },
    /// Hidden remote-side stdio proxy.
    RemoteServe {
        name: String,
        autostart: bool,
    },
    /// Hidden remote-side control runner: one JSON request in, one JSON response out.
    RemoteControl {
        name: String,
    },
    /// Hidden remote-side worktree runner: one JSON call in, one JSON reply out.
    RemoteWorktrees,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionsCommand {
    Watch {
        config_path: Option<String>,
    },
    List {
        format: ListFormat,
        remote: Option<String>,
        config_path: Option<String>,
    },
    Kill {
        name: String,
        remote: Option<String>,
        config_path: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExtensionsCommand {
    List {
        json: bool,
        verbose: bool,
        config_path: Option<String>,
    },
    New {
        id: String,
    },
    Check {
        path: PathBuf,
        json: bool,
    },
    Install {
        source: String,
        link: bool,
        config_path: Option<String>,
    },
    Remove {
        id: String,
        config_path: Option<String>,
    },
    Update {
        id: String,
        config_path: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SkillCommand {
    Install { global: bool },
    Uninstall { global: bool },
    Status { global: bool },
    Print,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateCommand {
    Check,
    Apply,
    Rollback,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ListFormat {
    #[default]
    Text,
    Json,
}

pub(crate) fn parse_cli_args(args: Vec<String>) -> std::result::Result<ParsedCli, String> {
    if args.first().is_some_and(|arg| arg == "--skill") {
        return if args.len() == 1 {
            Ok(ParsedCli::Skill(SkillCommand::Print))
        } else {
            Err("--skill must be used without other arguments".to_string())
        };
    }
    if args.first().is_some_and(|arg| arg == "skill") {
        return skill::parse_skill_args(&args[1..]);
    }
    // Help wins over whatever else was typed unless a namespace comes first, in which case that
    // namespace owns its help. `--advanced` is only ever read here, so it can never be silently
    // swallowed by another command.
    let help_index = args.iter().position(|arg| arg == "--help" || arg == "-h");
    let namespace_index = args.iter().position(|arg| {
        matches!(
            arg.as_str(),
            "agents" | "sessions" | "worktrees" | "extensions" | "skill"
        )
    });
    if help_index.is_some_and(|help| namespace_index.is_none_or(|namespace| help < namespace)) {
        return Ok(ParsedCli::Help {
            advanced: args.iter().any(|arg| arg == "--advanced"),
        });
    }
    let mut cli = CliArgs::default();
    let mut socket: Option<PathBuf> = None;
    let mut socket_flag_seen = false;
    let mut iter = args.into_iter().peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--version" | "-V" => return Ok(ParsedCli::Version),
            "api" => {
                let subcommand =
                    require_value(&mut iter, "api requires a subcommand (expected `describe`)")?;
                if subcommand != "describe" {
                    return Err(format!(
                        "unknown api subcommand `{subcommand}`; expected `describe`"
                    ));
                }
                reject_trailing_control_args(&mut iter, "api describe")?;
                return Ok(ParsedCli::ApiDescribe);
            }
            "install" => {
                reject_trailing_control_args(&mut iter, "install")?;
                return Ok(ParsedCli::Install);
            }
            "update" => {
                let command = match iter.next().as_deref() {
                    None => UpdateCommand::Apply,
                    Some("--check") => {
                        reject_trailing_control_args(&mut iter, "update --check")?;
                        UpdateCommand::Check
                    }
                    Some("--rollback") => {
                        reject_trailing_control_args(&mut iter, "update --rollback")?;
                        UpdateCommand::Rollback
                    }
                    Some(other) => {
                        return Err(format!("unexpected argument `{other}` after update"));
                    }
                };
                return Ok(ParsedCli::Update(command));
            }
            "sessions" => {
                if let Some(parsed) = sessions::parse(&mut iter, &mut cli)? {
                    return Ok(parsed);
                }
            }
            "worktrees" => return worktrees::parse(&mut iter, &mut cli),
            "extensions" => return extensions::parse(&mut iter, cli.config_path),
            "--server" => {
                if cli.remote.is_some() {
                    return Err("--remote cannot be combined with --server".to_string());
                }
                if cli.read_only {
                    return Err("--read-only cannot be combined with --server".to_string());
                }
                let name = match cli.attach_session.take() {
                    Some(name) => {
                        if !cli.session_flag_target || cli.session_command != SessionCommand::Dwim {
                            return Err("--server must follow --session <NAME>".to_string());
                        }
                        name
                    }
                    None => require_value(&mut iter, "--server requires a session name")?,
                };
                reject_trailing_control_args(&mut iter, "--server")?;
                return Ok(ParsedCli::Server {
                    name,
                    fresh: false,
                    config_path: cli.config_path,
                    startup_nonce: None,
                });
            }
            "--fresh-server" => {
                let Some(name) = cli.attach_session.take() else {
                    return Err("--fresh-server must follow --session <NAME>".to_string());
                };
                if !cli.session_flag_target || cli.session_command != SessionCommand::Dwim {
                    return Err("--fresh-server must follow --session <NAME>".to_string());
                }
                if cli.remote.is_some() {
                    return Err("--remote cannot be combined with --fresh-server".to_string());
                }
                if cli.read_only {
                    return Err("--read-only cannot be combined with --fresh-server".to_string());
                }
                reject_trailing_control_args(&mut iter, "--fresh-server")?;
                return Ok(ParsedCli::Server {
                    name,
                    fresh: true,
                    config_path: cli.config_path,
                    startup_nonce: None,
                });
            }
            "--server-start" => {
                let name = require_value(&mut iter, "--server-start requires a session name")?;
                let nonce = require_value(&mut iter, "--server-start requires a nonce")?;
                if nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err("--server-start requires a 32-digit hexadecimal nonce".to_string());
                }
                reject_trailing_control_args(&mut iter, "--server-start")?;
                return Ok(ParsedCli::Server {
                    name,
                    fresh: false,
                    config_path: cli.config_path,
                    startup_nonce: Some(nonce),
                });
            }
            "--remote-serve" => {
                let name = require_value(&mut iter, "--remote-serve requires a session name")?;
                reject_trailing_control_args(&mut iter, "--remote-serve")?;
                return Ok(ParsedCli::RemoteServe {
                    name,
                    autostart: true,
                });
            }
            "--remote-control" => {
                let name = require_value(&mut iter, "--remote-control requires a session name")?;
                reject_trailing_control_args(&mut iter, "--remote-control")?;
                return Ok(ParsedCli::RemoteControl { name });
            }
            "--remote-worktrees" => {
                reject_trailing_control_args(&mut iter, "--remote-worktrees")?;
                return Ok(ParsedCli::RemoteWorktrees);
            }
            "--remote-serve-existing" => {
                let name =
                    require_value(&mut iter, "--remote-serve-existing requires a session name")?;
                reject_trailing_control_args(&mut iter, "--remote-serve-existing")?;
                return Ok(ParsedCli::RemoteServe {
                    name,
                    autostart: false,
                });
            }
            "--remote" => {
                // Host is optional when `[remote] default_host` is set (resolved in app::run).
                let target = match iter.peek().map(|s| s.as_str()) {
                    Some(next)
                        if !next.starts_with('-') && !matches!(next, "sessions" | "worktrees") =>
                    {
                        let target = iter.next().expect("peeked");
                        session::remote::parse_remote_target(&target)?;
                        target
                    }
                    _ => String::new(),
                };
                if cli.remote.replace(target).is_some() {
                    return Err("--remote specified more than once".to_string());
                }
            }
            "--session" => {
                let name = require_value(&mut iter, "--session requires a session name")?;
                if cli.attach_session.is_some() {
                    return Err("session target specified more than once".to_string());
                }
                cli.attach_session = Some(name);
                cli.session_command = SessionCommand::Dwim;
                cli.session_flag_target = true;
            }
            "--profile" => {
                let profile = require_value(&mut iter, "--profile requires a profile name")?;
                if cli.profile.replace(profile).is_some() {
                    return Err("--profile specified more than once".to_string());
                }
            }
            "--cwd" => {
                let cwd = require_value(&mut iter, "--cwd requires a directory")?;
                if cli.cwd.replace(cwd).is_some() {
                    return Err("--cwd specified more than once".to_string());
                }
            }
            "--pick" => {
                cli.pick = true;
            }
            "--read-only" => {
                cli.read_only = true;
            }
            "--config" => {
                let path = require_value(&mut iter, "--config requires a path")?;
                if cli.config_path.replace(path).is_some() {
                    return Err("--config specified more than once".to_string());
                }
            }
            "--socket" => {
                socket_flag_seen = true;
                let path = require_value(&mut iter, "--socket requires a path")?;
                if socket.replace(PathBuf::from(path)).is_some() {
                    return Err("--socket specified more than once".to_string());
                }
            }
            "agents" => {
                let args = iter.collect::<Vec<_>>();
                if agents::wants_help(&args) {
                    return Ok(ParsedCli::AgentsHelp);
                }
                let (command, output_format) = agents::parse_agents_args(args)?;
                let endpoint = control_endpoint(&cli, socket, &command)?;
                if matches!(
                    command,
                    control::ControlCommand::AgentWait { .. }
                        | control::ControlCommand::AgentPrompt { .. }
                ) && !endpoint.is_session()
                {
                    return Err(
                        "agent waits and prompts are server-owned; select a named session with --session"
                            .to_string(),
                    );
                }
                if endpoint.is_session()
                    && matches!(
                        command,
                        control::ControlCommand::AgentReport { target: None, .. }
                            | control::ControlCommand::AgentRelease { target: None, .. }
                    )
                {
                    return Err(
                        "agents report/release with --session requires --target; inherited ROZI_PANE belongs to a different namespace"
                            .to_string(),
                    );
                }
                return Ok(ParsedCli::Control(ControlCli {
                    endpoint,
                    request: control_request(command),
                    output_format,
                }));
            }
            "list-panes" => {
                let output_format = parse_output_format(&mut iter, "list-panes")?;
                let command = control::ControlCommand::ListPanes;
                return Ok(ParsedCli::Control(ControlCli {
                    endpoint: control_endpoint(&cli, socket, &command)?,
                    request: control_request(command),
                    output_format,
                }));
            }
            "layout" | "pane" => {
                let (command, output_format) = if arg == "layout" {
                    layout::parse_layout_args(&mut iter)?
                } else {
                    layout::parse_pane_args(&mut iter)?
                };
                return Ok(ParsedCli::Control(ControlCli {
                    endpoint: control_endpoint(&cli, socket, &command)?,
                    request: control_request(command),
                    output_format,
                }));
            }
            "metrics" => {
                let output_format = parse_output_format(&mut iter, "metrics")?;
                let command = control::ControlCommand::Metrics;
                return Ok(ParsedCli::Control(ControlCli {
                    endpoint: control_endpoint(&cli, socket, &command)?,
                    request: control_request(command),
                    output_format,
                }));
            }
            "focus" => {
                let target = iter
                    .next()
                    .ok_or_else(|| "focus requires a pane id".to_string())?
                    .parse()
                    .map_err(|_| "focus requires a numeric pane id".to_string())?;
                reject_trailing_control_args(&mut iter, "focus")?;
                let command = control::ControlCommand::Focus { target };
                return Ok(ParsedCli::Control(ControlCli {
                    endpoint: control_endpoint(&cli, socket, &command)?,
                    request: control_request(command),
                    output_format: None,
                }));
            }
            "send-text" => {
                let mut target = None;
                let mut text = None;
                while let Some(next) = iter.next() {
                    match next.as_str() {
                        "--target" => target = Some(parse_target(&mut iter)?),
                        _ if text.is_none() => text = Some(next),
                        other => {
                            return Err(format!("unexpected argument `{other}` after send-text"));
                        }
                    }
                }
                let text = text.ok_or_else(|| "send-text requires literal text".to_string())?;
                let command = control::ControlCommand::SendText { target, text };
                return Ok(ParsedCli::Control(ControlCli {
                    endpoint: control_endpoint(&cli, socket, &command)?,
                    request: control_request(command),
                    output_format: None,
                }));
            }
            "send-keys" => {
                let mut literal = false;
                let mut target = None;
                let mut keys = Vec::new();
                let mut passthrough = false;
                while let Some(arg) = iter.next() {
                    if !passthrough {
                        if arg == "--" {
                            passthrough = true;
                            continue;
                        }
                        if arg == "-l" || arg == "--literal" {
                            literal = true;
                            continue;
                        }
                        if arg == "--target" && keys.is_empty() {
                            target = Some(parse_target(&mut iter)?);
                            continue;
                        }
                        if arg.starts_with('-') && keys.is_empty() && arg != "-" {
                            return Err(format!("unexpected send-keys flag `{arg}`"));
                        }
                    }
                    keys.push(arg);
                }
                if keys.is_empty() {
                    return Err("send-keys requires at least one key or text argument".to_string());
                }
                let command = control::ControlCommand::SendKeys {
                    target,
                    keys,
                    literal,
                };
                return Ok(ParsedCli::Control(ControlCli {
                    endpoint: control_endpoint(&cli, socket, &command)?,
                    request: control_request(command),
                    output_format: None,
                }));
            }
            "notify" => {
                let mut message = None;
                let mut title = None;
                let mut level = control::NotifyLevel::default();
                let mut passthrough = false;
                while let Some(arg) = iter.next() {
                    match arg.as_str() {
                        "--" if !passthrough => passthrough = true,
                        "--title" if !passthrough => {
                            title = Some(require_value(&mut iter, "--title requires text")?);
                        }
                        "--level" if !passthrough => {
                            let value = require_value(&mut iter, "--level requires a value")?;
                            level = control::NotifyLevel::parse_cli(&value)?;
                        }
                        other if !passthrough && other.starts_with('-') && other != "-" => {
                            return Err(format!("unexpected notify flag `{other}`"));
                        }
                        _ if message.is_none() => message = Some(arg),
                        _ => return Err(format!("unexpected argument `{arg}` after notify")),
                    }
                }
                let Some(message) = message else {
                    return Err("notify requires a message".to_string());
                };
                let command = control::ControlCommand::Notify {
                    message,
                    title,
                    level,
                };
                return Ok(ParsedCli::Control(ControlCli {
                    endpoint: control_endpoint(&cli, socket, &command)?,
                    request: control_request(command),
                    output_format: None,
                }));
            }
            "status" => {
                // A script driving a session it is not running inside has no `ROZI_PANE` and no
                // focused pane to fall back to, so `--target` is the only way it can name a pane.
                // It is accepted on either side of the value, since `status working --target 3`
                // and `status --target 3 working` both read naturally.
                let mut target = None;
                let mut value = None;
                let mut reason = None;
                let mut clear = false;
                while let Some(arg) = iter.next() {
                    match arg.as_str() {
                        "--target" => {
                            if target.replace(parse_target(&mut iter)?).is_some() {
                                return Err("status --target specified more than once".to_string());
                            }
                        }
                        "--clear" => clear = true,
                        "--reason" => {
                            if reason
                                .replace(require_value(&mut iter, "--reason requires text")?)
                                .is_some()
                            {
                                return Err("status --reason specified more than once".to_string());
                            }
                        }
                        other if other.starts_with('-') && other != "-" => {
                            return Err(format!("unexpected status flag `{other}`"));
                        }
                        _ if value.is_none() => value = Some(arg),
                        _ => return Err(format!("unexpected argument `{arg}` after status")),
                    }
                }
                if clear && value.is_some() {
                    return Err("status takes a value or --clear, not both".to_string());
                }
                if clear && reason.is_some() {
                    return Err("status --clear takes no --reason".to_string());
                }
                if !clear && value.is_none() {
                    return Err("status requires a value or --clear".to_string());
                }
                let command = control::ControlCommand::SetStatus {
                    target,
                    status: value,
                    reason,
                };
                return Ok(ParsedCli::Control(ControlCli {
                    endpoint: control_endpoint(&cli, socket, &command)?,
                    request: control_request(command),
                    output_format: None,
                }));
            }
            "publish" => {
                reject_trailing_control_args(&mut iter, "publish")?;
                return Ok(ParsedCli::Publish(PublishCli {
                    socket: ui_stream_socket(&cli, socket, &control::ControlCommand::Publish)?,
                }));
            }
            "subscribe" => {
                let mut events = Vec::new();
                let mut passthrough = false;
                for arg in iter.by_ref() {
                    if arg == "--" && !passthrough {
                        passthrough = true;
                    } else if !passthrough && arg.starts_with('-') {
                        return Err(format!("unexpected subscribe flag `{arg}`"));
                    } else {
                        events.push(arg);
                    }
                }
                return Ok(ParsedCli::Subscribe(SubscribeCli {
                    socket: ui_stream_socket(
                        &cli,
                        socket,
                        &control::ControlCommand::Subscribe {
                            events: events.clone(),
                        },
                    )?,
                    events,
                }));
            }
            "pick" => {
                let mut title = None;
                let mut placeholder = None;
                let mut json = false;
                let mut passthrough = false;
                while let Some(arg) = iter.next() {
                    match arg.as_str() {
                        "--" if !passthrough => passthrough = true,
                        "--json" if !passthrough => json = true,
                        "--title" | "-t" if !passthrough => {
                            title = Some(
                                iter.next()
                                    .ok_or_else(|| "--title requires a title".to_string())?,
                            );
                        }
                        other if !passthrough && other.starts_with("--title=") => {
                            title = Some(other.trim_start_matches("--title=").to_string());
                        }
                        "--placeholder" | "-p" if !passthrough => {
                            placeholder = Some(iter.next().ok_or_else(|| {
                                "--placeholder requires a placeholder".to_string()
                            })?);
                        }
                        other if !passthrough && other.starts_with("--placeholder=") => {
                            placeholder =
                                Some(other.trim_start_matches("--placeholder=").to_string());
                        }
                        other if !passthrough && other.starts_with('-') && other != "-" => {
                            return Err(format!("unexpected pick flag `{other}`"));
                        }
                        _ => {
                            return Err(format!("unexpected argument `{arg}` after pick"));
                        }
                    }
                }
                return Ok(ParsedCli::Pick(PickCli {
                    socket: ui_stream_socket(
                        &cli,
                        socket,
                        &control::ControlCommand::Pick {
                            title: title.clone(),
                            placeholder: placeholder.clone(),
                            empty: None,
                            width: None,
                            actions: Vec::new(),
                            tabs: Vec::new(),
                            tab: None,
                        },
                    )?,
                    title,
                    placeholder,
                    json,
                }));
            }
            "split" => {
                let mut command = None;
                let mut argv = None;
                let mut cwd = None;
                let mut title = None;
                let mut keep_open = false;
                let mut focus = false;
                let mut workspace = None;
                let mut passthrough = false;
                while let Some(arg) = iter.next() {
                    match arg.as_str() {
                        "--" if !passthrough => passthrough = true,
                        "--focus" if !passthrough => focus = true,
                        "--keep-open" if !passthrough => keep_open = true,
                        "--argv" if !passthrough && command.is_none() => {
                            let direct: Vec<String> = iter.by_ref().collect();
                            crate::pane::launch::PaneLaunch::direct(direct.clone())?;
                            argv = Some(direct);
                            break;
                        }
                        "--argv" if !passthrough => {
                            return Err(
                                "split accepts either COMMAND or --argv, not both".to_string()
                            );
                        }
                        "--cwd" if !passthrough && cwd.is_none() => {
                            cwd = Some(require_value(
                                &mut iter,
                                "split --cwd requires a directory",
                            )?);
                        }
                        "--cwd" if !passthrough => {
                            return Err("split --cwd specified more than once".to_string());
                        }
                        "--title" if !passthrough && title.is_none() => {
                            title = Some(require_value(&mut iter, "split --title requires text")?);
                        }
                        "--title" if !passthrough => {
                            return Err("split --title specified more than once".to_string());
                        }
                        "--workspace" if !passthrough && workspace.is_none() => {
                            let value = require_value(
                                &mut iter,
                                "split --workspace requires a workspace number",
                            )?;
                            workspace = Some(value.parse().map_err(|_| {
                                "split --workspace requires a workspace number".to_string()
                            })?);
                        }
                        "--workspace" if !passthrough => {
                            return Err("split --workspace specified more than once".to_string());
                        }
                        // A mistyped `--focu` must not become the command that gets run; `--` ends
                        // flag parsing for the rare command that really does start with a dash.
                        other if !passthrough && other.starts_with('-') && other != "-" => {
                            return Err(format!("unexpected split flag `{other}`"));
                        }
                        _ if command.is_none() => command = Some(arg),
                        _ => {
                            return Err(format!("unexpected argument `{arg}` after split"));
                        }
                    }
                }
                let command = control::ControlCommand::NewPane {
                    command,
                    argv,
                    cwd,
                    title,
                    keep_open,
                    focus,
                    workspace,
                };
                return Ok(ParsedCli::Control(ControlCli {
                    endpoint: control_endpoint(&cli, socket, &command)?,
                    request: control_request(command),
                    output_format: None,
                }));
            }
            "run-action" => {
                let action = require_value(&mut iter, "run-action requires an action id")?;
                reject_trailing_control_args(&mut iter, "run-action")?;
                let command = control::ControlCommand::RunAction { action };
                return Ok(ParsedCli::Control(ControlCli {
                    endpoint: control_endpoint(&cli, socket, &command)?,
                    request: control_request(command),
                    output_format: None,
                }));
            }
            "capture-pane" => {
                let mut target = None;
                let mut scrollback = None;
                let mut output_format = None;
                while let Some(next) = iter.next() {
                    match next.as_str() {
                        "--target" => target = Some(parse_target(&mut iter)?),
                        "--scrollback" => {
                            let value = iter.next().ok_or_else(|| {
                                "--scrollback requires a line count or `full`".to_string()
                            })?;
                            scrollback = Some(control::CaptureScrollback::parse_cli(&value)?);
                        }
                        "--last-output" => {
                            scrollback = Some(control::CaptureScrollback::Named(
                                control::CaptureScrollbackNamed::LastOutput,
                            ));
                        }
                        "--format" => {
                            let value = require_value(&mut iter, "--format requires text or json")?;
                            if output_format
                                .replace(parse_list_format(&value, "capture-pane")?)
                                .is_some()
                            {
                                return Err(
                                    "capture-pane --format specified more than once".to_string()
                                );
                            }
                        }
                        other => {
                            return Err(format!(
                                "unexpected argument `{other}` after capture-pane"
                            ));
                        }
                    }
                }
                let command = control::ControlCommand::CapturePane { target, scrollback };
                return Ok(ParsedCli::Control(ControlCli {
                    endpoint: control_endpoint(&cli, socket, &command)?,
                    request: control_request(command),
                    output_format,
                }));
            }
            "switch-workspace" => {
                let index = iter
                    .next()
                    .ok_or_else(|| "switch-workspace requires a workspace number".to_string())?
                    .parse()
                    .map_err(|_| {
                        "switch-workspace requires a numeric workspace number".to_string()
                    })?;
                reject_trailing_control_args(&mut iter, "switch-workspace")?;
                let command = control::ControlCommand::SwitchWorkspace { index };
                return Ok(ParsedCli::Control(ControlCli {
                    endpoint: control_endpoint(&cli, socket, &command)?,
                    request: control_request(command),
                    output_format: None,
                }));
            }
            "move-to-workspace" => {
                let index = iter
                    .next()
                    .ok_or_else(|| "move-to-workspace requires a workspace number".to_string())?
                    .parse()
                    .map_err(|_| {
                        "move-to-workspace requires a numeric workspace number".to_string()
                    })?;
                reject_trailing_control_args(&mut iter, "move-to-workspace")?;
                let command = control::ControlCommand::MoveToWorkspace { index };
                return Ok(ParsedCli::Control(ControlCli {
                    endpoint: control_endpoint(&cli, socket, &command)?,
                    request: control_request(command),
                    output_format: None,
                }));
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown flag `{other}`"));
            }
            name => {
                if socket_flag_seen {
                    return Err(format!(
                        "--socket requires a control command before `{name}`"
                    ));
                }
                if cli.attach_session.is_some() {
                    return Err(format!("unexpected argument `{name}`"));
                }
                cli.attach_session = Some(name.to_string());
                cli.session_command = SessionCommand::Dwim;
            }
        }
    }
    if socket_flag_seen {
        return Err("--socket requires a control command".to_string());
    }
    if cli.read_only && cli.attach_session.is_none() {
        return Err("--read-only requires a target or --session".to_string());
    }
    if cli.read_only && cli.session_command == SessionCommand::New {
        return Err("--read-only cannot be used with sessions new".to_string());
    }
    if cli.profile.is_some() && cli.session_command != SessionCommand::New {
        return Err("--profile can only be used with sessions new".to_string());
    }
    if cli.cwd.is_some() && cli.session_command != SessionCommand::New {
        return Err("--cwd can only be used with sessions new".to_string());
    }
    if cli.cwd.is_some() && cli.profile.is_some() {
        return Err("--cwd cannot be combined with --profile".to_string());
    }
    Ok(ParsedCli::Run(cli))
}

/// Take the value that must follow a name-taking flag or verb, rejecting a flag-shaped one.
///
/// The pane id after a `--target` flag, shared by every control command that accepts one.
pub(super) fn parse_target(
    iter: &mut impl Iterator<Item = String>,
) -> std::result::Result<crate::state::PaneId, String> {
    let value = require_value(iter, "--target requires a pane id")?;
    value
        .parse()
        .map_err(|_| "--target requires a numeric pane id".to_string())
}

/// Session names, profile names, and action ids all accept `-`, so a bare `next()` silently eats
/// the following option: without this, `rozi sessions attach --read-only` hunts for a session
/// literally named `--read-only`, and `rozi --server --pick` starts a session server for one. A
/// lone `-` is still a legal value; a real path that begins with a dash can be written `./-name`.
pub(super) fn require_value(
    iter: &mut impl Iterator<Item = String>,
    missing: &str,
) -> std::result::Result<String, String> {
    match iter.next() {
        Some(value) if value.starts_with('-') && value != "-" => {
            Err(format!("{missing} (got the flag `{value}`)"))
        }
        Some(value) => Ok(value),
        None => Err(missing.to_string()),
    }
}

pub(super) fn parse_list_format(
    value: &str,
    command: &str,
) -> std::result::Result<ListFormat, String> {
    match value {
        "text" => Ok(ListFormat::Text),
        "json" => Ok(ListFormat::Json),
        other => Err(format!(
            "unknown {command} --format `{other}` (expected text or json)"
        )),
    }
}

pub(super) fn parse_output_format(
    iter: &mut impl Iterator<Item = String>,
    command: &str,
) -> std::result::Result<Option<ListFormat>, String> {
    let mut output_format = None;
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--format" => {
                let value = require_value(iter, "--format requires text or json")?;
                if output_format
                    .replace(parse_list_format(&value, command)?)
                    .is_some()
                {
                    return Err(format!("{command} --format specified more than once"));
                }
            }
            other => return Err(format!("unexpected argument `{other}` after {command}")),
        }
    }
    Ok(output_format)
}

/// Decide which endpoint a control command talks to, rejecting launch-only options.
///
/// A control command never attaches anything, so the launch options are rejected rather than
/// ignored: accepting them silently would let a command answer from a rozi other than the one the
/// caller believed it had reached.
///
/// `--session <NAME>` is the one target that does apply. It selects the named session server
/// instead of a UI, for the commands a server can answer on its own; the rest say what they would
/// have needed a UI for, rather than reporting the session as unreachable.
///
/// `--remote <HOST>` qualifies that target rather than replacing it: with a session it names that
/// session on that host, and the same commands are served there. Without one there is nothing to
/// reach - a control command addresses a session server, and the far host's UI is not one.
pub(super) fn control_endpoint(
    cli: &CliArgs,
    socket: Option<PathBuf>,
    command: &control::ControlCommand,
) -> std::result::Result<ControlEndpoint, String> {
    let offender = if cli.config_path.is_some() {
        "--config"
    } else if cli.read_only {
        "--read-only"
    } else if cli.pick {
        "--pick"
    } else if cli.profile.is_some() {
        "--profile"
    } else if cli.cwd.is_some() {
        "--cwd"
    } else {
        ""
    };
    if !offender.is_empty() {
        return Err(format!("{offender} does not apply to control commands"));
    }
    let Some(session) = cli.attach_session.as_deref() else {
        if cli.remote.is_some() {
            return Err(
                "--remote needs --session <NAME>: a control command reaches a named session on that host, not its UI"
                    .to_string(),
            );
        }
        return Ok(ControlEndpoint::Ui(socket));
    };
    if !cli.session_flag_target {
        return Err(format!(
            "`{session}` is a launch target; write `--session {session}` before a control command to reach that session"
        ));
    }
    if socket.is_some() {
        return Err("--socket and --session name two different endpoints".to_string());
    }
    if let Some(reason) = crate::session::server::session_control_unsupported(command) {
        return Err(reason.to_string());
    }
    Ok(match cli.remote.clone() {
        Some(target) => ControlEndpoint::Remote {
            target,
            session: session.to_string(),
        },
        None => ControlEndpoint::Session(session.to_string()),
    })
}

/// The endpoint for a control command that is always a bidirectional UI stream (`publish`,
/// `subscribe`, `pick`).
///
/// Routing them through [`control_endpoint`] rather than a separate check keeps their refusal
/// wording identical to every other command's.
pub(super) fn ui_stream_socket(
    cli: &CliArgs,
    socket: Option<PathBuf>,
    command: &control::ControlCommand,
) -> std::result::Result<Option<PathBuf>, String> {
    match control_endpoint(cli, socket, command)? {
        ControlEndpoint::Ui(socket) => Ok(socket),
        // `session_control_unsupported` refuses all three against a session, so this cannot be
        // reached; spelled out rather than unwrapped so a later addition cannot slip past it.
        endpoint if endpoint.is_session() => {
            Err("this command needs a running rozi, not a session server".to_string())
        }
        _ => unreachable!("every endpoint is a UI or a session"),
    }
}

pub(super) fn reject_trailing_control_args(
    iter: &mut impl Iterator<Item = String>,
    command: &str,
) -> std::result::Result<(), String> {
    if let Some(extra) = iter.next() {
        Err(format!("unexpected argument `{extra}` after {command}"))
    } else {
        Ok(())
    }
}

pub(super) fn control_request(command: control::ControlCommand) -> control::ControlRequest {
    control::ControlRequest {
        command,
        source_pane: std::env::var("ROZI_PANE").ok().and_then(|v| v.parse().ok()),
        extension: crate::config::provenance_from_process(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_positional_target_and_rejects_removed_flags() {
        let positional = expect_run(parse_cli_args(vec!["dev".into()]).expect("parses"));
        assert_eq!(positional.session_command, SessionCommand::Dwim);
        assert_eq!(positional.profile, None);
        assert_eq!(positional.attach_session.as_deref(), Some("dev"));
        assert!(parse_cli_args(vec!["--profile".into(), "dev".into()]).is_err());
        assert!(parse_cli_args(vec!["--attach".into(), "dev".into()]).is_err());
    }

    #[test]
    fn cli_help_and_version_are_early_exit_variants() {
        assert!(matches!(
            parse_cli_args(vec!["--help".into()]).expect("parses"),
            ParsedCli::Help { advanced: false }
        ));
        assert!(matches!(
            parse_cli_args(vec!["-V".into()]).expect("parses"),
            ParsedCli::Version
        ));
        assert!(matches!(
            parse_cli_args(vec!["api".into(), "describe".into()]).expect("parses"),
            ParsedCli::ApiDescribe
        ));
        assert!(parse_cli_args(vec!["api".into()]).is_err());
        assert!(parse_cli_args(vec!["api".into(), "unknown".into()]).is_err());
    }

    #[test]
    fn cli_parses_managed_install_and_update_commands_strictly() {
        assert!(matches!(
            parse_cli_args(vec!["install".into()]).expect("parses"),
            ParsedCli::Install
        ));
        assert!(matches!(
            parse_cli_args(vec!["update".into()]).expect("parses"),
            ParsedCli::Update(UpdateCommand::Apply)
        ));
        assert!(matches!(
            parse_cli_args(vec!["update".into(), "--check".into()]).expect("parses"),
            ParsedCli::Update(UpdateCommand::Check)
        ));
        assert!(matches!(
            parse_cli_args(vec!["update".into(), "--rollback".into()]).expect("parses"),
            ParsedCli::Update(UpdateCommand::Rollback)
        ));
        assert!(parse_cli_args(vec!["install".into(), "extra".into()]).is_err());
        assert!(parse_cli_args(vec!["update".into(), "--check".into(), "extra".into()]).is_err());
        assert!(
            parse_cli_args(vec!["update".into(), "--rollback".into(), "--check".into()]).is_err()
        );
    }

    #[test]
    fn cli_reserved_control_commands_do_not_parse_as_profiles() {
        let parsed = parse_cli_args(vec!["list-panes".into()]).expect("parses");
        assert!(matches!(parsed, ParsedCli::Control(_)));
        let ParsedCli::Control(metrics) =
            parse_cli_args(vec!["metrics".into()]).expect("metrics parses")
        else {
            panic!("expected metrics control command");
        };
        assert_eq!(metrics.request.command, control::ControlCommand::Metrics);

        let profile = expect_run(
            parse_cli_args(vec!["--session".into(), "list-panes".into()]).expect("parses"),
        );
        assert_eq!(profile.attach_session.as_deref(), Some("list-panes"));
        for reserved in ["sessions", "extensions"] {
            let target = expect_run(
                parse_cli_args(vec!["--session".into(), reserved.into()]).expect("parses"),
            );
            assert_eq!(target.attach_session.as_deref(), Some(reserved));
        }
    }

    #[test]
    fn session_agent_reports_require_an_explicit_pane_namespace() {
        let error = parse_cli_args(vec![
            "--session".into(),
            "dev".into(),
            "agents".into(),
            "report".into(),
            "--agent".into(),
            "claude".into(),
            "--integration".into(),
            "hook-a".into(),
            "--state".into(),
            "working".into(),
            "--seq".into(),
            "1".into(),
        ])
        .expect_err("session report without target must fail");
        assert!(error.contains("requires --target"), "{error}");

        let parsed = parse_cli_args(vec![
            "agents".into(),
            "release".into(),
            "--integration".into(),
            "hook-a".into(),
            "--seq".into(),
            "2".into(),
        ])
        .expect("UI report may resolve its source pane");
        let ParsedCli::Control(control) = parsed else {
            panic!("expected control command");
        };
        assert!(matches!(
            control.request.command,
            control::ControlCommand::AgentRelease { target: None, .. }
        ));
    }

    #[test]
    fn layout_get_parses_a_workspace_and_format_and_refuses_what_it_cannot_answer() {
        let args = |args: &[&str]| args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        let ParsedCli::Control(parsed) = parse_cli_args(args(&[
            "layout",
            "get",
            "--workspace",
            "3",
            "--format",
            "json",
        ]))
        .expect("parses") else {
            panic!("expected control command");
        };
        assert_eq!(
            parsed.request.command,
            control::ControlCommand::LayoutGet { workspace: Some(3) }
        );
        assert_eq!(parsed.output_format, Some(ListFormat::Json));

        let ParsedCli::Control(session) =
            parse_cli_args(args(&["--session", "dev", "layout", "get"])).expect("parses")
        else {
            panic!("expected control command");
        };
        assert!(
            session.endpoint.is_session(),
            "a session server answers layout get"
        );

        for refused in [
            &["layout"][..],
            &["layout", "set"],
            &["layout", "get", "--workspace", "0"],
            &["layout", "get", "--workspace", "10"],
            &["layout", "get", "--workspace", "2", "--workspace", "3"],
            &["layout", "get", "extra"],
        ] {
            assert!(
                parse_cli_args(args(refused)).is_err(),
                "{refused:?} should be refused"
            );
        }
    }

    #[test]
    fn layout_and_pane_writes_parse_absolute_targets_and_refuse_ambiguous_ones() {
        let args = |args: &[&str]| args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        let command = |parts: &[&str]| match parse_cli_args(args(parts)) {
            Ok(ParsedCli::Control(parsed)) => parsed.request.command,
            other => panic!(
                "{parts:?} should parse as a control command, got {:?}",
                other.err()
            ),
        };

        assert_eq!(
            command(&[
                "layout",
                "set",
                "--workspace",
                "2",
                "Master",
                "--if-revision",
                "7"
            ]),
            control::ControlCommand::LayoutSet {
                workspace: 2,
                layout: control::ControlLayoutKind::Master,
                if_revision: Some(7),
            }
        );
        assert_eq!(
            command(&[
                "pane",
                "set",
                "--target",
                "4",
                "--floating",
                "true",
                "--rect",
                "-2,3,40,12",
            ]),
            control::ControlCommand::PaneSet {
                target: 4,
                floating: Some(true),
                fullscreen: None,
                rect: Some(control::CellRect {
                    x: -2,
                    y: 3,
                    width: 40,
                    height: 12,
                }),
                rect_fraction: None,
                if_revision: None,
            }
        );
        let control::ControlCommand::PaneSet { rect_fraction, .. } = command(&[
            "pane",
            "set",
            "--target",
            "4",
            "--rect-fraction",
            "0.1, 0.2, 0.5, 0.25",
        ]) else {
            panic!("expected pane set");
        };
        assert_eq!(
            rect_fraction,
            Some(control::FractionRect {
                x: 0.1,
                y: 0.2,
                width: 0.5,
                height: 0.25,
            })
        );

        assert_eq!(
            command(&["pane", "move", "--target", "4", "--workspace", "3"]),
            control::ControlCommand::PaneMove {
                target: 4,
                workspace: 3,
                if_revision: None,
            }
        );
        assert_eq!(
            command(&[
                "pane",
                "swap",
                "--target",
                "4",
                "--with",
                "6",
                "--if-revision",
                "2"
            ]),
            control::ControlCommand::PaneSwap {
                target: 4,
                with: 6,
                if_revision: Some(2),
            }
        );
        assert_eq!(
            command(&["pane", "close", "--target", "4"]),
            control::ControlCommand::PaneClose {
                target: 4,
                if_revision: None,
            }
        );

        for refused in [
            &["pane", "move", "--target", "4"][..],
            &["pane", "swap", "--target", "4"],
            &["pane", "close"],
            &["pane", "close", "--target", "4", "--floating", "true"],
            &[
                "pane",
                "move",
                "--target",
                "4",
                "--workspace",
                "2",
                "--with",
                "5",
            ],
            &["layout", "set", "grid"],
            &["layout", "set", "--workspace", "1"],
            &["layout", "set", "--workspace", "1", "spiral"],
            &["layout", "get", "--if-revision", "3"],
            &["pane", "set", "--floating", "true"],
            &["pane", "set", "--target", "4", "--floating", "yes"],
            &["pane", "set", "--target", "4", "--rect", "1,2,3"],
            &["pane", "set", "--target", "4", "--rect", "1,2,3.5,4"],
            &[
                "pane",
                "set",
                "--target",
                "4",
                "--fullscreen",
                "true",
                "--fullscreen",
                "false",
            ],
            &["pane", "get", "--target", "4"],
        ] {
            assert!(
                parse_cli_args(args(refused)).is_err(),
                "{refused:?} should be refused"
            );
        }
    }

    #[test]
    fn cli_report_commands_accept_an_explicit_text_or_json_format() {
        for (command, format) in [
            ("list-panes", ListFormat::Text),
            ("metrics", ListFormat::Json),
        ] {
            let ParsedCli::Control(parsed) = parse_cli_args(vec![
                command.into(),
                "--format".into(),
                match format {
                    ListFormat::Text => "text",
                    ListFormat::Json => "json",
                }
                .into(),
            ])
            .expect("parses") else {
                panic!("expected control command");
            };
            assert_eq!(parsed.output_format, Some(format));
        }

        let ParsedCli::Control(capture) = parse_cli_args(vec![
            "capture-pane".into(),
            "--target".into(),
            "7".into(),
            "--format".into(),
            "text".into(),
        ])
        .expect("parses") else {
            panic!("expected capture command");
        };
        assert_eq!(capture.output_format, Some(ListFormat::Text));
        assert!(
            parse_cli_args(vec!["list-panes".into(), "--format".into(), "yaml".into()]).is_err()
        );
    }

    #[test]
    fn a_session_target_routes_a_control_command_to_that_session_server() {
        let ParsedCli::Control(control) = parse_cli_args(vec![
            "--session".into(),
            "dev".into(),
            "capture-pane".into(),
            "--target".into(),
            "3".into(),
        ])
        .expect("parses") else {
            panic!("expected control");
        };
        assert_eq!(
            control.endpoint,
            ControlEndpoint::Session("dev".to_string())
        );
        assert_eq!(
            control.request.command,
            control::ControlCommand::CapturePane {
                target: Some(3),
                scrollback: None,
            }
        );
    }

    /// `rozi dev` launches a UI, so one trailing word must not silently turn the same target into
    /// "talk to the session server instead". The error says how to ask for that.
    #[test]
    fn a_positional_launch_target_is_not_a_control_target() {
        let error = parse_cli_args(vec!["dev".into(), "list-panes".into()])
            .expect_err("a positional target must not route a control command");
        assert!(error.contains("--session dev"), "{error}");
    }

    #[test]
    fn a_command_that_needs_a_screen_is_refused_at_parse_time_with_its_reason() {
        for (args, marker) in [
            (
                vec!["--session", "dev", "focus", "3"],
                "focus is client-local",
            ),
            (
                vec!["--session", "dev", "run-action", "toggle-float"],
                "actions run inside a UI",
            ),
            (
                vec!["--session", "dev", "switch-workspace", "2"],
                "active workspace is client-local",
            ),
            (
                vec!["--session", "dev", "subscribe"],
                "subscribe streams UI events",
            ),
            (
                vec!["--session", "dev", "pick"],
                "pick opens a modal in a UI",
            ),
            (
                vec!["--session", "dev", "publish"],
                "publish belongs to the pane",
            ),
        ] {
            let error = parse_cli_args(args.iter().map(|arg| (*arg).to_string()).collect())
                .expect_err("a UI-only command must not reach a session server");
            assert!(error.contains(marker), "{error}");
        }
    }

    #[test]
    fn a_socket_and_a_session_cannot_both_name_the_endpoint() {
        let error = parse_cli_args(vec![
            "--socket".into(),
            "/tmp/rozi.sock".into(),
            "--session".into(),
            "dev".into(),
            "list-panes".into(),
        ])
        .expect_err("two endpoints is a mistake, not a precedence question");
        assert!(error.contains("two different endpoints"), "{error}");
    }

    /// The launch-only options stay rejected now that one target is accepted, so
    /// `rozi --remote box --session dev list-panes` cannot look like it reached another host.
    #[test]
    fn launch_options_are_still_rejected_alongside_a_session_target() {
        // `--remote` is absent deliberately: with a session target it names which host that
        // session is on, which is a target, not a launch option.
        for args in [
            vec!["--config", "/tmp/c.toml", "--session", "dev", "list-panes"],
            vec!["--read-only", "--session", "dev", "list-panes"],
            vec!["--profile", "work", "--session", "dev", "list-panes"],
        ] {
            let error = parse_cli_args(args.iter().map(|arg| (*arg).to_string()).collect())
                .expect_err("launch options do not apply to control commands");
            assert!(
                error.contains("does not apply to control commands"),
                "{error}"
            );
        }
    }

    #[test]
    fn status_accepts_a_target_on_either_side_of_its_value() {
        let expected = control::ControlCommand::SetStatus {
            target: Some(3),
            status: Some("working".to_string()),
            reason: Some("building".to_string()),
        };
        for args in [
            vec!["status", "--target", "3", "working", "--reason", "building"],
            vec!["status", "working", "--target", "3", "--reason", "building"],
            vec!["status", "working", "--reason", "building", "--target", "3"],
        ] {
            let ParsedCli::Control(control) =
                parse_cli_args(args.iter().map(|arg| (*arg).to_string()).collect())
                    .expect("parses")
            else {
                panic!("expected control");
            };
            assert_eq!(control.request.command, expected);
        }
        let ParsedCli::Control(cleared) = parse_cli_args(vec![
            "status".into(),
            "--clear".into(),
            "--target".into(),
            "3".into(),
        ])
        .expect("parses") else {
            panic!("expected control");
        };
        assert_eq!(
            cleared.request.command,
            control::ControlCommand::SetStatus {
                target: Some(3),
                status: None,
                reason: None,
            }
        );
        assert!(parse_cli_args(vec!["status".into()]).is_err());
        assert!(parse_cli_args(vec!["status".into(), "--clear".into(), "working".into()]).is_err());
        assert!(
            parse_cli_args(vec![
                "status".into(),
                "--clear".into(),
                "--reason".into(),
                "why".into()
            ])
            .is_err()
        );
    }

    #[test]
    fn cli_control_socket_flag_is_preserved() {
        let parsed = parse_cli_args(vec![
            "--socket".into(),
            "/tmp/rozi.sock".into(),
            "send-text".into(),
            "hi".into(),
        ])
        .expect("parses");
        let ParsedCli::Control(control) = parsed else {
            panic!("expected control");
        };
        assert_eq!(
            control.endpoint,
            ControlEndpoint::Ui(Some(PathBuf::from("/tmp/rozi.sock")))
        );
        assert!(matches!(
            control.request.command,
            control::ControlCommand::SendText { .. }
        ));
    }

    #[test]
    fn cli_parses_run_action_capture_pane_and_workspace_commands() {
        let ParsedCli::Control(run_action) =
            parse_cli_args(vec!["run-action".into(), "toggle-float".into()]).expect("parses")
        else {
            panic!("expected control");
        };
        assert_eq!(
            run_action.request.command,
            control::ControlCommand::RunAction {
                action: "toggle-float".to_string()
            }
        );

        let ParsedCli::Control(capture) =
            parse_cli_args(vec!["capture-pane".into()]).expect("parses")
        else {
            panic!("expected control");
        };
        assert_eq!(
            capture.request.command,
            control::ControlCommand::CapturePane {
                target: None,
                scrollback: None
            }
        );

        let ParsedCli::Control(capture_target) =
            parse_cli_args(vec!["capture-pane".into(), "--target".into(), "7".into()])
                .expect("parses")
        else {
            panic!("expected control");
        };
        assert_eq!(
            capture_target.request.command,
            control::ControlCommand::CapturePane {
                target: Some(7),
                scrollback: None
            }
        );

        let ParsedCli::Control(capture_scrollback) = parse_cli_args(vec![
            "capture-pane".into(),
            "--scrollback".into(),
            "full".into(),
        ])
        .expect("parses") else {
            panic!("expected control");
        };
        assert_eq!(
            capture_scrollback.request.command,
            control::ControlCommand::CapturePane {
                target: None,
                scrollback: Some(control::CaptureScrollback::Named(
                    control::CaptureScrollbackNamed::Full
                ))
            }
        );

        let ParsedCli::Control(capture_last) =
            parse_cli_args(vec!["capture-pane".into(), "--last-output".into()]).expect("parses")
        else {
            panic!("expected control");
        };
        assert_eq!(
            capture_last.request.command,
            control::ControlCommand::CapturePane {
                target: None,
                scrollback: Some(control::CaptureScrollback::Named(
                    control::CaptureScrollbackNamed::LastOutput
                ))
            }
        );

        let ParsedCli::Control(send_keys) =
            parse_cli_args(vec!["send-keys".into(), "C-c".into(), "Enter".into()]).expect("parses")
        else {
            panic!("expected control");
        };
        assert_eq!(
            send_keys.request.command,
            control::ControlCommand::SendKeys {
                target: None,
                keys: vec!["C-c".into(), "Enter".into()],
                literal: false,
            }
        );

        // A script driving a pane it spawned has to address it: run from inside a pane, the
        // source-pane fallback would send the input back to the script's own pane.
        let ParsedCli::Control(targeted_text) = parse_cli_args(vec![
            "send-text".into(),
            "--target".into(),
            "3".into(),
            "ls".into(),
        ])
        .expect("parses") else {
            panic!("expected control");
        };
        assert_eq!(
            targeted_text.request.command,
            control::ControlCommand::SendText {
                target: Some(3),
                text: "ls".into(),
            }
        );

        let ParsedCli::Control(targeted_keys) = parse_cli_args(vec![
            "send-keys".into(),
            "--target".into(),
            "3".into(),
            "Enter".into(),
        ])
        .expect("parses") else {
            panic!("expected control");
        };
        assert_eq!(
            targeted_keys.request.command,
            control::ControlCommand::SendKeys {
                target: Some(3),
                keys: vec!["Enter".into()],
                literal: false,
            }
        );

        // `--target` is a flag only while it could still be one: past the first key it is text to
        // send, exactly as `-n` is after `--`.
        let ParsedCli::Control(literal_target) =
            parse_cli_args(vec!["send-keys".into(), "Enter".into(), "--target".into()])
                .expect("parses")
        else {
            panic!("expected control");
        };
        assert_eq!(
            literal_target.request.command,
            control::ControlCommand::SendKeys {
                target: None,
                keys: vec!["Enter".into(), "--target".into()],
                literal: false,
            }
        );
        // A script that spawns into the workspace someone is working in re-tiles their layout on
        // every pane; naming a workspace keeps it out of the way and keeps the geometry stable.
        let ParsedCli::Control(elsewhere) = parse_cli_args(vec![
            "split".into(),
            "--workspace".into(),
            "9".into(),
            "--argv".into(),
            "grok".into(),
        ])
        .expect("parses") else {
            panic!("expected control");
        };
        assert_eq!(
            elsewhere.request.command,
            control::ControlCommand::NewPane {
                command: None,
                argv: Some(vec!["grok".into()]),
                cwd: None,
                title: None,
                keep_open: false,
                focus: false,
                workspace: Some(9),
            }
        );
        assert!(parse_cli_args(vec!["split".into(), "--workspace".into()]).is_err());
        assert!(
            parse_cli_args(vec!["split".into(), "--workspace".into(), "later".into()]).is_err()
        );

        // Order-independent, like every other control command's flags.
        let ParsedCli::Control(trailing_target) = parse_cli_args(vec![
            "send-text".into(),
            "hi".into(),
            "--target".into(),
            "3".into(),
        ])
        .expect("parses") else {
            panic!("expected control");
        };
        assert_eq!(
            trailing_target.request.command,
            control::ControlCommand::SendText {
                target: Some(3),
                text: "hi".into(),
            }
        );
        assert!(
            parse_cli_args(vec!["send-text".into(), "one".into(), "two".into()]).is_err(),
            "send-text takes one block of text, not several"
        );

        let ParsedCli::Control(send_keys_dash) = parse_cli_args(vec![
            "send-keys".into(),
            "--".into(),
            "-n".into(),
            "hello".into(),
        ])
        .expect("parses") else {
            panic!("expected control");
        };
        assert_eq!(
            send_keys_dash.request.command,
            control::ControlCommand::SendKeys {
                target: None,
                keys: vec!["-n".into(), "hello".into()],
                literal: false,
            }
        );

        let ParsedCli::Control(switch) =
            parse_cli_args(vec!["switch-workspace".into(), "3".into()]).expect("parses")
        else {
            panic!("expected control");
        };
        assert_eq!(
            switch.request.command,
            control::ControlCommand::SwitchWorkspace { index: 3 }
        );

        let ParsedCli::Control(move_to) =
            parse_cli_args(vec!["move-to-workspace".into(), "4".into()]).expect("parses")
        else {
            panic!("expected control");
        };
        assert_eq!(
            move_to.request.command,
            control::ControlCommand::MoveToWorkspace { index: 4 }
        );

        assert!(parse_cli_args(vec!["run-action".into()]).is_err());
        assert!(parse_cli_args(vec!["switch-workspace".into(), "nope".into()]).is_err());
        assert!(parse_cli_args(vec!["capture-pane".into(), "--bogus".into()]).is_err());
    }

    #[test]
    fn cli_control_commands_reject_trailing_args() {
        assert!(parse_cli_args(vec!["focus".into(), "1".into(), "garbage".into()]).is_err());
        assert!(
            parse_cli_args(vec![
                "list-panes".into(),
                "--socket".into(),
                "/tmp/x".into()
            ])
            .is_err()
        );
        assert!(parse_cli_args(vec!["send-text".into(), "hi".into(), "extra".into()]).is_err());
    }

    #[test]
    fn cli_split_defaults_to_leaving_focus_put_and_takes_focus_in_either_order() {
        let split = |args: Vec<String>| {
            let ParsedCli::Control(control) = parse_cli_args(args).expect("parses") else {
                panic!("expected control");
            };
            control.request.command
        };
        let expected = |command: Option<&str>, focus: bool| control::ControlCommand::NewPane {
            command: command.map(str::to_string),
            argv: None,
            cwd: None,
            title: None,
            keep_open: false,
            focus,
            workspace: None,
        };

        assert_eq!(
            split(vec!["split".into(), "cargo test".into()]),
            expected(Some("cargo test"), false)
        );
        assert_eq!(split(vec!["split".into()]), expected(None, false));
        assert_eq!(
            split(vec!["split".into(), "cargo test".into(), "--focus".into()]),
            expected(Some("cargo test"), true)
        );
        assert_eq!(
            split(vec!["split".into(), "--focus".into(), "cargo test".into()]),
            expected(Some("cargo test"), true)
        );
        assert_eq!(
            split(vec![
                "split".into(),
                "cargo test".into(),
                "--cwd".into(),
                "/repo with space".into(),
                "--title".into(),
                "tests".into(),
                "--keep-open".into(),
                "--focus".into(),
            ]),
            control::ControlCommand::NewPane {
                command: Some("cargo test".into()),
                argv: None,
                cwd: Some("/repo with space".into()),
                title: Some("tests".into()),
                keep_open: true,
                focus: true,
                workspace: None,
            }
        );
        assert_eq!(
            split(vec!["split".into(), "--focus".into()]),
            expected(None, true)
        );

        assert!(
            parse_cli_args(vec!["split".into(), "one".into(), "two".into()]).is_err(),
            "a second positional is still rejected"
        );
    }

    #[test]
    fn cli_split_preserves_structured_argv_without_parsing_child_flags() {
        let ParsedCli::Control(control) = parse_cli_args(vec![
            "split".into(),
            "--cwd".into(),
            "/repo with spaces".into(),
            "--focus".into(),
            "--argv".into(),
            "/opt/tool with spaces".into(),
            "--literal".into(),
            "semi; $HOME 'quoted'".into(),
        ])
        .expect("structured argv parses") else {
            panic!("expected control");
        };
        assert_eq!(
            control.request.command,
            control::ControlCommand::NewPane {
                command: None,
                argv: Some(vec![
                    "/opt/tool with spaces".into(),
                    "--literal".into(),
                    "semi; $HOME 'quoted'".into(),
                ]),
                cwd: Some("/repo with spaces".into()),
                title: None,
                keep_open: false,
                focus: true,
                workspace: None,
            }
        );
        assert!(parse_cli_args(vec!["split".into(), "--argv".into()]).is_err());
        assert!(
            parse_cli_args(vec![
                "split".into(),
                "shell command".into(),
                "--argv".into(),
                "tool".into(),
            ])
            .is_err()
        );
    }

    #[test]
    fn cli_parses_status_set_and_clear() {
        let ParsedCli::Control(set) = parse_cli_args(vec![
            "status".into(),
            "blocked".into(),
            "--reason".into(),
            "needs approval".into(),
        ])
        .expect("parses") else {
            panic!("expected control");
        };
        assert_eq!(
            set.request.command,
            control::ControlCommand::SetStatus {
                target: None,
                status: Some("blocked".into()),
                reason: Some("needs approval".into()),
            }
        );

        let ParsedCli::Control(clear) =
            parse_cli_args(vec!["status".into(), "--clear".into()]).expect("parses")
        else {
            panic!("expected control");
        };
        assert_eq!(
            clear.request.command,
            control::ControlCommand::SetStatus {
                target: None,
                status: None,
                reason: None,
            }
        );
    }

    #[test]
    fn cli_rejects_ambiguous_or_malformed_status_commands() {
        for args in [
            vec!["status"],
            vec!["status", "--clear", "blocked"],
            vec!["status", "--clear", "--reason", "why"],
            vec!["status", "blocked", "--clear"],
            vec!["status", "blocked", "--reason"],
            vec!["status", "blocked", "--reason", "one", "extra"],
            vec!["status", "blocked", "--reason", "one", "--reason", "two"],
        ] {
            assert!(
                parse_cli_args(args.iter().map(|arg| (*arg).to_string()).collect()).is_err(),
                "accepted malformed args: {args:?}"
            );
        }
    }

    #[test]
    fn cli_parses_positional_and_namespaced_session_targets() {
        let attached = expect_run(parse_cli_args(vec!["dev".into()]).expect("parses"));
        assert_eq!(attached.attach_session.as_deref(), Some("dev"));
        assert_eq!(attached.session_command, SessionCommand::Dwim);
        let attached = expect_run(
            parse_cli_args(vec![
                "sessions".into(),
                "attach".into(),
                "dev".into(),
                "--read-only".into(),
            ])
            .expect("parses"),
        );
        assert_eq!(attached.attach_session.as_deref(), Some("dev"));
        assert_eq!(attached.session_command, SessionCommand::Attach);
        assert!(attached.read_only);
        let created = expect_run(
            parse_cli_args(vec![
                "sessions".into(),
                "new".into(),
                "work".into(),
                "--profile".into(),
                "rust-dev".into(),
            ])
            .expect("parses"),
        );
        assert_eq!(created.attach_session.as_deref(), Some("work"));
        assert_eq!(created.session_command, SessionCommand::New);
        assert_eq!(created.profile.as_deref(), Some("rust-dev"));
        let session =
            expect_run(parse_cli_args(vec!["--session".into(), "dev".into()]).expect("parses"));
        assert_eq!(session.attach_session.as_deref(), Some("dev"));
        assert!(parse_cli_args(vec!["sessions".into(), "kill".into()]).is_err());
        assert!(
            parse_cli_args(vec![
                "sessions".into(),
                "kill".into(),
                "dev/../other".into()
            ])
            .is_err()
        );
        assert!(
            parse_cli_args(vec!["sessions".into(), "kill".into(), "dev\nnext".into()]).is_err()
        );
        assert!(parse_cli_args(vec!["sessions".into(), "attach".into()]).is_err());
        assert!(parse_cli_args(vec!["sessions".into(), "new".into()]).is_err());
        assert!(
            parse_cli_args(vec![
                "sessions".into(),
                "new".into(),
                "dev".into(),
                "--read-only".into(),
            ])
            .is_err()
        );
    }

    #[test]
    fn cli_read_only_requires_attach_target() {
        assert!(parse_cli_args(vec!["--read-only".into()]).is_err());
        let args =
            expect_run(parse_cli_args(vec!["dev".into(), "--read-only".into()]).expect("parses"));
        assert_eq!(args.attach_session.as_deref(), Some("dev"));
        assert!(args.read_only);
    }

    #[test]
    fn cli_socket_without_control_command_errors() {
        assert!(parse_cli_args(vec!["--socket".into(), "/tmp/x".into()]).is_err());
        assert!(parse_cli_args(vec!["--socket".into(), "/tmp/x".into(), "dev".into()]).is_err());
    }

    #[test]
    fn cli_rejects_unknown_flags() {
        assert!(parse_cli_args(vec!["--nope".into()]).is_err());
    }

    #[test]
    fn cli_parses_pick_flag() {
        let default = expect_run(parse_cli_args(vec![]).expect("parses"));
        assert!(!default.pick);
        let picked = expect_run(parse_cli_args(vec!["--pick".into()]).expect("parses"));
        assert!(picked.pick);
        // `--pick` composes with a profile positional.
        let with_profile =
            expect_run(parse_cli_args(vec!["--pick".into(), "dev".into()]).expect("parses"));
        assert!(with_profile.pick);
        assert_eq!(with_profile.attach_session.as_deref(), Some("dev"));
    }

    #[test]
    fn cli_parses_remote_and_rejects_server_combo() {
        let args = expect_run(
            parse_cli_args(vec!["--remote".into(), "workbox".into(), "dev".into()])
                .expect("parses"),
        );
        assert_eq!(args.remote.as_deref(), Some("workbox"));
        assert_eq!(args.attach_session.as_deref(), Some("dev"));

        let bare_remote = expect_run(parse_cli_args(vec!["--remote".into()]).expect("parses"));
        assert_eq!(bare_remote.remote.as_deref(), Some(""));

        let remote_then_session = expect_run(
            parse_cli_args(vec!["--remote".into(), "--session".into(), "dev".into()])
                .expect("parses"),
        );
        assert_eq!(remote_then_session.remote.as_deref(), Some(""));
        assert_eq!(remote_then_session.attach_session.as_deref(), Some("dev"));

        assert!(matches!(
            parse_cli_args(vec![
                "--remote-serve".into(),
                "dev".into(),
            ])
            .expect("parses"),
            ParsedCli::RemoteServe {
                name,
                autostart: true
            } if name == "dev"
        ));
        assert!(matches!(
            parse_cli_args(vec![
                "--remote-serve-existing".into(),
                "dev".into(),
            ])
            .expect("parses"),
            ParsedCli::RemoteServe {
                name,
                autostart: false
            } if name == "dev"
        ));

        assert!(
            parse_cli_args(vec![
                "--remote".into(),
                "workbox".into(),
                "--server".into(),
                "dev".into(),
            ])
            .is_err()
        );
        assert!(parse_cli_args(vec!["--remote".into(), "ssh://".into()]).is_err());
    }

    #[test]
    fn cli_rejects_a_flag_where_a_name_belongs() {
        // Every one of these used to consume the following flag as a literal name, so
        // `--server --pick` started a session server for a session called `--pick`.
        for args in [
            vec!["sessions", "attach", "--read-only"],
            vec!["sessions", "new", "--profile"],
            vec!["--session", "--pick"],
            vec!["--server", "--pick"],
            vec!["--session", "dev", "--fresh-server", "--pick"],
            vec!["--remote-serve", "--pick"],
            vec!["--remote-serve-existing", "--pick"],
            vec!["sessions", "kill", "--remote"],
            vec!["--profile", "--read-only"],
            vec!["--config", "--read-only"],
            vec!["--socket", "--read-only", "list-panes"],
            vec!["run-action", "--focus"],
            vec!["status", "blocked", "--reason", "--clear"],
            vec!["sessions", "list", "--format", "--remote"],
        ] {
            let parsed = parse_cli_args(args.iter().map(|arg| (*arg).to_string()).collect());
            assert!(parsed.is_err(), "accepted a flag as a value: {args:?}");
        }
        // A lone `-` is still an ordinary value.
        let dash = expect_run(
            parse_cli_args(vec![
                "sessions".into(),
                "new".into(),
                "x".into(),
                "--profile".into(),
                "-".into(),
            ])
            .expect("parses"),
        );
        assert_eq!(dash.profile.as_deref(), Some("-"));
    }

    /// Plain lines are the default so a shell pipeline needs no `jq` on either end; `--json` is
    /// the opt-in for callers that want groups, badges, or a live-updating row set.
    #[test]
    fn pick_defaults_to_plain_lines_and_takes_json_as_an_opt_in() {
        let plain = match parse_cli_args(vec!["pick".into(), "--title".into(), "Branch".into()])
            .expect("plain pick parses")
        {
            ParsedCli::Pick(pick) => pick,
            other => panic!("expected a pick command, got {other:?}"),
        };
        assert!(!plain.json);
        assert_eq!(plain.title.as_deref(), Some("Branch"));

        let json =
            match parse_cli_args(vec!["pick".into(), "--json".into()]).expect("json pick parses") {
                ParsedCli::Pick(pick) => pick,
                other => panic!("expected a pick command, got {other:?}"),
            };
        assert!(json.json);

        assert!(
            parse_cli_args(vec!["pick".into(), "--jsn".into()]).is_err(),
            "a mistyped flag must not be swallowed"
        );
    }

    #[test]
    fn subscribe_accepts_an_optional_event_filter() {
        let ParsedCli::Subscribe(command) = parse_cli_args(vec![
            "subscribe".into(),
            "pane-exited".into(),
            "workspace-switched".into(),
        ])
        .expect("subscribe parses") else {
            panic!("expected subscribe");
        };
        assert_eq!(command.events, ["pane-exited", "workspace-switched"]);
        assert!(parse_cli_args(vec!["subscribe".into(), "--unknown".into()]).is_err());
    }

    /// `notify` is how a script reports an off-screen result, so its parsing has to survive
    /// messages that look like flags and reject a level it cannot honour.
    #[test]
    fn notify_parses_message_title_and_level() {
        let parsed = parse_cli_args(vec![
            "notify".into(),
            "deploy finished".into(),
            "--title".into(),
            "Deploy".into(),
            "--level".into(),
            "error".into(),
        ])
        .expect("notify parses");
        match parsed {
            ParsedCli::Control(control) => match control.request.command {
                control::ControlCommand::Notify {
                    message,
                    title,
                    level,
                } => {
                    assert_eq!(message, "deploy finished");
                    assert_eq!(title.as_deref(), Some("Deploy"));
                    assert_eq!(level, control::NotifyLevel::Error);
                }
                other => panic!("wrong command: {other:?}"),
            },
            other => panic!("wrong parse: {other:?}"),
        }

        assert!(
            parse_cli_args(vec!["notify".into()]).is_err(),
            "a message is required"
        );
        assert!(
            parse_cli_args(vec![
                "notify".into(),
                "x".into(),
                "--level".into(),
                "loud".into()
            ])
            .is_err(),
            "an unknown level must not be silently downgraded"
        );
        // `--` lets a message that starts with a dash through.
        let dashed = parse_cli_args(vec!["notify".into(), "--".into(), "-1 test failed".into()])
            .expect("dashed message parses");
        match dashed {
            ParsedCli::Control(control) => match control.request.command {
                control::ControlCommand::Notify { message, .. } => {
                    assert_eq!(message, "-1 test failed");
                }
                other => panic!("wrong command: {other:?}"),
            },
            other => panic!("wrong parse: {other:?}"),
        }
    }

    #[test]
    fn cli_control_commands_reject_launch_only_flags() {
        for args in [
            vec!["--config", "/tmp/other.toml", "list-panes"],
            vec!["--read-only", "list-panes"],
            vec!["--pick", "metrics"],
            // `--remote` qualifies a session target; on its own it names nothing a control command
            // can address, and answering from this machine instead would be the wrong host.
            vec!["--remote", "workbox", "list-panes"],
            vec!["--remote", "workbox", "publish"],
            vec!["--remote", "workbox", "capture-pane"],
            // A UI command has no session server to reach, here or anywhere.
            vec!["--remote", "workbox", "--session", "dev", "publish"],
            vec!["--remote", "workbox", "--session", "dev", "focus", "3"],
        ] {
            let parsed = parse_cli_args(args.iter().map(|arg| (*arg).to_string()).collect());
            assert!(parsed.is_err(), "silently ignored a launch flag: {args:?}");
        }
        assert!(
            parse_cli_args(vec![
                "--remote".into(),
                "workbox".into(),
                "list-panes".into()
            ])
            .expect_err("rejected")
            .contains("--session"),
            "the remote rejection should say what it was missing"
        );
    }

    /// The same command, the same endpoint kind, one hop away. What `--remote` changes is which
    /// machine's session answers - not which commands are allowed or how the answer is read.
    #[test]
    fn remote_plus_session_reaches_that_session_on_that_host() {
        let parsed = parse_cli_args(
            ["--remote", "workbox", "--session", "dev", "list-panes"]
                .iter()
                .map(|arg| (*arg).to_string())
                .collect(),
        )
        .expect("a remote session is a valid control target");

        let ParsedCli::Control(control) = parsed else {
            panic!("expected a control command");
        };
        assert_eq!(
            control.endpoint,
            ControlEndpoint::Remote {
                target: "workbox".to_string(),
                session: "dev".to_string(),
            }
        );
        assert!(control.endpoint.is_session());
    }

    /// Waits and prompts are server-owned, and a remote session server owns them just as much as a
    /// local one. Gating them on `Session` alone would have refused the whole point of forwarding.
    #[test]
    fn a_remote_session_may_be_waited_on_and_prompted() {
        for args in [
            vec![
                "--remote",
                "workbox",
                "--session",
                "dev",
                "agents",
                "wait",
                "--target",
                "3",
                "--until",
                "idle",
            ],
            vec![
                "--remote",
                "workbox",
                "--session",
                "dev",
                "agents",
                "prompt",
                "--target",
                "3",
                "run the tests",
            ],
        ] {
            assert!(
                parse_cli_args(args.iter().map(|arg| (*arg).to_string()).collect()).is_ok(),
                "refused {args:?}"
            );
        }
    }

    #[test]
    fn hidden_server_start_carries_only_a_valid_nonce() {
        let nonce = "0123456789abcdef0123456789abcdef";
        let parsed =
            parse_cli_args(vec!["--server-start".into(), "dev".into(), nonce.into()]).unwrap();
        let ParsedCli::Server {
            name,
            fresh,
            config_path,
            startup_nonce,
        } = parsed
        else {
            panic!("expected server");
        };
        assert_eq!(name, "dev");
        assert!(!fresh);
        assert!(config_path.is_none());
        assert_eq!(startup_nonce.as_deref(), Some(nonce));
        assert!(
            parse_cli_args(vec![
                "--server-start".into(),
                "dev".into(),
                "../not-a-nonce".into(),
            ])
            .is_err()
        );
    }

    #[test]
    fn cli_carries_config_path_to_every_command_that_loads_config() {
        // `--config` used to be parsed and then dropped for everything but the UI, so a server or a
        // remote listing quietly read the developer's own config instead.
        let ParsedCli::Server { config_path, .. } = parse_cli_args(vec![
            "--config".into(),
            "/tmp/alt.toml".into(),
            "--session".into(),
            "dev".into(),
            "--server".into(),
        ])
        .expect("parses") else {
            panic!("expected server");
        };
        assert_eq!(config_path.as_deref(), Some("/tmp/alt.toml"));

        let ParsedCli::Sessions(SessionsCommand::List { config_path, .. }) = parse_cli_args(vec![
            "--config".into(),
            "/tmp/alt.toml".into(),
            "sessions".into(),
            "list".into(),
        ])
        .expect("parses") else {
            panic!("expected sessions list");
        };
        assert_eq!(config_path.as_deref(), Some("/tmp/alt.toml"));

        let ParsedCli::Sessions(SessionsCommand::Kill { config_path, .. }) = parse_cli_args(vec![
            "--config".into(),
            "/tmp/alt.toml".into(),
            "sessions".into(),
            "kill".into(),
            "dev".into(),
        ])
        .expect("parses") else {
            panic!("expected sessions kill");
        };
        assert_eq!(config_path.as_deref(), Some("/tmp/alt.toml"));
    }

    #[test]
    fn cli_server_modes_reject_read_only_instead_of_dropping_it() {
        assert!(
            parse_cli_args(vec![
                "--session".into(),
                "dev".into(),
                "--read-only".into(),
                "--server".into(),
            ])
            .is_err()
        );
        assert!(
            parse_cli_args(vec![
                "--session".into(),
                "dev".into(),
                "--read-only".into(),
                "--fresh-server".into(),
            ])
            .is_err()
        );
    }

    #[test]
    fn cli_split_rejects_a_mistyped_flag_rather_than_running_it() {
        // `--focu` reaching the shell as the command is worse than an error.
        assert!(parse_cli_args(vec!["split".into(), "--focu".into()]).is_err());
        let ParsedCli::Control(dashed) =
            parse_cli_args(vec!["split".into(), "--".into(), "--odd-command".into()])
                .expect("parses")
        else {
            panic!("expected control");
        };
        assert_eq!(
            dashed.request.command,
            control::ControlCommand::NewPane {
                command: Some("--odd-command".into()),
                argv: None,
                cwd: None,
                title: None,
                keep_open: false,
                focus: false,
                workspace: None,
            }
        );
    }

    #[test]
    fn cli_repeated_socket_is_rejected_like_every_other_repeatable_flag() {
        assert!(
            parse_cli_args(vec![
                "--socket".into(),
                "/tmp/a".into(),
                "--socket".into(),
                "/tmp/b".into(),
                "list-panes".into(),
            ])
            .is_err()
        );
    }

    #[test]
    fn cli_help_wins_over_the_rest_of_a_mistyped_line() {
        // Someone who has already got the line wrong is exactly who is asking for help.
        for args in [
            vec!["attach", "--help"],
            vec!["--nope", "--help"],
            vec!["--socket", "-h"],
        ] {
            assert!(
                matches!(
                    parse_cli_args(args.iter().map(|arg| (*arg).to_string()).collect()),
                    Ok(ParsedCli::Help { .. })
                ),
                "help did not win in {args:?}"
            );
        }
    }

    fn expect_run(parsed: ParsedCli) -> CliArgs {
        match parsed {
            ParsedCli::Run(args) => args,
            other => panic!("expected run args, got {other:?}"),
        }
    }
}
