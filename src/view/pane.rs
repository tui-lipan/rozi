use tui_lipan::prelude::*;

use crate::state::{CapStyle, LayoutKind, Pane, PaneId, PaneTitlebarMode, TileGap, Workspace};
use crate::tiling::PanePlacement;
use crate::{HyprmuxApp, Msg};

use super::integrated_scrollbar_config;
use super::keys::{pane_body_key, pane_terminal_key, pane_window_key};

/// Caller-decided border-merge posture for one pane (see `view::render`).
#[derive(Clone, Copy, Default)]
pub(crate) struct PaneMerge {
    /// The pane is in the settled merged layer: its border may Exact-merge with neighbors.
    pub enabled: bool,
    /// The pane's left column is a neighbor's right border. A `Padded` title keeps its row off
    /// that column; a capped title instead draws its left cap there so the chip stays aligned
    /// with the border below (see `seam_left_bg`).
    pub left_seam: bool,
    /// Title background of the same-row neighbor sharing the left seam cell, if any. A capped
    /// left cap paints its off (left) half in this color so the shared cell reads as a split
    /// junction between the two titlebars. `None` when the neighbor shows a border there (a
    /// taller pane above the seam) - the cap falls back to the backdrop and sits on the border.
    pub seam_left_bg: Option<Color>,
    /// Title background of the same-row neighbor sharing the right seam cell, if any. Mirrors
    /// `seam_left_bg` for the right cap so either side of a seam renders the same split.
    pub seam_right_bg: Option<Color>,
}

/// A pane's titlebar background: the active border color when focused, otherwise the neutral
/// surface element color, honoring any per-pane `title-bg` chrome override.
pub(crate) fn pane_title_bg(
    app: &HyprmuxApp,
    ctx: &Context<HyprmuxApp>,
    pane_id: PaneId,
    focused: bool,
) -> Color {
    let theme = &ctx.state.theme;
    let target = if focused {
        theme.border_active
    } else {
        theme.surface.element
    };
    app.chrome_color(ctx, pane_id, "title-bg", target)
}

/// A padded integrated titlebar owns the frame's top border row: the decoration paints the
/// corners and slack cells, while the header paints the title text across the inner span.
fn integrated_titlebar_top_edge(bg: Color) -> EdgeDecoration {
    EdgeDecoration::new(Edge::Top)
        .glyph(DecorationGlyph::Custom(' '))
        .cap_start(DecorationGlyph::Custom(' '))
        .cap_end(DecorationGlyph::Custom(' '))
        .style(
            Style::new()
                .fg(bg)
                .bg(bg)
                .contrast_policy(ContrastPolicy::Off),
        )
}

/// Half-block integrated titles use their caps as the frame's corner cells rather than placing
/// them inside rounded/plain corners. The header paints the colored span between these caps.
fn integrated_half_titlebar_top_edge(title_bg: Color, frame_bg: Color) -> EdgeDecoration {
    EdgeDecoration::new(Edge::Top)
        .glyph(DecorationGlyph::Custom(' '))
        .cap_start(DecorationGlyph::Custom('▐'))
        .cap_end(DecorationGlyph::Custom('▌'))
        .style(
            Style::new()
                .fg(title_bg)
                .bg(frame_bg)
                .contrast_policy(ContrastPolicy::Off),
        )
}

