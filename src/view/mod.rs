mod bar;
mod keys;
mod overlays;
mod pane;

pub use keys::{
    pane_terminal_key, pane_window_key, profile_picker_key, rename_input_key, save_profile_key,
    search_input_key, theme_picker_key,
};
pub(crate) use pane::pane_element;

use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::geometry::{
    canvas_bounds_from_viewport, clamp_float_rect, clamp_floating_rect, close_rect,
    empty_workspace_rect, viewport_bounds,
};
use crate::layout::{ordered_panes, placement_for, workspace_target_rects_excluding};
use crate::state::TOP_BAR_HEIGHT;

use bar::{empty_workspace_panel, top_bar};
use overlays::{
    help_overlay, palette_overlay, profile_picker_overlay, rename_overlay, save_profile_overlay,
    search_overlay, theme_picker_overlay,
};
use pane::tiled_resize_strips;

pub fn render(app: &HyprmuxApp, ctx: &Context<HyprmuxApp>) -> Element {
    let theme = &ctx.state.theme;
    let viewport = ctx.viewport();
    let viewport_changed = ctx
        .state
        .last_viewport
        .replace(Some(viewport))
        .is_some_and(|previous| previous != viewport);
    let workspace = &ctx.state.workspaces[ctx.state.active_workspace];
    let bounds = canvas_bounds_from_viewport(viewport);
    let root_bounds = viewport_bounds(viewport);
    let moving_tiled = ctx
        .state
        .moving_pane
        .filter(|session| !session.was_floating)
        .map(|session| session.id);
    let placements = workspace_target_rects_excluding(workspace, bounds, moving_tiled);
    let focused_pane = workspace.focused_pane.or(ctx.state.focused_pane);
    // Sampled every frame (even while closed) so the slide transition is seeded at 0.0 and the
    // first open animates up from below.
    let scratch_progress = crate::scratchpad::scratch_progress(app, ctx);
    // Centered modal dialogs dim the workspace behind them the same way the scratchpad does, so
    // the dialog reads as the focused layer. The scrollback search is excluded: it scrolls the
    // panes to reveal matches, so they must stay readable.
    let dialog_open = ctx.state.show_palette
        || ctx.state.show_help
        || ctx.state.show_theme_picker
        || ctx.state.rename.is_some()
        || ctx.state.save_profile_prompt.is_some()
        || ctx.state.show_profile_picker;
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
    let mut fullscreen_layer = Canvas::new().height(Length::Flex(1));
    let mut has_fullscreen_layer = false;

    if workspace.panes.iter().all(|pane| pane.closing) {
        canvas = canvas.child_at(
            empty_workspace_rect(bounds).to_rect(),
            empty_workspace_panel(&ctx.state.config.input, theme),
        );
    }

    for pane in ordered_panes(workspace, focused_pane) {
        let base_rect = placement_for(&placements, pane.id)
            .unwrap_or_else(|| clamp_float_rect(pane.floating_rect, bounds));
        let moving = ctx
            .state
            .moving_pane
            .filter(|session| session.id == pane.id);
        let canvas_target_rect = if pane.closing {
            close_rect(pane.floating_rect)
        } else if let Some(session) = moving
            && !pane.fullscreen
        {
            clamp_floating_rect(session.drag_rect, bounds)
        } else {
            // Spawned panes appear at their tiled slot (and fade in via opacity); only
            // surrounding panes animate to make room.
            base_rect
        };
        let target_rect = if pane.fullscreen && !pane.closing {
            root_bounds
        } else {
            canvas_rect_to_root(canvas_target_rect)
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
        let render_in_fullscreen_layer =
            !pane.closing && (pane.fullscreen || animated_rect.y < f32::from(TOP_BAR_HEIGHT));
        let render_rect = if render_in_fullscreen_layer {
            animated_rect
        } else {
            root_rect_to_canvas(animated_rect)
        };
        let mut element = pane_element(app, ctx, pane, render_rect, focused_pane, &display_number);
        // Dim the workspace panes (opacity blends their text/borders rather than hiding them)
        // while a focused layer is up. instant_transition: `pane_dim` is already smoothed by the
        // underlying progress transitions, so this just applies it without re-easing.
        if pane_dim < 1.0 {
            element = Animated::new(element)
                .opacity(pane_dim)
                .transition(crate::anim::instant_transition())
                .into();
        }
        if render_in_fullscreen_layer {
            has_fullscreen_layer = true;
            fullscreen_layer = fullscreen_layer.child_at(render_rect.to_rect(), element);
        } else {
            canvas = canvas.child_at(render_rect.to_rect(), element);
        }
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

    let app_root = VStack::new()
        .style(theme.primary.patch(Style::new().bg(theme.surface.backdrop)))
        .child(top_bar(ctx).height(Length::Px(TOP_BAR_HEIGHT)))
        .child(canvas);
    let mut root = ZStack::new()
        .style(theme.primary.patch(Style::new().bg(theme.surface.backdrop)))
        .child(app_root);
    if has_fullscreen_layer {
        root = root.child(fullscreen_layer);
    }

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
    if ctx.state.save_profile_prompt.is_some() {
        root = root.child(save_profile_overlay(ctx));
    }
    if ctx.state.show_profile_picker {
        root = root.child(profile_picker_overlay(ctx));
    }

    ThemeProvider::new(ctx.state.theme.clone())
        .child(root)
        .into()
}

pub(crate) fn integrated_scrollbar_config() -> ScrollbarConfig {
    ScrollbarConfig::new()
        .variant(ScrollbarVariant::Integrated)
        .thumb('▐')
}

pub(crate) fn modal_scrollbar_config(theme: &Theme) -> ScrollbarConfig {
    integrated_scrollbar_config()
        .thumb_style(Style::new().fg(theme.border_active))
        .thumb_focus_style(Style::new().fg(theme.border_active))
}

pub(crate) fn shared_search_palette<T: Clone + PartialEq>(
    ctx: &Context<HyprmuxApp>,
    height: Length,
    highlight_matches: bool,
) -> SearchPalette<T> {
    let theme = &ctx.state.theme;
    let selection_style = Style::new()
        .fg(theme.surface.backdrop)
        .bg(theme.border_active)
        .bold()
        .contrast_policy(ContrastPolicy::BlackOrWhite);
    let input_style = theme.primary.patch(Style::new().bg(theme.surface.element));

    let palette = SearchPalette::<T>::new()
        .height(height)
        .input_border(false)
        .input_prefix("")
        .input_padding((0, 1))
        .input_style(input_style)
        .input_focus_style(
            Style::new()
                .fg(theme.border_active)
                .bg(theme.surface.element),
        )
        .input_placeholder_style(fg_only(&theme.muted))
        .list_border(false)
        .list_scrollbar(true)
        .list_scrollbar_config(modal_scrollbar_config(theme))
        .list_selection_full_width(true)
        .list_selection_symbol("")
        .list_unselected_symbol("")
        .list_selection_style(selection_style)
        .list_unfocused_selection_style(selection_style)
        .list_item_hover_style(Style::new().bg(theme.surface.element))
        .list_item_horizontal_padding((0, 1))
        .list_header_horizontal_padding((0, 1))
        .item_style(fg_only(&theme.primary))
        .active_item_style(search_palette_active_item_style())
        .active_description_style(fg_only(&theme.accent))
        .header_style(fg_only(&theme.accent).bold())
        .description_style(fg_only(&theme.muted))
        .empty_text_style(fg_only(&theme.muted));

    if highlight_matches {
        palette.match_style(search_palette_item_match_style(theme))
    } else {
        palette
    }
}

fn search_palette_active_item_style() -> Style {
    Style::new()
        .fg(Color::Yellow)
        .bold()
        .contrast_policy(ContrastPolicy::BlackOrWhite)
}

fn search_palette_item_match_style(theme: &Theme) -> Style {
    Style::new()
        .fg(theme.border_active)
        .bold()
        .contrast_policy(ContrastPolicy::BlackOrWhite)
}

/// Shared modal chrome for every overlay: a rounded border, an accent title, and the
/// surface-element background fill so overlays read as solid panels over the workspace.
pub(crate) fn styled_modal(ctx: &Context<HyprmuxApp>, title: &str, width: u16) -> Modal {
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
pub(crate) fn action_palette_modal(ctx: &Context<HyprmuxApp>, title: &str) -> Modal {
    styled_modal(ctx, title, 60).height(Length::Auto).padding(0)
}

fn canvas_rect_to_root(rect: FloatRect) -> FloatRect {
    FloatRect {
        y: rect.y + f32::from(TOP_BAR_HEIGHT),
        ..rect
    }
}

fn root_rect_to_canvas(rect: FloatRect) -> FloatRect {
    FloatRect {
        y: rect.y - f32::from(TOP_BAR_HEIGHT),
        ..rect
    }
}

/// A theme `Style` reduced to just its foreground, so text paints over the modal fill
/// instead of carrying the role's own background (which would draw a stray colored block).
pub(crate) fn fg_only(style: &Style) -> Style {
    style
        .fg
        .map(|paint| Style::new().fg(paint.color()))
        .unwrap_or_default()
}
