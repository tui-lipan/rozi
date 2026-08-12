use tui_lipan::prelude::*;
use tui_lipan::style::ThemeRole;

use crate::config::{BadgeColor, PaneConfig};
use crate::state::{
    AlertMode, LayoutKind, Pane, PaneBorderMode, PaneId, PaneTitlebarMode, TileGap, Workspace,
};
use crate::tiling::PanePlacement;
use crate::{AppRoot, Msg};

use super::integrated_scrollbar_config;
use super::keys::{pane_body_key, pane_terminal_key, pane_window_key};

/// Caller-decided border-merge posture for one pane (see `view::render`).
#[derive(Clone, Copy, Default)]
pub(crate) struct PaneMerge {
    /// The pane is in the settled merged layer: its border may Exact-merge with neighbors.
    pub enabled: bool,
    /// The pane's left column is a neighbor's right border. A `Padded` title keeps its row off
    /// that column; a capped title instead draws its left cap there so the chip stays aligned
    /// with the border below (see `seam_left_bg`).
    pub left_seam: bool,
    /// Title background of the same-row neighbor sharing the left seam cell, if any. A capped
    /// left cap paints its off (left) half in this color so the shared cell reads as a split
    /// junction between the two titlebars. `None` when the neighbor shows a border there (a
    /// taller pane above the seam) - the cap falls back to the backdrop and sits on the border.
    pub seam_left_bg: Option<Paint>,
    /// Title background of the same-row neighbor sharing the right seam cell, if any. Mirrors
    /// `seam_left_bg` for the right cap so either side of a seam renders the same split.
    pub seam_right_bg: Option<Paint>,
    /// Dividers mode is carrying this pane's border/integrated title on the horizontal divider
    /// above it, so the Frame must not also paint a header row below that divider.
    pub title_on_divider: bool,
    /// The pane's top row is a merged seam shared with the tile above it, so its border/integrated
    /// title is painted by `view::render` as a strip above every tile instead of by the Frame.
    pub title_on_seam: bool,
}

