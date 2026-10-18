use serde::{Deserialize, Serialize};

use super::{
    AgentIdentity, AgentIntegrationReport, AgentRef, DetectedAgent, PaneRuntimeState, PaneStatus,
    PublishedRow, RowKey, detected_agent_status, pane_status,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum AgentState {
    Working,
    Blocked,
    Idle,
    Done,
}

impl AgentState {
    pub fn from_status(status: &str) -> Self {
        let status = status.trim();
        if status.eq_ignore_ascii_case(pane_status::BLOCKED) {
            Self::Blocked
        } else if status.eq_ignore_ascii_case(pane_status::IDLE) {
            Self::Idle
        } else if status.eq_ignore_ascii_case(pane_status::DONE) {
            Self::Done
        } else {
            Self::Working
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Working => pane_status::WORKING,
            Self::Blocked => pane_status::BLOCKED,
            Self::Idle => pane_status::IDLE,
            Self::Done => pane_status::DONE,
        }
    }

    pub const fn is_quiescent(self) -> bool {
        matches!(self, Self::Idle | Self::Done)
    }
}

/// Presentation semantics for activities. Custom values remain active; agent APIs retain
/// their existing four-state interpretation, including treating `error` as working.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityState {
    Working,
    Blocked,
    Idle,
    Done,
    Error,
    Custom,
}

impl ActivityState {
    pub fn from_status(status: &str) -> Self {
        match status.trim().to_ascii_lowercase().as_str() {
            "working" => Self::Working,
            "blocked" => Self::Blocked,
            "idle" => Self::Idle,
            "done" => Self::Done,
            "error" => Self::Error,
            _ => Self::Custom,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Working | Self::Custom => "working",
            Self::Blocked => "blocked",
            Self::Idle => "idle",
            Self::Done => "done",
            Self::Error => "error",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Idle | Self::Done | Self::Error)
    }

    /// Successful terminal outcomes, distinct from a failed run ending.
    pub const fn is_successful_terminal(self) -> bool {
        matches!(self, Self::Idle | Self::Done)
    }

    /// Shared ordering for the drawn rows and their keyboard activation targets.
    pub const fn presentation_rank(self, finished_unseen: bool) -> u8 {
        if finished_unseen && self.is_successful_terminal() {
            Self::Done.rank()
        } else {
            self.rank()
        }
    }

    /// How much an agent in this state wants the user, lowest first: the order the Agents tab and
    /// view list agents in.
    ///
    /// Unlike [`Self::presentation_rank`], which orders what is happening in one place, this
    /// orders what to look at next. An agent waiting on the user or failing comes first, then a
    /// finished run nobody has looked at, and only then one still working - which needs nothing
    /// from anyone yet. A finished run already seen and an idle agent trail.
    pub const fn attention_rank(self, finished_unseen: bool) -> u8 {
        match self {
            Self::Blocked => 0,
            Self::Error => 1,
            Self::Idle | Self::Done if finished_unseen => 2,
            Self::Working | Self::Custom => 3,
            Self::Done => 4,
            Self::Idle => 5,
        }
    }

