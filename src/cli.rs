use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use tui_lipan::Result;

use crate::platform::ipc::{EndpointRegistry, IpcEndpoint};
use crate::{control, session};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CliArgs {
    pub(crate) session_command: SessionCommand,
    pub(crate) profile: Option<String>,
    pub(crate) config_path: Option<String>,
    pub(crate) attach_session: Option<String>,
    /// Open the session picker at startup instead of silently attaching to an ephemeral session
    /// (also enabled by `[session] startup = "picker"`). Ignored when a target is
    /// given or no named session exists.
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

#[derive(Debug)]
pub(crate) enum ParsedCli {
    Help,
    Version,
    Run(CliArgs),
    Control(ControlCli),
    Server {
        name: String,
        fresh: bool,
    },
    /// Hidden remote-side stdio proxy (`--remote-serve <NAME>`).
    RemoteServe {
        name: String,
    },
    ListSessions {
        format: ListFormat,
        remote: Option<String>,
    },
    KillSession {
        name: String,
        remote: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ListFormat {
    #[default]
    Text,
    Json,
}

pub(crate) fn parse_cli_args(args: Vec<String>) -> std::result::Result<ParsedCli, String> {
    let mut cli = CliArgs::default();
    let mut socket: Option<PathBuf> = None;
    let mut socket_flag_seen = false;
    let mut session_flag_target = false;
    let mut iter = args.into_iter().peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(ParsedCli::Help),
            "--version" | "-V" => return Ok(ParsedCli::Version),
            "list-sessions" => {
                let mut format = ListFormat::Text;
                let mut remote = None;
                while let Some(flag) = iter.next() {
                    match flag.as_str() {
                        "--format" => {
                            let value = iter
                                .next()
                                .ok_or_else(|| "--format requires text or json".to_string())?;
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
                            let target = iter.next().ok_or_else(|| {
                                "list-sessions --remote requires a host alias or ssh:// URL"
                                    .to_string()
                            })?;
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
                return Ok(ParsedCli::ListSessions { format, remote });
            }
            "kill-session" => {
                let name = iter
                    .next()
                    .ok_or_else(|| "kill-session requires a session name".to_string())?;
                let mut remote = None;
                while let Some(flag) = iter.next() {
                    match flag.as_str() {
                        "--remote" => {
                            let target = iter.next().ok_or_else(|| {
                                "kill-session --remote requires a host alias or ssh:// URL"
                                    .to_string()
                            })?;
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
                return Ok(ParsedCli::KillSession { name, remote });
            }
            "attach" => {
                if cli.attach_session.is_some() {
                    return Err("session target specified more than once".to_string());
                }
                cli.attach_session = Some(
                    iter.next()
                        .ok_or_else(|| "attach requires a session name".to_string())?,
                );
                cli.session_command = SessionCommand::Attach;
            }
            "new" => {
                if cli.attach_session.is_some() {
                    return Err("session target specified more than once".to_string());
                }
                cli.attach_session = Some(
                    iter.next()
                        .ok_or_else(|| "new requires a session name".to_string())?,
                );
                cli.session_command = SessionCommand::New;
            }
            "--server" => {
                if cli.remote.is_some() {
                    return Err("--remote cannot be combined with --server".to_string());
                }
                if let Some(name) = cli.attach_session.take() {
                    if !session_flag_target || cli.session_command != SessionCommand::Dwim {
                        return Err("--server must follow --session <NAME>".to_string());
                    }
                    reject_trailing_control_args(&mut iter, "--server")?;
                    return Ok(ParsedCli::Server { name, fresh: false });
                }
                let name = iter
                    .next()
                    .ok_or_else(|| "--server requires a session name".to_string())?;
                reject_trailing_control_args(&mut iter, "--server")?;
                return Ok(ParsedCli::Server { name, fresh: false });
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
                reject_trailing_control_args(&mut iter, "--fresh-server")?;
                return Ok(ParsedCli::Server { name, fresh: true });
            }
            "--remote-serve" => {
                let name = iter
                    .next()
                    .ok_or_else(|| "--remote-serve requires a session name".to_string())?;
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
                let name = iter
                    .next()
                    .ok_or_else(|| "--session requires a session name".to_string())?;
                if cli.attach_session.is_some() {
                    return Err("session target specified more than once".to_string());
                }
                cli.attach_session = Some(name);
                cli.session_command = SessionCommand::Dwim;
                session_flag_target = true;
            }
            "--profile" => {
                let profile = iter
                    .next()
                    .ok_or_else(|| "--profile requires a profile name".to_string())?;
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
                let path = iter
                    .next()
                    .ok_or_else(|| "--config requires a path".to_string())?;
                if cli.config_path.is_some() {
                    return Err("--config specified more than once".to_string());
                }
                cli.config_path = Some(path);
            }
            "--socket" => {
                socket_flag_seen = true;
                socket = Some(PathBuf::from(
                    iter.next()
                        .ok_or_else(|| "--socket requires a path".to_string())?,
                ));
            }
            "list" | "list-panes" => {
                reject_trailing_control_args(&mut iter, "list-panes")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket,
                    request: control_request(control::ControlCommand::ListPanes),
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
                    socket,
                    request: control_request(control::ControlCommand::Focus { target }),
                }));
            }
            "send-text" => {
                let text = iter
                    .next()
                    .ok_or_else(|| "send-text requires literal text".to_string())?;
                reject_trailing_control_args(&mut iter, "send-text")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket,
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
                    socket,
                    request: control_request(control::ControlCommand::SendKeys {
                        target: None,
                        keys,
                        literal,
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
                        Some(flag) if flag == "--reason" => Some(
                            iter.next()
                                .ok_or_else(|| "--reason requires text".to_string())?,
                        ),
                        Some(extra) => {
                            return Err(format!("unexpected argument `{extra}` after status"));
                        }
                    };
                    reject_trailing_control_args(&mut iter, "status")?;
                    (Some(first), reason)
                };
                return Ok(ParsedCli::Control(ControlCli {
                    socket,
                    request: control_request(control::ControlCommand::SetStatus {
                        target: None,
                        status,
                        reason,
                    }),
                }));
            }
            "split" | "new-pane" => {
                let command = iter.next();
                reject_trailing_control_args(&mut iter, "split")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket,
                    request: control_request(control::ControlCommand::NewPane {
                        command,
                        cwd: None,
                        title: None,
                        keep_open: false,
                    }),
                }));
            }
            "run-action" => {
                let action = iter
                    .next()
                    .ok_or_else(|| "run-action requires an action id".to_string())?;
                reject_trailing_control_args(&mut iter, "run-action")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket,
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
                    socket,
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
                    socket,
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
                    socket,
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
        source_pane: std::env::var("HYPRMUX_PANE")
            .ok()
            .and_then(|v| v.parse().ok()),
    }
}

fn discover_socket(explicit: Option<PathBuf>) -> std::result::Result<PathBuf, String> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    if let Some(path) = std::env::var_os("HYPRMUX_SOCKET").map(PathBuf::from) {
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
        [] => Err(
            "no live hyprmux control socket found (set HYPRMUX_SOCKET or pass --socket)"
                .to_string(),
        ),
        _ => Err("multiple live hyprmux sockets found; pass --socket PATH".to_string()),
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
        eprintln!("empty response from hyprmux");
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

pub(crate) fn run_list_sessions_cli(format: ListFormat, remote: Option<&str>) -> Result<()> {
    let rows = if let Some(remote) = remote {
        let target = session::remote::parse_remote_target(remote).map_err(std::io::Error::other)?;
        let config = crate::config::load_config().config.remote;
        session::discovery::discover_sessions_from(
            &session::discovery::SessionSource::Remote(target),
            &config,
        )?
    } else {
        session::discovery::discover_sessions()?
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

pub(crate) fn run_kill_session_cli(name: &str, remote: Option<&str>) -> Result<()> {
    if let Some(remote) = remote {
        return run_kill_session_remote(name, remote);
    }
    use crate::session::protocol::{
        ClientMessage, MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION, ServerMessage,
    };

    let path = session::server::session_socket_path(name)?;
    if !path.exists() {
        session::server::delete_snapshot(name)?;
        return Ok(());
    }
    match IpcEndpoint::at_path(&path).connect() {
        Ok(mut stream) => {
            stream.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
            session::protocol::write_frame(
                &mut stream,
                &ClientMessage::Attach {
                    session: name.to_string(),
                    protocol_version: PROTOCOL_VERSION,
                    min_protocol_version: MIN_SUPPORTED_PROTOCOL,
                    label: crate::platform::user::current_user_label(),
                    read_only: false,
                },
            )?;
            match session::protocol::read_frame::<_, ServerMessage>(&mut stream)? {
                ServerMessage::Attached { .. } => {
                    session::protocol::write_frame(&mut stream, &ClientMessage::Shutdown)?;
                    use std::io::Write;
                    stream.flush()?;
                    session::server::delete_snapshot(name)?;
                }
                other => {
                    eprintln!("could not attach to session {name:?}: {other:?}");
                    std::process::exit(1);
                }
            }
            Ok(())
        }
        Err(err) => {
            eprintln!("could not attach to session {name:?}: {err}");
            std::process::exit(1);
        }
    }
}

fn run_kill_session_remote(name: &str, remote: &str) -> Result<()> {
    use std::process::Stdio;

    let target = session::remote::parse_remote_target(remote)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    let config = crate::config::load_config().config.remote;
    let resolved = session::remote::ResolvedRemote::resolve(&target, &config);
    let remote_bin = resolved
        .binary_path
        .clone()
        .unwrap_or_else(|| "hyprmux".to_string());
    let mut command = session::remote::ssh_base_command(&resolved, &config, true);
    command.arg(resolved.ssh_destination());
    command.arg("--");
    command.arg(&remote_bin);
    command.arg("kill-session");
    command.arg(name);
    command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    let status = command.status()?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

pub(crate) fn print_help() {
    let endpoint_help = endpoint_help();
    println!(
        "\
hyprmux - Hyprland-style tiling terminal multiplexer

USAGE:
    hyprmux [TARGET] [--read-only]
    hyprmux attach <NAME> [--read-only]
    hyprmux new <NAME> [--profile <RECIPE>]
    hyprmux [--socket PATH] list|list-panes
    hyprmux [--socket PATH] focus <PANE_ID>
    hyprmux [--socket PATH] send-text <TEXT>
    hyprmux [--socket PATH] status <VALUE> [--reason <TEXT>]
    hyprmux [--socket PATH] status --clear
    hyprmux [--socket PATH] split [COMMAND]
    hyprmux [--socket PATH] run-action <ACTION_ID>
    hyprmux [--socket PATH] capture-pane [--target <PANE_ID>]
                            [--scrollback <N|full>] [--last-output]
    hyprmux [--socket PATH] send-keys [-l|--literal] [--] <KEY|TEXT>...
    hyprmux [--socket PATH] switch-workspace <1-9>
    hyprmux [--socket PATH] move-to-workspace <1-9>
    hyprmux --session <NAME> [--read-only]
    hyprmux --remote <HOST|ssh://URL> [TARGET] [--read-only]
    hyprmux --pick
    hyprmux list-sessions [--format text|json] [--remote <HOST>]
    hyprmux kill-session <NAME> [--remote <HOST>]
    hyprmux --server <NAME>
    hyprmux --session <NAME> --server

OPTIONS:
    -h, --help            Print help
    -V, --version         Print version
        --config <PATH>   Use an alternate hyprmux.toml (sets HYPRMUX_CONFIG)
        --socket <PATH>   Connect CLI control command to this endpoint
        --remote [HOST]   Attach via SSH (HOST alias or ssh:// URL; omit HOST to use [remote] default_host)
        --pick            Open the session picker at startup when a named session exists
        --read-only       Attach as a viewer that cannot type or control the layout
        --profile <NAME>  Seed an explicit new session from this profile

{endpoint_help}

TARGET attaches to a running named session or launches it from a same-named profile.
Leave the running app with prefix d (detach) or a configured quit binding."
    );
}

/// The `--socket`/`HYPRMUX_SOCKET` explanation, which differs by platform: a Unix-domain socket
/// path on Linux/macOS, a named-pipe registry entry on Windows (see `platform::ipc::windows` for
/// why the *entry*, not the pipe name, is what a user points at).
fn endpoint_help() -> String {
    let runtime_dir = crate::control::runtime_dir()
        .map(|dir| dir.display().to_string())
        .unwrap_or_else(|_| "the hyprmux runtime directory".to_string());
    if cfg!(windows) {
        format!(
            "Control endpoints live in {runtime_dir}. Each entry stands for a named pipe\n\
             (\\\\.\\pipe\\hyprmux.<user-sid>.control-<pid>); pass the entry, not the pipe name.\n\
             With no --socket, HYPRMUX_SOCKET is used, then the only live endpoint found there."
        )
    } else {
        format!(
            "Control sockets live in {runtime_dir} (one per running hyprmux, named by pid).\n\
             With no --socket, HYPRMUX_SOCKET is used, then the only live socket found there."
        )
    }
}

pub(crate) fn print_version() {
    use crate::session::protocol::{MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION};
    println!("hyprmux {}", env!("CARGO_PKG_VERSION"));
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
            ParsedCli::Help
        ));
        assert!(matches!(
            parse_cli_args(vec!["-V".into()]).expect("parses"),
            ParsedCli::Version
        ));
    }

    #[test]
    fn cli_reserved_control_commands_do_not_parse_as_profiles() {
        let parsed = parse_cli_args(vec!["list-panes".into()]).expect("parses");
        assert!(matches!(parsed, ParsedCli::Control(_)));

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
            "/tmp/hyprmux.sock".into(),
            "send-text".into(),
            "hi".into(),
        ])
        .expect("parses");
        let ParsedCli::Control(control) = parsed else {
            panic!("expected control");
        };
        assert_eq!(
            control.socket.as_deref(),
            Some(std::path::Path::new("/tmp/hyprmux.sock"))
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
                remote: None
            }
        ));
        assert!(matches!(
            parse_cli_args(vec!["kill-session".into(), "dev".into()]).expect("parses"),
            ParsedCli::KillSession {
                name,
                remote: None
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
                remote: None
            }
        ));
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

    fn expect_run(parsed: ParsedCli) -> CliArgs {
        match parsed {
            ParsedCli::Run(args) => args,
            other => panic!("expected run args, got {other:?}"),
        }
    }
}
