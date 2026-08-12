mod keys;
mod overlays;
mod pane;
pub(crate) mod session_status;
pub(crate) mod sidebar;
mod workbar;

pub use keys::{
    collaboration_key, follow_prompt_key, layout_picker_key, palette_key,
    pane_padding_horizontal_key, pane_padding_vertical_key, pane_terminal_key, pane_window_key,
    profile_picker_key, rename_input_key, rename_session_input_key, save_profile_key,
    search_input_key, session_picker_key, settings_palette_key, sidebar_body_key, theme_picker_key,
};
pub(crate) use pane::{
    PaneKind, PaneMerge, divider_title_element, has_pane_alert, pane_alert, pane_element,
    pane_has_tile_above, seam_title_element,
};
pub(crate) use sidebar::body_focus_key as sidebar_focus_key;
pub(crate) use workbar::{has_inactive_marked_workspace, workspace_marker, workspace_marker_color};

use tui_lipan::prelude::*;

use crate::geometry::{
    clamp_float_rect, clamp_floating_rect, close_rect, empty_workspace_rect, viewport_bounds,
};
use crate::layout::{ordered_panes, placement_for, workspace_target_rects_excluding_with_visible};
use crate::state::{PaneId, WORKBAR_HEIGHT};
use crate::tiling::PanePlacement;
use crate::{AppRoot, Msg};

use pane::pane_title_bg;

use overlays::{
    collaboration_overlay, follow_prompt_overlay, help_overlay, layout_picker_overlay,
    palette_overlay, pane_padding_overlay, profile_picker_overlay, rename_overlay,
    rename_session_overlay, save_profile_overlay, search_overlay, session_picker_overlay,
    settings_overlay, theme_picker_overlay,
};
use pane::tiled_resize_strips;
use workbar::{connecting_workspace_panel, empty_workspace_panel, launcher_panel, workbar};

/// How far a surface lifts under the pointer or the keyboard cursor. One constant across the
/// sidebar and the workbar so the two never drift to different hover weights.
pub(crate) const HOVER_LIFT: f32 = 0.08;

/// The hover lift as a *transform* rather than a color.
///
/// `ColorTransform::Elevate` is the relative form of `Color::elevate_by`, so it lands on the same
/// color as an absolute lift while composing with whatever background the target already carries.
/// That matters wherever an element paints its own background - an alerting workspace tab, a
/// selected row - because an absolute hover color would replace the signal instead of lifting it.
pub(crate) fn hover_lift() -> ColorTransform {
    ColorTransform::Elevate(HOVER_LIFT)
}