impl PaneMerge {
    /// The pane's border/integrated title is painted outside its own Frame - on a dividers-mode
    /// divider or on a merged seam - so the Frame must not also paint a header row.
    fn title_outside_frame(self) -> bool {
        self.title_on_divider || self.title_on_seam
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaneKind {
    Tiled,
    Floating,
    Fullscreen,
    Popup,
    Scratch,
}

impl PaneKind {
    fn is_special(self) -> bool {
        matches!(self, Self::Floating | Self::Popup | Self::Scratch)
    }
}

fn pane_scrollbar_variant(border_mode: PaneBorderMode) -> ScrollbarVariant {
    if border_mode.merges_frames() {
        ScrollbarVariant::Standalone
    } else {
        ScrollbarVariant::Integrated
    }
}

/// The configured attention state this pane currently draws. Configured-off states deliberately
/// fall through, so disabling `blocked` can still expose a finished/working/idle state.
/// The alert a pane draws, as the state that raised it plus its configured colour. The state is
/// carried alongside the colour because the colour is user-configurable and so cannot identify it:
/// `finished = "error"` would otherwise be indistinguishable from a blocked pane, and the two
/// breathe at different rates.
pub(crate) fn pane_alert(
    pane: &Pane,
    focused: bool,
    config: &PaneConfig,
) -> Option<(crate::state::PaneAlert, BadgeColor)> {
    if focused
        || pane.closing
        || matches!(pane.terminal.status, ManagedTerminalStatus::Exited(_))
        || config.alert_border == AlertMode::Off
    {
        return None;
    }
    use crate::state::PaneAlert;
    let colors = config.alert_colors;
    [
        (PaneAlert::Blocked, pane.awaits_input(), colors.blocked),
        (
            PaneAlert::Finished,
            pane.terminal.finished_unseen,
            colors.finished,
        ),
        (
            PaneAlert::Working,
            pane.terminal.is_working(),
            colors.working,
        ),
        (
            PaneAlert::Idle,
            pane.terminal
                .agent_status()
                .as_deref()
                .is_some_and(|status| {
                    status
                        .trim()
                        .eq_ignore_ascii_case(crate::session::protocol::pane_status::IDLE)
                }),
            colors.idle,
        ),
    ]
    .into_iter()
    .find_map(|(alert, active, color)| Some((alert, active.then_some(color).flatten()?)))
}

/// Whether a visible pane frame has an alert that can breathe. Other workspaces are represented by
/// tab markers, and divider-only/none modes have no per-pane frame to animate.
pub(crate) fn has_pane_alert(state: &crate::state::State) -> bool {
    state.config.pane.border_mode.draws_frames()
        && state.current().workspaces[state.current().active_workspace]
            .panes
            .iter()
            .any(|pane| {
                pane_alert(
                    pane,
                    state.current().workspaces[state.current().active_workspace]
                        .focused_pane
                        .or(state.current().focused_pane)
                        == Some(pane.id),
                    &state.config.pane,
                )
                .is_some()
            })
}

fn pane_alert_pulses(
    theme: &Theme,
    color: BadgeColor,
    config: &PaneConfig,
    armed: bool,
    animations: crate::anim::WindowAnimationConfig,
) -> bool {
    config.alert_border == AlertMode::Pulse
        && armed
        && animations.enabled
        && animations.focus_chrome
        && crate::ops::theme::pane_frame_alert_can_pulse(theme, color)
}

/// A pane's titlebar background: the active border color when focused, otherwise the neutral
/// surface element color, honoring any per-pane `title-bg` chrome override.
pub(crate) fn pane_title_bg(
    app: &AppRoot,
    ctx: &Context<AppRoot>,
    pane_id: PaneId,
    focused: bool,
) -> Paint {
    let theme = &ctx.state.theme;
    let target = if focused {
        theme.border_active
    } else {
        theme.surface.element
    };
    app.chrome_color(ctx, pane_id, "title-bg", target)
}

/// Whether `pane` has another tile immediately above it across the vertical tile gap, so its
/// border/integrated title has to leave the Frame: onto the divider in dividers mode (gap `1`), or
/// onto the shared seam row in merged mode (gap `-1`, where the tiles overlap by a row).
pub(crate) fn pane_has_tile_above(placements: &[PanePlacement], pane: PaneId, gap: f32) -> bool {
    let Some(rect) = placements
        .iter()
        .find(|placement| placement.id == pane)
        .map(|placement| placement.rect)
    else {
        return false;
    };
    let eps = 1.5;
    placements.iter().any(|other| {
        if other.id == pane {
            return false;
        }
        let other_bottom = other.rect.y + other.rect.h;
        (rect.y - (other_bottom + gap)).abs() < eps
            && other.rect.x < rect.x + rect.w - eps
            && rect.x < other.rect.x + other.rect.w - eps
    })
}

/// Text and colors for a pane titlebar that is drawn outside the pane's own Frame - on a
/// dividers-mode divider or on a merged seam. Derived exactly like the in-frame header in
/// `pane_element`, so a title looks the same wherever it ends up being painted.
struct TitleParts {
    icon: &'static str,
    badge: Option<&'static str>,
    title: String,
    title_bg: Paint,
    frame_bg: Paint,
    fill_style: Style,
    text_style: Style,
}

fn title_parts(
    app: &AppRoot,
    ctx: &Context<AppRoot>,
    pane: &Pane,
    focused_pane: Option<PaneId>,
) -> TitleParts {
    let theme = &ctx.state.theme;
    let id = pane.id;
    let titlebar = ctx.state.config.pane.titlebar;
    let focused = focused_pane == Some(id);
    let titlebar_focused = focused && ctx.state.config.pane.highlight_focused_titlebar;
    let icon = if pane.fullscreen {
        "󰊓"
    } else if pane.floating {
        "󰹙"
    } else {
        "󰖲"
    };
    let badge = if pane.fullscreen {
        Some("fullscreen")
    } else if pane.floating {
        Some("floating")
    } else {
        None
    };
    let mut title = pane.titlebar_title(ctx.state.current().remote_target.is_some());
    if let ManagedTerminalStatus::Exited(code) = pane.terminal.status {
        title.push_str(&format!(" [exited {code}]"));
    }
    if pane.logging {
        title.push_str(" [log]");
    }

    let title_bar_bg_target = if titlebar_focused {
        theme.border_active
    } else {
        theme.surface.element
    };
    let frame_bg_target = crate::ops::theme::pane_frame_background(
        theme,
        focused,
        ctx.state.config.pane.highlight_focused_background,
    );
    let title_fg_background = if titlebar == PaneTitlebarMode::Border {
        frame_bg_target
    } else {
        title_bar_bg_target
    };
    let title_fg_default = if titlebar == PaneTitlebarMode::Border {
        crate::ops::theme::pane_border_title_foreground(
            theme,
            titlebar_focused,
            title_fg_background,
        )
    } else {
        crate::ops::theme::pane_title_foreground(theme, titlebar_focused, title_fg_background)
    };
    let title_bar_fg = app.chrome_color(ctx, id, "title-fg", title_fg_default);
    let title_bg = pane_title_bg(app, ctx, id, titlebar_focused);
    let text_style = if titlebar_focused {
        Style::new()
            .fg(title_bar_fg)
            .bold()
            .contrast_policy(ContrastPolicy::Off)
    } else {
        Style::new()
            .fg(title_bar_fg)
            .contrast_policy(ContrastPolicy::Off)
    };
    TitleParts {
        icon,
        badge,
        title,
        title_bg,
        frame_bg: app.chrome_color(ctx, id, "frame-bg", frame_bg_target),
        fill_style: Style::new()
            .bg(title_bg)
            .contrast_policy(ContrastPolicy::Off),
        text_style,
    }
}

/// Title content drawn into a horizontal divider above `pane` in dividers mode.
///
/// `border` embeds the icon and title in the line (like a Frame border header). `integrated`
/// fills the whole divider row with the titlebar strip, replacing the line.
pub(crate) fn divider_title_element(
    app: &AppRoot,
    ctx: &Context<AppRoot>,
    pane: &Pane,
    focused_pane: Option<PaneId>,
) -> Option<Element> {
    if !ctx.state.config.pane.show_titles {
        return None;
    }
    let titlebar = ctx.state.config.pane.titlebar;
    if !matches!(
        titlebar,
        PaneTitlebarMode::Border | PaneTitlebarMode::Integrated
    ) {
        return None;
    }

    let id = pane.id;
    let TitleParts {
        icon,
        badge,
        title,
        title_bg: title_bar_bg,
        frame_bg,
        fill_style: title_bar_fill_style,
        text_style,
    } = title_parts(app, ctx, pane, focused_pane);

    match titlebar {
        PaneTitlebarMode::Border => {
            let mut label = format!("{icon}  {title}");
            if let Some(badge) = badge {
                label.push_str(&format!("  · {badge}"));
            }
            Some(
                Text::new(label)
                    .style(text_style)
                    .overflow(Overflow::Ellipsis)
                    .height(Length::Px(1))
                    .into(),
            )
        }
        PaneTitlebarMode::Integrated => {
            let title_text: Element = Text::new(format!("{icon}  {title}"))
                .style(text_style)
                .overflow(Overflow::Ellipsis)
                .width(Length::Flex(1))
                .height(Length::Px(1))
                .into();
            let badge_text: Option<Element> = badge.map(|badge| {
                Text::new(format!(" {badge}"))
                    .style(text_style)
                    .height(Length::Px(1))
                    .into()
            });
            let title_style = ctx.state.config.pane.title_style;
            let caps = title_style.glyphs();
            let title_row: Element = match caps {
                None => {
                    let mut row = HStack::new()
                        .style(title_bar_fill_style)
                        .padding((0, 1))
                        .width(Length::Flex(1))
                        .height(Length::Px(1))
                        .child(title_text);
                    if let Some(badge_text) = badge_text {
                        row = row.child(badge_text);
                    }
                    row.into()
                }
                Some((left, right)) => {
                    let mut middle = HStack::new()
                        .style(title_bar_fill_style)
                        .width(Length::Flex(1))
                        .height(Length::Px(1))
                        .child(title_text);
                    if let Some(badge_text) = badge_text {
                        middle = middle.child(badge_text);
                    }
                    HStack::new()
                        .width(Length::Flex(1))
                        .height(Length::Px(1))
                        .child(
                            Text::new(left)
                                .style(
                                    Style::new()
                                        .fg(title_bar_bg)
                                        .bg(frame_bg)
                                        .contrast_policy(ContrastPolicy::Off),
                                )
                                .width(Length::Px(1))
                                .height(Length::Px(1)),
                        )
                        .child(middle)
                        .child(
                            Text::new(right)
                                .style(
                                    Style::new()
                                        .fg(title_bar_bg)
                                        .bg(frame_bg)
                                        .contrast_policy(ContrastPolicy::Off),
                                )
                                .width(Length::Px(1))
                                .height(Length::Px(1)),
                        )
                        .into()
                }
            };
            Some(
                MouseRegion::new()
                    .capture_click(true)
                    .on_mouse_down(ctx.link().callback(move |_| Msg::FocusPane(id)))
                    .child(title_row)
                    .into(),
            )
        }
        PaneTitlebarMode::Bar => None,
    }
}

/// A titlebar lifted onto a merged-border seam, and how far it is inset from each side of the
/// pane's rect.
pub(crate) struct SeamTitle {
    pub inset: f32,
    pub element: Element,
}

/// The titlebar for a pane whose top row is a merged seam shared with the tile above it.
///
/// Merged tiles overlap by a row, so the pane's Frame cannot keep that row: the tile above paints
/// its bottom border over it whenever it draws later, and `ordered_panes` draws the focused pane
/// last. `view::render` therefore drops the in-frame header for these panes and paints this strip
/// above every tile instead. The layouts mirror `pane_element`'s header (and the top-edge
/// decoration that completes it) cell for cell, so a title looks the same on or off a seam.
///
/// Only plain tiles reach a seam - floating and fullscreen panes never merge - so there is no
/// badge to place here.
pub(crate) fn seam_title_element(
    app: &AppRoot,
    ctx: &Context<AppRoot>,
    pane: &Pane,
    focused_pane: Option<PaneId>,
) -> Option<SeamTitle> {
    if !ctx.state.config.pane.show_titles {
        return None;
    }
    let id = pane.id;
    let title_style = ctx.state.config.pane.title_style;
    let parts = title_parts(app, ctx, pane, focused_pane);
    let label = format!("{}  {}", parts.icon, parts.title);

    match ctx.state.config.pane.titlebar {
        // The neighbour above redraws the border line itself, so only the label is missing. Place
        // it where `BorderLabels::padding(1)` puts it: past the corner and its one `─` of padding.
        PaneTitlebarMode::Border => Some(SeamTitle {
            inset: 2.0,
            element: Text::new(label)
                .style(parts.text_style.bg(parts.frame_bg))
                .overflow(Overflow::Ellipsis)
                .height(Length::Px(1))
                .into(),
        }),
        PaneTitlebarMode::Integrated => {
            let title_text: Element = Text::new(label)
                .style(parts.text_style)
                .overflow(Overflow::Ellipsis)
                .width(Length::Flex(1))
                .height(Length::Px(1))
                .into();
            let cap_style = Style::new()
                .fg(parts.title_bg)
                .bg(parts.frame_bg)
                .contrast_policy(ContrastPolicy::Off);
            let cap = |glyph: &'static str| -> Element {
                Text::new(glyph)
                    .style(cap_style)
                    .width(Length::Px(1))
                    .height(Length::Px(1))
                    .into()
            };
            let (inset, row): (f32, Element) = match title_style.glyphs() {
                // `integrated_titlebar_top_edge` paints the corner cells in the title color too, so
                // the strip owns the whole row. Padding 2 lands the text exactly where the frame's
                // header (inset 1 past the corner, padded 1) would have put it.
                None => (
                    0.0,
                    HStack::new()
                        .style(parts.fill_style)
                        .padding((0, 2))
                        .width(Length::Flex(1))
                        .height(Length::Px(1))
                        .child(title_text)
                        .into(),
                ),
                // `integrated_half_titlebar_top_edge` turns the corner cells into half-block caps.
                Some((left, right)) if title_style == CapStyle::Half => (
                    0.0,
                    HStack::new()
                        .width(Length::Flex(1))
                        .height(Length::Px(1))
                        .child(cap(left))
                        .child(
                            HStack::new()
                                .style(parts.fill_style)
                                .padding((0, 0, 0, 1))
                                .width(Length::Flex(1))
                                .height(Length::Px(1))
                                .child(title_text),
                        )
                        .child(cap(right))
                        .into(),
                ),
                // Round and arrow caps sit inside the frame's corner glyphs, which stay on the seam
                // and keep fusing with the neighbours' borders - so inset past them.
                Some((left, right)) => (
                    1.0,
                    HStack::new()
                        .width(Length::Flex(1))
                        .height(Length::Px(1))
                        .child(cap(left))
                        .child(
                            HStack::new()
                                .style(parts.fill_style)
                                .width(Length::Flex(1))
                                .height(Length::Px(1))
                                .child(title_text),
                        )
                        .child(cap(right))
                        .into(),
                ),
            };
            Some(SeamTitle {
                inset,
                element: MouseRegion::new()
                    .capture_click(true)
                    .on_mouse_down(ctx.link().callback(move |_| Msg::FocusPane(id)))
                    .child(row)
                    .into(),
            })
        }
        PaneTitlebarMode::Bar => None,
    }
}

/// A padded integrated titlebar owns the frame's top border row: the decoration paints the
/// corners and slack cells, while the header paints the title text across the inner span.
fn integrated_titlebar_top_edge(bg: Paint) -> EdgeDecoration {
    EdgeDecoration::new(Edge::Top)
        .glyph(DecorationGlyph::Custom(' '))
        .cap_start(DecorationGlyph::Custom(' '))
        .cap_end(DecorationGlyph::Custom(' '))
        .style(
            Style::new()
                .fg(bg)
                .bg(bg)
                .contrast_policy(ContrastPolicy::Off),
        )
}

/// Half-block integrated titles use their caps as the frame's corner cells rather than placing
/// them inside rounded/plain corners. The header paints the colored span between these caps.
fn integrated_half_titlebar_top_edge(title_bg: Paint, frame_bg: Paint) -> EdgeDecoration {
    EdgeDecoration::new(Edge::Top)
        .glyph(DecorationGlyph::Custom(' '))
        .cap_start(DecorationGlyph::Custom('▐'))
        .cap_end(DecorationGlyph::Custom('▌'))
        .style(
            Style::new()
                .fg(title_bg)
                .bg(frame_bg)
                .contrast_policy(ContrastPolicy::Off),
        )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn pane_element(
    app: &AppRoot,
    ctx: &Context<AppRoot>,
    pane: &Pane,
    animated_rect: FloatRect,
    effective_focus: Option<PaneId>,
    title_marker: Option<&str>,
    kind: PaneKind,
    merge: PaneMerge,
) -> Element {
    let theme = &ctx.state.theme;
    let id = pane.id;
    let focused = effective_focus == Some(id);
    let icon = if pane.fullscreen {
        "󰊓"
    } else if pane.floating {
        "󰹙"
    } else {
        "󰖲"
    };
    let badge = if pane.fullscreen {
        Some("fullscreen")
    } else if pane.floating {
        Some("floating")
    } else {
        None
    };
    let border_mode = ctx.state.config.pane.border_mode;
    let show_border = border_mode.draws_frames()
        || (kind.is_special() && ctx.state.config.pane.keep_special_borders);
    let border_style = if kind.is_special() {
        BorderStyle::Double
    } else {
        ctx.state.config.pane.border_style.to_border_style()
    };

    let alert = border_mode
        .draws_frames()
        .then(|| pane_alert(pane, focused, &ctx.state.config.pane))
        .flatten();
    let alert_pulses = alert.is_some_and(|(_, color)| {
        pane_alert_pulses(
            theme,
            color,
            &ctx.state.config.pane,
            ctx.state.alert_pulse_armed,
            ctx.state.config.animations,
        )
    });
    // A finished pane is good news you have not read; only a blocked one is asking for an answer,
    // so the two breathe at different rates off the same beat.
    let alert_calm = alert.is_some_and(|(state, _)| state.is_calm());
    let alert_phase = if alert_calm {
        ctx.state.alert_pulse_calm_phase
    } else {
        ctx.state.alert_pulse_phase
    };
    let frame_fg_target = match alert {
        Some((_, color)) => {
            crate::ops::theme::pane_frame_alert_color(theme, color, alert_pulses, alert_phase)
        }
        None => crate::ops::theme::pane_frame_foreground(
            theme,
            focused,
            ctx.state.config.pane.highlight_focused_border,
        ),
    };
    let frame_fg = app.chrome_color_with(
        ctx,
        pane.id,
        "frame-fg",
        frame_fg_target,
        if alert_pulses && ctx.state.alert_pulse_armed {
            app.alert_pulse_transition_config(ctx, alert_calm)
        } else {
            app.focus_chrome_transition_config(ctx)
        },
    );
    let frame_bg_target = crate::ops::theme::pane_frame_background(
        theme,
        focused,
        ctx.state.config.pane.highlight_focused_background,
    );
    let frame_bg = app.chrome_color(ctx, pane.id, "frame-bg", frame_bg_target);
    let exited = matches!(pane.terminal.status, ManagedTerminalStatus::Exited(_));
    let frame_style = if exited {
        Style::new().fg(frame_fg).bg(frame_bg).dim()
    } else {
        Style::new().fg(frame_fg).bg(frame_bg)
    };
    let titlebar = ctx.state.config.pane.titlebar;
    let show_titles = ctx.state.config.pane.show_titles;
    let titlebar_focused = focused && ctx.state.config.pane.highlight_focused_titlebar;
    let title_bar_bg_target = if titlebar_focused {
        theme.border_active
    } else {
        theme.surface.element
    };
    let title_bar_bg = pane_title_bg(app, ctx, pane.id, titlebar_focused);
    // The *target* background, not the one mid-fade: this picks a contrasting foreground, and
    // deriving it from a moving colour would make the foreground wobble through the fade.
    let title_fg_background = if titlebar == PaneTitlebarMode::Border {
        frame_bg_target
    } else {
        title_bar_bg_target
    };
    let title_fg_default = if titlebar == PaneTitlebarMode::Border {
        crate::ops::theme::pane_border_title_foreground(
            theme,
            titlebar_focused,
            title_fg_background,
        )
    } else {
        crate::ops::theme::pane_title_foreground(theme, titlebar_focused, title_fg_background)
    };
    let title_bar_fg = app.chrome_color(ctx, pane.id, "title-fg", title_fg_default);
    let title_bar_fill_style = Style::new()
        .bg(title_bar_bg)
        .contrast_policy(ContrastPolicy::Off);
    let title_bar_text_style = if titlebar_focused {
        Style::new()
            .fg(title_bar_fg)
            .bold()
            .contrast_policy(ContrastPolicy::Off)
    } else {
        Style::new()
            .fg(title_bar_fg)
            .contrast_policy(ContrastPolicy::Off)
    };
    let border_title_text_style = if titlebar_focused {
        Style::new()
            .fg(title_bar_fg)
            .bold()
            .contrast_policy(ContrastPolicy::Off)
    } else {
        Style::new()
            .fg(title_bar_fg)
            .contrast_policy(ContrastPolicy::Off)
    };
    let title = show_titles.then(|| {
        let mut title = pane.titlebar_title(ctx.state.current().remote_target.is_some());
        if let ManagedTerminalStatus::Exited(code) = pane.terminal.status {
            title.push_str(&format!(" [exited {code}]"));
        }
        if pane.logging {
            title.push_str(" [log]");
        }
        match title_marker {
            Some(marker) => format!("{marker} · {title}"),
            None => title,
        }
    });

    // The wrapper stack must stay unstyled: a styled stack fills its whole rect with its
    // background, and merged panes overlap neighbors by a cell, so that fill would wipe the
    // neighbor's border glyph before this pane's border draws and fuses with it. The bar title
    // row and the body frame each paint their own background, covering the full rect anyway.
    let mut window_stack = VStack::new().align(Align::Stretch);
    if show_titles && titlebar == PaneTitlebarMode::Bar {
        let title_text: Element = Text::new(format!(
            "{icon}  {}",
            title.as_ref().expect("visible titlebar has a title")
        ))
        .style(title_bar_text_style)
        .overflow(Overflow::Ellipsis)
        .width(Length::Flex(1))
        .height(Length::Px(1))
        .into();
        let badge_text: Option<Element> = badge.map(|badge| {
            Text::new(format!(" {badge}"))
                .style(title_bar_text_style)
                .height(Length::Px(1))
                .into()
        });

        // `Padded` keeps the title flush with the frame below, with blank side padding. The cap
        // styles instead draw the titlebar color as end caps over the backdrop, so the row reads
        // as a rounded/pointed pill: the fill (and text) live in a Flex middle between the caps.
        let title_row: Element = match ctx.state.config.pane.title_style.glyphs() {
            None => {
                let mut row = HStack::new()
                    .style(title_bar_fill_style)
                    .padding((0, 1))
                    .width(Length::Flex(1))
                    .height(Length::Px(1))
                    .child(title_text);
                if let Some(badge_text) = badge_text {
                    row = row.child(badge_text);
                }
                row.into()
            }
            Some((left, right)) => {
                // A cap paints its filled half in the titlebar color and its off half in `bg`. On
                // a merged seam the off half takes the neighbor's title color, so the shared cell
                // reads as a split junction (the caller hands whichever pane draws last the same
                // left/right colors, so the seam looks the same regardless of draw order). Off a
                // seam the off half is the backdrop, giving the usual pill edge.
                let backdrop = Paint::Solid(theme.surface.backdrop);
                let left_cap_bg = if merge.left_seam {
                    merge.seam_left_bg.unwrap_or(backdrop)
                } else {
                    backdrop
                };
                let right_cap_bg = merge.seam_right_bg.unwrap_or(backdrop);
                // No horizontal padding here: the caps themselves stand in for the side padding.
                let mut middle = HStack::new()
                    .style(title_bar_fill_style)
                    .width(Length::Flex(1))
                    .height(Length::Px(1))
                    .child(title_text);
                if let Some(badge_text) = badge_text {
                    middle = middle.child(badge_text);
                }
                HStack::new()
                    .width(Length::Flex(1))
                    .height(Length::Px(1))
                    .child(
                        Text::new(left)
                            .style(
                                Style::new()
                                    .fg(title_bar_bg)
                                    .bg(left_cap_bg)
                                    .contrast_policy(ContrastPolicy::Off),
                            )
                            .width(Length::Px(1))
                            .height(Length::Px(1)),
                    )
                    .child(middle)
                    .child(
                        Text::new(right)
                            .style(
                                Style::new()
                                    .fg(title_bar_bg)
                                    .bg(right_cap_bg)
                                    .contrast_policy(ContrastPolicy::Off),
                            )
                            .width(Length::Px(1))
                            .height(Length::Px(1)),
                    )
                    .into()
            }
        };

        let mut title_bar: Element = MouseRegion::new()
            .capture_click(true)
            .on_mouse_down(ctx.link().callback(move |_| Msg::FocusPane(id)))
            .child(title_row)
            .into();
        if merge.left_seam && ctx.state.config.pane.title_style.glyphs().is_none() {
            // A `Padded` title has no cap to sit on the seam, so keep its row off the shared
            // border column: the spacer is an empty Text that leaves the seam cell untouched for
            // the neighbor's border glyph. (Capped titles instead draw their left cap there, so
            // the chip lands flush on the seam - no spacer.)
            title_bar = HStack::new()
                .height(Length::Px(1))
                .child(Text::new("").width(Length::Px(1)).height(Length::Px(1)))
                .child(title_bar)
                .into();
        }
        window_stack = window_stack.child(title_bar);
    }

    let terminal_ready = pane.terminal_active && !pane.opening && !pane.closing;
    // The widget reads the screen itself rather than being handed a snapshot, which keeps this
    // element identical from one chunk of output to the next - that is what lets `session::output`
    // ask for a repaint instead of a rebuild of every pane in the window. Decorations ride along
    // separately so search hits and hint labels survive a paint-only frame.
    let mut terminal_widget = Terminal::new()
        .screen(pane.terminal.screen_handle())
        .decorations(terminal_decorations_for_pane(ctx, pane))
        .paste_shortcut_behavior(TerminalPasteShortcutBehavior::Performable)
        // A mask over what the child program asks for: a pane wearing hint labels or one whose
        // command has exited shows no caret regardless.
        .show_cursor(app_allows_cursor(
            ctx.state.hint_mode.as_ref().map(|hints| hints.target),
            id,
            exited,
        ))
        .style(theme.primary.patch(Style::new().bg(frame_bg)))
        .selection_style(theme.text_selection)
        .focus_style(Style::default())
        .focusable(terminal_ready)
        .width(Length::Flex(1))
        .height(Length::Flex(1))
        .scrollbar_config({
            integrated_scrollbar_config()
                .variant(pane_scrollbar_variant(border_mode))
                .thumb_style(Style::new().fg(frame_fg))
                .thumb_focus_style(Style::new().fg(frame_fg))
                .track_style(Style::new().fg(frame_fg).bg(frame_bg))
        })
        .scroll_wheel(terminal_ready)
        .on_resize(ctx.link().callback(move |viewport: TerminalViewport| {
            Msg::PaneResize(id, viewport.cols, viewport.rows)
        }))
        .on_scroll_to(
            ctx.link()
                .callback(move |offset| Msg::PaneScroll(id, offset)),
        );
    if let Some(caret_color) = theme.caret.color {
        terminal_widget = terminal_widget.caret_color(caret_color);
    }
    if terminal_ready {
        terminal_widget = terminal_widget
            .on_input(ctx.link().callback(move |input| Msg::PaneInput(id, input)))
            .on_key(
                ctx.link()
                    .key_handler(move |key| Some(Msg::PaneKey(id, key))),
            )
            .on_mouse_forward(ctx.link().callback(move |bytes| Msg::PaneMouse(id, bytes)));
    }
    if let Some(selection) = copy_mode_selection(ctx, id) {
        // Navigation cursor (no range yet) uses accent so it stays distinct from
        // `text_selection`; an anchored range keeps the selection style.
        if copy_mode_is_cursor_only(ctx, id)
            && let Some(accent) = theme
                .role(ThemeRole::Accent)
                .resolved_fg()
                .filter(|color| !color.is_sentinel())
        {
            terminal_widget = terminal_widget.selection_style(Style::new().bg(accent).fg(frame_bg));
        }
        terminal_widget = terminal_widget.selection(Some(selection));
    }
    let terminal: Element = terminal_widget.into();
    let terminal = terminal.key(pane_terminal_key(id));

    // Border fusing is buffer-level: any two box-drawing glyphs sharing a cell merge unless the
    // later frame draws in Replace mode, so only panes in the settled merged layer may merge
    // (the caller decides - floating, fullscreen, scratch, mid-drag, and mid-animation panes must
    // occlude whatever is beneath them, not grow junctions into it). Fuzzy rather than Exact:
    // rounded corners have no arc junction glyphs, so Exact refuses to fuse them, while Fuzzy
    // merges exactly when possible and falls back to plain junctions for arcs.
    let mut body = Frame::new()
        .border(show_border)
        .border_style(border_style)
        .border_merge_mode(if border_mode.merges_frames() && merge.enabled {
            BorderMergeMode::Fuzzy
        } else {
            BorderMergeMode::Replace
        })
        .padding(ctx.state.config.pane.padding)
        .style(frame_style)
        .focus_style(Style::default());
    if show_titles {
        match titlebar {
            PaneTitlebarMode::Border if !merge.title_outside_frame() => {
                let title = title.as_ref().expect("visible titlebar has a title");
                let border_title = format!("{icon}  {title}");
                let mut labels = BorderLabels::new()
                    .left(border_title)
                    .style(border_title_text_style)
                    .padding(1);
                if let Some(badge) = badge {
                    labels = labels.right(format!("· {badge}"));
                }
                body = body.header(labels);
            }
            PaneTitlebarMode::Integrated if !merge.title_outside_frame() => {
                let title_text: Element = Text::new(format!(
                    "{icon}  {}",
                    title.as_ref().expect("visible titlebar has a title")
                ))
                .style(title_bar_text_style)
                .overflow(Overflow::Ellipsis)
                .width(Length::Flex(1))
                .height(Length::Px(1))
                .into();
                let badge_text: Option<Element> = badge.map(|badge| {
                    Text::new(format!(" {badge}"))
                        .style(title_bar_text_style)
                        .height(Length::Px(1))
                        .into()
                });
                let title_style = ctx.state.config.pane.title_style;
                let caps = title_style.glyphs();
                let title_row: Element = match caps {
                    None => {
                        let mut row = HStack::new()
                            .style(title_bar_fill_style)
                            .padding((0, 1))
                            .width(Length::Flex(1))
                            .height(Length::Px(1))
                            .child(title_text);
                        if let Some(badge_text) = badge_text {
                            row = row.child(badge_text);
                        }
                        row.into()
                    }
                    Some(_) if title_style == CapStyle::Half && show_border => {
                        let mut row = HStack::new()
                            .style(title_bar_fill_style)
                            .padding((0, 0, 0, 1))
                            .width(Length::Flex(1))
                            .height(Length::Px(1))
                            .child(title_text);
                        if let Some(badge_text) = badge_text {
                            row = row.child(badge_text);
                        }
                        row.into()
                    }
                    Some((left, right)) => {
                        let mut middle = HStack::new()
                            .style(title_bar_fill_style)
                            .width(Length::Flex(1))
                            .height(Length::Px(1))
                            .child(title_text);
                        if let Some(badge_text) = badge_text {
                            middle = middle.child(badge_text);
                        }
                        // With a frame, keep its corner glyphs visible by placing caps immediately
                        // inside them. Without one, the caps become the header's own outer cells.
                        HStack::new()
                            .width(Length::Flex(1))
                            .height(Length::Px(1))
                            .child(
                                Text::new(left)
                                    .style(
                                        Style::new()
                                            .fg(title_bar_bg)
                                            .bg(frame_bg)
                                            .contrast_policy(ContrastPolicy::Off),
                                    )
                                    .width(Length::Px(1))
                                    .height(Length::Px(1)),
                            )
                            .child(middle)
                            .child(
                                Text::new(right)
                                    .style(
                                        Style::new()
                                            .fg(title_bar_bg)
                                            .bg(frame_bg)
                                            .contrast_policy(ContrastPolicy::Off),
                                    )
                                    .width(Length::Px(1))
                                    .height(Length::Px(1)),
                            )
                            .into()
                    }
                };
                let header: Element = MouseRegion::new()
                    .capture_click(true)
                    .on_mouse_down(ctx.link().callback(move |_| Msg::FocusPane(id)))
                    .child(title_row)
                    .into();
                body = body.header_content(header);
                if show_border {
                    body = match title_style {
                        CapStyle::Padded => {
                            body.decoration(integrated_titlebar_top_edge(title_bar_bg))
                        }
                        CapStyle::Half => body
                            .decoration(integrated_half_titlebar_top_edge(title_bar_bg, frame_bg)),
                        CapStyle::Round | CapStyle::Arrow => body,
                    };
                }
            }
            PaneTitlebarMode::Bar => {}
            // Title rides the divider above this pane - see `view::divider_title_element`.
            PaneTitlebarMode::Border | PaneTitlebarMode::Integrated => {}
        }
    }
    let body: Element = body.child(terminal).into();
    let body = body.key(pane_body_key(id));
    window_stack = window_stack.child(body);

    // A pending prefix chord temporarily takes the same ownership of mouse gestures as the held
    // WM modifier. The framework clears the pending chord on button release, so a re-render after
    // the gesture restores ordinary terminal mouse handling.
    let prefix_active = ctx.command_chord_pending();
    let mouse_gesture_mods = if prefix_active {
        KeyMods::NONE
    } else {
        ctx.state.config.input.modifier.key_mods()
    };
    let mut window_region = MouseRegion::new()
        .capture_requires_mods(mouse_gesture_mods)
        .drag_requires_mods(mouse_gesture_mods)
        .right_drag_requires_mods(mouse_gesture_mods)
        .on_drag_start(ctx.link().callback(move |event: MouseDragEvent| {
            Msg::BeginMove(
                id,
                animated_rect,
                event.from_local_x,
                event.from_local_y,
                event.target_w,
                event.target_h,
                prefix_active || event.mods.alt || event.mods.super_key,
            )
        }))
        .on_drag(ctx.link().callback(move |event: MouseDragEvent| {
            Msg::MovePane(
                id,
                event.delta_x,
                event.delta_y,
                prefix_active || event.mods.alt || event.mods.super_key,
            )
        }))
        .on_drag_end(
            ctx.link()
                .callback(move |event: MouseDragEvent| Msg::EndMove(id, event.x, event.y)),
        )
        .on_right_drag_start(ctx.link().callback(move |event: MouseDragEvent| {
            Msg::BeginResize(
                id,
                crate::geometry::nearest_resize_corner(event),
                event.from_x,
                event.from_y,
                prefix_active || event.mods.alt || event.mods.super_key,
            )
        }))
        .on_right_drag(ctx.link().callback(move |event: MouseDragEvent| {
            Msg::ResizePane(
                id,
                crate::geometry::nearest_resize_corner(event),
                event.from_x,
                event.from_y,
                event.x,
                event.y,
                prefix_active || event.mods.alt || event.mods.super_key,
            )
        }))
        .on_right_drag_end(ctx.link().callback(move |_| Msg::EndResize(id)));

    if ctx.state.config.pane.focus_on_hover {
        window_region =
            window_region.on_mouse_move(ctx.link().callback(move |_| Msg::HoverPane(id)));
    }

    window_region = window_region
        .bubble_mouse_down(true)
        .on_mouse_down(ctx.link().callback(move |_| Msg::FocusPane(id)));

    let opacity = if pane.closing || pane.opening {
        0.0
    } else {
        1.0
    };
    let pane_tree: Element = ThemeProvider::new(ctx.state.theme.clone().focus(Style::default()))
        .child(window_region.child(window_stack))
        .into();
    let animated = Animated::new(pane_tree)
        .height(Length::Flex(1))
        .opacity(opacity)
        .transition(app.window_opacity_config(ctx, pane));
    // No `Animated::auto_exit` here. Framework retention freezes the already reconciled subtree
    // and can only clip it, so a pane would be sliced rather than scaled. The close animation is
    // the spawn animation in reverse: `pane.closing` keeps the pane described at a rectangle that
    // shrinks toward its centre, which re-lays the whole subtree out every frame so the border
    // scales with it. `prune_closed_pane` drops the state once it finishes.
    let element: Element = animated.into();

    element.key(pane_window_key(id, pane.pty_generation))
}

/// Overlays this pane's screen wears this frame: hint labels, or search-match highlights.
///
/// Returned separately from the screen so the widget can re-apply them to whatever the screen
/// reports at paint time. Both sources depend only on hint/search state, and a change in either
/// already warrants a full frame of its own.
fn terminal_decorations_for_pane(ctx: &Context<AppRoot>, pane: &Pane) -> Vec<TerminalDecoration> {
    if let Some(hints) = ctx
        .state
        .hint_mode
        .as_ref()
        .filter(|hints| hints.target == pane.id)
    {
        return hints
            .matches
            .iter()
            .enumerate()
            .filter_map(|(index, matched)| {
                let label = hints.labels.get(index)?;
                if !label.starts_with(&hints.input) {
                    return None;
                }
                Some([
                    TerminalDecoration::highlight(
                        matched.row,
                        matched.start_col..matched.end_col,
                        ctx.state.theme.text_selection,
                    ),
                    TerminalDecoration::label(
                        matched.row,
                        matched.end_col,
                        Span::new(label.as_str()).style(active_search_match_style()),
                    ),
                ])
            })
            .flatten()
            .collect();
    }
    let Some(query) = search_highlight_query(ctx, pane.id) else {
        return Vec::new();
    };
    let needle = query.to_ascii_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }

    // Matching runs over the plain snapshot text; only the highlights it produces are handed on.
    let snapshot = pane.terminal.snapshot();
    let active = active_search_highlight(ctx, pane);
    let mut decorations = Vec::new();
    for (row, line) in snapshot.text.lines().enumerate() {
        let haystack = line.to_ascii_lowercase();
        let mut search_from = 0usize;
        while search_from < haystack.len() {
            let Some(relative_start) = haystack[search_from..].find(&needle) else {
                break;
            };
            let start = search_from + relative_start;
            let end = start + needle.len();
            let line_spans = [Span::new(line)];
            let start_col = tui_lipan::utils::spans::char_col_to_display_col(
                &line_spans,
                line[..start].chars().count(),
            );
            let end_col = tui_lipan::utils::spans::char_col_to_display_col(
                &line_spans,
                line[..end].chars().count(),
            );
            if start_col < end_col {
                let style = if active.is_some_and(|active| {
                    active.line == row && active.start_col == start_col && active.end_col == end_col
                }) {
                    active_search_match_style()
                } else {
                    search_match_style()
                };
                decorations.push(TerminalDecoration::highlight(
                    row,
                    start_col..end_col,
                    style,
                ));
            }
            search_from = end;
        }
    }
    decorations
}

fn search_highlight_query(ctx: &Context<AppRoot>, id: PaneId) -> Option<&str> {
    let search = ctx.state.search.as_ref()?;
    let query = search.input.text().trim();
    if query.is_empty() || search.matches.is_empty() {
        return None;
    }
    search
        .matches
        .iter()
        .any(|matched| matched.pane == id)
        .then_some(query)
}

fn active_search_highlight(
    ctx: &Context<AppRoot>,
    pane: &Pane,
) -> Option<crate::pane::TerminalSearchHighlight> {
    let search = ctx.state.search.as_ref()?;
    let matched = search.matches.get(search.current)?;
    // `scrollback_offset()` reads the screen directly; going through `snapshot()` here would clone
    // a whole snapshot just to compare one integer.
    if matched.pane != pane.id || matched.offset != pane.terminal.scrollback_offset() {
        return None;
    }
    Some(crate::pane::TerminalSearchHighlight {
        line: matched.line,
        start_col: matched.start_col,
        end_col: matched.end_col,
    })
}

fn search_match_style() -> Style {
    Style::new()
        .fg(Color::White)
        .bg(Color::rgb(92, 64, 8))
        .contrast_policy(ContrastPolicy::BlackOrWhite)
}

fn active_search_match_style() -> Style {
    Style::new()
        .fg(Color::Black)
        .bg(Color::Yellow)
        .bold()
        .contrast_policy(ContrastPolicy::BlackOrWhite)
}

/// Draggable strips on the boundary between adjacent tiled panes. Each strip resizes the split
/// on that boundary. Only dwindle (both axes) and master (the master/stack divider) have
/// adjustable ratios, so grid/columns/rows/scrollable/monocle get no strips. With border merging on
/// the gap is zero, so the strip straddles the shared seam column/row (one cell thick) instead of
/// filling a gap.
pub(crate) fn tiled_resize_strips(
    ctx: &Context<AppRoot>,
    placements: &[PanePlacement],
    workspace: &Workspace,
) -> Vec<(FloatRect, Element)> {
    let master = matches!(workspace.layout_kind, LayoutKind::Master);
    if !master && workspace.layout_kind != LayoutKind::Dwindle {
        return Vec::new();
    }

    let tiled_ids = workspace.tiled_ids();
    let tiled: Vec<(PaneId, FloatRect)> = placements
        .iter()
        .filter(|placement| tiled_ids.contains(&placement.id))
        .map(|placement| (placement.id, placement.rect))
        .collect();

    let gap = ctx.state.tile_gap();
    let title_row =
        ctx.state.config.pane.show_titles && ctx.state.config.pane.titlebar.takes_title_row();
    let (vertical_strips, horizontal_strips) =
        resize_strip_hitboxes(&tiled, gap, master, title_row);
    // Junction segment lookup compares the drag origin against canvas-space starts, so shift by
    // the chrome offsets the same way event coordinates are rooted.
    let top_offset = f32::from(ctx.state.content_top_offset());
    let left_offset = f32::from(ctx.state.terminal_content_left_offset(ctx.viewport()));

    let mut strips = Vec::new();
    for strip in &vertical_strips {
        strips.push((strip.rect, resize_strip_element(ctx, strip, true)));
    }
    for strip in &horizontal_strips {
        strips.push((strip.rect, resize_strip_element(ctx, strip, false)));
    }
    for junction in resize_junction_hitboxes(&vertical_strips, &horizontal_strips) {
        strips.push((
            junction.rect,
            resize_junction_element(ctx, junction, left_offset, top_offset),
        ));
    }
    strips
}

fn resize_strip_hitboxes(
    tiled: &[(PaneId, FloatRect)],
    gap: TileGap,
    master: bool,
    // Whether a stacked pane leads with a titlebar row of its own (`[pane] titlebar = "bar"`),
    // which is then the only handle a horizontal boundary offers.
    title_row: bool,
) -> (Vec<ResizeStripHitbox>, Vec<ResizeStripHitbox>) {
    let h_gap = gap.horizontal;
    let v_gap = gap.vertical;
    let eps = 1.5;
    let mut vertical_strips = Vec::new();
    let mut horizontal_strips = Vec::new();
    for (a_id, a) in tiled {
        for (_b_id, b) in tiled {
            // Vertical boundary → horizontal (left|right) split. `a` is the left pane.
            let a_right = a.x + a.w;
            if (b.x - (a_right + h_gap)).abs() < eps {
                let y0 = a.y.max(b.y);
                let y1 = (a.y + a.h).min(b.y + b.h);
                if y1 - y0 > eps {
                    // Span only what lies *between* the two panes, so the strip never covers a
                    // vertical border: in the separate border mode that column carries the
                    // terminal's integrated scrollbar, and a strip over it would swallow every
                    // scrollbar press. Merged borders overlap by a column, which leaves nothing
                    // in between - `max(1)` puts the strip back on the shared seam.
                    vertical_strips.push(ResizeStripHitbox {
                        rect: FloatRect {
                            x: a_right.min(b.x),
                            y: y0,
                            w: (b.x - a_right).max(1.0),
                            h: y1 - y0,
                        },
                        pane_id: *a_id,
                    });
                }
            }
            // Horizontal boundary → vertical (top|bottom) split. Not adjustable in master.
            if !master {
                let a_bottom = a.y + a.h;
                if (b.y - (a_bottom + v_gap)).abs() < eps {
                    let x0 = a.x.max(b.x);
                    let x1 = (a.x + a.w).min(b.x + b.w);
                    if x1 - x0 > eps {
                        // Three shapes, depending on what sits between the panes:
                        // - `titlebar = "bar"`: the lower pane's bar is the handle. When dividers
                        //   mode leaves a gap above it, include that gap so the drawn divider is
                        //   grabable with the bar rather than a dead cell.
                        // - A positive gap with no bar (dividers + border/integrated/no titles):
                        //   the gap itself is the handle - never the panes' content rows.
                        // - Touching/overlapping borders: straddle both chrome rows.
                        let gap_rows = (b.y - a_bottom).max(0.0);
                        let (y, h) = if title_row {
                            (a_bottom, gap_rows + 1.0)
                        } else if gap_rows > 0.0 {
                            (a_bottom, gap_rows)
                        } else {
                            (a_bottom - 1.0, (b.y - a_bottom + 2.0).max(1.0))
                        };
                        horizontal_strips.push(ResizeStripHitbox {
                            rect: FloatRect {
                                x: x0,
                                y,
                                w: x1 - x0,
                                h,
                            },
                            pane_id: *a_id,
                        });
                    }
                }
            }
        }
    }
    (vertical_strips, horizontal_strips)
}

#[derive(Clone, Copy)]
struct ResizeStripHitbox {
    rect: FloatRect,
    /// The pane on the near side of the boundary (left or above); its trailing edge identifies the
    /// tree split that a drag on this strip resizes.
    pane_id: PaneId,
}

#[derive(Clone)]
struct ResizeJunctionHitbox {
    rect: FloatRect,
    /// Pane representatives and segment starts on vertical boundaries. The drag origin chooses
    /// one segment, which identifies one horizontal tree split.
    horizontal_targets: Vec<ResizeJunctionTarget>,
    /// Pane representatives and segment starts on horizontal boundaries. The drag origin chooses
    /// one segment, which identifies one vertical tree split.
    vertical_targets: Vec<ResizeJunctionTarget>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ResizeJunctionTarget {
    pane_id: PaneId,
    start: f32,
}

impl ResizeStripHitbox {
    fn junction_probe_rect(self, vertical: bool) -> FloatRect {
        if vertical {
            FloatRect {
                x: self.rect.x,
                y: self.rect.y - 1.0,
                w: self.rect.w,
                h: self.rect.h + 2.0,
            }
        } else {
            FloatRect {
                x: self.rect.x - 1.0,
                y: self.rect.y,
                w: self.rect.w + 2.0,
                h: self.rect.h,
            }
        }
    }
}

fn intersect_rect(a: FloatRect, b: FloatRect) -> Option<FloatRect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.w).min(b.x + b.w);
    let y1 = (a.y + a.h).min(b.y + b.h);
    (x1 > x0 && y1 > y0).then_some(FloatRect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    })
}

