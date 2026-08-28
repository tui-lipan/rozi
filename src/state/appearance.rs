use tui_lipan::prelude::{BorderStyle, CapStyle};
use tui_lipan::style::SplitterPalette;
use tui_lipan::{Color, ScrollbarPalette, Style, SurfacePalette, Theme, ThemePalette};

/// Structural presentation of a visible pane title. `Bar` is the existing separate title row above
/// the frame; `Border` and `Integrated` reuse the frame's top border row; `Inset` writes the title
/// inside the frame, on the first interior row beneath the top border. Visibility is controlled
/// independently by `pane.show_titles`, so toggling it never loses the selected layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// A pane state that may mark its border, in attention precedence order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneAlert {
    Blocked,
    Finished,
    Working,
    Idle,
}

impl PaneAlert {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Finished => "finished",
            Self::Working => "working",
            Self::Idle => "idle",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Blocked => "Blocked",
            Self::Finished => "Finished",
            Self::Working => "Working",
            Self::Idle => "Idle",
        }
    }

    /// Whether this state breathes at the slower calm rate. Only `Blocked` is a request for an
    /// answer; everything else is information you will get to, and should not compete with it.
    pub const fn is_calm(self) -> bool {
        !matches!(self, Self::Blocked)
    }
}

/// What a marked workspace tab colors. Orthogonal to [`AlertMode`], which says whether that color
/// holds still or breathes.
///
/// Workbar-only: a pane border *is* a foreground glyph, so it has no such choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AlertPaint {
    /// Fill the tab, and shape it with the same end caps the active tab uses. Reads from across the
    /// screen; the default because the workbar is usually in peripheral vision.
    #[default]
    Background,
    /// Color the label only, leaving the tab flat. Quieter, and the right choice when a filled tab
    /// would compete with the active one.
    Text,
}

impl AlertPaint {
    pub fn all() -> &'static [Self] {
        &[Self::Background, Self::Text]
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Text => "text",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Background => "Background",
            Self::Text => "Text",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "background" | "bg" | "fill" => Some(Self::Background),
            "text" | "fg" | "foreground" => Some(Self::Text),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        step_ring(Self::all(), self, false)
    }

    pub fn prev(self) -> Self {
        step_ring(Self::all(), self, true)
    }
}

/// How a surface draws configured alert colors. Shared by pane borders (`[pane] alert_border`) and
/// workspace tab markers (`[workbar.alert] mode`) so the two read and behave identically.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AlertMode {
    Off,
    Static,
    #[default]
    Pulse,
}

