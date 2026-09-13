use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::layout::geometry::{
    clamp_float_rect, clamp_floating_rect, close_rect, close_rect_scaled,
};
use crate::layout::tiling::PanePlacement;
use crate::layout::{ordered_panes, placement_for, workspace_target_rects_excluding_with_visible};
use crate::state::{ChromeSlot, PaneId};

use super::animation;
use super::canvas_rect_to_root;
use super::pane::pane_title_bg;
use super::pane::{
    PaneFrameChrome, PaneKind, PaneMerge, divider_title_element, pane_alert, pane_element,
    pane_frame_chrome, pane_has_tile_above, seam_title_element, tiled_resize_strips,
};

/// Everything that differs between the two workspace layers rozi draws: the attachment's active
/// workspace filling the canvas, and the client-local scratchpad workspace filling the dropdown.
///
/// Both go through [`render_workspace_panes`] so tiling, dividers, merged seams, drag previews,
/// and split-resize strips behave identically in the dropdown; only the box they lay out in, the
/// transition key namespace, and the slide offset change.
pub(crate) struct WorkspaceLayer<'a> {
    pub workspace: &'a crate::state::Workspace,
    /// Canvas-space rect the workspace tiles inside.
    pub bounds: FloatRect,
    /// Local canvas clamp for Scrollable, so follower letterbox overhang can still reveal clipped
    /// columns. `None` when the layer already lays out in local space.
    pub visible_bounds: Option<FloatRect>,
    pub top_gap: f32,
    /// Root-space rect a fullscreen pane in this layer expands to.
    pub fullscreen_bounds: FloatRect,
    /// Canvas origin the layer's stored floating rects are relative to.
    ///
    /// The workspace layer stores floats against the *canonical* canvas, so a follower's
    /// letterbox origin has to be added back to land them inside the centered canvas. The
    /// scratchpad is never letterboxed and stores its floats in plain canvas coordinates, so its
    /// origin is zero - translating those by the dropdown's own `y` would push a closing pane a
    /// dropdown's height down the screen and turn its shrink into a slide off the bottom.
    pub float_origin: (f32, f32),
    /// Distinguishes the two layers' transition keys and pane index badges.
    pub scratch: bool,
    pub viewport_changed: bool,
}

impl WorkspaceLayer<'_> {
    fn pane_rect_key(&self, id: PaneId) -> String {
        if self.scratch {
            format!("rozi-scratch-pane-rect-{id}")
        } else {
            format!("rozi-pane-rect-{id}")
        }
    }

    /// Slide progress for a pane arriving in or leaving this layer.
    fn pane_slide_key(&self, id: PaneId) -> String {
        if self.scratch {
            format!("rozi-scratch-pane-slide-{id}")
        } else {
            format!("rozi-pane-slide-{id}")
        }
    }

    /// The clip window for a sliding or centre-scaled pane. Keyed so the wrapper - and therefore
    /// the terminal subtree under it - survives every frame of the transition.
    fn pane_clip_key(&self, id: PaneId) -> String {
        if self.scratch {
            format!("rozi-scratch-pane-clip-{id}")
        } else {
            format!("rozi-pane-clip-{id}")
        }
    }

    fn pane_scale_key(&self, id: PaneId) -> String {
        if self.scratch {
            format!("rozi-scratch-pane-scale-{id}")
        } else {
            format!("rozi-pane-scale-{id}")
        }
    }

    fn pane_reveal_key(&self, id: PaneId) -> String {
        if self.scratch {
            format!("rozi-scratch-pane-reveal-{id}")
        } else {
            format!("rozi-pane-reveal-{id}")
        }
    }

    fn badge(&self) -> Option<&'static str> {
        self.scratch.then_some("S")
    }
}

/// A pane currently being carried, by this client or by the one holding the control lease.
///
/// Both are drawn the same way - lifted out of the tiling, above the settled tiles, with no merged
/// seams or dividers touching it - so the pane a colleague is moving looks like a pane being moved
/// rather than like a rendering fault.
#[derive(Clone, Copy)]
struct PaneDrag {
    pane_id: PaneId,
    /// Canvas-space rectangle to draw the pane at.
    rect: FloatRect,
    /// Floating panes are carried without leaving their layer, so they never vacate a tile.
    floating: bool,
}

