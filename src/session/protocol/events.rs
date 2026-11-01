//! The session event vocabulary: what a session server raises and what its clients and subscribers
//! read, and the fields a pane's runtime changes report.
//!
//! The field builders are shared because a client raises the same events itself for the panes it
//! owns, which never reach the session stream. An event means the same thing from either source.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::layout::shared::ClientId;
use crate::session::protocol::{
    AgentRuntime, AgentState, PaneCommandPhase, PaneStatus, SessionInstanceId,
};
use crate::state::PaneId;

/// An event's fields, as they are built before going onto the wire.
pub type EventFields = Vec<(&'static str, String)>;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum EventKind {
    PaneSpawned,
    PaneExited,
    PaneStatusChanged,
    PaneTitleChanged,
    PaneCwdChanged,
    AgentStateChanged,
    ForegroundChanged,
    Bell,
    FocusChanged,
    WorkspaceSwitched,
    LayoutChanged,
    SessionAttached,
    SessionDetached,
    SessionRenamed,
    SessionCreated,
    ControllerChanged,
    ClientJoined,
    ClientLeft,
    ProfileLoaded,
    ProfileApplied,
    ProfileSaved,
    ConfigReloaded,
}

impl EventKind {
    pub const ALL: [Self; 22] = [
        Self::PaneSpawned,
        Self::PaneExited,
        Self::PaneStatusChanged,
        Self::PaneTitleChanged,
        Self::PaneCwdChanged,
        Self::AgentStateChanged,
        Self::ForegroundChanged,
        Self::Bell,
        Self::FocusChanged,
        Self::WorkspaceSwitched,
        Self::LayoutChanged,
        Self::SessionAttached,
        Self::SessionDetached,
        Self::SessionRenamed,
        Self::SessionCreated,
        Self::ControllerChanged,
        Self::ClientJoined,
        Self::ClientLeft,
        Self::ProfileLoaded,
        Self::ProfileApplied,
        Self::ProfileSaved,
        Self::ConfigReloaded,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::PaneSpawned => "pane-spawned",
            Self::PaneExited => "pane-exited",
            Self::PaneStatusChanged => "pane-status-changed",
            Self::PaneTitleChanged => "pane-title-changed",
            Self::PaneCwdChanged => "pane-cwd-changed",
            Self::AgentStateChanged => "agent-state-changed",
            Self::ForegroundChanged => "foreground-changed",
            Self::Bell => "bell",
            Self::FocusChanged => "focus-changed",
            Self::WorkspaceSwitched => "workspace-switched",
            Self::LayoutChanged => "layout-changed",
            Self::SessionAttached => "session-attached",
            Self::SessionDetached => "session-detached",
            Self::SessionRenamed => "session-renamed",
            Self::SessionCreated => "session-created",
            Self::ControllerChanged => "controller-changed",
            Self::ClientJoined => "client-joined",
            Self::ClientLeft => "client-left",
            Self::ProfileLoaded => "profile-loaded",
            Self::ProfileApplied => "profile-applied",
            Self::ProfileSaved => "profile-saved",
            Self::ConfigReloaded => "config-reloaded",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.id() == value)
    }

    /// Whether a session server raises this event, rather than one UI about itself.
    pub fn is_session_owned(self) -> bool {
        match self {
            Self::PaneSpawned
            | Self::PaneExited
            | Self::PaneStatusChanged
            | Self::PaneTitleChanged
            | Self::PaneCwdChanged
            | Self::AgentStateChanged
            | Self::ForegroundChanged
            | Self::Bell
            | Self::LayoutChanged
            | Self::SessionRenamed
            | Self::ControllerChanged
            | Self::ClientJoined
            | Self::ClientLeft => true,
            Self::FocusChanged
            | Self::WorkspaceSwitched
            | Self::SessionAttached
            | Self::SessionDetached
            | Self::SessionCreated
            | Self::ProfileLoaded
            | Self::ProfileApplied
            | Self::ProfileSaved
            | Self::ConfigReloaded => false,
        }
    }
}

/// One `subscribe` event as it goes out on the wire.
///
/// `data` is deliberately a flat map of strings rather than a per-kind payload type. The fields an
/// event carries grow over time and a subscriber is expected to read the ones it knows and ignore
/// the rest, so the schema describes the envelope and the closed vocabulary of event names without
/// freezing each kind's field list.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct WireEvent {
    pub event: EventKind,
    pub data: BTreeMap<String, String>,
}

/// One sequenced event recorded by a [`SessionServer`](super::SessionServer).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct SessionEvent {
    pub session_instance: SessionInstanceId,
    pub seq: u64,
    pub kind: EventKind,
    pub data: BTreeMap<String, String>,
}

impl SessionEvent {
    /// Retained byte weight charged against [`SessionEventBuffer`]'s byte budget.
    pub fn byte_len(&self) -> usize {
        self.kind
            .id()
            .len()
            .saturating_add(self.session_instance.as_str().len())
            .saturating_add(
                self.data
                    .iter()
                    .map(|(key, value)| key.len().saturating_add(value.len()))
                    .sum::<usize>(),
            )
    }
}