fn resize_junction_hitboxes(
    vertical_strips: &[ResizeStripHitbox],
    horizontal_strips: &[ResizeStripHitbox],
) -> Vec<ResizeJunctionHitbox> {
    let mut junctions: Vec<ResizeJunctionHitbox> = Vec::new();
    for vertical in vertical_strips {
        for horizontal in horizontal_strips {
            let Some(rect) = intersect_rect(
                vertical.junction_probe_rect(true),
                horizontal.junction_probe_rect(false),
            ) else {
                continue;
            };
            let junction = junctions
                .iter_mut()
                .find(|junction| intersect_rect(junction.rect, rect).is_some());
            if let Some(junction) = junction {
                let x0 = junction.rect.x.min(rect.x);
                let y0 = junction.rect.y.min(rect.y);
                let x1 = (junction.rect.x + junction.rect.w).max(rect.x + rect.w);
                let y1 = (junction.rect.y + junction.rect.h).max(rect.y + rect.h);
                junction.rect = FloatRect {
                    x: x0,
                    y: y0,
                    w: x1 - x0,
                    h: y1 - y0,
                };
                let horizontal_target = ResizeJunctionTarget {
                    pane_id: vertical.pane_id,
                    start: vertical.rect.y,
                };
                if !junction.horizontal_targets.contains(&horizontal_target) {
                    junction.horizontal_targets.push(horizontal_target);
                }
                let vertical_target = ResizeJunctionTarget {
                    pane_id: horizontal.pane_id,
                    start: horizontal.rect.x,
                };
                if !junction.vertical_targets.contains(&vertical_target) {
                    junction.vertical_targets.push(vertical_target);
                }
            } else {
                junctions.push(ResizeJunctionHitbox {
                    rect,
                    horizontal_targets: vec![ResizeJunctionTarget {
                        pane_id: vertical.pane_id,
                        start: vertical.rect.y,
                    }],
                    vertical_targets: vec![ResizeJunctionTarget {
                        pane_id: horizontal.pane_id,
                        start: horizontal.rect.x,
                    }],
                });
            }
        }
    }
    junctions
}