pub(crate) fn pane_element(
    app: &HyprmuxApp,
    ctx: &Context<HyprmuxApp>,
    pane: &Pane,
    animated_rect: FloatRect,
    effective_focus: Option<PaneId>,
    title_marker: Option<&str>,
    merge: PaneMerge,
) -> Element {
    let theme = &ctx.state.theme;
    let id = pane.id;
    let focused = effective_focus == Some(id);
    let icon = if pane.fullscreen {
        "󰊓"
    } else if pane.floating {
        "󰹙"
    } else {
        "󰖲"
    };
    let badge = if pane.closing {
        Some("closing")
    } else if pane.fullscreen {
        Some("fullscreen")
    } else if pane.floating {
        Some("floating")
    } else {
        None
    };
    // Floating panes keep a Double border so they read as a distinct layer; tiled panes follow
    // the app-wide `pane.border_style` (cycled by `Action::CycleBorderStyle`). Border merging
    // is achieved by overlapping tiled pane rects a cell (see `State::tile_gap`) so neighbors
    // share a border column that the terminal backend fuses - no per-frame join flag needed.
    let border_style = if pane.floating {
        BorderStyle::Double
    } else {
        ctx.state.config.pane.border_style.to_border_style()
    };

    let frame_fg = app.chrome_color(
        ctx,
        pane.id,
        "frame-fg",
        crate::ops::theme::pane_frame_foreground(
            theme,
            focused,
            ctx.state.config.pane.highlight_focused_border,
        ),
    );
    let frame_bg = app.chrome_color(
        ctx,
        pane.id,
        "frame-bg",
        crate::ops::theme::pane_frame_background(
            theme,
            focused,
            ctx.state.config.pane.highlight_focused_background,
        ),
    );
    let exited = matches!(pane.terminal.status, ManagedTerminalStatus::Exited(_));
    let frame_style = if exited {
        Style::new().fg(frame_fg).bg(frame_bg).dim()
    } else {
        Style::new().fg(frame_fg).bg(frame_bg)
    };
    let titlebar = ctx.state.config.pane.titlebar;
    let show_titles = ctx.state.config.pane.show_titles;
    let titlebar_focused = focused && ctx.state.config.pane.highlight_focused_titlebar;
    let title_bar_bg_target = if titlebar_focused {
        theme.border_active
    } else {
        theme.surface.element
    };
    let title_bar_bg = pane_title_bg(app, ctx, pane.id, titlebar_focused);
    let title_fg_background = if titlebar == PaneTitlebarMode::Border {
        frame_bg
    } else {
        title_bar_bg_target
    };
    let title_fg_default = if titlebar == PaneTitlebarMode::Border {
        crate::ops::theme::pane_border_title_foreground(
            theme,
            titlebar_focused,
            title_fg_background,
        )
    } else {
        crate::ops::theme::pane_title_foreground(theme, titlebar_focused, title_fg_background)
    };
    let title_bar_fg = app.chrome_color(ctx, pane.id, "title-fg", title_fg_default);
    let title_bar_fill_style = Style::new()
        .bg(title_bar_bg)
        .contrast_policy(ContrastPolicy::Off);
    let title_bar_text_style = if titlebar_focused {
        Style::new()
            .fg(title_bar_fg)
            .bold()
            .contrast_policy(ContrastPolicy::Off)
    } else {
        Style::new()
            .fg(title_bar_fg)
            .contrast_policy(ContrastPolicy::Off)
    };
    let border_title_text_style = if titlebar_focused {
        Style::new()
            .fg(title_bar_fg)
            .bold()
            .contrast_policy(ContrastPolicy::Off)
    } else {
        Style::new()
            .fg(title_bar_fg)
            .contrast_policy(ContrastPolicy::Off)
    };
    let title = show_titles.then(|| {
        let mut title = pane.titlebar_title(ctx.state.current().remote_target.is_some());
        if let ManagedTerminalStatus::Exited(code) = pane.terminal.status {
            title.push_str(&format!(" [exited {code}]"));
        }
        if pane.logging {
            title.push_str(" [log]");
        }
        match title_marker {
            Some(marker) => format!("{marker} · {title}"),
            None => title,
        }
    });

    // The wrapper stack must stay unstyled: a styled stack fills its whole rect with its
    // background, and merged panes overlap neighbors by a cell, so that fill would wipe the
    // neighbor's border glyph before this pane's border draws and fuses with it. The bar title
    // row and the body frame each paint their own background, covering the full rect anyway.
    let mut window_stack = VStack::new().align(Align::Stretch);
    if show_titles && titlebar == PaneTitlebarMode::Bar {
        let title_text: Element = Text::new(format!(
            "{icon}  {}",
            title.as_ref().expect("visible titlebar has a title")
        ))
        .style(title_bar_text_style)
        .overflow(Overflow::Ellipsis)
        .width(Length::Flex(1))
        .height(Length::Px(1))
        .into();
        let badge_text: Option<Element> = badge.map(|badge| {
            Text::new(format!(" {badge}"))
                .style(title_bar_text_style)
                .height(Length::Px(1))
                .into()
        });

        // `Padded` keeps the title flush with the frame below, with blank side padding. The cap
        // styles instead draw the titlebar color as end caps over the backdrop, so the row reads
        // as a rounded/pointed pill: the fill (and text) live in a Flex middle between the caps.
        let title_row: Element = match ctx.state.config.pane.title_style.caps() {
            None => {
                let mut row = HStack::new()
                    .style(title_bar_fill_style)
                    .padding((0, 1))
                    .width(Length::Flex(1))
                    .height(Length::Px(1))
                    .child(title_text);
                if let Some(badge_text) = badge_text {
                    row = row.child(badge_text);
                }
                row.into()
            }
            Some((left, right)) => {
                // A cap paints its filled half in the titlebar color and its off half in `bg`. On
                // a merged seam the off half takes the neighbor's title color, so the shared cell
                // reads as a split junction (the caller hands whichever pane draws last the same
                // left/right colors, so the seam looks the same regardless of draw order). Off a
                // seam the off half is the backdrop, giving the usual pill edge.
                let backdrop = theme.surface.backdrop;
                let left_cap_bg = if merge.left_seam {
                    merge.seam_left_bg.unwrap_or(backdrop)
                } else {
                    backdrop
                };
                let right_cap_bg = merge.seam_right_bg.unwrap_or(backdrop);
                // No horizontal padding here: the caps themselves stand in for the side padding.
                let mut middle = HStack::new()
                    .style(title_bar_fill_style)
                    .width(Length::Flex(1))
                    .height(Length::Px(1))
                    .child(title_text);
                if let Some(badge_text) = badge_text {
                    middle = middle.child(badge_text);
                }
                HStack::new()
                    .width(Length::Flex(1))
                    .height(Length::Px(1))
                    .child(
                        Text::new(left)
                            .style(
                                Style::new()
                                    .fg(title_bar_bg)
                                    .bg(left_cap_bg)
                                    .contrast_policy(ContrastPolicy::Off),
                            )
                            .width(Length::Px(1))
                            .height(Length::Px(1)),
                    )
                    .child(middle)
                    .child(
                        Text::new(right)
                            .style(
                                Style::new()
                                    .fg(title_bar_bg)
                                    .bg(right_cap_bg)
                                    .contrast_policy(ContrastPolicy::Off),
                            )
                            .width(Length::Px(1))
                            .height(Length::Px(1)),
                    )
                    .into()
            }
        };

        let mut title_bar: Element = MouseRegion::new()
            .capture_click(true)
            .on_mouse_down(ctx.link().callback(move |_| Msg::FocusPane(id)))
            .child(title_row)
            .into();
        if merge.left_seam && ctx.state.config.pane.title_style.caps().is_none() {
            // A `Padded` title has no cap to sit on the seam, so keep its row off the shared
            // border column: the spacer is an empty Text that leaves the seam cell untouched for
            // the neighbor's border glyph. (Capped titles instead draw their left cap there, so
            // the chip lands flush on the seam - no spacer.)
            title_bar = HStack::new()
                .height(Length::Px(1))
                .child(Text::new("").width(Length::Px(1)).height(Length::Px(1)))
                .child(title_bar)
                .into();
        }
        window_stack = window_stack.child(title_bar);
    }

    let snapshot = terminal_snapshot_for_pane(ctx, pane);
    let terminal_ready = pane.terminal_active && !pane.opening && !pane.closing;
    let show_cursor = terminal_cursor_visible(
        snapshot.cursor_visible,
        ctx.state.hint_mode.as_ref().map(|hints| hints.target),
        id,
        exited,
    );
    let mut selection_style = theme.text_selection;
    if ctx
        .state
        .copy_flash
        .is_some_and(|flash| flash.target == id && !flash.clearing)
    {
        selection_style = selection_style.patch(ClipboardConfig::default().copy_feedback_style);
    }

    let mut terminal_widget = Terminal::new()
        .snapshot(snapshot)
        .paste_shortcut_behavior(TerminalPasteShortcutBehavior::Performable)
        .show_cursor(show_cursor)
        .style(theme.primary.patch(Style::new().bg(frame_bg)))
        .selection_style(selection_style)
        .focus_style(Style::default())
        .focusable(terminal_ready)
        .width(Length::Flex(1))
        .height(Length::Flex(1))
        .scrollbar_config(
            integrated_scrollbar_config()
                .thumb_style(Style::new().fg(frame_fg))
                .thumb_focus_style(Style::new().fg(frame_fg))
                .track_style(Style::new().fg(frame_fg).bg(frame_bg)),
        )
        .scroll_wheel(terminal_ready)
        .on_resize(ctx.link().callback(move |viewport: TerminalViewport| {
            Msg::PaneResize(id, viewport.cols, viewport.rows)
        }))
        .on_scroll_to(
            ctx.link()
                .callback(move |offset| Msg::PaneScroll(id, offset)),
        );
    if terminal_ready {
        terminal_widget = terminal_widget
            .on_input(ctx.link().callback(move |input| Msg::PaneInput(id, input)))
            .on_key(
                ctx.link()
                    .key_handler(move |key| Some(Msg::PaneKey(id, key))),
            )
            .on_mouse_forward(ctx.link().callback(move |bytes| Msg::PaneMouse(id, bytes)));
    }
    let controlled_selection =
        copy_mode_selection(ctx, id).or_else(|| copy_flash_selection(ctx, id));
    if controlled_selection.is_some()
        || ctx
            .state
            .copy_flash
            .is_some_and(|flash| flash.target == id && flash.clearing)
    {
        terminal_widget = terminal_widget.selection(controlled_selection);
    }
    let terminal: Element = terminal_widget.into();
    let terminal = terminal.key(pane_terminal_key(id));

    // Border fusing is buffer-level: any two box-drawing glyphs sharing a cell merge unless the
    // later frame draws in Replace mode, so only panes in the settled merged layer may merge
    // (the caller decides - floating, fullscreen, scratch, mid-drag, and mid-animation panes must
    // occlude whatever is beneath them, not grow junctions into it). Fuzzy rather than Exact:
    // rounded corners have no arc junction glyphs, so Exact refuses to fuse them, while Fuzzy
    // merges exactly when possible and falls back to plain junctions for arcs.
    let mut body = Frame::new()
        .border(true)
        .border_style(border_style)
        .border_merge_mode(if merge.enabled {
            BorderMergeMode::Fuzzy
        } else {
            BorderMergeMode::Replace
        })
        .padding(ctx.state.config.pane.padding)
        .style(frame_style)
        .focus_style(Style::default());
    if show_titles {
        match titlebar {
            PaneTitlebarMode::Border => {
                let title = title.as_ref().expect("visible titlebar has a title");
                let border_title = format!("{icon}  {title}");
                let mut labels = BorderLabels::new()
                    .left(border_title)
                    .style(border_title_text_style)
                    .padding(1);
                if let Some(badge) = badge {
                    labels = labels.right(format!("· {badge}"));
                }
                body = body.header(labels);
            }
            PaneTitlebarMode::Integrated => {
                let title_text: Element = Text::new(format!(
                    "{icon}  {}",
                    title.as_ref().expect("visible titlebar has a title")
                ))
                .style(title_bar_text_style)
                .overflow(Overflow::Ellipsis)
                .width(Length::Flex(1))
                .height(Length::Px(1))
                .into();
                let badge_text: Option<Element> = badge.map(|badge| {
                    Text::new(format!(" {badge}"))
                        .style(title_bar_text_style)
                        .height(Length::Px(1))
                        .into()
                });
                let title_style = ctx.state.config.pane.title_style;
                let caps = title_style.caps();
                let title_row: Element = match caps {
                    None => {
                        let mut row = HStack::new()
                            .style(title_bar_fill_style)
                            .padding((0, 1))
                            .width(Length::Flex(1))
                            .height(Length::Px(1))
                            .child(title_text);
                        if let Some(badge_text) = badge_text {
                            row = row.child(badge_text);
                        }
                        row.into()
                    }
                    Some(_) if title_style == CapStyle::Half => {
                        let mut row = HStack::new()
                            .style(title_bar_fill_style)
                            .padding((0, 0, 0, 1))
                            .width(Length::Flex(1))
                            .height(Length::Px(1))
                            .child(title_text);
                        if let Some(badge_text) = badge_text {
                            row = row.child(badge_text);
                        }
                        row.into()
                    }
                    Some((left, right)) => {
                        let mut middle = HStack::new()
                            .style(title_bar_fill_style)
                            .width(Length::Flex(1))
                            .height(Length::Px(1))
                            .child(title_text);
                        if let Some(badge_text) = badge_text {
                            middle = middle.child(badge_text);
                        }
                        // Keep the Frame's corner glyphs visible for capped integrated headers. The
                        // caps sit immediately inside them, using the pane background as their off
                        // color, while the colored middle still spans the compact top border row.
                        HStack::new()
                            .width(Length::Flex(1))
                            .height(Length::Px(1))
                            .child(
                                Text::new(left)
                                    .style(
                                        Style::new()
                                            .fg(title_bar_bg)
                                            .bg(frame_bg)
                                            .contrast_policy(ContrastPolicy::Off),
                                    )
                                    .width(Length::Px(1))
                                    .height(Length::Px(1)),
                            )
                            .child(middle)
                            .child(
                                Text::new(right)
                                    .style(
                                        Style::new()
                                            .fg(title_bar_bg)
                                            .bg(frame_bg)
                                            .contrast_policy(ContrastPolicy::Off),
                                    )
                                    .width(Length::Px(1))
                                    .height(Length::Px(1)),
                            )
                            .into()
                    }
                };
                let header: Element = MouseRegion::new()
                    .capture_click(true)
                    .on_mouse_down(ctx.link().callback(move |_| Msg::FocusPane(id)))
                    .child(title_row)
                    .into();
                body = body.header_content(header);
                body = match title_style {
                    CapStyle::Padded => body.decoration(integrated_titlebar_top_edge(title_bar_bg)),
                    CapStyle::Half => {
                        body.decoration(integrated_half_titlebar_top_edge(title_bar_bg, frame_bg))
                    }
                    CapStyle::Round | CapStyle::Arrow => body,
                };
            }
            PaneTitlebarMode::Bar => {}
        }
    }
    let body: Element = body.child(terminal).into();
    let body = body.key(pane_body_key(id));
    window_stack = window_stack.child(body);

    let mut window_region = MouseRegion::new()
        .capture_requires_mods(ctx.state.config.input.modifier.key_mods())
        .drag_requires_mods(ctx.state.config.input.modifier.key_mods())
        .right_drag_requires_mods(ctx.state.config.input.modifier.key_mods())
        .on_drag_start(ctx.link().callback(move |event: MouseDragEvent| {
            Msg::BeginMove(
                id,
                animated_rect,
                event.from_local_x,
                event.from_local_y,
                event.target_w,
                event.target_h,
                event.mods.alt || event.mods.super_key,
            )
        }))
        .on_drag(ctx.link().callback(move |event: MouseDragEvent| {
            Msg::MovePane(
                id,
                event.delta_x,
                event.delta_y,
                event.mods.alt || event.mods.super_key,
            )
        }))
        .on_drag_end(
            ctx.link()
                .callback(move |event: MouseDragEvent| Msg::EndMove(id, event.x, event.y)),
        )
        .on_right_drag_start(ctx.link().callback(move |event: MouseDragEvent| {
            Msg::BeginResize(
                id,
                crate::geometry::nearest_resize_corner(event),
                event.from_x,
                event.from_y,
                event.mods.alt || event.mods.super_key,
            )
        }))
        .on_right_drag(ctx.link().callback(move |event: MouseDragEvent| {
            Msg::ResizePane(
                id,
                crate::geometry::nearest_resize_corner(event),
                event.from_x,
                event.from_y,
                event.x,
                event.y,
                event.mods.alt || event.mods.super_key,
            )
        }))
        .on_right_drag_end(ctx.link().callback(move |_| Msg::EndResize(id)));

    if ctx.state.config.pane.focus_on_hover {
        window_region =
            window_region.on_mouse_move(ctx.link().callback(move |_| Msg::HoverPane(id)));
    }

    window_region = window_region
        .bubble_mouse_down(true)
        .on_mouse_down(ctx.link().callback(move |_| Msg::FocusPane(id)));

    let opacity = if pane.closing || pane.opening {
        0.0
    } else {
        1.0
    };
    let pane_tree: Element = ThemeProvider::new(ctx.state.theme.clone().focus(Style::default()))
        .child(window_region.child(window_stack))
        .into();
    let element: Element = Animated::new(pane_tree)
        .opacity(opacity)
        .transition(app.window_opacity_config(ctx, pane))
        .into();

    element.key(pane_window_key(id))
}

