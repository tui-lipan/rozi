use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::layout::shared::ClientId;
use crate::session::protocol::{
    AgentRecord, AgentRuntime, AgentState, PaneRuntimeState, PaneStatus, SessionInstanceId,
};
use crate::state::PaneId;

pub const DEFAULT_MAX_EVENTS: usize = 1024;
pub const DEFAULT_MAX_BYTES: usize = 1024 * 1024;

/// How long a pane's title must hold still before `pane-title-changed` is recorded.
///
/// Programs that animate a title - a spinner, an elapsed clock, blinking attention text - rewrite
/// it several times a second. A subscriber cares about the title a pane settled on, not every
/// frame of the animation, so a change is held until it survives this window.
pub const TITLE_SETTLE: Duration = Duration::from_millis(400);

/// Debounced title tracking for one pane, so a chatty title cannot flood the event buffer.
#[derive(Clone, Debug, Default)]
pub(super) struct PaneTitleWatch {
    /// The title last reported, or the title the pane started with.
    reported: Option<String>,
    /// The newest observed title and when it was first seen.
    pending: Option<(Option<String>, Instant)>,
}

impl PaneTitleWatch {
    pub(super) fn new(reported: Option<String>) -> Self {
        Self {
            reported,
            pending: None,
        }
    }

    /// Observe a pane's title after a chunk of output. Returning to the reported title cancels a
    /// pending change; any other title restarts the settle window from its first sighting.
    pub(super) fn observe(&mut self, title: Option<String>, now: Instant) {
        if title == self.reported {
            self.pending = None;
        } else if self
            .pending
            .as_ref()
            .is_none_or(|(pending, _)| pending != &title)
        {
            self.pending = Some((title, now));
        }
    }

    /// Take a title that has been stable for [`TITLE_SETTLE`], as `(previous, current)`.
    pub(super) fn settle(&mut self, now: Instant) -> Option<(Option<String>, Option<String>)> {
        let (title, since) = self.pending.as_ref()?;
        if now.duration_since(*since) < TITLE_SETTLE {
            return None;
        }
        let title = title.clone();
        let previous = std::mem::replace(&mut self.reported, title.clone());
        self.pending = None;
        Some((previous, title))
    }
}

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
    pub fn to_wire_event(&self) -> WireEvent {
        WireEvent {
            event: self.kind,
            data: self.data.clone(),
        }
    }

    pub fn into_wire_event(self) -> WireEvent {
        WireEvent {
            event: self.kind,
            data: self.data,
        }
    }

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

/// Ring buffer of recent session events bounded by both event count and total retained bytes.
#[derive(Clone, Debug)]
pub struct SessionEventBuffer {
    session_instance: SessionInstanceId,
    events: VecDeque<SessionEvent>,
    total_bytes: usize,
    latest_seq: u64,
    max_events: usize,
    max_bytes: usize,
}

impl SessionEventBuffer {
    pub fn new(session_instance: SessionInstanceId) -> Self {
        Self::with_limits(session_instance, DEFAULT_MAX_EVENTS, DEFAULT_MAX_BYTES)
    }

    pub fn with_limits(
        session_instance: SessionInstanceId,
        max_events: usize,
        max_bytes: usize,
    ) -> Self {
        Self {
            session_instance,
            events: VecDeque::new(),
            total_bytes: 0,
            latest_seq: 0,
            max_events,
            max_bytes,
        }
    }

