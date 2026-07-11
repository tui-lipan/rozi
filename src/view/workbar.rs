use tui_lipan::prelude::*;

use crate::config::{BadgeColor, InputConfig, WorkbarItem, WorkbarSegment};
use crate::input::Action;
use crate::state::{Mode, WORKBAR_HEIGHT};
use crate::{HyprmuxApp, Msg};

pub(crate) fn workbar(ctx: &Context<HyprmuxApp>) -> Element {
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
        if let Some(element) = left_segment_element(ctx, item) {
            row = row.child(element);
            let color = segment_edge_color(theme, item);
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
    }
    // Shared-session lease chip: only when more than one client is attached, so a solo session
    // stays uncluttered. Controller = success-colored CTRL, follower = info-colored VIEW.
    if state.session_attached && state.attached_client_count() > 1 {
        if state.is_controller() {
            trailing.push(TrailingChip::badge(" CTRL ", text_fg, theme.status.success));
        } else {
            trailing.push(TrailingChip::badge(" VIEW ", text_fg, theme.status.info));
        }
    }
    for item in &workbar_cfg.right {
        if let Some(chip) = trailing_chip(ctx, item) {
            trailing.push(chip);
        }
    }

    if let Some(last) = trailing.last() {
        right_cap_color = last.edge_color(panel_bg);
    }
    let badge_caps = ctx.state.config.pane.workbar_badge_style.caps();
    let powerline = ctx.state.config.pane.workbar_powerline;
    if let Some(cluster) = trailing_cluster(trailing, badge_caps, powerline, panel_bg) {
        row = row.child(cluster);
    }

    workbar_with_caps(
        ctx,
        row,
        left_cap_color.unwrap_or(panel_bg),
        right_cap_color,
    )
}

/// One element of the workbar's trailing (right-hand) cluster. `Badge` chips are colored pills
/// that chain into a powerline when badge caps are on; `Flex` wraps an opaque element (a plain
/// text segment or the workspace tabs) that just abuts its neighbors and breaks the color chain.
enum TrailingChip {
    Badge {
        label: String,
        text_fg: Color,
        bg: Color,
    },
    Flex(Box<Element>),
}

