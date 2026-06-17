use tui_lipan::prelude::*;

use crate::geometry::{clamp_float_rect, clamp_floating_rect, close_rect, empty_workspace_rect};
use crate::layout::{
    ordered_panes, placement_for, workspace_root_axis, workspace_target_rects_excluding,
};
use crate::state::{Pane, PaneId, TOP_BAR_HEIGHT};
use crate::{FrameworkFocus, HyprmuxApp, Msg};

pub fn render(app: &HyprmuxApp, ctx: &Context<HyprmuxApp>) -> Element {
    let viewport = ctx.viewport();
    let viewport_changed = ctx
        .state
        .last_viewport
        .replace(Some(viewport))
        .is_some_and(|previous| previous != viewport);
    let bounds = crate::geometry::canvas_bounds_from_viewport(viewport);
    let workspace = &ctx.state.workspaces[ctx.state.active_workspace];
    let moving_tiled = ctx
        .state
        .moving_pane
        .filter(|session| !session.was_floating)
        .map(|session| session.id);
    let placements = workspace_target_rects_excluding(workspace, bounds, moving_tiled);
    let effective_focus = effective_focused_pane(ctx, workspace);
    let mut canvas = Canvas::new()
        .style(Style::new().bg(Color::rgb(10, 12, 18)))
        .height(Length::Flex(1));

    if workspace.panes.iter().all(|pane| pane.closing) {
        canvas = canvas.child_at(
            empty_workspace_rect(bounds).to_rect(),
            empty_workspace_panel(&ctx.state.config.input),
        );
    }

    for pane in ordered_panes(workspace, effective_focus) {
        let base_rect = placement_for(&placements, pane.id)
            .unwrap_or_else(|| clamp_float_rect(pane.floating_rect, bounds));
        let moving = ctx
            .state
            .moving_pane
            .filter(|session| session.id == pane.id);
        let target_rect = if pane.closing {
            close_rect(pane.floating_rect)
        } else if let Some(session) = moving
            && !pane.fullscreen
        {
            if session.was_floating {
                clamp_floating_rect(session.drag_rect, bounds)
            } else {
                clamp_float_rect(session.drag_rect, bounds)
            }
        } else if pane.fullscreen {
            bounds
        } else {
            base_rect
        };
        let config = app.transition_config_for(ctx, pane, viewport_changed);
        let animated_rect = ctx.transition(
            format!("hyprmux-pane-rect-{}", pane.id),
            target_rect,
            config,
        );

        canvas = canvas.child_at(
            animated_rect.to_rect(),
            pane_element(app, ctx, pane, animated_rect, effective_focus),
        );
    }

    VStack::new()
        .style(
            Style::new()
                .bg(Color::rgb(10, 12, 18))
                .fg(Color::rgb(220, 225, 235)),
        )
        .child(top_bar(ctx, effective_focus).height(Length::Px(TOP_BAR_HEIGHT)))
        .child(canvas)
        .into()
}