/// Which pane owns the cell a pointer is over inside a resize strip. `boundary` is the far pane's
/// first row/column on the strip's axis, so a pointer at or past it sits on the far pane's leading
/// chrome (its separate titlebar row, when present); anything before it is still the near pane's
/// trailing border. Kept for unit tests of the ownership rule; live strips leave focus alone.
#[cfg(test)]
fn strip_pointer_owner(near: PaneId, far: PaneId, boundary: u16, along: u16) -> PaneId {
    if along >= boundary { far } else { near }
}

/// A resize strip sits *above* the panes it separates. Clicks and hover leave focus alone - a
/// press on a divider is almost always the start of a drag, not a request to select the pane
/// under it - so only a drag resizes the split.
fn resize_strip_element(
    ctx: &Context<AppRoot>,
    strip: &ResizeStripHitbox,
    horizontal_split: bool,
) -> Element {
    let pane_id = strip.pane_id;

    MouseRegion::new()
        // A divider has no click gesture to tell a drag apart from, so it tracks the pointer from
        // its first cell. On the default threshold a left/right drag ignored two columns and then
        // arrived three out at once, which reads as a dead zone followed by a jump.
        .drag_threshold(1, 1)
        .on_drag_start(ctx.link().callback(move |event: MouseDragEvent| {
            Msg::BeginResizeSplit(pane_id, horizontal_split, event.from_x, event.from_y)
        }))
        .on_drag(ctx.link().callback(move |event: MouseDragEvent| {
            Msg::ResizeSplit(
                pane_id,
                horizontal_split,
                event.from_x,
                event.from_y,
                event.x,
                event.y,
            )
        }))
        .on_drag_end(ctx.link().callback(move |_| Msg::EndResizeSplit))
        .child(Text::new("").width(Length::Flex(1)).height(Length::Flex(1)))
        .into()
}

