mod agents;
mod panes;
mod row;
mod sessions;
mod tree;
mod user_tabs;

pub(crate) use row::RowTarget;

use tui_lipan::prelude::*;

use crate::config::SidebarTab;
use crate::{HyprmuxApp, Msg};

pub(super) fn sidebar(ctx: &Context<HyprmuxApp>) -> Element {
    let theme = &ctx.state.theme;
    let panels: Element = if ctx.state.sidebar.panels.len() > 1 {
        let divider_style = Style::new()
            .fg(theme.surface.element.elevate_by(0.15))
            .bg(theme.surface.element);
        Splitter::horizontal()
            .split_id("hyprmux-sidebar-panels")
            .weights(vec![
                ctx.state.config.sidebar.split_ratio,
                1.0 - ctx.state.config.sidebar.split_ratio,
            ])
            .weights_nonce(ctx.state.sidebar.panel_splitter_nonce(
                ctx.state.sidebar.panels.len(),
                ctx.state.config.sidebar.split_ratio,
            ))
            .min_size(3)
            .handle_symbol('─')
            .handle_style(divider_style)
            .handle_hover_style(divider_style)
            .handle_active_style(
                Style::new()
                    .fg(theme.border_active)
                    .bg(theme.surface.element)
                    .bold(),
            )
            .on_resize(ctx.link().callback(Msg::SidebarPanelsResized))
            .child(panel(ctx, 0))
            .child(panel(ctx, 1))
            .into()
    } else {
        panel(ctx, 0)
    };

    Frame::new()
        .border(false)
        .padding(0)
        // The whole sidebar sits outside automatic focus: Tab never enters it (the ring belongs to
        // the panes) and, just as importantly, clicking a row never focuses it either — so a click
        // stays a one-shot gesture and leaves no cursor behind. `request_focus` on the body key is
        // the deliberate way in, and `Exclude` explicitly still honours that.
        .focus_scope(FocusScope::Exclude)
        .style(
            ctx.state
                .theme
                .primary
                .patch(Style::new().bg(ctx.state.theme.surface.element)),
        )
        .width(Length::Flex(1))
        .height(Length::Flex(1))
        .child(panels)
        .into()
}

fn panel(ctx: &Context<HyprmuxApp>, panel: usize) -> Element {
    let tabs = panel_tabs(&ctx.state, panel);
    let active_id = ctx.state.sidebar.active_tab_in(panel);
    let active = active_id
        .and_then(|id| tabs.iter().position(|tab| tab.id() == *id))
        .unwrap_or(0);
    let bar_id = panel_bar_id(panel);
    let hover_style = Style::new().fg(ctx.state.theme.surface.menu).bg(ctx
        .state
        .theme
        .surface
        .element
        .elevate_by(0.08));
    let tab_bar = DraggableTabBar::new()
        .tabs(tabs.iter().map(|tab| DraggableTab::new(tab.label())))
        .active(active)
        .bar_id(bar_id)
        .drag_group("hyprmux-sidebar-tabs")
        .reorder_mode(DragReorderMode::Live)
        .border(false)
        .divider(' ')
        .show_close_buttons(false)
        .show_overflow_controls(true)
        .overflow_left_label(|_| std::sync::Arc::from("❮ "))
        .overflow_right_label(|_| std::sync::Arc::from(" ❯"))
        .height(Length::Px(1))
        // An empty bar is still a drop target for the shared drag group, so the hint lives on the
        // bar row itself rather than in the body below it. The leading space matches the pad a real
        // tab label carries, so the hint starts in the same column a tab would.
        .empty_text(" Drag tabs here")
        .empty_text_style(super::fg_only(&ctx.state.theme.muted))
        .style(
            Style::new()
                .fg(ctx.state.theme.surface.menu)
                .bg(ctx.state.theme.surface.element),
        )
        .active_style(
            Style::new()
                .fg(ctx.state.theme.surface.backdrop)
                .bg(ctx.state.theme.border_active)
                .bold(),
        )
        .tab_hover_style(hover_style)
        .overflow_style(Style::new().fg(ctx.state.theme.border_active))
        .overflow_hover_style(
            Style::new().fg(ctx.state.theme.border_active).bg(ctx
                .state
                .theme
                .surface
                .element
                .elevate_by(0.08)),
        )
        .on_change(
            ctx.link()
                .callback(move |event: TabsEvent| Msg::SidebarTabSelected {
                    panel,
                    index: event.index,
                }),
        )
        .on_reorder(
            ctx.link()
                .callback(move |event| Msg::SidebarTabReordered { panel, event }),
        )
        .on_transfer(ctx.link().callback(Msg::SidebarTabTransferred));

    let body = match tabs.get(active).copied() {
        // No tabs at all: the bar's own placeholder says it, so the body stays blank rather than
        // repeating the same fact one row lower.
        None => empty_body(ctx, panel, None),
        Some(SidebarTab::Tree { view, config }) => tree::tree_tab(ctx, panel, *view, config),
        Some(tab) => row_list(ctx, panel, tab),
    };

    VStack::new()
        .gap(0)
        .width(Length::Flex(1))
        .height(Length::Flex(1))
        .child(tab_bar)
        .child(
            VStack::new()
                .gap(0)
                .padding((1, 0, 0, 0))
                .width(Length::Flex(1))
                .height(Length::Flex(1))
                .child(body),
        )
        .into()
}