fn pane_element(
    app: &HyprmuxApp,
    ctx: &Context<HyprmuxApp>,
    pane: &Pane,
    animated_rect: FloatRect,
    effective_focus: Option<PaneId>,
) -> Element {
    let id = pane.id;
    let focused = effective_focus == Some(id);
    let title_prefix = if pane.floating { "󰹙" } else { "󰖲" };
    let close_suffix = if pane.closing { " closing" } else { "" };
    let fullscreen_suffix = if pane.fullscreen { " fullscreen" } else { "" };
    let title = format!(
        " {title_prefix} #{} {}{}{} ",
        pane.id, pane.title, fullscreen_suffix, close_suffix
    );
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
            Color::rgb(124, 207, 255)
        } else {
            Color::rgb(125, 135, 150)
        },
    );
    let frame_bg = app.chrome_color(
        ctx,
        pane.id,
        "frame-bg",
        if focused {
            Color::rgb(18, 24, 34)
        } else {
            Color::rgb(15, 18, 26)
        },
    );
    let title_bar_bg = app.chrome_color(
        ctx,
        pane.id,
        "title-bg",
        if focused {
            Color::rgb(124, 207, 255)
        } else {
            Color::rgb(35, 42, 56)
        },
    );
    let title_bar_fg = app.chrome_color(
        ctx,
        pane.id,
        "title-fg",
        if focused {
            Color::rgb(15, 18, 26)
        } else {
            Color::rgb(175, 185, 202)
        },
    );

    let frame_style = Style::new().fg(frame_fg).bg(frame_bg);
    let title_bar_fill_style = Style::new().bg(title_bar_bg);
    let title_bar_text_style = if focused {
        Style::new().fg(title_bar_fg).bold()
    } else {
        Style::new().fg(title_bar_fg)
    };

    let title_bar: Element = MouseRegion::new()
        .capture_click(true)
        .on_mouse_down(
            ctx.link()
                .callback(move |_| Msg::FocusPane(id, FrameworkFocus::Request)),
        )
        .child(
            HStack::new()
                .style(title_bar_fill_style)
                .width(Length::Flex(1))
                .height(Length::Px(1))
                .child(
                    Text::new(format!(
                        "{title:<1} {} • {:.0}×{:.0} @ {:.0},{:.0} • {}",
                        if pane.fullscreen {
                            "fullscreen"
                        } else if pane.floating {
                            "floating"
                        } else {
                            "tiled"
                        },
                        animated_rect.w,
                        animated_rect.h,
                        animated_rect.x,
                        animated_rect.y,
                        pane.terminal.status_text(),
                    ))
                    .style(title_bar_text_style)
                    .overflow(Overflow::Ellipsis)
                    .width(Length::Flex(1))
                    .height(Length::Px(1)),
                ),
        )
        .into();

    let terminal: Element = Terminal::new()
        .snapshot(pane.terminal.snapshot.clone())
        .style(
            Style::new()
                .bg(Color::rgb(8, 10, 15))
                .fg(Color::rgb(222, 226, 235)),
        )
        .focusable(true)
        .width(Length::Flex(1))
        .height(Length::Flex(1))
        .scroll_wheel(true)
        .on_input(ctx.link().callback(move |input| Msg::PaneInput(id, input)))
        .on_key(
            ctx.link()
                .key_handler(move |key| Some(Msg::PaneKey(id, key))),
        )
        .on_resize(ctx.link().callback(move |viewport: TerminalViewport| {
            Msg::PaneResize(id, viewport.cols, viewport.rows)
        }))
        .on_scroll_to(
            ctx.link()
                .callback(move |offset| Msg::PaneScroll(id, offset)),
        )
        .on_mouse_forward(ctx.link().callback(move |bytes| Msg::PaneMouse(id, bytes)))
        .into();
    let terminal = terminal.key(pane_terminal_key(id));

    let body: Element = Frame::new()
        .border(true)
        .border_style(border_style)
        .style(frame_style)
        .focus_style(Style::default())
        .padding((0, 1, 0, 1))
        .child(terminal)
        .into();
    let body = body.key(pane_body_key(id));

    let window_stack = VStack::new()
        .style(frame_style)
        .child(title_bar)
        .child(body);

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
        .on_right_drag_end(ctx.link().callback(move |_| Msg::EndResize(id)))
        .on_mouse_move(ctx.link().callback(move |_| Msg::HoverPane(id)));

    window_region = window_region.bubble_mouse_down(true).on_mouse_down(
        ctx.link()
            .callback(move |_| Msg::FocusPane(id, FrameworkFocus::Preserve)),
    );

    let opacity = if pane.closing || pane.opening {
        0.0
    } else {
        1.0
    };
    let element: Element = Animated::new(window_region.child(window_stack))
        .opacity(opacity)
        .transition(app.window_opacity_config(pane))
        .into();

    element.key(pane_window_key(id))
}

