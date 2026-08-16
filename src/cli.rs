use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use tui_lipan::Result;

use crate::platform::ipc::{EndpointRegistry, IpcEndpoint};
use crate::{control, session, skill};

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
    socket: Option<PathBuf>,
    request: control::ControlRequest,
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
    socket: Option<PathBuf>,
}

#[derive(Debug)]
pub(crate) struct SubscribeCli {
    socket: Option<PathBuf>,
    events: Vec<String>,
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
    ListSessions {
        format: ListFormat,
        remote: Option<String>,
        config_path: Option<String>,
    },
    ListExtensions {
        json: bool,
        verbose: bool,
        config_path: Option<String>,
    },
    NewExtension {
        id: String,
    },
    CheckExtension {
        path: PathBuf,
        json: bool,
    },
    KillSession {
        name: String,
        remote: Option<String>,
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
        return parse_skill_args(&args[1..]);
    }
    // Help wins over whatever else was typed, and from wherever it was typed: someone who has
    // already mistyped the rest of the line is exactly who is asking for it. `--advanced` is only
    // ever read here, so it can never be silently swallowed by another command.
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
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
            "list-sessions" => {
                let mut format = ListFormat::Text;
                let mut remote = None;
                while let Some(flag) = iter.next() {
                    match flag.as_str() {
                        "--format" => {
                            let value = require_value(&mut iter, "--format requires text or json")?;
                            format = match value.as_str() {
                                "text" => ListFormat::Text,
                                "json" => ListFormat::Json,
                                other => {
                                    return Err(format!(
                                        "unknown list-sessions --format `{other}` (expected text or json)"
                                    ));
                                }
                            };
                        }
                        "--remote" => {
                            let target = require_value(
                                &mut iter,
                                "list-sessions --remote requires a host alias or ssh:// URL",
                            )?;
                            session::remote::parse_remote_target(&target)?;
                            remote = Some(target);
                        }
                        other => {
                            return Err(format!(
                                "unexpected argument `{other}` after list-sessions"
                            ));
                        }
                    }
                }
                return Ok(ParsedCli::ListSessions {
                    format,
                    remote,
                    config_path: cli.config_path,
                });
            }
            "list-extensions" => {
                let mut json = false;
                let mut verbose = false;
                for flag in iter.by_ref() {
                    match flag.as_str() {
                        "--json" if !json => json = true,
                        "--json" => {
                            return Err(
                                "list-extensions --json specified more than once".to_string()
                            );
                        }
                        "--verbose" if !verbose => verbose = true,
                        "--verbose" => {
                            return Err(
                                "list-extensions --verbose specified more than once".to_string()
                            );
                        }
                        other => {
                            return Err(format!(
                                "unexpected argument `{other}` after list-extensions"
                            ));
                        }
                    }
                }
                return Ok(ParsedCli::ListExtensions {
                    json,
                    verbose,
                    config_path: cli.config_path,
                });
            }
            "new-extension" => {
                let id = require_value(&mut iter, "new-extension requires an extension id")?;
                reject_trailing_control_args(&mut iter, "new-extension")?;
                return Ok(ParsedCli::NewExtension { id });
            }
            "check-extension" => {
                let path = PathBuf::from(require_value(
                    &mut iter,
                    "check-extension requires an extension directory or extension.toml path",
                )?);
                let mut json = false;
                for flag in iter.by_ref() {
                    match flag.as_str() {
                        "--json" if !json => json = true,
                        "--json" => {
                            return Err(
                                "check-extension --json specified more than once".to_string()
                            );
                        }
                        other => {
                            return Err(format!(
                                "unexpected argument `{other}` after check-extension"
                            ));
                        }
                    }
                }
                return Ok(ParsedCli::CheckExtension { path, json });
            }
            "kill-session" => {
                let name = require_value(&mut iter, "kill-session requires a session name")?;
                if !session::discovery::valid_attach_target(&name) {
                    return Err("invalid session name".to_string());
                }
                let mut remote = None;
                while let Some(flag) = iter.next() {
                    match flag.as_str() {
                        "--remote" => {
                            let target = require_value(
                                &mut iter,
                                "kill-session --remote requires a host alias or ssh:// URL",
                            )?;
                            session::remote::parse_remote_target(&target)?;
                            remote = Some(target);
                        }
                        other => {
                            return Err(format!(
                                "unexpected argument `{other}` after kill-session"
                            ));
                        }
                    }
                }
                return Ok(ParsedCli::KillSession {
                    name,
                    remote,
                    config_path: cli.config_path,
                });
            }
            "attach" => {
                if cli.attach_session.is_some() {
                    return Err("session target specified more than once".to_string());
                }
                cli.attach_session =
                    Some(require_value(&mut iter, "attach requires a session name")?);
                cli.session_command = SessionCommand::Attach;
            }
            "new" => {
                if cli.attach_session.is_some() {
                    return Err("session target specified more than once".to_string());
                }
                cli.attach_session = Some(require_value(&mut iter, "new requires a session name")?);
                cli.session_command = SessionCommand::New;
            }
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
                    Some(next)
                        if !next.starts_with('-')
                            && !matches!(
                                next,
                                "attach" | "new" | "list-sessions" | "kill-session"
                            ) =>
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
                reject_trailing_control_args(&mut iter, "list-panes")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket: reject_launch_flags(&cli, socket)?,
                    request: control_request(control::ControlCommand::ListPanes),
                }));
            }
            "metrics" => {
                reject_trailing_control_args(&mut iter, "metrics")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket: reject_launch_flags(&cli, socket)?,
                    request: control_request(control::ControlCommand::Metrics),
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
                }));
            }
            "send-text" => {
                let text = iter
                    .next()
                    .ok_or_else(|| "send-text requires literal text".to_string())?;
                reject_trailing_control_args(&mut iter, "send-text")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket: reject_launch_flags(&cli, socket)?,
                    request: control_request(control::ControlCommand::SendText {
                        target: None,
                        text,
                    }),
                }));
            }
            "send-keys" => {
                let mut literal = false;
                let mut keys = Vec::new();
                let mut passthrough = false;
                for arg in iter.by_ref() {
                    if !passthrough {
                        if arg == "--" {
                            passthrough = true;
                            continue;
                        }
                        if arg == "-l" || arg == "--literal" {
                            literal = true;
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
                        target: None,
                        keys,
                        literal,
                    }),
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
            "split" | "new-pane" => {
                let mut command = None;
                let mut argv = None;
                let mut cwd = None;
                let mut title = None;
                let mut keep_open = false;
                let mut focus = false;
                let mut passthrough = false;
                while let Some(arg) = iter.next() {
                    match arg.as_str() {
                        "--" if !passthrough => passthrough = true,
                        "--focus" if !passthrough => focus = true,
                        "--keep-open" if !passthrough => keep_open = true,
                        "--argv" if !passthrough && command.is_none() => {
                            let direct: Vec<String> = iter.by_ref().collect();
                            crate::pane_launch::PaneLaunch::direct(direct.clone())?;
                            argv = Some(direct);
                            break;
                        }
                        "--argv" if !passthrough => {
                            return Err(
                                "new-pane accepts either COMMAND or --argv, not both".to_string()
                            );
                        }
                        "--cwd" if !passthrough && cwd.is_none() => {
                            cwd = Some(require_value(
                                &mut iter,
                                "new-pane --cwd requires a directory",
                            )?);
                        }
                        "--cwd" if !passthrough => {
                            return Err("new-pane --cwd specified more than once".to_string());
                        }
                        "--title" if !passthrough && title.is_none() => {
                            title =
                                Some(require_value(&mut iter, "new-pane --title requires text")?);
                        }
                        "--title" if !passthrough => {
                            return Err("new-pane --title specified more than once".to_string());
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
                    }),
                }));
            }
            "run-action" => {
                let action = require_value(&mut iter, "run-action requires an action id")?;
                reject_trailing_control_args(&mut iter, "run-action")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket: reject_launch_flags(&cli, socket)?,
                    request: control_request(control::ControlCommand::RunAction { action }),
                }));
            }
            "capture-pane" => {
                let mut target = None;
                let mut scrollback = None;
                while let Some(next) = iter.next() {
                    match next.as_str() {
                        "--target" => {
                            let value = iter
                                .next()
                                .ok_or_else(|| "--target requires a pane id".to_string())?;
                            target =
                                Some(value.parse().map_err(|_| {
                                    "--target requires a numeric pane id".to_string()
                                })?);
                        }
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
        return Err("--read-only cannot be used with new".to_string());
    }
    if cli.profile.is_some() && cli.session_command != SessionCommand::New {
        return Err("--profile can only be used with new".to_string());
    }
    Ok(ParsedCli::Run(cli))
}

/// Take the value that must follow a name-taking flag or verb, rejecting a flag-shaped one.
///
/// Session names, profile names, and action ids all accept `-`, so a bare `next()` silently eats
/// the following option: without this, `rozi attach --read-only` hunts for a session literally
/// named `--read-only`, and `rozi --server --pick` starts a session server for one. A lone `-` is
/// still a legal value; a real path that begins with a dash can be written `./-name`.
fn require_value(
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

/// Pass `socket` through to a control command, rejecting launch-only options.
///
/// A control command talks to the local UI endpoint named by `--socket`/`ROZI_SOCKET` and never
/// loads config or attaches anything. Accepting these silently let `rozi --remote box list-panes`
/// answer from the *local* rozi while the caller believed it had reached another host.
fn reject_launch_flags(
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
        " (use `list-sessions --remote` or `kill-session --remote` to reach another host)"
    } else {
        ""
    };
    Err(format!(
        "{offender} does not apply to control commands{hint}"
    ))
}

fn reject_trailing_control_args(
    iter: &mut impl Iterator<Item = String>,
    command: &str,
) -> std::result::Result<(), String> {
    if let Some(extra) = iter.next() {
        Err(format!("unexpected argument `{extra}` after {command}"))
    } else {
        Ok(())
    }
}

fn control_request(command: control::ControlCommand) -> control::ControlRequest {
    control::ControlRequest {
        command,
        source_pane: std::env::var("ROZI_PANE").ok().and_then(|v| v.parse().ok()),
        extension: crate::config::provenance_from_process(),
    }
}

fn discover_socket(explicit: Option<PathBuf>) -> std::result::Result<PathBuf, String> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    if let Some(path) = std::env::var_os("ROZI_SOCKET").map(PathBuf::from) {
        return Ok(path);
    }
    let dir =
        control::runtime_dir().map_err(|err| format!("could not inspect runtime dir: {err}"))?;
    let live: Vec<PathBuf> = EndpointRegistry::list_live_control_endpoints(&dir)
        .map_err(|err| format!("could not read {}: {err}", dir.display()))?
        .into_iter()
        .map(|endpoint| endpoint.path().to_path_buf())
        .collect();
    match live.as_slice() {
        [path] => Ok(path.clone()),
        [] => {
            Err("no live rozi control socket found (set ROZI_SOCKET or pass --socket)".to_string())
        }
        _ => Err("multiple live rozi sockets found; pass --socket PATH".to_string()),
    }
}

