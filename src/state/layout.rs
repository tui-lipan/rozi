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
    Monocle,
}

impl LayoutKind {
    /// Every layout in cycle order. `toggled` walks this list, so the order here
    /// is the order `Action::ToggleLayout` rotates through.
    pub fn all() -> &'static [LayoutKind] {
        &[Self::Dwindle, Self::Master, Self::Grid, Self::Monocle]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Dwindle => "dwindle",
            Self::Master => "master",
            Self::Grid => "grid",
            Self::Monocle => "monocle",
        }
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
        assert_eq!(LayoutKind::Grid.toggled(), LayoutKind::Monocle);
        assert_eq!(LayoutKind::Monocle.toggled(), LayoutKind::Dwindle);
        assert_eq!(LayoutKind::all().len(), 4);
    }

    #[test]
    fn layout_kind_labels_are_distinct() {
        let labels: Vec<&str> = LayoutKind::all().iter().map(|k| k.label()).collect();
        assert_eq!(labels, ["dwindle", "master", "grid", "monocle"]);
    }
}
