//! Rendering for the hierarchical row tabs: Projects, JSON command tabs, and extension streams.
//!
//! The generic framework Tree owns hierarchy layout and interaction. Rozi supplies ordinary
//! [`Row`] content with its marker gutter, status, badge, and detail line. Producers describe
//! meaning through [`SidebarTone`]; this module maps that meaning to themed row content.

use tui_lipan::prelude::*;

use super::row::{Priority, Row, SidebarRow};
use crate::AppRoot;
use crate::state::{SidebarTone, TreeItem, TreeLine};

pub(super) fn tree_rows(ctx: &Context<AppRoot>, items: Vec<TreeItem>) -> Vec<SidebarRow> {
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
                TreeItem::Loading(_) => SidebarRow::item(
                    Row::new("Loading…").title_style(super::super::fg_only(&ctx.state.theme.muted)),
                    target,
                ),
                TreeItem::Row(line) => {
                    let row = tree_row(ctx, *line);
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

fn tree_row(ctx: &Context<AppRoot>, line: TreeLine) -> Row {
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
        .title_style(title_style);
    if let Some(glyph) = glyph {
        row = row.glyph(glyph);
    }
    if let Some(description) = content.description {
        row = row
            .priority(Priority::Balanced)
            .badge_text(description, muted.dim());
    }
    // A keyed action waiting for its second press says so on the row it will act on. The ✕'s own
    // arming is drawn by the ✕.
    if let crate::state::RowTarget::StreamRow(reference) = &line.row.target
        && let Some(crate::state::SidebarClose::StreamAction { row: armed, action }) =
            &ctx.state.sidebar.pending_row_close
        && armed == reference
        && line.row.close.as_ref() != ctx.state.sidebar.pending_row_close.as_ref()
        && let Some(label) = ctx.state.stream_row(reference).and_then(|(producer, _)| {
            producer
                .actions
                .iter()
                .find(|candidate| &candidate.id == action)
                .map(|candidate| candidate.label.clone())
        })
    {
        row = row.armed(format!("Again to {label}"));
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
pub(super) fn tone_glyph(
    ctx: &Context<AppRoot>,
    tone: SidebarTone,
) -> (Option<Element>, Option<Color>) {
    let status = &ctx.state.theme.status;
    let (glyph, color) = match tone {
        SidebarTone::Normal | SidebarTone::Muted => return (None, None),
        SidebarTone::Working => {
            return (
                Some(
                    Spinner::new()
                        .style(Style::new().fg(status.info))
                        .height(Length::Px(1))
                        .into(),
                ),
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
        Some(
            Text::new(glyph)
                .style(Style::new().fg(color))
                .height(Length::Px(1))
                .into(),
        ),
        Some(color),
    )
}

/// Convert the semantic projection into Tree nodes. Rozi supplies content and actions; the widget
/// supplies tree chrome, expansion hit targets, scrolling, and navigation.
pub(super) fn tab(
    ctx: &Context<AppRoot>,
    panel: usize,
    tab: &crate::config::SidebarTab,
    items: Vec<TreeItem>,
) -> Element {
    let rows = super::body_rows(ctx, tab);
    if rows.is_empty() {
        return super::empty_body(ctx, panel, Some(super::empty_text(ctx, tab)));
    }
    let focused = ctx.state.sidebar.focused && ctx.state.sidebar.active_panel_index() == panel;
    let cursor = super::cursor_index(ctx, panel, tab);
    let nodes = items
        .into_iter()
        .zip(rows)
        .enumerate()
        .map(|(index, (item, row))| {
            node(
                ctx,
                panel,
                cursor.filter(|_| focused) == Some(index),
                index,
                item,
                row,
            )
        })
        .collect::<Vec<_>>();
    let mut tree = Tree::forest(nested_nodes(&nodes, &mut 0, 0))
        .controlled_expansion(true)
        .toggle_on_activate(false)
        .indent_style(IndentStyle::Branch)
        .indent_guide_style(Style::new().fg(super::fill(ctx).elevate_by(0.15)))
        .indent(1)
        .content_gap(1)
        .expanded_arrow(if ctx.state.config.nerd_icons {
            ""
        } else {
            "▼"
        })
        .collapsed_arrow(if ctx.state.config.nerd_icons {
            ""
        } else {
            "▶"
        })
        .arrow_style(super::super::fg_only(&ctx.state.theme.muted))
        .scrollbar(true)
        .scrollbar_config(super::scrollbar_config())
        .scroll_keys(ScrollKeymap::DEFAULT | ScrollKeymap::VIM_JUMP)
        .selection_style(super::cursor_highlight(&ctx.state.theme, super::fill(ctx)))
        .unfocused_selection_style(Style::default())
        .item_hover_style(if ctx.state.sidebar.panels[panel].suppress_row_hover {
            Style::default()
        } else {
            super::row_highlight(super::fill(ctx))
        })
        .tab_stop(false)
        .focus_key(super::body_key(ctx, panel))
        .on_select(super::callback(ctx, move |event: TreeEvent| {
            crate::Msg::SidebarRowSelected {
                panel,
                index: event.index,
            }
        }))
        .on_activate(super::callback(ctx, move |event: TreeEvent| {
            crate::Msg::SidebarRowActivate {
                panel,
                index: event.index,
            }
        }))
        .on_toggle(super::callback(ctx, move |event: TreeToggleEvent| {
            crate::Msg::SidebarRowExpanded {
                panel,
                index: event.index,
                expanded: event.expanded,
            }
        }))
        .on_hover(super::callback(ctx, move |event: TreeHoverEvent| {
            crate::Msg::SidebarRowHover {
                panel,
                index: event.index,
                hovered: event.hovered,
            }
        }))
        .on_pointer_move(super::callback(ctx, move |_| {
            crate::Msg::SidebarPointerMoved(panel)
        }));
    let tab_id = tab.id();
    tree = tree.on_viewport_change(super::callback(ctx, move |event| {
        crate::Msg::SidebarViewportChanged {
            panel,
            tab_id: tab_id.clone(),
            event,
        }
    }));
    if let Some(index) = cursor {
        tree = tree.selected(index);
    }
    tree.key(super::body_key(ctx, panel))
}

fn node(
    ctx: &Context<AppRoot>,
    panel: usize,
    selected: bool,
    index: usize,
    item: TreeItem,
    row: SidebarRow,
) -> (usize, TreeNode) {
    let depth = match &item {
        TreeItem::Row(line) => line.branch.depth(),
        TreeItem::Loading(branch) => branch.depth(),
        _ => 0,
    };
    let selectable = row.selectable();
    let close = super::close_affordance(ctx, panel, &row, index, selected);
    let hovered = ctx.state.sidebar.panels[panel].hovered_row == Some(index)
        && !ctx.state.sidebar.panels[panel].suppress_row_hover;
    let label = match &item {
        TreeItem::Row(line) => line.row.content.label.clone(),
        TreeItem::Header(header) => header.clone(),
        TreeItem::Loading(_) => "Loading…".to_string(),
        TreeItem::Spacer => String::new(),
    };
    let metadata = ListItem::new(label).role(if selectable {
        ListItemRole::Normal
    } else {
        ListItemRole::Header
    });
    let mut node = TreeNode::new(metadata);
    node = match row.kind {
        super::row::RowKind::Item(row) => {
            let parts = row.parts(ctx, selected, hovered, close);
            // Content styles are app policy; Tree owns the row geometry and interactions.
            node.content(parts.content)
                .leading(parts.leading)
                .content_height(parts.height)
                .row_style(parts.style)
        }
        super::row::RowKind::Header(element) => node
            .content(*element)
            .show_arrow(matches!(item, TreeItem::Loading(_))),
        super::row::RowKind::Spacer => node
            .content(Text::new(" ").height(Length::Px(1)))
            .show_arrow(false),
    };
    if let TreeItem::Row(line) = item {
        node = node
            .expansion_mode(if line.expandable {
                TreeExpansionMode::Collapsible
            } else {
                TreeExpansionMode::Auto
            })
            .expanded(line.expanded)
            .key(format!("{:?}", line.key));
    } else {
        node = node.key(format!("structure-{index}"));
    }
    (depth, node)
}

/// The model projects only visible rows. Retain its order and depth; controlled expansion keeps
/// the widget synchronized with the identity-keyed expansion choices held by the application.
fn nested_nodes(nodes: &[(usize, TreeNode)], index: &mut usize, depth: usize) -> Vec<TreeNode> {
    let mut siblings = Vec::new();
    while let Some((node_depth, node)) = nodes.get(*index) {
        if *node_depth < depth {
            break;
        }
        let mut node = node.clone();
        *index += 1;
        if nodes.get(*index).is_some_and(|(next, _)| *next > depth) {
            node = node.children(nested_nodes(nodes, index, depth + 1));
        }
        siblings.push(node);
    }
    siblings
}
