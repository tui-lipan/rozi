//! Session subcommands: listing and killing sessions, and the two hidden server entry points a
//! client spawns rather than a person typing.

use std::io::Write;

use tui_lipan::Result;

use super::args::ListFormat;
use super::output::{OutputStyles, OutputTone, TableCell, format_table};
use crate::session;

pub(crate) fn run_server_cli(
    name: &str,
    fresh: bool,
    startup_nonce: Option<String>,
    resurrect: bool,
) -> Result<()> {
    session::server::run_named_session_mode_with_nonce(name, fresh, startup_nonce, resurrect)?;
    Ok(())
}

pub(crate) fn run_remote_serve_cli(name: &str, autostart: bool) -> Result<()> {
    session::remote::run_remote_serve(name, autostart)?;
    Ok(())
}

/// `--remote-control <NAME>`: run one forwarded control request against a local session.
///
/// Whether the command succeeded is in the response this prints, not in the exit status. A
/// non-zero exit means the session could not be reached at all, which the forwarding side reports
/// as a failed command rather than a refused one.
pub(crate) fn run_remote_control_cli(name: &str) -> Result<()> {
    print_remote_answer(session::remote::control::run_remote_control(name))
}

/// `--remote-call <NAME>`: run one forwarded action call, for as long as its caller is there.
pub(crate) fn run_remote_call_cli(name: &str) -> Result<()> {
    print_remote_answer(session::remote::control::run_remote_call(name).map(Some))
}

fn print_remote_answer(
    answer: std::result::Result<Option<crate::control::ControlResponse>, String>,
) -> Result<()> {
    match answer {
        Ok(Some(response)) => {
            println!(
                "{}",
                serde_json::to_string(&response).map_err(std::io::Error::other)?
            );
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(session::remote::control::UNREACHABLE_EXIT);
        }
    }
}

/// `--remote-worktrees`: run one forwarded worktree call on this host.
///
/// A call the host ran and refused is still an answer, printed like a success. A non-zero exit
/// means the call could not be read at all.
pub(crate) fn run_remote_worktrees_cli() -> Result<()> {
    match session::remote::worktrees::run_remote_worktrees() {
        Ok(reply) => {
            println!(
                "{}",
                serde_json::to_string(&reply).map_err(std::io::Error::other)?
            );
            Ok(())
        }
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}

pub(super) fn format_sessions_text(
    rows: &[session::discovery::DiscoveredSession],
    styles: OutputStyles,
) -> String {
    if rows.is_empty() {
        return format!(
            "{}\n",
            styles.paint("No sessions found.", OutputTone::Muted)
        );
    }
    let show_host = rows.iter().any(|row| row.host.is_some());
    let headers = if show_host {
        vec![
            "NAME", "STATUS", "PANES", "CLIENTS", "LAYOUT", "VERSION", "PROTOCOL", "HOST",
        ]
    } else {
        vec![
            "NAME", "STATUS", "PANES", "CLIENTS", "LAYOUT", "VERSION", "PROTOCOL",
        ]
    };
    let table_rows = rows
        .iter()
        .map(|session| {
            let (status, status_tone, panes, clients, layout, version, protocol) =
                match &session.status {
                    session::discovery::DiscoveredSessionStatus::Running {
                        panes,
                        clients,
                        has_layout,
                        effective_protocol,
                        rozi_version,
                    } => (
                        "running",
                        OutputTone::Success,
                        panes.to_string(),
                        clients.to_string(),
                        if *has_layout { "yes" } else { "no" }.to_string(),
                        rozi_version.as_deref().unwrap_or("—").to_string(),
                        effective_protocol
                            .map_or_else(|| "—".to_string(), |protocol| protocol.to_string()),
                    ),
                    // Unreachable from the CLI, which probes live and never reads a client's host
                    // cache. Spelled out rather than folded into a catch-all so that if a cached row
                    // ever reaches this table it cannot be printed as `running`.
                    session::discovery::DiscoveredSessionStatus::LastSeen { panes } => (
                        "last-seen",
                        OutputTone::Warning,
                        panes.to_string(),
                        "—".to_string(),
                        "—".to_string(),
                        "—".to_string(),
                        "—".to_string(),
                    ),
                    session::discovery::DiscoveredSessionStatus::Restorable => (
                        "restorable",
                        OutputTone::Accent,
                        "—".to_string(),
                        "0".to_string(),
                        "—".to_string(),
                        "—".to_string(),
                        "—".to_string(),
                    ),
                    session::discovery::DiscoveredSessionStatus::Unreadable => (
                        "unreadable",
                        OutputTone::Error,
                        "—".to_string(),
                        "0".to_string(),
                        "—".to_string(),
                        "—".to_string(),
                        "—".to_string(),
                    ),
                    session::discovery::DiscoveredSessionStatus::Busy => (
                        "busy",
                        OutputTone::Warning,
                        "—".to_string(),
                        "—".to_string(),
                        "—".to_string(),
                        "—".to_string(),
                        "—".to_string(),
                    ),
                    session::discovery::DiscoveredSessionStatus::Unknown => (
                        "unknown",
                        OutputTone::Error,
                        "—".to_string(),
                        "—".to_string(),
                        "—".to_string(),
                        "—".to_string(),
                        "—".to_string(),
                    ),
                };
            let mut cells = vec![
                TableCell::new(&session.name, OutputTone::Key),
                TableCell::new(status, status_tone),
                TableCell::plain(panes),
                TableCell::plain(clients),
                TableCell::plain(layout),
                TableCell::new(version, OutputTone::Muted),
                TableCell::new(protocol, OutputTone::Muted),
            ];
            if show_host {
                cells.push(TableCell::new(
                    session.host.as_deref().unwrap_or("—"),
                    OutputTone::Muted,
                ));
            }
            cells
        })
        .collect::<Vec<_>>();
    format_table(&headers, &table_rows, styles)
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
            print!("{}", format_sessions_text(&rows, OutputStyles::detect()));
        }
    }
    Ok(())
}

