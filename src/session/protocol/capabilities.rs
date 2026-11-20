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
    /// A client may reserve its slot for the process that replaces it
    /// ([`super::ClientMessage::PrepareHandoff`]) and the replacement claims it on attach, keeping
    /// the layout-control lease and parked state across a client relaunch.
    ClientHandoff,
    /// The server routes each pane's drag-and-drop conversation (OSC 72) to the client that owns
    /// it: [`super::ClientMessage::DragAndDrop`] and [`super::ServerMessage::PaneDragAndDrop`].
    DragAndDrop,
    /// The server keeps asynchronous requests between the agents of its session: the
    /// `agent-ask`, `agent-reply`, `agent-await`, `agent-request`, `agent-requests`, and
    /// `agent-cancel` control commands, and the `agent-cli` response source.
    AgentRequests,
    /// The server serves `agent-reply-later`: an agent keeps a request open for a reply after its
    /// turn ends.
    AgentReplyLater,
    /// The server sends [`super::ServerMessage::AgentRequestsChanged`]: the session's agent
    /// requests without their text, so a UI can show who is asking whom.
    AgentRequestUpdates,
    /// [`super::PaneMeta::program_title`] carries the program's own title, so a client can tell it
    /// from a name the user gave the pane. Without it, `title` is the only title there is.
    PaneProgramTitle,
    /// The server groups agent requests into conversations: `AgentRequestInfo::conversation`,
    /// and `agent-ask` with `follows`, which joins an asker with no agent to an earlier one.
    AgentConversations,
    /// The server stores files sent with `agent-attach`, so a request can carry them to an agent
    /// on another host.
    AgentAttachments,
    /// The server shares its session with other operating-system accounts: it serves the
    /// `share-grant`, `share-revoke`, and `share-list` control commands, reports each client's
    /// [`super::SessionRole`] in the roster, and refuses a request a client's access does not cover
    /// with [`super::PERMISSION_DENIED_ERROR_CODE`].
    SessionSharing,
    /// The server holds `publish` streams for panes that have no UI to publish through: it serves
    /// the `publish` control command on a session connection, takes
    /// [`super::ClientMessage::PublishRows`] on it, and forwards
    /// [`super::ClientMessage::ActivatePublishedRow`] to it as
    /// [`super::ServerMessage::PublishActivation`].
    SessionPublish,
    /// The server reopens an agent conversation in a new pane: it takes
    /// [`super::ClientMessage::SpawnPane`] with `agent_resume`.
    SpawnAgentResume,
    /// The server honors `agent-ask` with `now`: it types the request in at once, or refuses it.
    AgentAskNow,
}

impl Capability {
    /// Every capability this build knows, in wire-name order.
    pub const ALL: [Capability; 16] = [
        Capability::AgentAskNow,
        Capability::AgentAttachments,
        Capability::AgentConversations,
        Capability::AgentReplyLater,
        Capability::AgentRequestUpdates,
        Capability::AgentRequests,
        Capability::AgentSummaries,
        Capability::ClientHandoff,
        Capability::DragAndDrop,
        Capability::GraphicsRootFrameEdits,
        Capability::PaneProgramTitle,
        Capability::ProjectSummaries,
        Capability::RuntimeMetrics,
        Capability::SessionPublish,
        Capability::SessionSharing,
        Capability::SpawnAgentResume,
    ];

    /// The name this capability travels under.
    pub const fn as_str(self) -> &'static str {
        match self {
            Capability::RuntimeMetrics => "runtime-metrics",
            Capability::AgentSummaries => "agent-summaries",
            Capability::ProjectSummaries => "project-summaries",
            Capability::GraphicsRootFrameEdits => "graphics-root-frame-edits",
            Capability::ClientHandoff => "client-handoff",
            Capability::DragAndDrop => "drag-and-drop",
            Capability::AgentRequests => "agent-requests",
            Capability::AgentReplyLater => "agent-reply-later",
            Capability::AgentRequestUpdates => "agent-request-updates",
            Capability::PaneProgramTitle => "pane-program-title",
            Capability::SessionSharing => "session-sharing",
            Capability::SessionPublish => "session-publish",
            Capability::AgentConversations => "agent-conversations",
            Capability::AgentAttachments => "agent-attachments",
            Capability::SpawnAgentResume => "spawn-agent-resume",
            Capability::AgentAskNow => "agent-ask-now",
        }
    }

    /// First release that offers this optional session behavior.
    pub const fn introduced_in(self) -> &'static str {
        match self {
            Self::RuntimeMetrics
            | Self::AgentSummaries
            | Self::ProjectSummaries
            | Self::GraphicsRootFrameEdits
            | Self::ClientHandoff => "0.0.44",
            Self::DragAndDrop => "0.0.46",
            Self::AgentRequests => "0.0.47",
            Self::AgentReplyLater | Self::AgentRequestUpdates | Self::PaneProgramTitle => "0.0.48",
            Self::AgentConversations
            | Self::AgentAttachments
            | Self::SessionSharing
            | Self::SessionPublish => "0.0.50",
            Self::SpawnAgentResume => "0.1.1",
            Self::AgentAskNow => "0.1.2",
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
pub const GATED_VOCABULARY: &[GatedValue] = &[
    GatedValue {
        vocabulary: "AgentResponseSource",
        value: "agent-cli",
        capability: Capability::AgentRequests,
    },
    agent_request_command("agent-ask"),
    agent_request_command("agent-reply"),
    agent_request_command("agent-await"),
    agent_request_command("agent-request"),
    agent_request_command("agent-requests"),
    agent_request_command("agent-cancel"),
    GatedValue {
        vocabulary: "ControlRequest",
        value: "agent-reply-later",
        capability: Capability::AgentReplyLater,
    },
    GatedValue {
        vocabulary: "ControlRequest",
        value: "agent-attach",
        capability: Capability::AgentAttachments,
    },
    GatedValue {
        vocabulary: "ControlRequest",
        value: "agent-attach-cancel",
        capability: Capability::AgentAttachments,
    },
    session_sharing_command("share-grant"),
    session_sharing_command("share-revoke"),
    session_sharing_command("share-list"),
];

/// A control command a session server serves only with [`Capability::SessionSharing`].
const fn session_sharing_command(value: &'static str) -> GatedValue {
    GatedValue {
        vocabulary: "ControlRequest",
        value,
        capability: Capability::SessionSharing,
    }
}

/// A control command a session server serves only with [`Capability::AgentRequests`].
const fn agent_request_command(value: &'static str) -> GatedValue {
    GatedValue {
        vocabulary: "ControlRequest",
        value,
        capability: Capability::AgentRequests,
    }
}

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
