//! The row model every hierarchical sidebar tab shares.
//!
//! Projects, JSON command tabs, and extension stream tabs list very different things, but a row in
//! any of them says the same kinds of thing: a label, a right-aligned note, a second line, a state,
//! whether it is the current one, and what activating it does. Producers describe those as meaning,
//! never as colors or glyphs, and the sidebar decides how each looks, so every tree reads like the
//! built-in tabs around it.
//!
//! Rows arrive flat, each naming its parent, grouped in sections (one per producer, or one host per
//! section in Projects). [`flatten`] turns them into the visible list, in display order. Expansion is client-local view state, keyed by tab, section, and row id, and a producer's
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

fn is_normal(tone: &SidebarTone) -> bool {
    *tone == SidebarTone::Normal
}

/// One row a JSON command tab prints or a `rozi sidebar publish` stream sends.
///
/// The fields say what a row means; rozi decides how it looks. `id` is the row's identity, unique
/// within one producer and stable across snapshots: expansion, the cursor, and activations follow
/// it, never a position.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct SidebarRowSpec {
    pub id: String,
    /// The row this one sits under. A parent that is not in the snapshot makes it a top-level row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub label: String,
    /// Right-aligned on the label's line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A short value beside the label in the row's tone, such as how long it has run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<String>,
    /// A second line under the label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "is_normal")]
    pub tone: SidebarTone,
    /// The current one of its kind. At most one row per producer should set it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub active: bool,
    /// Shown but not selectable, with this reason as its second line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<String>,
    /// Has children even before any are listed. A stream is told when the user expands it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub expandable: bool,
    /// Initial expansion. The user's own choice wins once made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expanded: Option<bool>,
    /// A top-level row that titles the rows beneath it: drawn as a section heading, always open,
    /// and never activated.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub heading: bool,
    /// Stream tabs: which of the producer's actions apply to this row, by id. Absent means all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions: Option<Vec<String>>,
}

/// A key a stream producer offers on its rows.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct SidebarActionSpec {
    /// Returned as `action`.
    pub id: String,
    /// The chord that fires it on the row under the cursor, spelled as in `[keys]` (`d`,
    /// `ctrl-r`). The sidebar's own navigation keys are refused.
    pub key: String,
    /// What it does, in a few words.
    pub label: String,
    /// Require a second press on the same row.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confirm: bool,
    /// Show it as the row's ✕ and bind `x` to it too. Always confirms, like every sidebar ✕.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub close: bool,
}

/// What a JSON command tab prints, and each line a stream producer writes: the complete current
/// set of its rows.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct SidebarSnapshot {
    #[serde(default)]
    pub rows: Vec<SidebarRowSpec>,
    /// Stream tabs only: this producer's actions. Replaces the previous set when present and keeps
    /// it when absent, so a producer can declare them once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions: Option<Vec<SidebarActionSpec>>,
}

/// What rozi writes back on a stream, one JSON object per line.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum SidebarStreamEvent {
    /// The row was clicked, or Enter was pressed on it.
    Activate { activate: String },
    /// An `expandable` row was expanded: list its children.
    Expand { expand: String },
    /// One of the producer's actions fired on a row.
    Action { action: String, row: String },
    /// Whether the tab is on screen. Sent when the stream opens and whenever it changes, so a
    /// producer can stop working while nobody looks.
    Visible { visible: bool },
}

/// Rows kept from one snapshot. More is a producer describing something a sidebar cannot show.
pub const SIDEBAR_MAX_ROWS: usize = 500;
/// Actions kept from one producer.
pub const SIDEBAR_MAX_ACTIONS: usize = 16;

fn clean(text: &str, max_chars: usize) -> String {
    let text = tui_lipan::utils::sanitize_display_text(text);
    let text = text.trim();
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut short: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    short.push('…');
    short
}

fn clean_optional(text: Option<String>, max_chars: usize) -> Option<String> {
    text.map(|text| clean(&text, max_chars))
        .filter(|text| !text.is_empty())
}

