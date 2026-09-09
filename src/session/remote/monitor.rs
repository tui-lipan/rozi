//! Host metadata protocol. It never attaches to a session or requests terminal output.
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{RemoteTarget, ResolvedRemote};
use crate::platform::ipc::IpcConnection;
use crate::session::discovery::DiscoveredSession;

const VERSION: u32 = 1;
const MAX_MESSAGE: usize = 1024 * 1024;
const INTERVAL: Duration = Duration::from_secs(2);
const DEADLINE: Duration = Duration::from_secs(10);
/// What both ends must have for a host to be supervised at all. A peer missing one of these is
/// refused with a reason rather than half-supervised: a monitor that cannot list sessions or prove
/// the channel is alive has nothing left to do.
const REQUIRED_CAPABILITIES: &[&str] = &["session-list", "health-check"];
/// Everything this build advertises: the required set, plus the optional work it will do when the
/// peer asks for it. An optional name the peer has never heard of simply goes unexercised, which
/// is the point of negotiating a set rather than pinning both ends to one version.
const CAPABILITIES: &[&str] = &[
    "session-list",
    "health-check",
    crate::session::protocol::AGENT_SUMMARIES,
];

#[derive(Debug, Serialize, Deserialize)]
struct Hello {
    protocol_min: u32,
    protocol_max: u32,
    #[serde(default)]
    capabilities: Vec<String>,
}

impl Hello {
    fn current() -> Self {
        Self {
            protocol_min: VERSION,
            protocol_max: VERSION,
            capabilities: CAPABILITIES.iter().copied().map(str::to_owned).collect(),
        }
    }

    fn validate(&self) -> io::Result<()> {
        crate::session::protocol::negotiate_protocol(
            VERSION,
            VERSION,
            self.protocol_max,
            self.protocol_min,
        )
        .map_err(|err| io::Error::new(io::ErrorKind::Unsupported, err.message()))?;
        for capability in REQUIRED_CAPABILITIES {
            if !self.capabilities.iter().any(|item| item == capability) {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("Host monitoring requires {capability}; update remote Rozi"),
                ));
            }
        }
        Ok(())
    }

    fn supports(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|item| item == capability)
    }
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    sessions: serde_json::Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    agents: Vec<crate::session::protocol::AgentSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HostSnapshot {
    pub rows: Vec<DiscoveredSession>,
    pub agents: Vec<crate::session::protocol::AgentSummary>,
}