/// Bridge stdin/stdout to a `publish` control stream for the calling pane.
///
/// Runs until either side closes: rozi withdraws the pane's rows on EOF, so a publisher that
/// exits or crashes cleans up by construction and never has to say so.
pub(crate) fn run_publish_cli(command: PublishCli) -> Result<()> {
    let path = match discover_socket(command.socket) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    };
    let source_pane = std::env::var("ROZI_PANE")
        .ok()
        .and_then(|value| value.parse::<crate::state::PaneId>().ok());
    let mut stream = match IpcEndpoint::at_path(&path).connect() {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("could not connect to {}: {err}", path.display());
            std::process::exit(2);
        }
    };
    let mut request = control_request(control::ControlCommand::Publish);
    request.source_pane = source_pane;
    writeln!(stream, "{}", serde_json::to_string(&request).unwrap())?;

    let reader_stream = stream.try_clone()?;
    let mut reply = String::new();
    let mut reader = BufReader::new(reader_stream);
    reader.read_line(&mut reply)?;
    let value: serde_json::Value = serde_json::from_str(&reply).unwrap_or_default();
    if value.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
            eprintln!("{error}");
        }
        std::process::exit(1);
    }

    // Activations arrive whenever the user clicks; forward them as they come rather than pairing
    // them with anything this process writes.
    std::thread::spawn(move || {
        for line in reader.lines() {
            let Ok(line) = line else { return };
            let mut stdout = std::io::stdout().lock();
            // A publisher that stopped reading its activations has gone away; end the thread
            // rather than spinning on a broken pipe.
            if writeln!(stdout, "{line}")
                .and_then(|()| stdout.flush())
                .is_err()
            {
                return;
            }
        }
    });

    for line in std::io::stdin().lock().lines() {
        let line = line?;
        writeln!(stream, "{line}")?;
    }
    Ok(())
}

/// Print matching application events as newline-delimited JSON until the connection closes.
pub(crate) fn run_subscribe_cli(command: SubscribeCli) -> Result<()> {
    let path = match discover_socket(command.socket) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    };
    let mut stream = match IpcEndpoint::at_path(&path).connect() {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("could not connect to {}: {err}", path.display());
            std::process::exit(2);
        }
    };
    let request = control_request(control::ControlCommand::Subscribe {
        events: command.events,
    });
    writeln!(stream, "{}", serde_json::to_string(&request).unwrap())?;

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;
    let value: serde_json::Value = serde_json::from_str(&response).unwrap_or_default();
    if value.get("ok").and_then(|value| value.as_bool()) != Some(true) {
        if let Some(error) = value.get("error").and_then(|value| value.as_str()) {
            eprintln!("{error}");
        }
        std::process::exit(1);
    }

    let mut stdout = std::io::stdout().lock();
    for line in reader.lines() {
        writeln!(stdout, "{}", line?)?;
        stdout.flush()?;
    }
    Ok(())
}

