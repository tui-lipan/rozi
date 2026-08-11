use tui_lipan::prelude::*;

use crate::config::{BadgeColor, InputConfig, WorkbarAlertConfig, WorkbarItem, WorkbarSegment};
use crate::input::Action;
use crate::state::{AlertMode, AlertPaint, Mode, Pane, WORKBAR_HEIGHT, Workspace};
use crate::{HyprmuxApp, Msg};

pub(crate) fn workbar(app: &HyprmuxApp, ctx: &Context<HyprmuxApp>) -> Element {
    let state = &ctx.state;
    let theme = &ctx.state.theme;
    let workbar_cfg = &state.config.workbar;

    let panel_bg = theme.surface.panel;
    let mut row = HStack::new()
        .gap(1)
        .width(Length::Flex(1))
        .height(Length::Px(WORKBAR_HEIGHT))
        .style(Style::new().bg(panel_bg));

    // Track the background color of the elements landing on each outer edge so the whole-workbar
    // end caps can adopt it: a leading/trailing badge (the `hyprmux` title chip, session chip, or
    // a mode chip) rounds off in its own color, while a plain segment leaves the cap panel-colored.
    let mut left_cap_color: Option<Color> = None;
    let mut right_cap_color = panel_bg;

    for item in &workbar_cfg.left {
        if let Some(element) = left_segment_element(app, ctx, item) {
            row = row.child(element);
            let color = segment_edge_color(ctx, item);
            left_cap_color.get_or_insert(color);
            right_cap_color = color;
        }
    }

    // The workspace tabs already flex to fill slack; without them, insert a spacer so the
    // right region lands flush against the trailing edge. The spacer is transparent, so a bare
    // panel-colored cap sits on that edge.
    let has_workspaces = workbar_cfg
        .left
        .iter()
        .chain(workbar_cfg.right.iter())
        .any(|item| matches!(item.segment, WorkbarSegment::Workspaces));
    if !has_workspaces {
        row = row.child(Text::new("").width(Length::Flex(1)).height(Length::Px(1)));
        right_cap_color = panel_bg;
    }

    // Trailing cluster: transient mode chips first, then the configured right segments, so the
    // session badge - usually the last right segment, and the one that stays visible - is pinned
    // to the far edge and mode chips like PREFIX land to its left. Chips are collected first so
    // the cluster can decide how to lay them out.
    let text_fg = theme.surface.backdrop;
    let mut trailing: Vec<TrailingChip> = Vec::new();
    if ctx.command_chord_pending() {
        trailing.push(TrailingChip::badge(
            " PREFIX ",
            text_fg,
            theme.status.warning,
        ));
    }
    if state.mode == Mode::Resize {
        trailing.push(TrailingChip::badge(
            " RESIZE ",
            text_fg,
            theme.status.success,
        ));
    } else if state.mode == Mode::Copy {
        trailing.push(TrailingChip::badge(" COPY ", text_fg, theme.status.info));
    } else if state.mode == Mode::Hint {
        trailing.push(TrailingChip::badge(" HINT ", text_fg, theme.status.info));
    }
    // Not a `Mode`: the sidebar owning the keyboard is ordinary widget focus, and the framework is
    // the authority on that. Mirroring it into a mode would be a second source of truth that goes
    // stale the moment a click moves focus somewhere else.
    if state.sidebar.focused {
        trailing.push(TrailingChip::badge(" SIDEBAR ", text_fg, theme.status.info));
    }
    // Synchronization silently sends every keystroke to every pane on the workspace, and nothing
    // else on screen says so. It gets warning color for the same reason it gets a permanent chip:
    // typing into what looks like one pane while hitting several is the costly surprise.
    if state.current().workspaces[state.current().active_workspace].synchronized {
        trailing.push(TrailingChip::badge(" SYNC ", text_fg, theme.status.warning));
    }
    if state.do_not_disturb {
        trailing.push(TrailingChip::badge(" DND ", text_fg, theme.status.warning));
    }
    // Keep session identity in the configured session badge and collaboration state in one chip.
    // A normal solo client needs no status; read-only remains visible because it explains why
    // typing is blocked.
    if let Some((label, color)) = collaboration_status(state, theme) {
        trailing.push(TrailingChip::badge(label, text_fg, color));
    }
    for item in &workbar_cfg.right {
        if let Some(chip) = trailing_chip(app, ctx, item) {
            trailing.push(chip);
        }
    }

    if let Some(last) = trailing.last() {
        right_cap_color = last.edge_color(panel_bg);
    }
    let badge_style = ctx.state.config.pane.workbar_badge_style;
    let powerline = ctx.state.config.pane.workbar_powerline;
    if let Some(cluster) = trailing_cluster(ctx, trailing, badge_style, powerline, panel_bg) {
        row = row.child(cluster);
    }

    workbar_with_caps(
        ctx,
        row,
        left_cap_color.unwrap_or(panel_bg),
        right_cap_color,
    )
}

fn collaboration_status(
    state: &crate::state::State,
    theme: &Theme,
) -> Option<(&'static str, Color)> {
    let shared = state.current().shared.as_ref()?;
    if shared.read_only {
        return Some((" READ ONLY ", theme.status.warning));
    }
    // Only clients actually using the session make it shared. A parked one is a background
    // connection with no view and no claim on control, so badging CTRL/FOLLOW for it would report
    // company that is not there.
    if shared.active_clients() <= 1 {
        return None;
    }
    // A pending control request pulls the controller's badge to the warning color and appends a dot
    // so the controller notices without an intrusive prompt (the request also toasts once).
    if shared.is_controller() && shared.has_pending_control_requests() {
        return Some(if shared.input_locked {
            (" CTRL LOCK ● ", theme.status.warning)
        } else {
            (" CTRL ● ", theme.status.warning)
        });
    }
    Some(match (shared.is_controller(), shared.input_locked) {
        (true, true) => (" CTRL LOCK ", theme.status.warning),
        (true, false) => (" CTRL ", theme.status.success),
        (false, true) => (" FOLLOW LOCK ", theme.status.warning),
        (false, false) => (" FOLLOW ", theme.status.info),
    })
}

