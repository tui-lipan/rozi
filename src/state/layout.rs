use super::TILE_GAP;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

impl SplitAxis {
    pub fn flipped(self) -> Self {
        match self {
            Self::Horizontal => Self::Vertical,
            Self::Vertical => Self::Horizontal,
        }
    }

    pub fn at_depth(self, depth: usize) -> Self {
        if depth.is_multiple_of(2) {
            self
        } else {
            self.flipped()
        }
    }
}

/// Per-axis gap between tiled panes. Split apart because the two axes differ: left|right splits
/// carry a visible column gap, while top|bottom splits sit flush. Other border modes select zero,
/// positive divider, or negative merged-frame gaps through `State::tile_gap`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileGap {
    pub horizontal: f32,
    pub vertical: f32,
}

impl TileGap {
    /// The default un-merged gaps: a column between left|right splits, none between stacked panes.
    pub const DEFAULT: TileGap = TileGap {
        horizontal: TILE_GAP,
        vertical: 0.0,
    };

    pub fn for_axis(self, axis: SplitAxis) -> f32 {
        match axis {
            SplitAxis::Horizontal => self.horizontal,
            SplitAxis::Vertical => self.vertical,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Down,
    Up,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutKind {
    Dwindle,
    Master,
    Grid,
    Columns,
    Rows,
    Scrollable,
    Monocle,
}

impl LayoutKind {
    /// Every layout in cycle order. `toggled` walks this list, so the order here
    /// is the order `Action::ToggleLayout` rotates through.
    pub fn all() -> &'static [LayoutKind] {
        &[
            Self::Dwindle,
            Self::Master,
            Self::Grid,
            Self::Columns,
            Self::Rows,
            Self::Scrollable,
            Self::Monocle,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Dwindle => "dwindle",
            Self::Master => "master",
            Self::Grid => "grid",
            Self::Columns => "columns",
            Self::Rows => "rows",
            Self::Scrollable => "scrollable",
            Self::Monocle => "monocle",
        }
    }

    /// Parse a config `[layout] default` spelling back into a mode. Matching is
    /// case-insensitive against [`Self::label`]; unknown names yield `None` so the caller can
    /// warn and keep the built-in default.
    pub fn from_label(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase();
        Self::all()
            .iter()
            .copied()
            .find(|kind| kind.label() == name)
    }

    pub fn toggled(self) -> Self {
        let all = Self::all();
        let index = all.iter().position(|kind| *kind == self).unwrap_or(0);
        all[(index + 1) % all.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_kind_cycles_through_every_layout() {
        assert_eq!(LayoutKind::Dwindle.toggled(), LayoutKind::Master);
        assert_eq!(LayoutKind::Master.toggled(), LayoutKind::Grid);
        assert_eq!(LayoutKind::Grid.toggled(), LayoutKind::Columns);
        assert_eq!(LayoutKind::Columns.toggled(), LayoutKind::Rows);
        assert_eq!(LayoutKind::Rows.toggled(), LayoutKind::Scrollable);
        assert_eq!(LayoutKind::Scrollable.toggled(), LayoutKind::Monocle);
        assert_eq!(LayoutKind::Monocle.toggled(), LayoutKind::Dwindle);
        assert_eq!(LayoutKind::all().len(), 7);
    }

    #[test]
    fn layout_kind_labels_are_distinct() {
        let labels: Vec<&str> = LayoutKind::all().iter().map(|k| k.label()).collect();
        assert_eq!(
            labels,
            [
                "dwindle",
                "master",
                "grid",
                "columns",
                "rows",
                "scrollable",
                "monocle"
            ]
        );
    }

    #[test]
    fn from_label_round_trips_every_layout_case_insensitively() {
        for kind in LayoutKind::all() {
            assert_eq!(LayoutKind::from_label(kind.label()), Some(*kind));
        }
        assert_eq!(
            LayoutKind::from_label("  Master "),
            Some(LayoutKind::Master)
        );
        assert_eq!(LayoutKind::from_label("GRID"), Some(LayoutKind::Grid));
        assert_eq!(LayoutKind::from_label("spiral"), None);
        assert_eq!(LayoutKind::from_label(""), None);
    }
}
