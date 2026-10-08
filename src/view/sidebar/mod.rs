pub(crate) mod agents;
mod panes;
mod row;
mod sessions;
mod tree;
mod user_tabs;
mod worktrees;

use tui_lipan::prelude::*;

use crate::config::SidebarTab;
use crate::{AppRoot, Msg};

/// `width` is the panel's settled width, which is not always the width it is being drawn into: on
/// the way in and out it is clipped to however much of that the slide has handed over. It is laid
/// out at its full width regardless, so its tabs and rows keep their places and the clip cuts
/// through them - a `Flex` width would resolve against the clip and re-wrap the panel on every
/// frame of the slide instead.
pub(super) fn sidebar(
    ctx: &Context<AppRoot>,
    width: u16,
    side: crate::config::SidebarPosition,
) -> Element {
    let theme = &ctx.state.theme;
    let fill = fill_color(theme, ctx.state.config.sidebar.background_follows_canvas);
    let indices: Vec<_> = ctx
        .state
        .sidebar
        .panels
        .iter()
        .enumerate()
        .filter(|(_, p)| p.dock == side)
        .map(|(i, _)| i)
        .collect();
    let dock = ctx.state.config.sidebar.layout.dock(side);
    let weights = dock.displayed_weights();
    let split_id = format!("rozi-sidebar-{}-panels", side.id());
    let nonce = ctx
        .state
        .sidebar
        .splitter_nonce(&split_id, weights.iter().map(|w| w.to_bits()).collect());
    let panels: Element = if indices.len() > 1 {
        let divider_style = Style::new().fg(fill.elevate_by(0.15)).bg(fill);
        let mut splitter = Splitter::horizontal()
            .split_id(split_id)
            .weights(weights)
            .weights_nonce(nonce)
            .min_size(3)
            .handle_symbol('─')
            .handle_style(divider_style)
            .handle_hover_style(divider_style)
            .handle_active_style(Style::new().fg(theme.border_active).bg(fill).bold())
            .on_resize(crate::view::sidebar::callback(ctx, move |event| {
                Msg::SidebarDockPanelsResized { side, event }
            }));
        for index in indices {
            splitter = splitter.child(panel(ctx, index));
        }
        splitter.into()
    } else {
        panel(ctx, indices[0])
    };
    Frame::new()
        .border(false)
        .padding(0)
        .style(theme.primary.patch(Style::new().bg(fill)))
        .width(Length::Px(width))
        .height(Length::Flex(1))
        .child(panels)
        .key(super::sidebar_region_key())
        .pointer_focus(false)
}

