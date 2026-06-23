use tui_lipan::Justify::SpaceBetween;
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
    // Sampled every frame (even while closed) so the slide transition is seeded at 0.0 and the
    // first open animates up from below.
    let scratch_progress = crate::scratchpad::scratch_progress(app, ctx);
    // Centered modal dialogs dim the workspace behind them the same way the scratchpad does, so
    // the dialog reads as the focused layer. The scrollback search is excluded: it scrolls the
    // panes to reveal matches, so they must stay readable.
    let dialog_open = ctx.state.show_palette
        || ctx.state.show_help
        || ctx.state.show_theme_picker
        || ctx.state.rename.is_some();
    let dialog_dim_progress = ctx.transition::<f32>(
        "hyprmux-dialog-dim",
        if dialog_open { 1.0 } else { 0.0 },
        app.scratch_transition_config(),
    );
    // Panes dim for whichever focused layer is most deployed; the dims never compound.
    let pane_dim = crate::scratchpad::backdrop_dim(scratch_progress.max(dialog_dim_progress));
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
            clamp_floating_rect(session.drag_rect, bounds)
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
        // The titlebar shows a workspace-local position (1..N by insertion order), not the
        // process-wide `PaneId`, so panes renumber after a close instead of ticking upward
        // forever (the internal id still keys focus/tile-tree/sessions).
        let display_number = workspace
            .panes
            .iter()
            .position(|candidate| candidate.id == pane.id)
            .map(|index| index + 1)
            .unwrap_or_else(|| pane.id as usize)
            .to_string();
        let mut element = pane_element(
            app,
            ctx,
            pane,
            animated_rect,
            effective_focus,
            &display_number,
        );
        // Dim the workspace panes (opacity blends their text/borders rather than hiding them)
        // while a focused layer is up. instant_transition: `pane_dim` is already smoothed by the
        // underlying progress transitions, so this just applies it without re-easing.
        if pane_dim < 1.0 {
            element = Animated::new(element)
                .opacity(pane_dim)
                .transition(crate::anim::instant_transition())
                .into();
        }
        canvas = canvas.child_at(animated_rect.to_rect(), element);
    }

    // Draggable strips sit in the gaps between tiled panes so the split ratio can be adjusted
    // with the mouse (in addition to resize mode and modifier+right-drag).
    for (rect, element) in tiled_resize_strips(ctx, &placements, workspace) {
        canvas = canvas.child_at(rect.to_rect(), element);
    }

    // A transparent catcher swallows clicks meant for the dimmed panes and dismisses the
    // scratchpad when clicked; the dropdown then slides up from the bottom above everything.
    if let Some((rect, element)) = crate::scratchpad::scratch_backdrop(ctx, scratch_progress) {
        canvas = canvas.child_at(rect.to_rect(), element);
    }
    if let Some((rect, element)) = crate::scratchpad::scratch_placement(app, ctx, scratch_progress)
    {
        canvas = canvas.child_at(rect.to_rect(), element);
    }

    let mut root = VStack::new()
        .style(theme.primary.patch(Style::new().bg(theme.surface.backdrop)))
        .child(top_bar(ctx).height(Length::Px(TOP_BAR_HEIGHT)))
        .child(canvas);

    // Overlays portal to the root regardless of where they are attached.
    if ctx.state.show_palette {
        root = root.child(palette_overlay(ctx));
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
    if ctx.state.rename.is_some() {
        root = root.child(rename_overlay(ctx));
    }

    ThemeProvider::new(ctx.state.theme.clone())
        .child(root)
        .into()
}