/// One element of the workbar's trailing (right-hand) cluster. `Badge` chips are colored pills
/// that chain into a powerline when badge caps are on; `Flex` wraps an opaque element (a plain
/// text segment or the workspace tabs) that just abuts its neighbors and breaks the color chain.
enum TrailingChip {
    Badge {
        label: String,
        text_fg: Color,
        bg: Color,
        opens_sessions: bool,
    },
    Flex(Box<Element>),
}

impl TrailingChip {
    fn badge(label: impl Into<String>, text_fg: Color, bg: Color) -> Self {
        Self::Badge {
            label: label.into(),
            text_fg,
            bg,
            opens_sessions: false,
        }
    }

    fn sessions_badge(label: impl Into<String>, text_fg: Color, bg: Color) -> Self {
        Self::Badge {
            label: label.into(),
            text_fg,
            bg,
            opens_sessions: true,
        }
    }

    /// Background this chip paints where it meets a neighbor: its own color for a badge, the panel
    /// surface for anything else.
    fn edge_color(&self, panel_bg: Color) -> Color {
        match self {
            Self::Badge { bg, .. } => *bg,
            Self::Flex(_) => panel_bg,
        }
    }
}

/// The trailing chip for a configured right-region segment, or `None` when it renders nothing.
/// Every segment that has a text label becomes a colored `Badge` chip (so it can chain into the
/// powerline); the workspace tab strip is the one non-badge entry and rides along as a `Flex` chip.
fn trailing_chip(
    app: &HyprmuxApp,
    ctx: &Context<HyprmuxApp>,
    item: &WorkbarItem,
) -> Option<TrailingChip> {
    match &item.segment {
        WorkbarSegment::Workspaces => Some(TrailingChip::Flex(Box::new(workspace_tabs_element(
            app, ctx,
        )))),
        _ => {
            let label = segment_label(ctx, &item.segment)?;
            let (bg, fg) = item_colors(ctx, item);
            if matches!(
                item.segment,
                WorkbarSegment::Location | WorkbarSegment::Session
            ) {
                Some(TrailingChip::sessions_badge(label, fg, bg))
            } else {
                Some(TrailingChip::badge(label, fg, bg))
            }
        }
    }
}

/// Assemble the trailing cluster. `powerline` controls chaining independently of the cap shape:
/// when off, chips stand apart with a 1-cell gap and each cap (if any) is drawn over the panel bar;
/// when on, the gap collapses to zero and each badge's cap is drawn over its left neighbor's color,
/// so the chips interlock into a powerline that flows out of the panel bar. Returns `None` when there
/// is nothing trailing. Sizes to `Auto` so the cluster only occupies its own width and stays pinned
/// to the trailing edge.
fn trailing_cluster(
    ctx: &Context<HyprmuxApp>,
    chips: Vec<TrailingChip>,
    cap_style: CapStyle,
    powerline: bool,
    panel_bg: Color,
) -> Option<Element> {
    if chips.is_empty() {
        return None;
    }
    let mut cluster = HStack::new()
        .width(Length::Auto)
        .height(Length::Px(WORKBAR_HEIGHT))
        .gap(if powerline { 0 } else { 1 });
    // With powerline on, a badge's cap blends from its left neighbor's color; the first chip starts
    // from the panel bar. With it off, every cap sits over the panel bar, each badge is a full pill
    // (caps on both sides), and chips keep a gap.
    let mut prev_bg = panel_bg;
    let mut prev_was_badge = false;
    for chip in chips {
        match chip {
            TrailingChip::Badge {
                label,
                text_fg,
                bg,
                opens_sessions,
            } => {
                let (cap_sides, same_color) = if powerline && prev_was_badge && prev_bg == bg {
                    (CapSides::Left, true)
                } else if powerline {
                    (CapSides::Left, false)
                } else {
                    (CapSides::Both, false)
                };
                let badge = workbar_badge(
                    &label, text_fg, bg, prev_bg, cap_style, cap_sides, same_color,
                );
                let badge = if opens_sessions {
                    MouseRegion::new()
                        .on_click(
                            ctx.link()
                                .callback(|_| Msg::RunAction(Action::OpenSessionPicker)),
                        )
                        .child(badge)
                        .into()
                } else {
                    badge
                };
                cluster = cluster.child(badge);
                prev_bg = if powerline { bg } else { panel_bg };
                prev_was_badge = true;
            }
            TrailingChip::Flex(element) => {
                cluster = cluster.child(*element);
                prev_bg = panel_bg;
                prev_was_badge = false;
            }
        }
    }
    Some(cluster.into())
}

/// Background color a rendered workbar segment paints at the workbar's outer edge: its badge
/// background, except the workspace tab strip, which renders on the panel surface. Only called for
/// items that actually rendered, so it need not re-check visibility.
fn segment_edge_color(ctx: &Context<HyprmuxApp>, item: &WorkbarItem) -> Color {
    match item.segment {
        WorkbarSegment::Workspaces => ctx.state.theme.surface.panel,
        _ => item_colors(ctx, item).0,
    }
}