/// Resolve which pane this layer is carrying.
///
/// Local first: if this client is dragging, that gesture is the truth for its own screen, and a
/// relayed copy of it would only be its own position one round trip stale. A remote drag applies to
/// the workspace layer alone - the scratchpad is client-local, so no other client can be dragging
/// inside it - and only while the pane is really in this workspace, because the follower chooses
/// its own active workspace and may not be watching the one the controller is rearranging.
fn layer_drag(
    ctx: &Context<AppRoot>,
    layer: &WorkspaceLayer<'_>,
    here: &impl Fn(PaneId) -> bool,
) -> Option<PaneDrag> {
    if let Some(session) = ctx.state.moving_pane.filter(|session| here(session.id)) {
        return Some(PaneDrag {
            pane_id: session.id,
            rect: session.drag_rect,
            floating: session.was_floating,
        });
    }
    let drag = ctx
        .state
        .current()
        .remote_drag
        .filter(|drag| !layer.scratch && here(drag.pane_id))?;
    // Sent in canonical-canvas fractions, exactly like a floating pane's rect, so a follower whose
    // viewport differs from the controller's lands it inside the same letterboxed canvas.
    let (cols, rows) = ctx
        .state
        .current()
        .shared
        .as_ref()
        .and_then(|shared| shared.canonical_canvas)?;
    let rect = crate::layout::shared::frac_rect_to_float(drag.rect, cols, rows);
    Some(PaneDrag {
        pane_id: drag.pane_id,
        rect: FloatRect {
            x: rect.x + layer.float_origin.0,
            y: rect.y + layer.float_origin.1,
            ..rect
        },
        floating: false,
    })
}

