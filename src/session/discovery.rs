use std::collections::BTreeSet;
use std::time::Duration;

use crate::platform::ipc::{EndpointRegistry, IpcConnection, IpcEndpoint};
use crate::session::protocol::{
    ClientMessage, MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION, ServerMessage,
};

const QUERY_TIMEOUT: Duration = Duration::from_millis(60);
/// How long a pane looking for its own server waits for each one to answer. Discovery can afford
/// to miss a busy server for one sweep; this cannot, because the server that misses the window may
/// be the pane's own, and its report is then lost. Every server is asked at once, so this bounds
/// the whole lookup, which has to leave room in a hook's budget for the report itself.
const INSTANCE_QUERY_TIMEOUT: Duration = Duration::from_millis(400);
/// How long a command about one named session waits for it to answer.
const SINGLE_SESSION_QUERY_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscoveredSessionStatus {
    Running {
        panes: usize,
        clients: u32,
        has_layout: bool,
        effective_protocol: Option<u32>,
        rozi_version: Option<String>,
    },
    /// No server is running, but a resurrection snapshot can seed one under this name.
    Restorable,
    /// No server is running, and the snapshot saved under this name is one this build cannot
    /// restore, such as one in another build's snapshot format. It is kept for a build that can;
    /// starting the session here is refused rather than started empty over it.
    Unreadable,
    /// Remembered from the last successful probe of a host this client is not connected to.
    ///
    /// Nothing here was confirmed by a live handshake: the pane count is what the host reported the
    /// last time it answered, and the session may since have been killed. Kept distinct from
    /// [`Self::Running`] precisely so no surface can quietly present a memory as a live session —
    /// a host's known workplaces stay listed while it is offline, but they must say that they are.
    LastSeen {
        panes: usize,
    },
    Busy,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredSession {
    pub name: String,
    pub status: DiscoveredSessionStatus,
    pub origin: crate::session::origin::SessionOrigin,
    /// Auto-managed per-process session (`eph-*`), disposable and not user-named.
    pub ephemeral: bool,
    /// Remote host alias/URL when discovered over `--remote`; `None` for local.
    pub host: Option<String>,
    /// Exact remote endpoint used for discovery. Kept separately from the short display label so
    /// user/port distinctions survive activation and retained-attachment lookup.
    pub remote_target: Option<crate::session::remote::RemoteTarget>,
}

impl DiscoveredSession {
    pub fn effective_protocol(&self) -> Option<u32> {
        match &self.status {
            DiscoveredSessionStatus::Running {
                effective_protocol, ..
            } => *effective_protocol,
            _ => None,
        }
    }

    pub fn rozi_version(&self) -> Option<&str> {
        match &self.status {
            DiscoveredSessionStatus::Running { rozi_version, .. } => rozi_version.as_deref(),
            _ => None,
        }
    }
}

/// One local metadata sweep. Session rows and agent summaries must travel together so an Agents
/// view selection can attach to the same session the summary described.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalAgentSnapshot {
    pub sessions: Vec<DiscoveredSession>,
    pub agents: Vec<crate::session::protocol::AgentSummary>,
    /// The checkouts those sessions work in, for the Projects tab.
    pub projects: Vec<crate::session::protocol::ProjectSummary>,
}

/// Every live session on this host, with what its agent and project summaries say, from one
/// probe each.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct HostMetadata {
    pub sessions: Vec<DiscoveredSession>,
    pub agents: Vec<crate::session::protocol::AgentSummary>,
    pub projects: Vec<crate::session::protocol::ProjectSummary>,
}

/// A busy endpoint gave no semantic answer this poll. Keep its last agent summaries until it
/// answers again; a missing endpoint (which has no row) still removes its agents normally.
pub(crate) fn retain_busy_agent_summaries(
    rows: &[DiscoveredSession],
    agents: &mut Vec<crate::session::protocol::AgentSummary>,
    previous: &[crate::session::protocol::AgentSummary],
) {
    let busy_sessions: BTreeSet<_> = rows
        .iter()
        .filter(|row| matches!(row.status, DiscoveredSessionStatus::Busy))
        .map(|row| row.name.as_str())
        .collect();
    let retained: Vec<_> = previous
        .iter()
        .filter(|agent| {
            busy_sessions.contains(agent.session.as_str())
                && !agents.iter().any(|current| current.same_agent(agent))
        })
        .cloned()
        .collect();
    agents.extend(retained);
}

/// [`retain_busy_agent_summaries`] for project summaries: a busy session keeps the checkouts it
/// last reported until it answers again.
pub(crate) fn retain_busy_project_summaries(
    rows: &[DiscoveredSession],
    projects: &mut Vec<crate::session::protocol::ProjectSummary>,
    previous: &[crate::session::protocol::ProjectSummary],
) {
    let busy_sessions: BTreeSet<_> = rows
        .iter()
        .filter(|row| matches!(row.status, DiscoveredSessionStatus::Busy))
        .map(|row| row.name.as_str())
        .collect();
    let answered: BTreeSet<_> = projects
        .iter()
        .map(|project| project.session.clone())
        .collect();
    projects.extend(
        previous
            .iter()
            .filter(|project| {
                busy_sessions.contains(project.session.as_str())
                    && !answered.contains(&project.session)
            })
            .cloned(),
    );
}

/// Where to discover sessions from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionSource {
    Local,
    Remote(crate::session::remote::RemoteTarget),
}

/// The longest session name that can be bound as an endpoint.
///
/// A session's name is not just a label: it is spelled into its endpoint's file name, and on Unix
/// that path has a hard limit (see [`crate::platform::ipc::MAX_ENDPOINT_PATH_LEN`]). Unbounded, a
/// long enough name produced a session that could not be served at all, reported as whatever the
/// bind failed with - so the bound is stated here, where a name is admitted, rather than discovered
/// at the socket. 64 is far past any name a person types and still leaves the runtime directory
/// room to move (see [`crate::platform::paths::fallback_runtime_dir_path`]).
pub const MAX_SESSION_NAME_LEN: usize = 64;