    pub const fn rank(self) -> u8 {
        match self {
            Self::Blocked => 0,
            Self::Error => 1,
            Self::Working | Self::Custom => 2,
            Self::Done => 3,
            Self::Idle => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum AgentAuthority {
    Reported,
    Published,
    Detected,
}

/// What the session server tracks about one agent reference beyond the reference itself.
///
/// `completions` counts the runs this incarnation has finished: each time its state went from
/// working to idle or done. The server counts every transition it makes, so a client comparing
/// two values learns that a run ended even when it never saw the agent working - a monitor
/// polling between the two, or a client that was detached at the time. A new incarnation starts
/// again from zero, which is why the count is only meaningful beside the reference it belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct AgentRecord {
    pub reference: AgentRef,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub completions: u64,
}

impl From<AgentRef> for AgentRecord {
    /// A reference with no finished runs yet.
    fn from(reference: AgentRef) -> Self {
        Self {
            reference,
            completions: 0,
        }
    }
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// One automation-facing agent in a pane: an occupant of [`pane_occupants`] that is an agent,
/// paired with the reference that names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct AgentRuntime {
    pub reference: AgentRef,
    pub identity: AgentIdentity,
    pub label: String,
    pub state: AgentState,
    /// Why the agent is in its state, as the agent said it (`Permission required`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What the agent is working on, when it named its work: the title of the published row it
    /// is. Never a terminal title, which changes with every tool call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// Runs this agent has finished. See [`AgentRecord::completions`].
    #[serde(default, skip_serializing_if = "is_zero")]
    pub completions: u64,
    pub source: AgentAuthority,
}

/// One thing happening in a pane: the activity projection, which feeds the Activity sidebar.
///
/// Every occupant is an activity. Only one with an [`Self::agent`] identity is also an agent, and
/// only those reach the agent projection ([`effective_agent_runtimes`]): the Agents view,
/// `rozi agents`, waits, prompts, and the keep-awake policy. Agent identity comes from screen
/// detection, an integration report, or a publisher that declared itself
/// [`PublisherKind::Agent`](super::PublisherKind::Agent) - never from a row or a status merely
/// sharing a pane with one of those.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneOccupant {
    /// The published row this occupant is, or `None` for the pane's own agent or status.
    pub slot: Option<RowKey>,
    /// Who the agent is, for an agent occupant; `None` for an activity.
    pub agent: Option<AgentIdentity>,
    /// Display name: the agent's label, or an activity row's title. Empty for an untitled row and
    /// for the pane's own status, whose presenters fall back to the pane title.
    pub label: String,
    pub state: ActivityState,
    pub reason: Option<String>,
    pub source: AgentAuthority,
    pub program_status: Option<tui_lipan::ProgramStatusRecord>,
}

impl PaneOccupant {
    pub fn is_agent(&self) -> bool {
        self.agent.is_some()
    }
}

/// The pane's activity projection: its agents first, then its activity rows.
///
/// A pane's own `rozi status` with no agent to describe is an activity of its own. With a detected
/// or reported agent in the pane, the status is that agent's, exactly as it always was.
pub fn pane_occupants(runtime: &PaneRuntimeState) -> Vec<PaneOccupant> {
    let mut occupants = agent_occupants(runtime);
    if occupants.is_empty()
        && let Some(status) = &runtime.status
    {
        occupants.push(PaneOccupant {
            program_status: None,
            slot: None,
            agent: None,
            label: String::new(),
            state: ActivityState::from_status(&status.value),
            reason: status.reason.clone(),
            source: AgentAuthority::Reported,
        });
    }
    occupants.extend(
        runtime
            .rows
            .iter()
            .filter(|row| !row.is_agent())
            .map(|row| PaneOccupant {
                program_status: None,
                slot: Some(row.key()),
                agent: None,
                label: row.title.clone(),
                state: ActivityState::from_status(&row.status),
                reason: row.reason.clone(),
                source: AgentAuthority::Published,
            }),
    );
    occupants
}

/// The pane's agents, before references are attached.
///
/// An agent publisher's rows are the authoritative list while it has any, because one client can
/// run several conversations and only its rows list them all. The integration report still speaks
/// for the row whose native session it names: hooks see that conversation's state before any poll
/// does. A report naming no listed row describes a conversation the client no longer shows, so it
/// stays out of the list - it keeps its claim and its sequence fence all the same.
///
/// Activity rows never enter here, so a generic publisher can neither hide the pane's detected or
/// reported agent nor borrow its identity.
fn agent_occupants(runtime: &PaneRuntimeState) -> Vec<PaneOccupant> {
    let agent_rows: Vec<&PublishedRow> = runtime.rows.iter().filter(|row| row.is_agent()).collect();
    if !agent_rows.is_empty() {
        return agent_rows
            .into_iter()
            .map(|row| {
                let identity = runtime
                    .detected_agent
                    .as_ref()
                    .map(|detected| detected.agent.as_ref().clone())
                    .unwrap_or_else(|| AgentIdentity::new(&row.id, display_row_label(row)));
                let mut occupant = PaneOccupant {
                    program_status: None,
                    slot: Some(row.key()),
                    label: identity.label.clone(),
                    agent: Some(identity),
                    state: ActivityState::from_status(
                        AgentState::from_status(&row.status).as_str(),
                    ),
                    reason: row.reason.clone(),
                    // A hook report folded into the row is the row's word now.
                    source: if row.reported {
                        AgentAuthority::Reported
                    } else {
                        AgentAuthority::Published
                    },
                };
                if let Some(report) = row_integration(runtime.integration.as_deref(), row) {
                    occupant.state = ActivityState::from_status(report.state.as_str());
                    occupant.reason = report.reason.clone();
                    occupant.source = AgentAuthority::Reported;
                }
                occupant
            })
            .collect();
    }
    if let Some(report) = &runtime.integration {
        return vec![PaneOccupant {
            program_status: None,
            slot: None,
            label: report.identity.label.clone(),
            agent: Some(report.identity.clone()),
            state: ActivityState::from_status(report.state.as_str()),
            reason: report.reason.clone(),
            source: AgentAuthority::Reported,
        }];
    }
    let Some(detected) = runtime.detected_agent.as_ref() else {
        return Vec::new();
    };
    let Some((status, reason, source)) = effective_single_status(runtime) else {
        return Vec::new();
    };
    let identity = detected.agent.as_ref().clone();
    vec![PaneOccupant {
        program_status: None,
        slot: None,
        label: identity.label.clone(),
        agent: Some(identity),
        state: ActivityState::from_status(AgentState::from_status(status).as_str()),
        reason,
        source,
    }]
}

/// The identities the session server tracks references for: one per agent occupant, keyed by
/// slot. Activities get none, so nothing can wait on, prompt, or keep the machine awake for one.
pub fn agent_occupant_identities(runtime: &PaneRuntimeState) -> Vec<(Option<RowKey>, String)> {
    agent_occupants(runtime)
        .into_iter()
        .filter_map(|occupant| Some((occupant.slot, occupant.agent?.id)))
        .collect()
}

/// What an agent summary says about the pane's agents: who each is and what state it is in. Two
/// runtimes with equal signatures describe the same agents in the same states, whatever their
/// activities are doing.
pub fn agent_signature(runtime: &PaneRuntimeState) -> Vec<(Option<RowKey>, String, AgentState)> {
    agent_occupants(runtime)
        .into_iter()
        .filter_map(|occupant| {
            Some((
                occupant.slot,
                occupant.agent?.id,
                AgentState::from_status(occupant.state.as_str()),
            ))
        })
        .collect()
}

/// The agent projection: the pane's agent occupants, each with the reference that names it.
///
/// Every agent-facing surface reads this one function, so none of them filters activities out on
/// its own and none can disagree about what is an agent.
pub fn effective_agent_runtimes(
    runtime: &PaneRuntimeState,
    records: &[AgentRecord],
) -> Vec<AgentRuntime> {
    agent_occupants(runtime)
        .into_iter()
        .filter_map(|occupant| {
            let identity = occupant.agent?;
            let reference = match (&occupant.slot, &runtime.integration) {
                // The report alone speaks for the pane under the reference it was bound to.
                (None, Some(report)) => report.reference.clone(),
                (slot, _) => records
                    .iter()
                    .find(|record| record.reference.slot == *slot)?
                    .reference
                    .clone(),
            };
            let completions = records
                .iter()
                .find(|record| record.reference == reference)
                .map_or(0, |record| record.completions);
            let task = occupant_task(&runtime.rows, occupant.slot.as_ref(), &occupant.label);
            Some(AgentRuntime {
                reference,
                identity,
                label: occupant.label,
                state: AgentState::from_status(occupant.state.as_str()),
                reason: occupant.reason,
                task,
                completions,
                source: occupant.source,
            })
        })
        .collect()
}

/// What one agent occupant is working on: its published row's title, when it is a row that has
/// one saying more than the agent's own name.
pub fn occupant_task(rows: &[PublishedRow], slot: Option<&RowKey>, label: &str) -> Option<String> {
    let slot = slot?;
    let row = rows.iter().find(|row| row.has_key(slot))?;
    let title = row.title.trim();
    (!title.is_empty() && !title.eq_ignore_ascii_case(label.trim())).then(|| title.to_string())
}

fn effective_single_status(
    runtime: &PaneRuntimeState,
) -> Option<(&str, Option<String>, AgentAuthority)> {
    let detected = runtime.detected_agent.as_ref();
    let reported = runtime.status.as_ref();
    let detected_status = detected.map(detected_agent_status);
    if detected_status == Some(pane_status::BLOCKED)
        && reported.is_some_and(|status| super::status_is_quiescent(Some(&status.value)))
    {
        return Some((pane_status::BLOCKED, None, AgentAuthority::Detected));
    }
    match reported {
        Some(status) => Some((
            status.value.as_str(),
            status.reason.clone(),
            AgentAuthority::Reported,
        )),
        None => {
            detected_status.map(|detected_status| (detected_status, None, AgentAuthority::Detected))
        }
    }
}

/// The session a hook report has to name to speak for `row`: the id its process runs under now,
/// which is its native session unless the conversation moved to another process.
pub fn row_reporting_session(row: &PublishedRow) -> Option<&str> {
    row.live_session
        .as_deref()
        .or(row.native_session.as_deref())
}

/// The integration report that speaks for `row`: the pane's live report, when the row belongs to
/// an agent publisher and the report names its reporting session. An activity row never takes on
/// an agent's report, whatever it names.
fn row_integration<'a>(
    integration: Option<&'a AgentIntegrationReport>,
    row: &PublishedRow,
) -> Option<&'a AgentIntegrationReport> {
    integration.filter(|report| {
        row.is_agent()
            && report.native_session.is_some()
            && report.native_session.as_deref() == row_reporting_session(row)
    })
}

