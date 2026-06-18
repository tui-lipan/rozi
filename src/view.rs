use tui_lipan::prelude::*;

use crate::geometry::{clamp_float_rect, clamp_floating_rect, close_rect, empty_workspace_rect};
use crate::input::Action;
use crate::layout::{ordered_panes, placement_for, workspace_target_rects_excluding};
use crate::state::{Pane, PaneId, TOP_BAR_HEIGHT};
use crate::{FrameworkFocus, HyprmuxApp, Msg};

pub fn render(app: &HyprmuxApp, ctx: &Context<HyprmuxApp>) -> Element {
    let theme = &ctx.state.theme;
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
        .style(Style::new().bg(theme.surface.backdrop))
        .height(Length::Flex(1));

    if workspace.panes.iter().all(|pane| pane.closing) {
        canvas = canvas.child_at(
            empty_workspace_rect(bounds).to_rect(),
            empty_workspace_panel(&ctx.state.config.input, theme),
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
            // Spawned panes appear at their tiled slot (and fade in via opacity); only
            // surrounding panes animate to make room.
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

    let mut root = VStack::new()
        .style(theme.primary.patch(Style::new().bg(theme.surface.backdrop)))
        .child(top_bar(ctx).height(Length::Px(TOP_BAR_HEIGHT)))
        .child(canvas);

    // Overlays portal to the root regardless of where they are attached.
    if ctx.state.show_palette {
        root = root.child(
            CommandPalette::new()
                .title("hyprmux commands")
                .frame_style(Style::new().bg(Color::Reset))
                .on_close(ctx.link().callback(|_| Msg::ClosePalette)),
        );
    }
    if ctx.state.show_help {
        root = root.child(help_overlay(ctx));
    }
    if ctx.state.show_theme_picker {
        root = root.child(theme_picker_overlay(ctx));
    }
    if ctx.state.search.is_some() {
        root = root.child(search_overlay(ctx));
    }

    ThemeProvider::new(ctx.state.theme.clone())
        .child(root)
        .into()
}

fn help_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let theme = &ctx.state.theme;
    let prefix = ctx.state.config.input.prefix.to_formatted_string(false);
    let mut body = VStack::new().gap(1).child(
        Text::new(format!(
            "Prefix any key with {prefix}, or hold the configured modifier. Esc closes this help."
        ))
        .style(theme.muted),
    );

    let mut last_category: Option<&str> = None;
    for binding in &crate::input::command_bindings() {
        if last_category != Some(binding.category) {
            body = body.child(help_section(binding.category));
            last_category = Some(binding.category);
        }
        body = body.child(help_row(binding.keys, binding.label));
    }

    body = body
        .child(help_section("Workspaces"))
        .child(help_row("1-9", "Switch to workspace"))
        .child(help_row("Shift+1-9", "Move pane to workspace"))
        .child(help_section("Mouse"))
        .child(help_row(
            "mod-drag",
            "Move / resize pane (left / right drag)",
        ));

    Modal::new()
        .title("Keybindings")
        .width(Length::Px(56))
        .on_close(ctx.link().callback(|_| Msg::CloseHelp))
        .child(body)
        .into()
}

fn search_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let Some(search) = ctx.state.search.as_ref() else {
        return Text::new("").into();
    };
    let theme = &ctx.state.theme;
    let input = Input::bound(&search.input)
        .placeholder("Search scrollback...")
        .prefix("/ ")
        .style(theme.primary.patch(Style::new().bg(theme.surface.element)))
        .focus_style(
            Style::new()
                .fg(theme.border_active)
                .bg(theme.surface.element),
        )
        .selection_style(theme.text_selection)
        .width(Length::Flex(1))
        .on_change(ctx.link().callback(Msg::SearchChanged))
        .on_key(ctx.link().key_handler(|key| {
            if key.is(KeyCode::Esc) {
                Some(Msg::CloseSearch)
            } else if key.code == KeyCode::Enter
                && !key.mods.ctrl
                && !key.mods.alt
                && !key.mods.super_key
            {
                Some(Msg::SearchNext(key.mods.shift))
            } else {
                None
            }
        }));

    Modal::new()
        .title(format!("Search pane {}", search.target))
        .width(Length::Px(64))
        .on_close(ctx.link().callback(|_| Msg::CloseSearch))
        .child(
            VStack::new()
                .gap(1)
                .child(input.key(search_input_key()))
                .child(Text::new(search.status.clone()).style(theme.muted)),
        )
        .into()
}