pub(super) fn docking_shell(
    ctx: &Context<AppRoot>,
    viewport: Rect,
    content: Element,
    dialog_dim: super::animation::Fade,
    ui_flash: Option<(Color, super::animation::Fade)>,
) -> Element {
    use crate::config::SidebarPosition::{Left, Right};
    let widths = [
        ctx.state.dock_reserved_width(viewport, Left),
        ctx.state.dock_reserved_width(viewport, Right),
    ];
    if widths == [0, 0] {
        return content;
    }
    let deployed = ctx.state.dock_deployed_widths(viewport);
    let fade = super::animation::layer_fade(dialog_dim, ctx.state.theme.surface.backdrop, ui_flash);
    let divider_target = fill(ctx).blend_toward(fade.color, 1.0 - fade.opacity);
    let divider_bg = ctx.animated_color_with_frame_rate(
        "rozi-sidebar-divider",
        divider_target,
        fade.transition,
        ctx.state.runtime_frame_rate(),
    );
    let divider = Style::new()
        .fg(divider_bg)
        .bg(divider_bg)
        .transform_fg(ColorTransform::elevate(0.15));
    let mut children = Vec::new();
    let mut weights = Vec::new();
    let mut limits = Vec::new();
    let mut content = Some(content);
    for (index, side) in [Left, Right].into_iter().enumerate() {
        if side == Right {
            children.push(content.take().unwrap());
            weights.push(viewport.w.saturating_sub(widths[0] + widths[1]) as f32);
            limits.push(SplitterPaneLimits::min_size(
                crate::layout::geometry::MIN_CANVAS_COLS.min(viewport.w / 2),
            ));
        }
        if widths[index] == 0 {
            continue;
        }
        let panel_width = deployed[index].saturating_sub(1);
        let pane_width = widths[index].saturating_sub(1);
        let clip = Canvas::new()
            .child_at(
                FloatRect {
                    x: crate::layout::anim::sidebar_slide_offset(
                        pane_width,
                        panel_width,
                        side == Right,
                    ),
                    y: 0.0,
                    w: f32::from(panel_width),
                    h: f32::from(viewport.h),
                }
                .to_rect(),
                sidebar(ctx, panel_width, side),
            )
            .key(format!("rozi-sidebar-{}-clip", side.id()));
        children.push(
            fade.apply(Animated::new(clip))
                .height(Length::Flex(1))
                .into(),
        );
        weights.push(pane_width as f32);
        // The opposite dock shares the width budget; no drag may consume the central minimum.
        let opposite = widths[1 - index];
        let progress = if side == Left {
            ctx.state.sidebar_slide.get()
        } else {
            ctx.state.sidebar.right_slide.get()
        }
        .clamp(0.0, 1.0);
        let central_min = if viewport.w > 20 {
            20
        } else {
            viewport.w - viewport.w / 2
        };
        let max_deployed =
            crate::config::SIDEBAR_MAX_WIDTH.min(viewport.w.saturating_sub(opposite + central_min));
        let min = ((f32::from(crate::config::SIDEBAR_MIN_WIDTH.min(deployed[index])) * progress)
            .round() as u16)
            .saturating_sub(1);
        let max = ((f32::from(max_deployed) * progress).round() as u16).saturating_sub(1);
        limits.push(SplitterPaneLimits::range(min, max.max(pane_width)));
    }
    let nonce = ctx.state.sidebar.splitter_nonce(
        "rozi-sidebar-shell",
        vec![
            u32::from(viewport.w),
            u32::from(widths[0]),
            u32::from(widths[1]),
        ],
    );
    let mut splitter = Splitter::vertical()
        .split_id("rozi-sidebar-shell")
        .weights(weights)
        .weights_nonce(nonce)
        .pane_limits(limits)
        .min_size(0)
        .handle_symbol('│')
        .handle_style(divider)
        .handle_hover_style(divider)
        .handle_active_style(
            Style::new()
                .fg(ctx.state.theme.border_active)
                .bg(divider_bg)
                .bold(),
        )
        .on_resize_live(crate::view::sidebar::callback(
            ctx,
            Msg::SidebarWidthResizing,
        ))
        .on_resize(crate::view::sidebar::callback(
            ctx,
            Msg::SidebarWidthResized,
        ));
    for child in children {
        splitter = splitter.child(child);
    }
    splitter.into()
}