/// Make a producer's rows safe to show: control sequences stripped, text bounded, ids unique, and
/// rows without an id or a label dropped. Never trusts a producer, which is any program the user
/// or an extension runs.
pub fn sanitize_rows(rows: Vec<SidebarRowSpec>) -> Vec<SidebarRowSpec> {
    let mut seen = std::collections::HashSet::new();
    rows.into_iter()
        .take(SIDEBAR_MAX_ROWS)
        .filter_map(|row| {
            let id = clean(&row.id, 256);
            let label = clean(&row.label, 160);
            if id.is_empty() || label.is_empty() || !seen.insert(id.clone()) {
                return None;
            }
            Some(SidebarRowSpec {
                id,
                parent: clean_optional(row.parent, 256),
                label,
                description: clean_optional(row.description, 80),
                meta: clean_optional(row.meta, 24),
                detail: clean_optional(row.detail, 160),
                tone: row.tone,
                active: row.active,
                disabled: clean_optional(row.disabled, 80),
                expandable: row.expandable,
                expanded: row.expanded,
                heading: row.heading,
                actions: row.actions.map(|actions| {
                    actions
                        .into_iter()
                        .take(SIDEBAR_MAX_ACTIONS)
                        .map(|action| clean(&action, 64))
                        .collect()
                }),
            })
        })
        .collect()
}

impl TreeRow {
    /// A producer's row, doing `target` when activated.
    pub fn from_spec(
        spec: &SidebarRowSpec,
        target: RowTarget,
        close: Option<SidebarClose>,
    ) -> Self {
        Self {
            id: spec.id.clone(),
            parent: spec.parent.clone(),
            content: RowContent {
                label: spec.label.clone(),
                description: spec.description.clone(),
                meta: spec.meta.clone(),
                detail: spec.detail.clone(),
                tone: spec.tone,
                active: spec.active,
                disabled: spec.disabled.clone(),
                heading: spec.heading,
            },
            target,
            close,
            expandable: spec.expandable,
            expanded: spec.expanded,
        }
    }
}

/// What a row shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowContent {
    pub label: String,
    /// Right-aligned on the label's line.
    pub description: Option<String>,
    /// Beside the label, in the row's tone.
    pub meta: Option<String>,
    /// The row's second line.
    pub detail: Option<String>,
    pub tone: SidebarTone,
    /// The current one of its kind: the session on screen, the checkout the focused pane is in.
    pub active: bool,
    /// Shown but not selectable, with this reason as its second line.
    pub disabled: Option<String>,
    /// Titles the rows beneath it. Honored on top-level rows only.
    pub heading: bool,
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
    /// A section heading: always open, never activated.
    pub heading: bool,
    /// Folded: the most urgent tone among the rows it hides.
    pub hidden_tone: Option<SidebarTone>,
}

