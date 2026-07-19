use tui_lipan::prelude::TextInput;

use super::PaneId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScrollbackMatch {
    pub offset: usize,
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub text: String,
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
            Self::All => "all panes",
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
    pub current: usize,
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
            current: 0,
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
}
