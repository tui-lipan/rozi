//! The row model every hierarchical sidebar tab shares.
//!
//! Projects, JSON command tabs, and extension stream tabs list very different things, but a row in
//! any of them says the same kinds of thing: a label, a right-aligned note, a second line, a state,
//! whether it is the current one, and what activating it does. Producers describe those as meaning,
//! never as colors or glyphs, and the sidebar decides how each looks, so every tree reads like the
//! built-in tabs around it.
//!
//! Rows arrive flat, each naming its parent, grouped in sections (one per producer, or one host per
//! section in Projects). [`flatten`] turns them into the visible list, in display order, for both
//! the view and the click/keyboard projection: the two can never disagree about which row an index
//! means. Expansion is client-local view state, keyed by tab, section, and row id, and a producer's
//! own `expanded` hint applies only until the user chooses otherwise.

use std::collections::HashMap;

use tui_lipan::prelude::TreeBranch;

use super::sidebar::{RowTarget, SidebarClose};
use crate::config::SidebarTabId;

/// What a row's state means. Rozi maps it to a glyph and the theme's colors.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum SidebarTone {
    /// Nothing to report. No glyph.
    #[default]
    Normal,
    /// Secondary or inactive. Dimmed label, no glyph.
    Muted,
    /// Worth noticing, not urgent.
    Info,
    /// Work in progress. Animated.
    Working,
    /// Finished or healthy.
    Success,
    /// Needs a look.
    Warning,
    /// Waiting on the user.
    Blocked,
    /// Failed.
    Error,
}

impl SidebarTone {
    /// The tone an agent or activity status reads as: the vocabulary of `rozi status`, `rozi
    /// publish`, and detected agents, folded onto this one.
    pub fn from_status(status: &str, finished_unseen: bool) -> Self {
        use crate::session::protocol::ActivityState;
        let state = ActivityState::from_status(status);
        if finished_unseen && state.is_successful_terminal() {
            return Self::Success;
        }
        let status = status.trim();
        if status.eq_ignore_ascii_case(crate::session::protocol::pane_status::WORKING) {
            Self::Working
        } else if status.eq_ignore_ascii_case(crate::session::protocol::pane_status::BLOCKED) {
            Self::Blocked
        } else if status.eq_ignore_ascii_case("error") {
            Self::Error
        } else if status.eq_ignore_ascii_case(crate::session::protocol::pane_status::DONE) {
            Self::Success
        } else if status.eq_ignore_ascii_case(crate::session::protocol::pane_status::IDLE)
            || status.is_empty()
        {
            Self::Normal
        } else {
            // A publisher's own word (`compacting`) is work still in progress.
            Self::Working
        }
    }

    /// How much a row in this tone wants attention, lowest first. A group rolls up to the most
    /// urgent of its children.
    pub fn urgency(self) -> u8 {
        match self {
            Self::Blocked => 0,
            Self::Error => 1,
            Self::Warning => 2,
            Self::Working => 3,
            Self::Success => 4,
            Self::Info => 5,
            Self::Normal => 6,
            Self::Muted => 7,
        }
    }

    /// The most urgent of several tones, `Normal` for none.
    pub fn most_urgent(tones: impl IntoIterator<Item = Self>) -> Self {
        tones
            .into_iter()
            .min_by_key(|tone| tone.urgency())
            .unwrap_or_default()
    }
}

/// What a row shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowContent {
    pub label: String,
    /// Right-aligned on the label's line.
    pub description: Option<String>,
    /// The row's second line.
    pub detail: Option<String>,
    pub tone: SidebarTone,
    /// The current one of its kind: the session on screen, the checkout the focused pane is in.
    pub active: bool,
    /// Shown but not selectable, with this reason as its second line.
    pub disabled: Option<String>,
}

/// One row of a hierarchical tab, before flattening.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    /// Unique within its section.
    pub id: String,
    /// The row it sits under, in the same section. A parent that does not exist makes this a
    /// top-level row.
    pub parent: Option<String>,
    pub content: RowContent,
    /// What activating it does. A row with nothing to do but children toggles them instead.
    pub target: RowTarget,
    pub close: Option<SidebarClose>,
    /// Has children even when none are listed yet: a stream producer lists them on request.
    pub expandable: bool,
    /// The producer's initial expansion. `None` expands a row whose children are already listed.
    pub expanded: Option<bool>,
}

/// Rows from one source: a producer, or one host's projects.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TreeSection {
    /// Namespaces the section's row ids for expansion state.
    pub key: String,
    pub header: Option<String>,
    pub rows: Vec<TreeRow>,
    /// Children of a collapsed `expandable` row arrive only after it is expanded, so an expanded
    /// row with none yet shows a loading line.
    pub lazy: bool,
}

