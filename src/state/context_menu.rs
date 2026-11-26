use super::{AgentLocation, SessionIdentity, WorkspaceId};
use crate::pane::id::PaneId;

/// The one right-click menu that can be open, where it opened, and what it acts on.
///
/// The target is an identity, never a position, and an activation resolves it again: the row or
/// tab under the pointer may have moved, been filtered away, or gone by the time an entry is
/// chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextMenuState {
    pub target: ContextMenuTarget,
    /// Top-left of the menu, in screen cells.
    pub anchor: (u16, u16),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextMenuTarget {
    /// A pane, pinned to the child it ran when the menu opened: a respawn in between is a different
    /// program the user never right-clicked.
    Pane {
        id: PaneId,
        generation: u64,
    },
    Workspace(WorkspaceId),
    Session(SessionIdentity),
    Agent(AgentLocation),
}
