use std::time::Duration;

use crate::platform::ipc::{EndpointRegistry, IpcEndpoint};
use crate::session::protocol::{
    ClientMessage, MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION, ServerMessage,
};

const QUERY_TIMEOUT: Duration = Duration::from_millis(60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscoveredSessionStatus {
    Running {
        panes: usize,
        clients: u32,
        has_layout: bool,
        created_from_profile: Option<String>,
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
    /// Remote host alias/URL when discovered over `--remote`; `None` for local.
    pub host: Option<String>,
}

/// Where to discover sessions from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionSource {
    Local,
    Remote(crate::session::remote::RemoteTarget),
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

/// Probe one named session without scanning or mutating unrelated discovery entries.
pub fn discover_session(name: &str) -> std::io::Result<Option<DiscoveredSession>> {
    let endpoint = crate::session::server::session_endpoint(name)?;
    Ok(query_session_endpoint(name, &endpoint))
}

pub fn discover_sessions_excluding(
    exclude_name: Option<&str>,
) -> std::io::Result<Vec<DiscoveredSession>> {
    let dir = crate::control::runtime_dir()?;
    let mut endpoints = EndpointRegistry::list_session_endpoints(&dir)?;
    if let Some(exclude_name) = exclude_name {
        endpoints.retain(|(name, _)| name != exclude_name);
    }

    let mut handles = Vec::with_capacity(endpoints.len());
    for (name, endpoint) in endpoints {
        handles.push(std::thread::spawn(move || {
            query_session_endpoint(&name, &endpoint)
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

/// Session-picker/sidebar discovery policy: omit the current session while probing and hide
/// foreign disposable sessions. The caller adds its locally known current row afterwards.
pub(crate) fn discover_selectable_sessions(
    current_name: Option<&str>,
) -> std::io::Result<Vec<DiscoveredSession>> {
    let mut rows = discover_sessions_excluding(current_name)?;
    rows.retain(|entry| !entry.ephemeral);
    Ok(rows)
}

/// Probes one session endpoint. Returns `None` for a stale endpoint whose server is gone
/// (connection refused): the dead socket file is unlinked so a killed or crashed session stops
/// appearing in the list.
pub fn query_session_endpoint(name: &str, endpoint: &IpcEndpoint) -> Option<DiscoveredSession> {
    let status = match endpoint.connect() {
        Ok(mut stream) => {
            let _ = stream.set_read_timeout(Some(QUERY_TIMEOUT));
            let _ = stream.set_write_timeout(Some(QUERY_TIMEOUT));
            if crate::session::protocol::write_frame(
                &mut stream,
                &ClientMessage::Query {
                    session: name.to_string(),
                    protocol_version: PROTOCOL_VERSION,
                    min_protocol_version: MIN_SUPPORTED_PROTOCOL,
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
                        created_from_profile,
                        ..
                    }) => DiscoveredSessionStatus::Running {
                        panes,
                        clients,
                        has_layout,
                        created_from_profile,
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
            let _ = std::fs::remove_file(endpoint.path());
            return None;
        }
    };
    Some(DiscoveredSession {
        name: name.to_string(),
        ephemeral: crate::state::is_ephemeral_session_name(name),
        status,
        host: None,
    })
}

/// Discover sessions from a local runtime dir or a remote host (one ssh round-trip).
pub fn discover_sessions_from(
    source: &SessionSource,
    config: &crate::config::HyprmuxRemoteConfig,
) -> std::io::Result<Vec<DiscoveredSession>> {
    match source {
        SessionSource::Local => discover_sessions(),
        SessionSource::Remote(target) => discover_remote_sessions(target, config),
    }
}

fn discover_remote_sessions(
    target: &crate::session::remote::RemoteTarget,
    config: &crate::config::HyprmuxRemoteConfig,
) -> std::io::Result<Vec<DiscoveredSession>> {
    use std::process::Stdio;

    let resolved = crate::session::remote::ResolvedRemote::resolve(target, config);
    let host_label = resolved
        .alias
        .clone()
        .unwrap_or_else(|| resolved.host.clone());
    if !crate::platform::command::program_exists("ssh") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "ssh was not found on PATH",
        ));
    }
    let remote_bin = resolved
        .binary_path
        .clone()
        .unwrap_or_else(|| "hyprmux".to_string());
    // `ssh_base_command` applies `ConnectTimeout`: an unreachable configured host must fail fast
    // rather than stall the picker's recurring discovery sweep on a TCP connect.
    let mut command = crate::session::remote::ssh_base_command(&resolved, config);
    command.arg(resolved.ssh_destination());
    command.arg("--");
    command.arg(&remote_bin);
    command.arg("list-sessions");
    command.arg("--format");
    command.arg("json");
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = command.output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "remote list-sessions failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    parse_remote_list_json(&output.stdout, Some(host_label))
}

/// Parse `list-sessions --format json` output (also used by remote discovery).
pub fn parse_remote_list_json(
    bytes: &[u8],
    host: Option<String>,
) -> std::io::Result<Vec<DiscoveredSession>> {
    #[derive(serde::Deserialize)]
    struct Row {
        name: String,
        status: String,
        #[serde(default)]
        panes: Option<usize>,
        #[serde(default)]
        clients: Option<u32>,
        #[serde(default)]
        layout: Option<bool>,
        #[serde(default)]
        created_from_profile: Option<String>,
        #[serde(default)]
        ephemeral: bool,
    }
    let rows: Vec<Row> = serde_json::from_slice(bytes).map_err(std::io::Error::other)?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let status = match row.status.as_str() {
                "running" => DiscoveredSessionStatus::Running {
                    panes: row.panes.unwrap_or(0),
                    clients: row.clients.unwrap_or(0),
                    has_layout: row.layout.unwrap_or(false),
                    created_from_profile: row.created_from_profile,
                },
                "busy" => DiscoveredSessionStatus::Busy,
                _ => DiscoveredSessionStatus::Unknown,
            };
            DiscoveredSession {
                name: row.name,
                status,
                ephemeral: row.ephemeral,
                host: host.clone(),
            }
        })
        .collect())
}

/// Serialize discovered sessions as JSON (for `list-sessions --format json`).
pub fn sessions_to_json(rows: &[DiscoveredSession]) -> Result<String, serde_json::Error> {
    let value: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let mut obj = serde_json::json!({
                "name": row.name,
                "ephemeral": row.ephemeral,
            });
            if let Some(host) = &row.host {
                obj["host"] = serde_json::json!(host);
            }
            match &row.status {
                DiscoveredSessionStatus::Running {
                    panes,
                    clients,
                    has_layout,
                    created_from_profile,
                } => {
                    obj["status"] = serde_json::json!("running");
                    obj["panes"] = serde_json::json!(panes);
                    obj["clients"] = serde_json::json!(clients);
                    obj["layout"] = serde_json::json!(has_layout);
                    if let Some(profile) = created_from_profile {
                        obj["created_from_profile"] = serde_json::json!(profile);
                    }
                }
                DiscoveredSessionStatus::Busy => obj["status"] = serde_json::json!("busy"),
                DiscoveredSessionStatus::Unknown => obj["status"] = serde_json::json!("unknown"),
            }
            obj
        })
        .collect();
    serde_json::to_string_pretty(&value)
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

    #[test]
    fn list_json_round_trips_through_parser() {
        let rows = vec![DiscoveredSession {
            name: "dev".into(),
            ephemeral: false,
            host: Some("workbox".into()),
            status: DiscoveredSessionStatus::Running {
                panes: 2,
                clients: 1,
                has_layout: true,
                created_from_profile: Some("work".into()),
            },
        }];
        let json = sessions_to_json(&rows).unwrap();
        let parsed = parse_remote_list_json(json.as_bytes(), Some("workbox".into())).unwrap();
        assert_eq!(parsed[0].name, "dev");
        assert_eq!(parsed[0].host.as_deref(), Some("workbox"));
        assert!(matches!(
            parsed[0].status,
            DiscoveredSessionStatus::Running { panes: 2, .. }
        ));
    }
}
