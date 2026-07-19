use super::PaneId;

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

/// State for keyboard copy mode: a cursor and optional selection anchor in the target
/// pane's snapshot grid (viewport coordinates, which already reflect `offset`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyModeState {
    pub target: PaneId,
    pub cursor_row: usize,
    pub cursor_col: usize,
    /// Selection start, or `None` until the user presses `v`/`Space`.
    pub anchor: Option<(usize, usize)>,
    /// Scrollback offset the pane is parked at while in copy mode.
    pub offset: usize,
    /// Matches retained after a `/` search so `n`/`N` can cycle without reopening the overlay.
    pub search_matches: Vec<CopySearchMatch>,
    pub search_current: usize,
}

/// One scrollback match parked on [`CopyModeState`] for `n`/`N` cycling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopySearchMatch {
    pub offset: usize,
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
}

impl CopyModeState {
    pub fn selection(&self) -> Option<((usize, usize), (usize, usize))> {
        self.anchor
            .map(|anchor| (anchor, (self.cursor_row, self.cursor_col)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyFlashState {
    pub id: u64,
    pub target: PaneId,
    pub selection: ((usize, usize), (usize, usize)),
    pub return_to_live: bool,
    pub clearing: bool,
}