/// Which row an expansion choice belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TreeExpansionKey {
    pub tab: SidebarTabId,
    pub section: String,
    pub row: String,
}

/// User expansion choices, per row, for the life of this client.
pub type TreeExpansion = HashMap<TreeExpansionKey, bool>;

/// One visible line of a hierarchical tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeItem {
    Spacer,
    Header(String),
    Row(Box<TreeLine>),
    /// An expanded row whose children have not arrived.
    Loading(TreeBranch),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeLine {
    pub row: TreeRow,
    pub key: TreeExpansionKey,
    pub branch: TreeBranch,
    /// Has, or may have, children.
    pub expandable: bool,
    pub expanded: bool,
}

impl TreeItem {
    /// What activating this line does. A structural row with children toggles them.
    pub fn target(&self) -> RowTarget {
        match self {
            Self::Row(line) if line.row.content.disabled.is_some() => RowTarget::Inert,
            Self::Row(line) => match &line.row.target {
                RowTarget::Inert if line.expandable => RowTarget::TreeToggle(line.key.clone()),
                target => target.clone(),
            },
            _ => RowTarget::Inert,
        }
    }

    pub fn close(&self) -> Option<SidebarClose> {
        match self {
            Self::Row(line) if line.row.content.disabled.is_none() => line.row.close.clone(),
            _ => None,
        }
    }

    pub fn line(&self) -> Option<&TreeLine> {
        match self {
            Self::Row(line) => Some(line),
            _ => None,
        }
    }
}

/// Deepest nesting shown. Rows below it are not listed; a producer this deep is describing
/// something a sidebar cannot show anyway.
pub const MAX_DEPTH: usize = 12;

/// The visible lines of `sections`, in display order.
pub fn flatten(
    tab: &SidebarTabId,
    sections: &[TreeSection],
    expansion: &TreeExpansion,
) -> Vec<TreeItem> {
    let mut items = Vec::new();
    for section in sections {
        if section.rows.is_empty() {
            continue;
        }
        if !items.is_empty() {
            items.push(TreeItem::Spacer);
        }
        if let Some(header) = &section.header {
            items.push(TreeItem::Header(header.clone()));
        }
        let known: std::collections::HashSet<&str> =
            section.rows.iter().map(|row| row.id.as_str()).collect();
        let mut children: HashMap<&str, Vec<&TreeRow>> = HashMap::new();
        let mut roots = Vec::new();
        for row in &section.rows {
            match row.parent.as_deref() {
                Some(parent) if parent != row.id && known.contains(parent) => {
                    children.entry(parent).or_default().push(row)
                }
                _ => roots.push(row),
            }
        }
        // Top-level rows draw no guides, so they all start from the same root branch; their
        // descendants record sibling positions from there on.
        for row in roots {
            push_row(
                tab,
                section,
                &children,
                row,
                TreeBranch::root(),
                expansion,
                &mut items,
            );
        }
    }
    items
}

fn push_row(
    tab: &SidebarTabId,
    section: &TreeSection,
    children: &HashMap<&str, Vec<&TreeRow>>,
    row: &TreeRow,
    branch: TreeBranch,
    expansion: &TreeExpansion,
    items: &mut Vec<TreeItem>,
) {
    let kids = children
        .get(row.id.as_str())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let expandable = row.expandable || !kids.is_empty();
    let key = TreeExpansionKey {
        tab: tab.clone(),
        section: section.key.clone(),
        row: row.id.clone(),
    };
    let expanded = expandable
        && expansion
            .get(&key)
            .copied()
            .unwrap_or_else(|| row.expanded.unwrap_or(!kids.is_empty()));
    let depth = branch.depth();
    items.push(TreeItem::Row(Box::new(TreeLine {
        row: row.clone(),
        key,
        branch: branch.clone(),
        expandable,
        expanded,
    })));
    if !expanded || depth >= MAX_DEPTH {
        return;
    }
    if kids.is_empty() {
        if section.lazy && row.expandable {
            items.push(TreeItem::Loading(branch.child(true)));
        }
        return;
    }
    let count = kids.len();
    for (index, child) in kids.iter().enumerate() {
        push_row(
            tab,
            section,
            children,
            child,
            branch.child(index + 1 == count),
            expansion,
            items,
        );
    }
}