pub(crate) fn run_new_detached_session_cli(
    name: &str,
    resurrect: bool,
    remote: Option<&str>,
) -> Result<()> {
    let Some(remote) = remote else {
        session::server::create_detached_session(name, resurrect)?;
        return Ok(());
    };
    let target = session::remote::parse_remote_target(remote)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    let config = crate::config::load_config().config.remote;
    session::remote::create_remote_detached_session(&target, name, resurrect, &config)
        .map_err(std::io::Error::other)?;
    Ok(())
}

pub(crate) fn run_restart_session_cli(
    name: &str,
    resurrect: bool,
    remote: Option<&str>,
) -> Result<()> {
    let Some(remote) = remote else {
        session::server::restart_named_session(name, resurrect)?;
        return Ok(());
    };
    let target = session::remote::parse_remote_target(remote)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    let config = crate::config::load_config().config.remote;
    let stderr = session::remote::restart_remote_session(&target, name, resurrect, &config)
        .map_err(std::io::Error::other)?;
    std::io::stderr().lock().write_all(&stderr)?;
    Ok(())
}

pub(crate) fn run_upgrade_session_cli(name: &str, remote: Option<&str>) -> Result<()> {
    let Some(remote) = remote else {
        let current = env!("CARGO_PKG_VERSION");
        match session::discovery::session_rozi_version(name) {
            Some(version) if version == current => eprintln!(
                "Session `{name}` is already on rozi {current}; restarting from its snapshot."
            ),
            Some(version) => {
                eprintln!("Upgrading session `{name}` from rozi {version} to {current}.")
            }
            // Not running, or too old to say: the upgrade itself reports what it finds.
            None => {}
        }
        session::server::upgrade_named_session(name)?;
        return Ok(());
    };
    let target = session::remote::parse_remote_target(remote)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    let config = crate::config::load_config().config.remote;
    let stderr = session::remote::upgrade_remote_session(&target, name, &config)
        .map_err(std::io::Error::other)?;
    std::io::stderr().lock().write_all(&stderr)?;
    Ok(())
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
    let found = session::server::shutdown_named_session_if_present(name)
        .map_err(|err| std::io::Error::other(format!("could not kill session {name:?}: {err}")))?;
    if !found {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Session `{name}` was not found."),
        )
        .into());
    }
    Ok(())
}

fn run_kill_session_remote(name: &str, remote: &str) -> Result<()> {
    let target = session::remote::parse_remote_target(remote)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    let config = crate::config::load_config().config.remote;
    session::remote::kill_remote_session(&target, name, &config).map_err(std::io::Error::other)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_report_is_an_aligned_table_with_color_outside_its_widths() {
        let rows = vec![
            session::discovery::DiscoveredSession {
                name: "dev".into(),
                origin: Default::default(),
                status: session::discovery::DiscoveredSessionStatus::Running {
                    panes: 5,
                    clients: 1,
                    has_layout: true,
                    effective_protocol: Some(52),
                    rozi_version: Some("0.0.43".into()),
                },
                ephemeral: false,
                host: None,
                remote_target: None,
            },
            session::discovery::DiscoveredSession {
                name: "saved-work".into(),
                origin: Default::default(),
                status: session::discovery::DiscoveredSessionStatus::Restorable,
                ephemeral: false,
                host: None,
                remote_target: None,
            },
        ];
        let plain = format_sessions_text(&rows, OutputStyles::plain());
        assert_eq!(
            plain,
            "NAME        STATUS      PANES  CLIENTS  LAYOUT  VERSION  PROTOCOL\n\
             dev         running     5      1        yes     0.0.43   52\n\
             saved-work  restorable  —      0        —       —        —\n"
        );
        assert!(plain.lines().all(|line| !line.ends_with(' ')));

        let colored = format_sessions_text(&rows, OutputStyles::colored());
        // Column headers read like the section headings in `--help`: bold rose. The key column
        // under them is regular weight and softened toward lavender so it does not merge into them.
        use crate::platform::ansi::{BOLD, RESET, fg, palette};
        let heading = format!("{BOLD}{}", fg(palette::ROSE, true));
        let key = fg(palette::ROSE.mix(palette::LAVENDER, 1, 2), true);
        assert!(colored.starts_with(&format!("{heading}NAME{RESET}")));
        assert!(colored.contains(&format!("\n{key}dev{RESET}")));
        let mut stripped = String::with_capacity(colored.len());
        let mut rest = colored.as_str();
        while let Some(start) = rest.find('\x1b') {
            stripped.push_str(&rest[..start]);
            let end = rest[start..]
                .find('m')
                .expect("every output style ends in `m`");
            rest = &rest[start + end + 1..];
        }
        stripped.push_str(rest);
        assert_eq!(stripped, plain);
    }
}