fn panel_tabs(state: &crate::state::State, panel: usize) -> Vec<&SidebarTab> {
    state
        .sidebar
        .panels
        .get(panel)
        .into_iter()
        .flat_map(|panel| &panel.tabs)
        .filter_map(|id| state.config.sidebar.tabs.iter().find(|tab| tab.id() == *id))
        .collect()
}

fn panel_bar_id(panel: usize) -> String {
    format!("hyprmux-sidebar-panel-{panel}")
}

pub(crate) fn panel_from_bar_id(id: &str) -> Option<usize> {
    id.strip_prefix("hyprmux-sidebar-panel-")?.parse().ok()
}

/// A body with nothing to list. Stays focusable so `focus-sidebar` still has a target here.
fn empty_body(ctx: &Context<HyprmuxApp>, panel: usize, text: Option<&str>) -> Element {
    let mut view = ScrollView::new()
        .focusable(true)
        .scroll_keys(ScrollKeymap::NONE);
    if let Some(text) = text {
        view = view.child(placeholder(ctx, text));
    }
    view.key(body_key(panel))
}

/// A workspace's custom name, if it has a usable one.
fn workspace_name(state: &crate::state::State, index: usize) -> Option<&str> {
    state
        .current()
        .workspaces
        .get(index)?
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

/// Compact workspace identity for a row badge: `2`, or `2:build` when named. Matches how the
/// workbar's workspace tabs spell the same thing, so a number means one thing everywhere.
pub(super) fn workspace_badge(state: &crate::state::State, index: usize) -> String {
    let number = index + 1;
    match workspace_name(state, index) {
        Some(name) => format!("{number}:{name}"),
        None => format!("{number}"),
    }
}

/// Workspace identity for a section header, where there is room to say it plainly: `Workspace 2`,
/// or `Workspace 2: mine` when named. The `Workspace N` part is always present — the number is what
/// keybindings address, so a named workspace must not hide it.
pub(super) fn workspace_heading(state: &crate::state::State, index: usize) -> String {
    let number = index + 1;
    match workspace_name(state, index) {
        Some(name) => format!("Workspace {number}: {name}"),
        None => format!("Workspace {number}"),
    }
}

/// The configured tab currently showing, resolved the same way the view resolves it.
pub(crate) fn active_tab(ctx: &Context<HyprmuxApp>) -> Option<&SidebarTab> {
    active_tab_in(ctx, ctx.state.sidebar.active_panel)
}

pub(crate) fn active_tab_in(ctx: &Context<HyprmuxApp>, panel: usize) -> Option<&SidebarTab> {
    active_tab_in_state(&ctx.state, panel)
}

pub(crate) fn active_tab_in_state(
    state: &crate::state::State,
    panel: usize,
) -> Option<&SidebarTab> {
    let id = state.sidebar.active_tab_in(panel)?;
    state.config.sidebar.tabs.iter().find(|tab| tab.id() == *id)
}

/// Every elapsed time the Agents tab is currently showing, joined — the duration tick's "is there
/// anything to advance, and did it change" input. `None` whenever nothing is showing one: the
/// sidebar is hidden, another tab is up, or every agent is idle.
pub(crate) fn agent_durations(state: &crate::state::State) -> Option<String> {
    if !state.sidebar_visible
        || !(0..state.sidebar.panels.len())
            .any(|panel| matches!(active_tab_in_state(state, panel), Some(SidebarTab::Agents)))
    {
        return None;
    }
    agents::duration_digest(state)
}

/// The element key `focus-sidebar` aims at. The file tree remounts under a root-derived key so
/// switching projects does not inherit another directory's expansion state, so the focus target has
/// to be derived from state rather than assumed constant.
pub(crate) fn body_focus_key(ctx: &Context<HyprmuxApp>) -> String {
    body_focus_key_for(ctx, ctx.state.sidebar.active_panel)
}

pub(crate) fn body_focus_key_for(ctx: &Context<HyprmuxApp>, panel: usize) -> String {
    match active_tab_in(ctx, panel) {
        Some(SidebarTab::Tree { view, config }) => tree::tree_root(ctx, config)
            .map(|root| {
                if config.explorer {
                    tree::tree_focus_key(panel, *view, &root)
                } else {
                    tree::tree_key(panel, *view, &root)
                }
            })
            .unwrap_or_else(|| body_key(panel)),
        _ => body_key(panel),
    }
}

pub(crate) fn body_key(panel: usize) -> String {
    format!("{}-{panel}", super::sidebar_body_key())
}

/// Every sidebar row, in display order, for whichever tab is active. Pure in `State`, which is what
/// lets `update` rebuild the same list to resolve an activated index — so Enter and a click reach
/// the same handler instead of two callbacks that can drift apart.
pub(crate) fn body_rows(ctx: &Context<HyprmuxApp>, tab: &SidebarTab) -> Vec<row::SidebarRow> {
    match tab {
        SidebarTab::Panes => panes::panes_rows(ctx),
        SidebarTab::Agents => agents::agents_rows(ctx),
        SidebarTab::Sessions => sessions::sessions_rows(ctx),
        SidebarTab::Launcher { name, entries, .. } => user_tabs::launcher_rows(ctx, name, entries),
        SidebarTab::Command { name, on_click, .. } => {
            user_tabs::command_rows(ctx, name, on_click.is_some())
        }
        // The tree owns its own rows inside the widget; nothing to enumerate here.
        SidebarTab::Tree { .. } => Vec::new(),
    }
}

/// The message a tab shows in place of rows. Distinct from "loading" for command tabs, where an
/// absent entry means the first poll has not landed yet.
fn empty_text(ctx: &Context<HyprmuxApp>, tab: &SidebarTab) -> &'static str {
    match tab {
        SidebarTab::Panes => "No panes",
        SidebarTab::Agents => "No agents detected",
        SidebarTab::Sessions => "No sessions discovered",
        SidebarTab::Launcher { .. } => "No launcher entries",
        SidebarTab::Command { name, .. } => {
            if ctx.state.sidebar.command_output.contains_key(name) {
                "No output"
            } else {
                "Loading…"
            }
        }
        SidebarTab::Tree { .. } => "",
    }
}