/// Whether `name` is a well-formed *attach target*. Like [`valid_session_name`] but permits the
/// reserved `eph-` prefix: attaching to an already-running ephemeral session (our own, shown as the
/// current row) is legitimate even though users may not *create* ephemeral names.
pub fn valid_attach_target(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SESSION_NAME_LEN
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

/// Whether `name` is a valid *user-created* session name. Rejects the reserved ephemeral prefix so
/// create/rename never mints an `eph-…` name that would collide with an auto-managed session.
pub fn valid_session_name(name: &str) -> bool {
    valid_attach_target(name) && !crate::session::naming::is_ephemeral_session_name(name)
}

/// Whether `rozi update --rollback` can reach the build a session was started with: only an
/// install the install scripts manage keeps the previous version to go back to.
fn rollback_available() -> bool {
    crate::platform::install_source::detect_current()
        == crate::platform::install_source::InstallSource::Managed
}

/// What to do about local session `name` when its server runs a build this one cannot attach to,
/// phrased to follow a semicolon: finish its work on the build that started it where a rollback can
/// reach that build, then restart it, which reopens it from its snapshot but ends its programs.
pub(crate) fn incompatible_session_advice(name: &str) -> String {
    let restart = format!(
        "`rozi sessions restart {name}` reopens it from its last snapshot and ends its programs"
    );
    if rollback_available() {
        format!("to finish its work first, run `rozi update --rollback` and attach; then {restart}")
    } else {
        restart
    }
}

/// The toast hint for a session row whose server runs a build this one cannot attach to. A remote
/// row's server runs the host's install, so neither this machine's rollback nor its restart
/// behavior applies to it.
pub(crate) fn incompatible_session_hint(remote_host: Option<&str>) -> String {
    match remote_host {
        Some(host) => format!("roll back rozi on `{host}` to finish its work · Ctrl+E restarts it"),
        None if rollback_available() => {
            "`rozi update --rollback` to finish its work · Ctrl+E restarts it from its snapshot"
                .to_string()
        }
        None => "Ctrl+E restarts it from its snapshot".to_string(),
    }
}

pub fn discover_sessions() -> std::io::Result<Vec<DiscoveredSession>> {
    discover_sessions_excluding(None)
}

/// Discover every live local session plus any named session that can be resurrected from disk.
/// Live rows win when a server and its latest snapshot coexist.
pub(crate) fn discover_sessions_with_snapshots() -> std::io::Result<Vec<DiscoveredSession>> {
    let mut rows = discover_sessions()?;
    push_restorable_sessions(
        &mut rows,
        None,
        crate::session::server::list_snapshot_summaries_by_recency(),
    );
    Ok(rows)
}

#[derive(Default)]
pub(crate) struct SessionDiagnosticCounts {
    pub running: usize,
    pub incompatible: usize,
    pub restorable: usize,
    pub unknown: usize,
    pub running_versions: Vec<(Option<String>, Option<u32>)>,
}

#[derive(Clone)]
enum DiagnosticSessionStatus {
    Running {
        rozi_version: Option<String>,
        effective_protocol: Option<u32>,
    },
    Incompatible,
    Unknown,
}

/// Read-only diagnostics: use the same query and snapshot discovery as session listing,
/// without creating the runtime directory or retiring stale endpoints.
pub(crate) fn diagnostic_counts() -> std::io::Result<SessionDiagnosticCounts> {
    let env = crate::platform::paths::PlatformEnv::from_process();
    let dir = crate::platform::paths::runtime_dir_path(&env);
    let handles: Vec<_> = EndpointRegistry::list_session_endpoints(&dir)?
        .into_iter()
        .filter(|(name, _)| !crate::scratchpad::runtime::is_client_scratch_session(name))
        .map(|(name, endpoint)| {
            std::thread::spawn(move || {
                diagnostic_session_status(&name, &endpoint).map(|status| (name, status))
            })
        })
        .collect();
    let mut counts = SessionDiagnosticCounts::default();
    let mut live_names = std::collections::HashSet::new();
    for handle in handles {
        let Ok(Some((name, status))) = handle.join() else {
            continue;
        };
        live_names.insert(name);
        match status {
            DiagnosticSessionStatus::Running {
                rozi_version,
                effective_protocol,
            } => {
                counts.running += 1;
                counts
                    .running_versions
                    .push((rozi_version, effective_protocol));
            }
            DiagnosticSessionStatus::Incompatible => counts.incompatible += 1,
            DiagnosticSessionStatus::Unknown => counts.unknown += 1,
        }
    }
    counts.running_versions.sort();
    counts.restorable = crate::session::server::list_snapshot_summaries_by_recency()
        .iter()
        .filter(|snapshot| !live_names.contains(&snapshot.session))
        .count();
    Ok(counts)
}

/// Classify a live endpoint. Stale or retiring endpoints have no count.
fn diagnostic_session_status(
    name: &str,
    endpoint: &IpcEndpoint,
) -> Option<DiagnosticSessionStatus> {
    let mut stream = match endpoint.connect() {
        Ok(stream) => stream,
        Err(err) if connect_error_proves_stale(err.kind()) => return None,
        Err(_) => return Some(DiagnosticSessionStatus::Unknown),
    };
    match query_status(name, &mut stream, None) {
        Ok(answer) if answer.refusal.as_deref() == Some("protocol-mismatch") => {
            Some(DiagnosticSessionStatus::Incompatible)
        }
        Ok(QueryAnswer {
            status:
                DiscoveredSessionStatus::Running {
                    rozi_version,
                    effective_protocol,
                    ..
                },
            ..
        }) => Some(DiagnosticSessionStatus::Running {
            rozi_version,
            effective_protocol,
        }),
        Err(err) if peer_hung_up(err.kind()) => None,
        _ => Some(DiagnosticSessionStatus::Unknown),
    }
}

/// Probe one named session without scanning or mutating unrelated discovery entries.
pub fn discover_session(name: &str) -> std::io::Result<Option<DiscoveredSession>> {
    let endpoint = crate::session::server::session_endpoint(name)?;
    Ok(query_session_endpoint(name, &endpoint))
}

/// How a session's release compares with this build's, by semantic version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReleaseOrder {
    /// The session runs an older release: upgrading moves it onto this build.
    Older,
    Same,
    /// The session runs a newer release. Restarting it on this build would downgrade it, and
    /// this build may not read the snapshot that release wrote.
    Newer,
}

/// Compare a session's reported release with this build. `None` when either is not a version.
pub(crate) fn release_order(session_version: &str) -> Option<ReleaseOrder> {
    let session = semver::Version::parse(session_version).ok()?;
    let this = semver::Version::parse(env!("CARGO_PKG_VERSION")).ok()?;
    Some(match session.cmp(&this) {
        std::cmp::Ordering::Less => ReleaseOrder::Older,
        std::cmp::Ordering::Equal => ReleaseOrder::Same,
        std::cmp::Ordering::Greater => ReleaseOrder::Newer,
    })
}

/// What one named session says about the release it runs, asked once with time to answer. A
/// discovery sweep gives up on a slow server quickly and lists it busy; a command about this one
/// session can wait.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SessionRelease {
    /// It answered; the release is `None` only from a server that predates reporting it.
    Running(Option<String>),
    /// It refused this build's generation. `None` from a server before generation 52, which never
    /// said who it was and is therefore older than any build that asks.
    Incompatible(Option<crate::session::protocol::PeerRelease>),
    /// Nothing accepted the connection: no server runs under the name.
    NotRunning,
    /// A server accepted the connection but gave no answer this build could read: it timed out,
    /// sent something malformed, or refused for another reason.
    Unknown,
}

/// Whether an upgrade may stop a session's server and replace it with this build.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum UpgradeDisposition {
    /// The server is known to be older than this build, or this build exactly, or not running.
    Replace,
    /// The server runs this newer release or generation; replacing it would downgrade it.
    RefuseNewer(String),
    /// Nothing proves the server is safe to replace.
    RefuseUnverified,
}