fn help_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let theme = &ctx.state.theme;
    let prefix = ctx.state.config.input.prefix.to_string();
    let modifier = ctx.state.config.input.modifier.label();

    // Group bindings by category (first-seen order), so a category with non-contiguous
    // entries in the table still gets a single header — matching the command palette.
    let mut groups: Vec<(&'static str, Vec<(String, &'static str)>)> = Vec::new();
    for binding in &crate::input::command_bindings() {
        let row = (active_keys(ctx, binding), binding.label);
        match groups
            .iter_mut()
            .find(|(name, _)| *name == binding.category)
        {
            Some((_, rows)) => rows.push(row),
            None => groups.push((binding.category, vec![row])),
        }
    }
    // Workspace digits and mouse gestures aren't in the command table; append them.
    groups.push((
        "Workspaces",
        vec![
            ("1-9".to_string(), "Switch to workspace"),
            ("Shift+1-9".to_string(), "Move pane to workspace"),
        ],
    ));
    groups.push((
        "Mouse",
        vec![
            ("mod-drag".to_string(), "Move pane (left-drag)"),
            ("mod-right-drag".to_string(), "Resize pane from corner"),
            ("drag gap".to_string(), "Resize a tiled split"),
        ],
    ));

    let mut list = VStack::new();
    for (index, (category, rows)) in groups.iter().enumerate() {
        list = list.child(help_section(category, theme, index > 0));
        for (keys, label) in rows {
            list = list.child(help_row(keys, label, theme));
        }
    }

    let body = VStack::new()
        .child(
            Text::new(format!(
                "Prefix keys with {prefix}, or hold {modifier} with any listed key. Scroll for more · Esc closes."
            ))
            .style(theme.muted)
            .overflow(Overflow::Wrap)
            .height(Length::Auto),
        )
        .child(Text::new("").height(Length::Px(1)))
        .child(
            ScrollView::new()
                .children(vec![list.into()])
                .focusable(true)
                .scroll_wheel(true)
                .scrollbar(true)
                .scrollbar_config(ScrollbarConfig::new().variant(ScrollbarVariant::Integrated))
                .height(Length::Flex(1))
                .key(help_scroll_key()),
        );

    styled_modal(ctx, "Keybindings", 50)
        .height(Length::Percent(70))
        .padding((1, 1, 1, 2))
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
            } else if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
                Some(Msg::SearchCycleScope)
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

    styled_modal(
        ctx,
        &format!("Search · {} · Tab: scope", search.scope.label()),
        64,
    )
    .padding((1, 2, 1, 2))
    .on_close(ctx.link().callback(|_| Msg::CloseSearch))
    .child(
        VStack::new()
            .gap(1)
            .child(input.key(search_input_key()))
            .child(Text::new(search.status.clone()).style(theme.muted)),
    )
    .into()
}

fn rename_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let Some(rename) = ctx.state.rename.as_ref() else {
        return Text::new("").into();
    };
    let theme = &ctx.state.theme;
    let input = Input::bound(&rename.input)
        .placeholder("Pane name, empty clears custom title")
        .style(theme.primary.patch(Style::new().bg(theme.surface.element)))
        .focus_style(
            Style::new()
                .fg(theme.border_active)
                .bg(theme.surface.element),
        )
        .selection_style(theme.text_selection)
        .width(Length::Flex(1))
        .on_change(ctx.link().callback(Msg::RenamePaneChanged))
        .on_key(ctx.link().key_handler(|key| {
            if key.is(KeyCode::Esc) {
                Some(Msg::CloseRenamePane)
            } else if key.code == KeyCode::Enter
                && !key.mods.ctrl
                && !key.mods.alt
                && !key.mods.super_key
            {
                Some(Msg::SubmitRenamePane)
            } else {
                None
            }
        }));

    styled_modal(ctx, &format!("Rename pane {}", rename.target), 56)
        .padding((1, 2, 1, 2))
        .on_close(ctx.link().callback(|_| Msg::CloseRenamePane))
        .child(input.key(rename_input_key()))
        .into()
}

