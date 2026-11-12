use std::iter::Peekable;
use std::path::PathBuf;
use std::vec::IntoIter;

use super::{
    CliArgs, ControlCli, ListFormat, ParsedCli, control_endpoint, control_request,
    parse_list_format, require_value,
};
use crate::cli::help::{HelpSection, HelpStyles, append_help_sections, row};
use crate::control::ControlCommand;

pub(in crate::cli) const HELP_SECTIONS: &[HelpSection] = &[
    HelpSection {
        heading: "USAGE",
        advanced_only: false,
        note: "",
        rows: &[row("rozi ports <COMMAND> [OPTIONS]", "")],
    },
    HelpSection {
        heading: "COMMANDS",
        advanced_only: false,
        note: "",
        rows: &[
            row(
                "list [--format text|json]",
                "List client-owned SSH port forwards",
            ),
            row(
                "forward <PORT> [OPTIONS]",
                "Forward a remote TCP port onto 127.0.0.1",
            ),
            row(
                "close <PORT> [OPTIONS]",
                "Close an active or failed port forward",
            ),
        ],
    },
    HelpSection {
        heading: "OPTIONS",
        advanced_only: false,
        note: "",
        rows: &[
            row(
                "--remote <HOST>",
                "Target host (default: active remote session)",
            ),
            row("--host <HOST>", "Remote connect host (default: localhost)"),
            row(
                "--local-port <PORT>",
                "forward: bind this loopback port on 127.0.0.1",
            ),
            row("--format text|json", "Print structured output"),
            row("--json", "Alias for --format json"),
            row("-h, --help", "Print help"),
        ],
    },
];

pub(crate) fn print_help() {
    let styles = HelpStyles::detect();
    let mut out = styles.title_line("rozi ports", "manage client SSH port forwards");
    append_help_sections(&mut out, HELP_SECTIONS, &styles, true);
    println!("{out}");
}

pub(super) fn parse(
    iter: &mut Peekable<IntoIter<String>>,
    cli: &mut CliArgs,
    mut socket: Option<PathBuf>,
) -> std::result::Result<ParsedCli, String> {
    if iter
        .clone()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return Ok(ParsedCli::PortsHelp);
    }

    let Some(subcommand) = iter.next() else {
        return Ok(ParsedCli::PortsHelp);
    };

    match subcommand.as_str() {
        "help" => Ok(ParsedCli::PortsHelp),
        "list" => parse_list(iter, cli, socket),
        "forward" => parse_forward(iter, cli, &mut socket).map(ParsedCli::Control),
        "close" => parse_close(iter, cli, &mut socket).map(ParsedCli::Control),
        other if other.starts_with('-') => Err(format!("unknown ports option `{other}`")),
        other => Err(format!(
            "unknown ports subcommand `{other}`; expected `list`, `forward`, or `close`"
        )),
    }
}