pub(crate) fn session_release(name: &str) -> SessionRelease {
    session_release_and_server(name).0
}

/// [`session_release`], with the process id of the server that answered, so a caller about to act
/// on the answer can make sure it acts on that same server.
pub(crate) fn session_release_and_server(name: &str) -> (SessionRelease, Option<u32>) {
    let Some(mut stream) = crate::session::server::session_endpoint(name)
        .ok()
        .and_then(|endpoint| endpoint.connect().ok())
    else {
        return (SessionRelease::NotRunning, None);
    };
    let server = stream.peer_pid();
    let release = match query_status_within(name, &mut stream, None, SINGLE_SESSION_QUERY_TIMEOUT) {
        Ok(QueryAnswer {
            status: DiscoveredSessionStatus::Running { rozi_version, .. },
            ..
        }) => SessionRelease::Running(rozi_version),
        Ok(answer)
            if answer.refusal.as_deref()
                == Some(crate::session::protocol::PROTOCOL_MISMATCH_ERROR_CODE) =>
        {
            SessionRelease::Incompatible(answer.refused_by)
        }
        _ => SessionRelease::Unknown,
    };
    (release, server)
}

impl SessionRelease {
    /// Whether an upgrade may replace this server. It fails closed: only positive evidence that the
    /// server is no newer than this build allows it.
    pub(crate) fn upgrade_disposition(&self) -> UpgradeDisposition {
        use crate::session::protocol::{MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION};
        match self {
            Self::NotRunning => UpgradeDisposition::Replace,
            Self::Running(Some(version)) => match release_order(version) {
                Some(ReleaseOrder::Older | ReleaseOrder::Same) => UpgradeDisposition::Replace,
                Some(ReleaseOrder::Newer) => UpgradeDisposition::RefuseNewer(version.clone()),
                None => UpgradeDisposition::RefuseUnverified,
            },
            // Only a server before generation 52 refuses without saying who it is.
            Self::Incompatible(None) => UpgradeDisposition::Replace,
            Self::Incompatible(Some(peer)) => {
                let order = release_order(&peer.rozi_version);
                if order == Some(ReleaseOrder::Newer) || peer.protocol_min > PROTOCOL_VERSION {
                    UpgradeDisposition::RefuseNewer(peer.rozi_version.clone())
                } else if order == Some(ReleaseOrder::Older)
                    && peer.protocol_min <= peer.protocol_max
                    && peer.protocol_max < MIN_SUPPORTED_PROTOCOL
                {
                    UpgradeDisposition::Replace
                } else {
                    UpgradeDisposition::RefuseUnverified
                }
            }
            Self::Running(None) | Self::Unknown => UpgradeDisposition::RefuseUnverified,
        }
    }
}

pub fn discover_sessions_excluding(
    exclude_name: Option<&str>,
) -> std::io::Result<Vec<DiscoveredSession>> {
    Ok(sweep_sessions(exclude_name, false)?
        .into_iter()
        .map(|probed| probed.row)
        .collect())
}

/// What one endpoint answered. `agents` is empty unless the sweep asked for summaries and the
/// server agreed to send them.
pub(crate) struct ProbedSession {
    pub row: DiscoveredSession,
    pub agents: Vec<crate::session::protocol::AgentSummary>,
    pub projects: Vec<crate::session::protocol::ProjectSummary>,
}

/// Every live local session plus the agents inside them, in one probe each.
///
/// The host monitor wants both halves of this on every poll, and asking for them separately would
/// connect to every session server on the machine twice. The agent half stays opt-in because the
/// picker and sidebar sweeps that share this path want none of it: building summaries walks every
/// pane of every session, and those callers only ever needed the pane count.
///
/// Ephemeral sessions are not asked. The monitor drops their rows on arrival, so a summary from
/// one would describe a session the client will never show.
pub(crate) fn discover_host_metadata() -> std::io::Result<HostMetadata> {
    let mut metadata = HostMetadata::default();
    for mut probed in sweep_sessions(None, true)? {
        metadata.sessions.push(probed.row);
        metadata.agents.append(&mut probed.agents);
        metadata.projects.append(&mut probed.projects);
    }
    push_restorable_sessions(
        &mut metadata.sessions,
        None,
        crate::session::server::list_snapshot_summaries_by_recency(),
    );
    Ok(metadata)
}

fn sweep_sessions(
    exclude_name: Option<&str>,
    with_agents: bool,
) -> std::io::Result<Vec<ProbedSession>> {
    let dir = crate::control::runtime_dir()?;
    let mut endpoints = EndpointRegistry::list_session_endpoints(&dir)?;
    endpoints.retain(|(name, _)| !crate::scratchpad::runtime::is_client_scratch_session(name));
    if let Some(exclude_name) = exclude_name {
        endpoints.retain(|(name, _)| name != exclude_name);
    }

    let mut handles = Vec::with_capacity(endpoints.len());
    for (name, endpoint) in endpoints {
        let capabilities = (with_agents
            && !crate::session::naming::is_ephemeral_session_name(&name))
        .then(crate::session::protocol::Capabilities::current);
        handles.push(std::thread::spawn(move || {
            probe_session_endpoint(&name, &endpoint, capabilities.as_ref())
        }));
    }

    let mut probed = Vec::with_capacity(handles.len());
    for handle in handles {
        if let Ok(Some(session)) = handle.join() {
            probed.push(session);
        }
    }
    probed.sort_by(|a, b| a.row.name.cmp(&b.row.name));
    Ok(probed)
}

/// Session-picker/sidebar discovery policy: omit the current session while probing and hide
/// foreign disposable sessions. The caller adds its locally known current row afterwards.
pub(crate) fn discover_selectable_sessions(
    current_name: Option<&str>,
) -> std::io::Result<Vec<DiscoveredSession>> {
    let mut rows = discover_sessions_excluding(current_name)?;
    rows.retain(|entry| !entry.ephemeral);
    push_restorable_sessions(
        &mut rows,
        current_name,
        crate::session::server::list_snapshot_summaries_by_recency(),
    );
    Ok(rows)
}

fn push_restorable_sessions(
    rows: &mut Vec<DiscoveredSession>,
    current_name: Option<&str>,
    snapshots: impl IntoIterator<Item = crate::session::server::SnapshotSummary>,
) {
    for snapshot in snapshots {
        let name = snapshot.session;
        if current_name == Some(name.as_str()) || rows.iter().any(|row| row.name == name) {
            continue;
        }
        rows.push(DiscoveredSession {
            name,
            origin: snapshot.origin,
            status: if snapshot.readable {
                DiscoveredSessionStatus::Restorable
            } else {
                DiscoveredSessionStatus::Unreadable
            },
            ephemeral: false,
            host: None,
            remote_target: None,
        });
    }
}