impl AlertMode {
    pub fn all() -> &'static [Self] {
        &[Self::Off, Self::Static, Self::Pulse]
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Static => "static",
            Self::Pulse => "pulse",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            // Names what it does rather than merely that it is enabled, and contrasts with `Pulse`.
            Self::Static => "Static",
            Self::Pulse => "Pulse",
        }
    }

    /// Compact UI status that preserves the stored mode while explaining a static pulse fallback.
    pub fn status_label(self, animations_enabled: bool, focus_chrome: bool) -> String {
        match self {
            Self::Pulse if !animations_enabled => "Pulse (animations off)".to_string(),
            Self::Pulse if !focus_chrome => "Pulse (static)".to_string(),
            _ => self.label().to_string(),
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "static" => Some(Self::Static),
            "pulse" => Some(Self::Pulse),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        step_ring(Self::all(), self, false)
    }

    pub fn prev(self) -> Self {
        step_ring(Self::all(), self, true)
    }
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
        step_ring(Self::all(), self, false)
    }

    pub fn prev(self) -> Self {
        step_ring(Self::all(), self, true)
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

/// Parse rozi's historical cap-style aliases into tui-lipan's shared cap primitive.
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

fn step_ring<T: Copy + PartialEq>(all: &[T], current: T, reverse: bool) -> T {
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

/// Cycle all title/workbar styles in the app's established order.
pub(crate) fn next_cap_style(style: CapStyle) -> CapStyle {
    step_ring(CapStyle::all(), style, false)
}

pub(crate) fn prev_cap_style(style: CapStyle) -> CapStyle {
    step_ring(CapStyle::all(), style, true)
}

/// Cycle workbar badges and tabs without exposing the unsupported half-block option.
pub(crate) fn next_badge_cap_style(style: CapStyle) -> CapStyle {
    step_ring(BADGE_CAP_STYLES, style, false)
}

pub(crate) fn prev_badge_cap_style(style: CapStyle) -> CapStyle {
    step_ring(BADGE_CAP_STYLES, style, true)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemePreset {
    Rozi,
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
    pub fn all() -> [Self; 31] {
        [
            Self::Rozi,
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
            Self::Rozi => "rozi",
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
            Self::Rozi => "Rozi",
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
            // `default` follows the default rather than naming a palette, so it moved with it.
            "rozi" | "default" => Some(Self::Rozi),
            "lipan" | "tui-lipan" | "tuilipan" => Some(Self::Lipan),
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
            Self::Rozi => rozi_theme(),
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

fn rozi_theme() -> Theme {
    let background = Color::hex_u24(0x06070F);
    let text = Color::hex_u24(0xCCD0E6);
    let rose = Color::hex_u24(0xFD4A80);
    let violet = Color::hex_u24(0x982BF2);
    let mut theme = ThemePalette::new(text, background, rose)
        .selection(violet)
        .text_selection(rose)
        .border(Color::hex_u24(0x343858))
        .muted(Color::hex_u24(0x8E93B4))
        .scrollbar(Color::hex_u24(0x343858))
        .success(Color::hex_u24(0x4ADE80))
        .warning(Color::hex_u24(0xF0A830))
        .error(Color::hex_u24(0xFF5F57))
        .info(Color::hex_u24(0x82AAFF))
        .into_theme();

    theme.surface = SurfacePalette {
        backdrop: background,
        panel: Color::hex_u24(0x0B0D1C),
        element: Color::hex_u24(0x0B0D1C),
        menu: Color::hex_u24(0x14162A),
    };
    theme.selection = Style::new().fg(text).bg(violet);
    theme.text_selection = Style::new()
        .fg(text)
        .bg(background.blend_toward(rose, 0.32));
    // `theme.hover` stays empty, as it is in every other preset. The role is inherited by every
    // widget-level hover slot, so filling it makes whole lists, tab bars, and click-only
    // `MouseRegion`s paint on hover. Rozi's views style hover per element instead.
    theme.border_active = rose;
    theme.focus = Style::new().fg(rose);
    theme.scrollbar = ScrollbarPalette {
        track: Some(Color::hex_u24(0x0B0D1C)),
        thumb: Color::hex_u24(0x343858),
        thumb_focus: Some(rose),
    };
    theme.splitter = SplitterPalette {
        hover: violet,
        active: rose,
    };
    theme
}

pub struct ThemePickerPreview {
    pub theme: Theme,
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(PaneBorderMode::Separate.prev(), PaneBorderMode::Dividers);
        assert_eq!(PaneBorderMode::Dividers.prev(), PaneBorderMode::None);
    }

    #[test]
    fn alert_border_mode_round_trips_and_cycles() {
        assert_eq!(AlertMode::default(), AlertMode::Pulse);
        for mode in AlertMode::all() {
            assert_eq!(AlertMode::parse(mode.id()), Some(*mode));
        }
        assert_eq!(AlertMode::Off.next(), AlertMode::Static);
        assert_eq!(AlertMode::Static.next(), AlertMode::Pulse);
        assert_eq!(AlertMode::Pulse.next(), AlertMode::Off);
        assert_eq!(AlertMode::Off.prev(), AlertMode::Pulse);
        assert_eq!(AlertMode::Pulse.prev(), AlertMode::Static);
    }

    #[test]
    fn alert_border_status_label_explains_static_pulse_fallbacks() {
        assert_eq!(
            AlertMode::Pulse.status_label(false, true),
            "Pulse (animations off)"
        );
        assert_eq!(AlertMode::Pulse.status_label(true, false), "Pulse (static)");
        assert_eq!(AlertMode::Pulse.status_label(true, true), "Pulse");
        assert_eq!(AlertMode::Static.status_label(false, false), "Static");
    }

    #[test]
    fn every_builtin_theme_has_a_canonical_resolvable_id() {
        for preset in ThemePreset::all() {
            assert_eq!(ThemePreset::parse(preset.id()), Some(preset));
            let _ = preset.theme();
        }
    }

    #[test]
    fn rozi_theme_tracks_the_documentation_palette() {
        let theme = ThemePreset::Rozi.theme();

        assert_eq!(theme.surface.backdrop, Color::hex_u24(0x06070F));
        assert_eq!(theme.surface.panel, Color::hex_u24(0x0B0D1C));
        assert_eq!(theme.surface.element, Color::hex_u24(0x0B0D1C));
        assert_eq!(theme.accent.fg, Some(Color::hex_u24(0xFD4A80).into()));
        assert_eq!(theme.selection.bg, Some(Color::hex_u24(0x982BF2).into()));
        assert_eq!(theme.primary.fg, Some(Color::hex_u24(0xCCD0E6).into()));
    }

    #[test]
    fn no_builtin_theme_enables_the_generic_hover_role() {
        // Widget hover slots inherit this role, so a non-empty value paints whole lists, tab bars,
        // and click-only `MouseRegion`s. Hover belongs on the element, not on the theme.
        for preset in ThemePreset::all() {
            assert!(
                preset.theme().hover.is_empty(),
                "{} sets a generic hover style",
                preset.id()
            );
        }
    }
}