/// Which agent a bare pane id names.
#[derive(Debug, PartialEq)]
pub enum PaneAgent {
    Missing,
    One(Box<AgentRuntime>),
    /// Published rows, none of them on screen.
    NoneOnScreen,
}

/// The agent a pane id names among `runtimes`, the pane's [`effective_agent_runtimes`]: the pane's
/// own, or of its published rows the one on screen. A row that is not on screen - a background
/// conversation a client lists beside the one it shows - is reached by its exact reference, even
/// when it is the only row.
///
/// Each publisher may mark a row active, so a pane shared by several windows can have several on
/// screen. The pane id names the first, the same [`active_agent_row`] a restore resumes; `runtimes`
/// are already in publisher order.
pub fn pane_agent(runtimes: Vec<AgentRuntime>, rows: &[PublishedRow]) -> PaneAgent {
    if runtimes.is_empty() {
        return PaneAgent::Missing;
    }
    runtimes
        .into_iter()
        .find(|runtime| {
            runtime
                .reference
                .slot
                .as_ref()
                .is_none_or(|slot| rows.iter().any(|row| row.has_key(slot) && row.active))
        })
        .map_or(PaneAgent::NoneOnScreen, |runtime| {
            PaneAgent::One(Box::new(runtime))
        })
}

/// `row` as everything downstream of the protocol must present it: with the status and reason of
/// the integration report that speaks for it, when one does.
///
/// The one merge rule, shared by [`effective_agent_runtimes`], pane chrome, the server's run
/// clocks, and the client's rows and finish edges, so no surface shows the publisher's word where
/// another shows the hooks'.
pub fn effective_row(
    row: &PublishedRow,
    integration: Option<&AgentIntegrationReport>,
) -> PublishedRow {
    match row_integration(integration, row) {
        Some(report) => PublishedRow {
            status: report.state.as_str().to_string(),
            reason: report.reason.clone(),
            ..row.clone()
        },
        None => row.clone(),
    }
}