/// The curated default badge color for a segment when the user did not override it. `title` and
/// `session` keep the active accent; the rest get distinct, theme-derived hues so they read as
/// separate chips instead of one dim run.
fn curated_color(segment: &WorkbarSegment) -> BadgeColor {
    match segment {
        WorkbarSegment::Title | WorkbarSegment::Session => BadgeColor::Accent,
        WorkbarSegment::Location => BadgeColor::Info,
        WorkbarSegment::Clock => BadgeColor::Info,
        WorkbarSegment::Activity => BadgeColor::Warning,
        WorkbarSegment::Layout
        | WorkbarSegment::Text(_)
        | WorkbarSegment::Command { .. }
        | WorkbarSegment::Workspaces => BadgeColor::Neutral,
    }
}

/// Resolve a workbar item's `(bg, fg)` colors: its explicit override, else the segment's curated
/// default, mapped through the active theme.
fn item_colors(ctx: &Context<HyprmuxApp>, item: &WorkbarItem) -> (Color, Color) {
    let color = item.color.unwrap_or_else(|| {
        if matches!(item.segment, WorkbarSegment::Location) {
            location_badge_color(&ctx.state)
        } else {
            curated_color(&item.segment)
        }
    });
    resolve_badge_color(&ctx.state.theme, color)
}

fn location_badge_color(state: &crate::state::State) -> BadgeColor {
    use crate::state::ConnectionState;
    match state.current().connection {
        ConnectionState::Reconnecting | ConnectionState::Connecting => BadgeColor::Warning,
        ConnectionState::Disconnected
        | ConnectionState::AuthRequired
        | ConnectionState::Unreachable
        | ConnectionState::Incompatible
            if state.current().remote_target.is_some() =>
        {
            BadgeColor::Error
        }
        _ if state.current().remote_target.is_some() => BadgeColor::Info,
        _ => BadgeColor::Neutral,
    }
}

/// Map a [`BadgeColor`] role to concrete `(bg, fg)` colors from the active theme. Saturated roles
/// pair with the backdrop foreground for contrast; the muted `neutral`/`panel` roles use the
/// primary text color so a low-contrast surface still reads as text.
fn resolve_badge_color(theme: &Theme, color: BadgeColor) -> (Color, Color) {
    let on_accent = theme.surface.backdrop;
    let text = theme
        .primary
        .fg
        .map(Paint::color)
        .unwrap_or(theme.surface.backdrop);
    (
        crate::ops::theme::badge_role_color(theme, color),
        match color {
            BadgeColor::Neutral | BadgeColor::Panel => text,
            _ => on_accent,
        },
    )
}

/// Wrap the full-width workbar in end caps so the whole panel bar reads as a pill/point over the
/// backdrop, mirroring the pane titlebar and badge cap styles. `Padded` keeps the flush
/// edge-to-edge bar as-is.
///
/// The caps are painted over the bar's own two edge cells rather than added beside it: those cells
/// hold the outer segments' blank side padding, so - exactly like a capped titlebar dropping its
/// side padding - the cap stands in for that padding and the bar stays full width with its content
/// unshifted. Each cap is drawn in `left_color`/`right_color` - the background of the badge or
/// segment sitting on that edge - so a leading/trailing chip rounds off in its own color and a
/// plain bar edge stays panel-colored. A transparent spacer between the caps lets the panel bar
/// show through everywhere else. The `Frame` (borderless, no padding) only exists to give the
/// overlay stack the same `Flex(1)` width the bare row carried, since `ZStack` has no width
/// control of its own.
fn workbar_with_caps(
    ctx: &Context<HyprmuxApp>,
    row: HStack,
    left_color: Color,
    right_color: Color,
) -> Element {
    let Some((left, right)) = ctx.state.config.pane.workbar_style.glyphs() else {
        return row.into();
    };
    let backdrop = ctx.state.theme.surface.backdrop;
    let cap = |glyph: &'static str, color: Color| {
        Text::new(glyph)
            .style(
                Style::new()
                    .fg(color)
                    .bg(backdrop)
                    .contrast_policy(ContrastPolicy::Off),
            )
            .width(Length::Px(1))
            .height(Length::Px(WORKBAR_HEIGHT))
    };
    let caps_overlay = HStack::new()
        .width(Length::Flex(1))
        .height(Length::Px(WORKBAR_HEIGHT))
        .child(cap(left, left_color))
        .child(Spacer::new())
        .child(cap(right, right_color));
    Frame::new()
        .border(false)
        .padding(0)
        .width(Length::Flex(1))
        .height(Length::Px(WORKBAR_HEIGHT))
        // passthrough: the cap overlay is purely decorative, so pointer events must fall through
        // to the interactive bar beneath it (workspace tabs, mode chips) instead of the caps/spacer
        // swallowing every hover and click over the workbar.
        .child(
            ZStack::new()
                .passthrough(true)
                .child(row)
                .child(caps_overlay),
        )
        .into()
}

/// A colored workbar chip (`label` in `text_fg` on `badge_bg`, bold) with optional caps owned by the
/// framework's `Badge` primitive. The cap replaces the label's side padding, preserving the chip's
/// footprint while allowing powerline seams to paint over the preceding background.
fn workbar_badge(
    label: &str,
    text_fg: Color,
    badge_bg: Color,
    cap_behind: Color,
    cap_style: CapStyle,
    cap_sides: CapSides,
    same_color: bool,
) -> Element {
    let footprint = RichText::from(label.to_string()).width() as u16;
    let badge = Badge::new(label.to_string())
        .child(
            Spacer::new()
                .width(Length::Px(footprint))
                .height(Length::Px(1)),
        )
        .position(BadgePosition::TopStart)
        .style(Style::new().bg(badge_bg))
        .text_style(Style::new().fg(text_fg).bold())
        .width(Length::Auto)
        .height(Length::Px(1))
        .cap(cap_style)
        .cap_sides(cap_sides)
        .cap_behind(cap_behind)
        .cap_same_color(same_color);
    Frame::new()
        .border(false)
        .padding(0)
        .width(Length::Px(footprint))
        .height(Length::Px(1))
        .child(badge)
        .into()
}

