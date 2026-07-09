use std::path::Path;
use std::time::Duration;

use crate::session::protocol::{ClientMessage, PROTOCOL_VERSION, ServerMessage};

const QUERY_TIMEOUT: Duration = Duration::from_millis(60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscoveredSessionStatus {
    Running { panes: usize, has_layout: bool },
    Busy,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredSession {
    pub name: String,
    pub status: DiscoveredSessionStatus,
    /// Auto-managed per-process session (`eph-*`), disposable and not user-named.
    pub ephemeral: bool,
    /// A synthetic picker row (not a real discovered session): the always-present "new ephemeral
    /// session" entry. It has no socket, renders with a fixed label, always sits at the top, is
    /// always actionable via Enter, and cannot be killed.
    pub synthetic: bool,
}

impl DiscoveredSession {
    /// The synthetic "new ephemeral session" row shown at the top of the picker.
    pub fn new_ephemeral_row() -> Self {
        Self {
            name: String::new(),
            status: DiscoveredSessionStatus::Unknown,
            ephemeral: true,
            synthetic: true,
        }
    }
}

pub fn valid_session_name(name: &str) -> bool {
    !name.is_empty()
        && !crate::state::is_ephemeral_session_name(name)
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

pub fn discover_sessions() -> std::io::Result<Vec<DiscoveredSession>> {
    discover_sessions_excluding(None)
}

pub fn discover_sessions_excluding(
    exclude_name: Option<&str>,
) -> std::io::Result<Vec<DiscoveredSession>> {
    let dir = crate::control::runtime_dir()?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut sockets = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(name) = file_name
            .strip_prefix("session-")
            .and_then(|name| name.strip_suffix(".sock"))
        else {
            continue;
        };
        if exclude_name == Some(name) {
            continue;
        }
        sockets.push((name.to_string(), path));
    }

    let mut handles = Vec::with_capacity(sockets.len());
    for (name, path) in sockets {
        handles.push(std::thread::spawn(move || {
            query_session_socket(&name, &path)
        }));
    }

    let mut rows = Vec::with_capacity(handles.len());
    for handle in handles {
        if let Ok(Some(row)) = handle.join() {
            rows.push(row);
        }
    }
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(rows)
}

/// Probes one session socket. Returns `None` for a stale socket whose server is gone (connection
/// refused): the dead file is unlinked so a killed or crashed session stops appearing in the list.
pub fn query_session_socket(name: &str, path: &Path) -> Option<DiscoveredSession> {
    let status = match std::os::unix::net::UnixStream::connect(path) {
        Ok(mut stream) => {
            let _ = stream.set_read_timeout(Some(QUERY_TIMEOUT));
            let _ = stream.set_write_timeout(Some(QUERY_TIMEOUT));
            if crate::session::protocol::write_frame(
                &mut stream,
                &ClientMessage::Attach {
                    session: name.to_string(),
                    protocol_version: PROTOCOL_VERSION,
                },
            )
            .is_err()
            {
                DiscoveredSessionStatus::Unknown
            } else {
                match crate::session::protocol::read_frame::<_, ServerMessage>(&mut stream) {
                    Ok(ServerMessage::Attached {
                        panes, layout_blob, ..
                    }) => {
                        let _ = crate::session::protocol::write_frame(
                            &mut stream,
                            &ClientMessage::Detach,
                        );
                        DiscoveredSessionStatus::Running {
                            panes: panes.iter().filter(|pane| pane.exited.is_none()).count(),
                            has_layout: layout_blob.is_some(),
                        }
                    }
                    Err(err)
                        if matches!(
                            err.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        DiscoveredSessionStatus::Busy
                    }
                    _ => DiscoveredSessionStatus::Unknown,
                }
            }
        }
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            DiscoveredSessionStatus::Busy
        }
        Err(_) => {
            let _ = std::fs::remove_file(path);
            return None;
        }
    };
    Some(DiscoveredSession {
        name: name.to_string(),
        ephemeral: crate::state::is_ephemeral_session_name(name),
        status,
        synthetic: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_in_app_session_names() {
        assert!(valid_session_name("dev_1-prod"));
        assert!(!valid_session_name(""));
        assert!(!valid_session_name("bad/name"));
        assert!(!valid_session_name("bad name"));
        // The `eph-` prefix is reserved for auto-managed ephemeral sessions.
        assert!(!valid_session_name("eph-1234"));
    }

    #[test]
    fn synthetic_new_ephemeral_row_is_flagged_and_nameless() {
        let row = DiscoveredSession::new_ephemeral_row();
        assert!(row.synthetic);
        assert!(row.ephemeral);
        assert!(row.name.is_empty());
        // A nameless synthetic row must never be mistaken for a real, killable/attachable session.
        assert!(!valid_session_name(&row.name));
    }
}