fn terminal_snapshot_for_pane(ctx: &Context<HyprmuxApp>, pane: &Pane) -> TerminalRenderSnapshot {
    if let Some(hints) = ctx
        .state
        .hint_mode
        .as_ref()
        .filter(|hints| hints.target == pane.id)
    {
        return pane.terminal.hint_snapshot(
            &hints.matches,
            &hints.labels,
            &hints.input,
            ctx.state.theme.text_selection,
            active_search_match_style(),
        );
    }
    let Some(query) = search_highlight_query(ctx, pane.id) else {
        return pane.terminal.snapshot();
    };
    pane.terminal.search_highlighted_snapshot(
        query,
        search_match_style(),
        active_search_match_style(),
        active_search_highlight(ctx, pane),
    )
}

fn search_highlight_query(ctx: &Context<HyprmuxApp>, id: PaneId) -> Option<&str> {
    let search = ctx.state.search.as_ref()?;
    let query = search.input.text().trim();
    if query.is_empty() || search.matches.is_empty() {
        return None;
    }
    search
        .matches
        .iter()
        .any(|matched| matched.pane == id)
        .then_some(query)
}

fn active_search_highlight(
    ctx: &Context<HyprmuxApp>,
    pane: &Pane,
) -> Option<crate::pane::TerminalSearchHighlight> {
    let search = ctx.state.search.as_ref()?;
    let matched = search.matches.get(search.current)?;
    // `scrollback_offset()` reads the screen directly; going through `snapshot()` here would clone
    // a whole snapshot just to compare one integer.
    if matched.pane != pane.id || matched.offset != pane.terminal.scrollback_offset() {
        return None;
    }
    Some(crate::pane::TerminalSearchHighlight {
        line: matched.line,
        start_col: matched.start_col,
        end_col: matched.end_col,
    })
}