/// The badge label text (with its blank side padding) for a workbar segment, or `None` when the
/// segment renders nothing (`Workspaces` is the tab strip, not a badge; `Session`/`Activity` hide
/// when there is nothing to show).
fn segment_label(ctx: &Context<HyprmuxApp>, segment: &WorkbarSegment) -> Option<String> {
    match segment {
        WorkbarSegment::Title => Some(" hyprmux ".to_string()),
        WorkbarSegment::Location => location_label(&ctx.state),
        WorkbarSegment::Session => {
            let name = attached_session_name(ctx)?;
            let clients = ctx.state.attached_client_count();
            Some(if clients > 1 {
                format!(" 󰛤 {name} ·{clients} ")
            } else {
                format!(" 󰛤 {name} ")
            })
        }
        WorkbarSegment::Clock => {
            let text = format!(
                " {} ",
                chrono::Local::now().format(&ctx.state.config.workbar.clock_format)
            );
            // Record what actually reached the screen so `workbar_tick` can skip the frame while
            // the formatted string is unchanged (see `State::last_clock_text`).
            *ctx.state.last_clock_text.borrow_mut() = Some(text.clone());
            Some(text)
        }
        WorkbarSegment::Layout => Some(format!(
            " {} ",
            ctx.state.current().workspaces[ctx.state.current().active_workspace]
                .layout_kind
                .label()
        )),
        WorkbarSegment::Activity => {
            let current = ctx
                .state
                .current()
                .workspaces
                .iter()
                .map(|workspace| {
                    workspace
                        .panes
                        .iter()
                        .filter(|pane| !pane.closing && pane.activity.has_unseen_output)
                        .count()
                })
                .sum();
            // Retained background sessions keep processing output; surface their unread separately so
            // it never looks like activity in the visible view.
            let background = ctx
                .state
                .background
                .values()
                .flat_map(|attachment| attachment.workspaces.iter())
                .map(|workspace| {
                    workspace
                        .panes
                        .iter()
                        .filter(|pane| pane.activity.has_unseen_output)
                        .count()
                })
                .sum();
            match (current, background) {
                (0, 0) => None,
                (c, 0) => Some(format!(" ●{c} ")),
                (0, b) => Some(format!(" +{b} ")),
                (c, b) => Some(format!(" ●{c} +{b} ")),
            }
        }
        WorkbarSegment::Text(literal) => Some(format!(
            " {} ",
            substitute_placeholders(ctx, literal).trim()
        )),
        WorkbarSegment::Command { command, .. } => {
            let output = ctx
                .state
                .workbar
                .command_output
                .get(command)
                .map(String::as_str)
                .unwrap_or("");
            Some(format!(" {output} "))
        }
        WorkbarSegment::Workspaces => None,
    }
}

fn location_label(state: &crate::state::State) -> Option<String> {
    use crate::state::ConnectionState;
    if let Some(host) = state.current().remote_host.as_deref() {
        let suffix = match state.current().connection {
            ConnectionState::Connecting => " · connecting",
            ConnectionState::Reconnecting => " · reconnecting",
            ConnectionState::Disconnected
            | ConnectionState::AuthRequired
            | ConnectionState::Unreachable
            | ConnectionState::Incompatible => " · offline",
            ConnectionState::Connected => "",
        };
        return Some(format!(" 󰒍 {host}{suffix} "));
    }
    let retained = state
        .background
        .values()
        .filter(|attachment| {
            attachment.remote_target.is_some()
                && !matches!(
                    attachment.connection,
                    ConnectionState::Disconnected
                        | ConnectionState::AuthRequired
                        | ConnectionState::Unreachable
                        | ConnectionState::Incompatible
                )
        })
        .count();
    (retained > 0).then(|| format!(" 󰒍 {retained} "))
}

/// The element for a left-region workbar item: the workspace tab strip, or a colored badge. Left
/// badges cap on their right (leading pills, like the title chip) and stay gap-separated by the row
/// - powerline chaining is a trailing-cluster feature.
fn left_segment_element(
    app: &HyprmuxApp,
    ctx: &Context<HyprmuxApp>,
    item: &WorkbarItem,
) -> Option<Element> {
    if matches!(item.segment, WorkbarSegment::Workspaces) {
        return Some(workspace_tabs_element(app, ctx));
    }
    let label = segment_label(ctx, &item.segment)?;
    let (bg, fg) = item_colors(ctx, item);
    let badge = workbar_badge(
        &label,
        fg,
        bg,
        ctx.state.theme.surface.panel,
        ctx.state.config.pane.workbar_badge_style,
        CapSides::Right,
        false,
    );
    if matches!(
        item.segment,
        WorkbarSegment::Location | WorkbarSegment::Session
    ) {
        Some(
            MouseRegion::new()
                .on_click(
                    ctx.link()
                        .callback(|_| Msg::RunAction(Action::OpenSessionPicker)),
                )
                .child(badge)
                .into(),
        )
    } else {
        Some(badge)
    }
}

fn substitute_placeholders(ctx: &Context<HyprmuxApp>, literal: &str) -> String {
    let state = &ctx.state;
    let active = &state.current().workspaces[state.current().active_workspace];
    literal
        .replace("{host}", &workbar_hostname())
        .replace(
            "{workspace}",
            &workspace_placeholder_label(active.name.as_deref(), state.current().active_workspace),
        )
        .replace("{layout}", active.layout_kind.label())
        .replace("{session}", &attached_session_name(ctx).unwrap_or_default())
}