/// The row list for every tab except the file tree: composed rows in a scroll view.
///
/// Rows are direct `ScrollView` children so each can carry its own key — that is what lets
/// `scroll_to_key` follow the cursor. Nesting them inside one stack would make the whole body a
/// single child and scrolling would only ever resolve to the top of it.
fn row_list(ctx: &Context<HyprmuxApp>, panel: usize, tab: &SidebarTab) -> Element {
    let rows = body_rows(ctx, tab);
    if rows.is_empty() {
        return empty_body(ctx, panel, Some(empty_text(ctx, tab)));
    }
    let panel_state = &ctx.state.sidebar.panels[panel];
    let focused = ctx.state.sidebar.focused && ctx.state.sidebar.active_panel == panel;
    let cursor = cursor_index(ctx, panel, &rows);
    let tab_id = tab.id();

    let mut view = ScrollView::new()
        .scrollbar(true)
        .scrollbar_config(scrollbar_config())
        // Focusable so `focus-sidebar` has a target and the cursor can mean something, but its own
        // scroll keys are off: arrows move the cursor, and the view follows via `scroll_to_key`.
        .focusable(true)
        .scroll_keys(ScrollKeymap::NONE)
        .on_viewport_change(
            ctx.link()
                .callback(move |event| Msg::SidebarViewportChanged {
                    panel,
                    tab_id: tab_id.clone(),
                    event,
                }),
        );
    if let Some(cursor) = cursor.filter(|_| focused) {
        view = view.scroll_to_key(row_key(panel, cursor));
    }

    for (index, row) in rows.into_iter().enumerate() {
        let selectable = row.selectable();
        let close = close_affordance(ctx, panel, &row, index);
        let mut element = match row.kind {
            row::RowKind::Spacer => Text::new(" ").height(Length::Px(1)).into(),
            row::RowKind::Header(element) => *element,
            row::RowKind::Item(item) => item.build(ctx, focused && cursor == Some(index), close),
        };
        let close_hovered = ctx.has_hover_within_key(row::close_hover_key(panel, index));
        if close_hovered && !panel_state.suppress_row_hover {
            // Hover resolves to the innermost MouseRegion, so the row's native hover effect is
            // inactive while the keyed ✕ region owns hover. Apply the same background transform
            // through a scope around the row; the ✕ keeps its foreground-only native effect, and
            // nested effects compose without another hover state machine.
            element = EffectScope::new()
                .effect(VisualEffect::transform_bg(super::hover_lift()))
                .child(element)
                .into();
        }
        let element: Element = if selectable {
            let mut region = MouseRegion::new()
                .on_mouse_move(
                    ctx.link()
                        .callback(move |_| Msg::SidebarPointerMoved(panel)),
                )
                .on_click(
                    ctx.link()
                        .callback(move |_| Msg::SidebarRowActivate { panel, index }),
                )
                .on_hover_change(ctx.link().callback(move |hovered| Msg::SidebarRowHover {
                    panel,
                    index,
                    hovered,
                }))
                .child(element);
            if !panel_state.suppress_row_hover {
                // A transform rather than a style: it lifts whatever the row already painted, so
                // the active pane's row and the row under the keyboard cursor still respond to the
                // pointer. An absolute hover style sits *under* those backgrounds and never shows.
                region = region.hover_effect(VisualEffect::transform_bg(super::hover_lift()));
            }
            region.into()
        } else {
            element
        };
        view = view.child(element.key(row_key(panel, index)));
    }
    view.key(body_key(panel))
}