fn search_match_style() -> Style {
    Style::new()
        .fg(Color::White)
        .bg(Color::rgb(92, 64, 8))
        .contrast_policy(ContrastPolicy::BlackOrWhite)
}

fn active_search_match_style() -> Style {
    Style::new()
        .fg(Color::Black)
        .bg(Color::Yellow)
        .bold()
        .contrast_policy(ContrastPolicy::BlackOrWhite)
}

/// Draggable strips on the boundary between adjacent tiled panes. Each strip resizes the split
/// on that boundary. Only dwindle (both axes) and master (the master/stack divider) have
/// adjustable ratios, so grid and monocle get no strips. With border merging on the gap is zero,
/// so the strip straddles the shared seam column/row (one cell thick) instead of filling a gap.
pub(crate) fn tiled_resize_strips(
    ctx: &Context<HyprmuxApp>,
    placements: &[PanePlacement],
    workspace: &Workspace,
) -> Vec<(FloatRect, Element)> {
    let master = matches!(workspace.layout_kind, LayoutKind::Master);
    if !master && workspace.layout_kind != LayoutKind::Dwindle {
        return Vec::new();
    }

    let tiled_ids = workspace.tiled_ids();
    let tiled: Vec<(PaneId, FloatRect)> = placements
        .iter()
        .filter(|placement| tiled_ids.contains(&placement.id))
        .map(|placement| (placement.id, placement.rect))
        .collect();

    let gap = ctx.state.tile_gap();
    let (vertical_strips, horizontal_strips) = resize_strip_hitboxes(&tiled, gap, master);
    let top_offset = f32::from(ctx.state.content_top_offset());

    let mut strips = Vec::new();
    for strip in &vertical_strips {
        strips.push((
            strip.rect,
            resize_strip_element(ctx, strip, true, strip.boundary),
        ));
    }
    for strip in &horizontal_strips {
        // Strip rects are canvas-space; the chrome coordinate is compared against event
        // coordinates, which are root-space.
        strips.push((
            strip.rect,
            resize_strip_element(ctx, strip, false, strip.boundary + top_offset),
        ));
    }
    for junction in resize_junction_hitboxes(&vertical_strips, &horizontal_strips) {
        strips.push((
            junction.rect,
            resize_junction_element(ctx, junction.horizontal_panes, junction.vertical_panes),
        ));
    }
    strips
}

