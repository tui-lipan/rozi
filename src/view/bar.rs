use tui_lipan::prelude::*;

use crate::config::{BarSegment, InputConfig};
use crate::input::Action;
use crate::state::Mode;
use crate::{HyprmuxApp, Msg};

pub(crate) fn workbar(ctx: &Context<HyprmuxApp>) -> HStack {
    let state = &ctx.state;
    let theme = &ctx.state.theme;
    let bar = &state.config.bar;

    let mut row = HStack::new()
        .gap(1)
        .height(Length::Px(1))
        .style(Style::new().bg(theme.surface.panel));

    for segment in &bar.left {
        if let Some(element) = bar_segment_element(ctx, segment) {
            row = row.child(element);
        }
    }

    // The workspace tabs already flex to fill slack; without them, insert a spacer so the
    // right region lands flush against the trailing edge.
    let has_workspaces = bar
        .left
        .iter()
        .chain(bar.right.iter())
        .any(|segment| matches!(segment, BarSegment::Workspaces));
    if !has_workspaces {
        row = row.child(Text::new("").width(Length::Flex(1)).height(Length::Px(1)));
    }

    for segment in &bar.right {
        if let Some(element) = bar_segment_element(ctx, segment) {
            row = row.child(element);
        }
    }

    // Mode chips sit at the trailing edge, so they take a left cap (the title chip at the leading
    // edge takes a right cap instead - see `bar_segment_element`).
    let left_cap = ctx
        .state
        .config
        .pane
        .workbar_badge_style
        .caps()
        .map(|c| c.0);
    let panel_bg = theme.surface.panel;
    let text_fg = theme.surface.backdrop;
    if ctx.command_chord_pending() {
        row = row.child(workbar_badge(
            " PREFIX ",
            text_fg,
            theme.status.warning,
            panel_bg,
            left_cap,
            BadgeCap::Left,
        ));
    }

    if state.mode == Mode::Resize {
        row = row.child(workbar_badge(
            " RESIZE ",
            text_fg,
            theme.status.success,
            panel_bg,
            left_cap,
            BadgeCap::Left,
        ));
    } else if state.mode == Mode::Copy {
        row = row.child(workbar_badge(
            " COPY ",
            text_fg,
            theme.status.info,
            panel_bg,
            left_cap,
            BadgeCap::Left,
        ));
    }

    row
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
/// rounded/pointed pill; without a cap the chip is the plain flush block. The cap shares the
/// badge's own sub-stack (gap 0) so the surrounding `gap(1)` row spacing stays outside the pill.
fn workbar_badge(
    label: &str,
    text_fg: Color,
    badge_bg: Color,
    panel_bg: Color,
    cap: Option<&'static str>,
    side: BadgeCap,
) -> Element {
    let body = Text::new(label.to_string())
        .style(Style::new().fg(text_fg).bg(badge_bg).bold())
        .height(Length::Px(1));
    let Some(glyph) = cap else {
        return body.into();
    };
    let cap_el = Text::new(glyph)
        .style(Style::new().fg(badge_bg).bg(panel_bg))
        .height(Length::Px(1));
    let row = HStack::new().height(Length::Px(1));
    match side {
        BadgeCap::Left => row.child(cap_el).child(body),
        BadgeCap::Right => row.child(body).child(cap_el),
    }
    .into()
}

fn bar_segment_element(ctx: &Context<HyprmuxApp>, segment: &BarSegment) -> Option<Element> {
    let theme = &ctx.state.theme;
    match segment {
        BarSegment::Title => Some(workbar_badge(
            " hyprmux ",
            theme.surface.backdrop,
            theme.border_active,
            theme.surface.panel,
            ctx.state
                .config
                .pane
                .workbar_badge_style
                .caps()
                .map(|c| c.1),
            BadgeCap::Right,
        )),
        BarSegment::Workspaces => Some(workspace_tabs_element(ctx)),
        BarSegment::Session => session_indicator(ctx),
        BarSegment::Clock => {
            let now = chrono::Local::now();
            Some(bar_text(
                format!(" {} ", now.format(&ctx.state.config.bar.clock_format)),
                theme,
            ))
        }
        BarSegment::Layout => Some(bar_text(
            format!(
                " {} ",
                ctx.state.workspaces[ctx.state.active_workspace]
                    .layout_kind
                    .label()
            ),
            theme,
        )),
        BarSegment::Activity => {
            let count = ctx
                .state
                .workspaces
                .iter()
                .flat_map(|ws| ws.panes.iter())
                .filter(|pane| !pane.closing && pane.activity.has_unseen_output)
                .count();
            (count > 0).then(|| bar_text(format!(" ●{count} "), theme))
        }
        BarSegment::Text(literal) => Some(bar_text(substitute_placeholders(ctx, literal), theme)),
        BarSegment::Command { command, .. } => {
            let output = ctx
                .state
                .bar_command_output
                .get(command)
                .map(String::as_str)
                .unwrap_or("");
            Some(bar_text(format!(" {output} "), theme))
        }
    }
}

fn bar_text(text: impl Into<String>, theme: &Theme) -> Element {
    Text::new(text.into())
        .style(Style::new().fg(theme.surface.menu))
        .height(Length::Px(1))
        .into()
}

fn substitute_placeholders(ctx: &Context<HyprmuxApp>, literal: &str) -> String {
    let state = &ctx.state;
    let active = &state.workspaces[state.active_workspace];
    literal
        .replace("{host}", &bar_hostname())
        .replace(
            "{workspace}",
            &workspace_placeholder_label(active.name.as_deref(), state.active_workspace),
        )
        .replace("{layout}", active.layout_kind.label())
        .replace("{session}", &attached_session_name(ctx).unwrap_or_default())
}

/// Value substituted for the `{workspace}` bar placeholder: the workspace's custom name when
/// set, otherwise its 1-based number.
fn workspace_placeholder_label(name: Option<&str>, index: usize) -> String {
    match name {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => (index + 1).to_string(),
    }
}

/// The `Session` bar segment: an accented badge naming the attached session server. Renders
/// nothing while unattached, so in local mode the segment simply takes no space.
fn session_indicator(ctx: &Context<HyprmuxApp>) -> Option<Element> {
    let theme = &ctx.state.theme;
    let name = attached_session_name(ctx)?;
    Some(
        Text::new(format!(" 󰛤 {name} "))
            .style(
                Style::new()
                    .fg(theme.surface.backdrop)
                    .bg(theme.border_active)
                    .bold(),
            )
            .height(Length::Px(1))
            .into(),
    )
}

/// The live attached session name, if any - backs the `Session` segment and `{session}` placeholder.
fn attached_session_name(ctx: &Context<HyprmuxApp>) -> Option<String> {
    ctx.state
        .session_attached
        .then(|| ctx.state.session_name.clone())
        .flatten()
}

fn bar_hostname() -> String {
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

    Tabs::new()
        .tabs(tabs)
        .active(state.active_workspace.min(shown.saturating_sub(1)))
        .focusable(false)
        .width(Length::Flex(1))
        .height(Length::Px(1))
        .divider(' ')
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
    use super::{workspace_placeholder_label, workspace_tab_label};

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
