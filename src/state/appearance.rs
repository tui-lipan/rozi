use tui_lipan::prelude::{BorderStyle, TextInput, Theme};

/// The border glyphs tiled panes draw. A single app-wide setting (`Action::CycleBorderStyle`),
/// not per-pane. Floating panes keep their own `Double` border so they stay visually distinct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneBorderStyle {
    Rounded,
    Plain,
    Double,
    Thick,
}

impl PaneBorderStyle {
    /// Cycle order for `Action::CycleBorderStyle`.
    pub fn all() -> &'static [PaneBorderStyle] {
        &[Self::Rounded, Self::Plain, Self::Double, Self::Thick]
    }

    /// Config token and persisted value.
    pub fn id(self) -> &'static str {
        match self {
            Self::Rounded => "rounded",
            Self::Plain => "plain",
            Self::Double => "double",
            Self::Thick => "thick",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Rounded => "Rounded",
            Self::Plain => "Plain",
            Self::Double => "Double",
            Self::Thick => "Thick",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value
            .trim()
            .to_ascii_lowercase()
            .replace(['_', ' '], "-")
            .as_str()
        {
            "rounded" | "round" => Some(Self::Rounded),
            "plain" | "single" | "square" => Some(Self::Plain),
            "double" => Some(Self::Double),
            "thick" | "heavy" | "bold" => Some(Self::Thick),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let index = all.iter().position(|style| *style == self).unwrap_or(0);
        all[(index + 1) % all.len()]
    }

    pub fn to_border_style(self) -> BorderStyle {
        match self {
            Self::Rounded => BorderStyle::Rounded,
            Self::Plain => BorderStyle::Plain,
            Self::Double => BorderStyle::Double,
            Self::Thick => BorderStyle::Thick,
        }
    }
}

/// End-cap glyphs for a colored chip drawn over a background (pane titlebars via
/// `Action::CycleTitleStyle`, workbar badges via `Action::CycleWorkbarBadgeStyle`, workspace tabs
/// via `Action::CycleWorkbarTabStyle`). `Padded`
/// keeps a flush bar with blank side padding; the others draw the chip's ends in the chip color
/// over whatever is behind it, so it reads as a rounded/pointed pill. The cap glyphs (except
/// `Half`) are powerline separators and need a patched/Nerd font, like the titlebar's mode icons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapStyle {
    Padded,
    Half,
    Round,
    Arrow,
}