fn resize_strip_hitboxes(
    tiled: &[(PaneId, FloatRect)],
    gap: TileGap,
    master: bool,
) -> (Vec<ResizeStripHitbox>, Vec<ResizeStripHitbox>) {
    // Include both neighboring border cells plus the gap between them. Merged borders overlap by
    // one cell, so this naturally collapses back to a one-cell shared seam.
    let h_gap = gap.horizontal;
    let v_gap = gap.vertical;
    let eps = 1.5;
    let mut vertical_strips = Vec::new();
    let mut horizontal_strips = Vec::new();
    for (a_id, a) in tiled {
        for (b_id, b) in tiled {
            // Vertical boundary → horizontal (left|right) split. `a` is the left pane.
            let a_right = a.x + a.w;
            if (b.x - (a_right + h_gap)).abs() < eps {
                let y0 = a.y.max(b.y);
                let y1 = (a.y + a.h).min(b.y + b.h);
                if y1 - y0 > eps {
                    vertical_strips.push(ResizeStripHitbox {
                        rect: FloatRect {
                            x: a_right - 1.0,
                            y: y0,
                            w: (b.x - a_right + 2.0).max(1.0),
                            h: y1 - y0,
                        },
                        pane_id: *a_id,
                        neighbor_id: *b_id,
                        boundary: b.x,
                    });
                }
            }
            // Horizontal boundary → vertical (top|bottom) split. Not adjustable in master.
            if !master {
                let a_bottom = a.y + a.h;
                if (b.y - (a_bottom + v_gap)).abs() < eps {
                    let x0 = a.x.max(b.x);
                    let x1 = (a.x + a.w).min(b.x + b.w);
                    if x1 - x0 > eps {
                        horizontal_strips.push(ResizeStripHitbox {
                            rect: FloatRect {
                                x: x0,
                                y: a_bottom - 1.0,
                                w: x1 - x0,
                                h: (b.y - a_bottom + 2.0).max(1.0),
                            },
                            pane_id: *a_id,
                            neighbor_id: *b_id,
                            boundary: b.y,
                        });
                    }
                }
            }
        }
    }
    (vertical_strips, horizontal_strips)
}