pub(crate) fn run_pick_cli(command: PickCli) -> Result<()> {
    let path = match discover_socket(command.socket) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    };
    let mut stream = match IpcEndpoint::at_path(&path).connect() {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("could not connect to {}: {err}", path.display());
            std::process::exit(2);
        }
    };
    // In `--json` mode the first stdin line *is* the picker request, which is the only way to
    // declare `width` and `actions` - they have no flag spelling, and a mini-language inside one
    // would be worse than the object the caller is already writing. Its `rows`, if present, become
    // the initial set. Plain mode is a dumb list and needs none of it.
    let mut first_line = String::new();
    let mut opening_rows = None;
    let (title, placeholder, width, actions) = if command.json {
        std::io::stdin().lock().read_line(&mut first_line)?;
        let spec: serde_json::Value =
            serde_json::from_str(first_line.trim()).unwrap_or(serde_json::Value::Null);
        if spec.get("rows").is_some() {
            opening_rows = Some(serde_json::json!({ "rows": spec["rows"].clone() }));
        }
        (
            spec.get("title")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .or(command.title),
            spec.get("placeholder")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .or(command.placeholder),
            spec.get("width").and_then(|v| v.as_u64()).map(|v| v as u16),
            spec.get("actions")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default(),
        )
    } else {
        (command.title, command.placeholder, None, Vec::new())
    };

    let request = control_request(control::ControlCommand::Pick {
        title,
        placeholder,
        width,
        actions,
    });
    writeln!(stream, "{}", serde_json::to_string(&request).unwrap())?;

    let reader_stream = stream.try_clone()?;
    let mut reply = String::new();
    let mut reader = BufReader::new(reader_stream);
    reader.read_line(&mut reply)?;
    let value: serde_json::Value = serde_json::from_str(&reply).unwrap_or_default();
    if value.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
            eprintln!("{error}");
        }
        std::process::exit(1);
    }

    let json = command.json;
    let reader_thread = std::thread::spawn(move || {
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            match classify_pick_stream_event(&value) {
                PickStreamEvent::Action => {
                    if json {
                        println!("{line}");
                        let _ = std::io::stdout().flush();
                    }
                }
                PickStreamEvent::Selected(selected) => {
                    // Plain mode prints the id alone, so `rozi pick | xargs $EDITOR` needs no `jq`.
                    println!("{}", if json { &line } else { selected });
                    let _ = std::io::stdout().flush();
                    std::process::exit(0);
                }
                PickStreamEvent::Cancelled => {
                    if json {
                        println!("{line}");
                        let _ = std::io::stdout().flush();
                    }
                    std::process::exit(1);
                }
                PickStreamEvent::Ignore => {}
            }
        }
        std::process::exit(2);
    });

    if command.json {
        if let Some(rows) = opening_rows {
            let _ = writeln!(stream, "{rows}");
        }
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if writeln!(stream, "{line}").is_err() {
                break;
            }
        }
    } else {
        // Plain mode batches at EOF rather than streaming: it exists for `ls | rozi pick`, where
        // stdin closes immediately, and one send beats a redraw per line on a long pipeline. A
        // caller that wants to grow the list while the palette is open uses `--json` and controls
        // its own batching.
        let rows: Vec<serde_json::Value> = std::io::stdin()
            .lock()
            .lines()
            .map_while(std::result::Result::ok)
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::json!({ "id": line, "label": line }))
            .collect();
        let _ = writeln!(stream, "{}", serde_json::json!({ "rows": rows }));
    }

    let _ = reader_thread.join();
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickStreamEvent<'a> {
    Action,
    Selected(&'a str),
    Cancelled,
    Ignore,
}

fn classify_pick_stream_event(value: &serde_json::Value) -> PickStreamEvent<'_> {
    if value
        .get("action")
        .and_then(serde_json::Value::as_str)
        .is_some()
    {
        PickStreamEvent::Action
    } else if let Some(selected) = value.get("selected").and_then(serde_json::Value::as_str) {
        PickStreamEvent::Selected(selected)
    } else if value.get("cancelled").is_some() {
        PickStreamEvent::Cancelled
    } else {
        PickStreamEvent::Ignore
    }
}

pub(crate) fn run_control_cli(command: ControlCli) -> Result<()> {
    let path = match discover_socket(command.socket) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    };
    let mut stream = match IpcEndpoint::at_path(&path).connect() {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("could not connect to {}: {err}", path.display());
            std::process::exit(2);
        }
    };
    writeln!(
        stream,
        "{}",
        serde_json::to_string(&command.request).unwrap()
    )?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    if line.trim().is_empty() {
        eprintln!("empty response from rozi");
        std::process::exit(2);
    }
    println!("{}", line.trim_end());
    let value: serde_json::Value = match serde_json::from_str(&line) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("invalid JSON response: {err}");
            std::process::exit(2);
        }
    };
    if value.get("ok").and_then(|v| v.as_bool()) == Some(false) {
        if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
            eprintln!("{error}");
        }
        std::process::exit(1);
    }
    Ok(())
}

pub(crate) fn run_server_cli(name: &str, fresh: bool) -> Result<()> {
    session::server::run_named_session_mode(name, fresh)?;
    Ok(())
}

pub(crate) fn run_remote_serve_cli(name: &str) -> Result<()> {
    session::remote::run_remote_serve(name)?;
    Ok(())
}

pub(crate) fn recover_managed_installation() -> std::result::Result<(), String> {
    crate::platform::install::from_process()
        .recover_if_managed()
        .map(|_| ())
        .map_err(|error| format!("managed installation recovery failed: {error}"))
}

pub(crate) fn run_install_cli() -> std::result::Result<(), String> {
    let installation = crate::platform::install::from_process();
    let result = installation
        .install()
        .map_err(|error| format!("installation failed: {error}"))?;
    if result.changed {
        println!("Installed rozi v{}", result.version);
    } else {
        println!("rozi v{} is already installed and verified", result.version);
    }
    println!("Command  {}", installation.command_path().display());
    Ok(())
}

pub(crate) fn run_update_cli(command: UpdateCommand) -> std::result::Result<(), String> {
    let installation = crate::platform::install::from_process();
    match command {
        UpdateCommand::Check => {
            let result = installation
                .check_latest()
                .map_err(|error| format!("update check failed: {error}"))?;
            let running = semver::Version::parse(env!("CARGO_PKG_VERSION"))
                .map_err(|error| format!("invalid running version: {error}"))?;
            let current = result.current.as_ref().unwrap_or(&running);
            println!(
                "Current  v{}{}",
                current,
                if result.managed {
                    " (managed)"
                } else {
                    " (unmanaged)"
                }
            );
            println!("Latest   v{}", result.latest);
            let status = if result.latest > *current {
                "update available"
            } else if result.latest == *current {
                "up to date"
            } else {
                "running version is newer"
            };
            println!("Status   {status}");
        }
        UpdateCommand::Apply => {
            let result = installation
                .update()
                .map_err(|error| format!("update failed: {error}"))?;
            if result.changed {
                println!("Updated rozi to v{}", result.version);
            } else {
                println!("rozi v{} is up to date", result.version);
            }
        }
        UpdateCommand::Rollback => {
            let result = installation
                .rollback()
                .map_err(|error| format!("rollback failed: {error}"))?;
            println!("Rolled back rozi to v{}", result.version);
        }
    }
    Ok(())
}

const SKILL_HELP_SECTIONS: &[HelpSection] = &[
    HelpSection {
        heading: "USAGE",
        advanced_only: false,
        note: "",
        rows: &[row("rozi skill [COMMAND] [OPTIONS]", "")],
    },
    HelpSection {
        heading: "COMMANDS",
        advanced_only: false,
        note: "",
        rows: &[
            row("install [--global]", "Install the Rozi skill"),
            row("uninstall [--global]", "Remove the installed Rozi skill"),
            row("status [--global]", "Show skill installation status"),
            row("print", "Print the skill to stdout"),
        ],
    },
    HelpSection {
        heading: "OPTIONS",
        advanced_only: false,
        note: "",
        rows: &[
            row(
                "    --global",
                "Install, uninstall, or status for this user",
            ),
            row("-h, --help", "Print help"),
        ],
    },
];

