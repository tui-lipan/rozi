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
    /// Exact remote endpoint used for discovery. Kept separately from the short display label so
    /// user/port distinctions survive activation and retained-attachment lookup.
    pub remote_target: Option<crate::session::remote::RemoteTarget>,
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
        remote_target: None,
    })
}

/// Discover sessions from a local runtime dir or a remote host (one ssh round-trip).
pub fn discover_sessions_from(
    source: &SessionSource,
    config: &crate::config::RemoteConfig,
) -> std::io::Result<Vec<DiscoveredSession>> {
    match source {
        SessionSource::Local => discover_sessions(),
        SessionSource::Remote(target) => discover_remote_sessions(target, config),
    }
}

/// Why a host probe failed, in words a user can act on.
///
/// A failed probe carries whatever ssh, the remote shell, or the JSON parser said, which is a
/// paragraph of plumbing (`remote list-sessions failed: ssh: connect to host winvm port 22:
/// Connection refused`) written for a terminal, not for a sidebar column narrow enough that it
/// clips before reaching the part that matters. This maps the shapes that actually occur onto a
/// short phrase that names the thing to go fix. The raw message is kept in
/// [`crate::state::HostProbe::Failed`] rather than replaced, so nothing is lost for diagnosis.
///
/// Each phrase describes the state of the world rather than restating the syscall that reported it,
/// because the status badge beside it already says `Offline` and the only thing left worth saying is
/// *which* kind of offline. The two that are easy to confuse are kept apart deliberately:
///
/// - **Host not responding** — nothing answered at all: the machine is off, on another network, or
///   behind a firewall that drops. (`ETIMEDOUT`.)
/// - **SSH port closed** — something answered and said no, so the machine is up but nothing is
///   accepting SSH: `sshd` is stopped, the port is wrong, or a firewall rejects rather than drops.
///   (`ECONNREFUSED`, which is also what a stopped VM behind a published port forward gives.)
///
/// Ordered most-specific first: an authentication failure and a missing remote binary both mention
/// paths and files, so the distinctive phrases have to win before the generic ones are tried.
pub fn probe_failure_reason(error: &str) -> &'static str {
    let error = error.to_ascii_lowercase();
    let says = |needle: &str| error.contains(needle);

    if says("host key verification failed") || says("remote host identification has changed") {
        "Host key not trusted"
    } else if says("permission denied") || says("authentication failed") {
        "SSH login rejected"
    } else if says("could not resolve")
        || says("name or service not known")
        || says("nodename nor servname")
        || says("no address associated")
    {
        "Unknown host name"
    } else if says("connection refused") {
        "SSH port closed"
    } else if says("no route to host") || says("network is unreachable") {
        "Host unreachable"
    } else if says("timed out") {
        "Host not responding"
    } else if says("ssh was not found") {
        "ssh not installed here"
    // The remote shell could not run the binary. Checked after the ssh-level failures because
    // "no such file or directory" is also how a local spawn failure reads.
    } else if says("command not found")
        || says("is not recognized")
        || says("no such file or directory")
    {
        "No hyprmux on host"
    } else {
        "Connection failed"
    }
}

fn discover_remote_sessions(
    target: &crate::session::remote::RemoteTarget,
    config: &crate::config::RemoteConfig,
) -> std::io::Result<Vec<DiscoveredSession>> {
    use std::process::Stdio;

    crate::session::remote::validate_remote_target(target).map_err(std::io::Error::other)?;
    let resolved = crate::session::remote::ResolvedRemote::resolve(target, config);
    let host_label = target.display_label();
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
    crate::session::remote::validate_remote_executable_token(&remote_bin)
        .map_err(std::io::Error::other)?;
    // `ssh_base_command` applies `ConnectTimeout`: an unreachable configured host must fail fast
    // rather than stall the picker's recurring discovery sweep on a TCP connect.
    let mut command = crate::session::remote::ssh_base_command(&resolved, config);
    crate::session::remote::append_ssh_destination(&mut command, &resolved);
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
    let mut rows = parse_remote_list_json(&output.stdout, Some(host_label))?;
    for row in &mut rows {
        row.remote_target = Some(target.clone());
    }
    Ok(rows)
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
                remote_target: None,
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

    /// Real messages, verbatim from ssh/OpenSSH and from this module, mapped onto the phrase the
    /// sidebar shows. The point of the test is the shapes, not the mapping: each of these is what
    /// the user actually hits, and each names a different thing to go fix.
    #[test]
    fn probe_failures_name_what_to_go_fix_rather_than_quoting_ssh() {
        let reason = |raw: &str| probe_failure_reason(raw);
        let remote = |stderr: &str| format!("remote list-sessions failed: {stderr}");

        assert_eq!(
            reason(&remote(
                "ssh: Could not resolve hostname winvm: Name or service not known"
            )),
            "Unknown host name"
        );
        // The two offline kinds must stay distinguishable: refused means the machine answered and
        // SSH did not, timed out means nothing answered at all. They point at different fixes.
        assert_eq!(
            reason(&remote(
                "ssh: connect to host winvm port 22: Connection refused"
            )),
            "SSH port closed"
        );
        assert_eq!(
            reason(&remote(
                "ssh: connect to host winvm port 22: Connection timed out"
            )),
            "Host not responding"
        );
        // macOS spells the same timeout differently.
        assert_eq!(
            reason(&remote(
                "ssh: connect to host winvm port 22: Operation timed out"
            )),
            "Host not responding"
        );
        assert_eq!(
            reason(&remote(
                "ssh: connect to host winvm port 22: No route to host"
            )),
            "Host unreachable"
        );
        assert_eq!(
            reason(&remote("winvm: Permission denied (publickey,password).")),
            "SSH login rejected"
        );
        assert_eq!(
            reason(&remote("Host key verification failed.")),
            "Host key not trusted"
        );
        // The host answered; the remote shell could not run hyprmux.
        assert_eq!(
            reason(&remote("bash: line 1: hyprmux: command not found")),
            "No hyprmux on host"
        );
        assert_eq!(
            reason(&remote(
                "'hyprmux' is not recognized as an internal or external command"
            )),
            "No hyprmux on host"
        );
        assert_eq!(
            reason("ssh was not found on PATH"),
            "ssh not installed here"
        );

        // Anything unrecognized still says something, and never leaks the raw text.
        assert_eq!(
            reason(&remote("kex_exchange_identification: read: Broken pipe")),
            "Connection failed"
        );
        assert_eq!(reason(""), "Connection failed");

        // An authentication failure mentions no file, but a missing binary does; the specific
        // match has to win so the two do not collapse onto one phrase.
        assert_eq!(
            reason(&remote(
                "Permission denied (publickey).\nbash: hyprmux: No such file or directory"
            )),
            "SSH login rejected"
        );
    }

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
            remote_target: None,
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