    pub fn session_instance(&self) -> &SessionInstanceId {
        &self.session_instance
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn max_events(&self) -> usize {
        self.max_events
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    pub fn oldest_seq(&self) -> Option<u64> {
        self.events.front().map(|event| event.seq)
    }

    pub fn latest_seq(&self) -> u64 {
        self.latest_seq
    }

    pub fn iter(&self) -> impl Iterator<Item = &SessionEvent> {
        self.events.iter()
    }

    fn cursor_expired(&self) -> CursorExpired {
        CursorExpired {
            current_instance: self.session_instance.clone(),
            oldest_seq: self.oldest_seq(),
            latest_seq: self.latest_seq,
        }
    }

    /// Record a new session event at the next monotonic sequence number, evicting the oldest
    /// buffered events as needed to stay within both the count and byte limits.
    pub fn record(
        &mut self,
        kind: EventKind,
        fields: Vec<(&'static str, String)>,
    ) -> Option<&SessionEvent> {
        self.latest_seq = self.latest_seq.saturating_add(1);
        let data = fields
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect();
        let event = SessionEvent {
            session_instance: self.session_instance.clone(),
            seq: self.latest_seq,
            kind,
            data,
        };
        let entry_bytes = event.byte_len();
        if self.max_events == 0 || entry_bytes > self.max_bytes {
            self.events.clear();
            self.total_bytes = 0;
            return None;
        }
        while self.events.len() >= self.max_events
            || self.total_bytes.saturating_add(entry_bytes) > self.max_bytes
        {
            let Some(evicted) = self.events.pop_front() else {
                break;
            };
            self.total_bytes = self.total_bytes.saturating_sub(evicted.byte_len());
        }
        self.total_bytes = self.total_bytes.saturating_add(entry_bytes);
        self.events.push_back(event);
        self.events.back()
    }

    /// Return every buffered event with `event.seq > seq` for `instance`, or [`CursorExpired`]
    /// when `instance` does not match this server or `seq + 1` is no longer in the ring buffer.
    pub fn after(
        &self,
        instance: &SessionInstanceId,
        seq: u64,
    ) -> Result<Vec<SessionEvent>, CursorExpired> {
        if instance != &self.session_instance || seq > self.latest_seq {
            return Err(self.cursor_expired());
        }
        if seq == self.latest_seq {
            return Ok(Vec::new());
        }
        let next_seq = seq.saturating_add(1);
        let Some(oldest_seq) = self.oldest_seq() else {
            return Err(self.cursor_expired());
        };
        if next_seq < oldest_seq {
            return Err(self.cursor_expired());
        }
        let start = (next_seq - oldest_seq) as usize;
        Ok(self.events.iter().skip(start).cloned().collect())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PaneForeground {
    program: Option<String>,
    pid: Option<u32>,
}

impl PaneForeground {
    fn of(runtime: &PaneRuntimeState) -> Self {
        let program = crate::pane::launch::running_program(
            runtime.command_phase,
            runtime.foreground_program.as_deref(),
            runtime.foreground_shell,
        );
        Self {
            program: program.map(str::to_string),
            pid: program.and(runtime.foreground_pid),
        }
    }
}

/// Snapshot of a shared pane's observable event fields before a [`PaneRuntimeState`] mutation.
#[derive(Clone, Debug)]
pub(super) struct PaneRuntimeBaseline {
    pane_id: PaneId,
    status: Option<PaneStatus>,
    cwd: Option<String>,
    agents: Vec<AgentRuntime>,
    foreground: PaneForeground,
}

impl PaneRuntimeBaseline {
    pub(super) fn capture(
        session_instance: &SessionInstanceId,
        pane_id: PaneId,
        pane: &super::ServerPane,
    ) -> Self {
        let pane_ref = crate::session::protocol::PaneRef {
            session_instance: session_instance.clone(),
            pane_id,
            generation: pane.generation,
        };
        let records = pane.agent.records(pane_ref);
        Self {
            pane_id,
            status: pane.runtime.status.clone(),
            cwd: pane.runtime.cwd.clone(),
            agents: crate::session::protocol::effective_agent_runtimes(&pane.runtime, &records),
            foreground: PaneForeground::of(&pane.runtime),
        }
    }

    pub(super) fn into_events(
        self,
        after_state: &PaneRuntimeState,
        after_records: &[AgentRecord],
    ) -> Vec<(EventKind, Vec<(&'static str, String)>)> {
        let mut events = Vec::new();
        if self.status != after_state.status {
            events.push((
                EventKind::PaneStatusChanged,
                vec![
                    ("pane", self.pane_id.to_string()),
                    (
                        "status",
                        after_state
                            .status
                            .as_ref()
                            .map(|status| status.value.clone())
                            .unwrap_or_default(),
                    ),
                    (
                        "reason",
                        after_state
                            .status
                            .as_ref()
                            .and_then(|status| status.reason.clone())
                            .unwrap_or_default(),
                    ),
                    (
                        "previous_status",
                        self.status
                            .as_ref()
                            .map(|status| status.value.clone())
                            .unwrap_or_default(),
                    ),
                    (
                        "previous_reason",
                        self.status
                            .as_ref()
                            .and_then(|status| status.reason.clone())
                            .unwrap_or_default(),
                    ),
                ],
            ));
        }

        let after_agents =
            crate::session::protocol::effective_agent_runtimes(after_state, after_records);
        for fields in diff_agent_events(self.pane_id, &self.agents, &after_agents) {
            events.push((EventKind::AgentStateChanged, fields));
        }

        if after_state.cwd != self.cwd {
            events.push((
                EventKind::PaneCwdChanged,
                vec![
                    ("pane", self.pane_id.to_string()),
                    ("cwd", after_state.cwd.clone().unwrap_or_default()),
                    ("previous_cwd", self.cwd.clone().unwrap_or_default()),
                ],
            ));
        }

        let after_foreground = PaneForeground::of(after_state);
        if after_foreground != self.foreground {
            events.push((
                EventKind::ForegroundChanged,
                vec![
                    ("pane", self.pane_id.to_string()),
                    ("program", after_foreground.program.unwrap_or_default()),
                    (
                        "previous_program",
                        self.foreground.program.unwrap_or_default(),
                    ),
                    (
                        "pid",
                        after_foreground
                            .pid
                            .map(|pid| pid.to_string())
                            .unwrap_or_default(),
                    ),
                ],
            ));
        }
        events
    }
}

fn diff_agent_events(
    pane_id: PaneId,
    before: &[AgentRuntime],
    after: &[AgentRuntime],
) -> Vec<Vec<(&'static str, String)>> {
    let earlier = |runtime: &AgentRuntime| {
        before
            .iter()
            .find(|candidate| candidate.reference == runtime.reference)
    };
    let changed = after.iter().filter_map(|runtime| {
        let previous = earlier(runtime).map(|earlier| earlier.state);
        (previous != Some(runtime.state))
            .then(|| agent_event_fields(pane_id, runtime, Some(runtime.state), previous))
    });
    let gone = before
        .iter()
        .filter(|runtime| {
            !after
                .iter()
                .any(|candidate| candidate.reference == runtime.reference)
        })
        .map(|runtime| agent_event_fields(pane_id, runtime, None, Some(runtime.state)));
    changed.chain(gone).collect()
}

fn agent_event_fields(
    pane_id: PaneId,
    runtime: &AgentRuntime,
    state: Option<AgentState>,
    previous: Option<AgentState>,
) -> Vec<(&'static str, String)> {
    let name = |state: Option<AgentState>| state.map(AgentState::as_str).unwrap_or_default();
    vec![
        ("pane", pane_id.to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(id: &str) -> SessionInstanceId {
        serde_json::from_str(&format!("{id:?}")).expect("test session instance id")
    }

    #[test]
    fn title_watch_coalesces_until_a_title_holds_still() {
        let start = Instant::now();
        let mut watch = PaneTitleWatch::new(None);

        watch.observe(Some("build 1".into()), start);
        watch.observe(Some("build 2".into()), start + Duration::from_millis(50));
        watch.observe(Some("build 3".into()), start + Duration::from_millis(100));
        assert_eq!(watch.settle(start + Duration::from_millis(300)), None);

        // The newest title settles after holding for TITLE_SETTLE, not from the first sighting.
        let settled = watch.settle(start + Duration::from_millis(100) + TITLE_SETTLE);
        assert_eq!(settled, Some((None, Some("build 3".to_string()))));
        assert_eq!(
            watch.settle(start + TITLE_SETTLE + Duration::from_secs(1)),
            None
        );
    }

    #[test]
    fn title_watch_cancels_a_change_that_returns_to_the_reported_title() {
        let start = Instant::now();
        let mut watch = PaneTitleWatch::new(Some("shell".into()));

        watch.observe(Some("spinner".into()), start);
        watch.observe(Some("shell".into()), start + Duration::from_millis(50));
        assert_eq!(watch.settle(start + TITLE_SETTLE * 2), None);

        watch.observe(Some("editor".into()), start);
        assert_eq!(
            watch.settle(start + TITLE_SETTLE),
            Some((Some("shell".to_string()), Some("editor".to_string())))
        );
    }

    #[test]
    fn buffer_evicts_oldest_events_when_count_limit_is_reached() {
        let inst = instance("inst-count");
        let mut buffer = SessionEventBuffer::with_limits(inst.clone(), 3, 64 * 1024);

        for idx in 1..=5 {
            buffer.record(
                EventKind::PaneExited,
                vec![("pane", idx.to_string()), ("code", "0".to_string())],
            );
        }

        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.oldest_seq(), Some(3));
        assert_eq!(buffer.latest_seq(), 5);

        let events = buffer.after(&inst, 2).expect("seq 3..=5 still retained");
        let seqs: Vec<u64> = events.iter().map(|event| event.seq).collect();
        assert_eq!(seqs, vec![3, 4, 5]);
        assert_eq!(events[0].data["pane"], "3");
        assert_eq!(events[2].data["pane"], "5");

        assert_eq!(
            buffer.after(&inst, 1),
            Err(CursorExpired {
                current_instance: inst,
                oldest_seq: Some(3),
                latest_seq: 5,
            })
        );
    }

    #[test]
    fn buffer_evicts_oldest_events_when_byte_limit_is_reached() {
        let inst = instance("inst-bytes");
        let mut probe = SessionEventBuffer::new(inst.clone());
        let single_bytes = probe
            .record(
                EventKind::SessionRenamed,
                vec![
                    ("session", "alpha".to_string()),
                    ("previous", "beta".to_string()),
                ],
            )
            .unwrap()
            .byte_len();

        // Allow at most 2 events worth of bytes despite a higher event-count ceiling.
        let mut buffer = SessionEventBuffer::with_limits(inst.clone(), 16, single_bytes * 2);
        for name in ["s1", "s2", "s3"] {
            buffer.record(
                EventKind::SessionRenamed,
                vec![
                    ("session", format!("{name:<5}")),
                    ("previous", "beta".to_string()),
                ],
            );
        }

        assert_eq!(buffer.len(), 2);
        assert!(buffer.total_bytes() <= buffer.max_bytes());
        assert_eq!(buffer.oldest_seq(), Some(2));
        assert_eq!(buffer.latest_seq(), 3);

        let retained = buffer.after(&inst, 1).expect("seq 2..=3 retained");
        assert_eq!(retained.len(), 2);
        assert_eq!(retained[0].seq, 2);
        assert_eq!(retained[1].seq, 3);
    }

    #[test]
    fn cursor_lookup_handles_initial_caught_up_expired_and_wrong_instance() {
        let inst = instance("inst-main");
        let other = instance("inst-other");
        let mut buffer = SessionEventBuffer::with_limits(inst.clone(), 2, 64 * 1024);

        // Empty buffer: seq 0 is caught up, seq 1 is in the future.
        assert_eq!(buffer.after(&inst, 0), Ok(Vec::new()));
        assert_eq!(
            buffer.after(&inst, 1),
            Err(CursorExpired {
                current_instance: inst.clone(),
                oldest_seq: None,
                latest_seq: 0,
            })
        );

        buffer.record(EventKind::ClientJoined, vec![("count", "1".to_string())]);
        buffer.record(EventKind::ClientLeft, vec![("count", "0".to_string())]);

        let all = buffer
            .after(&inst, 0)
            .expect("seq 0 catches up from start when seq 1 is still buffered");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].seq, 1);
        assert_eq!(all[1].seq, 2);

        let tail = buffer.after(&inst, 1).expect("seq 1 returns event 2");
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].seq, 2);

        assert_eq!(buffer.after(&inst, 2), Ok(Vec::new()));

        // Future cursor is rejected as expired.
        assert_eq!(
            buffer.after(&inst, 3),
            Err(CursorExpired {
                current_instance: inst.clone(),
                oldest_seq: Some(1),
                latest_seq: 2,
            })
        );

        // Wrong session instance is rejected with the current server's instance and bounds.
        assert_eq!(
            buffer.after(&other, 1),
            Err(CursorExpired {
                current_instance: inst.clone(),
                oldest_seq: Some(1),
                latest_seq: 2,
            })
        );

        // Push a third event so seq 1 is evicted; seq 0 now expires, while seq 1..=3 work.
        buffer.record(EventKind::ClientJoined, vec![("count", "1".to_string())]);
        assert_eq!(
            buffer.after(&inst, 0),
            Err(CursorExpired {
                current_instance: inst.clone(),
                oldest_seq: Some(2),
                latest_seq: 3,
            })
        );
        assert_eq!(buffer.after(&inst, 1).unwrap().len(), 2);
        assert_eq!(buffer.after(&inst, 2).unwrap().len(), 1);
        assert_eq!(buffer.after(&inst, 3), Ok(Vec::new()));

        // Push a fourth event so seq 2 is also evicted; now seq 1 expires too.
        buffer.record(EventKind::ClientLeft, vec![("count", "0".to_string())]);
        assert_eq!(
            buffer.after(&inst, 1),
            Err(CursorExpired {
                current_instance: inst.clone(),
                oldest_seq: Some(3),
                latest_seq: 4,
            })
        );
        assert_eq!(buffer.after(&inst, 2).unwrap().len(), 2);
        assert_eq!(buffer.after(&inst, 3).unwrap().len(), 1);
        assert_eq!(buffer.after(&inst, 4), Ok(Vec::new()));
    }
}