#[derive(Clone, Copy)]
struct ResizeStripHitbox {
    rect: FloatRect,
    /// The pane on the near side of the boundary (left or above); its tree split is the one a
    /// drag on this strip resizes.
    pane_id: PaneId,
    /// The pane on the far side of the boundary (right or below).
    neighbor_id: PaneId,
    /// Leading edge of `neighbor_id` on the strip's axis, in canvas coordinates. The strip covers
    /// pane chrome on both sides of it (borders, and the far pane's separate titlebar row), so a plain
    /// click or hover is routed to whichever pane owns the cell under the pointer.
    boundary: f32,
}

#[derive(Clone)]
struct ResizeJunctionHitbox {
    rect: FloatRect,
    /// Pane representatives on vertical boundaries, used to find horizontal tree splits.
    horizontal_panes: Vec<PaneId>,
    /// Pane representatives on horizontal boundaries, used to find vertical tree splits.
    vertical_panes: Vec<PaneId>,
}

impl ResizeStripHitbox {
    fn junction_probe_rect(self, vertical: bool) -> FloatRect {
        if vertical {
            FloatRect {
                x: self.rect.x,
                y: self.rect.y - 1.0,
                w: self.rect.w,
                h: self.rect.h + 2.0,
            }
        } else {
            FloatRect {
                x: self.rect.x - 1.0,
                y: self.rect.y,
                w: self.rect.w + 2.0,
                h: self.rect.h,
            }
        }
    }
}

fn intersect_rect(a: FloatRect, b: FloatRect) -> Option<FloatRect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.w).min(b.x + b.w);
    let y1 = (a.y + a.h).min(b.y + b.h);
    (x1 > x0 && y1 > y0).then_some(FloatRect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    })
}

fn resize_junction_hitboxes(
    vertical_strips: &[ResizeStripHitbox],
    horizontal_strips: &[ResizeStripHitbox],
) -> Vec<ResizeJunctionHitbox> {
    let mut junctions: Vec<ResizeJunctionHitbox> = Vec::new();
    for vertical in vertical_strips {
        for horizontal in horizontal_strips {
            let Some(rect) = intersect_rect(
                vertical.junction_probe_rect(true),
                horizontal.junction_probe_rect(false),
            ) else {
                continue;
            };
            let junction = junctions
                .iter_mut()
                .find(|junction| intersect_rect(junction.rect, rect).is_some());
            if let Some(junction) = junction {
                let x0 = junction.rect.x.min(rect.x);
                let y0 = junction.rect.y.min(rect.y);
                let x1 = (junction.rect.x + junction.rect.w).max(rect.x + rect.w);
                let y1 = (junction.rect.y + junction.rect.h).max(rect.y + rect.h);
                junction.rect = FloatRect {
                    x: x0,
                    y: y0,
                    w: x1 - x0,
                    h: y1 - y0,
                };
                if !junction.horizontal_panes.contains(&vertical.pane_id) {
                    junction.horizontal_panes.push(vertical.pane_id);
                }
                if !junction.vertical_panes.contains(&horizontal.pane_id) {
                    junction.vertical_panes.push(horizontal.pane_id);
                }
            } else {
                junctions.push(ResizeJunctionHitbox {
                    rect,
                    horizontal_panes: vec![vertical.pane_id],
                    vertical_panes: vec![horizontal.pane_id],
                });
            }
        }
    }
    junctions
}