impl TrailingChip {
    fn badge(label: impl Into<String>, text_fg: Color, bg: Color) -> Self {
        Self::Badge {
            label: label.into(),
            text_fg,
            bg,
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
fn trailing_chip(ctx: &Context<HyprmuxApp>, item: &WorkbarItem) -> Option<TrailingChip> {
    let theme = &ctx.state.theme;
    match &item.segment {
        WorkbarSegment::Workspaces => {
            Some(TrailingChip::Flex(Box::new(workspace_tabs_element(ctx))))
        }
        _ => {
            let label = segment_label(ctx, &item.segment)?;
            let (bg, fg) = item_colors(theme, item);
            Some(TrailingChip::badge(label, fg, bg))
        }
    }
}

/// Assemble the trailing cluster. `powerline` controls chaining independently of the cap shape:
/// when off, chips stand apart with a 1-cell gap and each cap (if any) is drawn over the panel bar;
/// when on, the gap collapses to zero and each badge's cap is drawn over its left neighbor's color,
/// so the chips interlock into a powerline that flows out of the panel bar. `caps` still decides the
/// pill shape (rounded/pointed vs flush). Returns `None` when there is nothing trailing. Sizes to
/// `Auto` so the cluster only occupies its own width and stays pinned to the trailing edge.
fn trailing_cluster(
    chips: Vec<TrailingChip>,
    caps: Option<(&'static str, &'static str)>,
    powerline: bool,
    panel_bg: Color,
) -> Option<Element> {
    if chips.is_empty() {
        return None;
    }
    let left_cap = caps.map(|c| c.0);
    let mut cluster = HStack::new()
        .width(Length::Auto)
        .height(Length::Px(WORKBAR_HEIGHT))
        .gap(if powerline { 0 } else { 1 });
    // With powerline on, a badge's cap blends from its left neighbor's color; the first chip starts
    // from the panel bar. With it off, every cap sits over the panel bar and chips keep a gap.
    let mut prev_bg = panel_bg;
    for chip in chips {
        match chip {
            TrailingChip::Badge { label, text_fg, bg } => {
                cluster = cluster.child(workbar_badge(
                    &label,
                    text_fg,
                    bg,
                    prev_bg,
                    left_cap,
                    BadgeCap::Left,
                ));
                prev_bg = if powerline { bg } else { panel_bg };
            }
            TrailingChip::Flex(element) => {
                cluster = cluster.child(*element);
                prev_bg = panel_bg;
            }
        }
    }
    Some(cluster.into())
}

/// Background color a rendered workbar segment paints at the workbar's outer edge: its badge
/// background, except the workspace tab strip, which renders on the panel surface. Only called for
/// items that actually rendered, so it need not re-check visibility.
fn segment_edge_color(theme: &Theme, item: &WorkbarItem) -> Color {
    match item.segment {
        WorkbarSegment::Workspaces => theme.surface.panel,
        _ => item_colors(theme, item).0,
    }
}

/// The curated default badge color for a segment when the user did not override it. `title` and
/// `session` keep the active accent; the rest get distinct, theme-derived hues so they read as
/// separate chips instead of one dim run.
fn curated_color(segment: &WorkbarSegment) -> BadgeColor {
    match segment {
        WorkbarSegment::Title | WorkbarSegment::Session => BadgeColor::Accent,
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
fn item_colors(theme: &Theme, item: &WorkbarItem) -> (Color, Color) {
    let color = item.color.unwrap_or_else(|| curated_color(&item.segment));
    resolve_badge_color(theme, color)
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
    match color {
        BadgeColor::Accent => (theme.border_active, on_accent),
        BadgeColor::Info => (theme.status.info, on_accent),
        BadgeColor::Success => (theme.status.success, on_accent),
        BadgeColor::Warning => (theme.status.warning, on_accent),
        BadgeColor::Error => (theme.status.error, on_accent),
        BadgeColor::Neutral => (theme.surface.menu, text),
        BadgeColor::Panel => (theme.surface.panel, text),
    }
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
    let Some((left, right)) = ctx.state.config.pane.workbar_style.caps() else {
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

/// Which end a workbar badge's cap sits on. The title chip caps on the right (it starts flush at
/// the leading edge); mode chips cap on the left (they end flush at the trailing edge).
#[derive(Clone, Copy)]
enum BadgeCap {
    Left,
    Right,
}

/// A colored workbar chip (`label` in `text_fg` on `badge_bg`, bold) with an optional powerline
/// end cap. The cap is drawn in the badge color over the panel background so the chip reads as a
/// rounded/pointed pill; without a cap the chip is the plain flush block. Every element sizes to
/// content (`Length::Auto`) so a chip only ever occupies its own width - stacks default to
/// `Flex(1)`, which would otherwise let a capped chip swallow the whole workbar and break
/// placement.
fn workbar_badge(
    label: &str,
    text_fg: Color,
    badge_bg: Color,
    panel_bg: Color,
    cap: Option<&'static str>,
    side: BadgeCap,
) -> Element {
    let body_style = Style::new().fg(text_fg).bg(badge_bg).bold();
    let Some(glyph) = cap else {
        // Padded: a plain flush block that keeps the label's blank side padding.
        return Text::new(label.to_string())
            .style(body_style)
            .width(Length::Auto)
            .height(Length::Px(1))
            .into();
    };
    // Capped: the cap stands in for the padding on its side, so drop that padding space.
    let label = match side {
        BadgeCap::Left => label.strip_prefix(' ').unwrap_or(label),
        BadgeCap::Right => label.strip_suffix(' ').unwrap_or(label),
    };
    let body = Text::new(label.to_string())
        .style(body_style)
        .width(Length::Auto)
        .height(Length::Px(1));
    let cap_el = Text::new(glyph)
        .style(
            Style::new()
                .fg(badge_bg)
                .bg(panel_bg)
                .contrast_policy(ContrastPolicy::Off),
        )
        .width(Length::Px(1))
        .height(Length::Px(1));
    let row = HStack::new().width(Length::Auto).height(Length::Px(1));
    match side {
        BadgeCap::Left => row.child(cap_el).child(body),
        BadgeCap::Right => row.child(body).child(cap_el),
    }
    .into()
}

/// The badge label text (with its blank side padding) for a workbar segment, or `None` when the
/// segment renders nothing (`Workspaces` is the tab strip, not a badge; `Session`/`Activity` hide
/// when there is nothing to show).
fn segment_label(ctx: &Context<HyprmuxApp>, segment: &WorkbarSegment) -> Option<String> {
    match segment {
        WorkbarSegment::Title => Some(" hyprmux ".to_string()),
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
            let now = chrono::Local::now();
            Some(format!(
                " {} ",
                now.format(&ctx.state.config.workbar.clock_format)
            ))
        }
        WorkbarSegment::Layout => Some(format!(
            " {} ",
            ctx.state.workspaces[ctx.state.active_workspace]
                .layout_kind
                .label()
        )),
        WorkbarSegment::Activity => {
            let count = ctx
                .state
                .workspaces
                .iter()
                .flat_map(|ws| ws.panes.iter())
                .filter(|pane| !pane.closing && pane.activity.has_unseen_output)
                .count();
            (count > 0).then(|| format!(" ●{count} "))
        }
        WorkbarSegment::Text(literal) => Some(format!(
            " {} ",
            substitute_placeholders(ctx, literal).trim()
        )),
        WorkbarSegment::Command { command, .. } => {
            let output = ctx
                .state
                .workbar_command_output
                .get(command)
                .map(String::as_str)
                .unwrap_or("");
            Some(format!(" {output} "))
        }
        WorkbarSegment::Workspaces => None,
    }
}

/// The element for a left-region workbar item: the workspace tab strip, or a colored badge. Left
/// badges cap on their right (leading pills, like the title chip) and stay gap-separated by the row
/// - powerline chaining is a trailing-cluster feature.
fn left_segment_element(ctx: &Context<HyprmuxApp>, item: &WorkbarItem) -> Option<Element> {
    if matches!(item.segment, WorkbarSegment::Workspaces) {
        return Some(workspace_tabs_element(ctx));
    }
    let label = segment_label(ctx, &item.segment)?;
    let (bg, fg) = item_colors(&ctx.state.theme, item);
    Some(workbar_badge(
        &label,
        fg,
        bg,
        ctx.state.theme.surface.panel,
        ctx.state
            .config
            .pane
            .workbar_badge_style
            .caps()
            .map(|c| c.1),
        BadgeCap::Right,
    ))
}

fn substitute_placeholders(ctx: &Context<HyprmuxApp>, literal: &str) -> String {
    let state = &ctx.state;
    let active = &state.workspaces[state.active_workspace];
    literal
        .replace("{host}", &workbar_hostname())
        .replace(
            "{workspace}",
            &workspace_placeholder_label(active.name.as_deref(), state.active_workspace),
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
    if !ctx.state.session_attached || ctx.state.is_ephemeral_session() {
        return None;
    }
    ctx.state.session_name.clone()
}

fn workbar_hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|host| !host.trim().is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|host| host.trim().to_string())
                .filter(|host| !host.is_empty())
        })
        .unwrap_or_else(|| "localhost".to_string())
}

fn workspace_tabs_element(ctx: &Context<HyprmuxApp>) -> Element {
    let state = &ctx.state;
    let theme = &ctx.state.theme;
    let shown = workspace_tab_count(state);

    let tabs: Vec<Tab> = (0..shown)
        .map(|idx| {
            let count = state.workspaces[idx].visible_count();
            let label = workspace_tab_label(state.workspaces[idx].name.as_deref(), idx, count);
            Tab::new(label)
        })
        .collect();

    // The framework only caps the active/hovered tab (no powerline chaining between tabs) since
    // they are peers.
    let tab_caps = ctx
        .state
        .config
        .pane
        .workbar_tab_style
        .caps()
        .and_then(|(left, right)| Some((left.chars().next()?, right.chars().next()?)));

    Tabs::new()
        .tabs(tabs)
        .active(state.active_workspace.min(shown.saturating_sub(1)))
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
        .tab_hover_style(
            Style::new()
                .fg(theme.surface.menu)
                .bg(theme.surface.panel.elevate(0.08)),
        )
        .on_change(
            ctx.link()
                .callback(|event: TabsEvent| Msg::RunAction(Action::SwitchWorkspace(event.index))),
        )
        .into()
}

/// Label for a workspace tab: `<number>` normally, `<number>:<name>` when a custom name is set,
/// with a ` ·<count>` suffix while it holds panes.
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

/// Number of workspace tabs to show: at least 5, growing to include the active
/// workspace and the highest one that currently holds panes.
fn workspace_tab_count(state: &crate::state::State) -> usize {
    let occupied = state
        .workspaces
        .iter()
        .enumerate()
        .filter(|(_, ws)| ws.visible_count() > 0)
        .map(|(idx, _)| idx + 1)
        .max()
        .unwrap_or(0);
    occupied
        .max(state.active_workspace + 1)
        .max(5)
        .min(state.workspaces.len())
}

pub(crate) fn empty_workspace_panel(input: &InputConfig, theme: &Theme) -> Element {
    let prefix = input.prefix.to_string();
    Frame::new()
        .title(" Empty workspace ")
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
                .child(Text::new(format!(
                    "Press {}+Enter or {prefix} Enter to spawn a shell.",
                    input.modifier.label(),
                ))),
        )
        .into()
}

#[cfg(test)]
mod tests {
    use super::{
        curated_color, resolve_badge_color, workspace_placeholder_label, workspace_tab_label,
    };
    use crate::config::{BadgeColor, WorkbarSegment};
    use tui_lipan::prelude::Theme;

    #[test]
    fn curated_color_assigns_distinct_roles() {
        assert_eq!(curated_color(&WorkbarSegment::Title), BadgeColor::Accent);
        assert_eq!(curated_color(&WorkbarSegment::Session), BadgeColor::Accent);
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

    #[test]
    fn workspace_placeholder_label_prefers_the_custom_name() {
        assert_eq!(workspace_placeholder_label(None, 2), "3");
        assert_eq!(workspace_placeholder_label(Some(""), 2), "3");
        assert_eq!(workspace_placeholder_label(Some("code"), 2), "code");
    }
}