fn theme_picker_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let theme = &ctx.state.theme;
    let current = ctx.state.config.theme.preset;
    let applied_builtin = ctx.state.config.theme.path.is_none();
    let items = crate::state::ThemePreset::all().into_iter().map(|preset| {
        let item = ListItem::new(preset.label());
        if applied_builtin && preset == current {
            item.description("current").active(true)
        } else {
            item
        }
    });
    let body = VStack::new()
        .gap(1)
        .child(Text::new("Choose a built-in theme for this session.").style(theme.muted))
        .child(
            List::new()
                .items(items)
                .selected(ctx.state.theme_picker_selected)
                .height(Length::Auto)
                .border(true)
                .border_style(BorderStyle::Rounded)
                .style(theme.primary.patch(Style::new().bg(theme.surface.element)))
                .selection_full_width(true)
                .selection_symbol(Some("› "))
                .unselected_symbol(Some("  "))
                .active_symbol(Some("✓ "))
                .active_style(Style::new().fg(theme.status.success).bold())
                .selection_style(
                    Style::new()
                        .fg(theme.surface.backdrop)
                        .bg(theme.border_active),
                )
                .on_select(
                    ctx.link()
                        .callback(|event: ListEvent| Msg::ThemePickerSelected(event.index)),
                )
                .on_activate(
                    ctx.link()
                        .callback(|event: ListEvent| Msg::ThemePickerActivated(event.index)),
                )
                .key(theme_picker_key()),
        );

    Modal::new()
        .title("Choose theme")
        .width(Length::Px(36))
        .on_close(ctx.link().callback(|_| Msg::CloseThemePicker))
        .child(body)
        .into()
}

fn help_section(title: &str) -> Element {
    // The active ThemeProvider supplies inherited text/bg; use the theme accent for headings.
    Text::new(title.to_string())
        .style(Style::new().bold())
        .height(Length::Px(1))
        .into()
}

fn help_row(keys: &str, desc: &str) -> Element {
    HStack::new()
        .gap(2)
        .height(Length::Px(1))
        .child(
            Text::new(keys.to_string())
                .style(Style::new().bold())
                .width(Length::Px(12))
                .height(Length::Px(1)),
        )
        .child(
            Text::new(desc.to_string())
                .width(Length::Flex(1))
                .height(Length::Px(1)),
        )
        .into()
}