/// Whether a row shows its ✕ this frame, and in which state.
///
/// Hover is what reveals it, so a resting list stays quiet and no row advertises a destructive
/// action it is not being aimed at. An *armed* row keeps it regardless: hiding a live confirmation
/// the moment the pointer drifts would leave the next click on that ✕ killing something with no
/// warning on screen. `suppress_row_hover` gates the hover case the same way the row's hover lift
/// is gated, so keyboard navigation does not leave a ✕ behind under a stale pointer.
fn close_affordance(
    ctx: &Context<HyprmuxApp>,
    panel: usize,
    row: &row::SidebarRow,
    index: usize,
) -> Option<row::CloseAffordance> {
    let close = row.close.as_ref()?;
    let armed = ctx.state.sidebar.pending_row_close.as_ref() == Some(close);
    let panel_state = &ctx.state.sidebar.panels[panel];
    let hovered = panel_state.hovered_row == Some(index) && !panel_state.suppress_row_hover;
    (armed || hovered).then_some(row::CloseAffordance {
        panel,
        index,
        armed,
    })
}

/// Per-row element key, used both for reconciliation and as the `scroll_to_key` target.
fn row_key(panel: usize, index: usize) -> String {
    format!("sidebar-{panel}-row-{index}")
}

/// Where the cursor actually sits: the stored index if it still points at a selectable row,
/// otherwise the nearest one. Rows come and go underneath it as panes open, agents change state,
/// and command output refreshes, so the stored index is only ever a hint.
pub(crate) fn resolve_cursor(cursor: usize, rows: &[row::SidebarRow]) -> Option<usize> {
    if rows.get(cursor).is_some_and(row::SidebarRow::selectable) {
        return Some(cursor);
    }
    rows.iter()
        .position(row::SidebarRow::selectable)
        .map(|first| {
            rows.iter()
                .enumerate()
                .filter(|(_, row)| row.selectable())
                .map(|(index, _)| index)
                .min_by_key(|index| index.abs_diff(cursor))
                .unwrap_or(first)
        })
}