/// Whether a failed handshake means the peer hung up rather than answered badly. A server that
/// is retiring closes mid-exchange while its endpoint is still bound - the listening socket keeps
/// accepting until the process exits - so this, not a second connect, is what tells a dying
/// server apart from a live one we cannot talk to.
fn peer_hung_up(kind: std::io::ErrorKind) -> bool {
    matches!(
        kind,
        std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::NotConnected
    )
}

/// What one session server said about itself.
struct QueryAnswer {
    status: DiscoveredSessionStatus,
    origin: crate::session::origin::SessionOrigin,
    agents: Vec<crate::session::protocol::AgentSummary>,
    projects: Vec<crate::session::protocol::ProjectSummary>,
    /// `None` from a server that predates reporting it, as well as from one we cannot speak to.
    instance: Option<crate::session::protocol::SessionInstanceId>,
    /// The error code a server answered with instead, such as `protocol-mismatch`.
    refusal: Option<String>,
    /// The refusing server's release and window, when it said.
    refused_by: Option<crate::session::protocol::PeerRelease>,
}

impl QueryAnswer {
    fn unknown() -> Self {
        Self::without_answer(DiscoveredSessionStatus::Unknown)
    }

    fn without_answer(status: DiscoveredSessionStatus) -> Self {
        Self {
            status,
            origin: Default::default(),
            agents: Vec::new(),
            projects: Vec::new(),
            instance: None,
            refusal: None,
            refused_by: None,
        }
    }
}

/// Ask one connected endpoint what it is. `Err` means the handshake never completed; a server that
/// answers with anything but `SessionInfo` speaks a protocol we cannot use and is `Unknown`.
///
/// `capabilities` is what this probe is willing to be sent. Passing `None` is the picker's cheap
/// path and also what a pre-capability server sees, since the field is omitted from the wire.
fn query_status(
    name: &str,
    stream: &mut IpcConnection,
    capabilities: Option<&crate::session::protocol::Capabilities>,
) -> std::io::Result<QueryAnswer> {
    query_status_within(name, stream, capabilities, QUERY_TIMEOUT)
}

fn query_status_within(
    name: &str,
    stream: &mut IpcConnection,
    capabilities: Option<&crate::session::protocol::Capabilities>,
    timeout: Duration,
) -> std::io::Result<QueryAnswer> {
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    crate::session::protocol::write_frame(
        stream,
        &ClientMessage::Query {
            capabilities: capabilities.cloned(),
            session: name.to_string(),
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: MIN_SUPPORTED_PROTOCOL,
        },
    )?;
    match crate::session::protocol::read_frame::<_, ServerMessage>(stream)? {
        ServerMessage::SessionInfo {
            panes,
            clients,
            has_layout,
            effective_protocol,
            rozi_version,
            origin,
            agents,
            projects,
            instance,
            ..
        } => Ok(QueryAnswer {
            status: DiscoveredSessionStatus::Running {
                panes,
                clients,
                has_layout,
                effective_protocol: Some(effective_protocol),
                rozi_version: rozi_version
                    .filter(|v| !v.is_empty() && !v.chars().any(char::is_control)),
            },
            origin,
            agents,
            projects,
            instance,
            refusal: None,
            refused_by: None,
        }),
        ServerMessage::Error { code, peer, .. } => Ok(QueryAnswer {
            refusal: Some(code),
            refused_by: peer,
            ..QueryAnswer::unknown()
        }),
        _ => Ok(QueryAnswer::unknown()),
    }
}

/// What looking for a pane's own server by instance found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstanceLookup {
    /// The session whose server answers to the instance.
    Found(String),
    /// No server here answers to it. `unidentified` names live servers that answered without an
    /// instance - an older rozi - any of which may be the one; they are never guessed at.
    /// `other_versions` names servers that refused to talk at all because their protocol differs,
    /// as one left running across an update does. Any of those may be the one too.
    NotFound {
        unidentified: Vec<String>,
        other_versions: Vec<String>,
    },
}

/// The local session whose server is `instance`, for a pane that has to reach its own server and
/// knows only that. Every live endpoint is asked in parallel, as a discovery sweep asks them.
///
/// A name is never guessed, because a report applied to the wrong session lands on a stranger's
/// pane with the same id. The session may have ended, the pane may not be on this machine, or its
/// server may predate reporting its instance.
pub fn session_with_instance(
    instance: &crate::session::protocol::SessionInstanceId,
) -> std::io::Result<InstanceLookup> {
    let dir = crate::control::runtime_dir()?;
    let handles: Vec<_> = EndpointRegistry::list_session_endpoints(&dir)?
        .into_iter()
        .map(|(name, endpoint)| {
            std::thread::spawn(move || {
                // A discovery sweep may be holding the only free pipe instance. Missing
                // this server drops the pane's report, so wait out that window instead
                // of treating one busy connect as "not here".
                let mut stream = connect_for_instance_query(&endpoint)?;
                let answer =
                    query_status_within(&name, &mut stream, None, INSTANCE_QUERY_TIMEOUT).ok()?;
                Some((name, answer))
            })
        })
        .collect();
    let mut unidentified = Vec::new();
    let mut other_versions = Vec::new();
    for (name, answer) in handles
        .into_iter()
        .filter_map(|handle| handle.join().ok().flatten())
    {
        if answer.refusal.as_deref() == Some(crate::session::protocol::PROTOCOL_MISMATCH_ERROR_CODE)
        {
            other_versions.push(name);
            continue;
        }
        if !matches!(answer.status, DiscoveredSessionStatus::Running { .. }) {
            continue;
        }
        match answer.instance {
            Some(answered) if &answered == instance => return Ok(InstanceLookup::Found(name)),
            Some(_) => {}
            None => unidentified.push(name),
        }
    }
    unidentified.sort();
    other_versions.sort();
    Ok(InstanceLookup::NotFound {
        unidentified,
        other_versions,
    })
}

/// Connect for [`session_with_instance`], retrying while the endpoint is busy.
///
/// The wait shares [`INSTANCE_QUERY_TIMEOUT`]: a pane looking for its own server can afford one
/// bounded delay and cannot afford to skip the server. Any other connect error still means the
/// endpoint is not usable.
fn connect_for_instance_query(endpoint: &IpcEndpoint) -> Option<IpcConnection> {
    let deadline = std::time::Instant::now() + INSTANCE_QUERY_TIMEOUT;
    loop {
        match endpoint.connect() {
            Ok(stream) => return Some(stream),
            Err(err)
                if crate::platform::ipc::is_busy_error(&err)
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    }
}

/// Sessions on this host with a worktree origin, from snapshots and live servers.
#[derive(Debug, Default)]
pub(crate) struct WorktreeOrigins {
    /// `(session, canonical checkout path)` for every session that records one.
    pub known: Vec<(String, std::path::PathBuf)>,
    /// Live sessions whose origin could not be read, such as a server speaking another protocol.
    pub unverified: Vec<UnverifiedSession>,
    /// Sessions in `known` whose server answered; the rest are restorable snapshots.
    pub live: BTreeSet<String>,
}

/// A live session whose origin could not be read, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnverifiedSession {
    pub name: String,
    /// What went wrong, phrased to follow the session's name.
    pub reason: String,
}