impl CapStyle {
    /// Cycle order for `Action::CycleTitleStyle`.
    pub fn all() -> &'static [CapStyle] {
        &[Self::Padded, Self::Half, Self::Round, Self::Arrow]
    }

    /// Config token and persisted value.
    pub fn id(self) -> &'static str {
        match self {
            Self::Padded => "padded",
            Self::Half => "half",
            Self::Round => "round",
            Self::Arrow => "arrow",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Padded => "Padded",
            Self::Half => "Half block",
            Self::Round => "Round",
            Self::Arrow => "Arrow",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value
            .trim()
            .to_ascii_lowercase()
            .replace(['_', ' '], "-")
            .as_str()
        {
            "padded" | "pad" | "plain" | "none" => Some(Self::Padded),
            "half" | "half-block" | "block" => Some(Self::Half),
            "round" | "rounded" | "pill" => Some(Self::Round),
            "arrow" | "pointed" | "slant" | "powerline" => Some(Self::Arrow),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let index = all.iter().position(|style| *style == self).unwrap_or(0);
        all[(index + 1) % all.len()]
    }

    /// Cycle order for workbar badge/tab style actions-same as [`all`] except `Half` is excluded.
    pub fn badge_styles() -> &'static [CapStyle] {
        &[Self::Padded, Self::Round, Self::Arrow]
    }

    pub fn next_badge(self) -> Self {
        let all = Self::badge_styles();
        let index = all.iter().position(|style| *style == self).unwrap_or(0);
        all[(index + 1) % all.len()]
    }

    /// The (left, right) cap glyphs, or `None` for `Padded` (blank side padding, no glyphs). The
    /// caps paint in the titlebar color over the backdrop, so a left cap fills toward its right
    /// and a right cap toward its left.
    pub fn caps(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Padded => None,
            Self::Half => Some(("\u{2590}", "\u{258c}")),
            Self::Round => Some(("\u{e0b6}", "\u{e0b4}")),
            Self::Arrow => Some(("\u{e0b2}", "\u{e0b0}")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppearanceAction {
    Theme,
    EditPadding,
    ToggleTitles,
    ToggleWorkbar,
    ToggleWorkbarGap,
    ToggleWorkbarPosition,
    ToggleWorkbarPowerline,
    ToggleAnimations,
    ToggleHighlightFocusedBackground,
    ToggleHighlightFocusedBorder,
    ToggleBorderMerge,
    ToggleBackgroundFollowsTerminal,
    CycleBorderStyle,
    CycleTitleStyle,
    CycleWorkbarBadgeStyle,
    CycleWorkbarTabStyle,
    CycleWorkbarStyle,
}

/// Temporary values for the Appearance terminal-padding editor. Focus, rather than a second
/// stage flag, determines whether Enter advances or applies.
pub struct PanePaddingEditorState {
    pub vertical: TextInput,
    pub horizontal: TextInput,
    pub normalizes_asymmetric: bool,
}

impl PanePaddingEditorState {
    pub fn new(padding: (u16, u16, u16, u16)) -> Self {
        let symmetric = padding.0 == padding.2 && padding.1 == padding.3;
        let mut vertical = TextInput::new(if symmetric {
            padding.0.to_string()
        } else {
            String::new()
        });
        let mut horizontal = TextInput::new(if symmetric {
            padding.1.to_string()
        } else {
            String::new()
        });
        if symmetric {
            vertical.set_anchor(Some(0));
            horizontal.set_anchor(Some(0));
        }
        Self {
            vertical,
            horizontal,
            normalizes_asymmetric: !symmetric,
        }
    }
}

impl AppearanceAction {
    /// Whether this row configures a feature that is currently switched off, so the row is inert:
    /// it still renders (greyed) but activating it does nothing. Keeps the appearance list stable
    /// instead of hiding dependent rows as their parent toggles.
    pub fn disabled_reason(self, pane: &crate::config::HyprmuxPaneConfig) -> Option<&'static str> {
        match self {
            Self::CycleTitleStyle if !pane.show_titles => Some("Needs titlebar"),
            Self::ToggleWorkbarGap
            | Self::ToggleWorkbarPosition
            | Self::ToggleWorkbarPowerline
            | Self::CycleWorkbarBadgeStyle
            | Self::CycleWorkbarTabStyle
            | Self::CycleWorkbarStyle
                if !pane.show_workbar =>
            {
                Some("Needs workbar")
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemePreset {
    Lipan,
    OneDark,
    Dracula,
    Nord,
    Gruvbox,
    Catppuccin,
    TokyoNight,
    SolarizedDark,
    Monokai,
    Ansi,
}

impl ThemePreset {
    pub fn all() -> [Self; 10] {
        [
            Self::Lipan,
            Self::OneDark,
            Self::Dracula,
            Self::Nord,
            Self::Gruvbox,
            Self::Catppuccin,
            Self::TokyoNight,
            Self::SolarizedDark,
            Self::Monokai,
            Self::Ansi,
        ]
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Lipan => "lipan",
            Self::OneDark => "one-dark",
            Self::Dracula => "dracula",
            Self::Nord => "nord",
            Self::Gruvbox => "gruvbox",
            Self::Catppuccin => "catppuccin",
            Self::TokyoNight => "tokyo-night",
            Self::SolarizedDark => "solarized-dark",
            Self::Monokai => "monokai",
            Self::Ansi => "ansi",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Lipan => "Lipan",
            Self::OneDark => "One Dark",
            Self::Dracula => "Dracula",
            Self::Nord => "Nord",
            Self::Gruvbox => "Gruvbox",
            Self::Catppuccin => "Catppuccin",
            Self::TokyoNight => "Tokyo Night",
            Self::SolarizedDark => "Solarized Dark",
            Self::Monokai => "Monokai",
            Self::Ansi => "ANSI",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value
            .trim()
            .to_ascii_lowercase()
            .replace(['_', ' '], "-")
            .as_str()
        {
            "lipan" | "tui-lipan" | "tuilipan" | "default" => Some(Self::Lipan),
            "one-dark" | "onedark" => Some(Self::OneDark),
            "dracula" => Some(Self::Dracula),
            "nord" => Some(Self::Nord),
            "gruvbox" => Some(Self::Gruvbox),
            "catppuccin" => Some(Self::Catppuccin),
            "tokyo-night" | "tokyonight" => Some(Self::TokyoNight),
            "solarized-dark" | "solarized" => Some(Self::SolarizedDark),
            "monokai" => Some(Self::Monokai),
            "ansi" => Some(Self::Ansi),
            _ => None,
        }
    }

    pub fn theme(self) -> Theme {
        match self {
            Self::Lipan => Theme::lipan(),
            Self::OneDark => Theme::one_dark(),
            Self::Dracula => Theme::dracula(),
            Self::Nord => Theme::nord(),
            Self::Gruvbox => Theme::gruvbox(),
            Self::Catppuccin => Theme::catppuccin(),
            Self::TokyoNight => Theme::tokyo_night(),
            Self::SolarizedDark => Theme::solarized_dark(),
            Self::Monokai => Theme::monokai(),
            Self::Ansi => Theme::ansi(),
        }
    }
}

pub struct ThemePickerPreview {
    pub theme: Theme,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symmetric_padding_prefills_and_asymmetric_padding_requires_explicit_normalization() {
        let symmetric = PanePaddingEditorState::new((2, 1, 2, 1));
        assert_eq!(symmetric.vertical.text(), "2");
        assert_eq!(symmetric.horizontal.text(), "1");
        assert!(!symmetric.normalizes_asymmetric);

        let asymmetric = PanePaddingEditorState::new((1, 2, 3, 4));
        assert!(asymmetric.vertical.text().is_empty());
        assert!(asymmetric.horizontal.text().is_empty());
        assert!(asymmetric.normalizes_asymmetric);
    }
}