/// Value substituted for the `{workspace}` workbar placeholder: the workspace's custom name when
/// set, otherwise its 1-based number.
fn workspace_placeholder_label(name: Option<&str>, index: usize) -> String {
    match name {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => (index + 1).to_string(),
    }
}

/// The live attached session name, if any - backs the `Session` segment and `{session}` placeholder.
/// Ephemeral sessions return `None`: a bare launch is a disposable per-process session, so
/// the badge/placeholder stays empty until the session is given a real name.
fn attached_session_name(ctx: &Context<HyprmuxApp>) -> Option<String> {
    if !ctx.state.current().session_attached || ctx.state.is_ephemeral_session() {
        return None;
    }
    ctx.state.current().session_name.clone()
}

fn workbar_hostname() -> String {
    crate::platform::user::hostname().unwrap_or_else(|| "localhost".to_string())
}

fn workspace_tabs_element(app: &HyprmuxApp, ctx: &Context<HyprmuxApp>) -> Element {
    let state = &ctx.state;
    let theme = &ctx.state.theme;
    let shown = workspace_tab_count(state);

    let tabs: Vec<Tab> = (0..shown)
        .map(|idx| {
            let count = state.current().workspaces[idx].visible_count();
            let workspace = &state.current().workspaces[idx];
            let marker = workspace_marker(workspace, &state.config.workbar.alert);
            let label =
                workspace_tab_label(state.current().workspaces[idx].name.as_deref(), idx, count);
            let mut tab = Tab::new(label);
            // `static` colors the marked tab without moving it; only `pulse` breathes. The active
            // tab is never restyled - its solid `active_style` pill already reads as current, and
            // the pane border carries the alert on the workspace you are looking at.
            if let Some(marker) = marker
                && idx != state.current().active_workspace
            {
                let role = workspace_marker_color(marker);
                let paint = state.config.workbar.alert.paint;
                let animations = state.config.animations;
                let pulse = state.config.workbar.alert.mode == AlertMode::Pulse
                    && state.alert_pulse_armed
                    && animations.enabled
                    && animations.focus_chrome
                    && crate::ops::theme::tab_alert_can_pulse(theme, role, paint);
                // A finished workspace is good news you have not read; only blocked is asking for an
                // answer, so it breathes slower off the same beat.
                let calm = marker.is_calm();
                let phase = if calm {
                    state.alert_pulse_calm_phase
                } else {
                    state.alert_pulse_phase
                };
                // The label fades on the same clock as the tint, between the bar's resting
                // foreground and the contrast-correct one for the *peak* background.
                //
                // `ContrastPolicy::Off` is what makes that hold: left on, the renderer re-derives
                // the foreground against whatever the background holds mid-fade, so the label snaps
                // to white and then to black as the tint crosses the readability threshold. Naming
                // both endpoints ourselves turns that flip into a fade, and matches how every other
                // piece of hyprmux chrome already picks its own readable colours.
                let (fg_target, target) =
                    crate::ops::theme::tab_alert_colors(theme, role, paint, pulse, phase);
                let transition = if pulse && state.alert_pulse_armed {
                    app.alert_pulse_transition_config(ctx, calm)
                } else {
                    crate::anim::instant_transition()
                };
                tab = tab.style(
                    Style::new()
                        .fg(app.chrome_paint(
                            ctx,
                            format!("hyprmux-workspace-tab-{idx}-fg"),
                            fg_target,
                            transition,
                        ))
                        .bg(app.chrome_paint(
                            ctx,
                            format!("hyprmux-workspace-tab-{idx}"),
                            target,
                            transition,
                        ))
                        .contrast_policy(ContrastPolicy::Off),
                );
                // A filled tab is emphasized the same way the active one is, so it earns the same
                // shaping - flat next to a capped peer reads as a rendering glitch. A text-painted
                // tab has no background to fill a cap glyph with, so it stays flat by nature.
                tab = tab.capped(paint == AlertPaint::Background);
            }
            tab
        })
        .collect();

    // The framework only caps the active/hovered tab (no powerline chaining between tabs) since
    // they are peers.
    let tab_caps = ctx
        .state
        .config
        .pane
        .workbar_tab_style
        .glyphs()
        .and_then(|(left, right)| Some((left.chars().next()?, right.chars().next()?)));

    Tabs::new()
        .tabs(tabs)
        .active(
            state
                .current()
                .active_workspace
                .min(shown.saturating_sub(1)),
        )
        .focusable(false)
        .width(Length::Flex(1))
        .height(Length::Px(1))
        .divider(' ')
        .caps(tab_caps)
        .style(Style::new().fg(theme.surface.menu).bg(theme.surface.panel))
        .active_style(
            Style::new()
                .fg(theme.surface.backdrop)
                .bg(theme.border_active)
                .bold(),
        )
        // A relative transform, not an absolute color: an alerting tab carries its own background,
        // and `bg(panel.elevate_by(..))` would resolve to a fixed panel-derived color that overwrites
        // it, so hovering a marked tab would drop the alert. `Elevate` stacks on whatever the tab
        // already resolved to, mid-fade included, so hover lifts the alert instead of replacing it.
        // Elevate rather than Lighten: it is luminance-aware (lifting a dark tab, dimming a light
        // one) and preserves hue, so a light theme still gains contrast on hover and a hovered red
        // tab reads as brighter red rather than drifting toward pink.
        //
        // `transform_bg` rather than `elevate_by`, which would transform the foreground too: the
        // label is already contrast-picked against the tab, so lifting it as well would erode that.
        .tab_hover_style(Style::new().transform_bg(crate::view::hover_lift()))
        .on_change(
            ctx.link()
                .callback(|event: TabsEvent| Msg::RunAction(Action::SwitchWorkspace(event.index))),
        )
        .into()
}