impl UnverifiedSession {
    /// Why a server that answered the status query still gave no origin.
    fn from_answer(name: String, answer: &QueryAnswer) -> Self {
        let reason = match answer.refusal.as_deref() {
            Some(crate::session::protocol::PROTOCOL_MISMATCH_ERROR_CODE) => {
                "runs a different rozi version".to_string()
            }
            Some(code) => format!("refused the query ({code})"),
            None => "answered without its status".to_string(),
        };
        Self { name, reason }
    }
}

impl WorktreeOrigins {
    pub fn sessions_at(&self, path: &std::path::Path) -> Vec<String> {
        self.known
            .iter()
            .filter(|(_, tree)| tree == path)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// [`Self::sessions_at`], saying which are running and which could only be restored.
    pub fn session_refs_at(
        &self,
        path: &std::path::Path,
    ) -> Vec<crate::session::protocol::WorktreeSession> {
        self.sessions_at(path)
            .into_iter()
            .map(|name| crate::session::protocol::WorktreeSession {
                running: self.live.contains(&name),
                name,
            })
            .collect()
    }
}

pub(crate) fn worktree_session_origins() -> Result<WorktreeOrigins, String> {
    let canonical = |path: &str| {
        let path = std::path::Path::new(path);
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    };
    let mut known = BTreeSet::new();
    let mut unverified = Vec::new();
    let mut live = BTreeSet::new();
    for summary in crate::session::server::list_snapshot_summaries_by_recency() {
        if let Some(tree) = summary.origin.worktree.as_ref() {
            known.insert((summary.session, canonical(&tree.path)));
        }
    }
    let root = crate::control::runtime_dir().map_err(|err| err.to_string())?;
    let endpoints =
        EndpointRegistry::list_session_endpoints(&root).map_err(|err| err.to_string())?;
    for (name, endpoint) in endpoints {
        if crate::scratchpad::runtime::is_client_scratch_session(&name) {
            continue;
        }
        let mut stream = match endpoint.connect() {
            Ok(stream) => stream,
            Err(err) if connect_error_proves_stale(err.kind()) => continue,
            // A busy Windows pipe, a timeout, or a permission error: something may be listening,
            // and its origin is unknown, which a removal guard must not read as "unused".
            Err(err) => {
                unverified.push(UnverifiedSession {
                    name,
                    reason: format!("could not be reached ({err})"),
                });
                continue;
            }
        };
        match query_status(&name, &mut stream, None) {
            Ok(QueryAnswer {
                status: DiscoveredSessionStatus::Running { .. },
                origin,
                ..
            }) => {
                if let Some(tree) = origin.worktree.as_ref() {
                    live.insert(name.clone());
                    known.insert((name, canonical(&tree.path)));
                }
            }
            Ok(answer) => unverified.push(UnverifiedSession::from_answer(name, &answer)),
            Err(err) => unverified.push(UnverifiedSession {
                name,
                reason: format!("did not answer ({err})"),
            }),
        }
    }
    Ok(WorktreeOrigins {
        known: known.into_iter().collect(),
        unverified,
        live,
    })
}

/// Whether a failed connect proves that nothing listens at an endpoint: no socket file or pipe at
/// all, or a Unix socket file whose server is gone. Every other failure, including a Windows pipe
/// whose instances are all momentarily busy, may still have a live server behind it.
fn connect_error_proves_stale(kind: std::io::ErrorKind) -> bool {
    matches!(
        kind,
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
    )
}

/// Conservative removal guard: sessions whose recorded worktree origin is `path`, live or
/// restorable. A live session whose origin cannot be read is an error, because a guard must not
/// read "unknown" as "unused".
pub(crate) fn sessions_using_worktree(path: &str) -> Result<Vec<String>, String> {
    let origins = worktree_session_origins()?;
    if !origins.unverified.is_empty() {
        return Err(unverified_sessions_message(&origins.unverified));
    }
    let requested = std::path::Path::new(path);
    Ok(origins.sessions_at(
        &requested
            .canonicalize()
            .unwrap_or_else(|_| requested.to_path_buf()),
    ))
}

/// Names every session the guard could not check, so stopping one does not just reveal the next,
/// and says what the user can do about it.
fn unverified_sessions_message(unverified: &[UnverifiedSession]) -> String {
    let sessions = unverified
        .iter()
        .map(|session| format!("`{}` {}", session.name, session.reason))
        .collect::<Vec<_>>()
        .join("; ");
    let (subject, object) = match unverified {
        [_] => ("this session uses", "it"),
        _ => ("these sessions use", "them"),
    };
    format!(
        "cannot tell whether {subject} the worktree: {sessions}. Stop {object}, or remove the \
         checkout with `git worktree remove`"
    )
}

/// Probes one session endpoint. Returns `None` whenever the server behind it is gone, so a killed
/// or crashed session stops appearing in the list: an endpoint that refuses the connection outright
/// is stale and gets unlinked, and one that accepts and then hangs up is still retiring and is left
/// to unlink its own.
pub fn query_session_endpoint(name: &str, endpoint: &IpcEndpoint) -> Option<DiscoveredSession> {
    probe_session_endpoint(name, endpoint, None).map(|probed| probed.row)
}

/// [`query_session_endpoint`], plus whatever agent summaries `capabilities` asked the server for.
fn probe_session_endpoint(
    name: &str,
    endpoint: &IpcEndpoint,
    capabilities: Option<&crate::session::protocol::Capabilities>,
) -> Option<ProbedSession> {
    let QueryAnswer {
        status,
        origin,
        agents,
        projects,
        ..
    } = match endpoint.connect() {
        Ok(mut stream) => match query_status(name, &mut stream, capabilities) {
            Ok(answer) => answer,
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                QueryAnswer::without_answer(DiscoveredSessionStatus::Busy)
            }
            // Accepted, then hung up mid-handshake: a server on its way out, whose endpoint has
            // simply not been retired yet. Drop the row rather than reporting it "unavailable" for
            // one sweep, which makes a deliberate kill look like a fault. Leave the endpoint file
            // alone - the retiring server unlinks its own, and a crashed one is unlinked by the
            // next sweep once connecting is refused outright.
            Err(err) if peer_hung_up(err.kind()) => return None,
            // Answered, but not in a language we speak. Stays listed so it can be killed.
            Err(_) => QueryAnswer::unknown(),
        },
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            QueryAnswer::without_answer(DiscoveredSessionStatus::Busy)
        }
        Err(_) => {
            let _ = std::fs::remove_file(endpoint.path());
            return None;
        }
    };
    Some(ProbedSession {
        row: DiscoveredSession {
            name: name.to_string(),
            origin,
            ephemeral: crate::session::naming::is_ephemeral_session_name(name),
            status,
            host: None,
            remote_target: None,
        },
        agents,
        projects,
    })
}

