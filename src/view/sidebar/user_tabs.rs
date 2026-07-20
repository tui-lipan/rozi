use tui_lipan::prelude::*;

use crate::config::{SidebarLauncherEntry, SidebarTabId};
use crate::{HyprmuxApp, Msg};

pub(super) fn launcher_tab(
    ctx: &Context<HyprmuxApp>,
    tab_id: &SidebarTabId,
    entries: &[SidebarLauncherEntry],
) -> Element {
    if entries.is_empty() {
        return empty(ctx, "No launcher entries");
    }
    let config_epoch = ctx.state.sidebar.config_epoch;
    entries
        .iter()
        .enumerate()
        .fold(
            VStack::new().gap(1),
            |body, (index, entry)| {
                let id = tab_id.clone();
                body.child(
                    MouseRegion::new()
                        .hover_style(Style::new().bg(ctx.state.theme.surface.element.elevate(0.08)))
                        .on_click(ctx.link().callback(move |_| Msg::SidebarLauncherActivate {
                            config_epoch,
                            tab_id: id.clone(),
                            entry_index: index,
                        }))
                        .child(
                            Text::new(entry.label.clone())
                                .style(super::super::fg_only(&ctx.state.theme.primary)),
                        )
                        .key(format!("sidebar-launcher-{}-{index}", tab_id.as_str())),
                )
            },
        )
        .into()
}

pub(super) fn command_tab(
    ctx: &Context<HyprmuxApp>,
    tab_id: &SidebarTabId,
    clickable: bool,
) -> Element {
    let Some(output) = ctx.state.sidebar.command_output.get(tab_id) else {
        return empty(ctx, "Loading…");
    };
    if output.rows.is_empty() {
        return empty(ctx, "No output");
    }
    let config_epoch = ctx.state.sidebar.config_epoch;
    let output_epoch = output.epoch;
    output
        .rows
        .iter()
        .enumerate()
        .fold(
            VStack::new().gap(1),
            |body, (index, row)| {
                let style = if row.error {
                    Style::new().fg(ctx.state.theme.status.error)
                } else {
                    super::super::fg_only(&ctx.state.theme.primary)
                };
                let text = Text::new(row.display.clone()).style(style);
                if clickable && !row.error {
                    let id = tab_id.clone();
                    let line = row.raw.clone();
                    body.child(
                        MouseRegion::new()
                            .hover_style(
                                Style::new().bg(ctx.state.theme.surface.element.elevate(0.08)),
                            )
                            .on_click(ctx.link().callback(move |_| {
                                Msg::SidebarCommandRowActivate {
                                    config_epoch,
                                    tab_id: id.clone(),
                                    output_epoch,
                                    line: line.clone(),
                                }
                            }))
                            .child(text)
                            .key(format!("sidebar-command-{}-{index}", tab_id.as_str())),
                    )
                } else {
                    body.child(text)
                }
            },
        )
        .into()
}

fn empty(ctx: &Context<HyprmuxApp>, text: &str) -> Element {
    VStack::new()
        .padding((0, 0, 0, 1))
        .child(Text::new(text.to_string()).style(super::super::fg_only(&ctx.state.theme.muted)))
        .into()
}