fn parse_skill_args(args: &[String]) -> std::result::Result<ParsedCli, String> {
    let mut global = false;
    let mut command = None;
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => return Ok(ParsedCli::SkillHelp),
            "--global" => {
                if global {
                    return Err("--global specified more than once".to_string());
                }
                global = true;
            }
            "install" | "uninstall" | "status" | "print" if command.is_none() => {
                command = Some(arg.as_str());
            }
            other if command.is_none() => {
                return Err(format!("unknown skill command `{other}`"));
            }
            other => {
                return Err(format!("unexpected argument `{other}` after skill"));
            }
        }
    }
    match command {
        None => {
            if global {
                return Err("--global requires a skill command".to_string());
            }
            Ok(ParsedCli::SkillHelp)
        }
        Some("print") => {
            if global {
                return Err("skill print does not accept --global".to_string());
            }
            Ok(ParsedCli::Skill(SkillCommand::Print))
        }
        Some("install") => Ok(ParsedCli::Skill(SkillCommand::Install { global })),
        Some("uninstall") => Ok(ParsedCli::Skill(SkillCommand::Uninstall { global })),
        Some("status") => Ok(ParsedCli::Skill(SkillCommand::Status { global })),
        Some(other) => Err(format!("unknown skill command `{other}`")),
    }
}

pub(crate) fn print_skill() {
    skill::print_skill();
}

pub(crate) fn print_skill_help() {
    let styles = HelpStyles::detect();
    let mut out = format!(
        "{}rozi skill{} - install the built-in Rozi agent skill\n",
        styles.title, styles.reset
    );
    append_help_sections(&mut out, SKILL_HELP_SECTIONS, &styles, true);
    println!("{out}");
}

pub(crate) fn run_skill_cli(command: SkillCommand) -> Result<()> {
    match command {
        SkillCommand::Print => {
            print_skill();
            Ok(())
        }
        SkillCommand::Install { global } => {
            let paths = skill::default_paths(global).map_err(std::io::Error::other)?;
            let report = skill::install(&paths, crate::agent_detection::claude_cli_available())
                .map_err(std::io::Error::other)?;
            print!("{}", skill::format_install(&report, &paths));
            Ok(())
        }
        SkillCommand::Uninstall { global } => {
            let paths = skill::default_paths(global).map_err(std::io::Error::other)?;
            let report = skill::uninstall(&paths).map_err(std::io::Error::other)?;
            print!("{}", skill::format_uninstall(&report, &paths));
            Ok(())
        }
        SkillCommand::Status { global } => {
            let paths = skill::default_paths(global).map_err(std::io::Error::other)?;
            let report = skill::status(&paths, crate::agent_detection::claude_cli_available());
            print!("{}", skill::format_status(&report, &paths));
            Ok(())
        }
    }
}

