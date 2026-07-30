use tui_lipan::prelude::{BorderStyle, CapStyle, TextInput, Theme};

/// Structural presentation of a visible pane title. `Bar` is the existing separate title row;
/// the compact variants reuse the frame's top border row. Visibility is controlled independently
/// by `pane.show_titles`, so toggling it never loses the selected layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneTitlebarMode {
    Bar,
    Border,
    Integrated,
}

impl PaneTitlebarMode {
    /// Cycle order for `Action::CycleTitlebar`.
    pub fn all() -> &'static [PaneTitlebarMode] {
        &[Self::Bar, Self::Border, Self::Integrated]
    }

    /// Config token and persisted value.
    pub fn id(self) -> &'static str {
        match self {
            Self::Bar => "bar",
            Self::Border => "border",
            Self::Integrated => "integrated",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Bar => "Bar",
            Self::Border => "Border",
            Self::Integrated => "Integrated",
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
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let index = all.iter().position(|mode| *mode == self).unwrap_or(0);
        all[(index + 1) % all.len()]
    }

    /// Only the separate bar consumes a row in addition to the frame border.
    pub fn takes_title_row(self) -> bool {
        self == Self::Bar
    }
}

/// The border glyphs tiled panes draw. A single app-wide setting (`Action::CycleBorderStyle`),
/// not per-pane. Floating panes keep their own `Double` border so they stay visually distinct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneBorderStyle {
    Rounded,
    Plain,
    Double,
    Thick,
}

/// Structural presentation of pane borders. Glyph shape remains independently controlled by
/// `PaneBorderStyle`; this mode decides whether pane frames or internal split dividers are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
        let all = Self::all();
        let index = all.iter().position(|mode| *mode == self).unwrap_or(0);
        all[(index + 1) % all.len()]
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

const BADGE_CAP_STYLES: &[CapStyle] = &[CapStyle::Padded, CapStyle::Round, CapStyle::Arrow];

/// Parse hyprmux's historical cap-style aliases into tui-lipan's shared cap primitive.
pub(crate) fn parse_cap_style(value: &str) -> Option<CapStyle> {
    match value
        .trim()
        .to_ascii_lowercase()
        .replace(['_', ' '], "-")
        .as_str()
    {
        "padded" | "pad" | "plain" | "none" => Some(CapStyle::Padded),
        "half" | "half-block" | "block" => Some(CapStyle::Half),
        "round" | "rounded" | "pill" => Some(CapStyle::Round),
        "arrow" | "pointed" | "slant" | "powerline" => Some(CapStyle::Arrow),
        _ => None,
    }
}

/// Config token and persisted value for a cap style.
pub(crate) const fn cap_style_id(style: CapStyle) -> &'static str {
    match style {
        CapStyle::Padded => "padded",
        CapStyle::Half => "half",
        CapStyle::Round => "round",
        CapStyle::Arrow => "arrow",
    }
}

/// Human-facing label for a cap style.
pub(crate) const fn cap_style_label(style: CapStyle) -> &'static str {
    match style {
        CapStyle::Padded => "Padded",
        CapStyle::Half => "Half block",
        CapStyle::Round => "Round",
        CapStyle::Arrow => "Arrow",
    }
}

/// Cycle all title/workbar styles in the app's established order.
pub(crate) fn next_cap_style(style: CapStyle) -> CapStyle {
    let all = CapStyle::all();
    let index = all
        .iter()
        .position(|candidate| *candidate == style)
        .unwrap_or(0);
    all[(index + 1) % all.len()]
}

/// Cycle workbar badges and tabs without exposing the unsupported half-block option.
pub(crate) fn next_badge_cap_style(style: CapStyle) -> CapStyle {
    let index = BADGE_CAP_STYLES
        .iter()
        .position(|candidate| *candidate == style)
        .unwrap_or(0);
    BADGE_CAP_STYLES[(index + 1) % BADGE_CAP_STYLES.len()]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppearanceAction {
    Theme,
    EditPadding,
    ToggleTitles,
    CycleTitlebar,
    ToggleWorkbar,
    ToggleWorkbarGap,
    ToggleWorkbarPosition,
    ToggleWorkbarPowerline,
    ToggleAnimations,
    ToggleHighlightFocusedBackground,
    ToggleHighlightFocusedBorder,
    ToggleHighlightFocusedTitlebar,
    CycleBorderMode,
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
            Self::CycleTitlebar if !pane.show_titles => Some("Needs titlebar"),
            Self::CycleTitleStyle if !pane.show_titles => Some("Needs titlebar"),
            Self::CycleTitleStyle if pane.titlebar == PaneTitlebarMode::Border => {
                Some("Needs bar or integrated titlebar")
            }
            Self::ToggleHighlightFocusedBorder | Self::CycleBorderStyle
                if !pane.border_mode.draws_frames() =>
            {
                Some("Needs pane frames")
            }
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
    fn border_modes_parse_and_cycle() {
        assert_eq!(
            PaneBorderMode::parse("borderless"),
            Some(PaneBorderMode::None)
        );
        assert_eq!(
            PaneBorderMode::parse("divider"),
            Some(PaneBorderMode::Dividers)
        );
        assert_eq!(PaneBorderMode::Separate.next(), PaneBorderMode::Merged);
        assert_eq!(PaneBorderMode::Merged.next(), PaneBorderMode::None);
        assert_eq!(PaneBorderMode::None.next(), PaneBorderMode::Dividers);
        assert_eq!(PaneBorderMode::Dividers.next(), PaneBorderMode::Separate);
    }

    #[test]
    fn frame_only_controls_are_disabled_for_borderless_modes() {
        let mut pane = crate::config::HyprmuxPaneConfig::default();
        for mode in [PaneBorderMode::None, PaneBorderMode::Dividers] {
            pane.border_mode = mode;
            assert_eq!(
                AppearanceAction::CycleBorderStyle.disabled_reason(&pane),
                Some("Needs pane frames")
            );
            assert_eq!(
                AppearanceAction::ToggleHighlightFocusedBorder.disabled_reason(&pane),
                Some("Needs pane frames")
            );
        }
    }

    #[test]
    fn every_builtin_theme_has_a_canonical_resolvable_id() {
        for preset in ThemePreset::all() {
            assert_eq!(ThemePreset::parse(preset.id()), Some(preset));
            let _ = preset.theme();
        }
    }
}