/// Draw one workspace - panes, dividers, seam titles, and split-resize strips - into `canvas`.
///
/// Shared by the workspace layer and the scratchpad so the dropdown is a real tiling workspace
/// rather than a second, thinner implementation of one.
pub(crate) fn render_workspace_panes(
    ctx: &Context<AppRoot>,
    mut canvas: Canvas,
    layer: &WorkspaceLayer<'_>,
) -> Canvas {
    let theme = &ctx.state.theme;
    let workspace = layer.workspace;
    let bounds = layer.bounds;
    let top_offset = ctx.state.content_top_offset();
    let top_gap = layer.top_gap;
    let tile_gap = ctx.state.tile_gap();
    // A drag belongs to the current attachment. Match it against this layer's panes so the
    // dropdown does not exclude a workspace pane, nor the workspace a scratch pane.
    let moving_here = |id: PaneId| workspace.panes.iter().any(|pane| pane.id == id);
    let dragged = layer_drag(ctx, layer, &moving_here);
    let moving_tiled = dragged
        .filter(|drag| !drag.floating)
        .map(|drag| drag.pane_id);
    let placements = workspace_target_rects_excluding_with_visible(
        workspace,
        bounds,
        layer.visible_bounds,
        moving_tiled,
        top_gap,
        tile_gap,
    );
    let focused_pane = workspace.focused_pane.or_else(|| {
        (!layer.scratch)
            .then(|| ctx.state.current().focused_pane)
            .flatten()
    });
    // Tiles whose rects are still animating (and the dragged tile) are lifted above merged seams
    // or stable Divider widgets. They therefore occlude chrome they sweep across instead of
    // merging with it or having a divider painted over their terminal content. Each vec keeps
    // `ordered_panes` relative order while the focused pane stays last; divider title ids are kept
    // separately because a reveal must keep its title inside the effected pane frame.
    let merge_layering = ctx.state.config.pane.border_mode.merges_frames();
    let divider_mode = ctx.state.config.pane.border_mode.draws_dividers();
    let mut divider_panes = Vec::new();
    let mut divider_title_panes = Vec::new();
    let mut divider_alerts = Vec::new();
    let mut seam_titles: Vec<(FloatRect, Element)> = Vec::new();
    let mut animating_tiles: Vec<(FloatRect, Element)> = Vec::new();
    let mut dragged_tiles: Vec<(FloatRect, Element)> = Vec::new();
    let mut floating_panes: Vec<(FloatRect, Element)> = Vec::new();
    let mut fullscreen_panes: Vec<(FloatRect, Element)> = Vec::new();
    for pane in ordered_panes(workspace, focused_pane, |pane| {
        pane_alert(pane, focused_pane == Some(pane.id), &ctx.state.config.pane).is_some()
    }) {
        // Floating geometry is stored in canvas-origin coordinates; translate it by the (possibly
        // negative) letterbox origin so a follower's floats sit inside the centered canvas. This is
        // a no-op for the controller and local sessions, where `bounds` starts at the origin.
        let floating_rect = FloatRect {
            x: pane.floating_rect.x + layer.float_origin.0,
            y: pane.floating_rect.y + layer.float_origin.1,
            ..pane.floating_rect
        };
        let base_rect = placement_for(&placements, pane.id)
            .unwrap_or_else(|| clamp_float_rect(floating_rect, bounds));
        let moving = dragged.filter(|drag| drag.pane_id == pane.id);
        // Sliding and paint-effect panes use their real destination for the whole animation. Slide
        // carries the pane in below; Portal and Scan repaint its cells in place.
        let slides = crate::layout::anim::pane_slides(ctx.state.config.animations, pane);
        // Read unconditionally - see the note on `scale_progress` below. Keeping the key evaluated
        // while the pane is settled is what gives a later transition a value to depart from.
        let slide_progress = animation::slide_progress(ctx, pane, layer.pane_slide_key(pane.id));
        let pane_opening = crate::layout::anim::pane_opening_transition(pane);
        let sliding_now = slides && (pane_opening || pane.closing);
        let reveal_effect =
            crate::layout::anim::pane_reveal_effects_for_pane(ctx.state.config.animations, pane);
        let revealing_now = reveal_effect && (pane_opening || pane.closing);
        // Unconditional for the same reason as the two reads around it.
        let reveal_progress =
            animation::pane_reveal_progress(ctx, pane, layer.pane_reveal_key(pane.id));
        let animation_spec =
            crate::layout::anim::pane_animation_for_pane(ctx.state.config.animations, pane);
        // Evaluate the animation key on every frame the pane is drawn, including the frames where
        // `scales` below is false and nothing uses the result. This read is NOT redundant, and
        // gating it on the pane animating is the optimization that breaks it:
        //
        // A keyed transition read for the first time is *created at its target*. The clip only
        // mounts once a close begins - which is also the moment the target becomes 0.0 - so a key
        // created there has no previous value to interpolate from and lands on 0.0 immediately.
        // The pane snaps to its `scale_from` inset and holds it until it is pruned.
        //
        // Keeping the key warm while the pane sits settled leaves it at 1.0, which is the value the
        // close departs from. Mounting the visual effect stays conditional; retaining the
        // interpolation state does not.
        let scale_progress = animation::scale_progress(ctx, pane, layer.pane_scale_key(pane.id));
        // Whether to actually wrap the pane in that clip. Only a lifecycle snapshot gets the
        // fixed-allocation path; bare flags still use the legacy geometry transition that fixtures
        // and older attach paths rely on.
        let scale_transition = pane
            .opening_animation
            .is_some_and(|snapshot| snapshot.active)
            || pane
                .closing_animation
                .is_some_and(|snapshot| snapshot.active);
        let scales = animation_spec.kind == crate::layout::anim::PaneAnimationStyle::Scale
            && scale_transition;
        // Slide carries the pane inside a clip, the paint effects repaint its cells, and Scale
        // clips the settled subtree - so all three keep the terminal grid and titlebar on their
        // final allocation rather than re-laying it out every frame.
        let canvas_target_rect = if sliding_now || revealing_now || scales {
            base_rect
        } else if pane.closing {
            // Preserve the legacy bare-flag close path for un-snapshotted panes.
            close_rect(floating_rect)
        } else if pane.opening {
            // Preserve the legacy bare-flag open path for un-snapshotted panes.
            close_rect(base_rect)
        } else if let Some(drag) = moving
            && !pane.fullscreen
        {
            clamp_floating_rect(drag.rect, bounds)
        } else {
            base_rect
        };
        let target_rect = if pane.fullscreen && !pane.closing {
            layer.fullscreen_bounds
        } else {
            canvas_rect_to_root(canvas_target_rect, top_offset)
        };
        let config =
            animation::transition_config_for(ctx, pane, layer.viewport_changed, target_rect);
        let animated_rect = ctx.transition(layer.pane_rect_key(pane.id), target_rect, config);

        // Scale owns the visible motion in its clip viewport; its pane subtree always receives the
        // settled destination rather than an intermediate geometry transition value.
        let render_rect = if scales { target_rect } else { animated_rect };
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
        // A tile only joins the merged border layer once its rect and paint effect have settled:
        // while either animates it sweeps across settled panes, and Exact-merging every transient
        // overlap would smear junction glyphs along the way.
        // A sliding pane's rectangle never moves, so `rect_settled` calls it settled from the first
        // frame while it is in fact still travelling into place behind its clip. Merged seams and
        // seam titles have to wait for it to arrive, or a pane still off-screen would contribute
        // junction glyphs and park a title over its empty tile.
        let settled = rect_settled(animated_rect, target_rect)
            && slide_progress >= 1.0
            && reveal_progress >= 1.0
            && scale_progress >= 1.0;
        // Dividers always use target layout placements, including panes mid-open or mid-reflow.
        // Animating tiles still paint above the seams, so glyphs only show in emerging gaps
        // instead of vanishing for the whole spawn/close geometry animation. Reveal titles remain
        // in their effected pane until the paint transition settles.
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
            && ctx
                .state
                .config
                .effective_cap_style(ctx.state.config.pane.title_style)
                .glyphs()
                .is_some()
        {
            seam_neighbor_title_bgs(ctx, &placements, pane.id, base_rect, focused_pane)
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
        // Ordinary geometry animations keep their established divider-title timing. Paint effects
        // are different: their title may leave the effect scope only with the fully settled pane.
        let title_on_divider =
            divider_mode && title_row_shared_with_tile_above && (!reveal_effect || settled);
        if title_on_divider && divider_panes.contains(&pane.id) {
            divider_title_panes.push(pane.id);
        }
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
        } else if layer.scratch {
            PaneKind::Scratch
        } else {
            PaneKind::Tiled
        };
        let element = pane_element(
            ctx,
            pane,
            render_rect,
            focused_pane,
            layer.badge(),
            kind,
            merge,
            reveal_progress,
            scales,
        );
        // Everything below places the pane at `render_rect`, so the clip window takes that rect and
        // the pane moves *inside* it. A `Canvas` clips its descendants to its own allocation, which
        // is what makes the pane emerge from behind the seam instead of flying across its neighbour.
        // Mounted for the pane's whole life, not just while it moves, so arriving never remounts the
        // terminal underneath it.
        let element: Element = if slides {
            let (offset_x, offset_y) =
                crate::layout::anim::slide_offset(render_rect, pane.slide_edge, slide_progress);
            Canvas::new()
                // While the pane is only part-way in, the rest of the clip window is empty. Let a
                // click there fall through to the layer beneath instead of being swallowed by a
                // wrapper that exists purely to clip.
                .passthrough(true)
                .child_at(
                    FloatRect {
                        x: offset_x,
                        y: offset_y,
                        w: render_rect.w,
                        h: render_rect.h,
                    }
                    .to_rect(),
                    element,
                )
                .key(layer.pane_clip_key(pane.id))
        } else if scales {
            scale_pane_element(
                element,
                render_rect,
                animation_spec.scale_from,
                scale_progress,
                layer.pane_clip_key(pane.id),
                ScaleOverlay {
                    chrome: pane_frame_chrome(ctx, pane, focused_pane, kind),
                    opacity: crate::layout::anim::pane_opacity_target(
                        ctx.state.config.animations,
                        pane,
                    ),
                    opacity_transition: animation::window_opacity_config(ctx, pane),
                },
            )
        } else {
            element
        };
        let element_rect = if scales {
            scale_clip_rect(render_rect, animation_spec.scale_from, scale_progress)
        } else {
            render_rect
        };
        if title_on_seam && let Some(seam) = seam_title_element(ctx, pane, focused_pane) {
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
        // A pane in transition stays in the canvas layer, *under* the tile taking its space.
        //
        // Lifting it above looks wrong, and the reason is that a terminal cell has no transparency:
        // an effect that "removes" a cell paints a blank over it, so a pane drawn on top covers its
        // whole rectangle with a solid square whether or not the effect still has anything there.
        // Portal's ring ends up inside an opaque box, and Scale's shrinking frame floats over a
        // neighbour that has already claimed the space - both read as artifacts rather than motion.
        //
        // Underneath, the neighbour paints the space it has taken and the leaving pane shows
        // through wherever the neighbour has not reached yet, which is what "it is going away"
        // actually looks like. The two share a clock (see `pane_event_animation`), so the neighbour
        // cannot outrun the effect either.
        let above_settled_tiles = merge_layering || divider_mode;
        if pane.fullscreen {
            fullscreen_panes.push((element_rect, element));
        } else if pane.floating {
            floating_panes.push((element_rect, element));
        } else if above_settled_tiles && moving.is_some() {
            dragged_tiles.push((element_rect, element));
        } else if above_settled_tiles
            && (!settled || (divider_mode && (pane.opening || pane.closing)))
        {
            animating_tiles.push((element_rect, element));
        } else {
            canvas = canvas.child_at(element_rect.to_rect(), element);
        }
    }
    ctx.state.current().remote_drag_snap.set(None);

    if divider_mode {
        let highlight_focused = ctx.state.config.pane.highlight_focused_border;
        let normal_style = Style::new().fg(crate::ops::theme::pane_frame_foreground(
            theme, false, false,
        ));
        let focused_style = if highlight_focused {
            let target = crate::ops::theme::pane_frame_foreground(theme, true, true);
            let fg = focused_pane
                .and_then(|id| crate::pane::lifecycle::find_pane(&ctx.state, id))
                .map_or(Paint::Solid(target), |pane| {
                    animation::chrome_color(ctx, pane, ChromeSlot::DividerFg, target)
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
                        && divider_title_panes.contains(&below)
                        && let Some(pane) = workspace.panes.iter().find(|pane| pane.id == below)
                        && let Some(label) = divider_title_element(ctx, pane, focused_pane)
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
                            crate::state::PaneTitlebarMode::Bar
                            | crate::state::PaneTitlebarMode::Inset => line,
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
    canvas
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

/// Title backgrounds of the tiled neighbors sharing this pane's left and right seam columns, but
/// only when the neighbor's titlebar sits on this pane's top row (so its cap meets ours in the
/// shared cell). A taller pane above the seam shows a border there instead and yields `None`, so
/// the cap falls back to the backdrop. Returns `(left, right)`.
fn seam_neighbor_title_bgs(
    ctx: &Context<AppRoot>,
    placements: &[PanePlacement],
    pane_id: PaneId,
    base_rect: FloatRect,
    focused_pane: Option<PaneId>,
) -> (Option<Paint>, Option<Paint>) {
    let same_top_row = |other: &PanePlacement| (other.rect.y - base_rect.y).abs() < 0.5;
    let color_of = |id: PaneId| {
        crate::pane::lifecycle::find_pane(&ctx.state, id)
            .map(|pane| pane_title_bg(ctx, pane, focused_pane == Some(id)))
    };

    // A neighbor across the left seam has its right border column on our left column; across the
    // right seam, its left column is on our right border column.
    let left = placements
        .iter()
        .find(|other| {
            other.id != pane_id
                && same_top_row(other)
                && (other.rect.x + other.rect.w - 1.0 - base_rect.x).abs() < 0.5
        })
        .and_then(|other| color_of(other.id));
    let right = placements
        .iter()
        .find(|other| {
            other.id != pane_id
                && same_top_row(other)
                && (other.rect.x - (base_rect.x + base_rect.w - 1.0)).abs() < 0.5
        })
        .and_then(|other| color_of(other.id));
    (left, right)
}

/// Clip a pane that is already laid out at `rect` to a centred, animated outer rectangle. PanView's
/// fixed-size child keeps the pane's final allocation while its viewport supplies the moving clip
/// window, so terminal resize callbacks see only the settled geometry.
struct ScaleOverlay {
    chrome: PaneFrameChrome,
    /// Where the pane's own opacity is heading, and how it gets there. The overlay border is a
    /// sibling of the clipped pane rather than a child, so without this it stays fully opaque while
    /// everything inside it fades - a hard bright rectangle around a pane that is otherwise gone.
    opacity: f32,
    opacity_transition: TransitionConfig,
}

fn scale_pane_element(
    pane: Element,
    rect: FloatRect,
    scale_from: f32,
    progress: f32,
    key: String,
    overlay: ScaleOverlay,
) -> Element {
    let visible = scale_clip_rect(rect, scale_from, progress);
    let full = rect.to_rect();
    let viewport = visible.to_rect();
    let pane = Frame::new()
        .border(false)
        .width(Length::Px(full.w))
        .height(Length::Px(full.h))
        .child(pane);
    let clipped = PanView::new()
        .width(Length::Px(viewport.w))
        .height(Length::Px(viewport.h))
        .offset((
            (visible.x - rect.x).round() as i32,
            (visible.y - rect.y).round() as i32,
        ))
        .clamp(false)
        .drag_to_pan(false)
        .wheel_to_pan(false)
        .child(pane);
    let overlay_style = Style {
        bg: None,
        bg_transform: None,
        ..overlay.chrome.frame_style
    };
    let border = Frame::new()
        .width(Length::Px(viewport.w))
        .height(Length::Px(viewport.h))
        .border(overlay.chrome.show_border)
        .border_style(overlay.chrome.border_style)
        .style(overlay_style)
        .child(Text::new(""));
    let border = border
        .key(format!("{key}-border"))
        .min_width(Length::Px(viewport.w))
        .max_width(Length::Px(viewport.w))
        .min_height(Length::Px(viewport.h))
        .max_height(Length::Px(viewport.h));
    let border: Element = Animated::new(border)
        .opacity(overlay.opacity)
        .transition(overlay.opacity_transition)
        .into();
    ZStack::new()
        .passthrough(true)
        .child(clipped)
        .child(border)
        .min_width(Length::Px(viewport.w))
        .max_width(Length::Px(viewport.w))
        .min_height(Length::Px(viewport.h))
        .max_height(Length::Px(viewport.h))
        .key(key)
}

fn scale_clip_rect(rect: FloatRect, scale_from: f32, progress: f32) -> FloatRect {
    let scale = scale_from + (1.0 - scale_from) * progress.clamp(0.0, 1.0);
    close_rect_scaled(rect, scale)
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

    #[test]
    fn scale_clip_grows_from_the_center_without_changing_the_allocated_rect() {
        let settled = FloatRect {
            x: 10.0,
            y: 4.0,
            w: 20.0,
            h: 10.0,
        };
        let start = scale_clip_rect(settled, 0.6, 0.0);
        let middle = scale_clip_rect(settled, 0.6, 0.5);
        let end = scale_clip_rect(settled, 0.6, 1.0);

        assert_eq!(start, close_rect_scaled(settled, 0.6));
        assert_eq!(end, settled);
        assert!(middle.x < start.x && middle.x > end.x);
        assert!(middle.y < start.y && middle.y > end.y);
        assert!(middle.w > start.w && middle.w < end.w);
        assert!(middle.h > start.h && middle.h < end.h);
        assert_eq!(start.x + start.w / 2.0, settled.x + settled.w / 2.0);
        assert_eq!(start.y + start.h / 2.0, settled.y + settled.h / 2.0);
    }
}