pub fn render(app: &AppRoot, ctx: &Context<AppRoot>) -> Element {
    let theme = &ctx.state.theme;
    let viewport = ctx.viewport();
    let content_viewport = ctx.state.content_viewport(viewport);
    ctx.state.last_viewport.set(Some(viewport));
    let viewport_changed = ctx
        .state
        .last_content_viewport
        .replace(Some(content_viewport))
        .is_some_and(|previous| previous != content_viewport);
    let workspace = &ctx.state.current().workspaces[ctx.state.current().active_workspace];
    // A follower renders the controller's canonical pane canvas centered in its own viewport
    // (letterboxed); everyone else uses the full local canvas. Every downstream placement, float,
    // and empty-state rect derives from `bounds`, so centering here centers the whole workspace.
    let bounds = follower_letterbox_bounds(&ctx.state, viewport);
    let local_bounds = ctx.state.canvas_bounds_from_terminal_viewport(viewport);
    let top_offset = ctx.state.content_top_offset();
    let top_gap = ctx.state.workspace_top_gap();
    let tile_gap = ctx.state.tile_gap();
    let root_bounds = viewport_bounds(content_viewport);
    let moving_tiled = ctx
        .state
        .moving_pane
        .filter(|session| !session.was_floating)
        .map(|session| session.id);
    // Scrollable needs the local canvas as the scroll clamp so follower letterbox overhang can
    // still reveal clipped columns; other layouts ignore visible_bounds.
    let placements = workspace_target_rects_excluding_with_visible(
        workspace,
        bounds,
        Some(local_bounds),
        moving_tiled,
        top_gap,
        tile_gap,
    );
    let focused_pane = workspace.focused_pane.or(ctx.state.current().focused_pane);
    // Sampled every frame (even while closed) so the slide transition is seeded at 0.0 and the
    // first open animates up from below.
    let scratch_progress = crate::scratchpad::scratch_progress(app, ctx);
    // Centered modal dialogs dim the workspace behind them the same way the scratchpad does, so
    // the dialog reads as the focused layer. The scrollback search is excluded: it scrolls the
    // panes to reveal matches, so they must stay readable.
    let dialog_open = ctx.state.show_palette
        || ctx.state.show_help
        || ctx.state.show_settings
        || ctx.state.show_theme_picker
        || ctx.state.show_layout_picker
        || ctx.state.rename.is_some()
        || ctx.state.rename_session.is_some()
        || ctx.state.save_profile_prompt.is_some()
        || ctx.state.show_profile_picker
        || ctx.state.show_session_picker
        || ctx.state.collaboration.is_some()
        || ctx.state.follow_prompt.is_some();
    let dialog_dim_progress = ctx.transition::<f32>(
        "rozi-dialog-dim",
        if dialog_open { 1.0 } else { 0.0 },
        app.scratch_transition_config(ctx),
    );
    // The workspace layer dims for whichever focused layer is most deployed; the dims never
    // compound.
    let workspace_dim = crate::scratchpad::backdrop_dim(scratch_progress.max(dialog_dim_progress));
    let mut canvas = Canvas::new()
        .style(Style::new().bg(theme.surface.backdrop))
        .height(Length::Flex(1));
    // Tiles whose rects are still animating (and the dragged tile) are lifted above merged seams
    // or stable Divider widgets. They therefore occlude chrome they sweep across instead of
    // merging with it or having a divider painted over their terminal content. Each vec keeps
    // `ordered_panes` relative order while the focused pane stays last.
    let merge_layering = ctx.state.config.pane.border_mode.merges_frames();
    let divider_mode = ctx.state.config.pane.border_mode.draws_dividers();
    let mut divider_panes = Vec::new();
    let mut divider_alerts = Vec::new();
    let mut seam_titles: Vec<(FloatRect, Element)> = Vec::new();
    let mut animating_tiles: Vec<(FloatRect, Element)> = Vec::new();
    let mut dragged_tiles: Vec<(FloatRect, Element)> = Vec::new();
    let mut floating_panes: Vec<(FloatRect, Element)> = Vec::new();
    let mut fullscreen_panes: Vec<(FloatRect, Element)> = Vec::new();

    if workspace.panes.iter().all(|pane| pane.closing) {
        // Mid-attach with no panes yet: show a live "connecting" spinner rather than the idle
        // "empty workspace" hint, which would read as "done, nothing here" during a connect.
        let connecting = matches!(
            ctx.state.current().connection,
            crate::state::ConnectionState::Connecting | crate::state::ConnectionState::Reconnecting
        ) && ctx.state.current().pending_session_attach.is_some();
        let panel = if connecting {
            connecting_workspace_panel(
                ctx.state.current().remote_host.as_deref(),
                ctx.state.current().connection == crate::state::ConnectionState::Reconnecting,
                theme,
            )
        } else if ctx.state.is_launcher() {
            launcher_panel(ctx, theme)
        } else {
            empty_workspace_panel(&ctx.state.config.input, theme)
        };
        canvas = canvas.child_at(
            canvas_rect_to_root(empty_workspace_rect(bounds), top_offset).to_rect(),
            panel,
        );
    }

    if ctx.state.config.pane.show_workbar {
        let workbar_rect = if ctx.state.config.pane.workbar_at_bottom {
            FloatRect {
                x: 0.0,
                y: f32::from(content_viewport.h.saturating_sub(WORKBAR_HEIGHT)),
                w: f32::from(content_viewport.w),
                h: f32::from(WORKBAR_HEIGHT),
            }
        } else {
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: f32::from(content_viewport.w),
                h: f32::from(WORKBAR_HEIGHT),
            }
        };
        canvas = canvas.child_at(workbar_rect.to_rect(), workbar(app, ctx));
    }

    for pane in ordered_panes(workspace, focused_pane, |pane| {
        pane_alert(pane, focused_pane == Some(pane.id), &ctx.state.config.pane).is_some()
    }) {
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
            // The reverse of the spawn animation: shrink toward the centre of the rectangle the
            // pane held when it was closed, which `close_pane_inner` froze into `floating_rect`
            // before dropping it from the tiling layout.
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
        let animated_rect =
            ctx.transition(format!("rozi-pane-rect-{}", pane.id), target_rect, config);

        let render_rect = animated_rect;
        // With merged borders, a bar title must keep its left edge off a neighbor's right border,
        // or its background would cover the seam. Compact titlebars live in the frame border and
        // do not need this spacer.
        let left_seam = ctx.state.config.pane.show_titles
            && ctx.state.config.pane.titlebar == crate::state::PaneTitlebarMode::Bar
            && tile_gap.horizontal < 0.0
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
        // Dividers always use target layout placements, including panes mid-open or mid-reflow.
        // Animating tiles still paint above the seams, so glyphs only show in emerging gaps
        // instead of vanishing for the whole spawn/close geometry animation.
        if divider_mode && !pane.floating && !pane.fullscreen && !pane.closing && moving.is_none() {
            divider_panes.push(pane.id);
            if let Some((_, color)) =
                pane_alert(pane, focused_pane == Some(pane.id), &ctx.state.config.pane)
            {
                divider_alerts.push((pane.id, color));
            }
        }
        // Capped titles paint their seam cap in the neighbor's title color so a shared cell reads
        // as a split junction; only same-row neighbors (their titlebar on this pane's top row)
        // qualify, so a taller pane above the seam leaves the cap on the plain backdrop.
        let (seam_left_bg, seam_right_bg) = if merge_layering
            && !pane.floating
            && !pane.fullscreen
            && ctx.state.config.pane.show_titles
            && ctx.state.config.pane.titlebar == crate::state::PaneTitlebarMode::Bar
            && ctx.state.config.pane.title_style.glyphs().is_some()
        {
            seam_neighbor_title_bgs(app, ctx, &placements, pane.id, base_rect, focused_pane)
        } else {
            (None, None)
        };
        let merge_enabled =
            merge_layering && !pane.floating && !pane.fullscreen && moving.is_none() && settled;
        // A border/integrated title lives on the pane's top row, and the tile above owns that row
        // in both gapped modes: it is the divider row in dividers mode, and the overlapped seam row
        // in merged mode. Either way the Frame cannot keep the title - it has to be drawn into the
        // row separately, after the neighbor that shares it.
        let title_row_shared_with_tile_above = !pane.floating
            && !pane.fullscreen
            && ctx.state.config.pane.show_titles
            && matches!(
                ctx.state.config.pane.titlebar,
                crate::state::PaneTitlebarMode::Border | crate::state::PaneTitlebarMode::Integrated
            )
            && pane_has_tile_above(&placements, pane.id, tile_gap.vertical);
        let title_on_divider = divider_mode && title_row_shared_with_tile_above;
        // Whichever pane draws later owns the shared seam row, and `ordered_panes` draws the
        // focused pane last - so a title below the focused pane would be painted over by its bottom
        // border. It moves to a strip drawn above every tile instead. Unsettled tiles keep their
        // in-frame header: they already draw above the settled layer, so nothing buries them.
        let title_on_seam = merge_enabled && title_row_shared_with_tile_above;
        let merge = PaneMerge {
            enabled: merge_enabled,
            left_seam,
            seam_left_bg,
            seam_right_bg,
            title_on_divider,
            title_on_seam,
        };
        let kind = if pane.fullscreen {
            PaneKind::Fullscreen
        } else if pane.floating {
            PaneKind::Floating
        } else {
            PaneKind::Tiled
        };
        let element = pane_element(app, ctx, pane, render_rect, focused_pane, None, kind, merge);
        if title_on_seam && let Some(seam) = seam_title_element(app, ctx, pane, focused_pane) {
            seam_titles.push((
                FloatRect {
                    x: render_rect.x + seam.inset,
                    y: render_rect.y,
                    w: (render_rect.w - seam.inset * 2.0).max(0.0),
                    h: 1.0,
                },
                seam.element,
            ));
        }
        if pane.fullscreen {
            fullscreen_panes.push((render_rect, element));
        } else if pane.floating {
            floating_panes.push((render_rect, element));
        } else if (merge_layering || divider_mode) && moving.is_some() {
            dragged_tiles.push((render_rect, element));
        } else if (merge_layering || divider_mode)
            && (!settled || (divider_mode && (pane.opening || pane.closing)))
        {
            animating_tiles.push((render_rect, element));
        } else {
            canvas = canvas.child_at(render_rect.to_rect(), element);
        }
    }

    if divider_mode {
        let highlight_focused = ctx.state.config.pane.highlight_focused_border;
        let normal_style = Style::new().fg(crate::ops::theme::pane_frame_foreground(
            theme, false, false,
        ));
        let focused_style = if highlight_focused {
            let target = crate::ops::theme::pane_frame_foreground(theme, true, true);
            let fg = focused_pane.map_or(Paint::Solid(target), |id| {
                app.chrome_color(ctx, id, "divider-fg", target)
            });
            Style::new().fg(fg)
        } else {
            normal_style
        };
        let title_on_dividers = ctx.state.config.pane.show_titles
            && matches!(
                ctx.state.config.pane.titlebar,
                crate::state::PaneTitlebarMode::Border | crate::state::PaneTitlebarMode::Integrated
            );
        // Quiet seams first, alerting seams next, focused contacts last so junctions inherit the
        // highest-priority visible state. Divider alerts are deliberately static.
        let mut dividers = internal_dividers_for(&placements, &divider_panes);
        dividers.sort_by_key(|divider| {
            (
                divider_alerts
                    .iter()
                    .any(|(id, _)| divider.touches_pane(*id)),
                focused_pane.is_some_and(|id| divider.touches_pane(id)) && highlight_focused,
            )
        });
        for divider in dividers {
            let accent =
                highlight_focused && focused_pane.is_some_and(|id| divider.touches_pane(id));
            let alert = divider_alerts
                .iter()
                .find(|(id, _)| divider.touches_pane(*id))
                .map(|(_, color)| *color);
            let divider_style = if accent {
                focused_style
            } else if let Some(color) = alert {
                Style::new().fg(crate::ops::theme::pane_frame_alert_foreground(theme, color))
            } else {
                normal_style
            };
            let element: Element = match divider.orientation {
                Orientation::Vertical => Divider::vertical().style(divider_style).into(),
                Orientation::Horizontal => {
                    let mut line = Divider::horizontal().style(divider_style);
                    if title_on_dividers
                        && let Some(below) = divider.below
                        && let Some(pane) = workspace.panes.iter().find(|pane| pane.id == below)
                        && let Some(label) = divider_title_element(app, ctx, pane, focused_pane)
                    {
                        line = match ctx.state.config.pane.titlebar {
                            // Embed the title in the line, like a Frame border header: one leading
                            // `─` after any `├` junction, and no trailing gap before the line
                            // continues. Horizontal dividers extend one cell into a vertical for
                            // that junction, so inset past the junction cell when present.
                            crate::state::PaneTitlebarMode::Border => {
                                let pane_left = placements
                                    .iter()
                                    .find(|placement| placement.id == below)
                                    .map(|placement| placement.rect.x);
                                let pad_left =
                                    if pane_left.is_some_and(|x| divider.rect.x < x - 0.5) {
                                        2
                                    } else {
                                        1
                                    };
                                line.label(label)
                                    .label_alignment(Align::Start)
                                    .label_padding_axes(pad_left, 0)
                            }
                            // Fill the whole gap row with the titlebar strip, replacing the line.
                            crate::state::PaneTitlebarMode::Integrated => line
                                .label(label)
                                .label_alignment(Align::Stretch)
                                .label_padding(0),
                            crate::state::PaneTitlebarMode::Bar => line,
                        };
                    }
                    line.into()
                }
            };
            canvas = canvas.child_at(
                canvas_rect_to_root(divider.rect, top_offset).to_rect(),
                element,
            );
        }
    }

    // Above every settled tile, so the neighbor sharing their row can never paint its bottom border
    // over them - but below the moving tiles, which sweep over the settled layer as one piece and
    // must not be crossed by a stray row of someone else's chrome.
    for (rect, element) in seam_titles {
        canvas = canvas.child_at(rect.to_rect(), element);
    }
    for (rect, element) in animating_tiles.into_iter().chain(dragged_tiles) {
        canvas = canvas.child_at(rect.to_rect(), element);
    }
    // Draggable strips sit above every tiled pane but below floating/fullscreen panes, so a
    // floating pane occludes split handles underneath it instead of passing drag events through.
    for (rect, element) in tiled_resize_strips(ctx, &placements, workspace) {
        canvas = canvas.child_at(canvas_rect_to_root(rect, top_offset).to_rect(), element);
    }
    for (rect, element) in floating_panes {
        canvas = canvas.child_at(rect.to_rect(), element);
    }
    for (rect, element) in fullscreen_panes {
        canvas = canvas.child_at(rect.to_rect(), element);
    }

    // The whole workspace layer (workbar, tiled/floating panes, fullscreen panes) dims as one
    // unit while a focused layer (the scratchpad or a modal dialog) is up; opacity blends its
    // text and borders toward the backdrop rather than hiding them. instant_transition: the
    // dim is already smoothed by the underlying progress transitions, so this just applies it
    // without re-easing.
    // Keep the dimming wrapper mounted so the keyed workspace Canvas remains under the same
    // parent while panes are removed and retained for their automatic exit animation.
    let workspace_host_key = format!(
        "rozi-workspace-canvas-{}-{}-{}",
        ctx.state.runtime_epoch,
        ctx.state.current().active_workspace,
        ctx.state.pane_canvas_epoch
    );
    let workspace_layer: Element = Animated::new(canvas.key(workspace_host_key))
        .height(Length::Flex(1))
        .opacity(workspace_dim)
        .opacity_target(theme.surface.backdrop)
        .transition(crate::anim::instant_transition())
        .into();
    let mut root = ZStack::new()
        // The always-mounted popup host is intentionally empty while no popup is open. Let an
        // empty host miss fall through to the workspace instead of making it a pointer shield.
        .passthrough(!ctx.state.popup_is_present())
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

    {
        let mut popup_canvas = Canvas::new().height(Length::Flex(1)).passthrough(true);
        if let Some((rect, element)) = crate::popup::backdrop(ctx) {
            popup_canvas =
                popup_canvas.child_at(canvas_rect_to_root(rect, top_offset).to_rect(), element);
        }
        if let Some((rect, element)) = crate::popup::placement(app, ctx) {
            popup_canvas =
                popup_canvas.child_at(canvas_rect_to_root(rect, top_offset).to_rect(), element);
        }
        let popup_host: Element =
            popup_canvas.key(format!("rozi-popup-host-{}", ctx.state.runtime_epoch));
        root = root.child(popup_host);
    }

    // Overlays portal to the root regardless of where they are attached.
    if ctx.state.show_palette {
        root = root.child(palette_overlay(ctx));
    }
    if ctx.state.show_settings {
        root = root.child(settings_overlay(app, ctx));
    }
    if ctx.state.show_settings && ctx.state.pane_padding_editor.is_some() {
        root = root.child(pane_padding_overlay(ctx));
    }
    if ctx.state.show_help {
        root = root.child(help_overlay(ctx));
    }
    if ctx.state.show_theme_picker {
        root = root.child(theme_picker_overlay(ctx));
    }
    if ctx.state.show_layout_picker {
        root = root.child(layout_picker_overlay(ctx));
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
    if ctx.state.collaboration.is_some() {
        root = root.child(collaboration_overlay(ctx));
    }
    if ctx.state.follow_prompt.is_some() {
        root = root.child(follow_prompt_overlay(ctx));
    }

    let content: Element = root.into();
    let sidebar_width = ctx.state.effective_sidebar_width(viewport);
    let shell: Element = if sidebar_width == 0 {
        content
    } else {
        // A modal dialog covers the whole shell, so the sidebar dims with it exactly like the
        // workspace layer - otherwise a bright column stays beside the dialog. The wrapper is
        // always mounted so the sidebar's keyed splitter/tab state keeps the same parent as the
        // dim animates. The scratchpad drops out of this dim: it is a workspace-local layer that
        // never covers the sidebar.
        let sidebar_dim = crate::scratchpad::backdrop_dim(dialog_dim_progress);
        let sidebar: Element = Animated::new(sidebar::sidebar(ctx))
            .height(Length::Flex(1))
            .opacity(sidebar_dim)
            .opacity_target(theme.surface.backdrop)
            .transition(crate::anim::instant_transition())
            .into();
        let sidebar_pane_width = sidebar_width.saturating_sub(1);
        let content_width = viewport.w.saturating_sub(sidebar_width);
        // The splitter paints its own handle column outside both children, so it has to be dimmed
        // by hand to keep the seam from staying lit between two dimmed panes.
        let divider_bg = theme
            .surface
            .element
            .blend_toward(theme.surface.backdrop, 1.0 - sidebar_dim);
        let divider_style = Style::new().fg(divider_bg.elevate_by(0.15)).bg(divider_bg);
        let mut splitter = Splitter::vertical()
            .split_id("rozi-sidebar-shell")
            .weights_nonce(ctx.state.sidebar.outer_splitter_nonce(
                viewport.w,
                sidebar_width,
                ctx.state.config.sidebar.position == crate::config::SidebarPosition::Right,
            ))
            .min_size(1)
            .handle_symbol('│')
            .handle_style(divider_style)
            .handle_hover_style(divider_style)
            .handle_active_style(
                Style::new()
                    .fg(ctx.state.theme.border_active)
                    .bg(divider_bg)
                    .bold(),
            )
            .on_resize_live(ctx.link().callback(Msg::SidebarWidthResizing))
            .on_resize(ctx.link().callback(Msg::SidebarWidthResized));
        splitter = match ctx.state.config.sidebar.position {
            crate::config::SidebarPosition::Left => splitter
                .weights(vec![sidebar_pane_width as f32, content_width as f32])
                .child(sidebar)
                .child(content),
            crate::config::SidebarPosition::Right => splitter
                .weights(vec![content_width as f32, sidebar_pane_width as f32])
                .child(content)
                .child(sidebar),
        };
        splitter.into()
    };

    ThemeProvider::new(ctx.state.theme.clone())
        .child(shell)
        .into()
}