/// Label for a workspace tab: `<number>` normally, `<number>:<name>` when a custom name is set,
/// with a ` ·<count>` suffix while it holds panes.
/// Tab text is identity only - number, optional name, pane count. Alerts are carried entirely by the
/// tab's background color, so a workspace does not change width when an agent blocks and the tabs
/// beside it do not shift.
fn workspace_tab_label(name: Option<&str>, index: usize, pane_count: usize) -> String {
    let base = match name {
        Some(name) if !name.is_empty() => format!("{}:{name}", index + 1),
        _ => (index + 1).to_string(),
    };
    if pane_count > 0 {
        format!("{base} ·{pane_count}")
    } else {
        base
    }
}

/// Fixed sidebar-compatible workspace marker vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceMarker {
    Attention,
    Finished,
    Working,
    Idle,
}

impl WorkspaceMarker {
    /// Whether this marker breathes at the slower calm rate. Mirrors `PaneAlert::is_calm`: only
    /// something asking for an answer gets the urgent rate.
    const fn is_calm(self) -> bool {
        !matches!(self, Self::Attention)
    }
}

fn live_pane(pane: &Pane) -> bool {
    !pane.closing && !matches!(pane.terminal.status, ManagedTerminalStatus::Exited(_))
}

/// The marker for a workspace, driven only by `[workbar.alert]`. Bell/blocked is actionable and
/// therefore wins over finished, then ambient working/idle markers.
///
/// `mode` is checked here rather than at the call sites so `off` suppresses the glyph, its color,
/// and the pulse arming together - one switch, no surface left half-marked.
pub(crate) fn workspace_marker(
    workspace: &Workspace,
    alert: &WorkbarAlertConfig,
) -> Option<WorkspaceMarker> {
    if alert.mode == AlertMode::Off {
        return None;
    }
    let mut panes = workspace.panes.iter().filter(|pane| live_pane(pane));
    if (alert.bell && panes.clone().any(|pane| pane.activity.bell))
        || (alert.blocked && panes.clone().any(Pane::awaits_input))
    {
        return Some(WorkspaceMarker::Attention);
    }
    if alert.finished && panes.clone().any(|pane| pane.terminal.finished_unseen) {
        return Some(WorkspaceMarker::Finished);
    }
    if alert.working && panes.clone().any(|pane| pane.terminal.is_working()) {
        return Some(WorkspaceMarker::Working);
    }
    if alert.idle
        && panes.any(|pane| {
            pane.terminal
                .agent_status()
                .as_deref()
                .is_some_and(|status| {
                    status
                        .trim()
                        .eq_ignore_ascii_case(crate::session::protocol::pane_status::IDLE)
                })
        })
    {
        return Some(WorkspaceMarker::Idle);
    }
    None
}

/// Background tabs alone can keep the shared pulse alive; the active workspace's solid marker
/// remains visible but its tab never breathes.
pub(crate) fn has_inactive_marked_workspace(state: &crate::state::State) -> bool {
    state
        .current()
        .workspaces
        .iter()
        .enumerate()
        .any(|(index, workspace)| {
            index != state.current().active_workspace
                && workspace_marker(workspace, &state.config.workbar.alert).is_some()
        })
}

pub(crate) fn workspace_marker_color(marker: WorkspaceMarker) -> BadgeColor {
    match marker {
        WorkspaceMarker::Attention => BadgeColor::Error,
        WorkspaceMarker::Finished => BadgeColor::Success,
        WorkspaceMarker::Working => BadgeColor::Info,
        WorkspaceMarker::Idle => BadgeColor::Neutral,
    }
}

/// Number of workspace tabs to show: at least 5, growing to include the active
/// workspace and the highest one that currently holds panes.
fn workspace_tab_count(state: &crate::state::State) -> usize {
    let occupied = state
        .current()
        .workspaces
        .iter()
        .enumerate()
        .filter(|(_, ws)| ws.visible_count() > 0)
        .map(|(idx, _)| idx + 1)
        .max()
        .unwrap_or(0);
    occupied
        .max(state.current().active_workspace + 1)
        .max(5)
        .min(state.current().workspaces.len())
}

pub(crate) fn empty_workspace_panel(input: &InputConfig, theme: &Theme) -> Element {
    let prefix = input.prefix.to_string();
    let spawn_hint = if input.modifier_shortcuts {
        format!(
            "Press {}+Enter or {prefix} Enter to spawn a shell.",
            input.modifier.label(),
        )
    } else {
        format!("Press {prefix} Enter to spawn a shell.")
    };
    Frame::new()
        .header_left("Empty workspace")
        .header_padding(1)
        .border(true)
        .border_style(BorderStyle::Rounded)
        .style(
            Style::new()
                .fg(theme.surface.menu)
                .bg(theme.surface.backdrop),
        )
        .padding(1)
        .child(
            VStack::new()
                .gap(1)
                .child(Text::new("No panes here yet."))
                .child(Text::new(spawn_hint)),
        )
        .into()
}

