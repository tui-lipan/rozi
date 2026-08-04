use super::PaneId;
use tui_lipan::prelude::TerminalCopyMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Resize,
    Copy,
    Hint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HintModeState {
    pub target: PaneId,
    pub matches: Vec<crate::hints::HintMatch>,
    pub labels: Vec<String>,
    pub input: String,
    pub offset: usize,
}

/// Application state around tui-lipan's copy-mode navigation and hyprmux's search integration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyModeState {
    pub target: PaneId,
    pub navigation: TerminalCopyMode,
    /// Matches retained after a `/` search so `n`/`N` can cycle without reopening the overlay.
    pub search_matches: Vec<CopySearchMatch>,
    pub search_current: usize,
    /// Whether additional matches existed beyond the bounded retained set.
    pub search_truncated: bool,
}

/// One scrollback match parked on [`CopyModeState`] for `n`/`N` cycling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopySearchMatch {
    pub offset: usize,
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
}
