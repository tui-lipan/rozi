use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use tui_lipan::Result;

use crate::platform::ipc::{EndpointRegistry, IpcEndpoint};
use crate::{control, session};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CliArgs {
    pub(crate) profile: Option<String>,
    pub(crate) config_path: Option<String>,
    pub(crate) attach_session: Option<String>,
    /// Open the session picker at startup instead of silently attaching to an ephemeral session
    /// (also enabled by `[session] startup = "picker"`). Ignored when `--attach`/`--session` is
    /// given or no named session exists.
    pub(crate) pick: bool,
    pub(crate) read_only: bool,
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
    Server { name: String },
    ListSessions,
    KillSession { name: String },
}

pub(crate) fn parse_cli_args(args: Vec<String>) -> std::result::Result<ParsedCli, String> {
    let mut cli = CliArgs::default();
    let mut socket: Option<PathBuf> = None;
    let mut socket_flag_seen = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(ParsedCli::Help),
            "--version" | "-V" => return Ok(ParsedCli::Version),
            "list-sessions" => {
                reject_trailing_control_args(&mut iter, "list-sessions")?;
                return Ok(ParsedCli::ListSessions);
            }
            "kill-session" => {
                let name = iter
                    .next()
                    .ok_or_else(|| "kill-session requires a session name".to_string())?;
                reject_trailing_control_args(&mut iter, "kill-session")?;
                return Ok(ParsedCli::KillSession { name });
            }
            "--server" => {
                let name = iter
                    .next()
                    .ok_or_else(|| "--server requires a session name".to_string())?;
                reject_trailing_control_args(&mut iter, "--server")?;
                return Ok(ParsedCli::Server { name });
            }
            "--session" => {
                let name = iter
                    .next()
                    .ok_or_else(|| "--session requires a session name".to_string())?;
                match iter.next().as_deref() {
                    Some("--server") => {
                        reject_trailing_control_args(&mut iter, "--server")?;
                        return Ok(ParsedCli::Server { name });
                    }
                    Some("--read-only") => {
                        cli.attach_session = Some(name);
                        cli.read_only = true;
                    }
                    Some(other) => {
                        return Err(format!(
                            "unexpected argument `{other}` after --session <NAME>"
                        ));
                    }
                    None => {
                        cli.attach_session = Some(name);
                    }
                }
            }
            "--attach" => {
                cli.attach_session = Some(
                    iter.next()
                        .ok_or_else(|| "--attach requires a session name".to_string())?,
                );
            }
            "--pick" => {
                cli.pick = true;
            }
            "--read-only" => {
                cli.read_only = true;
            }
            "--profile" | "-p" => {
                let name = iter
                    .next()
                    .ok_or_else(|| "--profile requires a profile name".to_string())?;
                if cli.profile.is_some() {
                    return Err("profile name specified more than once".to_string());
                }
                cli.profile = Some(name);
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
                let text = iter.next().ok_or_else(|| {
                    "send-keys requires literal text (named keys are not implemented)".to_string()
                })?;
                reject_trailing_control_args(&mut iter, "send-keys")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket,
                    request: control_request(control::ControlCommand::SendText {
                        target: None,
                        text,
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
                if let Some(next) = iter.next() {
                    if next == "--target" {
                        let value = iter
                            .next()
                            .ok_or_else(|| "--target requires a pane id".to_string())?;
                        target = Some(
                            value
                                .parse()
                                .map_err(|_| "--target requires a numeric pane id".to_string())?,
                        );
                    } else {
                        return Err(format!("unexpected argument `{next}` after capture-pane"));
                    }
                }
                reject_trailing_control_args(&mut iter, "capture-pane")?;
                return Ok(ParsedCli::Control(ControlCli {
                    socket,
                    request: control_request(control::ControlCommand::CapturePane { target }),
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
                if cli.profile.is_some() {
                    return Err(format!("unexpected argument `{name}`"));
                }
                cli.profile = Some(name.to_string());
            }
        }
    }
    if socket_flag_seen {
        return Err("--socket requires a control command".to_string());
    }
    if cli.read_only && cli.attach_session.is_none() {
        return Err("--read-only requires --attach or --session".to_string());
    }
    Ok(ParsedCli::Run(cli))
}

fn reject_trailing_control_args(
    iter: &mut std::vec::IntoIter<String>,
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

pub(crate) fn run_server_cli(name: &str) -> Result<()> {
    session::server::run_named_session(name)?;
    Ok(())
}

pub(crate) fn run_list_sessions_cli() -> Result<()> {
    for session in session::discovery::discover_sessions()? {
        match session.status {
            session::discovery::DiscoveredSessionStatus::Running {
                panes,
                clients,
                has_layout,
            } => println!(
                "{}\trunning\tpanes={}\tclients={}\tlayout={}",
                session.name,
                panes,
                clients,
                if has_layout { "yes" } else { "no" }
            ),
            session::discovery::DiscoveredSessionStatus::Busy => {
                println!("{}\tbusy\tpanes=?\tclients=?\tlayout=?", session.name)
            }
            session::discovery::DiscoveredSessionStatus::Unknown => {
                println!("{}\tunknown\tpanes=?\tclients=?\tlayout=?", session.name)
            }
        }
    }
    Ok(())
}

pub(crate) fn run_kill_session_cli(name: &str) -> Result<()> {
    use crate::session::protocol::{ClientMessage, PROTOCOL_VERSION, ServerMessage};

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
                    label: std::env::var("USER").unwrap_or_else(|_| "client".to_string()),
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

pub(crate) fn print_help() {
    println!(
        "\
hyprmux - Hyprland-style tiling terminal multiplexer

USAGE:
    hyprmux [PROFILE]
    hyprmux --profile <NAME>
    hyprmux -p <NAME>
    hyprmux [--socket PATH] list|list-panes
    hyprmux [--socket PATH] focus <PANE_ID>
    hyprmux [--socket PATH] send-text <TEXT>
    hyprmux [--socket PATH] split [COMMAND]
    hyprmux [--socket PATH] run-action <ACTION_ID>
    hyprmux [--socket PATH] capture-pane [--target <PANE_ID>]
    hyprmux [--socket PATH] switch-workspace <1-9>
    hyprmux [--socket PATH] move-to-workspace <1-9>
    hyprmux --attach <NAME> [--read-only]
    hyprmux --session <NAME> [--read-only]
    hyprmux --pick
    hyprmux list-sessions
    hyprmux kill-session <NAME>
    hyprmux --server <NAME>
    hyprmux --session <NAME> --server

OPTIONS:
    -h, --help            Print help
    -V, --version         Print version
    -p, --profile <NAME>  Load a named profile from ~/.config/hyprmux/profiles/<NAME>.toml
        --config <PATH>   Use an alternate hyprmux.toml (sets HYPRMUX_CONFIG)
        --socket <PATH>   Connect CLI control command to this socket
        --pick            Open the session picker at startup when a named session exists
        --read-only       Attach as a viewer that cannot type or control the layout

A bare PROFILE positional is equivalent to --profile PROFILE.
Leave the running app with prefix d (detach) or a configured quit binding."
    );
}

pub(crate) fn print_version() {
    println!("hyprmux {}", env!("CARGO_PKG_VERSION"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_profile_flag_and_positional() {
        let flag =
            expect_run(parse_cli_args(vec!["--profile".into(), "dev".into()]).expect("parses"));
        assert_eq!(flag.profile.as_deref(), Some("dev"));

        let positional = expect_run(parse_cli_args(vec!["dev".into()]).expect("parses"));
        assert_eq!(positional.profile.as_deref(), Some("dev"));
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
            parse_cli_args(vec!["--profile".into(), "list-panes".into()]).expect("parses"),
        );
        assert_eq!(profile.profile.as_deref(), Some("list-panes"));
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
            control::ControlCommand::CapturePane { target: None }
        );

        let ParsedCli::Control(capture_target) =
            parse_cli_args(vec!["capture-pane".into(), "--target".into(), "7".into()])
                .expect("parses")
        else {
            panic!("expected control");
        };
        assert_eq!(
            capture_target.request.command,
            control::ControlCommand::CapturePane { target: Some(7) }
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
    fn cli_parses_session_verbs_and_attach() {
        assert!(matches!(
            parse_cli_args(vec!["list-sessions".into()]).expect("parses"),
            ParsedCli::ListSessions
        ));
        assert!(matches!(
            parse_cli_args(vec!["kill-session".into(), "dev".into()]).expect("parses"),
            ParsedCli::KillSession { name } if name == "dev"
        ));
        let attached =
            expect_run(parse_cli_args(vec!["--attach".into(), "dev".into()]).expect("parses"));
        assert_eq!(attached.attach_session.as_deref(), Some("dev"));
        let session =
            expect_run(parse_cli_args(vec!["--session".into(), "dev".into()]).expect("parses"));
        assert_eq!(session.attach_session.as_deref(), Some("dev"));
        assert!(parse_cli_args(vec!["kill-session".into()]).is_err());
    }

    #[test]
    fn cli_read_only_requires_attach_target() {
        assert!(parse_cli_args(vec!["--read-only".into()]).is_err());
        let args = expect_run(
            parse_cli_args(vec!["--attach".into(), "dev".into(), "--read-only".into()])
                .expect("parses"),
        );
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
        assert_eq!(with_profile.profile.as_deref(), Some("dev"));
    }

    fn expect_run(parsed: ParsedCli) -> CliArgs {
        match parsed {
            ParsedCli::Run(args) => args,
            other => panic!("expected run args, got {other:?}"),
        }
    }
}
