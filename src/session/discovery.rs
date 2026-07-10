use std::path::Path;
use std::time::Duration;

use crate::session::protocol::{ClientMessage, PROTOCOL_VERSION, ServerMessage};

const QUERY_TIMEOUT: Duration = Duration::from_millis(60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscoveredSessionStatus {
    Running {
        panes: usize,
        clients: u32,
        has_layout: bool,
    },
    Busy,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredSession {
    pub name: String,
    pub status: DiscoveredSessionStatus,
    /// Auto-managed per-process session (`eph-*`), disposable and not user-named.
    pub ephemeral: bool,
}

/// Whether `name` is a well-formed *attach target*. Like [`valid_session_name`] but permits the
/// reserved `eph-` prefix: attaching to an already-running ephemeral session (our own, shown as the
/// current row) is legitimate even though users may not *create* ephemeral names.
pub fn valid_attach_target(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

/// Whether `name` is a valid *user-created* session name. Rejects the reserved ephemeral prefix so
/// create/rename never mints an `eph-…` name that would collide with an auto-managed session.
pub fn valid_session_name(name: &str) -> bool {
    valid_attach_target(name) && !crate::state::is_ephemeral_session_name(name)
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
                &ClientMessage::Query {
                    session: name.to_string(),
                    protocol_version: PROTOCOL_VERSION,
                },
            )
            .is_err()
            {
                DiscoveredSessionStatus::Unknown
            } else {
                match crate::session::protocol::read_frame::<_, ServerMessage>(&mut stream) {
                    Ok(ServerMessage::SessionInfo {
                        panes,
                        clients,
                        has_layout,
                        ..
                    }) => DiscoveredSessionStatus::Running {
                        panes,
                        clients,
                        has_layout,
                    },
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
    fn attach_target_permits_ephemeral_but_not_junk() {
        // Attaching to an already-running ephemeral (our own current row) is legitimate even though
        // it can never be *created* by the user.
        assert!(valid_attach_target("eph-1234"));
        assert!(valid_attach_target("dev_1-prod"));
        assert!(!valid_attach_target(""));
        assert!(!valid_attach_target("bad/name"));
        assert!(!valid_attach_target("bad name"));
    }
}
