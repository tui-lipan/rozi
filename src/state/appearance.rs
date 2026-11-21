use std::borrow::Cow;

use crate::pane::chrome::step_ring;
use tui_lipan::prelude::{BorderStyle, CapStyle};
use tui_lipan::style::SplitterPalette;
use tui_lipan::{Color, ScrollbarPalette, Style, SurfacePalette, Theme, ThemePalette};

/// Rose used by the Rozi theme for both `accent` and persistent interface chrome.
///
/// `accent` is focus, active state, and highlights. [`RoziColor`] is headers, directories,
/// section titles, and the rest of the persistent pink styling. They share this value so the
/// split does not change what is on screen.
pub const ROZI_ROSE: u32 = 0xFD4A80;

/// Rozi's persistent interface color, stored on the active [`Theme`].
///
/// Framework `accent` stays the interaction color. This extension is the color of Rozi's own
/// chrome. Built-in themes seed it from `accent`, and the Rozi theme sets it to [`ROZI_ROSE`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoziColor {
    pub style: Style,
}

impl RoziColor {
    /// The Rozi theme's chrome rose.
    pub fn rose() -> Self {
        Self {
            style: Style::new().fg(Color::hex_u24(ROZI_ROSE)),
        }
    }
}

/// Attach [`RoziColor`] when a theme does not already carry one.
///
/// The seed is that theme's `accent`, so headers and directory labels keep the color they had
/// when those call sites read `accent` directly.
pub fn ensure_rozi_color(theme: Theme) -> Theme {
    if theme.extension::<RoziColor>().is_some() {
        return theme;
    }
    let style = theme.accent;
    theme.with_extension(RoziColor { style })
}

/// Persistent Rozi chrome style: headers, directories, section titles.
///
/// Falls back to `accent` when a theme was built without the extension, which is the color those
/// surfaces used before the token existed.
pub fn rozi_style(theme: &Theme) -> Style {
    theme
        .extension::<RoziColor>()
        .map(|color| color.style)
        .unwrap_or(theme.accent)
}

/// Frame glyphs for pane and picker chrome. Tiled, floating, scratchpad, fullscreen, and picker
/// borders each store their own value; cycling one does not move the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneBorderStyle {
    Rounded,
    Plain,
    Double,
    Thick,
    LightDoubleDashed,
    HeavyDoubleDashed,
    LightTripleDashed,
    HeavyTripleDashed,
    LightQuadrupleDashed,
    HeavyQuadrupleDashed,
}

/// A pane state that may mark its border, in attention precedence order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneAlert {
    Blocked,
    Error,
    Finished,
    Working,
    Idle,
}

impl PaneAlert {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Error => "error",
            Self::Finished => "finished",
            Self::Working => "working",
            Self::Idle => "idle",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Blocked => "Blocked",
            Self::Error => "Error",
            Self::Finished => "Finished",
            Self::Working => "Working",
            Self::Idle => "Idle",
        }
    }

    /// Blocked work and failures use the urgent pulse; other states use the slower calm rate.
    pub const fn is_calm(self) -> bool {
        !matches!(self, Self::Blocked | Self::Error)
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

/// What a pane alert colors. Orthogonal to [`AlertMode`], which says whether that color holds still
/// or breathes: the border and the content share one beat.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaneAlertPaint {
    /// Color the pane frame only.
    Border,
    /// Wash the terminal content with a faint tint of the alert color, leaving the frame alone.
    /// Works in every border mode, `none` included.
    Content,
    /// The frame and the content together.
    #[default]
    Both,
}