/// Reported when a resume cursor names a different session instance, a sequence number that has
/// already been evicted from the ring buffer, or a sequence ahead of the server's latest event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct CursorExpired {
    pub current_instance: SessionInstanceId,
    pub oldest_seq: Option<u64>,
    pub latest_seq: u64,
}

impl std::fmt::Display for CursorExpired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cursor expired: current instance is {}, ",
            self.current_instance
        )?;
        match self.oldest_seq {
            Some(oldest) => write!(f, "oldest retained seq is {oldest}, ")?,
            None => write!(f, "no events are retained, ")?,
        }
        write!(f, "latest seq is {}", self.latest_seq)
    }
}

/// Current snapshot of a session as streamed at subscription start when `--snapshot` is requested.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct SessionSubscribeSnapshot {
    pub session_instance: SessionInstanceId,
    pub session: String,
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller: Option<ClientId>,
    pub client_count: u32,
    pub layout_rev: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<crate::control::LayoutReport>,
    pub panes: Vec<crate::control::PaneInfo>,
}

/// Typed records streamed by a session subscription.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum SessionSubscribeRecord {
    Snapshot(SessionSubscribeSnapshot),
    Event {
        #[serde(flatten)]
        event: SessionEvent,
    },
    Lagged,
}

/// The program a pane runs in its foreground, other than its shell, and its process group.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneForeground {
    program: Option<String>,
    pid: Option<u32>,
}

impl PaneForeground {
    /// Read off what profile capture and resurrection read, so an event never disagrees with what
    /// a subscriber reads back when it asks.
    pub fn running(
        command_phase: PaneCommandPhase,
        program: Option<&str>,
        shell: bool,
        pid: Option<u32>,
    ) -> Self {
        let program = crate::pane::launch::running_program(command_phase, program, shell);
        Self {
            program: program.map(str::to_string),
            // The group a shell leads is the shell sitting at its prompt.
            pid: program.and(pid),
        }
    }

    /// `foreground-changed`'s fields for a change from `before` to this.
    pub fn changed_from(&self, pane: PaneId, before: &Self) -> EventFields {
        vec![
            ("pane", pane.to_string()),
            ("program", self.program.clone().unwrap_or_default()),
            (
                "previous_program",
                before.program.clone().unwrap_or_default(),
            ),
            (
                "pid",
                self.pid.map(|pid| pid.to_string()).unwrap_or_default(),
            ),
        ]
    }
}

/// `pane-status-changed`'s fields.
pub fn pane_status_fields(
    pane: PaneId,
    previous: Option<&PaneStatus>,
    current: Option<&PaneStatus>,
) -> EventFields {
    let value = |status: Option<&PaneStatus>| status.map(|s| s.value.clone()).unwrap_or_default();
    let reason =
        |status: Option<&PaneStatus>| status.and_then(|s| s.reason.clone()).unwrap_or_default();
    vec![
        ("pane", pane.to_string()),
        ("status", value(current)),
        ("reason", reason(current)),
        ("previous_status", value(previous)),
        ("previous_reason", reason(previous)),
    ]
}

/// `agent-state-changed`'s fields, for each agent whose state differs between `before` and `after`.
///
/// An agent is matched by its reference, which names one incarnation of an agent across all the
/// runs it finishes. A replacement - the agent relaunched in the same pane - is a new incarnation:
/// the old one goes (`state` empty) and the new one appears (`previous_state` empty).
pub fn agent_state_changes(
    pane: PaneId,
    before: &[AgentRuntime],
    after: &[AgentRuntime],
) -> Vec<EventFields> {
    let earlier = |runtime: &AgentRuntime| {
        before
            .iter()
            .find(|candidate| candidate.reference == runtime.reference)
    };
    let changed = after.iter().filter_map(|runtime| {
        let previous = earlier(runtime).map(|earlier| earlier.state);
        (previous != Some(runtime.state))
            .then(|| agent_state_fields(pane, runtime, Some(runtime.state), previous))
    });
    let gone = before
        .iter()
        .filter(|runtime| {
            !after
                .iter()
                .any(|candidate| candidate.reference == runtime.reference)
        })
        .map(|runtime| agent_state_fields(pane, runtime, None, Some(runtime.state)));
    changed.chain(gone).collect()
}

fn agent_state_fields(
    pane: PaneId,
    runtime: &AgentRuntime,
    state: Option<AgentState>,
    previous: Option<AgentState>,
) -> EventFields {
    let name = |state: Option<AgentState>| state.map(AgentState::as_str).unwrap_or_default();
    vec![
        ("pane", pane.to_string()),
        ("agent", runtime.identity.id.clone()),
        ("label", runtime.label.clone()),
        (
            "ref",
            serde_json::to_string(&runtime.reference).expect("agent references serialize"),
        ),
        ("state", name(state).to_string()),
        ("previous_state", name(previous).to_string()),
        (
            "reason",
            state.and(runtime.reason.clone()).unwrap_or_default(),
        ),
    ]
}
