//! Optional wire behavior, negotiated separately from the wire generation.
//!
//! Two questions, answered independently:
//!
//! ```text
//! generation      can these two builds talk at all?
//! capabilities    which messages, values, and meanings may this connection use?
//! ```
//!
//! Rozi's session servers are deliberately persistent, so a new client must not imply restarting a
//! remote server and disturbing the processes inside it. Keeping the second question separate is
//! what lets one feature go missing on an older peer while the connection stays fully usable for
//! everything else.
//!
//! Every wire change that the generation rules allow without a bump is a capability: a new message
//! variant, a new frame kind, a new value in a closed vocabulary, or a new meaning for an existing
//! field. `dev/wire-compatibility.md` is the full rulebook. On the wire a capability set stays an
//! open set of strings, so a name this build has never heard of is dropped during negotiation
//! rather than refused. In code every name is a [`Capability`], so names cannot drift and a `match`
//! lists all of them.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// One optional piece of session wire behavior, named by [`Capability::as_str`] on the wire.
///
/// Add a variant, never rename one: the wire name is a contract with every released peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    /// Periodic server-side runtime metrics, on request.
    RuntimeMetrics,
    /// Semantic agent state in a [`super::ServerMessage::SessionInfo`] reply, for a host monitor
    /// that never attaches. See [`super::AgentSummary`].
    AgentSummaries,
    /// The checkouts each session works in, in the same [`super::ServerMessage::SessionInfo`]
    /// reply. See [`super::ProjectSummary`].
    ProjectSummaries,
    /// The client applies Kitty root-frame edits (`a=f,r=1`) in pane output, so the server may
    /// relay a replaced image as edits of what changed. A client without it would keep the
    /// previous frame.
    GraphicsRootFrameEdits,
}

impl Capability {
    /// Every capability this build knows, in wire-name order.
    pub const ALL: [Capability; 4] = [
        Capability::AgentSummaries,
        Capability::GraphicsRootFrameEdits,
        Capability::ProjectSummaries,
        Capability::RuntimeMetrics,
    ];

    /// The name this capability travels under.
    pub const fn as_str(self) -> &'static str {
        match self {
            Capability::RuntimeMetrics => "runtime-metrics",
            Capability::AgentSummaries => "agent-summaries",
            Capability::ProjectSummaries => "project-summaries",
            Capability::GraphicsRootFrameEdits => "graphics-root-frame-edits",
        }
    }

    /// First release that offers this optional session behavior.
    pub const fn introduced_in(self) -> &'static str {
        match self {
            Self::RuntimeMetrics
            | Self::AgentSummaries
            | Self::ProjectSummaries
            | Self::GraphicsRootFrameEdits => "0.0.44",
        }
    }

    /// The capability a wire name stands for, or `None` for a name this build does not know.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|cap| cap.as_str() == name)
    }
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An open set of capability names. Unknown ones deserialize and are dropped during negotiation,
/// so a future peer offering more than this build knows is downgraded rather than refused.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct Capabilities(pub BTreeSet<String>);

impl Capabilities {
    /// Everything this build can do.
    pub fn current() -> Self {
        Self::from_iter(Capability::ALL)
    }

    /// What both ends agreed to: the intersection of this build's set with the peer's.
    ///
    /// A peer that sends no set gets none of the optional work. Every build of this generation
    /// sends one, so a missing set only comes from a peer that declines everything.
    pub fn negotiated(peer: Option<&Self>) -> Self {
        let Some(peer) = peer else {
            return Self::default();
        };
        Self::from_iter(
            Capability::ALL
                .into_iter()
                .filter(|cap| peer.0.contains(cap.as_str())),
        )
    }

    pub fn supports(&self, capability: Capability) -> bool {
        self.0.contains(capability.as_str())
    }

    /// Drop one capability, as a peer that lacks it would.
    pub fn without(mut self, capability: Capability) -> Self {
        self.0.remove(capability.as_str());
        self
    }
}

/// A value added to a closed wire vocabulary within the current generation.
///
/// An older peer decodes it as its vocabulary's `unknown`, but a sender still checks `capability`
/// before sending it, so a peer that negotiated it shows the real value. The wire classifier
/// requires every value added since the generation's frozen schema to be listed in
/// [`GATED_VOCABULARY`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GatedValue {
    /// The schema definition the value belongs to, such as `AgentState`.
    pub vocabulary: &'static str,
    /// The value as it travels on the wire.
    pub value: &'static str,
    pub capability: Capability,
}

/// Every vocabulary value added within this generation. Empty at the start of a generation.
pub const GATED_VOCABULARY: &[GatedValue] = &[];

impl FromIterator<Capability> for Capabilities {
    fn from_iter<I: IntoIterator<Item = Capability>>(iter: I) -> Self {
        Self(
            iter.into_iter()
                .map(|cap| cap.as_str().to_owned())
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omission_empty_and_unknown_capabilities_negotiate_to_known_names_only() {
        assert_eq!(Capabilities::negotiated(None), Capabilities::default());
        assert_eq!(
            Capabilities::negotiated(Some(&Capabilities::default())),
            Capabilities::default()
        );
        let offered: Capabilities =
            serde_json::from_str(r#"["runtime-metrics","future-feature"]"#).unwrap();
        assert_eq!(
            Capabilities::negotiated(Some(&offered)),
            Capabilities::from_iter([Capability::RuntimeMetrics])
        );
        assert!(!Capabilities::negotiated(None).supports(Capability::AgentSummaries));
    }

    #[test]
    fn wire_names_are_unique_sorted_and_round_trip() {
        let names: Vec<_> = Capability::ALL.iter().map(|cap| cap.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            names, sorted,
            "Capability::ALL must list unique names in order"
        );
        for cap in Capability::ALL {
            assert_eq!(Capability::from_name(cap.as_str()), Some(cap));
        }
        assert_eq!(Capability::from_name("teleportation"), None);
    }
}