/// Discover sessions locally or via the shared read-only remote executable resolver.
pub fn discover_sessions_from(
    source: &SessionSource,
    config: &crate::config::RemoteConfig,
) -> std::io::Result<Vec<DiscoveredSession>> {
    match source {
        SessionSource::Local => discover_sessions(),
        SessionSource::Remote(target) => discover_remote_sessions(target, config),
    }
}

/// Whether retrying SSH requires an explicit connection to fix authentication or trust.
pub(crate) fn ssh_failure_needs_user(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    [
        "permission denied",
        "authentication failed",
        "too many authentication failures",
        "host key verification failed",
        "remote host identification has changed",
        "host key has changed",
    ]
    .iter()
    .any(|message| error.contains(message))
}

/// Why a host probe failed, in words a user can act on.
///
/// A failed probe carries whatever ssh, the remote shell, or the JSON parser said, which is a
/// paragraph of plumbing (`remote sessions list failed: ssh: connect to host winvm port 22:
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

    // A key that *changed* is a different piece of news from one that was never seen, and the more
    // urgent of the two: OpenSSH refuses it outright rather than offering the yes/no prompt an
    // unknown key gets, precisely because it can mean somebody is in the middle. Checked first, so
    // the generic verification failure below cannot swallow it.
    if says("remote host identification has changed") || says("host key has changed") {
        "Host key changed"
    } else if says("host key verification failed") {
        "Host key not trusted"
    } else if says("permission denied")
        || says("authentication failed")
        || says("too many authentication failures")
    {
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
    } else if says("remote installation cancelled") {
        "Install cancelled"
    } else if says("protocol range") || says("too old for --remote") {
        "Rozi update needed"
    } else if says("no rozi binary found")
        || says("command not found")
        || says("is not recognized")
        || says("no such file or directory")
    {
        "No rozi on host"
    } else {
        "Connection failed"
    }
}

/// Largest remote `sessions list --format json` output accepted.
const REMOTE_LIST_LIMIT: usize = 8 * 1024 * 1024;

/// Reject a listing captured with [`REMOTE_LIST_LIMIT`]` + 1` bytes retained. The extra byte is what
/// tells a listing of exactly the limit apart from one that was cut off.
fn check_remote_list_size(stdout: &[u8], host_label: &str) -> std::io::Result<()> {
    if stdout.len() > REMOTE_LIST_LIMIT {
        return Err(std::io::Error::other(format!(
            "remote sessions list from {host_label} exceeded {} MiB",
            REMOTE_LIST_LIMIT / (1024 * 1024)
        )));
    }
    Ok(())
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
    let remote_bin = crate::session::remote::binary::resolve(target, config)
        .map_err(|error| std::io::Error::other(error.message))?;
    crate::session::remote::validate_remote_executable_token(&remote_bin.path)
        .map_err(std::io::Error::other)?;
    // `ssh_base_command` applies `ConnectTimeout`, so an unreachable configured host fails fast
    // rather than stalling the picker's recurring discovery sweep on a TCP connect. The deadline
    // covers a connected ssh whose remote shell or listing never finishes.
    let mut command = crate::session::remote::ssh_base_command(&resolved, config);
    crate::session::remote::append_ssh_destination(&mut command, &resolved);
    crate::session::remote::append_remote_rozi_command(
        &mut command,
        &remote_bin.path,
        &["sessions", "list", "--format", "json"],
        remote_bin.family,
    );
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = crate::session::remote::run_ssh_command_capturing(
        &mut command,
        None,
        crate::session::remote::SshDeadline::for_config(config).command(),
        REMOTE_LIST_LIMIT + 1,
    )?;
    if output.status != Some(0) {
        crate::session::remote::binary::invalidate(target, config);
        return Err(std::io::Error::other(
            crate::session::remote::sessions_command_failure(
                "list",
                &String::from_utf8_lossy(&output.stderr),
            ),
        ));
    }
    check_remote_list_size(&output.stdout, &host_label)?;
    let mut rows = parse_remote_list_json(&output.stdout, Some(host_label))?;
    for row in &mut rows {
        row.remote_target = Some(target.clone());
    }
    Ok(rows)
}

/// Parse `sessions list --format json` output (also used by remote discovery).
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
        effective_protocol: Option<u32>,
        #[serde(default)]
        rozi_version: Option<String>,
        #[serde(default)]
        origin: crate::session::origin::SessionOrigin,
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
                    effective_protocol: row.effective_protocol,
                    rozi_version: row
                        .rozi_version
                        .filter(|v| !v.is_empty() && !v.chars().any(char::is_control)),
                },
                "restorable" => DiscoveredSessionStatus::Restorable,
                "unreadable" => DiscoveredSessionStatus::Unreadable,
                "busy" => DiscoveredSessionStatus::Busy,
                _ => DiscoveredSessionStatus::Unknown,
            };
            DiscoveredSession {
                name: row.name,
                origin: row.origin,
                status,
                ephemeral: row.ephemeral,
                host: host.clone(),
                remote_target: None,
            }
        })
        .collect())
}

