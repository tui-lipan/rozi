use tui_lipan::prelude::*;

use super::row::{self, Row};
use crate::config::{SidebarLauncherEntry, SidebarTabId, UserCommandAction};
use crate::{HyprmuxApp, Msg};

/// The glyph and human name for what activating an entry does. The glyph sits in the same column as
/// the agent tab's status glyph so launcher rows line up with the built-in lists.
fn action_glyph(action: &UserCommandAction) -> (&'static str, &'static str) {
    match action {
        UserCommandAction::Run { .. } => ("▸", "run"),
        UserCommandAction::Send(_) => ("⏎", "send"),
        UserCommandAction::Popup { .. } => ("▫", "popup"),
    }
}

pub(super) fn launcher_tab(
    ctx: &Context<HyprmuxApp>,
    tab_id: &SidebarTabId,
    entries: &[SidebarLauncherEntry],
) -> Element {
    if entries.is_empty() {
        return row::empty(ctx, "No launcher entries");
    }
    let config_epoch = ctx.state.sidebar.config_epoch;
    entries
        .iter()
        .enumerate()
        .fold(VStack::new().gap(0), |body, (index, entry)| {
            let (glyph, kind) = action_glyph(&entry.action);
            let content = Row::new(entry.label.clone())
                .title_style(super::super::fg_only(&ctx.state.theme.primary))
                .glyph(
                    Text::new(glyph)
                        .style(super::super::fg_only(&ctx.state.theme.accent))
                        .height(Length::Px(1)),
                )
                // The second line says what the entry actually does — the launcher equivalent of the
                // agent tab's status line, and the only place a user can check a binding without
                // reopening their config.
                .detail(kind, super::super::fg_only(&ctx.state.theme.muted))
                .detail(
                    row::truncate(entry.action.target(), 24),
                    super::super::fg_only(&ctx.state.theme.muted).dim(),
                )
                .build(ctx);
            let id = tab_id.clone();
            body.child(
                MouseRegion::new()
                    .hover_effect(VisualEffect::transform_bg(ColorTransform::Lighten(0.08)))
                    .on_click(ctx.link().callback(move |_| Msg::SidebarLauncherActivate {
                        config_epoch,
                        tab_id: id.clone(),
                        entry_index: index,
                    }))
                    .child(content)
                    .key(format!("sidebar-launcher-{}-{index}", tab_id.as_str())),
            )
        })
        .into()
}

pub(super) fn command_tab(
    ctx: &Context<HyprmuxApp>,
    tab_id: &SidebarTabId,
    clickable: bool,
) -> Element {
    let Some(output) = ctx.state.sidebar.command_output.get(tab_id) else {
        return row::empty(ctx, "Loading…");
    };
    if output.rows.is_empty() {
        return row::empty(ctx, "No output");
    }
    let config_epoch = ctx.state.sidebar.config_epoch;
    let output_epoch = output.epoch;
    output
        .rows
        .iter()
        .enumerate()
        .fold(VStack::new().gap(0), |body, (index, row)| {
            let (glyph, style) = if row.error {
                ("!", Style::new().fg(ctx.state.theme.status.error))
            } else if clickable {
                ("·", super::super::fg_only(&ctx.state.theme.primary))
            } else {
                (" ", super::super::fg_only(&ctx.state.theme.primary))
            };
            let content = Row::new(row.display.clone())
                .title_style(style)
                .glyph(
                    Text::new(glyph)
                        .style(if row.error {
                            Style::new().fg(ctx.state.theme.status.error)
                        } else {
                            super::super::fg_only(&ctx.state.theme.muted)
                        })
                        .height(Length::Px(1)),
                )
                .build(ctx);
            if clickable && !row.error {
                let id = tab_id.clone();
                let line = row.raw.clone();
                body.child(
                    MouseRegion::new()
                        .hover_effect(VisualEffect::transform_bg(ColorTransform::Lighten(0.08)))
                        .on_click(
                            ctx.link()
                                .callback(move |_| Msg::SidebarCommandRowActivate {
                                    config_epoch,
                                    tab_id: id.clone(),
                                    output_epoch,
                                    line: line.clone(),
                                }),
                        )
                        .child(content)
                        .key(format!("sidebar-command-{}-{index}", tab_id.as_str())),
                )
            } else {
                body.child(content)
            }
        })
        .into()
}
