//! Where a session's work happens, for a client that never attaches to it.
//!
//! The host monitor's agent summaries answer "does anything here want me?". This answers the
//! question one level up: "which repositories, and which checkouts of them, is this session working
//! in?" — what the Projects tab arranges every session on every host by.
//!
//! It is still the semantic layer, not a second session protocol: a checkout's root, the repository
//! it belongs to, its branch, and which panes and published rows work there. A pane's own working
//! directory below the checkout root, its screen, scrollback, layout, and process arguments stay
//! behind a real attachment, as they do for [`super::AgentSummary`].
use serde::{Deserialize, Serialize};

/// One checkout a session works in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub session: String,
    /// The checkout's root on the session's host: the primary checkout, or a linked worktree.
    pub root: String,
    /// The primary checkout `root` is a linked worktree of. Absent when `root` is the primary
    /// checkout itself. Together with the host, this is what identifies a project across its
    /// worktrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// Branch checked out at `root`, or a short commit id when `HEAD` is detached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Live panes whose working directory is inside this checkout.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub panes: Vec<crate::state::PaneId>,
    /// Published rows working in this checkout from a pane whose own directory is elsewhere — a
    /// program running each of its agents in its own worktree.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<ProjectRowRef>,
}

impl ProjectSummary {
    /// The repository this checkout belongs to: its primary checkout.
    pub fn repository_root(&self) -> &str {
        self.repository.as_deref().unwrap_or(&self.root)
    }
}

/// The checkout the agent in `pane` of `session` works in, as `row` when it is one of several
/// published there. A published row with a place of its own is placed by it, anything else by its
/// pane's directory.
pub fn checkout_of<'a>(
    summaries: &'a [ProjectSummary],
    session: &str,
    pane: crate::state::PaneId,
    row: Option<&super::RowKey>,
) -> Option<&'a ProjectSummary> {
    let in_session = || {
        summaries
            .iter()
            .filter(move |summary| summary.session == session)
    };
    row.and_then(|row| {
        in_session().find(|summary| {
            summary
                .rows
                .iter()
                .any(|held| held.pane == pane && &held.row == row)
        })
    })
    .or_else(|| in_session().find(|summary| summary.panes.contains(&pane)))
}

/// A published row, by the pane that publishes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRowRef {
    pub pane: crate::state::PaneId,
    pub row: super::RowKey,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A published row with its own worktree belongs to that worktree, not to its pane's: that is
    /// what lets one program's agents spread across the checkouts they really work in.
    #[test]
    fn a_published_row_is_placed_by_its_own_checkout() {
        let row = super::super::RowKey::new("sessions", "fix");
        let pane_checkout = ProjectSummary {
            session: "dev".into(),
            root: "/x/rozi".into(),
            repository: None,
            branch: Some("master".into()),
            panes: vec![1],
            rows: Vec::new(),
        };
        let row_checkout = ProjectSummary {
            session: "dev".into(),
            root: "/x/rozi/.worktrees/fix".into(),
            repository: Some("/x/rozi".into()),
            branch: Some("fix".into()),
            panes: Vec::new(),
            rows: vec![ProjectRowRef {
                pane: 1,
                row: row.clone(),
            }],
        };
        let summaries = [pane_checkout.clone(), row_checkout.clone()];
        assert_eq!(
            checkout_of(&summaries, "dev", 1, Some(&row)),
            Some(&row_checkout)
        );
        assert_eq!(
            checkout_of(&summaries, "dev", 1, None),
            Some(&pane_checkout)
        );
        let other = super::super::RowKey::new("sessions", "other");
        assert_eq!(
            checkout_of(&summaries, "dev", 1, Some(&other)),
            Some(&pane_checkout)
        );
        assert_eq!(checkout_of(&summaries, "api", 1, None), None);
        assert_eq!(row_checkout.repository_root(), "/x/rozi");
        assert_eq!(pane_checkout.repository_root(), "/x/rozi");
    }
}