pub(crate) fn run_list_sessions_cli(format: ListFormat, remote: Option<&str>) -> Result<()> {
    let rows = if let Some(remote) = remote {
        let target = session::remote::parse_remote_target(remote).map_err(std::io::Error::other)?;
        let config = crate::config::load_config().config.remote;
        session::discovery::discover_sessions_from(
            &session::discovery::SessionSource::Remote(target),
            &config,
        )?
    } else {
        session::discovery::discover_sessions_with_snapshots()?
    };
    match format {
        ListFormat::Json => {
            println!(
                "{}",
                session::discovery::sessions_to_json(&rows).map_err(std::io::Error::other)?
            );
        }
        ListFormat::Text => {
            for session in rows {
                let host = session
                    .host
                    .as_deref()
                    .map(|host| format!("\thost={host}"))
                    .unwrap_or_default();
                match session.status {
                    session::discovery::DiscoveredSessionStatus::Running {
                        panes,
                        clients,
                        has_layout,
                        ..
                    } => println!(
                        "{}\trunning\tpanes={}\tclients={}\tlayout={}{host}",
                        session.name,
                        panes,
                        clients,
                        if has_layout { "yes" } else { "no" }
                    ),
                    session::discovery::DiscoveredSessionStatus::Restorable => {
                        println!(
                            "{}\trestorable\tpanes=?\tclients=0\tlayout=?{host}",
                            session.name
                        )
                    }
                    session::discovery::DiscoveredSessionStatus::Busy => {
                        println!("{}\tbusy\tpanes=?\tclients=?\tlayout=?{host}", session.name)
                    }
                    session::discovery::DiscoveredSessionStatus::Unknown => {
                        println!(
                            "{}\tunknown\tpanes=?\tclients=?\tlayout=?{host}",
                            session.name
                        )
                    }
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn run_new_extension_cli(id: &str) -> Result<()> {
    let parent = std::env::current_dir()?;
    let destination =
        crate::config::create_extension_scaffold(id, &parent).map_err(std::io::Error::other)?;
    println!("Created {}", destination.display());
    println!("Validate: rozi check-extension {}", destination.display());
    println!("Invoke after installation: rozi run-action {id}.hello");
    Ok(())
}

pub(crate) fn run_list_extensions_cli(json: bool, verbose: bool) -> Result<()> {
    let scan = crate::config::scan_extensions_for_cli();
    for error in &scan.root_errors {
        eprintln!("rozi: {error}");
    }
    let entries = scan.entries();
    if json {
        let document = crate::config::ExtensionListDocument::new(entries);
        println!(
            "{}",
            serde_json::to_string_pretty(&document).map_err(std::io::Error::other)?
        );
        return Ok(());
    }
    println!("NAME\tTITLE\tVERSION\tCOMMANDS\tSERVICES\tSTATUS");
    for extension in &entries {
        let name = extension.display_name();
        let title = extension.title.as_deref().unwrap_or("-");
        let version = extension.version.as_deref().unwrap_or("-");
        println!(
            "{name}\t{title}\t{version}\t{}\t{}\t{}",
            extension.commands.len(),
            extension.services.len(),
            extension.status_detail()
        );
        if verbose {
            println!("  directory: {}", extension.path);
            println!("  manifest:  {}", extension.manifest_path);
            println!(
                "  id:        {}",
                extension.id.as_deref().unwrap_or("<unresolved>")
            );
            println!("  title:     {}", extension.title.as_deref().unwrap_or("-"));
            println!(
                "  api:       {}",
                extension
                    .api
                    .map(|api| api.to_string())
                    .unwrap_or_else(|| "-".to_string())
            );
            if !extension.commands.is_empty() {
                println!("  commands:");
                for id in &extension.commands {
                    if let Some(path) = extension.command_paths.get(id) {
                        println!("    {id}\t{path}");
                    } else {
                        println!("    {id}");
                    }
                }
            }
            if !extension.services.is_empty() {
                println!("  services:");
                for id in &extension.services {
                    if let Some(path) = extension.service_paths.get(id) {
                        println!("    {id}\t{path}");
                    } else {
                        println!("    {id}");
                    }
                }
            }
            for error in &extension.errors {
                println!("  error:     {error}");
            }
        }
    }
    Ok(())
}

pub(crate) fn run_check_extension_cli(path: &std::path::Path, json: bool) -> Result<bool> {
    let extension = crate::config::check_extension(path);
    let info = &extension.info;
    if json {
        let document = crate::config::ExtensionCheckDocument::new(info.clone());
        println!(
            "{}",
            serde_json::to_string_pretty(&document).map_err(std::io::Error::other)?
        );
        return Ok(info.status == crate::config::ExtensionStatus::Loaded);
    }
    println!("Extension: {}", info.display_name());
    println!("Version:   {}", info.version.as_deref().unwrap_or("-"));
    println!(
        "API:       {}",
        info.api
            .map(|api| api.to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    println!();
    if info.status == crate::config::ExtensionStatus::Loaded {
        println!("✓ manifest valid");
        println!("✓ extension id valid");
        println!("✓ {} commands", info.commands.len());
        println!("✓ {} services", info.services.len());
        println!("✓ executable paths resolved");
    } else {
        println!("status: {}", info.status.as_str());
        for error in &info.errors {
            eprintln!("rozi: {error}");
        }
    }
    if !info.command_details.is_empty() {
        println!("\nCommands:");
        for command in &info.command_details {
            println!("  {}", command.id);
            println!("    launch: {}", format_extension_launch(&command.launch));
            println!("    cwd:    {}", command.cwd);
            println!(
                "    env:    {}",
                format_extension_env(&command.injected_env)
            );
        }
    }
    if !info.service_details.is_empty() {
        println!("\nServices:");
        for service in &info.service_details {
            println!("  {}", service.id);
            println!("    launch: {}", format_extension_launch(&service.launch));
            println!("    cwd:    {}", service.cwd);
            println!("    restart: {}", service.restart);
            println!(
                "    env:    {}",
                format_extension_env(&service.injected_env)
            );
            if !service.configured_env_keys.is_empty() {
                println!(
                    "    manifest env: {} (values redacted)",
                    service.configured_env_keys.join(", ")
                );
            }
        }
    }
    Ok(info.status == crate::config::ExtensionStatus::Loaded)
}

fn format_extension_launch(launch: &crate::config::ExtensionLaunchDiagnostic) -> String {
    match launch {
        crate::config::ExtensionLaunchDiagnostic::Direct { argv } => {
            serde_json::to_string(argv).unwrap_or_else(|_| "[]".to_string())
        }
        crate::config::ExtensionLaunchDiagnostic::Shell { command } => {
            format!("shell {command:?}")
        }
        crate::config::ExtensionLaunchDiagnostic::Send { text } => format!("send {text:?}"),
    }
}

fn format_extension_env(env: &std::collections::BTreeMap<String, String>) -> String {
    env.iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn run_kill_session_cli(name: &str, remote: Option<&str>) -> Result<()> {
    if !session::discovery::valid_attach_target(name) {
        return Err(
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid session name").into(),
        );
    }
    if let Some(remote) = remote {
        return run_kill_session_remote(name, remote);
    }
    session::server::shutdown_named_session(name)
        .map_err(|err| std::io::Error::other(format!("could not kill session {name:?}: {err}")))?;
    Ok(())
}

fn run_kill_session_remote(name: &str, remote: &str) -> Result<()> {
    let target = session::remote::parse_remote_target(remote)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    let config = crate::config::load_config().config.remote;
    session::remote::kill_remote_session(&target, name, &config).map_err(std::io::Error::other)?;
    Ok(())
}

pub(crate) fn print_help(advanced: bool) {
    println!(
        "{}",
        help_text(&HelpStyles::detect(), &endpoint_help(), advanced)
    );
}

/// SGR sequences for the help screen, all empty when the stream cannot render them.
///
/// The palette stays on the terminal's own eight colours instead of picking exact RGB, so help
/// follows whatever theme the user already reads every other tool in.
#[derive(Clone, Copy)]
struct HelpStyles {
    title: &'static str,
    heading: &'static str,
    /// Text to type exactly as shown: command words and flags.
    literal: &'static str,
    /// Text to replace with a value: `<PANE_ID>`, `[COMMAND]`.
    placeholder: &'static str,
    reset: &'static str,
}

impl HelpStyles {
    const fn plain() -> Self {
        Self {
            title: "",
            heading: "",
            literal: "",
            placeholder: "",
            reset: "",
        }
    }

    const fn colored() -> Self {
        Self {
            title: "\x1b[1m",
            heading: "\x1b[1;32m",
            literal: "\x1b[1;36m",
            placeholder: "\x1b[35m",
            reset: "\x1b[0m",
        }
    }

    fn detect() -> Self {
        if crate::platform::ansi::stdout_supports_color() {
            Self::colored()
        } else {
            Self::plain()
        }
    }
}

/// One help row: the literal to type, and what it does.
///
/// An empty `name` continues the previous row's description on a new line. A `name` too wide for
/// the description column takes a line of its own, which is what keeps the long session and
/// capture signatures from pushing every description past 80 columns.
struct HelpRow {
    name: &'static str,
    description: &'static str,
    advanced_only: bool,
}

struct HelpSection {
    heading: &'static str,
    /// Prose shown under the heading, before the rows. Empty for most sections.
    note: &'static str,
    /// Shown only under `--help --advanced`, keeping plumbing out of the first help a new user
    /// reads without hiding it from someone who needs it.
    advanced_only: bool,
    rows: &'static [HelpRow],
}

/// Write a row's name, styling each token by what the reader has to do with it.
///
/// Three kinds: a *literal* is typed exactly as shown (`focus`, `--remote`, `json`), a
/// *placeholder* stands for a value the reader supplies (`<PANE_ID>`, `[COMMAND]`), and the
/// brackets, pipes and commas that hold a signature together are left unstyled so the structure
/// recedes behind both. Under `HelpStyles::plain` every sequence is empty, so this appends the name
/// unchanged.
fn push_styled_name(out: &mut String, name: &str, styles: &HelpStyles) {
    let bytes = name.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'<' {
            // `<KEY|TEXT>` is one placeholder, alternatives and all; an unclosed `<` runs to the end.
            let end = name[index..]
                .find('>')
                .map_or(name.len(), |offset| index + offset + 1);
            out.push_str(styles.placeholder);
            out.push_str(&name[index..end]);
            out.push_str(styles.reset);
            index = end;
        } else if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' {
            let mut end = index;
            while end < bytes.len()
                && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'-' || bytes[end] == b'_')
            {
                end += 1;
            }
            let word = &name[index..end];
            let style = if is_placeholder_word(word) {
                styles.placeholder
            } else {
                styles.literal
            };
            out.push_str(style);
            out.push_str(word);
            out.push_str(styles.reset);
            index = end;
        } else {
            let character = name[index..]
                .chars()
                .next()
                .expect("index is a char boundary");
            out.push(character);
            index += character.len_utf8();
        }
    }
}

/// Whether a bare word stands for a value rather than something to type.
///
/// Bracketed placeholders are written in caps (`[COMMAND]`, `[ARGS]`), which separates them from
/// the literal values that appear in the same position (`[--format text|json]`). A flag is never a
/// placeholder however it is cased.
fn is_placeholder_word(word: &str) -> bool {
    !word.starts_with('-')
        && word.bytes().any(|byte| byte.is_ascii_uppercase())
        && !word.bytes().any(|byte| byte.is_ascii_lowercase())
}

/// Width of the name column. Descriptions start at `HELP_INDENT + HELP_NAME_WIDTH`.
const HELP_NAME_WIDTH: usize = 27;
const HELP_INDENT: &str = "    ";

const fn row(name: &'static str, description: &'static str) -> HelpRow {
    HelpRow {
        name,
        description,
        advanced_only: false,
    }
}

const fn advanced_row(name: &'static str, description: &'static str) -> HelpRow {
    HelpRow {
        name,
        description,
        advanced_only: true,
    }
}

const HELP_SECTIONS: &[HelpSection] = &[
    HelpSection {
        heading: "USAGE",
        advanced_only: false,
        note: "",
        rows: &[
            row(
                "rozi [TARGET] [OPTIONS]",
                "Attach to TARGET, or launch its profile",
            ),
            row("rozi <COMMAND> [ARGS]", ""),
        ],
    },
    HelpSection {
        heading: "SESSIONS",
        advanced_only: false,
        note: "",
        rows: &[
            row("attach <NAME>", "Attach to a running session, never create"),
            row(
                "new <NAME> [--profile <PROFILE>]",
                "Create a session, optionally from a profile",
            ),
            row(
                "list-sessions [--format text|json] [--remote <HOST>]",
                "List connectable sessions",
            ),
            row(
                "kill-session <NAME> [--remote <HOST>]",
                "Stop a session and all of its panes",
            ),
        ],
    },
    HelpSection {
        heading: "PANES",
        advanced_only: false,
        note: "",
        rows: &[
            row("list-panes", "Print live panes as JSON"),
            row("focus <PANE_ID>", "Focus a pane"),
            row("send-text <TEXT>", "Send literal text to this pane"),
            row(
                "send-keys [-l|--literal] [--] <KEY|TEXT>...",
                "Send tmux-style key names, text, or both",
            ),
            row(
                "split [OPTIONS] [COMMAND | --argv PROGRAM [ARG...]]",
                "Spawn a pane; new-pane alias",
            ),
            row(
                "capture-pane [--target <PANE_ID>] [--scrollback <N|full>] [--last-output]",
                "Print a pane's contents",
            ),
            row("switch-workspace <1-9>", "Switch the active workspace"),
            row(
                "move-to-workspace <1-9>",
                "Move the focused pane to a workspace",
            ),
        ],
    },
    HelpSection {
        heading: "SCRIPTING",
        advanced_only: false,
        note: "",
        rows: &[
            row("status <VALUE> [--reason <TEXT>]", ""),
            row("status --clear", "Set or clear this pane's reported status"),
            row(
                "run-action <ACTION_ID>",
                "Run a bindable action by its command id",
            ),
            row(
                "notify <MESSAGE> [--title T] [--level info|error]",
                "Raise a toast from a script",
            ),
            row("publish", "Publish activity rows over stdio"),
            row("subscribe [EVENT...]", "Stream application events as JSON"),
            row(
                "pick [--title T] [--placeholder P] [--json]",
                "Choose a line of stdin in a modal picker",
            ),
            row("metrics", "Print runtime metrics as JSON"),
        ],
    },
    HelpSection {
        heading: "EXTENSIONS",
        advanced_only: false,
        note: "",
        rows: &[
            row(
                "list-extensions [OPTIONS]",
                "Show discovery status (--verbose, --json)",
            ),
            row("new-extension <ID>", "Create a valid extension scaffold"),
            row(
                "check-extension PATH [OPTIONS]",
                "Validate an unpacked extension (--json)",
            ),
        ],
    },
    HelpSection {
        heading: "AGENTS",
        advanced_only: false,
        note: "",
        rows: &[row("skill [COMMAND]", "Manage the Rozi agent skill")],
    },
    HelpSection {
        heading: "INSTALLATION",
        advanced_only: false,
        note: "",
        rows: &[
            row("install", "Install this binary as a managed `rozi`"),
            row(
                "update [--check|--rollback]",
                "Update in place, check, or roll back",
            ),
        ],
    },
    HelpSection {
        heading: "OPTIONS",
        advanced_only: false,
        note: "",
        rows: &[
            row(
                "-h, --help [--advanced]",
                "Print help; --advanced adds internals",
            ),
            row("-V, --version", "Print version and protocol range"),
            row(
                "    --session <NAME>",
                "Session target, same as a positional TARGET",
            ),
            row(
                "    --profile <NAME>",
                "Seed a `new` session from this profile",
            ),
            row("    --read-only", "Attach as a viewer; cannot type or tile"),
            row("    --pick", "Force the startup session picker, whatever"),
            row("", "`[session] startup` selects"),
            row(
                "    --remote [HOST]",
                "Attach over SSH to a host alias or ssh://",
            ),
            row("", "URL; omit HOST for `[remote] default_host`"),
            row("    --config <PATH>", "Load an alternate config.toml"),
            advanced_row(
                "    --socket <PATH>",
                "Send the control command to this endpoint",
            ),
            advanced_row("    --skill", "Print agent control instructions"),
        ],
    },
    HelpSection {
        heading: "ADVANCED",
        advanced_only: true,
        note: "Server plumbing; a normal launch needs none of it.",
        rows: &[
            row("    --server", "Run --session <NAME>'s server in this"),
            row("", "process instead of attaching a UI"),
        ],
    },
];

/// The help body, with the platform-specific endpoint paragraph passed in so a test can measure
/// the template's own width without depending on how long this machine's runtime directory is.
fn help_text(styles: &HelpStyles, endpoint_help: &str, advanced: bool) -> String {
    let HelpStyles {
        title,
        heading,
        reset,
        ..
    } = *styles;
    let version = env!("CARGO_PKG_VERSION");
    let mut out = format!("{title}rozi {version}{reset} - dynamic tiling terminal multiplexer\n");
    append_help_sections(&mut out, HELP_SECTIONS, styles, advanced);

    if advanced {
        out.push_str(&format!(
            "\n{heading}ENDPOINTS{reset}\n{HELP_INDENT}{endpoint_help}\n"
        ));
    }
    out.push_str("\nDetach with prefix d, or use a configured quit binding.");
    out
}

fn append_help_sections(
    out: &mut String,
    sections: &[HelpSection],
    styles: &HelpStyles,
    advanced: bool,
) {
    let HelpStyles { heading, reset, .. } = *styles;
    for section in sections {
        if section.advanced_only && !advanced {
            continue;
        }
        out.push_str(&format!("\n{heading}{}{reset}\n", section.heading));
        if !section.note.is_empty() {
            out.push_str(&format!("{HELP_INDENT}{}\n\n", section.note));
        }
        for HelpRow {
            name,
            description,
            advanced_only,
        } in section.rows
        {
            if *advanced_only && !advanced {
                continue;
            }
            if name.is_empty() {
                out.push_str(&format!(
                    "{HELP_INDENT}{:width$}{description}\n",
                    "",
                    width = HELP_NAME_WIDTH
                ));
                continue;
            }
            out.push_str(HELP_INDENT);
            push_styled_name(out, name, styles);
            if description.is_empty() {
                out.push('\n');
                continue;
            }
            if name.chars().count() < HELP_NAME_WIDTH {
                out.push_str(&format!(
                    "{:width$}{description}\n",
                    "",
                    width = HELP_NAME_WIDTH - name.chars().count()
                ));
            } else {
                out.push_str(&format!(
                    "\n{HELP_INDENT}{:width$}{description}\n",
                    "",
                    width = HELP_NAME_WIDTH
                ));
            }
        }
    }
}

/// The `--socket`/`ROZI_SOCKET` explanation, which differs by platform: a Unix-domain socket
/// path on Linux/macOS, a named-pipe registry entry on Windows (see `platform::ipc::windows` for
/// why the *entry*, not the pipe name, is what a user points at).
fn endpoint_help() -> String {
    let runtime_dir = crate::control::runtime_dir()
        .map(|dir| dir.display().to_string())
        .unwrap_or_else(|_| "the rozi runtime directory".to_string());
    if cfg!(windows) {
        format!(
            "Control endpoints live one per running rozi, named by pid, in\n        \
             {runtime_dir}\n    \
             Each entry stands for a named pipe (\\\\.\\pipe\\rozi.<sid>.control-<pid>);\n    \
             pass the entry, not the pipe name. Unless --socket is given, rozi uses\n    \
             ROZI_SOCKET; failing that, the only live endpoint there."
        )
    } else {
        format!(
            "Control sockets live one per running rozi, named by pid, in\n        \
             {runtime_dir}\n    \
             Unless --socket is given, rozi uses ROZI_SOCKET; failing that, the\n    \
             only live socket there."
        )
    }
}

pub(crate) fn print_version() {
    use crate::session::protocol::{MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION};
    println!("rozi {}", env!("CARGO_PKG_VERSION"));
    println!("protocol_min={MIN_SUPPORTED_PROTOCOL}");
    println!("protocol_max={PROTOCOL_VERSION}");
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
    fn cli_skill_is_a_strict_early_variant_backed_by_the_contract_document() {
        assert!(matches!(
            parse_cli_args(vec!["--skill".into()]).expect("parses"),
            ParsedCli::Skill(SkillCommand::Print)
        ));
        assert!(parse_cli_args(vec!["--skill".into(), "extra".into()]).is_err());
        assert!(parse_cli_args(vec!["target".into(), "--skill".into()]).is_err());
        assert!(matches!(
            parse_cli_args(vec!["skill".into(), "print".into()]).expect("parses"),
            ParsedCli::Skill(SkillCommand::Print)
        ));
        assert!(matches!(
            parse_cli_args(vec!["skill".into()]).expect("parses"),
            ParsedCli::SkillHelp
        ));
        assert!(matches!(
            parse_cli_args(vec!["skill".into(), "-h".into()]).expect("parses"),
            ParsedCli::SkillHelp
        ));
        assert!(matches!(
            parse_cli_args(vec!["skill".into(), "install".into(), "--global".into()])
                .expect("parses"),
            ParsedCli::Skill(SkillCommand::Install { global: true })
        ));

        for section in [
            "---\nname: rozi",
            "ROZI=1",
            "ROZI_SOCKET",
            "ROZI_PANE",
            "--socket PATH",
            "rozi --help",
            "rozi list-panes",
            "rozi split [COMMAND]",
            "send-text",
            "send-keys",
            "capture-pane",
            "status --clear",
            "pty_ready:true",
            "does **not** move focus",
            "queued as type-ahead",
            "rozi list-sessions",
            "rozi kill-session <NAME>",
            "read-only",
            "Input lock",
        ] {
            assert!(
                crate::skill::SKILL_MD.contains(section),
                "missing skill section: {section}"
            );
        }
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
        let reserved =
            expect_run(parse_cli_args(vec!["--session".into(), "attach".into()]).expect("parses"));
        assert_eq!(reserved.attach_session.as_deref(), Some("attach"));
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
            split(vec![
                "new-pane".into(),
                "--focus".into(),
                "cargo test".into()
            ]),
            expected(Some("cargo test"), true)
        );
        assert_eq!(
            split(vec![
                "new-pane".into(),
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
    fn cli_new_pane_preserves_structured_argv_without_parsing_child_flags() {
        let ParsedCli::Control(control) = parse_cli_args(vec![
            "new-pane".into(),
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
            }
        );
        assert!(parse_cli_args(vec!["new-pane".into(), "--argv".into()]).is_err());
        assert!(
            parse_cli_args(vec![
                "new-pane".into(),
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
    fn cli_parses_session_verbs_and_attach() {
        assert!(matches!(
            parse_cli_args(vec!["list-sessions".into()]).expect("parses"),
            ParsedCli::ListSessions {
                format: ListFormat::Text,
                remote: None,
                ..
            }
        ));
        assert!(matches!(
            parse_cli_args(vec!["kill-session".into(), "dev".into()]).expect("parses"),
            ParsedCli::KillSession {
                name,
                remote: None,
                ..
            } if name == "dev"
        ));
        assert!(matches!(
            parse_cli_args(vec![
                "list-sessions".into(),
                "--format".into(),
                "json".into()
            ])
            .expect("parses"),
            ParsedCli::ListSessions {
                format: ListFormat::Json,
                remote: None,
                ..
            }
        ));
        assert!(matches!(
            parse_cli_args(vec!["list-extensions".into()]).expect("parses"),
            ParsedCli::ListExtensions {
                json: false,
                verbose: false,
                ..
            }
        ));
        assert!(matches!(
            parse_cli_args(vec![
                "list-extensions".into(),
                "--verbose".into(),
                "--json".into()
            ])
            .expect("parses"),
            ParsedCli::ListExtensions {
                json: true,
                verbose: true,
                ..
            }
        ));
        assert!(matches!(
            parse_cli_args(vec![
                "check-extension".into(),
                "./git-tools".into(),
                "--json".into()
            ])
            .expect("parses"),
            ParsedCli::CheckExtension { path, json: true }
                if path == std::path::Path::new("./git-tools")
        ));
        assert!(matches!(
            parse_cli_args(vec!["new-extension".into(), "git-tools".into()]).expect("parses"),
            ParsedCli::NewExtension { id } if id == "git-tools"
        ));
        assert!(parse_cli_args(vec!["new-extension".into()]).is_err());
        assert!(parse_cli_args(vec!["new-extension".into(), "one".into(), "two".into()]).is_err());
        let attached = expect_run(parse_cli_args(vec!["dev".into()]).expect("parses"));
        assert_eq!(attached.attach_session.as_deref(), Some("dev"));
        assert_eq!(attached.session_command, SessionCommand::Dwim);
        let attached = expect_run(
            parse_cli_args(vec!["attach".into(), "dev".into(), "--read-only".into()])
                .expect("parses"),
        );
        assert_eq!(attached.attach_session.as_deref(), Some("dev"));
        assert_eq!(attached.session_command, SessionCommand::Attach);
        assert!(attached.read_only);
        let created = expect_run(
            parse_cli_args(vec![
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
        assert!(parse_cli_args(vec!["kill-session".into()]).is_err());
        assert!(parse_cli_args(vec!["kill-session".into(), "dev/../other".into()]).is_err());
        assert!(parse_cli_args(vec!["kill-session".into(), "dev\nnext".into()]).is_err());
        assert!(parse_cli_args(vec!["attach".into()]).is_err());
        assert!(parse_cli_args(vec!["new".into()]).is_err());
        assert!(parse_cli_args(vec!["new".into(), "dev".into(), "--read-only".into()]).is_err());
        assert!(parse_cli_args(vec!["attach".into(), "dev".into(), "--server".into()]).is_err());
        assert!(parse_cli_args(vec!["new".into(), "dev".into(), "--server".into()]).is_err());
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
            vec!["attach", "--read-only"],
            vec!["new", "--profile"],
            vec!["--session", "--pick"],
            vec!["--server", "--pick"],
            vec!["--session", "dev", "--fresh-server", "--pick"],
            vec!["--remote-serve", "--pick"],
            vec!["kill-session", "--remote"],
            vec!["--profile", "--read-only"],
            vec!["--config", "--read-only"],
            vec!["--socket", "--read-only", "list-panes"],
            vec!["run-action", "--focus"],
            vec!["status", "blocked", "--reason", "--clear"],
            vec!["list-sessions", "--format", "--remote"],
        ] {
            let parsed = parse_cli_args(args.iter().map(|arg| (*arg).to_string()).collect());
            assert!(parsed.is_err(), "accepted a flag as a value: {args:?}");
        }
        // A lone `-` is still an ordinary value.
        let dash = expect_run(
            parse_cli_args(vec![
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
    fn picker_actions_are_non_terminal_even_when_they_carry_a_selection() {
        assert_eq!(
            classify_pick_stream_event(&serde_json::json!({
                "action": "delete",
                "selected": "feature"
            })),
            PickStreamEvent::Action
        );
        assert_eq!(
            classify_pick_stream_event(&serde_json::json!({
                "action": "refresh",
                "selected": null
            })),
            PickStreamEvent::Action
        );
        assert_eq!(
            classify_pick_stream_event(&serde_json::json!({ "selected": "feature" })),
            PickStreamEvent::Selected("feature")
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
            .contains("list-sessions --remote"),
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

        let ParsedCli::ListSessions { config_path, .. } = parse_cli_args(vec![
            "--config".into(),
            "/tmp/alt.toml".into(),
            "list-sessions".into(),
        ])
        .expect("parses") else {
            panic!("expected list-sessions");
        };
        assert_eq!(config_path.as_deref(), Some("/tmp/alt.toml"));

        let ParsedCli::KillSession { config_path, .. } = parse_cli_args(vec![
            "--config".into(),
            "/tmp/alt.toml".into(),
            "kill-session".into(),
            "dev".into(),
        ])
        .expect("parses") else {
            panic!("expected kill-session");
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
    fn cli_help_fits_an_eighty_column_terminal() {
        // The help is the only UI a first run gets; wrapping it destroys the aligned columns.
        let endpoint = "Control sockets live one per running rozi, named by pid, in\n    \
                        <dir>\n    \
                        With no --socket, ROZI_SOCKET is used, then the only live socket there.";
        for advanced in [false, true] {
            let mut widest = 0;
            for line in help_text(&HelpStyles::plain(), endpoint, advanced).lines() {
                assert!(
                    !line.ends_with(' '),
                    "trailing whitespace in help line: {line:?}"
                );
                widest = widest.max(line.chars().count());
            }
            assert!(
                widest <= 80,
                "help reaches {widest} columns (advanced: {advanced})"
            );
        }
        let mut skill_help = String::from("rozi skill - install the built-in Rozi agent skill\n");
        append_help_sections(
            &mut skill_help,
            SKILL_HELP_SECTIONS,
            &HelpStyles::plain(),
            true,
        );
        for line in skill_help.lines() {
            assert!(
                !line.ends_with(' '),
                "trailing whitespace in skill help line: {line:?}"
            );
            assert!(
                line.chars().count() <= 80,
                "skill help reaches {} columns: {line}",
                line.chars().count()
            );
        }
    }

    fn heading_order(text: &str, headings: &[&str]) {
        let mut pos = 0;
        for heading in headings {
            let Some(found) = text[pos..].find(heading) else {
                panic!("{heading} missing or out of order in help");
            };
            pos += found + heading.len();
        }
    }

    #[test]
    fn cli_advanced_help_gates_server_plumbing_without_hiding_it() {
        let render = |advanced| help_text(&HelpStyles::plain(), "<endpoints>", advanced);
        let normal = render(false);
        let advanced = render(true);
        heading_order(
            &normal,
            &[
                "USAGE",
                "SESSIONS",
                "PANES",
                "SCRIPTING",
                "EXTENSIONS",
                "AGENTS",
                "INSTALLATION",
                "OPTIONS",
            ],
        );
        heading_order(
            &advanced,
            &[
                "USAGE",
                "SESSIONS",
                "PANES",
                "SCRIPTING",
                "EXTENSIONS",
                "AGENTS",
                "INSTALLATION",
                "OPTIONS",
                "ADVANCED",
                "ENDPOINTS",
            ],
        );

        assert!(
            !normal.contains("--server"),
            "plumbing should stay out of the first help a new user reads"
        );
        assert!(!normal.contains("ADVANCED"));
        assert!(!normal.contains("ENDPOINTS"));
        assert!(!normal.contains("--socket"));
        assert!(!normal.contains("--skill"));
        assert!(!normal.contains("CONTROL"));
        assert!(advanced.contains("--server"));
        assert!(advanced.contains("--socket"));
        assert!(advanced.contains("--skill"));
        // Normal help still has to say where the rest went.
        assert!(normal.contains("--advanced"));

        // `--advanced` reads on either side of the help flag, and means nothing without it.
        for args in [
            vec!["--help", "--advanced"],
            vec!["--advanced", "--help"],
            vec!["-h", "--advanced"],
        ] {
            assert!(matches!(
                parse_cli_args(args.iter().map(|arg| (*arg).to_string()).collect())
                    .expect("parses"),
                ParsedCli::Help { advanced: true }
            ));
        }
        assert!(matches!(
            parse_cli_args(vec!["--help".into()]).expect("parses"),
            ParsedCli::Help { advanced: false }
        ));
        assert!(parse_cli_args(vec!["--advanced".into()]).is_err());
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

    #[test]
    fn cli_help_names_split_into_literals_and_placeholders() {
        // Marker styles rather than real SGR, so a failure reads as the classification it is.
        let styles = HelpStyles {
            title: "",
            heading: "",
            literal: "L<",
            placeholder: "P<",
            reset: ">",
        };
        let styled = |name: &str| {
            let mut out = String::new();
            push_styled_name(&mut out, name, &styles);
            out
        };

        assert_eq!(styled("focus <PANE_ID>"), "L<focus> P<<PANE_ID>>");
        // Brackets and pipes stay unstyled; what is inside them is classified on its own.
        assert_eq!(
            styled("new <NAME> [--profile <PROFILE>]"),
            "L<new> P<<NAME>> [L<--profile> P<<PROFILE>>]"
        );
        // An all-caps bracketed word is a value to supply; a lowercase one is typed verbatim.
        assert_eq!(styled("split [COMMAND]"), "L<split> [P<COMMAND>]");
        assert_eq!(
            styled("list-sessions [--format text|json]"),
            "L<list-sessions> [L<--format> L<text>|L<json>]"
        );
        // `-l` is a flag, not a placeholder, and `<KEY|TEXT>` is one placeholder including its
        // alternatives.
        assert_eq!(
            styled("send-keys [-l|--literal] [--] <KEY|TEXT>..."),
            "L<send-keys> [L<-l>|L<--literal>] [L<-->] P<<KEY|TEXT>>..."
        );
        assert_eq!(styled("-h, --help"), "L<-h>, L<--help>");

        // Plain styling has to leave every name exactly as written.
        for section in HELP_SECTIONS {
            for row in section.rows {
                let mut plain = String::new();
                push_styled_name(&mut plain, row.name, &HelpStyles::plain());
                assert_eq!(plain, row.name);
            }
        }
    }

    #[test]
    fn cli_help_styling_only_adds_escapes() {
        // Colour must not move a single glyph: the styled help has to be the plain help with SGR
        // sequences woven in, or the aligned columns drift the moment a terminal supports colour.
        let endpoint = "Control sockets live in <dir>.";
        let plain = help_text(&HelpStyles::plain(), endpoint, true);
        let colored = help_text(&HelpStyles::colored(), endpoint, true);
        assert_ne!(plain, colored, "colored help should carry escapes");

        let mut stripped = String::with_capacity(colored.len());
        let mut rest = colored.as_str();
        while let Some(start) = rest.find('\x1b') {
            stripped.push_str(&rest[..start]);
            let end = rest[start..]
                .find('m')
                .expect("every SGR sequence this help emits ends in `m`");
            rest = &rest[start + end + 1..];
        }
        stripped.push_str(rest);
        assert_eq!(stripped, plain);
    }

    fn expect_run(parsed: ParsedCli) -> CliArgs {
        match parsed {
            ParsedCli::Run(args) => args,
            other => panic!("expected run args, got {other:?}"),
        }
    }
}
