//! Rendering for the sidebar's row tabs through the framework Tree.
//!
//! The Tree owns hierarchy layout and interaction. Rozi supplies ordinary [`Row`] content with its
//! marker gutter, status, badge, and detail line, and places each row with a depth and a
//! [`Shape`]. The hierarchical tabs (Projects, JSON command tabs, and extension streams) derive
//! rows from a semantic projection, whose producers describe meaning through [`SidebarTone`].

use tui_lipan::prelude::*;

use super::row::{Priority, Row, Shape, SidebarRow};
use crate::AppRoot;
use crate::state::{SidebarTone, State, TreeItem, TreeLine};

pub(super) fn tree_rows(state: &State, items: Vec<TreeItem>) -> Vec<SidebarRow> {
    let mut rows = Vec::with_capacity(items.len());
    // A break between sections is the next row's gap, not a row of its own.
    let mut apart = false;
    for item in items {
        let target = item.target();
        let close = item.close();
        let row = match item {
            TreeItem::Spacer => {
                apart = true;
                continue;
            }
            TreeItem::Header(header) => {
                SidebarRow::header(super::row::header(state, header, false))
            }
            TreeItem::Loading(branch) => SidebarRow::item(
                Row::new("Loading…").title_style(super::super::fg_only(&state.theme.muted)),
                target,
            )
            .at(branch.depth()),
            TreeItem::Row(line) if line.heading => {
                let key = format!("{:?}", line.key);
                let content = line.row.content;
                let heading = match content.description {
                    Some(note) => {
                        super::row::header_with_note(state, content.label, note, Priority::Label)
                    }
                    None => super::row::header(state, content.label, false),
                };
                SidebarRow::header(heading).shape(Shape::Heading).keyed(key)
            }
            TreeItem::Row(line) => {
                let depth = line.branch.depth();
                let key = format!("{:?}", line.key);
                let expansion = line.expandable.then(|| (line.key.clone(), line.expanded));
                // A folded row's arrow takes the color of the most urgent row it hides.
                let tint = line.hidden_tone.and_then(|tone| tone_color(state, tone));
                let row = tree_row(state, *line);
                let mut row = SidebarRow::item(row, target).at(depth).keyed(key);
                if let Some((key, expanded)) = expansion {
                    row = row.collapsible(key, expanded);
                }
                if let Some(tint) = tint {
                    row = row.arrow_style(Style::new().fg(tint));
                }
                if let Some(close) = close {
                    row = row.closable(close);
                }
                row
            }
        };
        rows.push(if std::mem::take(&mut apart) {
            row.apart()
        } else {
            row
        });
    }
    rows
}