fn resize_junction_element(
    ctx: &Context<AppRoot>,
    junction: ResizeJunctionHitbox,
    left_offset: f32,
    top_offset: f32,
) -> Element {
    let start_targets = (
        junction.horizontal_targets.clone(),
        junction.vertical_targets.clone(),
    );
    let horizontal_targets = junction.horizontal_targets;
    let vertical_targets = junction.vertical_targets;
    // Segments are picked from the drag origin, so the pair a gesture grabs is fixed for its whole
    // lifetime; the drag session keeps it even if a later event is routed from another strip.
    MouseRegion::new()
        .drag_threshold(1, 1)
        .on_drag_start(ctx.link().callback(move |event: MouseDragEvent| {
            let (horizontal_panes, vertical_panes) = junction_targets_at(
                &start_targets.0,
                &start_targets.1,
                &event,
                left_offset,
                top_offset,
            );
            Msg::BeginResizeSplitJunction(
                horizontal_panes,
                vertical_panes,
                event.from_x,
                event.from_y,
            )
        }))
        .on_drag(ctx.link().callback(move |event: MouseDragEvent| {
            let (horizontal_panes, vertical_panes) = junction_targets_at(
                &horizontal_targets,
                &vertical_targets,
                &event,
                left_offset,
                top_offset,
            );
            Msg::ResizeSplitJunction(
                horizontal_panes,
                vertical_panes,
                event.from_x,
                event.from_y,
                event.x,
                event.y,
            )
        }))
        .on_drag_end(ctx.link().callback(move |_| Msg::EndResizeSplit))
        .child(Text::new("").width(Length::Flex(1)).height(Length::Flex(1)))
        .into()
}

