//! Structural pane chrome: how titles and borders are laid out. Plain vocabulary shared by the
//! shared layout document, the config and the renderer.

/// Structural presentation of a visible pane title. `Bar` is the existing separate title row above
/// the frame; `Border` and `Integrated` reuse the frame's top border row; `Inset` writes the title
/// inside the frame, on the first interior row beneath the top border. Visibility is controlled
/// independently by `pane.show_titles`, so toggling it never loses the selected layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum PaneTitlebarMode {
    Bar,
    Border,
    Integrated,
    Inset,
}

impl PaneTitlebarMode {
    /// Cycle order for `Action::CycleTitlebar`.
    pub fn all() -> &'static [PaneTitlebarMode] {
        &[Self::Bar, Self::Border, Self::Integrated, Self::Inset]
    }

    /// Config token and persisted value.
    pub fn id(self) -> &'static str {
        match self {
            Self::Bar => "bar",
            Self::Border => "border",
            Self::Integrated => "integrated",
            Self::Inset => "inset",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Bar => "Bar",
            Self::Border => "Border",
            Self::Integrated => "Integrated",
            Self::Inset => "Inset",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value
            .trim()
            .to_ascii_lowercase()
            .replace(['_', ' '], "-")
            .as_str()
        {
            "bar" | "titlebar" => Some(Self::Bar),
            "border" | "frame" => Some(Self::Border),
            "integrated" | "compact" => Some(Self::Integrated),
            "inset" | "inside" | "inner" => Some(Self::Inset),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        step_ring(Self::all(), self, false)
    }

    pub fn prev(self) -> Self {
        step_ring(Self::all(), self, true)
    }

    /// Whether the layout adds a chrome row *outside* the frame, which is what tile geometry has to
    /// account for: the row cannot overlap a merged neighbour's border, and it widens the drag
    /// handle on a stacked boundary. Only the separate bar does. `Inset` also spends a row on its
    /// title, but that row is interior, so the frame's own top border still merges as usual.
    pub fn takes_outer_row(self) -> bool {
        self == Self::Bar
    }

    /// Whether the title is a strip filled with the titlebar background. `Border` and `Inset` draw
    /// plain text over the pane instead, so they take their foreground from the frame background
    /// rather than the titlebar color, and have no end caps for `pane.title_style` to shape.
    pub fn fills_strip(self) -> bool {
        matches!(self, Self::Bar | Self::Integrated)
    }
}

/// Structural presentation of pane borders. Glyph shape remains independently controlled by
/// `PaneBorderStyle`; this mode decides whether pane frames or internal split dividers are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum PaneBorderMode {
    Separate,
    Merged,
    None,
    Dividers,
}

impl PaneBorderMode {
    pub fn all() -> &'static [PaneBorderMode] {
        &[Self::Separate, Self::Merged, Self::None, Self::Dividers]
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Separate => "separate",
            Self::Merged => "merged",
            Self::None => "none",
            Self::Dividers => "dividers",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Separate => "Separate",
            Self::Merged => "Merged",
            Self::None => "None",
            Self::Dividers => "Dividers",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value
            .trim()
            .to_ascii_lowercase()
            .replace(['_', ' '], "-")
            .as_str()
        {
            "separate" | "frames" | "frame" | "default" => Some(Self::Separate),
            "merged" | "merge" | "joined" => Some(Self::Merged),
            "none" | "disabled" | "off" | "borderless" => Some(Self::None),
            "dividers" | "divider" | "separators" => Some(Self::Dividers),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        step_ring(Self::all(), self, false)
    }

    pub fn prev(self) -> Self {
        step_ring(Self::all(), self, true)
    }

    pub fn draws_frames(self) -> bool {
        matches!(self, Self::Separate | Self::Merged)
    }

    pub fn merges_frames(self) -> bool {
        self == Self::Merged
    }

    pub fn draws_dividers(self) -> bool {
        self == Self::Dividers
    }
}

pub(crate) fn step_ring<T: Copy + PartialEq>(all: &[T], current: T, reverse: bool) -> T {
    let index = all
        .iter()
        .position(|candidate| *candidate == current)
        .unwrap_or(0);
    if reverse {
        all[(index + all.len() - 1) % all.len()]
    } else {
        all[(index + 1) % all.len()]
    }
}
