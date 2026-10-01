use serde::{Deserialize, Serialize};

use super::{
    AgentIdentity, AgentIntegrationReport, AgentRef, DetectedAgent, PaneRuntimeState, PaneStatus,
    PublishedRow, detected_agent_status, pane_status,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum AgentAuthority {
    Reported,
    Published,
    Detected,
}

/// One automation-facing semantic occupant of a pane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct AgentRuntime {
    pub reference: AgentRef,
    pub identity: AgentIdentity,
    pub label: String,
    pub state: AgentState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub source: AgentAuthority,
}

/// Resolve detection, explicit status, and published rows into the records automation should use.
pub fn effective_agent_runtimes(
    runtime: &PaneRuntimeState,
    references: &[AgentRef],
) -> Vec<AgentRuntime> {
    // Published rows win over a live integration report, because one client can run several
    // conversations and only the rows list them all. The report still speaks for the row whose
    // native session it names: hooks see that conversation's state before any poll does. A
    // report naming no listed row describes a conversation the client no longer shows, so it
    // stays out of the list - it keeps its claim and its sequence fence all the same.
    if !runtime.rows.is_empty() {
        return runtime
            .rows
            .iter()
            .filter_map(|row| {
                let mut published = published_runtime(runtime, references, row)?;
                if let Some(report) = row_integration(runtime.integration.as_deref(), row) {
                    published.state = report.state;
                    published.reason = report.reason.clone();
                    published.source = AgentAuthority::Reported;
                }
                Some(published)
            })
            .collect();
    }
    if let Some(report) = &runtime.integration {
        return vec![AgentRuntime {
            reference: report.reference.clone(),
            label: report.identity.label.clone(),
            identity: report.identity.clone(),
            state: report.state,
            reason: report.reason.clone(),
            source: AgentAuthority::Reported,
        }];
    }
    let Some(reference) = references
        .iter()
        .find(|reference| reference.slot.is_none())
        .cloned()
    else {
        return Vec::new();
    };
    let Some((status, reason, source)) = effective_single_status(runtime) else {
        return Vec::new();
    };
    let identity = runtime
        .detected_agent
        .as_ref()
        .map(|detected| detected.agent.as_ref().clone())
        .unwrap_or_else(|| AgentIdentity::new("reported", "Agent"));
    vec![AgentRuntime {
        reference,
        label: identity.label.clone(),
        identity,
        state: AgentState::from_status(status),
        reason,
        source,
    }]
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

/// The integration report that speaks for `row`: the pane's live report, when it names the row's
/// native session.
fn row_integration<'a>(
    integration: Option<&'a AgentIntegrationReport>,
    row: &PublishedRow,
) -> Option<&'a AgentIntegrationReport> {
    integration.filter(|report| {
        report.native_session.is_some() && report.native_session == row.native_session
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
    slot: Option<&str>,
) -> Option<String> {
    match slot {
        Some(slot) => rows
            .iter()
            .find(|row| row.id == slot)
            .and_then(|row| row.native_session.clone()),
        None => integration.and_then(|report| report.native_session.clone()),
    }
}

/// The directory one agent occupant works in: its row's own when it is a published row that names
/// one, the pane's otherwise.
pub fn occupant_cwd(
    rows: &[PublishedRow],
    slot: Option<&str>,
    pane_cwd: Option<String>,
) -> Option<String> {
    slot.and_then(|slot| rows.iter().find(|row| row.id == slot))
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

fn published_runtime(
    runtime: &PaneRuntimeState,
    references: &[AgentRef],
    row: &PublishedRow,
) -> Option<AgentRuntime> {
    let reference = references
        .iter()
        .find(|reference| reference.slot.as_deref() == Some(row.id.as_str()))?
        .clone();
    let identity = runtime
        .detected_agent
        .as_ref()
        .map(|detected| detected.agent.as_ref().clone())
        .unwrap_or_else(|| AgentIdentity::new(&row.id, display_row_label(row)));
    Some(AgentRuntime {
        reference,
        label: identity.label.clone(),
        identity,
        state: AgentState::from_status(&row.status),
        reason: row.reason.clone(),
        source: AgentAuthority::Published,
    })
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
            slot: slot.map(str::to_string),
            incarnation,
        }
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
        let projected = effective_agent_runtimes(&runtime, &[reference(None, 1)]);
        assert_eq!(projected[0].state, AgentState::Blocked);
        assert_eq!(projected[0].source, AgentAuthority::Detected);
        assert_eq!(projected[0].reason, None);
    }

    #[test]
    fn published_rows_project_independently() {
        let runtime = PaneRuntimeState {
            rows: vec![
                PublishedRow {
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
            &[reference(Some("one"), 1), reference(Some("two"), 2)],
        );
        assert_eq!(projected.len(), 2);
        assert_eq!(projected[0].state, AgentState::Working);
        assert_eq!(projected[1].state, AgentState::Done);
        assert_eq!(projected[1].reason.as_deref(), Some("complete"));
    }

    fn session_row(id: &str, status: &str, native: Option<&str>) -> PublishedRow {
        PublishedRow {
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
        let references = [reference(Some("one"), 1), reference(Some("two"), 2)];
        let projected = effective_agent_runtimes(&runtime, &references);
        assert_eq!(projected.len(), 2);
        assert_eq!(projected[0].reference.slot.as_deref(), Some("one"));
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
            occupant_native_session(&runtime.rows, runtime.integration.as_deref(), Some("two"))
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
        let projected = effective_agent_runtimes(&runtime, &[reference(Some("one"), 1)]);
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
            effective_agent_runtimes(&unnamed, &[reference(Some("one"), 1)])[0].state,
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

        let projected = effective_agent_runtimes(&runtime, &[reference(None, 2)]);
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].state, AgentState::Blocked);
        assert_eq!(projected[0].source, AgentAuthority::Reported);
    }
}
