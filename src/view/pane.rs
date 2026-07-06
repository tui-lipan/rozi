use tui_lipan::prelude::*;

use crate::state::{LayoutKind, Pane, PaneId, TILE_GAP, Workspace};
use crate::tiling::PanePlacement;
use crate::{HyprmuxApp, Msg};

use super::integrated_scrollbar_config;
use super::keys::{pane_body_key, pane_terminal_key, pane_window_key};

pub(crate) fn pane_element(
    app: &HyprmuxApp,
    ctx: &Context<HyprmuxApp>,
    pane: &Pane,
    animated_rect: FloatRect,
    effective_focus: Option<PaneId>,
    display_number: &str,
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
    let border_style = if pane.floating {
        BorderStyle::Double
    } else {
        BorderStyle::Rounded
    };

    let frame_fg = app.chrome_color(
        ctx,
        pane.id,
        "frame-fg",
        if focused {
            crate::theme_ops::pane_frame_foreground(theme, true)
        } else {
            crate::theme_ops::pane_frame_foreground(theme, false)
        },
    );
    let frame_bg = app.chrome_color(
        ctx,
        pane.id,
        "frame-bg",
        crate::theme_ops::pane_frame_background(
            theme,
            focused,
            ctx.state.config.pane.highlight_focused_background,
        ),
    );
    let frame_style = Style::new().fg(frame_fg).bg(frame_bg);

    let mut window_stack = VStack::new().align(Align::Stretch).style(frame_style);
    if ctx.state.config.pane.show_titles {
        let title_bar_bg_target = if focused {
            theme.border_active
        } else {
            theme.surface.element
        };
        let title_bar_bg = app.chrome_color(ctx, pane.id, "title-bg", title_bar_bg_target);
        let title_bar_fg = app.chrome_color(
            ctx,
            pane.id,
            "title-fg",
            crate::theme_ops::pane_title_foreground(theme, focused, title_bar_bg_target),
        );
        let title_bar_fill_style = Style::new().bg(title_bar_bg);
        let title_bar_text_style = if focused {
            Style::new()
                .fg(title_bar_fg)
                .bold()
                .contrast_policy(ContrastPolicy::BlackOrWhite)
        } else {
            Style::new()
                .fg(title_bar_fg)
                .contrast_policy(ContrastPolicy::BlackOrWhite)
        };

        let mut title = pane.display_title(pane.terminal.title());
        if let Some(subtitle) = pane.subtitle_for_title(&title) {
            title.push_str(" - ");
            title.push_str(subtitle);
        }
        let mut title_row = HStack::new()
            .style(title_bar_fill_style)
            .width(Length::Flex(1))
            .height(Length::Px(1))
            .child(
                Text::new(format!(" {icon}  {display_number} · {title} "))
                    .style(title_bar_text_style)
                    .overflow(Overflow::Ellipsis)
                    .width(Length::Flex(1))
                    .height(Length::Px(1)),
            );
        if let Some(badge) = badge {
            title_row = title_row.child(
                Text::new(format!(" {badge} "))
                    .style(title_bar_text_style)
                    .height(Length::Px(1)),
            );
        }

        let title_bar: Element = MouseRegion::new()
            .capture_click(true)
            .on_mouse_down(ctx.link().callback(move |_| Msg::FocusPane(id)))
            .child(title_row)
            .into();
        window_stack = window_stack.child(title_bar);
    }

    let snapshot = terminal_snapshot_for_pane(ctx, pane);
    let terminal_ready = pane.terminal_active && !pane.opening && !pane.closing;
    let mut terminal_widget = Terminal::new()
        .snapshot(snapshot)
        .style(theme.primary.patch(Style::new().bg(frame_bg)))
        .selection_style(theme.text_selection)
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
    if let Some(selection) = copy_mode_selection(ctx, id) {
        terminal_widget = terminal_widget.selection(Some(selection));
    }
    let terminal: Element = terminal_widget.into();
    let terminal = terminal.key(pane_terminal_key(id));

    let body: Element = Frame::new()
        .border(true)
        .border_style(border_style)
        .style(frame_style)
        .focus_style(Style::default())
        .child(terminal)
        .into();
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
                event.mods.alt || event.mods.super_key,
            )
        }))
        .on_right_drag(ctx.link().callback(move |event: MouseDragEvent| {
            Msg::ResizePane(
                id,
                crate::geometry::nearest_resize_corner(event),
                event.delta_x,
                event.delta_y,
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
        .transition(app.window_opacity_config(pane))
        .into();

    element.key(pane_window_key(id))
}

fn terminal_snapshot_for_pane(ctx: &Context<HyprmuxApp>, pane: &Pane) -> TerminalRenderSnapshot {
    let Some(query) = search_highlight_query(ctx, pane.id) else {
        return pane.terminal.snapshot.clone();
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
    if matched.pane != pane.id || matched.offset != pane.terminal.snapshot.scrollback_offset {
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

/// Draggable strips in the gaps between adjacent tiled panes. Each strip resizes the split on
/// that boundary. Only dwindle (both axes) and master (the master/stack divider) have
/// adjustable ratios, so grid and monocle get no strips.
pub(crate) fn tiled_resize_strips(
    ctx: &Context<HyprmuxApp>,
    placements: &[PanePlacement],
    workspace: &Workspace,
) -> Vec<(FloatRect, Element)> {
    let master = matches!(workspace.layout_kind, LayoutKind::Master);
    if !master
        && workspace.layout_kind != LayoutKind::Dwindle
    {
        return Vec::new();
    }

    let tiled_ids = workspace.tiled_ids();
    let tiled: Vec<(PaneId, FloatRect)> = placements
        .iter()
        .filter(|placement| tiled_ids.contains(&placement.id))
        .map(|placement| (placement.id, placement.rect))
        .collect();

    let eps = 1.5;
    let mut strips = Vec::new();
    for (a_id, a) in &tiled {
        for (_b_id, b) in &tiled {
            // Vertical gap → horizontal (left|right) split. `a` is the left pane.
            let a_right = a.x + a.w;
            if (b.x - (a_right + TILE_GAP)).abs() < eps {
                let y0 = a.y.max(b.y);
                let y1 = (a.y + a.h).min(b.y + b.h);
                if y1 - y0 > eps {
                    strips.push((
                        FloatRect {
                            x: a_right,
                            y: y0,
                            w: TILE_GAP,
                            h: y1 - y0,
                        },
                        resize_strip_element(ctx, *a_id, true),
                    ));
                }
            }
            // Horizontal gap → vertical (top|bottom) split. Not adjustable in master.
            if !master {
                let a_bottom = a.y + a.h;
                if (b.y - (a_bottom + TILE_GAP)).abs() < eps {
                    let x0 = a.x.max(b.x);
                    let x1 = (a.x + a.w).min(b.x + b.w);
                    if x1 - x0 > eps {
                        strips.push((
                            FloatRect {
                                x: x0,
                                y: a_bottom,
                                w: x1 - x0,
                                h: TILE_GAP,
                            },
                            resize_strip_element(ctx, *a_id, false),
                        ));
                    }
                }
            }
        }
    }
    strips
}

fn resize_strip_element(
    ctx: &Context<HyprmuxApp>,
    pane_id: PaneId,
    horizontal_split: bool,
) -> Element {
    MouseRegion::new()
        .on_drag(ctx.link().callback(move |event: MouseDragEvent| {
            Msg::ResizeSplit(pane_id, horizontal_split, event.delta_x, event.delta_y)
        }))
        .child(Text::new("").width(Length::Flex(1)).height(Length::Flex(1)))
        .into()
}

/// Controlled selection for the copy-mode target pane. With no anchor it highlights just the
/// cursor cell; with an anchor it spans anchor→cursor inclusive (matching `extract_text`).
fn copy_mode_selection(ctx: &Context<HyprmuxApp>, id: PaneId) -> Option<TerminalSelection> {
    let copy = ctx.state.copy_mode.filter(|copy| copy.target == id)?;
    let cursor = (copy.cursor_row, copy.cursor_col);
    let (a, b) = copy
        .anchor
        .map(|anchor| (anchor, cursor))
        .unwrap_or((cursor, cursor));
    let (start, end) = if a <= b { (a, b) } else { (b, a) };
    Some(TerminalSelection {
        anchor: tui_lipan::utils::GridPos {
            row: start.0,
            col: start.1,
        },
        // Exclusive end column so the cursor/anchor cell is included in the highlight.
        cursor: tui_lipan::utils::GridPos {
            row: end.0,
            col: end.1 + 1,
        },
    })
}