/// Which pane owns the cell a pointer is over inside a resize strip. `boundary` is the far pane's
/// first row/column on the strip's axis, so a pointer at or past it sits on the far pane's leading
/// chrome (its separate titlebar row, when present); anything before it is still the near pane's
/// trailing border.
fn strip_pointer_owner(near: PaneId, far: PaneId, boundary: u16, along: u16) -> PaneId {
    if along >= boundary { far } else { near }
}

/// A resize strip sits *above* the panes it separates, so it also swallows pointer events over the
/// pane chrome beneath it. With a separate bar a horizontal strip is two rows - the upper pane's
/// bottom border and the lower pane's whole titlebar - so without this the titlebar of every pane
/// below the first row is unclickable. Focus clicks and hover-focus are re-issued here for
/// whichever pane owns the cell under the pointer; drags still resize the split.
fn resize_strip_element(
    ctx: &Context<HyprmuxApp>,
    strip: &ResizeStripHitbox,
    horizontal_split: bool,
    boundary: f32,
) -> Element {
    let near = strip.pane_id;
    let far = strip.neighbor_id;
    let pane_id = near;
    let boundary = boundary.round().max(0.0) as u16;
    let owner = move |x: u16, y: u16| {
        let along = if horizontal_split { x } else { y };
        strip_pointer_owner(near, far, boundary, along)
    };

    let mut region = MouseRegion::new()
        .on_mouse_down(
            ctx.link()
                .callback(move |event: MouseEvent| Msg::FocusPane(owner(event.x, event.y))),
        )
        .on_drag_start(ctx.link().callback(move |event: MouseDragEvent| {
            Msg::BeginResizeSplit(pane_id, horizontal_split, event.from_x, event.from_y)
        }))
        .on_drag(ctx.link().callback(move |event: MouseDragEvent| {
            Msg::ResizeSplit(
                pane_id,
                horizontal_split,
                event.from_x,
                event.from_y,
                event.x,
                event.y,
            )
        }))
        .on_drag_end(ctx.link().callback(move |_| Msg::EndResizeSplit));

    if ctx.state.config.pane.focus_on_hover {
        region =
            region
                .on_mouse_move(ctx.link().callback(move |event: MouseMoveEvent| {
                    Msg::HoverPane(owner(event.x, event.y))
                }));
    }

    region
        .child(Text::new("").width(Length::Flex(1)).height(Length::Flex(1)))
        .into()
}

fn resize_junction_element(
    ctx: &Context<HyprmuxApp>,
    horizontal_panes: Vec<PaneId>,
    vertical_panes: Vec<PaneId>,
) -> Element {
    MouseRegion::new()
        .on_drag_start(ctx.link().callback(move |event: MouseDragEvent| {
            Msg::BeginResizeSplitJunction(event.from_x, event.from_y)
        }))
        .on_drag(ctx.link().callback(move |event: MouseDragEvent| {
            Msg::ResizeSplitJunction(
                horizontal_panes.clone(),
                vertical_panes.clone(),
                event.from_x,
                event.from_y,
                event.x,
                event.y,
            )
        }))
        .on_drag_end(ctx.link().callback(move |_| Msg::EndResizeSplit))
        .child(Text::new("").width(Length::Flex(1)).height(Length::Flex(1)))
        .into()
}

/// Controlled selection for the copy-mode target pane. With no anchor it highlights just the
/// cursor cell; with an anchor it spans anchor→cursor inclusive (matching `extract_text`).
fn copy_mode_selection(ctx: &Context<HyprmuxApp>, id: PaneId) -> Option<TerminalSelection> {
    let copy = ctx
        .state
        .copy_mode
        .as_ref()
        .filter(|copy| copy.target == id)?;
    let cursor = (copy.cursor_row, copy.cursor_col);
    let (a, b) = copy
        .anchor
        .map(|anchor| (anchor, cursor))
        .unwrap_or((cursor, cursor));
    Some(selection_from_points(a, b))
}

fn copy_flash_selection(ctx: &Context<HyprmuxApp>, id: PaneId) -> Option<TerminalSelection> {
    let flash = ctx
        .state
        .copy_flash
        .filter(|flash| flash.target == id && !flash.clearing)?;
    Some(selection_from_points(flash.selection.0, flash.selection.1))
}

fn selection_from_points(a: (usize, usize), b: (usize, usize)) -> TerminalSelection {
    let (start, end) = if a <= b { (a, b) } else { (b, a) };
    TerminalSelection {
        anchor: tui_lipan::utils::GridPos {
            row: start.0,
            col: start.1,
        },
        // Exclusive end column so the cursor/anchor cell is included in the highlight.
        cursor: tui_lipan::utils::GridPos {
            row: end.0,
            col: end.1 + 1,
        },
    }
}

