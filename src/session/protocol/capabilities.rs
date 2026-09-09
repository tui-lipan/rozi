//! Optional wire behavior, negotiated separately from the compatible base protocol range.
//!
//! Two questions, answered independently:
//!
//! ```text
//! protocol range  can these two builds talk at all?
//! capabilities    which optional work may this connection ask for?
//! ```
//!
//! Rozi's session servers are deliberately persistent, so a new client must not imply restarting a
//! remote server and disturbing the processes inside it. Keeping the second question separate is
//! what lets one feature go missing — an older server, a peer that declined the work — while the
//! connection stays fully usable for everything else.
//!
//! Names are added only when compatibility genuinely turns on one: a capability describes protocol
//! behavior that evolves on its own, never a UI feature. Adding one for every toggle would recreate
//! the coordinated-upgrade problem in a longer list.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Periodic server-side runtime metrics, on request.
pub const RUNTIME_METRICS: &str = "runtime-metrics";
/// Semantic agent state in a [`super::ServerMessage::SessionInfo`] reply, for a host monitor that
/// never attaches. See [`super::AgentSummary`].
pub const AGENT_SUMMARIES: &str = "agent-summaries";

/// An open set of capability names. Unknown ones deserialize and are dropped during negotiation,
/// so a future peer offering more than this build knows is downgraded rather than refused.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Capabilities(pub BTreeSet<String>);

impl Capabilities {
    /// Everything this build can do.
    pub fn current() -> Self {
        Self(
            [RUNTIME_METRICS, AGENT_SUMMARIES]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        )
    }

    /// What a peer that sends no capability field is taken to mean.
    ///
    /// Protocol 5 already carried runtime metrics, before there was any way to advertise them, so
    /// an omitted field has to keep meaning "metrics, and nothing added since". This is why the
    /// field is `Option` rather than a set that defaults to empty: omission and an explicit empty
    /// set are different claims, and only the second one declines the metrics.
    pub fn legacy() -> Self {
        Self([RUNTIME_METRICS.to_owned()].into_iter().collect())
    }

    /// What both ends agreed to: the intersection of this build's set with the peer's.
    pub fn negotiated(peer: Option<&Self>) -> Self {
        let local = Self::current();
        let legacy = Self::legacy();
        let peer = peer.unwrap_or(&legacy);
        Self(local.0.intersection(&peer.0).cloned().collect())
    }

    pub fn supports(&self, capability: &str) -> bool {
        self.0.contains(capability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omission_empty_and_unknown_capabilities_have_distinct_meanings() {
        assert_eq!(Capabilities::negotiated(None), Capabilities::legacy());
        assert_eq!(
            Capabilities::negotiated(Some(&Capabilities::default())),
            Capabilities::default()
        );
        let offered: Capabilities =
            serde_json::from_str(r#"["runtime-metrics","future-feature"]"#).unwrap();
        assert_eq!(
            Capabilities::negotiated(Some(&offered)),
            Capabilities::legacy()
        );
        assert!(!Capabilities::negotiated(None).supports(AGENT_SUMMARIES));
    }
}