/// The native conversation one agent occupant stands for: its row's own when it is a published
/// row, the pane's integration report's otherwise.
pub fn occupant_native_session(
    rows: &[PublishedRow],
    integration: Option<&AgentIntegrationReport>,
    slot: Option<&RowKey>,
) -> Option<String> {
    match slot {
        Some(slot) => rows
            .iter()
            .find(|row| row.has_key(slot))
            .and_then(|row| row.native_session.clone()),
        None => integration.and_then(|report| report.native_session.clone()),
    }
}

/// The directory one occupant works in: its row's own when it is a published row that names one,
/// the pane's otherwise.
pub fn occupant_cwd(
    rows: &[PublishedRow],
    slot: Option<&RowKey>,
    pane_cwd: Option<String>,
) -> Option<String> {
    slot.and_then(|slot| rows.iter().find(|row| row.has_key(slot)))
        .and_then(|row| row.cwd.clone())
        .or(pane_cwd)
}

/// The pane's rows with each one's status as [`effective_agent_runtimes`] reports it.
pub fn rows_with_integration(runtime: &PaneRuntimeState) -> Vec<PublishedRow> {
    runtime
        .rows
        .iter()
        .map(|row| effective_row(row, runtime.integration.as_deref()))
        .collect()
}

/// The pane's agent rows, merged with the integration report: what an agent publisher says the
/// pane's one agent state is, and what replaces screen detection while it says anything.
pub fn agent_rows_with_integration(runtime: &PaneRuntimeState) -> Vec<PublishedRow> {
    runtime
        .rows
        .iter()
        .filter(|row| row.is_agent())
        .map(|row| effective_row(row, runtime.integration.as_deref()))
        .collect()
}

