mod keys;
mod overlays;
mod pane;
mod workbar;

pub use keys::{
    appearance_palette_key, palette_key, pane_padding_horizontal_key, pane_padding_vertical_key,
    pane_terminal_key, pane_window_key, profile_picker_key, rename_input_key,
    rename_session_input_key, save_profile_key, search_input_key, session_picker_key,
    theme_picker_key,
};
pub(crate) use pane::{PaneMerge, pane_element};

use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::geometry::{
    clamp_float_rect, clamp_floating_rect, close_rect, empty_workspace_rect, viewport_bounds,
};
use crate::layout::{ordered_panes, placement_for, workspace_target_rects_excluding};
use crate::state::PaneId;
use crate::tiling::PanePlacement;

use pane::pane_title_bg;

use overlays::{
    appearance_overlay, help_overlay, palette_overlay, pane_padding_overlay,
    profile_picker_overlay, rename_overlay, rename_session_overlay, save_profile_overlay,
    search_overlay, session_picker_overlay, theme_picker_overlay,
};
use pane::tiled_resize_strips;
use workbar::{empty_workspace_panel, workbar};

pub fn render(app: &HyprmuxApp, ctx: &Context<HyprmuxApp>) -> Element {
    let theme = &ctx.state.theme;
    let viewport = ctx.viewport();
    let viewport_changed = ctx
        .state
        .last_viewport
        .replace(Some(viewport))
        .is_some_and(|previous| previous != viewport);
    let workspace = &ctx.state.workspaces[ctx.state.active_workspace];
    // A follower renders the controller's canonical pane canvas centered in its own viewport
    // (letterboxed); everyone else uses the full local canvas. Every downstream placement, float,
    // and empty-state rect derives from `bounds`, so centering here centers the whole workspace.
    let bounds = follower_letterbox_bounds(&ctx.state, viewport);
    let top_offset = ctx.state.content_top_offset();
    let top_gap = ctx.state.workspace_top_gap();
    let tile_gap = ctx.state.tile_gap();
    let root_bounds = viewport_bounds(viewport);
    let moving_tiled = ctx
        .state
        .moving_pane
        .filter(|session| !session.was_floating)
        .map(|session| session.id);
    let placements =
        workspace_target_rects_excluding(workspace, bounds, moving_tiled, top_gap, tile_gap);
    let focused_pane = workspace.focused_pane.or(ctx.state.focused_pane);
    // Sampled every frame (even while closed) so the slide transition is seeded at 0.0 and the
    // first open animates up from below.
    let scratch_progress = crate::scratchpad::scratch_progress(app, ctx);
    // Centered modal dialogs dim the workspace behind them the same way the scratchpad does, so
    // the dialog reads as the focused layer. The scrollback search is excluded: it scrolls the
    // panes to reveal matches, so they must stay readable.
    let dialog_open = ctx.state.show_palette
        || ctx.state.show_help
        || ctx.state.show_appearance
        || ctx.state.show_theme_picker
        || ctx.state.rename.is_some()
        || ctx.state.rename_session.is_some()
        || ctx.state.save_profile_prompt.is_some()
        || ctx.state.show_profile_picker
        || ctx.state.show_session_picker;
    let dialog_dim_progress = ctx.transition::<f32>(
        "hyprmux-dialog-dim",
        if dialog_open { 1.0 } else { 0.0 },
        app.scratch_transition_config(ctx),
    );
    // The workspace layer dims for whichever focused layer is most deployed; the dims never
    // compound.
    let workspace_dim = crate::scratchpad::backdrop_dim(scratch_progress.max(dialog_dim_progress));
    let mut canvas = Canvas::new()
        .style(Style::new().bg(theme.surface.backdrop))
        .height(Length::Flex(1));
    let mut fullscreen_layer = Canvas::new().height(Length::Flex(1));
    let mut has_fullscreen_layer = false;
    // With merged borders, tiles whose rects are still animating (and the dragged tile) are
    // lifted above the settled merged layer: they draw in Replace mode, so on top they cleanly
    // occlude the seams they sweep across instead of settled panes Exact-merging with their
    // transient border positions. Each vec keeps `ordered_panes` relative order (closing tiles
    // stay under the panes expanding into their space; the focused pane stays last).
    let merge_layering = ctx.state.config.pane.merge_borders;
    let mut animating_tiles: Vec<(FloatRect, Element)> = Vec::new();
    let mut dragged_tiles: Vec<(FloatRect, Element)> = Vec::new();
    let mut floating_panes: Vec<(FloatRect, Element)> = Vec::new();

    if workspace.panes.iter().all(|pane| pane.closing) {
        canvas = canvas.child_at(
            empty_workspace_rect(bounds).to_rect(),
            empty_workspace_panel(&ctx.state.config.input, theme),
        );
    }

    for pane in ordered_panes(workspace, focused_pane) {
        // Floating geometry is stored in canvas-origin coordinates; translate it by the (possibly
        // negative) letterbox origin so a follower's floats sit inside the centered canvas. This is
        // a no-op for the controller and local sessions, where `bounds` starts at the origin.
        let floating_rect = FloatRect {
            x: pane.floating_rect.x + bounds.x,
            y: pane.floating_rect.y + bounds.y,
            ..pane.floating_rect
        };
        let base_rect = placement_for(&placements, pane.id)
            .unwrap_or_else(|| clamp_float_rect(floating_rect, bounds));
        let moving = ctx
            .state
            .moving_pane
            .filter(|session| session.id == pane.id);
        let canvas_target_rect = if pane.closing {
            close_rect(floating_rect)
        } else if pane.opening {
            close_rect(base_rect)
        } else if let Some(session) = moving
            && !pane.fullscreen
        {
            clamp_floating_rect(session.drag_rect, bounds)
        } else {
            base_rect
        };
        let target_rect = if pane.fullscreen && !pane.closing {
            root_bounds
        } else {
            canvas_rect_to_root(canvas_target_rect, top_offset)
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
        let render_in_fullscreen_layer = !pane.closing && pane.fullscreen;
        let render_rect = if render_in_fullscreen_layer {
            animated_rect
        } else {
            root_rect_to_canvas(animated_rect, top_offset)
        };
        // With merged borders, a tiled pane whose left column is a neighbor's right border must
        // keep its title row off that column, or the title background would cover the seam.
        let left_seam = tile_gap.horizontal < 0.0
            && !pane.floating
            && !pane.fullscreen
            && placements.iter().any(|other| {
                other.id != pane.id
                    && (other.rect.x + other.rect.w - 1.0 - base_rect.x).abs() < 0.5
                    && other.rect.y <= base_rect.y + 0.5
                    && base_rect.y < other.rect.y + other.rect.h - 0.5
            });
        // A tile only joins the merged border layer once its rect has settled: while its
        // geometry animates it sweeps across settled panes, and Exact-merging every transient
        // overlap would smear junction glyphs along the way.
        let settled = rect_settled(animated_rect, target_rect);
        // Capped titles paint their seam cap in the neighbor's title color so a shared cell reads
        // as a split junction; only same-row neighbors (their titlebar on this pane's top row)
        // qualify, so a taller pane above the seam leaves the cap on the plain backdrop.
        let (seam_left_bg, seam_right_bg) = if merge_layering
            && !pane.floating
            && !pane.fullscreen
            && ctx.state.config.pane.show_titles
            && ctx.state.config.pane.title_style.caps().is_some()
        {
            seam_neighbor_title_bgs(app, ctx, &placements, pane.id, base_rect, focused_pane)
        } else {
            (None, None)
        };
        let merge = PaneMerge {
            enabled: merge_layering
                && !pane.floating
                && !pane.fullscreen
                && moving.is_none()
                && settled,
            left_seam,
            seam_left_bg,
            seam_right_bg,
        };
        let element = pane_element(
            app,
            ctx,
            pane,
            render_rect,
            focused_pane,
            &display_number,
            merge,
        );
        if render_in_fullscreen_layer {
            has_fullscreen_layer = true;
            fullscreen_layer = fullscreen_layer.child_at(render_rect.to_rect(), element);
        } else if pane.floating {
            floating_panes.push((render_rect, element));
        } else if merge_layering && moving.is_some() {
            dragged_tiles.push((render_rect, element));
        } else if merge_layering && !settled {
            animating_tiles.push((render_rect, element));
        } else {
            canvas = canvas.child_at(render_rect.to_rect(), element);
        }
    }

    // Draggable strips sit above tiled panes but below floating/fullscreen panes, so a floating
    // pane occludes split handles underneath it instead of passing drag events through.
    for (rect, element) in tiled_resize_strips(ctx, &placements, workspace) {
        canvas = canvas.child_at(rect.to_rect(), element);
    }

    for (rect, element) in animating_tiles
        .into_iter()
        .chain(dragged_tiles)
        .chain(floating_panes)
    {
        canvas = canvas.child_at(rect.to_rect(), element);
    }

    let mut app_root =
        VStack::new().style(theme.primary.patch(Style::new().bg(theme.surface.backdrop)));
    if ctx.state.config.pane.show_workbar {
        let workbar = workbar(ctx);
        if ctx.state.config.pane.workbar_at_bottom {
            app_root = app_root.child(canvas).child(workbar);
        } else {
            app_root = app_root.child(workbar).child(canvas);
        }
    } else {
        app_root = app_root.child(canvas);
    }

    // The whole workspace layer (workbar, tiled/floating panes, fullscreen panes) dims as one
    // unit while a focused layer (the scratchpad or a modal dialog) is up; opacity blends its
    // text and borders toward the backdrop rather than hiding them. instant_transition: the
    // dim is already smoothed by the underlying progress transitions, so this just applies it
    // without re-easing.
    let mut workspace_stack = ZStack::new().child(app_root);
    if has_fullscreen_layer {
        workspace_stack = workspace_stack.child(fullscreen_layer);
    }
    let mut workspace_layer: Element = workspace_stack.into();
    if workspace_dim < 1.0 {
        workspace_layer = Animated::new(workspace_layer)
            .opacity(workspace_dim)
            .opacity_target(theme.surface.backdrop)
            .transition(crate::anim::instant_transition())
            .into();
    }
    let mut root = ZStack::new()
        .style(theme.primary.patch(Style::new().bg(theme.surface.backdrop)))
        .child(workspace_layer);

    // The scratchpad renders above the dimmed workspace: a transparent catcher swallows clicks
    // meant for the dimmed panes and dismisses the scratchpad when clicked; the dropdown slides
    // up from the bottom. Modal dialogs stack above the scratchpad, so it dims by the dialog
    // progress alone (its own progress dims only the workspace beneath it).
    let scratch_scrim = crate::scratchpad::scratch_backdrop(ctx, scratch_progress);
    let scratch_pane = crate::scratchpad::scratch_placement(app, ctx, scratch_progress);
    // Drawn last so the drag handle sits above the pane's top chrome and captures the resize drag.
    let scratch_resize = crate::scratchpad::scratch_resize_strip(ctx, scratch_progress);
    if scratch_scrim.is_some() || scratch_pane.is_some() {
        let mut scratch_canvas = Canvas::new().height(Length::Flex(1));
        for (rect, element) in scratch_scrim
            .into_iter()
            .chain(scratch_pane)
            .chain(scratch_resize)
        {
            scratch_canvas =
                scratch_canvas.child_at(canvas_rect_to_root(rect, top_offset).to_rect(), element);
        }
        let mut scratch_layer: Element = scratch_canvas.into();
        let scratch_dim = crate::scratchpad::backdrop_dim(dialog_dim_progress);
        if scratch_dim < 1.0 {
            scratch_layer = Animated::new(scratch_layer)
                .opacity(scratch_dim)
                .opacity_target(theme.surface.backdrop)
                .transition(crate::anim::instant_transition())
                .into();
        }
        root = root.child(scratch_layer);
    }

    // Overlays portal to the root regardless of where they are attached.
    if ctx.state.show_palette {
        root = root.child(palette_overlay(ctx));
    }
    if ctx.state.show_appearance {
        root = root.child(appearance_overlay(ctx));
    }
    if ctx.state.show_appearance && ctx.state.pane_padding_editor.is_some() {
        root = root.child(pane_padding_overlay(ctx));
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
    if ctx.state.rename_session.is_some() {
        root = root.child(rename_session_overlay(ctx));
    }
    if ctx.state.save_profile_prompt.is_some() {
        root = root.child(save_profile_overlay(ctx));
    }
    if ctx.state.show_profile_picker {
        root = root.child(profile_picker_overlay(ctx));
    }
    if ctx.state.show_session_picker {
        root = root.child(session_picker_overlay(ctx));
    }

    ThemeProvider::new(ctx.state.theme.clone())
        .child(root)
        .into()
}

/// The canvas bounds to render the active workspace into. A follower returns the controller's
/// canonical canvas centered in its own viewport (letterboxed; the origin may be negative when the
/// canonical canvas is larger than the local one, clipping at the viewport edges). The controller
/// and local/unattached sessions return their own full canvas.
fn follower_letterbox_bounds(state: &crate::state::State, viewport: Rect) -> FloatRect {
    let local = state.canvas_bounds(viewport);
    let Some((cols, rows)) = state.follower_canonical_canvas() else {
        return local;
    };
    let w = f32::from(cols.max(1));
    let h = f32::from(rows.max(1));
    FloatRect {
        x: local.x + (local.w - w) / 2.0,
        y: local.y + (local.h - h) / 2.0,
        w,
        h,
    }
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
        .list_item_hover_style(Style::new().bg(theme.surface.element.elevate(0.08)))
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
/// (the `SearchPalette` manages its own). The modal hugs its content so filtering to a few
/// matches shrinks it, but is capped at 65% of the viewport (the inner list scrolls past that);
/// `reserve_height` keeps the modal's top edge fixed as it shrinks below the cap instead of
/// re-centering, so the palette does not drift while you type.
pub(crate) fn action_palette_modal(ctx: &Context<HyprmuxApp>, title: &str) -> Modal {
    action_palette_modal_with_width(ctx, title, 60)
}

pub(crate) fn action_palette_modal_with_width(
    ctx: &Context<HyprmuxApp>,
    title: &str,
    width: u16,
) -> Modal {
    styled_modal(ctx, title, width)
        .height(Length::Auto)
        .max_height(Length::Percent(65))
        .reserve_height(Length::Percent(65))
        .padding(0)
}

/// Wrap palette content in a borderless, content-sized frame so it hugs the currently-visible
/// rows; the enclosing [`action_palette_modal`] owns the 65% height cap.
pub(crate) fn action_palette_frame(child: impl Into<Element>) -> Element {
    Frame::new()
        .border(false)
        .height(Length::Auto)
        .padding(0)
        .child(child)
        .into()
}

/// Title backgrounds of the tiled neighbors sharing this pane's left and right seam columns, but
/// only when the neighbor's titlebar sits on this pane's top row (so its cap meets ours in the
/// shared cell). A taller pane above the seam shows a border there instead and yields `None`, so
/// the cap falls back to the backdrop. Returns `(left, right)`.
fn seam_neighbor_title_bgs(
    app: &HyprmuxApp,
    ctx: &Context<HyprmuxApp>,
    placements: &[PanePlacement],
    pane_id: PaneId,
    base_rect: FloatRect,
    focused_pane: Option<PaneId>,
) -> (Option<Color>, Option<Color>) {
    let same_top_row = |other: &PanePlacement| (other.rect.y - base_rect.y).abs() < 0.5;
    let color_of = |id: PaneId| pane_title_bg(app, ctx, id, focused_pane == Some(id));
    // A neighbor across the left seam has its right border column on our left column; across the
    // right seam, its left column is on our right border column.
    let left = placements
        .iter()
        .find(|other| {
            other.id != pane_id
                && same_top_row(other)
                && (other.rect.x + other.rect.w - 1.0 - base_rect.x).abs() < 0.5
        })
        .map(|other| color_of(other.id));
    let right = placements
        .iter()
        .find(|other| {
            other.id != pane_id
                && same_top_row(other)
                && (other.rect.x - (base_rect.x + base_rect.w - 1.0)).abs() < 0.5
        })
        .map(|other| color_of(other.id));
    (left, right)
}

/// Whether a pane's animated rect has reached its target. Transitions end by clamping to the
/// target value, so a tight epsilon only has to absorb float noise, not easing asymptotes.
fn rect_settled(animated: FloatRect, target: FloatRect) -> bool {
    let eps = 0.01;
    (animated.x - target.x).abs() < eps
        && (animated.y - target.y).abs() < eps
        && (animated.w - target.w).abs() < eps
        && (animated.h - target.h).abs() < eps
}

fn canvas_rect_to_root(rect: FloatRect, top_chrome: u16) -> FloatRect {
    FloatRect {
        y: rect.y + f32::from(top_chrome),
        ..rect
    }
}

fn root_rect_to_canvas(rect: FloatRect, top_chrome: u16) -> FloatRect {
    FloatRect {
        y: rect.y - f32::from(top_chrome),
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