#[derive(Clone, Debug, PartialEq)]
struct InternalDivider {
    orientation: Orientation,
    rect: FloatRect,
    /// Pane above a horizontal divider. Vertical dividers leave it `None`.
    above: Option<PaneId>,
    /// Pane below a horizontal divider. Border/integrated titles in dividers mode are drawn into
    /// this segment; vertical dividers leave it `None`.
    below: Option<PaneId>,
    /// Pane left of a vertical divider. Horizontal dividers leave it `None`.
    left: Option<PaneId>,
    /// Pane right of a vertical divider. Horizontal dividers leave it `None`.
    right: Option<PaneId>,
}

impl InternalDivider {
    fn touches_pane(&self, pane: PaneId) -> bool {
        self.above == Some(pane)
            || self.below == Some(pane)
            || self.left == Some(pane)
            || self.right == Some(pane)
    }
}

/// Derive visible split separators from final pane adjacency. Extending each segment by one cell
/// along its span makes nested split endpoints overlap, allowing tui-lipan to compose junctions.
fn internal_dividers(placements: &[PanePlacement]) -> Vec<InternalDivider> {
    if placements.len() < 2 {
        return Vec::new();
    }

    let min_x = placements
        .iter()
        .map(|placement| placement.rect.x)
        .fold(f32::INFINITY, f32::min);
    let min_y = placements
        .iter()
        .map(|placement| placement.rect.y)
        .fold(f32::INFINITY, f32::min);
    let max_x = placements
        .iter()
        .map(|placement| placement.rect.x + placement.rect.w)
        .fold(f32::NEG_INFINITY, f32::max);
    let max_y = placements
        .iter()
        .map(|placement| placement.rect.y + placement.rect.h)
        .fold(f32::NEG_INFINITY, f32::max);
    let mut dividers = Vec::new();

    for (index, first) in placements.iter().enumerate() {
        for second in &placements[index + 1..] {
            let (left_placement, right_placement) = if first.rect.x <= second.rect.x {
                (first, second)
            } else {
                (second, first)
            };
            let left = left_placement.rect;
            let right = right_placement.rect;
            let overlap_top = left.y.max(right.y);
            let overlap_bottom = (left.y + left.h).min(right.y + right.h);
            if (left.x + left.w + 1.0 - right.x).abs() < 0.5 && overlap_bottom > overlap_top {
                let y = (overlap_top - 1.0).max(min_y);
                let bottom = (overlap_bottom + 1.0).min(max_y);
                let divider = InternalDivider {
                    orientation: Orientation::Vertical,
                    rect: FloatRect {
                        x: left.x + left.w,
                        y,
                        w: 1.0,
                        h: bottom - y,
                    },
                    above: None,
                    below: None,
                    left: Some(left_placement.id),
                    right: Some(right_placement.id),
                };
                push_internal_divider(&mut dividers, divider);
            }

            let (top_placement, bottom_placement) = if first.rect.y <= second.rect.y {
                (first, second)
            } else {
                (second, first)
            };
            let top = top_placement.rect;
            let bottom_rect = bottom_placement.rect;
            let overlap_left = top.x.max(bottom_rect.x);
            let overlap_right = (top.x + top.w).min(bottom_rect.x + bottom_rect.w);
            if (top.y + top.h + 1.0 - bottom_rect.y).abs() < 0.5 && overlap_right > overlap_left {
                let x = (overlap_left - 1.0).max(min_x);
                let right = (overlap_right + 1.0).min(max_x);
                let divider = InternalDivider {
                    orientation: Orientation::Horizontal,
                    rect: FloatRect {
                        x,
                        y: top.y + top.h,
                        w: right - x,
                        h: 1.0,
                    },
                    above: Some(top_placement.id),
                    below: Some(bottom_placement.id),
                    left: None,
                    right: None,
                };
                push_internal_divider(&mut dividers, divider);
            }
        }
    }

    dividers
}