/// The pane's active agent row, for the one conversation a restored pane resumes.
///
/// "Only one `active: true` row" is a rule per publisher, so a pane can have several active rows.
/// The pane-level active row is the first active row of an agent publisher, in the order the
/// pane's rows are kept: publishers in the order they first published, each one's rows in its own
/// order. Activity rows are never resume candidates.
pub fn active_agent_row(rows: &[PublishedRow]) -> Option<&PublishedRow> {
    rows.iter().find(|row| row.is_agent() && row.active)
}

fn display_row_label(row: &PublishedRow) -> &str {
    if row.title.is_empty() {
        &row.id
    } else {
        &row.title
    }
}

pub fn effective_agent_state(
    reported: Option<&PaneStatus>,
    detected_status: Option<&str>,
) -> Option<AgentState> {
    let status = match (reported, detected_status) {
        (Some(reported), Some(pane_status::BLOCKED))
            if super::status_is_quiescent(Some(&reported.value)) =>
        {
            pane_status::BLOCKED
        }
        (Some(reported), _) => &reported.value,
        (None, Some(detected)) => detected,
        (None, None) => return None,
    };
    Some(AgentState::from_status(status))
}

pub fn effective_semantic_agent_state(
    integration: Option<&AgentIntegrationReport>,
    reported: Option<&PaneStatus>,
    detected: Option<&DetectedAgent>,
) -> Option<AgentState> {
    integration
        .map(|integration| integration.state)
        .or_else(|| effective_agent_state(reported, detected.map(detected_agent_status)))
}

/// Activity display aggregation only. No agent identity or parent state is inferred.
pub fn program_status_summary(
    snapshot: &tui_lipan::ProgramStatusSnapshot,
) -> Option<&tui_lipan::ProgramStatusRecord> {
    snapshot.records.iter().min_by_key(|record| {
        (
            match record.state {
                tui_lipan::ProgramStatusState::Blocked => ActivityState::Blocked,
                tui_lipan::ProgramStatusState::Error => ActivityState::Error,
                tui_lipan::ProgramStatusState::Working => ActivityState::Working,
                tui_lipan::ProgramStatusState::Done => ActivityState::Done,
                tui_lipan::ProgramStatusState::Idle => ActivityState::Idle,
            }
            .rank(),
            std::cmp::Reverse(record.updated),
            record.id.as_ref(),
        )
    })
}