fn palette_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    // Commands are sourced from the single binding table; only entries flagged for
    // the palette appear here. The help overlay remains the full keybinding reference.
    // Group by category (first-seen order) so each category header appears once even
    // when bindings of the same category are not declared contiguously.
    let mut groups: Vec<(&'static str, Vec<SearchEntry<Action>>)> = Vec::new();
    for binding in crate::input::command_bindings()
        .into_iter()
        .filter(|binding| binding.palette)
    {
        let mut entry = SearchEntry::item(binding.label, binding.action);
        let keys = active_keys(ctx, &binding);
        if !keys.is_empty() {
            entry = entry.description(ItemDescription::new().right(keys));
        }
        match groups
            .iter_mut()
            .find(|(category, _)| *category == binding.category)
        {
            Some((_, items)) => items.push(entry),
            None => groups.push((binding.category, vec![entry])),
        }
    }
    let mut entries: Vec<SearchEntry<Action>> = Vec::new();
    for (category, items) in groups {
        entries.push(SearchEntry::header(category));
        entries.extend(items);
    }

    let palette = action_search_palette(ctx, entries, "Search commands…");

    action_palette_modal(ctx, "Commands")
        .on_close(ctx.link().callback(|_| Msg::ClosePalette))
        .child(palette)
        .key(palette_key())
}

/// A fully-configured `SearchPalette` shared by the command palette and the theme picker, so
/// the two overlays look and behave identically (only their entries/placeholder differ).
fn action_search_palette(
    ctx: &Context<HyprmuxApp>,
    entries: Vec<SearchEntry<Action>>,
    placeholder: &str,
) -> SearchPalette<Action> {
    let theme = &ctx.state.theme;
    let selection_style = Style::new()
        .fg(theme.surface.backdrop)
        .bg(theme.border_active)
        .bold();
    let input_style = theme.primary.patch(Style::new().bg(theme.surface.element));

    SearchPalette::<Action>::new()
        .entries(entries)
        .placeholder(placeholder)
        .height(Length::Auto)
        .input_border(false)
        .input_prefix("")
        .input_style(input_style)
        .input_focus_style(
            Style::new()
                .fg(theme.border_active)
                .bg(theme.surface.element),
        )
        .input_placeholder_style(theme.muted)
        .list_border(false)
        .list_scrollbar(true)
        .list_selection_full_width(true)
        .list_selection_symbol("")
        .list_unselected_symbol("")
        .list_selection_style(selection_style)
        .list_item_hover_style(Style::new().bg(theme.surface.element))
        .list_item_horizontal_padding((0, 1, 0, 1))
        .list_header_horizontal_padding((0, 1, 0, 1))
        .header_style(theme.accent.bold())
        .description_style(theme.muted)
        .match_style(Style::new().fg(theme.border_active).bold())
        .preserve_groups(true)
        .on_activate(
            ctx.link()
                .callback(|event: SearchEvent<Action>| Msg::RunAction(event.item.value)),
        )
}

/// Shared modal chrome for every overlay: a rounded border, an accent title, and the
/// surface-element background fill so overlays read as solid panels over the workspace.
fn styled_modal(ctx: &Context<HyprmuxApp>, title: &str, width: u16) -> Modal {
    let theme = &ctx.state.theme;
    Modal::new()
        .title(title.to_string())
        .title_style(theme.accent.bold())
        .width(Length::Px(width))
        .border_style(BorderStyle::Rounded)
        .frame_style(Style::new().bg(theme.surface.element))
}

/// The command palette / theme picker modal: shared chrome, content-sized, no inner padding
/// (the `SearchPalette` manages its own).
fn action_palette_modal(ctx: &Context<HyprmuxApp>, title: &str) -> Modal {
    styled_modal(ctx, title, 60).height(Length::Auto).padding(0)
}

fn theme_picker_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let current = ctx.state.config.theme.preset;
    let applied_builtin = ctx.state.config.theme.path.is_none();

    // Mirror the command palette so theme selection reuses the same fuzzy-search UX.
    let entries: Vec<SearchEntry<Action>> = crate::state::ThemePreset::all()
        .into_iter()
        .map(|preset| {
            let mut entry = SearchEntry::item(preset.label(), Action::SelectTheme(preset));
            if applied_builtin && preset == current {
                entry = entry.description(ItemDescription::new().right("current"));
            }
            entry
        })
        .collect();

    let palette = action_search_palette(ctx, entries, "Search themes…");

    action_palette_modal(ctx, "Choose theme")
        .on_close(ctx.link().callback(|_| Msg::CloseThemePicker))
        .child(palette)
        .key(theme_picker_key())
}

