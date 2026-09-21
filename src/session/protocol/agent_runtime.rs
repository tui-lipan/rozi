use serde::{Deserialize, Serialize};

use super::{
    AgentIdentity, AgentRef, PaneRuntimeState, PaneStatus, PublishedRow, detected_agent_status,
    pane_status,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
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
pub enum AgentAuthority {
    Reported,
    Published,
    Detected,
}

/// One automation-facing semantic occupant of a pane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    if !runtime.rows.is_empty() {
        return runtime
            .rows
            .iter()
            .filter_map(|row| published_runtime(runtime, references, row))
            .collect();
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
                },
                PublishedRow {
                    id: "two".into(),
                    title: "Second".into(),
                    status: "done".into(),
                    reason: Some("complete".into()),
                    active: false,
                    work_started_at: None,
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

    #[test]
    fn integration_report_has_authority_over_published_rows() {
        let runtime = PaneRuntimeState {
            rows: vec![PublishedRow {
                id: "background".into(),
                title: "Background".into(),
                status: "working".into(),
                reason: None,
                active: true,
                work_started_at: None,
            }],
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
