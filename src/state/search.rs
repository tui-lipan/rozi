use std::sync::Arc;

use tui_lipan::prelude::{SearchItem, TextInput};

use super::PaneId;

pub const MAX_MATCHES: usize = 2_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScrollbackSearchScan {
    pub epoch: u64,
    pub query: Arc<str>,
    pub panes: Arc<[PaneId]>,
    pub pane_ends: Arc<[usize]>,
    pub pane_index: usize,
    /// Number of retained lines consumed from the newest end of the current pane. Zero starts at
    /// the live bottom; the cursor resets to zero when advancing to the next pane.
    pub line_cursor: usize,
    pub first_jump_done: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScrollbackMatch {
    pub offset: usize,
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub text: Arc<str>,
    pub pane: PaneId,
}

/// Which panes a scrollback search scans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchScope {
    FocusedPane,
    Workspace,
    All,
}

impl SearchScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::FocusedPane => "pane",
            Self::Workspace => "workspace",
            Self::All => "all",
        }
    }

    /// Cycle pane → workspace → all → pane (bound to `Tab` in the search overlay).
    pub fn cycled(self) -> Self {
        match self {
            Self::FocusedPane => Self::Workspace,
            Self::Workspace => Self::All,
            Self::All => Self::FocusedPane,
        }
    }
}

pub struct ScrollbackSearchState {
    pub target: PaneId,
    pub scope: SearchScope,
    pub input: TextInput,
    pub matches: Vec<ScrollbackMatch>,
    /// Stable palette rows rebuilt only when the search result set changes.
    pub items: Arc<[SearchItem<usize>]>,
    pub current: usize,
    pub truncated: bool,
    pub scan: Option<ScrollbackSearchScan>,
    pub status: String,
    /// When true, confirm/cancel returns to copy mode and Enter parks the copy cursor on the match.
    pub from_copy_mode: bool,
}

impl ScrollbackSearchState {
    pub fn new(target: PaneId) -> Self {
        Self {
            target,
            scope: SearchScope::FocusedPane,
            input: TextInput::new(""),
            matches: Vec::new(),
            items: Arc::from([]),
            current: 0,
            truncated: false,
            scan: None,
            status: "Type to search scrollback".to_string(),
            from_copy_mode: false,
        }
    }

    pub fn from_copy_mode(target: PaneId) -> Self {
        let mut state = Self::new(target);
        state.from_copy_mode = true;
        state.status = "Type to search (copy mode)".to_string();
        state
    }

    pub fn refresh_match_status(&mut self) {
        let progress = if self.scan.is_some() { "…" } else { "" };
        if self.matches.is_empty() {
            self.status = format!("0 matches{progress} ({})", self.scope.label());
            return;
        }
        let suffix = if self.truncated { "+" } else { "" };
        self.status = format!(
            "{} / {}{suffix} matches{progress} ({})",
            self.current + 1,
            self.matches.len(),
            self.scope.label()
        );
    }

    pub fn replace_results(&mut self, matches: Vec<ScrollbackMatch>, truncated: bool) {
        self.matches = matches;
        self.truncated = truncated;
        self.rebuild_items();
    }

    pub fn append_results(&mut self, mut matches: Vec<ScrollbackMatch>) {
        if matches.is_empty() {
            return;
        }
        let base = self.matches.len();
        let mut items = self.items.iter().cloned().collect::<Vec<_>>();
        items.extend(build_items(&matches, base));
        self.matches.append(&mut matches);
        self.items = items.into();
    }

    pub fn retain_matches(&mut self, mut keep: impl FnMut(&ScrollbackMatch) -> bool) -> bool {
        if self.matches.iter().all(&mut keep) {
            return false;
        }
        let selected = self.matches.get(self.current).cloned();
        self.matches.retain(keep);
        self.current = selected
            .as_ref()
            .and_then(|selected| self.matches.iter().position(|matched| matched == selected))
            .unwrap_or_else(|| self.current.min(self.matches.len().saturating_sub(1)));
        self.rebuild_items();
        self.refresh_match_status();
        true
    }

    pub fn rebuild_items(&mut self) {
        self.items = build_items(&self.matches, 0).into();
    }
}

fn build_items(matches: &[ScrollbackMatch], base: usize) -> Vec<SearchItem<usize>> {
    let mut previous_text: Option<Arc<str>> = None;
    let mut previous_label: Option<Arc<str>> = None;
    matches
        .iter()
        .enumerate()
        .map(|(offset, matched)| {
            let label = if previous_text
                .as_ref()
                .is_some_and(|text| Arc::ptr_eq(text, &matched.text))
            {
                Arc::clone(previous_label.as_ref().expect("paired cached label"))
            } else {
                let trimmed = matched.text.trim();
                let label = if trimmed.is_empty() {
                    Arc::from("(blank line)")
                } else if trimmed.len() == matched.text.len() {
                    Arc::clone(&matched.text)
                } else {
                    Arc::from(trimmed)
                };
                previous_text = Some(Arc::clone(&matched.text));
                previous_label = Some(Arc::clone(&label));
                label
            };
            SearchItem::new(label, base + offset).description(format!(
                "pane {} · row {} · col {}",
                matched.pane,
                matched.line + 1,
                matched.start_col + 1
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matched(text: Arc<str>, start_col: usize) -> ScrollbackMatch {
        ScrollbackMatch {
            offset: 0,
            line: 2,
            start_col,
            end_col: start_col + 3,
            text,
            pane: 1,
        }
    }

    #[test]
    fn cached_search_items_share_line_labels_and_ignore_selection_changes() {
        let padded: Arc<str> = Arc::from("  hit hit  ");
        let mut search = ScrollbackSearchState::new(1);
        search.replace_results(
            vec![
                matched(Arc::clone(&padded), 2),
                matched(Arc::clone(&padded), 6),
            ],
            false,
        );

        assert_eq!(search.items.len(), 2);
        assert_eq!(search.items[0].label.as_ref(), "hit hit");
        assert!(Arc::ptr_eq(&search.items[0].label, &search.items[1].label));
        assert!(!Arc::ptr_eq(&padded, &search.items[0].label));
        assert!(search.items.iter().all(|item| !item.active));

        let items = Arc::clone(&search.items);
        search.current = 1;
        search.refresh_match_status();
        assert!(Arc::ptr_eq(&items, &search.items));

        let plain: Arc<str> = Arc::from("hit line");
        search.replace_results(vec![matched(Arc::clone(&plain), 0)], false);
        assert!(Arc::ptr_eq(&plain, &search.items[0].label));
    }

    #[test]
    fn retaining_rows_preserves_selected_match_identity_when_it_survives() {
        let text: Arc<str> = Arc::from("needle");
        let mut first = matched(Arc::clone(&text), 0);
        first.pane = 1;
        let mut removed = matched(Arc::clone(&text), 1);
        removed.pane = 2;
        let mut selected = matched(text, 2);
        selected.pane = 1;
        let mut search = ScrollbackSearchState::new(1);
        search.replace_results(vec![first, removed, selected.clone()], false);
        search.current = 2;

        assert!(search.retain_matches(|matched| matched.pane != 2));
        assert_eq!(search.current, 1);
        assert_eq!(search.matches[search.current], selected);
        assert_eq!(search.items[search.current].value, search.current);
    }
}