fn write_message(writer: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    if bytes.len() > MAX_MESSAGE {
        return Err(io::Error::other("host metadata exceeds size limit"));
    }
    writer.write_all(&(bytes.len() as u32).to_be_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()
}

fn read_message<T: serde::de::DeserializeOwned>(reader: &mut impl Read) -> io::Result<T> {
    let mut length = [0; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_MESSAGE {
        return Err(io::Error::other("host metadata exceeds size limit"));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

/// Serve one SSH stdio channel until the client closes it. Each request is also a health check.
pub(crate) fn serve(reader: &mut impl Read, writer: &mut impl Write) -> io::Result<()> {
    let hello: Hello = read_message(reader)?;
    hello.validate()?;
    // Optional, so an older client that cannot deserialize summaries is served without them
    // rather than refused. Building them means one query per running session on this host, which
    // is work worth skipping for a peer that would only discard it.
    let include_agents = hello.supports(crate::session::protocol::AGENT_SUMMARIES);
    write_message(writer, &Hello::current())?;
    loop {
        let mut request = [0];
        if reader.read(&mut request)? == 0 {
            return Ok(());
        }
        if request[0] != b'?' {
            return Err(io::Error::other("unknown host metadata request"));
        }
        let (rows, agents) = if include_agents {
            crate::session::discovery::discover_sessions_with_agents()?
        } else {
            (
                crate::session::discovery::discover_sessions_with_snapshots()?,
                Vec::new(),
            )
        };
        let json = crate::session::discovery::sessions_to_json(&rows).map_err(io::Error::other)?;
        write_message(
            writer,
            &Snapshot {
                agents,
                sessions: serde_json::from_str(&json).map_err(io::Error::other)?,
            },
        )?;
    }
}

/// Client-owned cancellation. Dropping it interrupts reads and retry waits, without touching the
/// shared SSH master or any session server.
pub(crate) struct Monitor {
    pub target: RemoteTarget,
    pub generation: u64,
    stop: mpsc::Sender<()>,
    connection: Arc<Mutex<Option<IpcConnection>>>,
}

#[cfg(test)]
impl Monitor {
    pub(crate) fn dormant(target: RemoteTarget, generation: u64) -> Self {
        let (stop, _) = mpsc::channel();
        Self {
            target,
            generation,
            stop,
            connection: Arc::new(Mutex::new(None)),
        }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(connection) = self.connection.lock().unwrap().as_ref() {
            let _ = connection.shutdown(std::net::Shutdown::Both);
        }
    }
}

fn connect(
    target: &RemoteTarget,
    config: &crate::config::RemoteConfig,
) -> io::Result<(IpcConnection, std::thread::JoinHandle<String>)> {
    super::validate_remote_target(target).map_err(io::Error::other)?;
    let resolved = ResolvedRemote::resolve(target, config);
    let binary = resolved.binary_path.as_deref().unwrap_or("rozi");
    super::validate_remote_executable_token(binary).map_err(io::Error::other)?;
    let mut config = config.clone();
    config.batch_mode = true;
    let mut command = super::ssh_base_command(&resolved, &config);
    super::append_ssh_destination(&mut command, &resolved);
    command
        .args([binary, "sessions", "watch"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn()?;
    let stderr = super::connect::spawn_stderr_collector(child.stderr.take().expect("piped stderr"));
    let connection = crate::platform::ipc::connection_from_child(child)?;
    connection.set_read_timeout(Some(DEADLINE))?;
    Ok((connection, stderr))
}

fn snapshot(connection: &mut IpcConnection, target: &RemoteTarget) -> io::Result<HostSnapshot> {
    connection.write_all(b"?")?;
    connection.flush()?;
    let snapshot: Snapshot = read_message(connection)?;
    let bytes = serde_json::to_vec(&snapshot.sessions).map_err(io::Error::other)?;
    let mut rows =
        crate::session::discovery::parse_remote_list_json(&bytes, Some(target.display_label()))?;
    rows.retain(|row| !row.ephemeral);
    for row in &mut rows {
        row.remote_target = Some(target.clone());
    }
    Ok(HostSnapshot {
        rows,
        agents: snapshot.agents,
    })
}

fn retry_delay(attempt: u32) -> Duration {
    Duration::from_millis((500_u64 << attempt.min(6)).min(30_000))
}

pub(crate) fn start(
    target: RemoteTarget,
    generation: u64,
    config: crate::config::RemoteConfig,
    report: impl Fn(Result<HostSnapshot, String>) + Send + 'static,
) -> Monitor {
    let (stop, stopped) = mpsc::channel();
    let shared = Arc::new(Mutex::new(None));
    let monitor = Monitor {
        target: target.clone(),
        generation,
        stop,
        connection: shared.clone(),
    };
    std::thread::spawn(move || {
        let mut attempt = 0;
        loop {
            if stopped.try_recv() != Err(mpsc::TryRecvError::Empty) {
                break;
            }
            let result = watch(&target, &config, &shared, &stopped, &report, &mut attempt);
            shared.lock().unwrap().take();
            let Err(error) = result else { break };
            let unsupported = matches!(
                error.kind(),
                io::ErrorKind::Unsupported | io::ErrorKind::PermissionDenied
            );
            report(Err(format!(
                "{}; Enter or Ctrl+R reconnects with SSH authentication",
                error
            )));
            if unsupported {
                break;
            }
            if stopped.recv_timeout(retry_delay(attempt)) != Err(mpsc::RecvTimeoutError::Timeout) {
                break;
            }
            attempt = attempt.saturating_add(1);
        }
    });
    monitor
}

fn watch(
    target: &RemoteTarget,
    config: &crate::config::RemoteConfig,
    shared: &Mutex<Option<IpcConnection>>,
    stopped: &mpsc::Receiver<()>,
    report: &impl Fn(Result<HostSnapshot, String>),
    attempt: &mut u32,
) -> io::Result<()> {
    let (mut connection, stderr) = connect(target, config)?;
    *shared.lock().unwrap() = Some(connection.try_clone()?);
    if stopped.try_recv() != Err(mpsc::TryRecvError::Empty) {
        return Ok(());
    }
    if let Err(error) = handshake(&mut connection) {
        let _ = connection.shutdown(std::net::Shutdown::Both);
        let detail = stderr.join().unwrap_or_default();
        return Err(handshake_error(error, &detail));
    }
    let mut previous = None;
    loop {
        let rows = snapshot(&mut connection, target)?;
        *attempt = 0;
        if previous.as_ref() != Some(&rows) {
            report(Ok(rows.clone()));
            previous = Some(rows);
        }
        if stopped.recv_timeout(INTERVAL) != Err(mpsc::RecvTimeoutError::Timeout) {
            return Ok(());
        }
    }
}

fn handshake(connection: &mut (impl Read + Write)) -> io::Result<()> {
    write_message(connection, &Hello::current())?;
    let hello: Hello = read_message(connection)?;
    hello.validate()
}

fn handshake_error(error: io::Error, detail: &str) -> io::Error {
    let kind =
        if detail.contains("unknown sessions command") || detail.contains("unexpected argument") {
            io::ErrorKind::Unsupported
        } else if detail.contains("Permission denied")
            || detail.contains("Host key verification failed")
        {
            io::ErrorKind::PermissionDenied
        } else {
            error.kind()
        };
    let detail = detail.trim();
    io::Error::new(
        kind,
        format!("Host metadata connection failed: {error}. {detail}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_are_additive_and_required_features_are_checked() {
        let mut hello = Hello::current();
        hello.capabilities.push("future-feature".into());
        hello.validate().unwrap();
        hello.capabilities.retain(|value| value != "health-check");
        assert_eq!(
            hello.validate().unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
    }

    /// Agent summaries are the optional half of the set. Dropping one must cost the feature and
    /// nothing else — a client that predates them still gets a supervised host, just a quieter one.
    #[test]
    fn an_optional_capability_degrades_instead_of_refusing_the_connection() {
        let mut hello = Hello::current();
        hello
            .capabilities
            .retain(|value| value != crate::session::protocol::AGENT_SUMMARIES);
        hello.validate().unwrap();
        assert!(!hello.supports(crate::session::protocol::AGENT_SUMMARIES));

        let mut request = Vec::new();
        write_message(&mut request, &hello).unwrap();
        request.push(b'?');
        let mut response = Vec::new();
        serve(&mut request.as_slice(), &mut response).unwrap();
        let mut response = response.as_slice();
        read_message::<Hello>(&mut response)
            .unwrap()
            .validate()
            .unwrap();
        let snapshot: Snapshot = read_message(&mut response).unwrap();
        assert!(snapshot.sessions.is_array());
        assert!(snapshot.agents.is_empty());
    }

    /// The snapshot's agent list is a late addition to a message older builds already send, so it
    /// has to be absent from the wire when empty and readable when a peer omits it entirely.
    #[test]
    fn snapshot_agents_are_omitted_when_empty_and_optional_when_read() {
        let empty = serde_json::to_value(Snapshot {
            sessions: serde_json::json!([]),
            agents: Vec::new(),
        })
        .unwrap();
        assert_eq!(empty, serde_json::json!({ "sessions": [] }));

        let decoded: Snapshot =
            serde_json::from_value(serde_json::json!({ "sessions": [] })).unwrap();
        assert!(decoded.agents.is_empty());
    }

    #[test]
    fn protocol_skew_and_oversized_metadata_are_rejected() {
        let mut hello = Hello::current();
        hello.protocol_min = VERSION + 1;
        hello.protocol_max = VERSION + 1;
        assert!(hello.validate().is_err());
        let oversized = (MAX_MESSAGE as u32 + 1).to_be_bytes();
        assert!(read_message::<Hello>(&mut oversized.as_slice()).is_err());
    }

    #[test]
    fn server_answers_health_requests_without_an_attachment() {
        let mut request = Vec::new();
        write_message(&mut request, &Hello::current()).unwrap();
        request.extend_from_slice(b"??");
        let mut response = Vec::new();
        serve(&mut request.as_slice(), &mut response).unwrap();
        let mut response = response.as_slice();
        read_message::<Hello>(&mut response)
            .unwrap()
            .validate()
            .unwrap();
        for _ in 0..2 {
            let snapshot: Snapshot = read_message(&mut response).unwrap();
            assert!(snapshot.sessions.is_array());
        }
        assert!(response.is_empty());
    }

    #[test]
    fn authentication_and_old_command_errors_require_attention() {
        for (detail, kind) in [
            (
                "unknown sessions command `watch`",
                io::ErrorKind::Unsupported,
            ),
            (
                "Permission denied (publickey)",
                io::ErrorKind::PermissionDenied,
            ),
            (
                "Host key verification failed",
                io::ErrorKind::PermissionDenied,
            ),
            ("connection reset", io::ErrorKind::UnexpectedEof),
        ] {
            assert_eq!(
                handshake_error(io::Error::from(io::ErrorKind::UnexpectedEof), detail).kind(),
                kind
            );
        }
    }

    #[test]
    fn dropping_monitor_interrupts_a_silent_channel_and_retry_wait() {
        let (_input, writer) = std::io::pipe().unwrap();
        let (reader, _output) = std::io::pipe().unwrap();
        let connection = IpcConnection::from_piped(
            crate::platform::ipc::PipedConnection::from_reader_writer(writer, reader),
        );
        connection
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reading = connection.try_clone().unwrap();
        let (stop, stopped) = mpsc::channel();
        let monitor = Monitor {
            target: RemoteTarget::Alias("test.invalid".into()),
            generation: 1,
            stop,
            connection: Arc::new(Mutex::new(Some(connection))),
        };
        let blocked = std::thread::spawn(move || {
            let mut byte = [0];
            reading.read(&mut byte)
        });
        drop(monitor);
        stopped.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(blocked.join().unwrap().unwrap(), 0);
    }

    #[test]
    fn retry_is_bounded() {
        assert_eq!(retry_delay(0), Duration::from_millis(500));
        assert_eq!(retry_delay(1), Duration::from_secs(1));
        assert_eq!(retry_delay(u32::MAX), Duration::from_secs(30));
    }
}
