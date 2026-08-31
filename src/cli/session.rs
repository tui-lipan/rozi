//! Session subcommands: listing and killing sessions, and the two hidden server entry points a
//! client spawns rather than a person typing.

use tui_lipan::Result;

use super::args::ListFormat;
use super::output::{OutputStyles, OutputTone, TableCell, format_table};
use crate::session;

pub(crate) fn run_server_cli(name: &str, fresh: bool) -> Result<()> {
    session::server::run_named_session_mode(name, fresh)?;
    Ok(())
}

pub(crate) fn run_remote_serve_cli(name: &str) -> Result<()> {
    session::remote::run_remote_serve(name)?;
    Ok(())
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
        vec!["NAME", "STATUS", "PANES", "CLIENTS", "LAYOUT", "HOST"]
    } else {
        vec!["NAME", "STATUS", "PANES", "CLIENTS", "LAYOUT"]
    };
    let table_rows = rows
        .iter()
        .map(|session| {
            let (status, status_tone, panes, clients, layout) = match session.status {
                session::discovery::DiscoveredSessionStatus::Running {
                    panes,
                    clients,
                    has_layout,
                    ..
                } => (
                    "running",
                    OutputTone::Success,
                    panes.to_string(),
                    clients.to_string(),
                    if has_layout { "yes" } else { "no" }.to_string(),
                ),
                session::discovery::DiscoveredSessionStatus::Restorable => (
                    "restorable",
                    OutputTone::Accent,
                    "—".to_string(),
                    "0".to_string(),
                    "—".to_string(),
                ),
                session::discovery::DiscoveredSessionStatus::Busy => (
                    "busy",
                    OutputTone::Warning,
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
                ),
            };
            let mut cells = vec![
                TableCell::new(&session.name, OutputTone::Accent),
                TableCell::new(status, status_tone),
                TableCell::plain(panes),
                TableCell::plain(clients),
                TableCell::plain(layout),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_report_is_an_aligned_table_with_color_outside_its_widths() {
        let rows = vec![
            session::discovery::DiscoveredSession {
                name: "dev".into(),
                status: session::discovery::DiscoveredSessionStatus::Running {
                    panes: 5,
                    clients: 1,
                    has_layout: true,
                    created_from_profile: None,
                },
                ephemeral: false,
                host: None,
                remote_target: None,
            },
            session::discovery::DiscoveredSession {
                name: "saved-work".into(),
                status: session::discovery::DiscoveredSessionStatus::Restorable,
                ephemeral: false,
                host: None,
                remote_target: None,
            },
        ];
        let plain = format_sessions_text(&rows, OutputStyles::plain());
        assert_eq!(
            plain,
            "NAME        STATUS      PANES  CLIENTS  LAYOUT\n\
             dev         running     5      1        yes\n\
             saved-work  restorable  —      0        —\n"
        );
        assert!(plain.lines().all(|line| !line.ends_with(' ')));

        let colored = format_sessions_text(&rows, OutputStyles::colored());
        assert!(colored.contains("\x1b["));
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