/// The row a cursor on `index` moves to with Left: its parent, if it has one.
pub fn parent_index(items: &[TreeItem], index: usize) -> Option<usize> {
    let depth = items.get(index)?.line()?.branch.depth();
    if depth == 0 {
        return None;
    }
    items[..index]
        .iter()
        .rposition(|item| item.line().is_some_and(|line| line.branch.depth() < depth))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, parent: Option<&str>) -> TreeRow {
        TreeRow {
            id: id.into(),
            parent: parent.map(str::to_string),
            content: RowContent {
                label: id.into(),
                ..RowContent::default()
            },
            target: RowTarget::Inert,
            close: None,
            expandable: false,
            expanded: None,
        }
    }

    fn labels(items: &[TreeItem]) -> Vec<String> {
        items
            .iter()
            .map(|item| match item {
                TreeItem::Spacer => "-".to_string(),
                TreeItem::Header(header) => format!("#{header}"),
                TreeItem::Row(line) => format!(
                    "{}{}",
                    "  ".repeat(line.branch.depth()),
                    line.row.content.label
                ),
                TreeItem::Loading(branch) => format!("{}…", "  ".repeat(branch.depth())),
            })
            .collect()
    }

    fn tab() -> SidebarTabId {
        SidebarTabId::new("t")
    }

    /// Children follow their parent in listing order, and a row whose parent is not in the snapshot
    /// is still listed rather than silently lost.
    #[test]
    fn rows_nest_under_their_parents_and_orphans_stay_visible() {
        let section = TreeSection {
            key: "s".into(),
            header: None,
            rows: vec![
                row("a", None),
                row("b", Some("a")),
                row("c", Some("missing")),
                row("d", Some("a")),
            ],
            lazy: false,
        };
        let items = flatten(&tab(), &[section], &TreeExpansion::new());
        assert_eq!(labels(&items), ["a", "  b", "  d", "c"]);
    }

    /// A cycle has no root to reach it from, so it is not listed — and flattening terminates.
    #[test]
    fn a_parent_cycle_is_dropped() {
        let section = TreeSection {
            key: "s".into(),
            rows: vec![row("x", Some("y")), row("y", Some("x")), row("z", None)],
            ..TreeSection::default()
        };
        let items = flatten(&tab(), &[section], &TreeExpansion::new());
        assert_eq!(labels(&items), ["z"]);
    }

    /// The user's choice wins over the producer's hint, and an expanded lazy row with no children
    /// yet says it is loading them.
    #[test]
    fn expansion_is_the_users_and_lazy_rows_show_loading() {
        let mut lazy = row("remote", None);
        lazy.expandable = true;
        lazy.expanded = Some(false);
        let section = TreeSection {
            key: "s".into(),
            header: Some("WORKBOX".into()),
            rows: vec![row("a", None), row("b", Some("a")), lazy],
            lazy: true,
        };
        let mut expansion = TreeExpansion::new();
        let items = flatten(&tab(), std::slice::from_ref(&section), &expansion);
        assert_eq!(labels(&items), ["#WORKBOX", "a", "  b", "remote"]);
        assert_eq!(parent_index(&items, 2), Some(1));
        assert_eq!(parent_index(&items, 1), None);

        expansion.insert(
            TreeExpansionKey {
                tab: tab(),
                section: "s".into(),
                row: "a".into(),
            },
            false,
        );
        expansion.insert(
            TreeExpansionKey {
                tab: tab(),
                section: "s".into(),
                row: "remote".into(),
            },
            true,
        );
        let items = flatten(&tab(), &[section], &expansion);
        assert_eq!(labels(&items), ["#WORKBOX", "a", "remote", "  …"]);
    }

    /// Sections are separated, and a structural row with children toggles them when activated.
    #[test]
    fn sections_are_spaced_and_structural_rows_toggle() {
        let one = TreeSection {
            key: "1".into(),
            rows: vec![row("a", None), row("b", Some("a"))],
            ..TreeSection::default()
        };
        let two = TreeSection {
            key: "2".into(),
            header: Some("TWO".into()),
            rows: vec![row("c", None)],
            ..TreeSection::default()
        };
        let items = flatten(&tab(), &[one, two], &TreeExpansion::new());
        assert_eq!(labels(&items), ["a", "  b", "-", "#TWO", "c"]);
        assert!(matches!(items[0].target(), RowTarget::TreeToggle(_)));
        assert_eq!(items[1].target(), RowTarget::Inert);
    }

    #[test]
    fn statuses_fold_onto_tones_and_roll_up_by_urgency() {
        assert_eq!(
            SidebarTone::from_status("blocked", false),
            SidebarTone::Blocked
        );
        assert_eq!(SidebarTone::from_status("idle", true), SidebarTone::Success);
        assert_eq!(SidebarTone::from_status("idle", false), SidebarTone::Normal);
        assert_eq!(
            SidebarTone::from_status("compacting", false),
            SidebarTone::Working
        );
        assert_eq!(
            SidebarTone::most_urgent([SidebarTone::Working, SidebarTone::Blocked]),
            SidebarTone::Blocked
        );
        assert_eq!(SidebarTone::most_urgent([]), SidebarTone::Normal);
    }
}
