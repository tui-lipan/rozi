//! Rendering for the hierarchical row tabs: Projects, JSON command tabs, and extension streams.
//!
//! Every row is the sidebar's ordinary [`Row`], indented with the framework's tree guides, so a
//! tree reads like the flat tabs around it: the same marker gutter, glyph column, badge, and detail
//! line. Producers say what a row means through [`SidebarTone`]; this is the only place that turns
//! a tone into a glyph and a color.

use tui_lipan::prelude::*;

use super::row::{Row, RowTree, SidebarRow};
use crate::AppRoot;
use crate::state::{SidebarTone, TreeItem, TreeLine};

/// Cells per tree level. Two keeps a deep tree inside a 32-column sidebar.
const INDENT: u16 = 2;

fn guides(ctx: &Context<AppRoot>) -> TreeGuides {
    TreeGuides::new(IndentStyle::Short)
        .indent(INDENT)
        .guide_style(Style::new().fg(super::fill(ctx).elevate_by(0.3)))
}

pub(super) fn tree_rows(ctx: &Context<AppRoot>, items: Vec<TreeItem>) -> Vec<SidebarRow> {
    let guides = guides(ctx);
    let max_depth = items
        .iter()
        .filter_map(|item| item.line().map(|line| line.branch.depth()))
        .max()
        .unwrap_or(0);
    // A disclosure column only when something can expand, so a flat list stays flush left.
    let reserve = items
        .iter()
        .any(|item| item.line().is_some_and(|line| line.expandable));
    items
        .into_iter()
        .map(|item| {
            let target = item.target();
            let close = item.close();
            match item {
                TreeItem::Spacer => SidebarRow::spacer(),
                TreeItem::Header(header) => {
                    SidebarRow::header(super::row::header(ctx, header, false))
                }
                TreeItem::Loading(branch) => {
                    let row = Row::new("Loading…")
                        .title_style(super::super::fg_only(&ctx.state.theme.muted))
                        .glyph(blank())
                        .tree(RowTree {
                            first: guides.spans(&branch, max_depth),
                            rest: Vec::new(),
                            disclosure: None,
                            reserve_disclosure: reserve,
                        });
                    SidebarRow::header(row.build(ctx, false, false, None, None))
                }
                TreeItem::Row(line) => {
                    let row = tree_row(ctx, &guides, *line, max_depth, reserve);
                    let mut row = SidebarRow::item(row, target);
                    if let Some(close) = close {
                        row = row.closable(close);
                    }
                    row
                }
            }
        })
        .collect()
}

fn blank() -> Element {
    Text::new(" ").height(Length::Px(1)).into()
}

fn tree_row(
    ctx: &Context<AppRoot>,
    guides: &TreeGuides,
    line: TreeLine,
    max_depth: usize,
    reserve: bool,
) -> Row {
    let theme = &ctx.state.theme;
    let content = line.row.content;
    let muted = super::super::fg_only(&theme.muted);
    let disabled = content.disabled.is_some();
    let title_style = if disabled || content.tone == SidebarTone::Muted {
        muted
    } else {
        super::super::fg_only(&theme.primary)
    };
    let (glyph, color) = tone_glyph(
        ctx,
        if disabled {
            SidebarTone::Muted
        } else {
            content.tone
        },
    );
    let mut row = Row::new(content.label)
        .active(content.active)
        .glyph(glyph)
        .title_style(title_style)
        .tree(RowTree {
            first: guides.spans(&line.branch, max_depth),
            rest: guides.continuation_spans(&line.branch, max_depth),
            disclosure: line.expandable.then_some(line.expanded),
            reserve_disclosure: reserve,
        });
    if let Some(description) = content.description {
        row = row.badge_text(description, muted.dim());
    }
    match (content.disabled, content.detail) {
        (Some(reason), _) => row = row.detail(reason, muted.dim()),
        (None, Some(detail)) => {
            let style = match color {
                Some(color) if content.tone != SidebarTone::Normal => Style::new().fg(color),
                _ => muted.dim(),
            };
            row = row.detail(detail, style);
        }
        (None, None) => {}
    }
    row
}

/// The glyph and color a tone shows. `None` color for tones that draw nothing.
pub(super) fn tone_glyph(ctx: &Context<AppRoot>, tone: SidebarTone) -> (Element, Option<Color>) {
    let status = &ctx.state.theme.status;
    let (glyph, color) = match tone {
        SidebarTone::Normal | SidebarTone::Muted => return (blank(), None),
        SidebarTone::Working => {
            return (
                Spinner::new()
                    .style(Style::new().fg(status.info))
                    .height(Length::Px(1))
                    .into(),
                Some(status.info),
            );
        }
        SidebarTone::Info => ("•", status.info),
        SidebarTone::Success => ("✓", status.success),
        SidebarTone::Warning => ("!", status.warning),
        SidebarTone::Blocked => ("▲", status.warning),
        SidebarTone::Error => ("×", status.error),
    };
    (
        Text::new(glyph)
            .style(Style::new().fg(color))
            .height(Length::Px(1))
            .into(),
        Some(color),
    )
}