impl PaneAlertPaint {
    pub fn all() -> &'static [Self] {
        &[Self::Border, Self::Content, Self::Both]
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::Border => "border",
            Self::Content => "content",
            Self::Both => "both",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Border => "Border",
            Self::Content => "Content",
            Self::Both => "Border + content",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "border" | "frame" => Some(Self::Border),
            "content" | "terminal" => Some(Self::Content),
            "both" | "all" => Some(Self::Both),
            _ => None,
        }
    }

    pub const fn paints_border(self) -> bool {
        matches!(self, Self::Border | Self::Both)
    }

    pub const fn paints_content(self) -> bool {
        matches!(self, Self::Content | Self::Both)
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

impl PaneBorderStyle {
    /// Cycle order for the border-style settings rows.
    pub fn all() -> &'static [PaneBorderStyle] {
        &[
            Self::Rounded,
            Self::Plain,
            Self::Double,
            Self::Thick,
            Self::LightDoubleDashed,
            Self::HeavyDoubleDashed,
            Self::LightTripleDashed,
            Self::HeavyTripleDashed,
            Self::LightQuadrupleDashed,
            Self::HeavyQuadrupleDashed,
        ]
    }

    /// Config token and persisted value.
    pub fn id(self) -> &'static str {
        match self {
            Self::Rounded => "rounded",
            Self::Plain => "plain",
            Self::Double => "double",
            Self::Thick => "thick",
            Self::LightDoubleDashed => "light-double-dashed",
            Self::HeavyDoubleDashed => "heavy-double-dashed",
            Self::LightTripleDashed => "light-triple-dashed",
            Self::HeavyTripleDashed => "heavy-triple-dashed",
            Self::LightQuadrupleDashed => "light-quadruple-dashed",
            Self::HeavyQuadrupleDashed => "heavy-quadruple-dashed",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Rounded => "Rounded",
            Self::Plain => "Plain",
            Self::Double => "Double",
            Self::Thick => "Thick",
            Self::LightDoubleDashed => "Light dashed",
            Self::HeavyDoubleDashed => "Heavy dashed",
            Self::LightTripleDashed => "Light triple dashed",
            Self::HeavyTripleDashed => "Heavy triple dashed",
            Self::LightQuadrupleDashed => "Light quadruple dashed",
            Self::HeavyQuadrupleDashed => "Heavy quadruple dashed",
        }
    }

    pub fn expected_ids() -> String {
        Self::all()
            .iter()
            .map(|style| style.id())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase().replace(['_', ' '], "-");
        if let Some(style) = Self::all()
            .iter()
            .copied()
            .find(|style| style.id() == normalized)
        {
            return Some(style);
        }
        match normalized.as_str() {
            "round" => Some(Self::Rounded),
            "single" | "square" => Some(Self::Plain),
            "heavy" | "bold" => Some(Self::Thick),
            "dashed" | "light-dashed" | "double-dashed" => Some(Self::LightDoubleDashed),
            "heavy-dashed" => Some(Self::HeavyDoubleDashed),
            "triple-dashed" => Some(Self::LightTripleDashed),
            "quadruple-dashed" | "quad-dashed" | "light-quad-dashed" => {
                Some(Self::LightQuadrupleDashed)
            }
            "heavy-quad-dashed" => Some(Self::HeavyQuadrupleDashed),
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
            Self::LightDoubleDashed => BorderStyle::LightDoubleDashed,
            Self::HeavyDoubleDashed => BorderStyle::HeavyDoubleDashed,
            Self::LightTripleDashed => BorderStyle::LightTripleDashed,
            Self::HeavyTripleDashed => BorderStyle::HeavyTripleDashed,
            Self::LightQuadrupleDashed => BorderStyle::LightQuadrupleDashed,
            Self::HeavyQuadrupleDashed => BorderStyle::HeavyQuadrupleDashed,
        }
    }
}

const BADGE_CAP_STYLES: &[CapStyle] = &[
    CapStyle::Padded,
    CapStyle::Round,
    CapStyle::Arrow,
    CapStyle::Slant,
    CapStyle::Trapezoid,
];

pub(crate) fn badge_cap_styles() -> &'static [CapStyle] {
    BADGE_CAP_STYLES
}

/// Parse a cap style: one of rozi's named styles and their aliases, or two single-width characters
/// for a custom left and right cap.
pub(crate) fn parse_cap_style(value: &str) -> Option<CapStyle> {
    let value = value.trim();
    let named = match value.to_ascii_lowercase().replace(['_', ' '], "-").as_str() {
        "padded" | "pad" | "plain" | "none" => Some(CapStyle::Padded),
        "half" | "half-block" | "block" => Some(CapStyle::Half),
        "round" | "rounded" | "pill" => Some(CapStyle::Round),
        "arrow" | "pointed" | "powerline" => Some(CapStyle::Arrow),
        "slant" | "slanted" => Some(CapStyle::Slant),
        "trapezoid" => Some(CapStyle::Trapezoid),
        _ => None,
    };
    named.or_else(|| {
        let mut chars = value.chars();
        let (left, right) = (chars.next()?, chars.next()?);
        let single = |ch: char| unicode_width::UnicodeWidthChar::width(ch) == Some(1);
        (chars.next().is_none() && single(left) && single(right))
            .then_some(CapStyle::Custom { left, right })
    })
}

/// Config value for a cap style: its name, or the two characters of a custom pair.
pub(crate) fn cap_style_id(style: CapStyle) -> Cow<'static, str> {
    match style {
        CapStyle::Custom { left, right } => Cow::Owned(format!("{left}{right}")),
        named => Cow::Borrowed(named_cap_style_id(named)),
    }
}