fn take_output_or_socket(
    arg: &str,
    iter: &mut Peekable<IntoIter<String>>,
    output_format: &mut Option<ListFormat>,
    socket: &mut Option<PathBuf>,
    command_name: &str,
) -> std::result::Result<bool, String> {
    match arg {
        "--json" => {
            if output_format.replace(ListFormat::Json).is_some() {
                return Err(format!(
                    "{command_name} output format specified more than once"
                ));
            }
            Ok(true)
        }
        "--format" => {
            let value = require_value(iter, "--format requires text or json")?;
            let parsed = parse_list_format(&value, command_name)?;
            if output_format.replace(parsed).is_some() {
                return Err(format!(
                    "{command_name} output format specified more than once"
                ));
            }
            Ok(true)
        }
        "--socket" => {
            let path = require_value(iter, "--socket requires a path")?;
            if socket.replace(PathBuf::from(path)).is_some() {
                return Err("--socket specified more than once".to_string());
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn parse_list(
    iter: &mut Peekable<IntoIter<String>>,
    cli: &CliArgs,
    mut socket: Option<PathBuf>,
) -> std::result::Result<ParsedCli, String> {
    let mut output_format = None;
    while let Some(arg) = iter.next() {
        if !take_output_or_socket(&arg, iter, &mut output_format, &mut socket, "ports list")? {
            return Err(format!("unexpected argument `{arg}` after ports list"));
        }
    }
    let command = ControlCommand::PortsList;
    if cli.attach_session.is_none() && cli.remote.is_some() {
        return Err("ports list does not accept `--remote`".to_string());
    }
    Ok(ParsedCli::Control(ControlCli {
        config_path: cli.config_path.clone(),
        endpoint: control_endpoint(cli, socket, &command)?,
        request: control_request(command),
        output_format,
        output: None,
    }))
}

fn parse_port_number(raw: &str, label: &str) -> std::result::Result<u16, String> {
    let trimmed = raw.trim();
    let parsed: u16 = trimmed
        .parse()
        .map_err(|_| format!("{label} `{trimmed}` must be a port number between 1 and 65535"))?;
    if parsed == 0 {
        return Err(format!("{label} must be between 1 and 65535"));
    }
    Ok(parsed)
}

fn parse_forward(
    iter: &mut Peekable<IntoIter<String>>,
    cli: &mut CliArgs,
    socket: &mut Option<PathBuf>,
) -> std::result::Result<ControlCli, String> {
    let mut remote = if cli.attach_session.is_none() {
        cli.remote.take()
    } else {
        None
    };
    let mut host = None;
    let mut port = None;
    let mut local_port = None;
    let mut output_format = None;

    while let Some(arg) = iter.next() {
        if take_output_or_socket(&arg, iter, &mut output_format, socket, "ports forward")? {
            continue;
        }
        match arg.as_str() {
            "--remote" => {
                let value = require_value(iter, "--remote requires a host")?;
                if remote.replace(value).is_some() {
                    return Err("ports forward --remote specified more than once".to_string());
                }
            }
            "--host" => {
                let value = require_value(iter, "--host requires a hostname or IP")?;
                if host.replace(value).is_some() {
                    return Err("ports forward --host specified more than once".to_string());
                }
            }
            "--local-port" => {
                let value = require_value(iter, "--local-port requires a port number")?;
                let parsed = parse_port_number(&value, "local port")?;
                if local_port.replace(parsed).is_some() {
                    return Err("ports forward --local-port specified more than once".to_string());
                }
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown ports forward option `{other}`"));
            }
            positional => {
                if port.is_some() {
                    return Err(format!(
                        "unexpected argument `{positional}` after ports forward"
                    ));
                }
                port = Some(parse_port_number(positional, "remote port")?);
            }
        }
    }

    let port = port.ok_or_else(|| "ports forward requires a remote port".to_string())?;
    let command = ControlCommand::PortsForward {
        remote,
        host,
        port,
        local_port,
    };
    Ok(ControlCli {
        config_path: cli.config_path.clone(),
        endpoint: control_endpoint(cli, socket.take(), &command)?,
        request: control_request(command),
        output_format,
        output: None,
    })
}

fn parse_close(
    iter: &mut Peekable<IntoIter<String>>,
    cli: &mut CliArgs,
    socket: &mut Option<PathBuf>,
) -> std::result::Result<ControlCli, String> {
    let mut remote = if cli.attach_session.is_none() {
        cli.remote.take()
    } else {
        None
    };
    let mut host = None;
    let mut port = None;
    let mut output_format = None;

    while let Some(arg) = iter.next() {
        if take_output_or_socket(&arg, iter, &mut output_format, socket, "ports close")? {
            continue;
        }
        match arg.as_str() {
            "--remote" => {
                let value = require_value(iter, "--remote requires a host")?;
                if remote.replace(value).is_some() {
                    return Err("ports close --remote specified more than once".to_string());
                }
            }
            "--host" => {
                let value = require_value(iter, "--host requires a hostname or IP")?;
                if host.replace(value).is_some() {
                    return Err("ports close --host specified more than once".to_string());
                }
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown ports close option `{other}`"));
            }
            positional => {
                if port.is_some() {
                    return Err(format!(
                        "unexpected argument `{positional}` after ports close"
                    ));
                }
                port = Some(parse_port_number(positional, "remote port")?);
            }
        }
    }

    let port = port.ok_or_else(|| "ports close requires a remote port".to_string())?;
    let command = ControlCommand::PortsClose { remote, host, port };
    Ok(ControlCli {
        config_path: cli.config_path.clone(),
        endpoint: control_endpoint(cli, socket.take(), &command)?,
        request: control_request(command),
        output_format,
        output: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::{ControlEndpoint, parse_cli_args};

    fn parse(args: &[&str]) -> std::result::Result<ParsedCli, String> {
        parse_cli_args(args.iter().map(|arg| (*arg).to_string()).collect())
    }

    #[test]
    fn ports_help_and_subcommands_parse_and_target_ui_socket() {
        assert!(matches!(parse(&["ports"]), Ok(ParsedCli::PortsHelp)));
        assert!(matches!(
            parse(&["ports", "--help"]),
            Ok(ParsedCli::PortsHelp)
        ));

        let ParsedCli::Control(list) = parse(&["ports", "list", "--json"]).unwrap() else {
            panic!("expected control command");
        };
        assert_eq!(list.endpoint, ControlEndpoint::Ui(None));
        assert_eq!(list.output_format, Some(ListFormat::Json));
        assert_eq!(list.request.command, ControlCommand::PortsList);

        let ParsedCli::Control(fwd) = parse(&[
            "ports",
            "forward",
            "5173",
            "--remote",
            "workbox",
            "--host",
            "db.internal",
            "--local-port",
            "15173",
        ])
        .unwrap() else {
            panic!("expected control command");
        };
        assert_eq!(fwd.endpoint, ControlEndpoint::Ui(None));
        assert_eq!(
            fwd.request.command,
            ControlCommand::PortsForward {
                remote: Some("workbox".into()),
                host: Some("db.internal".into()),
                port: 5173,
                local_port: Some(15173),
            }
        );

        let ParsedCli::Control(fwd_prefix) =
            parse(&["--remote", "workbox", "ports", "forward", "3000"]).unwrap()
        else {
            panic!("expected control command");
        };
        assert_eq!(
            fwd_prefix.request.command,
            ControlCommand::PortsForward {
                remote: Some("workbox".into()),
                host: None,
                port: 3000,
                local_port: None,
            }
        );

        let ParsedCli::Control(close) =
            parse(&["ports", "close", "5173", "--remote", "workbox"]).unwrap()
        else {
            panic!("expected control command");
        };
        assert_eq!(
            close.request.command,
            ControlCommand::PortsClose {
                remote: Some("workbox".into()),
                host: None,
                port: 5173,
            }
        );
    }

    #[test]
    fn ports_rejects_session_targets_and_invalid_ports() {
        let session_err = parse(&["--session", "dev", "ports", "list"]).unwrap_err();
        assert!(session_err.contains("UI client"));
        let here_err = parse(&["--here", "ports", "forward", "5173"]).unwrap_err();
        assert!(here_err.contains("UI client"));
        assert!(parse(&["ports", "forward", "0"]).is_err());
        assert!(parse(&["ports", "forward", "5173", "--local-port", "0"]).is_err());
        assert!(parse(&["ports", "close"]).is_err());
    }
}
