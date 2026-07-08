use tui_lipan::prelude::*;

use crate::state::{LayoutKind, Pane, PaneId, TileGap, Workspace};
use crate::tiling::PanePlacement;
use crate::{HyprmuxApp, Msg};

use super::integrated_scrollbar_config;
use super::keys::{pane_body_key, pane_terminal_key, pane_window_key};

/// Caller-decided border-merge posture for one pane (see `view::render`).
#[derive(Clone, Copy, Default)]
pub(crate) struct PaneMerge {
    /// The pane is in the settled merged layer: its border may Exact-merge with neighbors.
    pub enabled: bool,
    /// The pane's left column is a neighbor's right border; the title row keeps off it.
    pub left_seam: bool,
}

pub(crate) fn pane_element(
    app: &HyprmuxApp,
    ctx: &Context<HyprmuxApp>,
    pane: &Pane,
    animated_rect: FloatRect,
    effective_focus: Option<PaneId>,
    display_number: &str,
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
        crate::theme_ops::pane_frame_foreground(
            theme,
            focused,
            ctx.state.config.pane.highlight_focused_border,
        ),
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

    // The wrapper stack must stay unstyled: a styled stack fills its whole rect with its
    // background, and merged panes overlap neighbors by a cell, so that fill would wipe the
    // neighbor's border glyph before this pane's border draws and fuses with it. The title row
    // and the body frame each paint their own background, covering the full rect anyway.
    let mut window_stack = VStack::new().align(Align::Stretch);
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

        let mut title_bar: Element = MouseRegion::new()
            .capture_click(true)
            .on_mouse_down(ctx.link().callback(move |_| Msg::FocusPane(id)))
            .child(title_row)
            .into();
        if merge.left_seam {
            // Keep the title row off the shared border column. The spacer is an empty Text so
            // the seam cell is left untouched for the neighbor's border glyph.
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

    // Border fusing is buffer-level: any two box-drawing glyphs sharing a cell merge unless the
    // later frame draws in Replace mode, so only panes in the settled merged layer may merge
    // (the caller decides - floating, fullscreen, scratch, mid-drag, and mid-animation panes must
    // occlude whatever is beneath them, not grow junctions into it). Fuzzy rather than Exact:
    // rounded corners have no arc junction glyphs, so Exact refuses to fuse them, while Fuzzy
    // merges exactly when possible and falls back to plain junctions for arcs.
    let body: Element = Frame::new()
        .border(true)
        .border_style(border_style)
        .border_merge_mode(if merge.enabled {
            BorderMergeMode::Fuzzy
        } else {
            BorderMergeMode::Replace
        })
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

    let mut strips = Vec::new();
    for strip in &vertical_strips {
        strips.push((strip.rect, resize_strip_element(ctx, strip.pane_id, true)));
    }
    for strip in &horizontal_strips {
        strips.push((strip.rect, resize_strip_element(ctx, strip.pane_id, false)));
    }
    for vertical in &vertical_strips {
        for horizontal in &horizontal_strips {
            if let Some(rect) = intersect_rect(
                vertical.junction_probe_rect(true),
                horizontal.junction_probe_rect(false),
            ) {
                strips.push((
                    rect,
                    resize_junction_element(ctx, vertical.pane_id, horizontal.pane_id),
                ));
            }
        }
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
        for (_b_id, b) in tiled {
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
    pane_id: PaneId,
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

fn resize_strip_element(
    ctx: &Context<HyprmuxApp>,
    pane_id: PaneId,
    horizontal_split: bool,
) -> Element {
    MouseRegion::new()
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
        .on_drag_end(ctx.link().callback(move |_| Msg::EndResizeSplit))
        .child(Text::new("").width(Length::Flex(1)).height(Length::Flex(1)))
        .into()
}

fn resize_junction_element(ctx: &Context<HyprmuxApp>, left_id: PaneId, top_id: PaneId) -> Element {
    MouseRegion::new()
        .on_drag_start(ctx.link().callback(move |event: MouseDragEvent| {
            Msg::BeginResizeSplitJunction(left_id, top_id, event.from_x, event.from_y)
        }))
        .on_drag(ctx.link().callback(move |event: MouseDragEvent| {
            Msg::ResizeSplitJunction(
                left_id,
                top_id,
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let junction = intersect_rect(
            vertical[0].junction_probe_rect(true),
            horizontal[0].junction_probe_rect(false),
        )
        .unwrap();

        assert_eq!(junction, rect(9.0, 9.0, 2.0, 2.0));
    }
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