/// Config value for a named cap style. Settings only offers named styles, so it saves through this.
pub(crate) const fn named_cap_style_id(style: CapStyle) -> &'static str {
    match style {
        CapStyle::Padded => "padded",
        CapStyle::Half => "half",
        CapStyle::Round => "round",
        CapStyle::Arrow => "arrow",
        CapStyle::Slant => "slant",
        CapStyle::Trapezoid => "trapezoid",
        // A custom pair is never one of the offered choices; CapStyle is non-exhaustive.
        _ => "padded",
    }
}

/// Human-facing label for a cap style.
pub(crate) const fn cap_style_label(style: CapStyle) -> &'static str {
    match style {
        CapStyle::Padded => "Padded",
        CapStyle::Half => "Half block",
        CapStyle::Round => "Round",
        CapStyle::Arrow => "Arrow",
        CapStyle::Slant => "Slant",
        CapStyle::Trapezoid => "Trapezoid",
        _ => "Custom",
    }
}

/// Cycle all title/workbar styles in the app's established order.
pub(crate) fn next_cap_style(style: CapStyle) -> CapStyle {
    step_ring(CapStyle::all(), style, false)
}

/// Cycle workbar badges and tabs without exposing the unsupported half-block option.
pub(crate) fn next_badge_cap_style(style: CapStyle) -> CapStyle {
    step_ring(BADGE_CAP_STYLES, style, false)
}

/// A built-in theme: Rozi's own palette or any preset shipped by `tui-lipan`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemePreset(&'static str);

impl ThemePreset {
    pub const ROZI: Self = Self("rozi");
    pub const LIPAN: Self = Self("lipan");
    pub const ANSI: Self = Self("ansi");

    /// Every built-in theme: Rozi first, then every preset `tui-lipan` ships.
    pub fn all() -> Vec<Self> {
        std::iter::once(Self::ROZI)
            .chain(
                tui_lipan::style::presets::PRESET_NAMES
                    .iter()
                    .copied()
                    .map(Self),
            )
            .collect()
    }

    pub const fn id(self) -> &'static str {
        self.0
    }

    pub fn label(self) -> String {
        if self == Self::ANSI {
            return "ANSI".to_string();
        }
        self.0
            .split('-')
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    Some(first) => {
                        let mut titled = first.to_uppercase().to_string();
                        titled.push_str(chars.as_str());
                        titled
                    }
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn is_light(self) -> bool {
        self != Self::ANSI && !self.theme().surface.backdrop.is_dark()
    }

    pub const fn is_signature(self) -> bool {
        matches!(self.0.as_bytes(), b"rozi" | b"lipan")
    }

    pub fn parse(value: &str) -> Option<Self> {
        let normalized = normalize_preset_name(value);
        match normalized.as_str() {
            // `default` follows the default rather than naming a palette, so it moved with it.
            "rozi" | "default" => Some(Self::ROZI),
            "tuilipan" => Some(Self::LIPAN),
            "gruvbox" => Self::from_framework_name("gruvbox-dark"),
            "catppuccin" => Self::from_framework_name("catppuccin-mocha"),
            "solarized" => Self::from_framework_name("solarized-dark"),
            _ => tui_lipan::style::presets::PRESET_NAMES
                .iter()
                .copied()
                .find(|name| normalize_preset_name(name) == normalized)
                .map(Self),
        }
    }

    fn from_framework_name(canonical: &str) -> Option<Self> {
        tui_lipan::style::presets::PRESET_NAMES
            .iter()
            .copied()
            .find(|&name| name == canonical)
            .map(Self)
    }

    pub fn theme(self) -> Theme {
        let theme = if self == Self::ROZI {
            rozi_theme()
        } else {
            tui_lipan::style::presets::preset_by_name(self.0)
                .expect("ThemePreset only wraps tui_lipan::style::presets::PRESET_NAMES")
        };
        ensure_rozi_color(theme)
    }
}

fn normalize_preset_name(name: &str) -> String {
    name.trim()
        .chars()
        .filter(|ch| !matches!(ch, '-' | '_' | ' '))
        .flat_map(char::to_lowercase)
        .collect()
}

fn rozi_theme() -> Theme {
    let background = Color::hex_u24(0x06070F);
    let text = Color::hex_u24(0xCCD0E6);
    let rose = Color::hex_u24(ROZI_ROSE);
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
    // Chrome is the same rose as `accent`. Call sites that mean headers and persistent styling
    // read this extension; focus and active state keep reading `accent`.
    theme.with_extension(RoziColor::rose())
}