fn panel(ctx: &Context<AppRoot>, panel: usize) -> Element {
    let tabs = panel_tabs(&ctx.state, panel);
    let active_id = ctx.state.sidebar.active_tab_in(panel);
    let active = active_id
        .and_then(|id| tabs.iter().position(|tab| tab.id() == *id))
        .unwrap_or(0);
    let bar_id = panel_bar_id(&ctx.state, panel);
    let strip = strip_fill(ctx);
    let theme = &ctx.state.theme;
    let hover_bg = strip.elevate_by(0.08);
    let hover_style = Style::new()
        .fg(crate::ops::theme::chrome_label_fg(theme, hover_bg))
        .bg(hover_bg);
    let tab_caps = ctx
        .state
        .config
        .effective_cap_style(ctx.state.config.sidebar.tab_style)
        .chars();
    let tab_bar = DraggableTabBar::new()
        .tabs(tabs.iter().map(|tab| DraggableTab::new(tab.label())))
        .active(active)
        .bar_id(bar_id)
        .drag_group("rozi-sidebar-tabs")
        .reorder_mode(DragReorderMode::Live)
        .border(false)
        .divider(' ')
        .caps(tab_caps)
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
                .fg(crate::ops::theme::chrome_label_fg(theme, strip))
                .bg(strip),
        )
        .active_style({
            let active_bg = theme.border_active;
            Style::new()
                .fg(crate::ops::theme::chrome_label_fg(theme, active_bg))
                .bg(active_bg)
                .bold()
        })
        .tab_hover_style(hover_style)
        .overflow_style(Style::new().fg(ctx.state.theme.border_active))
        .overflow_hover_style(
            Style::new()
                .fg(ctx.state.theme.border_active)
                .bg(strip.elevate_by(0.08)),
        )
        .on_change(crate::view::sidebar::callback(
            ctx,
            move |event: TabsEvent| Msg::SidebarTabSelected {
                panel,
                index: event.index,
            },
        ))
        .on_reorder(crate::view::sidebar::callback(ctx, move |event| {
            Msg::SidebarTabReordered { panel, event }
        }))
        .on_transfer(crate::view::sidebar::callback(
            ctx,
            Msg::SidebarTabTransferred,
        ));

    let body = match tabs.get(active).copied() {
        // No tabs at all: the bar's own placeholder says it, so the body stays blank rather than
        // repeating the same fact one row lower.
        None => empty_body(ctx, panel, None),
        Some(SidebarTab::Tree { view, config }) => tree::tree_tab(ctx, panel, *view, config),
        Some(tab) => row_list(ctx, panel, tab),
    };

    VStack::new()
        .gap(u16::from(ctx.state.config.sidebar.gap))
        .width(Length::Flex(1))
        .height(Length::Flex(1))
        .child(tab_bar)
        .child(body)
        .key(panel_identity(&ctx.state, panel))
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

fn panel_identity(state: &crate::state::State, panel: usize) -> String {
    let panel = &state.sidebar.panels[panel];
    format!("{}-{}", panel.dock.id(), panel.home)
}

fn panel_bar_id(state: &crate::state::State, panel: usize) -> String {
    format!("rozi-sidebar-panel-{}", panel_identity(state, panel))
}

pub(crate) fn panel_from_bar_id(state: &crate::state::State, id: &str) -> Option<usize> {
    (0..state.sidebar.panels.len()).find(|panel| panel_bar_id(state, *panel) == id)
}