/// Display keys for a binding: the user's configured override if any, else the default text.
fn active_keys(ctx: &Context<HyprmuxApp>, binding: &crate::input::CommandBinding) -> String {
    ctx.state
        .config
        .keymap
        .keys_for(binding.action)
        .unwrap_or_else(|| binding.keys.to_string())
}

/// A theme `Style` reduced to just its foreground, so text paints over the modal fill
/// instead of carrying the role's own background (which would draw a stray colored block).
fn fg_only(style: &Style) -> Style {
    style
        .fg
        .map(|paint| Style::new().fg(paint.color()))
        .unwrap_or_default()
}

fn help_section(title: &str, theme: &Theme, spaced: bool) -> Element {
    // A horizontal rule with the section title on it — the title in the accent color, the
    // line muted — so groups read as clear dividers without competing with the key text.
    let divider = Divider::horizontal()
        .label(
            Text::new(format!(" {} ", title.to_uppercase())).style(fg_only(&theme.accent).bold()),
        )
        .label_alignment(Align::Center)
        .label_padding(1)
        .style(fg_only(&theme.muted))
        .width(Length::Flex(1));
    if spaced {
        VStack::new()
            .child(Text::new("").height(Length::Px(1)))
            .child(divider)
            .into()
    } else {
        divider.into()
    }
}

fn help_row(keys: &str, desc: &str, theme: &Theme) -> Element {
    let (keys_text, keys_style) = if keys.is_empty() {
        ("not set".to_string(), fg_only(&theme.muted))
    } else {
        (
            keys.to_string(),
            Style::new().fg(theme.border_active).bold(),
        )
    };
    HStack::new()
        .gap(2)
        .height(Length::Px(1))
        .justify(SpaceBetween)
        .child(
            Text::new(keys_text)
                .style(keys_style)
                .width(Length::Auto)
                .height(Length::Px(1))
                .overflow(Overflow::Ellipsis),
        )
        .child(
            Text::new(desc.to_string())
                .style(fg_only(&theme.primary))
                .width(Length::Auto)
                .height(Length::Px(1))
                .overflow(Overflow::Ellipsis),
        )
        .into()
}

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
            theme.border_active
        } else {
            theme.surface.menu
        },
    );
    let frame_bg = app.chrome_color(
        ctx,
        pane.id,
        "frame-bg",
        crate::theme_ops::pane_frame_background(theme, focused),
    );
    let frame_style = Style::new().fg(frame_fg).bg(frame_bg);

    let mut window_stack = VStack::new().align(Align::Stretch).style(frame_style);
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

        let mut title = pane.display_title(pane.terminal.title());
        if let Some(subtitle) = pane.subtitle() {
            title.push_str(" — ");
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
            .on_mouse_down(
                ctx.link()
                    .callback(move |_| Msg::FocusPane(id, FrameworkFocus::Request)),
            )
            .child(title_row)
            .into();
        window_stack = window_stack.child(title_bar);
    }

    let mut terminal_widget = Terminal::new()
        .snapshot(pane.terminal.snapshot.clone())
        .style(theme.primary.patch(Style::new().bg(frame_bg)))
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
        .on_mouse_forward(ctx.link().callback(move |bytes| Msg::PaneMouse(id, bytes)));
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

/// Draggable strips in the gaps between adjacent tiled panes. Each strip resizes the split on
/// that boundary. Only dwindle (both axes) and master (the master/stack divider) have
/// adjustable ratios, so other layouts get no strips.
fn tiled_resize_strips(
    ctx: &Context<HyprmuxApp>,
    placements: &[crate::tiling::PanePlacement],
    workspace: &crate::state::Workspace,
) -> Vec<(FloatRect, Element)> {
    use crate::state::{LayoutKind, TILE_GAP};
    let master = matches!(workspace.layout_kind, LayoutKind::Master);
    if !master && !matches!(workspace.layout_kind, LayoutKind::Dwindle) {
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

fn workspace_tabs_element(ctx: &Context<HyprmuxApp>) -> Element {
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

    Tabs::new()
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
        )
        .into()
}

fn session_name(ctx: &Context<HyprmuxApp>) -> Option<String> {
    ctx.state
        .config
        .profile
        .path
        .as_ref()
        .and_then(|path| path.file_stem())
        .map(|stem| stem.to_string_lossy().to_string())
}

fn bar_hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|host| !host.trim().is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|host| host.trim().to_string())
                .filter(|host| !host.is_empty())
        })
        .unwrap_or_else(|| "localhost".to_string())
}