pub struct ThemePickerPreview {
    pub theme: Theme,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::chrome::PaneBorderMode;

    #[test]
    fn border_styles_parse_and_cycle_every_tui_lipan_style() {
        for style in PaneBorderStyle::all() {
            assert_eq!(PaneBorderStyle::parse(style.id()), Some(*style));
            assert_eq!(style.prev().next(), *style);
        }
        assert_eq!(
            PaneBorderStyle::parse("round"),
            Some(PaneBorderStyle::Rounded)
        );
        assert_eq!(
            PaneBorderStyle::parse("dashed"),
            Some(PaneBorderStyle::LightDoubleDashed)
        );
        assert_eq!(
            PaneBorderStyle::parse("heavy"),
            Some(PaneBorderStyle::Thick)
        );
        assert_eq!(
            PaneBorderStyle::Thick.next(),
            PaneBorderStyle::LightDoubleDashed
        );
        assert_eq!(
            PaneBorderStyle::HeavyQuadrupleDashed.next(),
            PaneBorderStyle::Rounded
        );
        assert_eq!(
            PaneBorderStyle::LightDoubleDashed.to_border_style(),
            BorderStyle::LightDoubleDashed
        );
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
    fn pane_alert_paint_round_trips_cycles_and_splits_into_surfaces() {
        assert_eq!(PaneAlertPaint::default(), PaneAlertPaint::Both);
        for paint in PaneAlertPaint::all() {
            assert_eq!(PaneAlertPaint::parse(paint.id()), Some(*paint));
            assert_eq!(paint.prev().next(), *paint);
        }
        assert_eq!(
            PaneAlertPaint::parse(" Terminal "),
            Some(PaneAlertPaint::Content)
        );
        assert_eq!(PaneAlertPaint::parse("glow"), None);
        assert!(PaneAlertPaint::Border.paints_border() && !PaneAlertPaint::Border.paints_content());
        assert!(
            !PaneAlertPaint::Content.paints_border() && PaneAlertPaint::Content.paints_content()
        );
        assert!(PaneAlertPaint::Both.paints_border() && PaneAlertPaint::Both.paints_content());
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
        assert_eq!(ThemePreset::parse("default"), Some(ThemePreset::ROZI));
        assert_eq!(ThemePreset::parse("tui-lipan"), Some(ThemePreset::LIPAN));
        assert_eq!(ThemePreset::parse("tuilipan"), Some(ThemePreset::LIPAN));
        assert_eq!(
            ThemePreset::parse("onedark"),
            ThemePreset::parse("one-dark")
        );
        assert_eq!(
            ThemePreset::parse("tokyonight"),
            ThemePreset::parse("tokyo-night")
        );
        assert_eq!(
            ThemePreset::parse("gruvbox"),
            ThemePreset::parse("gruvbox-dark")
        );
        assert_eq!(
            ThemePreset::parse("catppuccin"),
            ThemePreset::parse("catppuccin-mocha")
        );
        assert_eq!(
            ThemePreset::parse("solarized"),
            ThemePreset::parse("solarized-dark")
        );
        assert_eq!(ThemePreset::ANSI.label(), "ANSI");
    }

    #[test]
    fn rozi_theme_tracks_the_documentation_palette() {
        let theme = ThemePreset::ROZI.theme();

        assert_eq!(theme.surface.backdrop, Color::hex_u24(0x06070F));
        assert_eq!(theme.surface.panel, Color::hex_u24(0x0B0D1C));
        assert_eq!(theme.surface.element, Color::hex_u24(0x0B0D1C));
        assert_eq!(theme.accent.fg, Some(Color::hex_u24(ROZI_ROSE).into()));
        assert_eq!(
            rozi_style(&theme).fg,
            Some(Color::hex_u24(ROZI_ROSE).into())
        );
        assert_eq!(theme.border_active, Color::hex_u24(ROZI_ROSE));
        assert_eq!(theme.selection.bg, Some(Color::hex_u24(0x982BF2).into()));
        assert_eq!(theme.primary.fg, Some(Color::hex_u24(0xCCD0E6).into()));
    }

    #[test]
    fn every_builtin_theme_seeds_chrome_from_its_accent() {
        for preset in ThemePreset::all() {
            let theme = preset.theme();
            assert_eq!(
                rozi_style(&theme).fg,
                theme.accent.fg,
                "{} chrome diverges from accent",
                preset.id()
            );
        }
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