fn internal_dividers_for(
    placements: &[PanePlacement],
    eligible_panes: &[PaneId],
) -> Vec<InternalDivider> {
    let settled: Vec<_> = placements
        .iter()
        .copied()
        .filter(|placement| eligible_panes.contains(&placement.id))
        .collect();
    internal_dividers(&settled)
}

fn push_internal_divider(dividers: &mut Vec<InternalDivider>, mut divider: InternalDivider) {
    loop {
        let merge_index = dividers
            .iter()
            .position(|existing| match divider.orientation {
                Orientation::Vertical => {
                    // Keep per left|right pair so a focused seam can accent only the contact that
                    // touches the focused pane instead of the whole column.
                    existing.orientation == Orientation::Vertical
                        && existing.left == divider.left
                        && existing.right == divider.right
                        && (existing.rect.x - divider.rect.x).abs() < 0.5
                        && existing.rect.y <= divider.rect.y + divider.rect.h
                        && divider.rect.y <= existing.rect.y + existing.rect.h
                }
                Orientation::Horizontal => {
                    // Keep per-lower-pane segments so a border/integrated title can ride the
                    // divider above its own pane without being merged into a neighbor's span.
                    existing.orientation == Orientation::Horizontal
                        && existing.above == divider.above
                        && existing.below == divider.below
                        && (existing.rect.y - divider.rect.y).abs() < 0.5
                        && existing.rect.x <= divider.rect.x + divider.rect.w
                        && divider.rect.x <= existing.rect.x + existing.rect.w
                }
            });
        let Some(index) = merge_index else {
            dividers.push(divider);
            return;
        };
        let existing = dividers.swap_remove(index);
        match divider.orientation {
            Orientation::Vertical => {
                let start = existing.rect.y.min(divider.rect.y);
                let end = (existing.rect.y + existing.rect.h).max(divider.rect.y + divider.rect.h);
                divider.rect.y = start;
                divider.rect.h = end - start;
            }
            Orientation::Horizontal => {
                let start = existing.rect.x.min(divider.rect.x);
                let end = (existing.rect.x + existing.rect.w).max(divider.rect.x + divider.rect.w);
                divider.rect.x = start;
                divider.rect.w = end - start;
            }
        }
    }
}