/// Shown in the pane area when the client has no session at all: the startup picker was dismissed,
/// or the last session was killed. Distinct from the empty-workspace hint, which talks about
/// spawning a pane — here there is nothing to spawn a pane *into* yet, so it points at the ways
/// out: pick a session, start a shell, or leave the client.
///
/// A bare `Enter` starts the shell here (`key_routing::launcher_start_key`) because no pane is
/// competing for it; the ordinary `spawn` binding is listed beside it, since that is what works
/// everywhere else. Spawn and session-picker rows spell their default binding the prefix way:
/// `scheme_shortcuts` generates one table entry per key as both `<prefix> <key>` and
/// `<modifier>-<key>`. The leave row reads the first live shortcut for both stable action ids, so
/// overrides and unbinding remain truthful without repeating every modifier mirror.
///
/// The keys are rows rather than a sentence: `empty_workspace_rect` fixes this panel at 46x9, which
/// leaves room for the headline plus three shortcut rows, and a prose spelling of the shortcuts
/// does not survive the wrap. The key column is measured rather than fixed so a long custom prefix
/// still aligns.
pub(crate) fn launcher_panel(ctx: &Context<HyprmuxApp>, theme: &Theme) -> Element {
    let input = &ctx.state.config.input;
    let prefix = input.prefix.compact_display();
    let mut leave_keys: Vec<_> = ["quit", "detach"]
        .into_iter()
        .filter_map(|id| crate::commands::command_prefix_chord(ctx, id))
        .collect();
    leave_keys.dedup();
    let leave_keys = if leave_keys.is_empty() {
        "not set".to_string()
    } else {
        leave_keys.join(" / ")
    };
    let rows = [
        (format!("enter / {prefix} enter"), "ephemeral shell"),
        (format!("{prefix} s"), "pick a session"),
        (leave_keys, "leave"),
    ];
    let key_width = rows
        .iter()
        .map(|(keys, _)| keys.chars().count())
        .max()
        .unwrap_or(0)
        + 2;
    let keys = rows.iter().fold(VStack::new(), |stack, (keys, what)| {
        stack.child(Text::new(format!("{keys:<key_width$}{what}")))
    });
    Frame::new()
        .header_left("No session")
        .header_padding(1)
        .border(true)
        .border_style(BorderStyle::Rounded)
        .style(
            Style::new()
                .fg(theme.surface.menu)
                .bg(theme.surface.backdrop),
        )
        .padding(1)
        .child(
            VStack::new()
                .gap(1)
                .child(Text::new("Not attached to any session."))
                .child(keys),
        )
        .into()
}

/// Placeholder shown in the pane area while a session attach is in flight and no panes have arrived
/// yet. A live spinner makes the wait legible — a bare "empty workspace" reads as "done, nothing
/// here" when we are actually mid-connect. The spinner animates itself (the runtime advances
/// auto-frame spinners), so it keeps moving without the app scheduling ticks.
pub(crate) fn connecting_workspace_panel(
    host: Option<&str>,
    reconnecting: bool,
    theme: &Theme,
) -> Element {
    let verb = if reconnecting {
        "Reconnecting"
    } else {
        "Connecting"
    };
    let headline = match host {
        Some(host) => format!("{verb} to {host}…"),
        None => format!("{verb}…"),
    };
    let subtext = if host.is_some() {
        "Establishing the SSH connection — this can take a moment."
    } else {
        "Attaching to the session server…"
    };
    Frame::new()
        .header_left("Session")
        .header_padding(1)
        .border(true)
        .border_style(BorderStyle::Rounded)
        .style(
            Style::new()
                .fg(theme.surface.menu)
                .bg(theme.surface.backdrop),
        )
        .padding(1)
        .child(
            VStack::new()
                .gap(1)
                .child(
                    Spinner::new()
                        .spinner_style(SpinnerStyle::Dots)
                        .label(headline)
                        .style(Style::new().fg(theme.status.info))
                        .label_style(super::fg_only(&theme.primary)),
                )
                .child(Text::new(subtext).style(super::fg_only(&theme.muted))),
        )
        .into()
}

#[cfg(test)]
mod tests {
    use super::{
        WorkspaceMarker, collaboration_status, curated_color, has_inactive_marked_workspace,
        location_label, resolve_badge_color, workspace_marker, workspace_marker_color,
        workspace_placeholder_label, workspace_tab_label,
    };
    use crate::config::{BadgeColor, WorkbarAlertConfig, WorkbarSegment};
    use crate::state::{AlertMode, Pane, Workspace};
    use tui_lipan::prelude::{FloatRect, Theme};

    #[test]
    fn curated_color_assigns_distinct_roles() {
        assert_eq!(curated_color(&WorkbarSegment::Title), BadgeColor::Accent);
        assert_eq!(curated_color(&WorkbarSegment::Session), BadgeColor::Accent);
        assert_eq!(curated_color(&WorkbarSegment::Location), BadgeColor::Info);
        assert_eq!(curated_color(&WorkbarSegment::Clock), BadgeColor::Info);
        assert_eq!(
            curated_color(&WorkbarSegment::Activity),
            BadgeColor::Warning
        );
        assert_eq!(curated_color(&WorkbarSegment::Layout), BadgeColor::Neutral);
        assert_eq!(
            curated_color(&WorkbarSegment::Text("hi".to_string())),
            BadgeColor::Neutral
        );
    }

    #[test]
    fn location_label_identifies_remote_and_retained_connections() {
        let mut state =
            crate::state::State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        state.current_mut().remote_host = Some("workbox".to_string());
        state.current_mut().remote_target = Some(crate::session::remote::RemoteTarget::Alias(
            "workbox".to_string(),
        ));
        state.current_mut().connection = crate::state::ConnectionState::Connected;
        assert_eq!(location_label(&state).as_deref(), Some(" 󰒍 workbox "));

        state.runtime_epoch = 1;
        state.park_current(1, crate::state::Attachment::new());
        state.current_mut().connection = crate::state::ConnectionState::Connected;
        assert_eq!(location_label(&state).as_deref(), Some(" 󰒍 1 "));
    }