fn tree_row(state: &State, line: TreeLine) -> Row {
    let theme = &state.theme;
    let content = line.row.content;
    let muted = super::super::fg_only(&theme.muted);
    let disabled = content.disabled.is_some();
    let title_style = if disabled || content.tone == SidebarTone::Muted {
        muted
    } else {
        super::super::fg_only(&theme.primary)
    };
    let (glyph, color) = tone_glyph(
        state,
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
    if let Some(meta) = content.meta {
        row = row.meta(meta, color.map_or(muted, |color| Style::new().fg(color)));
    }
    // A keyed action waiting for its second press says so on the row it will act on. The ✕'s own
    // arming is drawn by the ✕.
    if let crate::state::RowTarget::StreamRow(reference) = &line.row.target
        && let Some(crate::state::SidebarClose::StreamAction { row: armed, action }) =
            &state.sidebar.pending_row_close
        && armed == reference
        && line.row.close.as_ref() != state.sidebar.pending_row_close.as_ref()
        && let Some(label) = state.stream_row(reference).and_then(|(producer, _)| {
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

/// The color a tone shows. `None` for tones that draw nothing.
fn tone_color(state: &State, tone: SidebarTone) -> Option<Color> {
    let status = &state.theme.status;
    match tone {
        SidebarTone::Normal | SidebarTone::Muted => None,
        SidebarTone::Working | SidebarTone::Info => Some(status.info),
        SidebarTone::Success => Some(status.success),
        SidebarTone::Warning | SidebarTone::Blocked => Some(status.warning),
        SidebarTone::Error => Some(status.error),
    }
}

/// The glyph and color a tone shows. `None` color for tones that draw nothing.
fn tone_glyph(state: &State, tone: SidebarTone) -> (Option<Element>, Option<Color>) {
    let Some(color) = tone_color(state, tone) else {
        return (None, None);
    };
    let glyph = match tone {
        SidebarTone::Working => Spinner::new()
            .style(Style::new().fg(color))
            .height(Length::Px(1))
            .into(),
        _ => Text::new(match tone {
            SidebarTone::Success => "✓",
            SidebarTone::Warning => "!",
            SidebarTone::Blocked => "▲",
            SidebarTone::Error => "×",
            _ => "•",
        })
        .style(Style::new().fg(color))
        .height(Length::Px(1))
        .into(),
    };
    (Some(glyph), Some(color))
}

/// A tab's placed rows as a Tree. Rozi supplies content and actions; the widget supplies tree
/// chrome, expansion hit targets, scrolling, and navigation.
pub(super) fn tab(
    ctx: &Context<AppRoot>,
    panel: usize,
    tab: &crate::config::SidebarTab,
) -> Element {
    let rows = super::rows(&ctx.state, tab);
    if rows.is_empty() {
        return super::empty_body(ctx, panel, Some(super::empty_text(ctx, tab)));
    }
    let focused = ctx.state.sidebar.focused && ctx.state.sidebar.active_panel_index() == panel;
    let cursor = super::resolve_cursor(ctx.state.sidebar.panels[panel].cursor, &rows);
    let nodes = rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            node(
                ctx,
                panel,
                cursor.filter(|_| focused) == Some(index),
                index,
                row,
            )
        })
        .collect::<Vec<_>>();
    let folding: Vec<usize> = nodes
        .iter()
        .filter(|(_, shape, _)| matches!(shape, Shape::Collapsible { .. }))
        .map(|(depth, _, _)| *depth)
        .collect();
    let mut tree = Tree::forest(nested_nodes(&nodes, &mut 0, 0, &folding))
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
    row: SidebarRow,
) -> (usize, Shape, TreeNode) {
    let depth = row.depth;
    let shape = row.shape;
    let arrow_style = row.arrow_style;
    let gap_before = row.gap_before;
    let key = row
        .key
        .clone()
        .unwrap_or_else(|| format!("structure-{index}"));
    let selectable = row.selectable();
    let close = super::close_affordance(ctx, panel, &row, index, selected);
    let hovered = ctx.state.sidebar.panels[panel].hovered_row == Some(index)
        && !ctx.state.sidebar.panels[panel].suppress_row_hover;
    let metadata = ListItem::new(row.label().to_string()).role(if selectable {
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
        super::row::RowKind::Header(element) => node.content(*element),
    };
    node = match shape {
        Shape::Leaf => node,
        Shape::Flush => node.show_arrow(false),
        Shape::Heading => node
            .expansion_mode(TreeExpansionMode::AlwaysOpen)
            .show_arrow(false),
        Shape::Collapsible { expanded } => node
            .expansion_mode(TreeExpansionMode::Collapsible)
            .expanded(expanded),
    };
    if let Some(style) = arrow_style {
        node = node.arrow_style(style);
    }
    (depth, shape, node.gap_before(gap_before).key(key))
}

/// The model projects only visible rows. Retain its order and depth; controlled expansion keeps
/// the widget synchronized with the identity-keyed expansion choices held by the application.
///
/// Leaves keep the arrow column only at a depth where some row folds, so every label at that depth
/// lines up across groups too, or just beneath one, so its children read a level deeper than its
/// label. Elsewhere the branch meets the label directly: `├ editor`, not `├─ editor`.
fn nested_nodes(
    nodes: &[(usize, Shape, TreeNode)],
    index: &mut usize,
    depth: usize,
    folding: &[usize],
) -> Vec<TreeNode> {
    let mut siblings = Vec::new();
    while let Some((node_depth, shape, node)) = nodes.get(*index) {
        if *node_depth < depth {
            break;
        }
        let mut node = node.clone();
        *index += 1;
        if nodes.get(*index).is_some_and(|(next, _, _)| *next > depth) {
            node = node.children(nested_nodes(nodes, index, depth + 1, folding));
        }
        let beneath_fold = depth > 0 && folding.contains(&(depth - 1));
        if *shape == Shape::Leaf && !folding.contains(&depth) && !beneath_fold {
            node = node.show_arrow(false);
        }
        siblings.push(node);
    }
    siblings
}