fn terminal_cursor_visible(
    snapshot_cursor_visible: bool,
    hint_target: Option<PaneId>,
    id: PaneId,
    exited: bool,
) -> bool {
    snapshot_cursor_visible && hint_target != Some(id) && !exited
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hint_mode_and_exit_state_suppress_snapshot_cursor_visibility() {
        assert!(!terminal_cursor_visible(true, Some(7), 7, false));
        assert!(terminal_cursor_visible(true, Some(7), 8, false));
        assert!(terminal_cursor_visible(true, None, 7, false));
        assert!(!terminal_cursor_visible(false, None, 7, false));
        assert!(!terminal_cursor_visible(false, Some(8), 7, false));
        assert!(!terminal_cursor_visible(true, None, 7, true));
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> FloatRect {
        FloatRect { x, y, w, h }
    }

    #[test]
    fn resize_strips_cover_both_borders_and_gap() {
        let tiled = vec![
            (1, rect(0.0, 0.0, 10.0, 10.0)),
            (2, rect(11.0, 0.0, 10.0, 10.0)),
        ];
        let (vertical, horizontal) = resize_strip_hitboxes(&tiled, TileGap::DEFAULT, false);

        assert!(horizontal.is_empty());
        assert_eq!(vertical.len(), 1);
        assert_eq!(vertical[0].rect, rect(9.0, 0.0, 3.0, 10.0));
    }

    #[test]
    fn stacked_resize_strips_cover_both_touching_borders() {
        let tiled = vec![
            (1, rect(0.0, 0.0, 10.0, 10.0)),
            (2, rect(0.0, 10.0, 10.0, 10.0)),
        ];
        let (vertical, horizontal) = resize_strip_hitboxes(&tiled, TileGap::DEFAULT, false);

        assert!(vertical.is_empty());
        assert_eq!(horizontal.len(), 1);
        assert_eq!(horizontal[0].rect, rect(0.0, 9.0, 10.0, 2.0));
    }

    #[test]
    fn resize_strip_junction_is_the_overlap_of_perpendicular_strips() {
        let tiled = vec![
            (1, rect(0.0, 0.0, 10.0, 10.0)),
            (2, rect(11.0, 0.0, 10.0, 10.0)),
            (3, rect(0.0, 10.0, 10.0, 10.0)),
            (4, rect(11.0, 10.0, 10.0, 10.0)),
        ];
        let (vertical, horizontal) = resize_strip_hitboxes(&tiled, TileGap::DEFAULT, false);
        let junctions = resize_junction_hitboxes(&vertical, &horizontal);

        assert_eq!(junctions.len(), 1, "coincident intersections must merge");
        assert_eq!(junctions[0].rect, rect(9.0, 9.0, 3.0, 2.0));
        assert_eq!(junctions[0].horizontal_panes, vec![1, 3]);
        assert_eq!(junctions[0].vertical_panes, vec![1, 2]);
    }

    /// With a separate bar, merged borders keep a zero vertical gap, so the strip between stacked
    /// panes covers two rows: the upper pane's bottom border and the lower pane's titlebar. The
    /// strip draws above both panes, so it must route a pointer on the lower row to the lower
    /// pane - otherwise no pane below the first row can be focused by its title.
    #[test]
    fn stacked_strip_routes_the_titlebar_row_to_the_lower_pane() {
        let gap = TileGap {
            horizontal: -1.0,
            vertical: 0.0,
        };
        let tiled = vec![
            (1, rect(0.0, 0.0, 10.0, 10.0)),
            (2, rect(0.0, 10.0, 10.0, 10.0)),
        ];
        let (_, horizontal) = resize_strip_hitboxes(&tiled, gap, false);

        assert_eq!(horizontal.len(), 1);
        let strip = horizontal[0];
        assert_eq!(strip.rect, rect(0.0, 9.0, 10.0, 2.0));
        assert_eq!(strip.boundary, 10.0);

        let boundary = strip.boundary as u16;
        let owner = |y| strip_pointer_owner(strip.pane_id, strip.neighbor_id, boundary, y);
        assert_eq!(owner(9), 1, "upper pane's bottom border stays with it");
        assert_eq!(owner(10), 2, "lower pane's titlebar row focuses that pane");
    }

    #[test]
    fn side_by_side_strip_routes_the_seam_column_to_the_right_pane() {
        let gap = TileGap {
            horizontal: -1.0,
            vertical: 0.0,
        };
        let tiled = vec![
            (1, rect(0.0, 0.0, 10.0, 10.0)),
            (2, rect(9.0, 0.0, 10.0, 10.0)),
        ];
        let (vertical, _) = resize_strip_hitboxes(&tiled, gap, false);

        assert_eq!(vertical.len(), 1);
        let strip = vertical[0];
        assert_eq!(strip.boundary, 9.0);

        let boundary = strip.boundary as u16;
        let owner = |x| strip_pointer_owner(strip.pane_id, strip.neighbor_id, boundary, x);
        assert_eq!(owner(8), 1, "left pane keeps its own right border");
        assert_eq!(
            owner(9),
            2,
            "the shared seam cell belongs to the right pane"
        );
    }
}