fn top_bar(ctx: &Context<HyprmuxApp>, effective_focus: Option<PaneId>) -> VStack {
    let state = &ctx.state;
    let workspace = &state.workspaces[state.active_workspace];
    let workspace_strip = state.workspaces.iter().enumerate().fold(
        HStack::new().gap(1).height(Length::Px(1)),
        |row, (idx, workspace)| {
            let active = idx == state.active_workspace;
            let label = format!(
                " {}:{}{} ",
                idx + 1,
                workspace.visible_count(),
                if active { "*" } else { "" }
            );
            let style = if active {
                Style::new()
                    .fg(Color::rgb(15, 18, 26))
                    .bg(Color::rgb(124, 207, 255))
                    .bold()
            } else {
                Style::new()
                    .fg(Color::rgb(120, 130, 145))
                    .bg(Color::rgb(18, 24, 34))
            };
            row.child(Text::new(label).style(style).height(Length::Px(1)))
        },
    );

    let focus_label = effective_focus
        .map(|id| format!("#{id}"))
        .unwrap_or_else(|| "none".to_string());
    let mode_style = if state.mode == crate::state::Mode::Prefix {
        Style::new()
            .fg(Color::rgb(15, 18, 26))
            .bg(Color::rgb(255, 213, 110))
            .bold()
    } else {
        Style::new()
            .fg(Color::rgb(155, 166, 185))
            .bg(Color::rgb(18, 24, 34))
    };

    VStack::new()
        .style(Style::new().bg(Color::rgb(10, 12, 18)))
        .child(
            HStack::new()
                .gap(2)
                .height(Length::Px(1))
                .child(
                    Text::new(" hyprmux ")
                        .style(
                            Style::new()
                                .fg(Color::rgb(240, 245, 255))
                                .bg(Color::rgb(57, 91, 162))
                                .bold(),
                        )
                        .height(Length::Px(1)),
                )
                .child(workspace_strip)
                .child(
                    Text::new(format!(" mode={} ", state.mode.label()))
                        .style(mode_style)
                        .height(Length::Px(1)),
                )
                .child(
                    Text::new(format!(
                        "active={} focused={} root={} mod={} shell panes",
                        workspace.name,
                        focus_label,
                        workspace_root_axis(workspace).label(),
                        state.config.input.modifier.label(),
                    ))
                    .style(Style::new().fg(Color::rgb(150, 160, 176)))
                    .height(Length::Px(1)),
                ),
        )
        .child(
            Text::new(
                "Ctrl-a prefix • Enter/c spawn • w close • h/j/k/l focus • t float • f fullscreen • Space flip • [/] ratio • mod-drag move/resize",
            )
            .style(Style::new().fg(Color::rgb(155, 166, 185)))
            .height(Length::Px(1)),
        )
        .child(
            Text::new(state.status.clone())
                .style(Style::new().fg(Color::rgb(109, 213, 175)))
                .height(Length::Px(1)),
        )
}

fn empty_workspace_panel(input: &crate::state::InputConfig) -> Element {
    Frame::new()
        .title(" Empty workspace ")
        .border(true)
        .border_style(BorderStyle::Rounded)
        .style(
            Style::new()
                .fg(Color::rgb(130, 145, 165))
                .bg(Color::rgb(15, 18, 26)),
        )
        .padding(1)
        .child(
            VStack::new()
                .gap(1)
                .child(Text::new("No panes here yet."))
                .child(Text::new(format!(
                    "Press {}+Enter or Ctrl-a Enter to spawn a shell.",
                    input.modifier.label()
                ))),
        )
        .into()
}

fn effective_focused_pane(
    ctx: &Context<HyprmuxApp>,
    workspace: &crate::state::Workspace,
) -> Option<PaneId> {
    workspace
        .panes
        .iter()
        .filter(|pane| !pane.closing)
        .find(|pane| ctx.has_focus_within_key(pane_window_key(pane.id)))
        .map(|pane| pane.id)
        .or(ctx.state.focused_pane)
}

pub fn pane_window_key(id: PaneId) -> String {
    format!("hyprmux-pane-{id}")
}

pub fn pane_body_key(id: PaneId) -> String {
    format!("hyprmux-pane-body-{id}")
}

pub fn pane_terminal_key(id: PaneId) -> String {
    format!("hyprmux-terminal-{id}")
}