/// The canvas bounds to render the active workspace into. A follower returns the controller's
/// canonical canvas centered in its own viewport (letterboxed; the origin may be negative when the
/// canonical canvas is larger than the local one, clipping at the viewport edges). The controller
/// and local/unattached sessions return their own full canvas.
pub(crate) fn follower_letterbox_bounds(state: &crate::state::State, viewport: Rect) -> FloatRect {
    let local = state.canvas_bounds_from_terminal_viewport(viewport);
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
    ctx: &Context<AppRoot>,
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
        // Every rozi palette is a type-to-filter picker: the query input owns focus and the
        // input's key interceptor drives list navigation, so a focusable list only adds a second
        // tab stop that focus can get stuck on (and that reopening restores to). It is also
        // invisible - `list_unfocused_selection_style` below deliberately matches
        // `list_selection_style`, so a focused list looks identical to an unfocused one.
        .list_focusable(false)
        .match_mode(SearchMatchMode::Hybrid)
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
        .list_item_hover_style(Style::new().bg(theme.surface.element.elevate_by(0.08)))
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

/// Flatten `(category, items)` buckets with one non-selectable blank row between adjacent groups.
pub(crate) fn search_entries_with_groups<T>(
    groups: impl IntoIterator<Item = (impl Into<std::sync::Arc<str>>, Vec<SearchEntry<T>>)>,
) -> Vec<SearchEntry<T>> {
    let mut entries = Vec::new();
    for (index, (category, items)) in groups.into_iter().enumerate() {
        if index > 0 {
            entries.push(SearchEntry::spacer());
        }
        entries.push(SearchEntry::header(category));
        entries.extend(items);
    }
    entries
}

#[cfg(test)]
mod grouped_search_tests {
    use super::search_entries_with_groups;
    use tui_lipan::prelude::SearchEntry;

    #[test]
    fn grouped_search_entries_have_one_spacer_between_groups() {
        let entries = search_entries_with_groups([
            ("General", vec![SearchEntry::item("Theme", 1)]),
            ("Panes", vec![SearchEntry::item("Border", 2)]),
            ("Alerts", vec![SearchEntry::item("Bell", 3)]),
        ]);

        assert!(matches!(entries[0], SearchEntry::Header(_)));
        assert!(matches!(entries[2], SearchEntry::Spacer));
        assert!(matches!(entries[3], SearchEntry::Header(_)));
        assert!(matches!(entries[5], SearchEntry::Spacer));
        assert!(matches!(entries[6], SearchEntry::Header(_)));
        assert_eq!(
            entries
                .iter()
                .filter(|entry| matches!(entry, SearchEntry::Spacer))
                .count(),
            2
        );
    }
}

fn search_palette_active_item_style() -> Style {
    Style::new()
        .fg(Color::Yellow)
        .bold()
        .contrast_policy(ContrastPolicy::BlackOrWhite)
}

fn search_palette_item_match_style(theme: &Theme) -> Style {
    Style::new().fg(theme.status.info).bold()
}

/// Shared modal chrome for every overlay: a rounded border, an accent title, and the
/// surface-element background fill so overlays read as solid panels over the workspace.
pub(crate) fn styled_modal(ctx: &Context<AppRoot>, title: &str, width: u16) -> Modal {
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
pub(crate) fn action_palette_modal(ctx: &Context<AppRoot>, title: &str) -> Modal {
    action_palette_modal_with_width(ctx, title, 60)
}

pub(crate) fn action_palette_modal_with_width(
    ctx: &Context<AppRoot>,
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
    app: &AppRoot,
    ctx: &Context<AppRoot>,
    placements: &[PanePlacement],
    pane_id: PaneId,
    base_rect: FloatRect,
    focused_pane: Option<PaneId>,
) -> (Option<Paint>, Option<Paint>) {
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

/// A theme `Style` reduced to just its foreground, so text paints over the modal fill
/// instead of carrying the role's own background (which would draw a stray colored block).
pub(crate) fn fg_only(style: &Style) -> Style {
    style
        .fg
        .map(|paint| Style::new().fg(paint.color()))
        .unwrap_or_default()
}

#[cfg(test)]
mod divider_tests {
    use super::*;

    fn placement(id: PaneId, x: f32, y: f32, w: f32, h: f32) -> PanePlacement {
        PanePlacement {
            id,
            rect: FloatRect { x, y, w, h },
        }
    }

    #[test]
    fn nested_split_dividers_overlap_at_their_junction() {
        let dividers = internal_dividers(&[
            placement(1, 0.0, 0.0, 9.0, 21.0),
            placement(2, 10.0, 0.0, 10.0, 9.0),
            placement(3, 10.0, 10.0, 10.0, 11.0),
        ]);
        let horizontal = dividers
            .iter()
            .find(|divider| divider.orientation == Orientation::Horizontal)
            .expect("nested split should have a horizontal divider");
        assert_eq!(horizontal.rect.y, 9.0);
        assert_eq!(horizontal.rect.x, 9.0);
        assert!(dividers.iter().any(|divider| {
            divider.orientation == Orientation::Vertical
                && divider.rect.x == 9.0
                && divider.rect.y <= horizontal.rect.y
                && divider.rect.y + divider.rect.h > horizontal.rect.y
        }));
    }

    #[test]
    fn horizontal_dividers_remember_the_panes_above_and_below() {
        let dividers = internal_dividers(&[
            placement(1, 0.0, 0.0, 20.0, 9.0),
            placement(2, 0.0, 10.0, 9.0, 10.0),
            placement(3, 10.0, 10.0, 10.0, 10.0),
        ]);
        let horizontal: Vec<_> = dividers
            .iter()
            .filter(|divider| divider.orientation == Orientation::Horizontal)
            .collect();
        assert_eq!(
            horizontal.len(),
            2,
            "each lower pane keeps its own titled segment"
        );
        assert!(
            horizontal
                .iter()
                .any(|d| d.above == Some(1) && d.below == Some(2))
        );
        assert!(
            horizontal
                .iter()
                .any(|d| d.above == Some(1) && d.below == Some(3))
        );
    }

    #[test]
    fn vertical_dividers_remember_left_and_right_panes() {
        let dividers = internal_dividers(&[
            placement(1, 0.0, 0.0, 9.0, 20.0),
            placement(2, 10.0, 0.0, 10.0, 9.0),
            placement(3, 10.0, 10.0, 10.0, 10.0),
        ]);
        let vertical: Vec<_> = dividers
            .iter()
            .filter(|divider| divider.orientation == Orientation::Vertical)
            .collect();
        assert!(
            vertical
                .iter()
                .any(|d| d.left == Some(1) && d.right == Some(2)),
            "top-right contact keeps its own vertical segment: {vertical:?}"
        );
        assert!(
            vertical
                .iter()
                .any(|d| d.left == Some(1) && d.right == Some(3)),
            "bottom-right contact keeps its own vertical segment: {vertical:?}"
        );
        assert_eq!(
            vertical.len(),
            2,
            "different neighbor pairs must not merge into one column"
        );
    }

    #[test]
    fn divider_touches_only_its_adjacent_panes() {
        let dividers = internal_dividers(&[
            placement(1, 0.0, 0.0, 9.0, 9.0),
            placement(2, 10.0, 0.0, 10.0, 9.0),
            placement(3, 0.0, 10.0, 9.0, 10.0),
            placement(4, 10.0, 10.0, 10.0, 10.0),
        ]);
        let focus_2 = dividers
            .iter()
            .filter(|divider| divider.touches_pane(2))
            .collect::<Vec<_>>();
        assert!(
            focus_2
                .iter()
                .any(|d| d.orientation == Orientation::Vertical
                    && d.left == Some(1)
                    && d.right == Some(2))
        );
        assert!(
            focus_2
                .iter()
                .any(|d| d.orientation == Orientation::Horizontal
                    && d.above == Some(2)
                    && d.below == Some(4))
        );
        assert!(focus_2.iter().all(|d| !d.touches_pane(3)));
    }

    #[test]
    fn monocle_placements_have_no_internal_dividers() {
        assert!(
            internal_dividers(&[
                placement(1, 0.0, 0.0, 20.0, 10.0),
                placement(2, 0.0, 0.0, 20.0, 10.0),
            ])
            .is_empty()
        );
    }

    #[test]
    fn excluded_pane_does_not_remove_unrelated_dividers() {
        // Closing/dragged panes stay out of the eligible set; their absence must not wipe seams
        // among the remaining tiles.
        let placements = [
            placement(1, 0.0, 0.0, 9.0, 9.0),
            placement(2, 0.0, 10.0, 9.0, 10.0),
            placement(3, 10.0, 0.0, 10.0, 9.0),
            placement(4, 10.0, 10.0, 10.0, 10.0),
        ];
        let dividers = internal_dividers_for(&placements, &[1, 2, 3]);

        assert!(
            dividers
                .iter()
                .any(|divider| divider.orientation == Orientation::Horizontal),
            "the stable left column keeps its row divider"
        );
        assert!(
            dividers
                .iter()
                .any(|divider| divider.orientation == Orientation::Vertical),
            "the stable top row keeps its column divider"
        );
        assert!(dividers.iter().all(|divider| {
            divider.rect.y + divider.rect.h <= 10.0 || divider.rect.x + divider.rect.w <= 10.0
        }));
    }
}