/// Pane representatives for the horizontal and vertical splits a junction drag moves, chosen from
/// the drag origin. Strip rects are canvas-space; event coordinates are root-space.
fn junction_targets_at(
    horizontal: &[ResizeJunctionTarget],
    vertical: &[ResizeJunctionTarget],
    event: &MouseDragEvent,
    left_offset: f32,
    top_offset: f32,
) -> (Vec<PaneId>, Vec<PaneId>) {
    (
        junction_target_at(horizontal, f32::from(event.from_y) - top_offset)
            .into_iter()
            .collect(),
        junction_target_at(vertical, f32::from(event.from_x) - left_offset)
            .into_iter()
            .collect(),
    )
}

/// Select the segment under the drag origin. Merged borders can overlap by one cell, so the
/// segment with the latest start at or before the pointer owns that shared cell.
fn junction_target_at(targets: &[ResizeJunctionTarget], along: f32) -> Option<PaneId> {
    targets
        .iter()
        .filter(|target| target.start <= along)
        .max_by(|a, b| a.start.total_cmp(&b.start))
        .or_else(|| targets.iter().min_by(|a, b| a.start.total_cmp(&b.start)))
        .map(|target| target.pane_id)
}

/// Controlled selection for the copy-mode target pane. With no anchor it highlights just the
/// cursor cell; with an anchor it spans anchor→cursor inclusive (matching copy extraction).
fn copy_mode_selection(ctx: &Context<AppRoot>, id: PaneId) -> Option<TerminalSelection> {
    let copy = ctx
        .state
        .copy_mode
        .as_ref()
        .filter(|copy| copy.target == id)?;
    let total = crate::pane_lifecycle::find_pane(&ctx.state, id)
        .map(|pane| pane.terminal.total_scrollback_rows())
        .unwrap_or(0);
    let selection = copy.navigation.selection(total).unwrap_or_else(|| {
        let (row, col) = copy.navigation.cursor();
        let offset = copy.navigation.scrollback_offset();
        TerminalSelection::new(TerminalPos {
            line: absolute_line(total, offset, row),
            col,
        })
    });
    Some(selection_for_render(&selection))
}