    #[test]
    fn resolve_badge_color_maps_roles_to_theme() {
        let theme = Theme::default();
        assert_eq!(
            resolve_badge_color(&theme, BadgeColor::Accent),
            (theme.border_active, theme.surface.backdrop)
        );
        assert_eq!(
            resolve_badge_color(&theme, BadgeColor::Info).0,
            theme.status.info
        );
        assert_eq!(
            resolve_badge_color(&theme, BadgeColor::Success).0,
            theme.status.success
        );
        assert_eq!(
            resolve_badge_color(&theme, BadgeColor::Warning).0,
            theme.status.warning
        );
        assert_eq!(
            resolve_badge_color(&theme, BadgeColor::Error).0,
            theme.status.error
        );
        assert_eq!(
            resolve_badge_color(&theme, BadgeColor::Neutral).0,
            theme.surface.menu
        );
        assert_eq!(
            resolve_badge_color(&theme, BadgeColor::Panel).0,
            theme.surface.panel
        );
    }

    #[test]
    fn workspace_tab_label_falls_back_to_number_without_a_name() {
        assert_eq!(workspace_tab_label(None, 0, 0), "1");
        assert_eq!(workspace_tab_label(Some(""), 0, 0), "1");
        assert_eq!(workspace_tab_label(None, 0, 3), "1 ·3");
    }

    #[test]
    fn workspace_tab_label_prefixes_the_custom_name_with_the_number() {
        assert_eq!(workspace_tab_label(Some("code"), 0, 0), "1:code");
        assert_eq!(workspace_tab_label(Some("code"), 0, 2), "1:code ·2");
    }

    fn marker_pane(id: u32) -> Pane {
        Pane::new(
            id,
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
    fn workspace_markers_are_precedence_ordered_and_independent_of_border_config() {
        let mut workspace = Workspace::new(0);
        let mut pane = marker_pane(1);
        pane.activity.bell = true;
        pane.terminal.finished_unseen = true;
        workspace.panes.push(pane);
        let alert = WorkbarAlertConfig::default();
        assert_eq!(
            workspace_marker(&workspace, &alert),
            Some(WorkspaceMarker::Attention)
        );

        workspace.panes[0].activity.bell = false;
        assert_eq!(
            workspace_marker(&workspace, &alert),
            Some(WorkspaceMarker::Finished)
        );
        workspace.panes[0].closing = true;
        assert_eq!(workspace_marker(&workspace, &alert), None);
        // `off` is the one switch that silences the whole surface: no glyph, so no color and
        // nothing for the pulse to arm on either.
        workspace.panes[0].closing = false;
        let off = WorkbarAlertConfig {
            mode: AlertMode::Off,
            ..alert
        };
        assert_eq!(workspace_marker(&workspace, &off), None);
        assert_eq!(
            workspace_marker(
                &workspace,
                &WorkbarAlertConfig {
                    mode: AlertMode::Static,
                    ..alert
                }
            ),
            Some(WorkspaceMarker::Finished),
            "static still marks; it only stops the tab from breathing"
        );
        workspace.panes[0].closing = true;
        assert_eq!(
            workspace_marker_color(WorkspaceMarker::Attention),
            BadgeColor::Error
        );
        assert_eq!(
            workspace_marker_color(WorkspaceMarker::Finished),
            BadgeColor::Success
        );

        let mut pane = marker_pane(2);
        pane.terminal.reported_status = Some(crate::session::protocol::PaneStatus {
            value: "idle".into(),
            reason: None,
            set_at: 0,
        });
        pane.terminal.detected_agent = Some(crate::session::protocol::DetectedAgent {
            kind: crate::session::protocol::AgentKind::OpenCode,
            state: crate::session::protocol::DetectedAgentState::Blocked,
        });
        let mut workspace = Workspace::new(1);
        workspace.panes.push(pane);
        assert_eq!(
            workspace_marker(&workspace, &WorkbarAlertConfig::default()),
            Some(WorkspaceMarker::Attention)
        );
    }

    #[test]
    fn inactive_marker_scan_excludes_the_active_workspace() {
        let mut state =
            crate::state::State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        state.current_mut().workspaces[0].panes[0]
            .terminal
            .finished_unseen = true;
        assert!(!has_inactive_marked_workspace(&state));
        state.current_mut().workspaces[1].panes.push({
            let mut pane = marker_pane(2);
            pane.terminal.finished_unseen = true;
            pane
        });
        assert!(has_inactive_marked_workspace(&state));
    }

    #[test]
    fn workspace_placeholder_label_prefers_the_custom_name() {
        assert_eq!(workspace_placeholder_label(None, 2), "3");
        assert_eq!(workspace_placeholder_label(Some(""), 2), "3");
        assert_eq!(workspace_placeholder_label(Some("code"), 2), "code");
    }

    #[test]
    fn collaboration_status_is_single_contextual_chip() {
        let theme = Theme::default();
        let mut state =
            crate::state::State::new(crate::config::HyprmuxConfig::default(), theme.clone());
        let mut shared = crate::state::SharedSessionState::new(1);
        shared.controller = Some(1);
        shared.clients = vec![crate::session::protocol::ClientInfo {
            id: 1,
            label: "one".into(),
            read_only: false,
            requesting_control: false,
            parked: false,
        }];
        state.current_mut().shared = Some(shared);
        assert!(collaboration_status(&state, &theme).is_none());

        let shared = state.current_mut().shared.as_mut().unwrap();
        shared.clients.push(crate::session::protocol::ClientInfo {
            id: 2,
            label: "two".into(),
            read_only: false,
            requesting_control: false,
            parked: false,
        });
        assert_eq!(collaboration_status(&state, &theme).unwrap().0, " CTRL ");
        state.current_mut().shared.as_mut().unwrap().input_locked = true;
        assert_eq!(
            collaboration_status(&state, &theme).unwrap().0,
            " CTRL LOCK "
        );
        let shared = state.current_mut().shared.as_mut().unwrap();
        shared.client_id = 2;
        shared.read_only = true;
        assert_eq!(
            collaboration_status(&state, &theme).unwrap().0,
            " READ ONLY "
        );
    }
}
