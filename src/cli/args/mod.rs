//! Parsing `rozi`'s argv into the [`ParsedCli`] shape `main.rs` dispatches on.
//!
//! Nothing here performs work: every variant is a decision already made, so a bad command line
//! fails before any socket, config file, or session is touched.

use std::path::PathBuf;

use crate::{control, session};

mod extensions;
mod sessions;
mod skill;

#[cfg(test)]
pub(super) use extensions::HELP_SECTIONS as EXTENSIONS_HELP_SECTIONS;
pub(crate) use extensions::print_check_help as print_extensions_check_help;
pub(crate) use extensions::print_help as print_extensions_help;
#[cfg(test)]
pub(super) use sessions::HELP_SECTIONS as SESSIONS_HELP_SECTIONS;
pub(crate) use sessions::print_help as print_sessions_help;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CliArgs {
    pub(crate) session_command: SessionCommand,
    pub(crate) profile: Option<String>,
    pub(crate) config_path: Option<String>,
    pub(crate) attach_session: Option<String>,
    /// Force the startup session picker, overriding whatever `[session] startup` selects. A target
    /// wins over it, and `--remote` skips it: the session lives on the far host, which local
    /// discovery does not describe.
    pub(crate) pick: bool,
    pub(crate) read_only: bool,
    /// SSH remote host alias or `ssh://` URL (`--remote`).
    pub(crate) remote: Option<String>,
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
    pub(super) socket: Option<PathBuf>,
    pub(super) request: control::ControlRequest,
    /// Explicit report format. Without one, a terminal gets human output and a pipe gets JSON.
    pub(super) output_format: Option<ListFormat>,
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
    Skill(SkillCommand),
    SkillHelp,
    Sessions(SessionsCommand),
    Extensions(ExtensionsCommand),
    SessionsHelp,
    ExtensionsHelp,
    ExtensionsCheckHelp,
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
    },
    /// Hidden remote-side stdio proxy (`--remote-serve <NAME>`).
    RemoteServe {
        name: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionsCommand {
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

const RETIRED: &[(&str, &str)] = &[
    ("list-sessions", "sessions list"),
    ("kill-session", "sessions kill"),
    ("attach", "sessions attach"),
    ("new", "sessions new"),
    ("list-extensions", "extensions list"),
    ("new-extension", "extensions new"),
    ("check-extension", "extensions check"),
    ("new-pane", "split"),
];

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
    let namespace_index = args
        .iter()
        .position(|arg| matches!(arg.as_str(), "sessions" | "extensions" | "skill"));
    if help_index.is_some_and(|help| namespace_index.is_none_or(|namespace| help < namespace)) {
        return Ok(ParsedCli::Help {
            advanced: args.iter().any(|arg| arg == "--advanced"),
        });
    }
    let mut cli = CliArgs::default();
    let mut socket: Option<PathBuf> = None;
    let mut socket_flag_seen = false;
    let mut session_flag_target = false;
    let mut iter = args.into_iter().peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--version" | "-V" => return Ok(ParsedCli::Version),
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
                        if !session_flag_target || cli.session_command != SessionCommand::Dwim {
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
                });
            }
            "--fresh-server" => {
                let Some(name) = cli.attach_session.take() else {
                    return Err("--fresh-server must follow --session <NAME>".to_string());
                };
                if !session_flag_target || cli.session_command != SessionCommand::Dwim {
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
                });
            }
            "--remote-serve" => {
                let name = require_value(&mut iter, "--remote-serve requires a session name")?;
                reject_trailing_control_args(&mut iter, "--remote-serve")?;
                return Ok(ParsedCli::RemoteServe { name });
            }
            "--remote" => {
                // Host is optional when `[remote] default_host` is set (resolved in app::run).
                let target = match iter.peek().map(|s| s.as_str()) {
                    Some(next) if !next.starts_with('-') && !matches!(next, "sessions") => {
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
                session_flag_target = true;
            }
            "--profile" => {
                let profile = require_value(&mut iter, "--profile requires a profile name")?;
                if cli.profile.replace(profile).is_some() {
                    return Err("--profile specified more than once".to_string());
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
            "list-panes" => {
                let output_format = parse_output_format(&mut iter, "list-panes")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket: reject_launch_flags(&cli, socket)?,
                    request: control_request(control::ControlCommand::ListPanes),
                    output_format,
                }));
            }
            "metrics" => {
                let output_format = parse_output_format(&mut iter, "metrics")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket: reject_launch_flags(&cli, socket)?,
                    request: control_request(control::ControlCommand::Metrics),
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
                return Ok(ParsedCli::Control(ControlCli {
                    socket: reject_launch_flags(&cli, socket)?,
                    request: control_request(control::ControlCommand::Focus { target }),
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
                return Ok(ParsedCli::Control(ControlCli {
                    socket: reject_launch_flags(&cli, socket)?,
                    request: control_request(control::ControlCommand::SendText { target, text }),
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
                return Ok(ParsedCli::Control(ControlCli {
                    socket: reject_launch_flags(&cli, socket)?,
                    request: control_request(control::ControlCommand::SendKeys {
                        target,
                        keys,
                        literal,
                    }),
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
                return Ok(ParsedCli::Control(ControlCli {
                    socket: reject_launch_flags(&cli, socket)?,
                    request: control_request(control::ControlCommand::Notify {
                        message,
                        title,
                        level,
                    }),
                    output_format: None,
                }));
            }
            "status" => {
                let first = iter
                    .next()
                    .ok_or_else(|| "status requires a value or --clear".to_string())?;
                let (status, reason) = if first == "--clear" {
                    reject_trailing_control_args(&mut iter, "status --clear")?;
                    (None, None)
                } else {
                    if first.starts_with('-') {
                        return Err(format!("unexpected status flag `{first}`"));
                    }
                    let reason = match iter.next() {
                        None => None,
                        Some(flag) if flag == "--reason" => {
                            Some(require_value(&mut iter, "--reason requires text")?)
                        }
                        Some(extra) => {
                            return Err(format!("unexpected argument `{extra}` after status"));
                        }
                    };
                    reject_trailing_control_args(&mut iter, "status")?;
                    (Some(first), reason)
                };
                return Ok(ParsedCli::Control(ControlCli {
                    socket: reject_launch_flags(&cli, socket)?,
                    request: control_request(control::ControlCommand::SetStatus {
                        target: None,
                        status,
                        reason,
                    }),
                    output_format: None,
                }));
            }
            "publish" => {
                reject_trailing_control_args(&mut iter, "publish")?;
                return Ok(ParsedCli::Publish(PublishCli {
                    socket: reject_launch_flags(&cli, socket)?,
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
                    socket: reject_launch_flags(&cli, socket)?,
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
                    title,
                    placeholder,
                    socket: reject_launch_flags(&cli, socket)?,
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
                return Ok(ParsedCli::Control(ControlCli {
                    socket: reject_launch_flags(&cli, socket)?,
                    request: control_request(control::ControlCommand::NewPane {
                        command,
                        argv,
                        cwd,
                        title,
                        keep_open,
                        focus,
                        workspace,
                    }),
                    output_format: None,
                }));
            }
            "run-action" => {
                let action = require_value(&mut iter, "run-action requires an action id")?;
                reject_trailing_control_args(&mut iter, "run-action")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket: reject_launch_flags(&cli, socket)?,
                    request: control_request(control::ControlCommand::RunAction { action }),
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
                return Ok(ParsedCli::Control(ControlCli {
                    socket: reject_launch_flags(&cli, socket)?,
                    request: control_request(control::ControlCommand::CapturePane {
                        target,
                        scrollback,
                    }),
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
                return Ok(ParsedCli::Control(ControlCli {
                    socket: reject_launch_flags(&cli, socket)?,
                    request: control_request(control::ControlCommand::SwitchWorkspace { index }),
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
                return Ok(ParsedCli::Control(ControlCli {
                    socket: reject_launch_flags(&cli, socket)?,
                    request: control_request(control::ControlCommand::MoveToWorkspace { index }),
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
                if let Some((_, replacement)) = RETIRED.iter().find(|(retired, _)| *retired == name)
                {
                    return Err(format!("`{name}` was renamed to `{replacement}`"));
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

/// Pass `socket` through to a control command, rejecting launch-only options.
///
/// A control command talks to the local UI endpoint named by `--socket`/`ROZI_SOCKET` and never
/// loads config or attaches anything. Accepting these silently let `rozi --remote box list-panes`
/// answer from the *local* rozi while the caller believed it had reached another host.
pub(super) fn reject_launch_flags(
    cli: &CliArgs,
    socket: Option<PathBuf>,
) -> std::result::Result<Option<PathBuf>, String> {
    let offender = if cli.remote.is_some() {
        "--remote"
    } else if cli.config_path.is_some() {
        "--config"
    } else if cli.read_only {
        "--read-only"
    } else if cli.pick {
        "--pick"
    } else if cli.profile.is_some() {
        "--profile"
    } else if cli.attach_session.is_some() {
        "a session target"
    } else {
        return Ok(socket);
    };
    let hint = if offender == "--remote" {
        " (use `sessions list --remote` or `sessions kill --remote` to reach another host)"
    } else {
        ""
    };
    Err(format!(
        "{offender} does not apply to control commands{hint}"
    ))
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
        let retired =
            expect_run(parse_cli_args(vec!["--session".into(), "attach".into()]).expect("parses"));
        assert_eq!(retired.attach_session.as_deref(), Some("attach"));
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
            control.socket.as_deref(),
            Some(std::path::Path::new("/tmp/rozi.sock"))
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
            ParsedCli::RemoteServe { name } if name == "dev"
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
        // A control command talks to the local UI endpoint, so silently dropping `--remote` would
        // answer from this machine while the caller believed it had reached another host.
        for args in [
            vec!["--remote", "workbox", "list-panes"],
            vec!["--config", "/tmp/other.toml", "list-panes"],
            vec!["--read-only", "list-panes"],
            vec!["--pick", "metrics"],
            vec!["--remote", "workbox", "publish"],
            vec!["--remote", "workbox", "capture-pane"],
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
            .contains("sessions list --remote"),
            "the remote rejection should point at the command that does reach a host"
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
    fn retired_cli_spellings_report_their_replacements() {
        for (retired, replacement) in RETIRED {
            let error = parse_cli_args(vec![(*retired).to_string()]).expect_err("must be retired");
            assert_eq!(error, format!("`{retired}` was renamed to `{replacement}`"));
        }
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
