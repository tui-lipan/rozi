use tui_lipan::prelude::*;

use super::row::{self, Row, RowTarget, SidebarRow};
use crate::HyprmuxApp;
use crate::config::{SidebarLauncherEntry, SidebarTabId, UserCommandAction};

/// The glyph and human name for what activating an entry does. The glyph sits in the same column as
/// the agent tab's status glyph so launcher rows line up with the built-in lists.
fn action_glyph(action: &UserCommandAction) -> (&'static str, &'static str) {
    match action {
        UserCommandAction::Run { .. } => ("▸", "run"),
        UserCommandAction::Send(_) => ("⏎", "send"),
        UserCommandAction::Popup { .. } => ("▫", "popup"),
    }
}

pub(super) fn launcher_rows(
    ctx: &Context<HyprmuxApp>,
    tab_id: &SidebarTabId,
    entries: &[SidebarLauncherEntry],
) -> Vec<SidebarRow> {
    let config_epoch = ctx.state.sidebar.config_epoch;
    entries
        .iter()
        .enumerate()
        .map(|(entry_index, entry)| {
            let (glyph, kind) = action_glyph(&entry.action);
            let row = Row::new(entry.label.clone())
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
                );
            SidebarRow::item(
                row,
                RowTarget::Launcher {
                    config_epoch,
                    tab_id: tab_id.clone(),
                    entry_index,
                },
            )
        })
        .collect()
}

pub(super) fn command_rows(
    ctx: &Context<HyprmuxApp>,
    tab_id: &SidebarTabId,
    clickable: bool,
) -> Vec<SidebarRow> {
    let Some(output) = ctx.state.sidebar.command_output.get(tab_id) else {
        return Vec::new();
    };
    let config_epoch = ctx.state.sidebar.config_epoch;
    let output_epoch = output.epoch;
    output
        .rows
        .iter()
        .map(|row| {
            let style = if row.error {
                Style::new().fg(ctx.state.theme.status.error)
            } else {
                super::super::fg_only(&ctx.state.theme.primary)
            };
            let glyph_style = if row.error {
                Style::new().fg(ctx.state.theme.status.error)
            } else {
                super::super::fg_only(&ctx.state.theme.muted)
            };
            let mut built = Row::new(row.display.clone()).title_style(style);
            if row.error || clickable {
                let glyph = if row.error { "!" } else { "·" };
                built = built.glyph(Text::new(glyph).style(glyph_style).height(Length::Px(1)));
            } else {
                built = built.group_level();
            }
            // An error row carries no command to re-run, and a tab without `on_click` has nothing to
            // do with any row; both stay in the list as context but refuse selection and activation.
            if clickable && !row.error {
                SidebarRow::item(
                    built,
                    RowTarget::CommandRow {
                        config_epoch,
                        tab_id: tab_id.clone(),
                        output_epoch,
                        line: row.raw.clone(),
                    },
                )
            } else {
                SidebarRow::header(built.build(ctx, false, None))
            }
        })
        .collect()
}