/// Disarm invisible formatting when protocol text is shown outside the terminal grid.
pub fn program_status_display(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(*c,
                    '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' |
                    '\u{2060}'..='\u{206f}' | '\u{feff}'
                )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::protocol::{DetectedAgent, DetectedAgentState, PaneRef, SessionInstanceId};

    fn reference(slot: Option<&str>, incarnation: u64) -> AgentRef {
        AgentRef {
            pane: PaneRef {
                session_instance: SessionInstanceId::for_test("server"),
                pane_id: 3,
                generation: 7,
            },
            slot: slot.map(|id| RowKey::new("sessions", id)),
            incarnation,
        }
    }

    fn key(id: &str) -> RowKey {
        RowKey::new("sessions", id)
    }

    fn runtime(slot: Option<&str>) -> AgentRuntime {
        AgentRuntime {
            reference: reference(slot, 1),
            label: "Claude Code".into(),
            identity: AgentIdentity::new("claude", "Claude Code"),
            state: AgentState::Idle,
            reason: None,
            task: None,
            completions: 0,
            source: AgentAuthority::Published,
        }
    }

    fn listed(id: &str, active: bool) -> PublishedRow {
        PublishedRow {
            publisher: "sessions".into(),
            kind: crate::session::protocol::PublisherKind::Agent,
            id: id.into(),
            title: id.into(),
            status: "idle".into(),
            reason: None,
            active,
            work_started_at: None,
            cwd: None,
            project: None,
            native_session: None,
            live_session: None,
            reported: false,
        }
    }

    #[test]
    fn generic_error_keeps_agent_api_compatibility() {
        let runtime = PaneRuntimeState {
            status: Some(PaneStatus {
                value: "error".into(),
                reason: None,
                set_at: 1,
            }),
            ..Default::default()
        };
        assert_eq!(pane_occupants(&runtime)[0].state, ActivityState::Error);
        assert_eq!(AgentState::from_status("error"), AgentState::Working);
        assert!(!AgentState::from_status("error").is_quiescent());
        assert_eq!(ActivityState::from_status("custom"), ActivityState::Custom);
        assert!(!ActivityState::Custom.is_terminal());
    }

    #[test]
    fn a_pane_id_names_its_own_agent_or_the_row_on_screen() {
        assert_eq!(pane_agent(Vec::new(), &[]), PaneAgent::Missing);
        assert_eq!(
            pane_agent(vec![runtime(None)], &[]),
            PaneAgent::One(Box::new(runtime(None)))
        );
        let rows = [listed("shown", true), listed("background", false)];
        assert_eq!(
            pane_agent(
                vec![runtime(Some("shown")), runtime(Some("background"))],
                &rows
            ),
            PaneAgent::One(Box::new(runtime(Some("shown"))))
        );
        // A lone background row is still not the agent on screen.
        assert_eq!(
            pane_agent(
                vec![runtime(Some("background"))],
                &[listed("background", false)]
            ),
            PaneAgent::NoneOnScreen
        );
    }

    #[test]
    fn a_pane_id_names_the_first_active_row_when_several_publishers_show_one() {
        let mut first = listed("first", true);
        first.publisher = "1/sessions".into();
        let mut second = listed("second", true);
        second.publisher = "2/sessions".into();
        let runtimes: Vec<_> = [&first, &second]
            .into_iter()
            .map(|row| {
                let mut runtime = runtime(None);
                runtime.reference.slot = Some(row.key());
                runtime
            })
            .collect();
        let expected = runtimes[0].clone();
        let rows = [first, second];
        assert_eq!(
            active_agent_row(&rows).map(|row| row.id.as_str()),
            Some("first")
        );
        assert_eq!(
            pane_agent(runtimes, &rows),
            PaneAgent::One(Box::new(expected))
        );
    }

    #[test]
    fn reported_status_wins_except_for_a_detected_block() {
        let runtime = PaneRuntimeState {
            status: Some(PaneStatus {
                value: "done".into(),
                reason: Some("finished".into()),
                set_at: 42,
            }),
            detected_agent: Some(DetectedAgent {
                agent: AgentIdentity::new("claude", "Claude Code").into(),
                state: DetectedAgentState::Blocked,
            }),
            ..PaneRuntimeState::default()
        };
        let projected = effective_agent_runtimes(&runtime, &[reference(None, 1).into()]);
        assert_eq!(projected[0].state, AgentState::Blocked);
        assert_eq!(projected[0].source, AgentAuthority::Detected);
        assert_eq!(projected[0].reason, None);
    }

    #[test]
    fn published_rows_project_independently() {
        let runtime = PaneRuntimeState {
            rows: vec![
                PublishedRow {
                    publisher: "sessions".into(),
                    kind: crate::session::protocol::PublisherKind::Agent,
                    live_session: None,
                    reported: false,
                    id: "one".into(),
                    title: "First".into(),
                    status: "working".into(),
                    reason: None,
                    active: true,
                    work_started_at: Some(10),
                    cwd: None,
                    project: None,
                    native_session: None,
                },
                PublishedRow {
                    publisher: "sessions".into(),
                    kind: crate::session::protocol::PublisherKind::Agent,
                    live_session: None,
                    reported: false,
                    id: "two".into(),
                    title: "Second".into(),
                    status: "done".into(),
                    reason: Some("complete".into()),
                    active: false,
                    work_started_at: None,
                    cwd: None,
                    project: None,
                    native_session: None,
                },
            ],
            ..PaneRuntimeState::default()
        };
        let projected = effective_agent_runtimes(
            &runtime,
            &[
                reference(Some("one"), 1).into(),
                reference(Some("two"), 2).into(),
            ],
        );
        assert_eq!(projected.len(), 2);
        assert_eq!(projected[0].state, AgentState::Working);
        assert_eq!(projected[1].state, AgentState::Done);
        assert_eq!(projected[1].reason.as_deref(), Some("complete"));
    }

    fn session_row(id: &str, status: &str, native: Option<&str>) -> PublishedRow {
        PublishedRow {
            publisher: "sessions".into(),
            kind: crate::session::protocol::PublisherKind::Agent,
            live_session: None,
            reported: false,
            id: id.into(),
            title: id.into(),
            status: status.into(),
            reason: None,
            active: false,
            work_started_at: None,
            cwd: None,
            project: None,
            native_session: native.map(str::to_string),
        }
    }

    fn hook_report(native: &str, state: AgentState) -> Box<super::super::AgentIntegrationReport> {
        Box::new(super::super::AgentIntegrationReport {
            integration: "hook-abc".into(),
            identity: AgentIdentity::new("claude", "Claude Code"),
            reference: reference(None, 9),
            state,
            reason: Some("approval".into()),
            native_session: Some(native.into()),
            seq: 4,
            reported_at_unix_ms: 10,
        })
    }

    /// One client running several conversations: every conversation stays listed, and the hooks'
    /// report drives only the row for the conversation it follows.
    #[test]
    fn an_integration_report_speaks_for_its_own_row_and_leaves_the_rest_listed() {
        let runtime = PaneRuntimeState {
            rows: vec![
                session_row("one", "idle", Some("abc")),
                session_row("two", "working", Some("def")),
            ],
            integration: Some(hook_report("abc", AgentState::Blocked)),
            ..PaneRuntimeState::default()
        };
        let references = [
            reference(Some("one"), 1).into(),
            reference(Some("two"), 2).into(),
        ];
        let projected = effective_agent_runtimes(&runtime, &references);
        assert_eq!(projected.len(), 2);
        assert_eq!(projected[0].reference.slot, Some(key("one")));
        assert_eq!(projected[0].state, AgentState::Blocked);
        assert_eq!(projected[0].reason.as_deref(), Some("approval"));
        assert_eq!(projected[0].source, AgentAuthority::Reported);
        assert_eq!(projected[1].state, AgentState::Working);
        assert_eq!(projected[1].source, AgentAuthority::Published);

        // The pane's one state follows the same merge: the blocked conversation outranks.
        assert_eq!(
            crate::session::protocol::aggregate_row_state(&rows_with_integration(&runtime)),
            Some(crate::session::protocol::DetectedAgentState::Blocked)
        );
        assert_eq!(
            occupant_native_session(
                &runtime.rows,
                runtime.integration.as_deref(),
                Some(&key("two"))
            )
            .as_deref(),
            Some("def")
        );
        assert_eq!(
            occupant_native_session(&runtime.rows, runtime.integration.as_deref(), None).as_deref(),
            Some("abc")
        );
    }

    /// After the client switches conversation its hooks can go quiet, leaving a report about a
    /// conversation no row lists. That report must neither hide the rows nor add a phantom one.
    #[test]
    fn a_report_for_an_unlisted_conversation_is_not_shown_beside_rows() {
        let runtime = PaneRuntimeState {
            rows: vec![session_row("one", "idle", Some("def"))],
            integration: Some(hook_report("gone", AgentState::Working)),
            ..PaneRuntimeState::default()
        };
        let projected = effective_agent_runtimes(&runtime, &[reference(Some("one"), 1).into()]);
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].state, AgentState::Idle);
        assert_eq!(projected[0].source, AgentAuthority::Published);
        // A row without a native session never matches, even against a report without one.
        let unnamed = PaneRuntimeState {
            rows: vec![session_row("one", "idle", None)],
            integration: Some(Box::new(super::super::AgentIntegrationReport {
                native_session: None,
                ..*hook_report("x", AgentState::Working)
            })),
            ..PaneRuntimeState::default()
        };
        assert_eq!(
            effective_agent_runtimes(&unnamed, &[reference(Some("one"), 1).into()])[0].state,
            AgentState::Idle
        );
    }

    #[test]
    fn an_integration_report_alone_speaks_for_the_pane() {
        let runtime = PaneRuntimeState {
            integration: Some(Box::new(super::super::AgentIntegrationReport {
                integration: "hook-abc".into(),
                identity: AgentIdentity::new("claude", "Claude Code"),
                reference: reference(None, 2),
                state: AgentState::Blocked,
                reason: Some("approval".into()),
                native_session: Some("abc".into()),
                seq: 4,
                reported_at_unix_ms: 10,
            })),
            ..PaneRuntimeState::default()
        };

        let projected = effective_agent_runtimes(&runtime, &[reference(None, 2).into()]);
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].state, AgentState::Blocked);
        assert_eq!(projected[0].source, AgentAuthority::Reported);
    }

    fn activity_row(publisher: &str, id: &str, status: &str) -> PublishedRow {
        PublishedRow {
            publisher: publisher.into(),
            kind: crate::session::protocol::PublisherKind::Activity,
            ..session_row(id, status, None)
        }
    }

    fn codex() -> Option<DetectedAgent> {
        Some(DetectedAgent {
            agent: AgentIdentity::new("codex", "Codex").into(),
            state: DetectedAgentState::Working,
        })
    }

    /// A generic PR publisher beside a detected Codex: both are activities, only Codex is an
    /// agent, and the PR rows never take on Codex's identity.
    #[test]
    fn a_generic_publisher_beside_a_detected_agent_is_only_an_activity() {
        let runtime = PaneRuntimeState {
            detected_agent: codex(),
            rows: vec![
                activity_row("publish:prs", "pr-1", "working"),
                activity_row("publish:prs", "pr-2", "idle"),
            ],
            ..PaneRuntimeState::default()
        };
        let occupants = pane_occupants(&runtime);
        assert_eq!(occupants.len(), 3, "Codex and both PR rows are activities");
        assert_eq!(occupants[0].slot, None);
        assert_eq!(
            occupants[0].agent.as_ref().map(|agent| agent.id.as_str()),
            Some("codex")
        );
        assert!(
            occupants[1..]
                .iter()
                .all(|occupant| occupant.agent.is_none())
        );
        assert_eq!(occupants[1].label, "pr-1");

        let agents = effective_agent_runtimes(&runtime, &[reference(None, 1).into()]);
        assert_eq!(agents.len(), 1, "only Codex is an agent");
        assert_eq!(agents[0].identity.id, "codex");
        assert_eq!(agents[0].state, AgentState::Working);
        assert_eq!(
            agent_occupant_identities(&runtime),
            [(None, "codex".to_string())]
        );
    }

    /// An agent publisher with no screen detection behind it is still an agent.
    #[test]
    fn a_declared_agent_publisher_is_an_agent_without_detection() {
        let runtime = PaneRuntimeState {
            rows: vec![session_row("api-run", "working", None)],
            ..PaneRuntimeState::default()
        };
        let agents = effective_agent_runtimes(&runtime, &[reference(Some("api-run"), 1).into()]);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].identity.id, "api-run");
        assert_eq!(agents[0].state, AgentState::Working);
    }

    /// A plain `rozi status`, and a generic publisher, in a pane nothing detected: both are
    /// activities, and neither is an agent.
    #[test]
    fn a_status_or_generic_publisher_alone_is_never_an_agent() {
        let runtime = PaneRuntimeState {
            status: Some(PaneStatus {
                value: "working".into(),
                reason: Some("cargo build".into()),
                set_at: 1,
            }),
            rows: vec![activity_row("publish:default", "build", "working")],
            ..PaneRuntimeState::default()
        };
        let occupants = pane_occupants(&runtime);
        assert_eq!(occupants.len(), 2);
        assert!(occupants.iter().all(|occupant| !occupant.is_agent()));
        assert_eq!(occupants[0].reason.as_deref(), Some("cargo build"));
        assert!(effective_agent_runtimes(&runtime, &[reference(None, 1).into()]).is_empty());
        assert!(agent_occupant_identities(&runtime).is_empty());
    }

    /// An agent publisher's rows are the pane's agents; a generic publisher beside it neither
    /// adds agents nor takes the hooks' report, even for a matching native session.
    #[test]
    fn an_integration_report_never_speaks_for_an_activity_row() {
        let mut activity = activity_row("publish:prs", "one", "idle");
        activity.native_session = Some("abc".into());
        let runtime = PaneRuntimeState {
            detected_agent: codex(),
            rows: vec![activity.clone(), session_row("one", "idle", Some("abc"))],
            integration: Some(hook_report("abc", AgentState::Blocked)),
            ..PaneRuntimeState::default()
        };
        assert_eq!(
            effective_row(&activity, runtime.integration.as_deref()),
            activity
        );
        let agents = effective_agent_runtimes(&runtime, &[reference(Some("one"), 1).into()]);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].reference.slot, Some(key("one")));
        assert_eq!(agents[0].state, AgentState::Blocked);
        assert_eq!(agents[0].identity.id, "codex");
        assert_eq!(
            aggregate_rows(&agent_rows_with_integration(&runtime)),
            Some(DetectedAgentState::Blocked)
        );
    }

    #[test]
    fn only_an_agent_row_is_the_panes_active_row() {
        let mut activity = activity_row("publish:prs", "pr", "idle");
        activity.active = true;
        activity.native_session = Some("not-a-conversation".into());
        let mut agent = session_row("conv", "idle", Some("abc"));
        agent.active = true;
        assert_eq!(active_agent_row(std::slice::from_ref(&activity)), None);
        assert_eq!(
            active_agent_row(&[activity, agent.clone()]).map(|row| row.id.as_str()),
            Some("conv")
        );
    }

    fn aggregate_rows(rows: &[PublishedRow]) -> Option<DetectedAgentState> {
        crate::session::protocol::aggregate_row_state(rows)
    }
}
