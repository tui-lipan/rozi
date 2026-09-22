use tui_lipan::prelude::*;

use super::row::{self, Row, RowTarget, SidebarRow};
use crate::AppRoot;
use crate::config::{SidebarLauncherEntry, SidebarTabId, UserCommandAction};

/// The glyph and human name for what activating an entry does. The glyph sits in the same column as
/// the agent tab's status glyph so launcher rows line up with the built-in lists.
fn action_glyph(action: &UserCommandAction) -> (&'static str, &'static str) {
    match action {
        UserCommandAction::Run { .. } => ("▸", "run"),
        UserCommandAction::Send(_) => ("⏎", "send"),
        UserCommandAction::Popup { .. } => ("▫", "popup"),
        // Unreachable today: a launcher entry never parses into `Exec` (see
        // `SidebarLauncherEntrySpec::action`). Kept concrete so adding it there is a visible
        // decision rather than a glyph that silently reads "run".
        UserCommandAction::Exec { .. } | UserCommandAction::ExecDirect { .. } => ("▹", "exec"),
    }
}

/// Entries arrive already clustered by group (see `config::sidebar::cluster_by_group`), so a
/// section starts wherever the group changes. `crate::state::State::sidebar_item_projections`
/// walks the same shape; keep the two in step.
pub(super) fn launcher_rows(
    ctx: &Context<AppRoot>,
    tab_id: &SidebarTabId,
    entries: &[SidebarLauncherEntry],
) -> Vec<SidebarRow> {
    let config_epoch = ctx.state.sidebar.config_epoch;
    let mut rows = Vec::new();
    let mut current: Option<&String> = None;
    for (entry_index, entry) in entries.iter().enumerate() {
        if let Some(group) = entry.group.as_ref().filter(|group| Some(*group) != current) {
            if !rows.is_empty() {
                rows.push(SidebarRow::spacer());
            }
            rows.push(SidebarRow::header(row::header(ctx, group.clone(), false)));
            current = Some(group);
        }
        let (glyph, kind) = action_glyph(&entry.action);
        let target = entry.action.target();
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
                row::truncate(target.as_ref(), 24),
                super::super::fg_only(&ctx.state.theme.muted).dim(),
            );
        rows.push(SidebarRow::item(
            row,
            RowTarget::Launcher {
                config_epoch,
                tab_id: tab_id.clone(),
                entry_index,
            },
        ));
    }
    rows
}

pub(super) fn command_rows(
    ctx: &Context<AppRoot>,
    tab_id: &SidebarTabId,
    clickable: bool,
) -> Vec<SidebarRow> {
    let Some(output) = ctx.state.fresh_command_output(tab_id) else {
        return Vec::new();
    };
    let config_epoch = ctx.state.sidebar.config_epoch;
    let output_epoch = output.epoch;
    let mut rows = Vec::new();
    for row in &output.rows {
        // A `group_prefix` line labels the rows beneath it, so it renders as the same section
        // header the launcher and the built-in tabs use rather than as a row of output.
        if row.header {
            if !rows.is_empty() {
                rows.push(SidebarRow::spacer());
            }
            rows.push(SidebarRow::header(row::header(
                ctx,
                row.display.clone(),
                false,
            )));
            continue;
        }
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
        rows.push(if clickable && !row.error {
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
            SidebarRow::header(built.build(ctx, false, false, None))
        });
    }
    rows
}