fn substitute_placeholders(ctx: &Context<HyprmuxApp>, literal: &str) -> String {
    let state = &ctx.state;
    literal
        .replace("{host}", &bar_hostname())
        .replace("{workspace}", &(state.active_workspace + 1).to_string())
        .replace(
            "{layout}",
            state.workspaces[state.active_workspace].layout_kind.label(),
        )
        .replace("{session}", &session_name(ctx).unwrap_or_default())
}

fn bar_text(text: impl Into<String>, theme: &Theme) -> Element {
    Text::new(text.into())
        .style(Style::new().fg(theme.surface.menu))
        .height(Length::Px(1))
        .into()
}

fn bar_segment_element(
    ctx: &Context<HyprmuxApp>,
    segment: &crate::state::BarSegment,
) -> Option<Element> {
    use crate::state::BarSegment;
    let theme = &ctx.state.theme;
    match segment {
        BarSegment::Title => Some(
            Text::new(" hyprmux ")
                .style(
                    Style::new()
                        .fg(theme.surface.backdrop)
                        .bg(theme.border_active)
                        .bold(),
                )
                .height(Length::Px(1))
                .into(),
        ),
        BarSegment::Workspaces => Some(workspace_tabs_element(ctx)),
        BarSegment::Session => session_name(ctx).map(|name| bar_text(format!(" {name} "), theme)),
        BarSegment::Clock => {
            let now = chrono::Local::now();
            Some(bar_text(
                format!(" {} ", now.format(&ctx.state.config.bar.clock_format)),
                theme,
            ))
        }
        BarSegment::Layout => Some(bar_text(
            format!(
                " {} ",
                ctx.state.workspaces[ctx.state.active_workspace]
                    .layout_kind
                    .label()
            ),
            theme,
        )),
        BarSegment::Text(literal) => Some(bar_text(substitute_placeholders(ctx, literal), theme)),
    }
}

fn top_bar(ctx: &Context<HyprmuxApp>) -> HStack {
    let state = &ctx.state;
    let theme = &ctx.state.theme;
    let bar = &state.config.bar;

    let mut row = HStack::new()
        .gap(1)
        .height(Length::Px(1))
        .style(Style::new().bg(theme.surface.backdrop));

    for segment in &bar.left {
        if let Some(element) = bar_segment_element(ctx, segment) {
            row = row.child(element);
        }
    }

    // The workspace tabs already flex to fill slack; without them, insert a spacer so the
    // right region lands flush against the trailing edge.
    let has_workspaces = bar
        .left
        .iter()
        .chain(bar.right.iter())
        .any(|segment| matches!(segment, crate::state::BarSegment::Workspaces));
    if !has_workspaces {
        row = row.child(Text::new("").width(Length::Flex(1)).height(Length::Px(1)));
    }

    for segment in &bar.right {
        if let Some(element) = bar_segment_element(ctx, segment) {
            row = row.child(element);
        }
    }

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
    } else if state.mode == crate::state::Mode::Copy {
        row = row.child(
            Text::new(" COPY hjkl v y Esc ")
                .style(
                    Style::new()
                        .fg(theme.surface.backdrop)
                        .bg(theme.status.info)
                        .bold(),
                )
                .height(Length::Px(1)),
        );
    }

    row
}

fn empty_workspace_panel(input: &crate::state::InputConfig, theme: &Theme) -> Element {
    let prefix = input.prefix.to_string();
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

pub fn rename_input_key() -> &'static str {
    "hyprmux-rename-input"
}

pub fn theme_picker_key() -> &'static str {
    "hyprmux-theme-picker"
}

pub fn palette_key() -> &'static str {
    "hyprmux-command-palette"
}

pub fn help_scroll_key() -> &'static str {
    "hyprmux-help-scroll"
}