fn cursor_index(
    ctx: &Context<HyprmuxApp>,
    panel: usize,
    rows: &[row::SidebarRow],
) -> Option<usize> {
    resolve_cursor(ctx.state.sidebar.panels[panel].cursor, rows)
}

/// Scrollbar presentation shared by every scrolling surface in the sidebar, including the file
/// tree's own. A right half block sits against the panel's right edge, so the bar reads as a thin
/// rule beside the content instead of the default full-cell block.
///
/// The tab strip is deliberately excluded: it hides its scrollbar entirely (`h_scrollbar(false)`),
/// so there is no thumb to style.
pub(super) fn scrollbar_config() -> ScrollbarConfig {
    ScrollbarConfig::new().thumb('▐')
}

/// The lift a row gets under the keyboard cursor, matching what pointer hover looks like.
///
/// Deliberately quiet: a cursor and a hovered row mean the same thing — "this is the one you are
/// about to act on" — and giving the cursor its own louder treatment made every row it touched
/// shout. It also keeps each row's own colors readable, which a solid accent fill destroyed; agent
/// status, git state, and error red all carry meaning here.
///
/// Pointer hover is a *transform* of the same size rather than this style, so hovering a row that
/// is already active or already under the cursor still reads as a change.
pub(super) fn row_highlight(theme: &Theme) -> Style {
    Style::new().bg(theme.surface.element.elevate_by(super::HOVER_LIFT))
}

pub(super) fn placeholder(ctx: &Context<HyprmuxApp>, text: &str) -> Element {
    VStack::new()
        .padding((0, 0, 0, 1))
        .child(Text::new(text.to_string()).style(super::fg_only(&ctx.state.theme.muted)))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_labels_keep_the_number_visible_and_ignore_blank_names() {
        let mut state = crate::state::State::new(
            crate::config::HyprmuxConfig::default(),
            tui_lipan::prelude::Theme::default(),
        );
        assert_eq!(workspace_badge(&state, 1), "2");
        assert_eq!(workspace_heading(&state, 1), "Workspace 2");

        state.current_mut().workspaces[1].name = Some("mine".into());
        assert_eq!(workspace_badge(&state, 1), "2:mine");
        assert_eq!(workspace_heading(&state, 1), "Workspace 2: mine");

        // A name that is only whitespace is not a name; it must not leave dangling separators.
        state.current_mut().workspaces[1].name = Some("   ".into());
        assert_eq!(workspace_badge(&state, 1), "2");
        assert_eq!(workspace_heading(&state, 1), "Workspace 2");

        // Out of range stays addressable rather than panicking or losing the number.
        assert_eq!(workspace_badge(&state, 99), "100");
    }
}