impl TreeItem {
    /// What activating this line does. A structural row with children toggles them.
    pub fn target(&self) -> RowTarget {
        match self {
            Self::Row(line) if line.heading || line.row.content.disabled.is_some() => {
                RowTarget::Inert
            }
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
    let depth = branch.depth();
    let heading = row.content.heading && depth == 0;
    let expandable = !heading && (row.expandable || !kids.is_empty());
    let key = TreeExpansionKey {
        tab: tab.clone(),
        section: section.key.clone(),
        row: row.id.clone(),
    };
    let expanded = heading
        || expandable
            && expansion
                .get(&key)
                .copied()
                .unwrap_or_else(|| row.expanded.unwrap_or(!kids.is_empty()));
    let hidden_tone = (expandable && !expanded && !kids.is_empty())
        .then(|| SidebarTone::most_urgent(descendants(children, kids).map(|row| row.content.tone)));
    items.push(TreeItem::Row(Box::new(TreeLine {
        row: row.clone(),
        key,
        branch: branch.clone(),
        expandable,
        expanded,
        heading,
        hidden_tone,
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

/// Every row beneath `kids`, at any depth.
fn descendants<'a>(
    children: &'a HashMap<&str, Vec<&'a TreeRow>>,
    kids: &'a [&'a TreeRow],
) -> Box<dyn Iterator<Item = &'a TreeRow> + 'a> {
    Box::new(kids.iter().flat_map(move |row| {
        let below = children
            .get(row.id.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        std::iter::once(*row).chain(descendants(children, below))
    }))
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

    /// A top-level heading keeps its rows in view whatever the stored expansion says, and does
    /// nothing when activated. Deeper down, `heading` is ignored.
    #[test]
    fn a_heading_is_always_open_and_inert_and_only_at_the_top() {
        let mut heading = row("group", None);
        heading.content.heading = true;
        heading.target = RowTarget::NewSession(None);
        let mut nested = row("nested", Some("group"));
        nested.content.heading = true;
        let section = TreeSection {
            key: "s".into(),
            rows: vec![heading, nested, row("leaf", Some("nested"))],
            ..TreeSection::default()
        };
        let mut expansion = TreeExpansion::new();
        for id in ["group", "nested"] {
            expansion.insert(
                TreeExpansionKey {
                    tab: tab(),
                    section: "s".into(),
                    row: id.into(),
                },
                false,
            );
        }
        let items = flatten(&tab(), &[section], &expansion);
        assert_eq!(labels(&items), ["group", "  nested"]);
        let line = items[0].line().expect("heading line");
        assert!(line.heading && line.expanded && !line.expandable);
        assert_eq!(items[0].target(), RowTarget::Inert);
        let nested = items[1].line().expect("nested line");
        assert!(!nested.heading && nested.expandable && !nested.expanded);
    }

    /// A folded row carries the most urgent tone of everything it hides, at any depth, so its
    /// arrow can say a problem is under it. An open one carries none: its rows say it themselves.
    #[test]
    fn a_folded_row_reports_the_most_urgent_tone_it_hides() {
        let mut deep = row("deep", Some("child"));
        deep.content.tone = SidebarTone::Error;
        let mut child = row("child", Some("parent"));
        child.content.tone = SidebarTone::Working;
        let section = TreeSection {
            key: "s".into(),
            rows: vec![row("parent", None), child, deep],
            ..TreeSection::default()
        };
        let mut expansion = TreeExpansion::new();
        let key = TreeExpansionKey {
            tab: tab(),
            section: "s".into(),
            row: "parent".into(),
        };
        let open = flatten(&tab(), std::slice::from_ref(&section), &expansion);
        assert_eq!(open[0].line().and_then(|line| line.hidden_tone), None);

        expansion.insert(key, false);
        let folded = flatten(&tab(), &[section], &expansion);
        assert_eq!(
            folded[0].line().and_then(|line| line.hidden_tone),
            Some(SidebarTone::Error)
        );
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

    /// A producer is any program: its text is stripped of control sequences and bounded, a repeated
    /// id keeps only its first row, and a row with nothing to show is dropped.
    #[test]
    fn rows_are_sanitized_before_they_are_shown() {
        let spec = |id: &str, label: &str| SidebarRowSpec {
            id: id.into(),
            parent: None,
            label: label.into(),
            description: Some("\u{1b}[31mred\u{1b}[0m".into()),
            detail: Some("   ".into()),
            tone: SidebarTone::Normal,
            active: false,
            disabled: None,
            expandable: false,
            expanded: None,
            heading: false,
            actions: None,
            meta: Some(format!("\u{1b}[1m{}", "9".repeat(40))),
        };
        let rows = sanitize_rows(vec![
            spec("a", "first"),
            spec("a", "duplicate"),
            spec("", "no id"),
            spec("b", ""),
            spec("c", &"x".repeat(400)),
        ]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "first");
        assert_eq!(rows[0].description.as_deref(), Some("red"));
        assert_eq!(rows[0].detail, None);
        assert_eq!(rows[1].label.chars().count(), 160);
        let meta = rows[0].meta.as_deref().expect("meta kept");
        assert!(!meta.contains('\u{1b}') && meta.chars().count() == 24);
    }

    /// The wire form a producer writes: tones by name, absent fields defaulted, unknown ones
    /// ignored so a newer producer still works against an older rozi.
    #[test]
    fn snapshots_parse_from_the_documented_wire_form() {
        let snapshot: SidebarSnapshot = serde_json::from_str(
            r#"{"rows":[{"id":"r","label":"main","tone":"blocked","parent":"p","future":1,
                          "meta":"2m","heading":true}],
               "actions":[{"id":"rm","key":"d","label":"remove","close":true}]}"#,
        )
        .expect("parses");
        assert_eq!(snapshot.rows[0].tone, SidebarTone::Blocked);
        assert_eq!(snapshot.rows[0].meta.as_deref(), Some("2m"));
        assert!(snapshot.rows[0].heading);
        assert_eq!(snapshot.rows[0].parent.as_deref(), Some("p"));
        assert!(snapshot.actions.unwrap()[0].close);
        assert_eq!(
            serde_json::to_string(&SidebarStreamEvent::Action {
                action: "rm".into(),
                row: "r".into()
            })
            .unwrap(),
            r#"{"action":"rm","row":"r"}"#
        );
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