fn pane_element(
    app: &HyprmuxApp,
    ctx: &Context<HyprmuxApp>,
    pane: &Pane,
    animated_rect: FloatRect,
    effective_focus: Option<PaneId>,
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
            theme.border_active
        } else {
            theme.surface.menu
        },
    );
    let frame_bg = app.chrome_color(
        ctx,
        pane.id,
        "frame-bg",
        if focused {
            theme.surface.panel
        } else {
            theme.surface.backdrop
        },
    );
    let frame_style = Style::new().fg(frame_fg).bg(frame_bg);

    let mut window_stack = VStack::new().style(frame_style);
    if ctx.state.show_titles {
        let title_bar_bg = app.chrome_color(
            ctx,
            pane.id,
            "title-bg",
            if focused {
                theme.border_active
            } else {
                theme.surface.element
            },
        );
        let title_bar_fg = app.chrome_color(
            ctx,
            pane.id,
            "title-fg",
            if focused {
                theme.surface.backdrop
            } else {
                theme.surface.menu
            },
        );
        let title_bar_fill_style = Style::new().bg(title_bar_bg);
        let title_bar_text_style = if focused {
            Style::new().fg(title_bar_fg).bold()
        } else {
            Style::new().fg(title_bar_fg)
        };

        let title = pane.terminal.title().unwrap_or_else(|| pane.title.clone());
        let mut title_row = HStack::new()
            .style(title_bar_fill_style)
            .width(Length::Flex(1))
            .height(Length::Px(1))
            .child(
                Text::new(format!(" {icon}  {} · {title} ", pane.id))
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
            .on_mouse_down(
                ctx.link()
                    .callback(move |_| Msg::FocusPane(id, FrameworkFocus::Request)),
            )
            .child(title_row)
            .into();
        window_stack = window_stack.child(title_bar);
    }

    let terminal: Element = Terminal::new()
        .snapshot(pane.terminal.snapshot.clone())
        .style(theme.primary.patch(Style::new().bg(theme.surface.backdrop)))
        .selection_style(theme.text_selection)
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

/// Number of workspace tabs to show: at least 5, growing to include the active
/// workspace and the highest one that currently holds panes.
fn workspace_tab_count(state: &crate::state::State) -> usize {
    let occupied = state
        .workspaces
        .iter()
        .enumerate()
        .filter(|(_, ws)| ws.visible_count() > 0)
        .map(|(idx, _)| idx + 1)
        .max()
        .unwrap_or(0);
    occupied
        .max(state.active_workspace + 1)
        .max(5)
        .min(state.workspaces.len())
}

fn top_bar(ctx: &Context<HyprmuxApp>) -> HStack {
    let state = &ctx.state;
    let theme = &ctx.state.theme;
    let shown = workspace_tab_count(state);

    let tabs: Vec<Tab> = (0..shown)
        .map(|idx| {
            let count = state.workspaces[idx].visible_count();
            let label = if count > 0 {
                format!("{} ·{count}", idx + 1)
            } else {
                format!("{}", idx + 1)
            };
            Tab::new(label)
        })
        .collect();

    let workspace_tabs = Tabs::new()
        .tabs(tabs)
        .active(state.active_workspace.min(shown.saturating_sub(1)))
        .focusable(false)
        .width(Length::Flex(1))
        .height(Length::Px(1))
        .divider(' ')
        .style(Style::new().fg(theme.surface.menu).bg(theme.surface.panel))
        .active_style(
            Style::new()
                .fg(theme.surface.backdrop)
                .bg(theme.border_active)
                .bold(),
        )
        .tab_hover_style(
            Style::new()
                .fg(theme.surface.menu)
                .bg(theme.surface.element),
        )
        .on_change(
            ctx.link()
                .callback(|event: TabsEvent| Msg::RunAction(Action::SwitchWorkspace(event.index))),
        );

    let mut row = HStack::new()
        .gap(1)
        .height(Length::Px(1))
        .style(Style::new().bg(theme.surface.backdrop))
        .child(
            Text::new(" hyprmux ")
                .style(
                    Style::new()
                        .fg(theme.surface.backdrop)
                        .bg(theme.border_active)
                        .bold(),
                )
                .height(Length::Px(1)),
        )
        .child(workspace_tabs);

    if state.mode == crate::state::Mode::Prefix {
        row = row.child(
            Text::new(" PREFIX ")
                .style(
                    Style::new()
                        .fg(theme.surface.backdrop)
                        .bg(theme.status.warning)
                        .bold(),
                )
                .height(Length::Px(1)),
        );
    } else if state.mode == crate::state::Mode::Resize {
        row = row.child(
            Text::new(" RESIZE hjkl Esc ")
                .style(
                    Style::new()
                        .fg(theme.surface.backdrop)
                        .bg(theme.status.success)
                        .bold(),
                )
                .height(Length::Px(1)),
        );
    }

    row
}

fn empty_workspace_panel(input: &crate::state::InputConfig, theme: &Theme) -> Element {
    let prefix = input.prefix.to_formatted_string(false);
    Frame::new()
        .title(" Empty workspace ")
        .border(true)
        .border_style(BorderStyle::Rounded)
        .style(
            Style::new()
                .fg(theme.surface.menu)
                .bg(theme.surface.backdrop),
        )
        .padding(1)
        .child(
            VStack::new()
                .gap(1)
                .child(Text::new("No panes here yet."))
                .child(Text::new(format!(
                    "Press {}+Enter or {prefix} Enter to spawn a shell.",
                    input.modifier.label(),
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

pub fn search_input_key() -> &'static str {
    "hyprmux-search-input"
}

pub fn theme_picker_key() -> &'static str {
    "hyprmux-theme-picker"
}