fn copy_mode_is_cursor_only(ctx: &Context<AppRoot>, id: PaneId) -> bool {
    ctx.state
        .copy_mode
        .as_ref()
        .filter(|copy| copy.target == id)
        .is_some_and(|copy| copy.navigation.anchor().is_none())
}

fn selection_for_render(selection: &TerminalSelection) -> TerminalSelection {
    let (start, end) = selection.normalized();
    TerminalSelection {
        anchor: start,
        // Exclusive end column so the cursor/anchor cell is included in the highlight.
        cursor: TerminalPos {
            line: end.line,
            col: end.col.saturating_add(1),
        },
    }
}

/// Whether the app permits this pane a caret at all; the child program still decides whether it
/// wants one, and the widget ANDs the two.
fn app_allows_cursor(hint_target: Option<PaneId>, id: PaneId, exited: bool) -> bool {
    hint_target != Some(id) && !exited
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PaneAlert;

    fn alert_test_pane() -> Pane {
        Pane::new(
            1,
            100,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        )
    }

    #[test]
    fn pane_alert_obeys_precedence_and_all_gates() {
        let mut config = PaneConfig::default();
        let mut pane = alert_test_pane();
        pane.terminal.reported_status = Some(crate::session::protocol::PaneStatus {
            value: "blocked".into(),
            reason: None,
            set_at: 0,
        });
        pane.terminal.finished_unseen = true;
        assert_eq!(
            pane_alert(&pane, false, &config),
            Some((PaneAlert::Blocked, BadgeColor::Error))
        );

        config.alert_colors.blocked = None;
        assert_eq!(
            pane_alert(&pane, false, &config),
            Some((PaneAlert::Finished, BadgeColor::Success))
        );
        pane.terminal.finished_unseen = false;
        pane.terminal
            .reported_status
            .as_mut()
            .expect("status")
            .value = "working".into();
        config.alert_colors.working = Some(BadgeColor::Info);
        assert_eq!(
            pane_alert(&pane, false, &config),
            Some((PaneAlert::Working, BadgeColor::Info))
        );

        pane.terminal
            .reported_status
            .as_mut()
            .expect("status")
            .value = "idle".into();
        pane.terminal.detected_agent = Some(crate::session::protocol::DetectedAgent {
            kind: crate::session::protocol::AgentKind::OpenCode,
            state: crate::session::protocol::DetectedAgentState::Blocked,
        });
        config.alert_colors.blocked = Some(BadgeColor::Error);
        assert_eq!(
            pane_alert(&pane, false, &config),
            Some((PaneAlert::Blocked, BadgeColor::Error))
        );
        assert_eq!(pane_alert(&pane, true, &config), None);
        pane.closing = true;
        assert_eq!(pane_alert(&pane, false, &config), None);
        pane.closing = false;
        pane.terminal.status = ManagedTerminalStatus::Exited(0);
        assert_eq!(pane_alert(&pane, false, &config), None);
        config.alert_border = AlertMode::Off;
        assert_eq!(pane_alert(&pane, false, &config), None);
    }

    #[test]
    fn static_alert_borders_stay_at_peak_while_tab_pulse_uses_the_shared_phase() {
        let theme = crate::state::ThemePreset::OneDark.theme();
        let mut config = PaneConfig {
            alert_border: AlertMode::Static,
            ..PaneConfig::default()
        };
        assert!(!pane_alert_pulses(
            &theme,
            BadgeColor::Error,
            &config,
            true,
            crate::anim::WindowAnimationConfig::default(),
        ));
        assert_eq!(
            crate::ops::theme::pane_frame_alert_color(&theme, BadgeColor::Error, false, true),
            crate::ops::theme::pane_frame_alert_foreground(&theme, BadgeColor::Error)
        );
        config.alert_border = AlertMode::Pulse;
        let mut animations = crate::anim::WindowAnimationConfig::default();
        assert!(pane_alert_pulses(
            &theme,
            BadgeColor::Error,
            &config,
            true,
            animations,
        ));
        animations.enabled = false;
        assert!(!pane_alert_pulses(
            &theme,
            BadgeColor::Error,
            &config,
            true,
            animations,
        ));
        assert_eq!(
            crate::ops::theme::pane_frame_alert_color(&theme, BadgeColor::Error, false, true),
            crate::ops::theme::pane_frame_alert_foreground(&theme, BadgeColor::Error)
        );
    }

    #[test]
    fn merged_frames_use_a_standalone_terminal_scrollbar() {
        assert!(matches!(
            pane_scrollbar_variant(PaneBorderMode::Merged),
            ScrollbarVariant::Standalone
        ));
        assert!(matches!(
            pane_scrollbar_variant(PaneBorderMode::Separate),
            ScrollbarVariant::Integrated
        ));
    }

    #[test]
    fn pane_has_tile_above_requires_the_gap_and_a_horizontal_overlap() {
        // Dividers mode: a `1` gap row between the tiles.
        let placements = vec![
            PanePlacement {
                id: 1,
                rect: rect(0.0, 0.0, 10.0, 10.0),
            },
            PanePlacement {
                id: 2,
                rect: rect(0.0, 11.0, 10.0, 10.0),
            },
            PanePlacement {
                id: 3,
                rect: rect(12.0, 11.0, 8.0, 10.0),
            },
        ];
        assert!(pane_has_tile_above(&placements, 2, 1.0));
        assert!(!pane_has_tile_above(&placements, 1, 1.0));
        // Pane 3 sits beside pane 1, not below it: no shared row.
        assert!(!pane_has_tile_above(&placements, 3, 1.0));
    }

    #[test]
    fn pane_has_tile_above_follows_the_merged_overlap_row() {
        // Merged mode: a `-1` gap, so the lower tile's top row is the upper tile's bottom row.
        let placements = vec![
            PanePlacement {
                id: 1,
                rect: rect(0.0, 0.0, 10.0, 10.0),
            },
            PanePlacement {
                id: 2,
                rect: rect(0.0, 9.0, 10.0, 10.0),
            },
            PanePlacement {
                id: 3,
                rect: rect(12.0, 9.0, 8.0, 10.0),
            },
        ];
        assert!(pane_has_tile_above(&placements, 2, -1.0));
        assert!(!pane_has_tile_above(&placements, 1, -1.0));
        assert!(!pane_has_tile_above(&placements, 3, -1.0));
    }

    #[test]
    fn hint_mode_and_exit_state_withhold_the_cursor() {
        // The pane wearing the hint labels loses its caret; its neighbors keep theirs.
        assert!(!app_allows_cursor(Some(7), 7, false));
        assert!(app_allows_cursor(Some(7), 8, false));
        assert!(app_allows_cursor(None, 7, false));
        assert!(!app_allows_cursor(None, 7, true));
        assert!(!app_allows_cursor(Some(7), 7, true));
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> FloatRect {
        FloatRect { x, y, w, h }
    }

    /// A vertical strip covers only what lies between the panes, never their side borders. With
    /// separate borders the terminal draws its scrollbar into that border column, and a strip on
    /// top of it swallows every scrollbar press.
    #[test]
    fn a_vertical_strip_stays_off_the_panes_side_borders() {
        let tiled = vec![
            (1, rect(0.0, 0.0, 10.0, 10.0)),
            (2, rect(11.0, 0.0, 10.0, 10.0)),
        ];
        let (vertical, horizontal) = resize_strip_hitboxes(&tiled, TileGap::DEFAULT, false, false);

        assert!(horizontal.is_empty());
        assert_eq!(vertical.len(), 1);
        // Pane 1 ends on column 9 and pane 2 starts on 11, so only column 10 is up for grabs.
        assert_eq!(vertical[0].rect, rect(10.0, 0.0, 1.0, 10.0));
    }

    /// Merged borders overlap by a column, leaving nothing between the panes: the strip falls
    /// back to the shared seam rather than to zero width.
    #[test]
    fn a_merged_vertical_strip_falls_back_to_the_shared_seam() {
        let gap = TileGap {
            horizontal: -1.0,
            vertical: -1.0,
        };
        let tiled = vec![
            (1, rect(0.0, 0.0, 10.0, 10.0)),
            (2, rect(9.0, 0.0, 10.0, 10.0)),
        ];
        let (vertical, _) = resize_strip_hitboxes(&tiled, gap, false, false);

        assert_eq!(vertical.len(), 1);
        assert_eq!(vertical[0].rect, rect(9.0, 0.0, 1.0, 10.0));
    }

    /// `boundary` is canvas-space and the pointer is root-space, so `tiled_resize_strips` shifts
    /// it by the chrome offset first. Without that a left sidebar pushed every strip cell past
    /// the boundary, and a press beside the near pane's border focused the pane on the far side.
    #[test]
    fn strip_ownership_is_decided_in_root_space() {
        let (near, far, boundary, sidebar) = (1, 2, 11u16, 32u16);

        assert_eq!(strip_pointer_owner(near, far, boundary, 10), near);
        assert_eq!(strip_pointer_owner(near, far, boundary, 11), far);

        // The same two cells arrive shifted by the sidebar; the boundary has to shift with them.
        assert_eq!(
            strip_pointer_owner(near, far, boundary + sidebar, 10 + sidebar),
            near
        );
        assert_eq!(
            strip_pointer_owner(near, far, boundary + sidebar, 11 + sidebar),
            far
        );
        // Leaving the boundary in canvas space is what the bug looked like: both read as `far`.
        assert_eq!(strip_pointer_owner(near, far, boundary, 10 + sidebar), far);
    }

    /// With no titlebar row there is nothing between the panes to grab - a stacked gap is zero
    /// even with separate borders - so the strip straddles the two touching border rows.
    #[test]
    fn stacked_resize_strips_cover_both_touching_borders() {
        let tiled = vec![
            (1, rect(0.0, 0.0, 10.0, 10.0)),
            (2, rect(0.0, 10.0, 10.0, 10.0)),
        ];
        let (vertical, horizontal) = resize_strip_hitboxes(&tiled, TileGap::DEFAULT, false, false);

        assert!(vertical.is_empty());
        assert_eq!(horizontal.len(), 1);
        assert_eq!(horizontal[0].rect, rect(0.0, 9.0, 10.0, 2.0));
    }

    #[test]
    fn resize_strip_junction_is_the_overlap_of_perpendicular_strips() {
        let tiled = vec![
            (1, rect(0.0, 0.0, 10.0, 10.0)),
            (2, rect(11.0, 0.0, 10.0, 10.0)),
            (3, rect(0.0, 10.0, 10.0, 10.0)),
            (4, rect(11.0, 10.0, 10.0, 10.0)),
        ];
        let (vertical, horizontal) = resize_strip_hitboxes(&tiled, TileGap::DEFAULT, false, false);
        let junctions = resize_junction_hitboxes(&vertical, &horizontal);

        assert_eq!(junctions.len(), 1, "coincident intersections must merge");
        assert_eq!(junctions[0].rect, rect(10.0, 9.0, 1.0, 2.0));
        assert_eq!(
            junctions[0].horizontal_targets,
            vec![
                ResizeJunctionTarget {
                    pane_id: 1,
                    start: 0.0
                },
                ResizeJunctionTarget {
                    pane_id: 3,
                    start: 10.0
                }
            ]
        );
        assert_eq!(
            junctions[0].vertical_targets,
            vec![
                ResizeJunctionTarget {
                    pane_id: 1,
                    start: 0.0
                },
                ResizeJunctionTarget {
                    pane_id: 2,
                    start: 11.0
                }
            ]
        );
        assert_eq!(
            junction_target_at(&junctions[0].horizontal_targets, 9.0),
            Some(1)
        );
        assert_eq!(
            junction_target_at(&junctions[0].horizontal_targets, 10.0),
            Some(3)
        );
        assert_eq!(
            junction_target_at(&junctions[0].vertical_targets, 10.0),
            Some(1)
        );
        assert_eq!(
            junction_target_at(&junctions[0].vertical_targets, 11.0),
            Some(2)
        );
    }

    /// `titlebar = "bar"` with no gap (separate/merged): the lower pane's bar is the whole handle.
    #[test]
    fn a_titlebar_row_is_the_only_handle_on_a_stacked_boundary() {
        let gap = TileGap {
            horizontal: -1.0,
            vertical: 0.0,
        };
        let tiled = vec![
            (1, rect(0.0, 0.0, 10.0, 10.0)),
            (2, rect(0.0, 10.0, 10.0, 10.0)),
        ];
        let (_, horizontal) = resize_strip_hitboxes(&tiled, gap, false, true);

        assert_eq!(horizontal.len(), 1);
        let strip = horizontal[0];
        assert_eq!(strip.rect, rect(0.0, 10.0, 10.0, 1.0));
        assert_eq!(strip.pane_id, 1);

        // Every cell of the strip is the lower pane's own bar, so a press there focuses it.
        assert_eq!(strip_pointer_owner(1, 2, 10, 10), 2);
    }

    /// Dividers mode leaves a gap row above a bar titlebar for the drawn divider. Both that gap
    /// and the bar must be grabable - otherwise the visible divider is a dead cell.
    #[test]
    fn a_divider_above_a_titlebar_is_grabable_with_it() {
        let gap = TileGap {
            horizontal: 1.0,
            vertical: 1.0,
        };
        let tiled = vec![
            (1, rect(0.0, 0.0, 10.0, 10.0)),
            (2, rect(0.0, 11.0, 10.0, 10.0)),
        ];
        let (_, horizontal) = resize_strip_hitboxes(&tiled, gap, false, true);

        assert_eq!(horizontal.len(), 1);
        let strip = horizontal[0];
        assert_eq!(strip.rect, rect(0.0, 10.0, 10.0, 2.0));
        assert_eq!(strip.pane_id, 1);

        let owner = |y| strip_pointer_owner(1, 2, 11, y);
        assert_eq!(owner(10), 1, "the divider row focuses the upper pane");
        assert_eq!(owner(11), 2, "the titlebar row focuses the lower pane");
    }

    /// Dividers without a bar: only the gap is the handle - never a content row of either pane.
    #[test]
    fn a_divider_gap_without_a_bar_is_the_only_handle() {
        let gap = TileGap {
            horizontal: 1.0,
            vertical: 1.0,
        };
        let tiled = vec![
            (1, rect(0.0, 0.0, 10.0, 10.0)),
            (2, rect(0.0, 11.0, 10.0, 10.0)),
        ];
        let (_, horizontal) = resize_strip_hitboxes(&tiled, gap, false, false);

        assert_eq!(horizontal.len(), 1);
        assert_eq!(horizontal[0].rect, rect(0.0, 10.0, 10.0, 1.0));
        assert_eq!(horizontal[0].pane_id, 1);
    }

    #[test]
    fn side_by_side_strip_routes_the_seam_column_to_the_right_pane() {
        let gap = TileGap {
            horizontal: -1.0,
            vertical: 0.0,
        };
        let tiled = vec![
            (1, rect(0.0, 0.0, 10.0, 10.0)),
            (2, rect(9.0, 0.0, 10.0, 10.0)),
        ];
        let (vertical, _) = resize_strip_hitboxes(&tiled, gap, false, true);

        assert_eq!(vertical.len(), 1);
        assert_eq!(vertical[0].pane_id, 1);
        assert_eq!(vertical[0].rect, rect(9.0, 0.0, 1.0, 10.0));

        let owner = |x| strip_pointer_owner(1, 2, 9, x);
        assert_eq!(owner(8), 1, "left pane keeps its own right border");
        assert_eq!(
            owner(9),
            2,
            "the shared seam cell belongs to the right pane"
        );
    }
}