/// A body with nothing to list. Stays focusable so `focus-sidebar` still has a target here.
fn empty_body(ctx: &Context<AppRoot>, panel: usize, text: Option<&str>) -> Element {
    let mut view = ScrollView::new()
        .focusable(true)
        .tab_stop(false)
        .scroll_keys(ScrollKeymap::NONE);
    if let Some(text) = text {
        view = view.child(placeholder(ctx, text));
    }
    view.key(body_key(ctx, panel))
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
pub(crate) fn active_tab(ctx: &Context<AppRoot>) -> Option<&SidebarTab> {
    active_tab_in(ctx, ctx.state.sidebar.active_panel)
}

pub(crate) fn active_tab_in(ctx: &Context<AppRoot>, panel: usize) -> Option<&SidebarTab> {
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
    if !state.sidebar_shown()
        || !(0..state.sidebar.panels.len())
            .filter(|panel| state.sidebar_panel_visible(*panel))
            .any(|panel| {
                matches!(
                    active_tab_in_state(state, panel),
                    Some(SidebarTab::Activity)
                )
            })
    {
        return None;
    }
    agents::duration_digest(state)
}

/// The element key `focus-sidebar` aims at. The file tree remounts under a root-derived key so
/// switching projects does not inherit another directory's expansion state, so the focus target has
/// to be derived from state rather than assumed constant.
pub(crate) fn body_focus_key(ctx: &Context<AppRoot>) -> String {
    body_focus_key_for(ctx, ctx.state.sidebar.active_panel)
}

pub(crate) fn body_focus_key_for(ctx: &Context<AppRoot>, panel: usize) -> String {
    match active_tab_in(ctx, panel) {
        Some(SidebarTab::Tree { view, config }) => tree::tree_root(ctx, config)
            .map(|root| {
                if config.explorer {
                    tree::tree_focus_key(ctx, panel, *view, &root)
                } else {
                    tree::tree_key(ctx, panel, *view, &root)
                }
            })
            .unwrap_or_else(|| body_key(ctx, panel)),
        _ => body_key(ctx, panel),
    }
}

pub(crate) fn body_key(ctx: &Context<AppRoot>, panel: usize) -> String {
    format!(
        "{}-{}",
        super::sidebar_body_key(),
        panel_identity(&ctx.state, panel)
    )
}

/// Every sidebar row, in display order, with interaction metadata supplied by the model projection
/// that update handlers consume too.
pub(crate) fn body_rows(ctx: &Context<AppRoot>, tab: &SidebarTab) -> Vec<row::SidebarRow> {
    let mut rows = visual_body_rows(ctx, tab);
    let projections = ctx.state.sidebar_item_projections(tab);
    debug_assert_eq!(
        rows.len(),
        projections.len(),
        "sidebar visual rows and semantic projections diverged for {:?}",
        tab.id()
    );
    if rows.len() != projections.len() {
        // Fail closed in release builds: a stale visual/model mapping must never activate or close
        // a different row merely because it happens to occupy the same index.
        for row in &mut rows {
            row.target = crate::state::RowTarget::Inert;
            row.close = None;
        }
        return rows;
    }
    for (row, projection) in rows.iter_mut().zip(projections) {
        row.target = projection.target;
        row.close = projection.close;
    }
    rows
}

fn visual_body_rows(ctx: &Context<AppRoot>, tab: &SidebarTab) -> Vec<row::SidebarRow> {
    match tab {
        SidebarTab::Panes => panes::panes_rows(ctx),
        SidebarTab::Activity => agents::agents_rows(ctx),
        SidebarTab::Sessions => sessions::sessions_rows(ctx),
        SidebarTab::Worktrees => worktrees::worktrees_rows(ctx),
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
fn empty_text<'a>(ctx: &'a Context<AppRoot>, tab: &SidebarTab) -> &'a str {
    match tab {
        SidebarTab::Panes => "No panes",
        SidebarTab::Activity => "No activity",
        SidebarTab::Sessions => "No sessions discovered",
        // No repository to list: same one-cell inset `placeholder` uses on every other empty tab.
        SidebarTab::Worktrees => ctx
            .state
            .sidebar
            .worktrees
            .unavailable
            .as_deref()
            .unwrap_or("Not in a Git repository"),
        SidebarTab::Launcher { .. } => "No launcher entries",
        SidebarTab::Command { name, .. } => {
            if ctx.state.fresh_command_output(name).is_some() {
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
fn row_list(ctx: &Context<AppRoot>, panel: usize, tab: &SidebarTab) -> Element {
    let rows = body_rows(ctx, tab);
    if rows.is_empty() {
        return empty_body(ctx, panel, Some(empty_text(ctx, tab)));
    }
    let panel_state = &ctx.state.sidebar.panels[panel];
    let focused = ctx.state.sidebar.focused && ctx.state.sidebar.active_panel == panel;
    let cursor = cursor_index(ctx, panel, tab);
    let tab_id = tab.id();

    let mut view = ScrollView::new()
        .scrollbar(true)
        .scrollbar_config(scrollbar_config())
        // Focusable so `focus-sidebar` has a target and the cursor can mean something, but its own
        // scroll keys are off: arrows move the cursor, and the view follows via `scroll_to_key`.
        .focusable(true)
        .tab_stop(false)
        .scroll_keys(ScrollKeymap::NONE)
        .on_viewport_change(crate::view::sidebar::callback(ctx, move |event| {
            Msg::SidebarViewportChanged {
                panel,
                tab_id: tab_id.clone(),
                event,
            }
        }));
    if let Some(cursor) = cursor.filter(|_| focused) {
        view = view.scroll_to_key(row_key(ctx, panel, cursor));
    }

    for (index, row) in rows.into_iter().enumerate() {
        let selectable = row.selectable();
        let closable = row.close.is_some();
        let selected = focused && cursor == Some(index);
        let close = close_affordance(ctx, panel, &row, index, selected);
        let hovered = panel_state.hovered_row == Some(index) && !panel_state.suppress_row_hover;
        let element = match row.kind {
            row::RowKind::Spacer => Text::new(" ").height(Length::Px(1)).into(),
            row::RowKind::Header(element) => *element,
            row::RowKind::Item(item) => item.build(ctx, selected, hovered, close),
        };
        // The row used to lose its own hover effect whenever the keyed ✕ region owned hover, so
        // this re-applied the same background transform through a scope. tui-lipan 0.7.0 makes a
        // `MouseRegion`'s hover visuals cover interactive descendants, so the row's native effect
        // stays on and re-applying it lifted the background twice.
        // Hover is tracked for *every* row, not only the ones a click does something to. Pointing
        // at a row is a fact about the pointer; whether the row can be activated is not. Tying the
        // two together left `hovered_row` pointing at a row whose region had gone: a host row stops
        // being selectable the moment connecting starts, so the pointer leaving it fired no
        // `on_hover_change`, and the index stayed hovered forever — long enough for the failed
        // host to come back wearing "Connect" instead of its status, and for a connected host
        // (inert, but closable) to be unable to show its ✕ at all.
        let interactive = selectable || closable;
        let mut region = MouseRegion::new()
            .on_mouse_move(crate::view::sidebar::callback(ctx, move |_| {
                Msg::SidebarPointerMoved(panel)
            }))
            .on_hover_change(crate::view::sidebar::callback(ctx, move |hovered| {
                Msg::SidebarRowHover {
                    panel,
                    index,
                    hovered,
                }
            }))
            .child(element);
        if selectable {
            region = region.on_click(crate::view::sidebar::callback(ctx, move |_| {
                Msg::SidebarRowActivate { panel, index }
            }));
        }
        if interactive && !panel_state.suppress_row_hover {
            // A transform rather than a style: it lifts whatever the row already painted, so
            // the active pane's row and the row under the keyboard cursor still respond to the
            // pointer. An absolute hover style sits *under* those backgrounds and never shows.
            region = region.hover_effect(VisualEffect::transform_bg(super::hover_lift()));
        }
        let element: Element = region.into();
        view = view.child(element.key(row_key(ctx, panel, index)));
    }
    view.key(body_key(ctx, panel))
}

/// Whether a row shows its ✕ this frame, and in which state.
///
/// Aiming is what reveals it — the pointer on the row, or the keyboard cursor while the sidebar
/// owns focus — so a resting list stays quiet and no row advertises a destructive action it is not
/// being aimed at. An *armed* row keeps it regardless: hiding a live confirmation the moment the
/// pointer drifts would leave the next click on that ✕ killing something with no warning on screen.
/// `suppress_row_hover` gates the hover case the same way the row's hover lift is gated, so
/// keyboard navigation does not leave a ✕ behind under a stale pointer; the cursor case is the
/// keyboard's own aim and is not gated by that flag.
fn close_affordance(
    ctx: &Context<AppRoot>,
    panel: usize,
    row: &row::SidebarRow,
    index: usize,
    selected: bool,
) -> Option<row::CloseAffordance> {
    let close = row.close.as_ref()?;
    let armed = ctx.state.sidebar.pending_row_close.as_ref() == Some(close);
    let panel_state = &ctx.state.sidebar.panels[panel];
    let hovered = panel_state.hovered_row == Some(index) && !panel_state.suppress_row_hover;
    (armed || hovered || selected).then_some(row::CloseAffordance {
        panel,
        index,
        armed,
    })
}

/// Per-row element key, used both for reconciliation and as the `scroll_to_key` target.
fn row_key(ctx: &Context<AppRoot>, panel: usize, index: usize) -> String {
    format!("sidebar-{}-row-{index}", panel_identity(&ctx.state, panel))
}

fn cursor_index(ctx: &Context<AppRoot>, panel: usize, tab: &SidebarTab) -> Option<usize> {
    crate::state::SidebarItemProjection::resolve_cursor(
        ctx.state.sidebar.panels[panel].cursor,
        &ctx.state.sidebar_item_projections(tab),
    )
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

/// The lift a row gets under the pointer in the file tree, matching pointer hover in the composed
/// row lists, which apply the same lift as a transform.
pub(super) fn row_highlight(fill: Color) -> Style {
    Style::new().bg(fill.elevate_by(super::HOVER_LIFT))
}

/// How far the keyboard cursor's background leans toward the active border color.
const CURSOR_TINT: f32 = 0.25;

/// The background a row gets under the keyboard cursor while the sidebar owns the keyboard.
///
/// A hover-sized lift alone was too quiet to find: the active row already carries a smaller lift of
/// its own, and the two differed by a few shades. Tinting toward the theme's active border instead
/// reads as "selected" at a glance without becoming a solid accent fill, so each span keeps the
/// color that carries its meaning — agent status, git state, and error red stay readable.
///
/// A fill with no RGB value (a theme following the terminal background) cannot be lifted or mixed,
/// so the cursor falls back to a tint of the element surface rather than vanishing.
pub(super) fn cursor_highlight(theme: &Theme, fill: Color) -> Style {
    let base = if fill.to_rgb().is_some() {
        fill
    } else {
        theme.surface.element
    };
    Style::new().bg(base
        .elevate_by(super::HOVER_LIFT)
        .blend_toward(theme.border_active, CURSOR_TINT))
}

pub(crate) fn fill_color(theme: &Theme, follow_canvas: bool) -> Color {
    if follow_canvas {
        theme.surface.backdrop
    } else {
        theme.surface.element
    }
}

pub(super) fn fill(ctx: &Context<AppRoot>) -> Color {
    fill_color(
        &ctx.state.theme,
        ctx.state.config.sidebar.background_follows_canvas,
    )
}

fn strip_fill(ctx: &Context<AppRoot>) -> Color {
    let host = fill(ctx);
    if ctx.state.config.sidebar.background {
        super::strip_background(
            &ctx.state.theme,
            ctx.state.config.sidebar.background_follows_canvas,
            host,
        )
    } else {
        host
    }
}

/// Where an empty tab's message sits. One cell in, so a sentence never starts hard against the
/// panel edge — shared with the file tree, whose own placeholder is rendered by the widget.
pub(super) const PLACEHOLDER_PADDING: (u16, u16, u16, u16) = (0, 0, 0, 1);

pub(super) fn placeholder(ctx: &Context<AppRoot>, text: &str) -> Element {
    VStack::new()
        .padding(PLACEHOLDER_PADDING)
        .child(Text::new(text.to_string()).style(super::fg_only(&ctx.state.theme.muted)))
        .into()
}

/// Reject queued UI events from a layout that has already been replaced.
fn callback<T: Send + 'static>(
    ctx: &Context<AppRoot>,
    map: impl Fn(T) -> Msg + Send + Sync + 'static,
) -> Callback<T> {
    let epoch = ctx.state.sidebar.layout_epoch;
    ctx.link().callback(move |value| Msg::SidebarUiEvent {
        epoch,
        event: Box::new(map(value)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_labels_keep_the_number_visible_and_ignore_blank_names() {
        let mut state = crate::state::State::new(
            crate::config::Config::default(),
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

    #[test]
    fn the_cursor_stands_apart_from_hover_and_the_active_row() {
        let theme = Theme::default();
        let fill = fill_color(&theme, false);
        let cursor = cursor_highlight(&theme, fill).bg;
        assert_ne!(cursor, row_highlight(fill).bg, "cursor differs from hover");
        assert_ne!(
            cursor,
            Style::new().bg(fill.elevate_by(0.04)).bg,
            "cursor differs from the active row"
        );

        // A fill that cannot be lifted still leaves the cursor visible.
        let cursor = cursor_highlight(&theme, Color::Reset).bg;
        assert_ne!(cursor, Style::new().bg(Color::Reset).bg);
        assert_ne!(cursor, None);
    }

    #[test]
    fn fill_tracks_the_follow_canvas_flag() {
        let theme = Theme::default();
        assert_eq!(fill_color(&theme, false), theme.surface.element);
        assert_eq!(fill_color(&theme, true), theme.surface.backdrop);
        assert_eq!(
            crate::view::strip_background(&theme, false, theme.surface.element),
            theme.surface.element.elevate_by(crate::view::STRIP_LIFT)
        );
        assert_ne!(
            crate::view::strip_background(&theme, false, theme.surface.element),
            theme.surface.element
        );
        assert_eq!(
            crate::view::strip_background(&theme, true, theme.surface.backdrop),
            theme.surface.element
        );
    }
}
