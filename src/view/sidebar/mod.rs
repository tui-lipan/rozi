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

pub(super) fn sidebar(ctx: &Context<HyprmuxApp>, width: u16) -> Element {
    let tabs = &ctx.state.config.sidebar.tabs;
    let active = ctx
        .state
        .sidebar
        .active_tab
        .as_ref()
        .and_then(|id| tabs.iter().position(|tab| tab.id() == *id))
        .unwrap_or(0);
    let theme = &ctx.state.theme;
    let tab_caps = ctx
        .state
        .config
        .pane
        .workbar_tab_style
        .glyphs()
        .and_then(|(left, right)| Some((left.chars().next()?, right.chars().next()?)));
    let tab_bar = tabs.iter().fold(Tabs::new(), |bar, tab| {
        bar.tab(Tab::new(tab.label().to_string()))
    });
    let tab_ids: Vec<_> = tabs.iter().map(SidebarTab::id).collect();
    let active_tab_start = tabs
        .iter()
        .take(active)
        .map(|tab| {
            RichText::from(tab.label().to_string())
                .width()
                .saturating_add(3)
        })
        .sum::<usize>();
    let active_tab_end = active_tab_start.saturating_add(
        tabs.get(active)
            .map(|tab| {
                RichText::from(tab.label().to_string())
                    .width()
                    .saturating_add(2)
            })
            .unwrap_or(0),
    );
    // Selection presentation matches the workbar's workspace tabs so the two strips read as one
    // system; the sidebar sits on the element surface rather than the panel, so the resting and
    // hover backgrounds follow that surface instead.
    let tab_bar = tab_bar
        .active(active)
        .focusable(false)
        .width(Length::Auto)
        .height(Length::Px(1))
        .divider(' ')
        .caps(tab_caps)
        .style(
            Style::new()
                .fg(theme.surface.menu)
                .bg(theme.surface.element),
        )
        .active_style(
            Style::new()
                .fg(theme.surface.backdrop)
                .bg(theme.border_active)
                .bold(),
        )
        .tab_hover_style(
            Style::new()
                .fg(theme.surface.menu)
                .bg(theme.surface.element.elevate(0.08)),
        )
        .on_change(ctx.link().callback(move |event: TabsEvent| {
            Msg::SidebarTabSelected(tab_ids[event.index].clone())
        }));
    let tab_scroll = ScrollView::new()
        .axis(ScrollAxis::Horizontal)
        .h_scrollbar(false)
        .height(Length::Px(1))
        .reveal_horizontal_range(active_tab_start, active_tab_end)
        .child(tab_bar);

    let active_tab = tabs.get(active);
    // The file tree is its own focusable widget; every other tab is a `List` built from the shared
    // row grammar. Both are keyboard-navigable and both distinguish a focused cursor from a resting
    // one, so the sidebar behaves the same way whichever tab is up.
    let body: Element = match active_tab {
        None => placeholder(ctx, "No sidebar tabs configured"),
        Some(SidebarTab::Tree { view, config }) => tree::tree_tab(ctx, *view, config),
        Some(tab) => row_list(ctx, tab),
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
        .width(Length::Px(width))
        .height(Length::Flex(1))
        .child(
            HStack::new()
                .child(
                    VStack::new()
                        .gap(0)
                        .width(Length::Flex(1))
                        .child(tab_scroll)
                        .child(
                            // Top inset sits outside the scroll so it stays under the tab bar
                            // instead of scrolling away with the first row.
                            VStack::new()
                                .gap(0)
                                .padding((1, 0, 0, 0))
                                .width(Length::Flex(1))
                                .height(Length::Flex(1))
                                .child(body),
                        ),
                )
                .child(
                    Divider::vertical()
                        .style(Style::new().fg(ctx.state.theme.surface.element.elevate(0.15))),
                ),
        )
        .into()
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
    active_tab_of(&ctx.state)
}

/// [`active_tab`] against bare state, for the update side, which decides whether a tab is on screen
/// without holding a view context.
pub(crate) fn active_tab_of(state: &crate::state::State) -> Option<&SidebarTab> {
    let tabs = &state.config.sidebar.tabs;
    let index = state
        .sidebar
        .active_tab
        .as_ref()
        .and_then(|id| tabs.iter().position(|tab| tab.id() == *id))
        .unwrap_or(0);
    tabs.get(index)
}

/// Every elapsed time the Agents tab is currently showing, joined — the duration tick's "is there
/// anything to advance, and did it change" input. `None` whenever nothing is showing one: the
/// sidebar is hidden, another tab is up, or every agent is idle.
pub(crate) fn agent_durations(state: &crate::state::State) -> Option<String> {
    if !state.sidebar_visible || !matches!(active_tab_of(state), Some(SidebarTab::Agents)) {
        return None;
    }
    agents::duration_digest(state)
}

/// The element key `focus-sidebar` aims at. The file tree remounts under a root-derived key so
/// switching projects does not inherit another directory's expansion state, so the focus target has
/// to be derived from state rather than assumed constant.
pub(crate) fn body_focus_key(ctx: &Context<HyprmuxApp>) -> String {
    match active_tab(ctx) {
        Some(SidebarTab::Tree { view, config }) => tree::tree_root(ctx, config)
            .map(|root| tree::tree_key(*view, &root))
            .unwrap_or_else(|| super::sidebar_body_key().to_string()),
        _ => super::sidebar_body_key().to_string(),
    }
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
fn row_list(ctx: &Context<HyprmuxApp>, tab: &SidebarTab) -> Element {
    let rows = body_rows(ctx, tab);
    if rows.is_empty() {
        return row::empty(ctx, empty_text(ctx, tab));
    }
    let focused = ctx.state.sidebar.focused;
    let cursor = cursor_index(ctx, &rows);

    let mut view = ScrollView::new()
        .scrollbar(true)
        .scrollbar_config(scrollbar_config())
        // Focusable so `focus-sidebar` has a target and the cursor can mean something, but its own
        // scroll keys are off: arrows move the cursor, and the view follows via `scroll_to_key`.
        .focusable(true)
        .scroll_keys(ScrollKeymap::NONE);
    if let Some(cursor) = cursor.filter(|_| focused) {
        view = view.scroll_to_key(row_key(cursor));
    }

    for (index, row) in rows.into_iter().enumerate() {
        let selectable = row.selectable();
        let close = close_affordance(ctx, &row, index);
        let mut element = match row.kind {
            row::RowKind::Spacer => Text::new(" ").height(Length::Px(1)).into(),
            row::RowKind::Header(element) => *element,
            row::RowKind::Item(item) => item.build(ctx, focused && cursor == Some(index), close),
        };
        let close_hovered = ctx.has_hover_within_key(row::close_hover_key(index));
        if close_hovered && !ctx.state.sidebar.suppress_row_hover {
            // Hover resolves to the innermost MouseRegion, so the row's native hover effect is
            // inactive while the keyed ✕ region owns hover. Apply the same background transform
            // through a scope around the row; the ✕ keeps its foreground-only native effect, and
            // nested effects compose without another hover state machine.
            element = EffectScope::new()
                .effect(VisualEffect::transform_bg(hover_lift(&ctx.state.theme)))
                .child(element)
                .into();
        }
        let element: Element = if selectable {
            let mut region = MouseRegion::new()
                .on_mouse_move(ctx.link().callback(|_| Msg::SidebarPointerMoved))
                .on_click(ctx.link().callback(move |_| Msg::SidebarRowActivate(index)))
                .on_hover_change(
                    ctx.link()
                        .callback(move |hovered| Msg::SidebarRowHover { index, hovered }),
                )
                .child(element);
            if !ctx.state.sidebar.suppress_row_hover {
                // A transform rather than a style: it lifts whatever the row already painted, so
                // the active pane's row and the row under the keyboard cursor still respond to the
                // pointer. An absolute hover style sits *under* those backgrounds and never shows.
                region =
                    region.hover_effect(VisualEffect::transform_bg(hover_lift(&ctx.state.theme)));
            }
            region.into()
        } else {
            element
        };
        view = view.child(element.key(row_key(index)));
    }
    view.key(super::sidebar_body_key())
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
    row: &row::SidebarRow,
    index: usize,
) -> Option<row::CloseAffordance> {
    let close = row.close.as_ref()?;
    let armed = ctx.state.sidebar.pending_row_close.as_ref() == Some(close);
    let hovered =
        ctx.state.sidebar.hovered_row == Some(index) && !ctx.state.sidebar.suppress_row_hover;
    (armed || hovered).then_some(row::CloseAffordance { index, armed })
}

/// Per-row element key, used both for reconciliation and as the `scroll_to_key` target.
fn row_key(index: usize) -> String {
    format!("sidebar-row-{index}")
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

fn cursor_index(ctx: &Context<HyprmuxApp>, rows: &[row::SidebarRow]) -> Option<usize> {
    resolve_cursor(ctx.state.sidebar.cursor, rows)
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
    Style::new().bg(theme.surface.element.elevate(HOVER_LIFT))
}

/// How far a row lifts under the pointer or the keyboard cursor. Shared so the two stay the same
/// weight even though one is an absolute background and the other a transform of what is there.
const HOVER_LIFT: f32 = 0.08;

/// The pointer-hover lift, as a transform.
///
/// `Color::elevate` is the theme-aware move — it lightens a dark surface and dims a light one — but
/// `ColorTransform` has no elevate variant, only the two directions. So pick the direction the same
/// way `elevate` does. Deciding it once from the surface the sidebar sits on is enough: every row
/// background is derived from that surface, so they are all on the same side of the light/dark line.
fn hover_lift(theme: &Theme) -> ColorTransform {
    if theme.surface.element.is_dark() {
        ColorTransform::Lighten(HOVER_LIFT)
    } else {
        ColorTransform::Dim(HOVER_LIFT)
    }
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
