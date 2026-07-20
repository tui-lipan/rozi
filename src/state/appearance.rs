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
    GruvboxDark,
    CatppuccinMocha,
    TokyoNight,
    SolarizedDark,
    Monokai,
    Ansi,
    SolarizedLight,
    GruvboxLight,
    TokyoNightDay,
    CatppuccinLatte,
    CatppuccinFrappe,
    CatppuccinMacchiato,
    RosePine,
    RosePineMoon,
    RosePineDawn,
    Kanagawa,
    Everforest,
    AyuDark,
    AyuMirage,
    AyuLight,
    Nightfox,
    Nordfox,
    NightOwl,
    MaterialPalenight,
    Oxocarbon,
    Zenburn,
}

impl ThemePreset {
    pub fn all() -> [Self; 30] {
        [
            Self::Lipan,
            Self::OneDark,
            Self::Dracula,
            Self::Nord,
            Self::GruvboxDark,
            Self::CatppuccinMocha,
            Self::TokyoNight,
            Self::SolarizedDark,
            Self::Monokai,
            Self::Ansi,
            Self::SolarizedLight,
            Self::GruvboxLight,
            Self::TokyoNightDay,
            Self::CatppuccinLatte,
            Self::CatppuccinFrappe,
            Self::CatppuccinMacchiato,
            Self::RosePine,
            Self::RosePineMoon,
            Self::RosePineDawn,
            Self::Kanagawa,
            Self::Everforest,
            Self::AyuDark,
            Self::AyuMirage,
            Self::AyuLight,
            Self::Nightfox,
            Self::Nordfox,
            Self::NightOwl,
            Self::MaterialPalenight,
            Self::Oxocarbon,
            Self::Zenburn,
        ]
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Lipan => "lipan",
            Self::OneDark => "one-dark",
            Self::Dracula => "dracula",
            Self::Nord => "nord",
            Self::GruvboxDark => "gruvbox-dark",
            Self::CatppuccinMocha => "catppuccin-mocha",
            Self::TokyoNight => "tokyo-night",
            Self::SolarizedDark => "solarized-dark",
            Self::Monokai => "monokai",
            Self::Ansi => "ansi",
            Self::SolarizedLight => "solarized-light",
            Self::GruvboxLight => "gruvbox-light",
            Self::TokyoNightDay => "tokyo-night-day",
            Self::CatppuccinLatte => "catppuccin-latte",
            Self::CatppuccinFrappe => "catppuccin-frappe",
            Self::CatppuccinMacchiato => "catppuccin-macchiato",
            Self::RosePine => "rose-pine",
            Self::RosePineMoon => "rose-pine-moon",
            Self::RosePineDawn => "rose-pine-dawn",
            Self::Kanagawa => "kanagawa",
            Self::Everforest => "everforest",
            Self::AyuDark => "ayu-dark",
            Self::AyuMirage => "ayu-mirage",
            Self::AyuLight => "ayu-light",
            Self::Nightfox => "nightfox",
            Self::Nordfox => "nordfox",
            Self::NightOwl => "night-owl",
            Self::MaterialPalenight => "material-palenight",
            Self::Oxocarbon => "oxocarbon",
            Self::Zenburn => "zenburn",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Lipan => "Lipan",
            Self::OneDark => "One Dark",
            Self::Dracula => "Dracula",
            Self::Nord => "Nord",
            Self::GruvboxDark => "Gruvbox Dark",
            Self::CatppuccinMocha => "Catppuccin Mocha",
            Self::TokyoNight => "Tokyo Night",
            Self::SolarizedDark => "Solarized Dark",
            Self::Monokai => "Monokai",
            Self::Ansi => "ANSI",
            Self::SolarizedLight => "Solarized Light",
            Self::GruvboxLight => "Gruvbox Light",
            Self::TokyoNightDay => "Tokyo Night Day",
            Self::CatppuccinLatte => "Catppuccin Latte",
            Self::CatppuccinFrappe => "Catppuccin Frappe",
            Self::CatppuccinMacchiato => "Catppuccin Macchiato",
            Self::RosePine => "Rose Pine",
            Self::RosePineMoon => "Rose Pine Moon",
            Self::RosePineDawn => "Rose Pine Dawn",
            Self::Kanagawa => "Kanagawa",
            Self::Everforest => "Everforest",
            Self::AyuDark => "Ayu Dark",
            Self::AyuMirage => "Ayu Mirage",
            Self::AyuLight => "Ayu Light",
            Self::Nightfox => "Nightfox",
            Self::Nordfox => "Nordfox",
            Self::NightOwl => "Night Owl",
            Self::MaterialPalenight => "Material Palenight",
            Self::Oxocarbon => "Oxocarbon",
            Self::Zenburn => "Zenburn",
        }
    }

    pub fn is_light(self) -> bool {
        matches!(
            self,
            Self::SolarizedLight
                | Self::GruvboxLight
                | Self::TokyoNightDay
                | Self::CatppuccinLatte
                | Self::RosePineDawn
                | Self::AyuLight
        )
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
            "gruvbox-dark" | "gruvbox" => Some(Self::GruvboxDark),
            "catppuccin-mocha" | "catppuccin" => Some(Self::CatppuccinMocha),
            "tokyo-night" | "tokyonight" => Some(Self::TokyoNight),
            "solarized-dark" | "solarized" => Some(Self::SolarizedDark),
            "monokai" => Some(Self::Monokai),
            "ansi" => Some(Self::Ansi),
            "solarized-light" => Some(Self::SolarizedLight),
            "gruvbox-light" => Some(Self::GruvboxLight),
            "tokyo-night-day" => Some(Self::TokyoNightDay),
            "catppuccin-latte" => Some(Self::CatppuccinLatte),
            "catppuccin-frappe" => Some(Self::CatppuccinFrappe),
            "catppuccin-macchiato" => Some(Self::CatppuccinMacchiato),
            "rose-pine" => Some(Self::RosePine),
            "rose-pine-moon" => Some(Self::RosePineMoon),
            "rose-pine-dawn" => Some(Self::RosePineDawn),
            "kanagawa" => Some(Self::Kanagawa),
            "everforest" => Some(Self::Everforest),
            "ayu-dark" => Some(Self::AyuDark),
            "ayu-mirage" => Some(Self::AyuMirage),
            "ayu-light" => Some(Self::AyuLight),
            "nightfox" => Some(Self::Nightfox),
            "nordfox" => Some(Self::Nordfox),
            "night-owl" => Some(Self::NightOwl),
            "material-palenight" => Some(Self::MaterialPalenight),
            "oxocarbon" => Some(Self::Oxocarbon),
            "zenburn" => Some(Self::Zenburn),
            _ => None,
        }
    }

    pub fn theme(self) -> Theme {
        match self {
            Self::Lipan => Theme::lipan(),
            Self::OneDark => Theme::one_dark(),
            Self::Dracula => Theme::dracula(),
            Self::Nord => Theme::nord(),
            Self::GruvboxDark => Theme::gruvbox_dark(),
            Self::CatppuccinMocha => Theme::catppuccin_mocha(),
            Self::TokyoNight => Theme::tokyo_night(),
            Self::SolarizedDark => Theme::solarized_dark(),
            Self::Monokai => Theme::monokai(),
            Self::Ansi => Theme::ansi(),
            Self::SolarizedLight => Theme::solarized_light(),
            Self::GruvboxLight => Theme::gruvbox_light(),
            Self::TokyoNightDay => Theme::tokyo_night_day(),
            Self::CatppuccinLatte => Theme::catppuccin_latte(),
            Self::CatppuccinFrappe => Theme::catppuccin_frappe(),
            Self::CatppuccinMacchiato => Theme::catppuccin_macchiato(),
            Self::RosePine => Theme::rose_pine(),
            Self::RosePineMoon => Theme::rose_pine_moon(),
            Self::RosePineDawn => Theme::rose_pine_dawn(),
            Self::Kanagawa => Theme::kanagawa(),
            Self::Everforest => Theme::everforest(),
            Self::AyuDark => Theme::ayu_dark(),
            Self::AyuMirage => Theme::ayu_mirage(),
            Self::AyuLight => Theme::ayu_light(),
            Self::Nightfox => Theme::nightfox(),
            Self::Nordfox => Theme::nordfox(),
            Self::NightOwl => Theme::night_owl(),
            Self::MaterialPalenight => Theme::material_palenight(),
            Self::Oxocarbon => Theme::oxocarbon(),
            Self::Zenburn => Theme::zenburn(),
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

    #[test]
    fn every_builtin_theme_has_a_canonical_resolvable_id() {
        for preset in ThemePreset::all() {
            assert_eq!(ThemePreset::parse(preset.id()), Some(preset));
            let _ = preset.theme();
        }
    }
}