/// Serialize discovered sessions as JSON (for `sessions list --format json`).
pub fn sessions_to_json(rows: &[DiscoveredSession]) -> Result<String, serde_json::Error> {
    let value: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let mut obj = serde_json::json!({
                "name": row.name,
                "ephemeral": row.ephemeral,
            });
            if !row.origin.is_empty() {
                obj["origin"] = serde_json::json!(row.origin);
            }
            if let Some(host) = &row.host {
                obj["host"] = serde_json::json!(host);
            }
            match &row.status {
                DiscoveredSessionStatus::Running {
                    panes,
                    clients,
                    has_layout,
                    effective_protocol,
                    rozi_version,
                } => {
                    obj["status"] = serde_json::json!("running");
                    obj["panes"] = serde_json::json!(panes);
                    obj["clients"] = serde_json::json!(clients);
                    obj["layout"] = serde_json::json!(has_layout);
                    if let Some(protocol) = effective_protocol {
                        obj["effective_protocol"] = serde_json::json!(protocol);
                    }
                    if let Some(version) = rozi_version {
                        obj["rozi_version"] = serde_json::json!(version);
                    }
                }
                // The CLI probes live and never reads the client's host cache, so this does not
                // occur today. Serialized honestly anyway: `panes` is a remembered count, and a
                // consumer must be able to tell it apart from one a server just reported.
                DiscoveredSessionStatus::LastSeen { panes } => {
                    obj["status"] = serde_json::json!("last-seen");
                    obj["panes"] = serde_json::json!(panes);
                }
                DiscoveredSessionStatus::Restorable => {
                    obj["status"] = serde_json::json!("restorable")
                }
                DiscoveredSessionStatus::Unreadable => {
                    obj["status"] = serde_json::json!("unreadable")
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
    #[test]
    fn remote_list_of_exactly_the_limit_is_accepted() {
        use super::{REMOTE_LIST_LIMIT, check_remote_list_size};
        assert!(check_remote_list_size(&vec![b' '; REMOTE_LIST_LIMIT], "host").is_ok());
        let error = check_remote_list_size(&vec![b' '; REMOTE_LIST_LIMIT + 1], "host").unwrap_err();
        assert!(error.to_string().contains("exceeded 8 MiB"), "{error}");
    }

    use super::*;

    /// An upgrade replaces a server only on positive evidence that it is no newer than this build.
    /// A newer release or generation is refused by name, and an answer that proves nothing - a
    /// timeout, a malformed or missing version - is refused too.
    #[test]
    fn an_upgrade_replaces_only_a_server_proven_no_newer() {
        use crate::session::protocol::{MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION, PeerRelease};
        use UpgradeDisposition::{RefuseNewer, RefuseUnverified, Replace};
        let this = semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
        let newer = semver::Version::new(this.major, this.minor, this.patch + 1).to_string();
        let peer = |version: &str, min: u32, max: u32| {
            SessionRelease::Incompatible(Some(PeerRelease {
                rozi_version: version.to_string(),
                protocol_min: min,
                protocol_max: max,
            }))
        };
        let cases = [
            (SessionRelease::Running(Some("0.0.1".into())), Replace),
            (SessionRelease::Running(Some(this.to_string())), Replace),
            (
                SessionRelease::Running(Some(newer.clone())),
                RefuseNewer(newer.clone()),
            ),
            (
                SessionRelease::Running(Some("not a version".into())),
                RefuseUnverified,
            ),
            (SessionRelease::Running(None), RefuseUnverified),
            // A refusal that names no release comes from a server before generation 52.
            (SessionRelease::Incompatible(None), Replace),
            (
                peer(
                    "0.0.1",
                    MIN_SUPPORTED_PROTOCOL - 1,
                    MIN_SUPPORTED_PROTOCOL - 1,
                ),
                Replace,
            ),
            (
                peer(&newer, PROTOCOL_VERSION + 1, PROTOCOL_VERSION + 1),
                RefuseNewer(newer.clone()),
            ),
            // An older release that claims a newer generation is still a newer generation.
            (
                peer("0.0.1", PROTOCOL_VERSION + 1, PROTOCOL_VERSION + 1),
                RefuseNewer("0.0.1".into()),
            ),
            // Older by release but its range overlaps ours, or is nonsense: not proven older.
            (
                peer("0.0.1", PROTOCOL_VERSION, PROTOCOL_VERSION),
                RefuseUnverified,
            ),
            (peer("0.0.1", 9, 3), RefuseUnverified),
            (peer("garbage", 1, 1), RefuseUnverified),
            (SessionRelease::Unknown, RefuseUnverified),
            (SessionRelease::NotRunning, Replace),
        ];
        for (release, expected) in cases {
            assert_eq!(release.upgrade_disposition(), expected, "{release:?}");
        }
    }

    /// A remote row's server runs the host's install, so its hint must not offer this machine's
    /// rollback, whatever kind of install this one is.
    #[test]
    fn an_incompatible_remote_session_is_pointed_at_its_host() {
        let hint = incompatible_session_hint(Some("devbox"));
        assert!(hint.contains("on `devbox`"), "{hint}");
        assert!(!hint.contains("rozi update --rollback"), "{hint}");
        assert!(incompatible_session_hint(None).contains("restarts it from its snapshot"));
    }

    #[test]
    fn ssh_failures_requiring_user_action_stop_retries() {
        for (message, needs_user) in [
            ("user@host: Permission denied (publickey,password).", true),
            ("Authentication failed.", true),
            (
                "Received disconnect from 192.0.2.1 port 22:2: Too many authentication failures",
                true,
            ),
            ("Host key verification failed.", true),
            ("WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!", true),
            ("The host key has changed", true),
            (
                "ssh: connect to host host port 22: Connection timed out",
                false,
            ),
            (
                "ssh: connect to host host port 22: Connection refused",
                false,
            ),
            (
                "ssh: Could not resolve hostname host: Name or service not known",
                false,
            ),
            ("Connection reset by peer", false),
        ] {
            assert_eq!(ssh_failure_needs_user(message), needs_user, "{message}");
        }
    }

    /// The worktree removal guard skips an endpoint only when connecting proves nobody is there.
    /// Windows reports a pipe whose instances are all in use as `WouldBlock` - a live server - so
    /// treating that like a missing endpoint would let a checkout be removed under its session.
    #[test]
    fn only_a_missing_or_refused_endpoint_proves_a_session_gone() {
        use std::io::ErrorKind;
        assert!(connect_error_proves_stale(ErrorKind::NotFound));
        assert!(connect_error_proves_stale(ErrorKind::ConnectionRefused));
        for live_or_unknown in [
            ErrorKind::WouldBlock,
            ErrorKind::TimedOut,
            ErrorKind::PermissionDenied,
            ErrorKind::Interrupted,
            ErrorKind::Other,
        ] {
            assert!(
                !connect_error_proves_stale(live_or_unknown),
                "{live_or_unknown:?} must leave the session unverified"
            );
        }
    }

    /// A private endpoint for one test, named so parallel tests and repeated runs never collide.
    ///
    /// The label names the directory, not just the socket in it: a parent shared by every test in
    /// this module races on creation, and the losing thread fails outright (see
    /// [`crate::test_support::private_temp_dir`]).
    fn test_endpoint(label: &str) -> IpcEndpoint {
        let endpoint = IpcEndpoint::at_path(
            crate::test_support::private_temp_dir(&format!("discovery-{label}"))
                .join(format!("{label}-{}.sock", std::process::id())),
        );
        endpoint.remove_stale();
        endpoint
    }

    /// Real messages, verbatim from ssh/OpenSSH and from this module, mapped onto the phrase the
    /// sidebar shows. The point of the test is the shapes, not the mapping: each of these is what
    /// the user actually hits, and each names a different thing to go fix.
    #[test]
    fn probe_failures_name_what_to_go_fix_rather_than_quoting_ssh() {
        let reason = |raw: &str| probe_failure_reason(raw);
        let remote = |stderr: &str| format!("remote sessions list failed: {stderr}");

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
        // A key that changed is the alarming case and says so, rather than reading like a host
        // rozi has simply not met. OpenSSH reports both in one stderr, so the order matters.
        assert_eq!(
            reason(&remote(
                "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n\
                 @    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\n\
                 @@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n\
                 IT IS POSSIBLE THAT SOMEONE IS DOING SOMETHING NASTY!\n\
                 Host key for 127.0.0.1 has changed and you have requested strict checking.\n\
                 Host key verification failed."
            )),
            "Host key changed"
        );
        // The host answered; the remote shell could not run rozi.
        assert_eq!(
            reason(&remote("bash: line 1: rozi: command not found")),
            "No rozi on host"
        );
        assert_eq!(
            reason(&remote(
                "'rozi' is not recognized as an internal or external command"
            )),
            "No rozi on host"
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
                "Permission denied (publickey).\nbash: rozi: No such file or directory"
            )),
            "SSH login rejected"
        );
    }

    /// A server on its way out accepts the probe and then hangs up. Discovery must read that as
    /// "gone", not as a session that exists and is unusable: the picker refreshes the instant a
    /// kill is requested, and a lingering "unavailable" row makes a deliberate kill look like a
    /// fault. The endpoint itself is left alone - the retiring server retires its own.
    #[test]
    fn a_server_that_hangs_up_mid_probe_is_gone_rather_than_unavailable() {
        let endpoint = test_endpoint("hangup");
        let listener = endpoint.bind().expect("bind").into_listener();
        let server = std::thread::spawn(move || drop(listener.accept().expect("accept")));

        assert_eq!(query_session_endpoint("hangup", &endpoint), None);
        server.join().expect("server thread");
        assert!(
            endpoint.path().exists(),
            "a live endpoint is never unlinked"
        );
        endpoint.remove_stale();
    }

    /// A server that answers, but with something other than `SessionInfo`, is a live session we
    /// cannot speak to. It stays listed so it can still be killed from the picker, and the worktree
    /// removal guard says it runs another version rather than only that it could not be checked.
    #[test]
    fn a_server_that_refuses_the_handshake_stays_listed_as_unknown() {
        let endpoint = test_endpoint("mismatch");
        let listener = endpoint.bind().expect("bind").into_listener();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let mut stream = listener.accept().expect("accept");
                let _: ClientMessage =
                    crate::session::protocol::read_frame(&mut stream).expect("query");
                crate::session::protocol::write_frame(
                    &mut stream,
                    &ServerMessage::Error {
                        code: crate::session::protocol::PROTOCOL_MISMATCH_ERROR_CODE.to_string(),
                        message: "speaks another version".to_string(),
                        peer: None,
                    },
                )
                .expect("error reply");
            }
        });

        let row = query_session_endpoint("mismatch", &endpoint).expect("listed");
        assert_eq!(row.status, DiscoveredSessionStatus::Unknown);
        let mut stream = endpoint.connect().expect("connect");
        let answer = query_status("mismatch", &mut stream, None).expect("answer");
        assert_eq!(
            UnverifiedSession::from_answer("mismatch".to_string(), &answer).reason,
            "runs a different rozi version"
        );
        server.join().expect("server thread");
        endpoint.remove_stale();
    }

    #[test]
    fn removal_guard_names_every_unverified_session_and_the_way_out() {
        let session = |name: &str, reason: &str| UnverifiedSession {
            name: name.to_string(),
            reason: reason.to_string(),
        };
        assert_eq!(
            unverified_sessions_message(&[session("dev", "runs a different rozi version")]),
            "cannot tell whether this session uses the worktree: `dev` runs a different rozi \
             version. Stop it, or remove the checkout with `git worktree remove`"
        );
        assert_eq!(
            unverified_sessions_message(&[
                session("dev", "runs a different rozi version"),
                session("eph-1", "did not answer (timed out)"),
            ]),
            "cannot tell whether these sessions use the worktree: `dev` runs a different rozi \
             version; `eph-1` did not answer (timed out). Stop them, or remove the checkout with \
             `git worktree remove`"
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
    fn restorable_snapshots_fill_picker_gaps_without_shadowing_live_sessions() {
        let mut rows = vec![DiscoveredSession {
            name: "live".into(),
            origin: Default::default(),
            ephemeral: false,
            host: None,
            remote_target: None,
            status: DiscoveredSessionStatus::Busy,
        }];

        push_restorable_sessions(
            &mut rows,
            Some("current"),
            ["live", "current", "saved"].map(|name| crate::session::server::SnapshotSummary {
                session: name.to_string(),
                saved_at: 1,
                origin: crate::session::origin::SessionOrigin {
                    worktree: Some(crate::session::origin::WorktreeOrigin {
                        path: "C:\\code\\feature".into(),
                    }),
                    ..Default::default()
                },
                readable: true,
                panes: 1,
            }),
        );

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].name, "saved");
        assert_eq!(rows[1].status, DiscoveredSessionStatus::Restorable);
        assert_eq!(
            rows[1]
                .origin
                .worktree
                .as_ref()
                .map(|tree| tree.path.as_str()),
            Some("C:\\code\\feature")
        );
    }

    #[test]
    fn list_json_round_trips_through_parser() {
        let rows = vec![
            DiscoveredSession {
                name: "dev".into(),
                origin: crate::session::origin::SessionOrigin {
                    profile: Some("work".into()),
                    worktree: Some(crate::session::origin::WorktreeOrigin {
                        path: "C:\\code\\feature".into(),
                    }),
                },
                ephemeral: false,
                host: Some("workbox".into()),
                remote_target: None,
                status: DiscoveredSessionStatus::Running {
                    panes: 2,
                    clients: 1,
                    has_layout: true,
                    effective_protocol: Some(52),
                    rozi_version: Some("0.0.41".into()),
                },
            },
            DiscoveredSession {
                name: "saved".into(),
                origin: Default::default(),
                ephemeral: false,
                host: None,
                remote_target: None,
                status: DiscoveredSessionStatus::Restorable,
            },
            DiscoveredSession {
                name: "future".into(),
                origin: Default::default(),
                ephemeral: false,
                host: None,
                remote_target: None,
                status: DiscoveredSessionStatus::Unreadable,
            },
        ];
        let json = sessions_to_json(&rows).unwrap();
        let parsed = parse_remote_list_json(json.as_bytes(), Some("workbox".into())).unwrap();
        assert_eq!(parsed[0].name, "dev");
        assert_eq!(parsed[0].host.as_deref(), Some("workbox"));
        assert_eq!(parsed[0].origin, rows[0].origin);
        assert_eq!(parsed[0].effective_protocol(), Some(52));
        assert_eq!(parsed[0].rozi_version(), Some("0.0.41"));
        assert!(matches!(
            parsed[0].status,
            DiscoveredSessionStatus::Running { panes: 2, .. }
        ));
        assert_eq!(parsed[1].status, DiscoveredSessionStatus::Restorable);
        assert_eq!(parsed[2].status, DiscoveredSessionStatus::Unreadable);
    }

    /// A name is spelled into a socket path, so an unbounded one is a session that cannot be
    /// served. Rejecting it where names are admitted turns a bind failure into a refused name.
    #[test]
    fn a_name_too_long_to_bind_is_not_a_valid_target() {
        assert!(valid_attach_target(&"a".repeat(MAX_SESSION_NAME_LEN)));
        assert!(!valid_attach_target(&"a".repeat(MAX_SESSION_NAME_LEN + 1)));
        assert!(!valid_session_name(&"a".repeat(MAX_SESSION_NAME_LEN + 1)));
    }
}
