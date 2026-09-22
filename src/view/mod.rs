pub(crate) mod agents;
pub(crate) mod animation;
pub(crate) mod exit;
pub(crate) mod keys_display;
mod overlays;
mod pane;
mod pane_reveal;
pub(crate) mod session_status;
pub(crate) mod sidebar;
mod which_key;
mod widget_keys;
mod workbar;
mod workspace;

pub(crate) use pane::{PaneKind, PaneMerge, has_pane_alert, pane_alert, pane_element};
pub(crate) use sidebar::body_focus_key as sidebar_focus_key;
#[cfg(test)]
pub use widget_keys::pane_window_key;
pub use widget_keys::{
    agent_picker_key, askpass_input_key, collaboration_key, dialog_answer_key,
    extension_detail_key, extension_install_input_key, extensions_key, follow_prompt_key,
    help_filter_key, host_form_input_key, keybinding_capture_key, layout_picker_key, palette_key,
    pane_body_key, pane_id_from_window_key, pane_padding_horizontal_key, pane_padding_vertical_key,
    pane_terminal_key, pick_key, pick_prompt_input_key, profile_picker_key, remote_picker_key,
    rename_input_key, rename_session_input_key, save_profile_key, search_input_key,
    session_picker_key, settings_choice_key, settings_palette_key, sidebar_body_key,
    sidebar_region_key, theme_picker_key,
};
pub(crate) use workbar::{has_inactive_marked_workspace, workspace_marker, workspace_marker_color};
pub(crate) use workspace::{WorkspaceLayer, render_workspace_panes};

use tui_lipan::prelude::*;

use crate::layout::geometry::{empty_workspace_rect, viewport_bounds};
use crate::state::WORKBAR_HEIGHT;
use crate::{AppRoot, Msg};

pub(crate) use overlays::{DIALOG_AFFIRM, neighbor_keybinding_id, settings_query_selection};

use overlays::{
    agent_picker_overlay, askpass_overlay, collaboration_overlay, extension_detail_overlay,
    extension_install_progress_overlay, extension_install_prompt_overlay, extensions_overlay,
    follow_prompt_overlay, help_overlay, keybinding_editor_dialog_overlay, layout_picker_overlay,
    palette_overlay, pane_padding_overlay, pick_overlay, pick_prompt_overlay,
    profile_picker_overlay, reconnecting_overlay, remote_picker_overlay, rename_overlay,
    rename_session_overlay, save_profile_overlay, search_overlay, session_picker_overlay,
    settings_choice_overlay, settings_overlay, theme_picker_overlay,
};
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

pub(crate) const STRIP_LIFT: f32 = 0.05;

pub(crate) fn strip_background(theme: &Theme, follow_terminal: bool, host: Color) -> Color {
    if follow_terminal {
        theme.surface.element
    } else {
        host.elevate_by(STRIP_LIFT)
    }
}

fn workspace_empty_panel(ctx: &Context<AppRoot>) -> Element {
    let theme = &ctx.state.theme;
    let connecting = matches!(
        ctx.state.current().connection,
        crate::state::ConnectionState::Connecting | crate::state::ConnectionState::Reconnecting
    ) && ctx.state.current().pending_session_attach.is_some();
    if connecting {
        connecting_workspace_panel(
            ctx.state.current().remote_host.as_deref(),
            ctx.state.current().connection == crate::state::ConnectionState::Reconnecting,
            theme,
        )
    } else if ctx.state.is_launcher() {
        launcher_panel(ctx, theme)
    } else {
        empty_workspace_panel(&ctx.state.config.input, theme)
    }
}

/// The session's own content: workbar and workspace pages, over the backdrop.
fn session_content(
    ctx: &Context<AppRoot>,
    content_viewport: Rect,
    viewport_changed: bool,
) -> Element {
    let theme = &ctx.state.theme;
    let mut canvas = Canvas::new()
        .style(Style::new().bg(theme.surface.backdrop))
        .height(Length::Flex(1));

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
        canvas = canvas.child_at(workbar_rect.to_rect(), workbar(ctx));
    }

    workspace::workspace_pages(ctx, canvas, viewport_changed)
}

pub fn render(ctx: &Context<AppRoot>) -> Element {
    let theme = &ctx.state.theme;
    let viewport = ctx.viewport();
    // Sampled before anything derives geometry, because the columns the sidebar reserves are a
    // function of it and the whole pane layout follows from those. Sampled every frame (even while
    // hidden) so the transition is seeded at the value the config asks for - a sidebar configured
    // visible starts settled rather than sliding in at launch.
    let sidebar_progress = ctx.transition::<f32>(
        "rozi-sidebar-progress",
        if ctx.state.sidebar_visible { 1.0 } else { 0.0 },
        crate::layout::anim::sidebar_transition(ctx.state.config.animations),
    );
    ctx.state.sidebar_slide.set(sidebar_progress);
    let content_viewport = ctx.state.content_viewport(viewport);
    let top_offset = ctx.state.content_top_offset();
    ctx.state.last_viewport.set(Some(viewport));
    let viewport_changed = ctx
        .state
        .last_content_viewport
        .replace(Some(content_viewport))
        .is_some_and(|previous| previous != content_viewport);
    // Dock deployment and overlay visibility are separate: floating-only scratch has no dock to
    // animate, but remains a focused overlay with the same backdrop treatment.
    let scratch_progress = crate::scratchpad::scratch_progress(ctx);
    let scratch_backdrop_progress = crate::scratchpad::backdrop_progress(ctx);
    // Centered modal dialogs dim the workspace behind them the same way the scratchpad does, so
    // the dialog reads as the focused layer. The scrollback search is excluded: it scrolls the
    // panes to reveal matches, so they must stay readable.
    let reconnecting = ctx.state.current().connection
        == crate::state::ConnectionState::Reconnecting
        && ctx
            .state
            .current()
            .pending_session_attach
            .as_ref()
            .is_some_and(|pending| pending.reconnect);
    let offline = ctx.state.current().connection == crate::state::ConnectionState::Unreachable
        && ctx.state.current().session_name.is_some()
        && ctx.state.current().pending_session_attach.is_none();
    let picker_dialog_open = ctx.state.has_modal_overlay();
    // Offline and reconnect chrome yield to another overlay (Sessions, a password prompt) so those
    // stay reachable. Once it closes, the connection overlay returns if it still applies.
    let show_offline = offline && !picker_dialog_open;
    let show_reconnecting = reconnecting && !picker_dialog_open;
    let dialog_open = show_reconnecting || show_offline || picker_dialog_open;
    let dialog_dim_progress = ctx.transition::<f32>(
        "rozi-dialog-dim",
        if dialog_open { 1.0 } else { 0.0 },
        animation::scratch_transition_config(ctx),
    );
    // The workspace layer dims for whichever focused layer is most deployed; the dims never
    // compound.
    let workspace_dim =
        crate::scratchpad::backdrop_dim(scratch_backdrop_progress.max(dialog_dim_progress));
    // A session switch resolves the workbar and panes in place over the backdrop. The sidebar is
    // composed outside this layer and stays put: it navigates between sessions, so it is the
    // anchor while the thing it navigates changes. Sampled before the workbar, panes, and sidebar
    // are built: it also decides whether their focus chrome snaps on this frame.
    let reveal = animation::session_reveal(ctx);
    let workspace_opacity = workspace_dim * reveal.opacity;
    // Keyed by attachment so a switch replaces the whole layer, and the outgoing one is retained
    // frozen beneath its successor (see `animation::session_layer_exit`). A fresh attach still in
    // its grace period draws nothing, so the previous session's last picture stands in for it.
    let holding = crate::ops::session::holding_previous_view(&ctx.state);
    let animations = ctx.state.config.animations;
    let exit = animation::session_layer_exit(animations);
    let empty = || -> Element { Canvas::new().height(Length::Flex(1)).into() };
    let session_layer: Element = if crate::layout::anim::session_portal_enabled(animations) {
        // The portal composites the session's content, already dimmed for any dialog, over the
        // retained outgoing layer, which carries its own dim and must not be dimmed twice. That
        // takes a dimming layer inside the portal and a retained one around it; only the portal
        // pays for the two levels, since every level is recursion the whole view tree carries.
        let content = if holding {
            empty()
        } else {
            Animated::new(session_content(ctx, content_viewport, viewport_changed))
                .height(Length::Flex(1))
                .opacity(workspace_opacity)
                .opacity_target(theme.surface.backdrop)
                .transition(crate::layout::anim::instant_transition())
                .into()
        };
        Animated::new(pane_reveal::session_portal_scope(
            content,
            reveal.portal,
            pane_reveal::SessionPortalRing::from_theme(theme),
        ))
        .height(Length::Flex(1))
        .transition(crate::layout::anim::instant_transition())
        .auto_exit(exit)
        .into()
    } else {
        let layer = if holding {
            Animated::new(empty())
        } else {
            Animated::new(session_content(ctx, content_viewport, viewport_changed))
                .opacity(workspace_opacity)
                .opacity_target(theme.surface.backdrop)
        };
        layer
            .height(Length::Flex(1))
            .transition(crate::layout::anim::instant_transition())
            .auto_exit(exit)
            .into()
    };
    // Alone in its own stack so a retained layer has no live sibling to be ordered against and
    // lands beneath the live one; in the root stack it would sit above it, over the new session.
    let workspace_layer: Element = ZStack::new()
        .child(session_layer.key(format!("rozi-session-view-{}", ctx.state.runtime_epoch)))
        .key("rozi-session-views");
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
    let scratch_scrim = crate::scratchpad::scratch_backdrop(ctx, scratch_backdrop_progress);
    // Between the dismiss scrim and docked panes: the dock's gaps and divider rows belong to the
    // scratch overlay, not to the scrim that closes it. Floating-only presentation has no shield.
    let scratch_shield = crate::scratchpad::scratch_shield(ctx, scratch_progress);
    // Drawn last so the drag handle sits above the dropdown's top edge and captures the resize drag.
    let scratch_resize = crate::scratchpad::scratch_resize_strip(ctx, scratch_progress);
    // Mounted while the dropdown is retracting (progress still above zero) *and* from the frame
    // the toggle lands on: the slide transition reads ~0.0 on that first frame, so gating on the
    // scrim alone would hold the whole layer back a frame and leave `scratch_panes`' own
    // still-hidden check unreachable.
    let scratch_pane_closing = ctx.state.scratch.panes.iter().any(|pane| pane.closing);
    if scratch_scrim.is_some()
        || (ctx.state.scratch_visible && !ctx.state.scratch.panes.is_empty())
        || scratch_pane_closing
    {
        let mut scratch_canvas = Canvas::new().height(Length::Flex(1));
        for (rect, element) in scratch_scrim.into_iter().chain(scratch_shield) {
            scratch_canvas =
                scratch_canvas.child_at(canvas_rect_to_root(rect, top_offset).to_rect(), element);
        }
        scratch_canvas = crate::scratchpad::scratch_panes(
            ctx,
            scratch_canvas,
            scratch_progress,
            viewport_changed,
        );
        if let Some((rect, element)) = scratch_resize {
            scratch_canvas =
                scratch_canvas.child_at(canvas_rect_to_root(rect, top_offset).to_rect(), element);
        }
        let mut scratch_layer: Element = scratch_canvas.into();
        let scratch_dim = crate::scratchpad::backdrop_dim(dialog_dim_progress);
        if scratch_dim < 1.0 {
            scratch_layer = Animated::new(scratch_layer)
                .opacity(scratch_dim)
                .opacity_target(theme.surface.backdrop)
                .transition(crate::layout::anim::instant_transition())
                .into();
        }
        root = root.child(scratch_layer);
    }

    {
        let mut popup_canvas = Canvas::new().height(Length::Flex(1)).passthrough(true);
        if let Some((rect, element)) = crate::ops::popup::backdrop(ctx) {
            popup_canvas =
                popup_canvas.child_at(canvas_rect_to_root(rect, top_offset).to_rect(), element);
        }
        if let Some((rect, element)) = crate::ops::popup::placement(ctx) {
            popup_canvas =
                popup_canvas.child_at(canvas_rect_to_root(rect, top_offset).to_rect(), element);
        }
        let popup_host: Element =
            popup_canvas.key(format!("rozi-popup-host-{}", ctx.state.runtime_epoch));
        root = root.child(popup_host);
    }

    // Above the panes so it is never covered, below the modal overlays so it never competes with
    // one - and passthrough, because it is chrome that must not eat a click meant for a pane.
    if let Some((rect, element)) = which_key::layer(ctx, content_viewport) {
        root = root.child(
            Canvas::new()
                .height(Length::Flex(1))
                .passthrough(true)
                .child_at(rect, element)
                .key("rozi-which-key"),
        );
    }

    // Overlays portal to the root regardless of where they are attached.
    if ctx.state.show_palette {
        root = root.child(palette_overlay(ctx));
    }
    if ctx.state.show_settings {
        root = root.child(settings_overlay(ctx));
    }
    if ctx.state.show_settings && ctx.state.pane_padding_editor.is_some() {
        root = root.child(pane_padding_overlay(ctx));
    }
    if ctx.state.show_settings && ctx.state.settings_choice.is_some() {
        root = root.child(settings_choice_overlay(ctx));
    }
    // The report replaces the picker rather than stacking on it, like every other nested dialog
    // (see `ops::overlay_return`). Its query lives in state, so the way back finds it unchanged.
    if ctx
        .state
        .extensions
        .as_ref()
        .is_some_and(|state| state.detail.is_none() && state.catalog_detail.is_none())
    {
        root = root.child(extensions_overlay(ctx));
    }
    // A running installation's progress takes the place of the report or prompt that started it.
    let installing = crate::ops::extensions_manager::visible_install(&ctx.state).is_some();
    if !installing
        && ctx
            .state
            .extensions
            .as_ref()
            .is_some_and(|state| state.detail.is_some() || state.catalog_detail.is_some())
    {
        root = root.child(extension_detail_overlay(ctx));
    }
    if !installing
        && ctx
            .state
            .extensions
            .as_ref()
            .and_then(|state| state.install_prompt.as_ref())
            .is_some()
    {
        root = root.child(extension_install_prompt_overlay(ctx));
    }
    if installing {
        root = root.child(extension_install_progress_overlay(ctx));
    }
    if let Some(keybindings) = ctx.state.keybindings.as_ref() {
        root = root.child(help_overlay(ctx, keybindings));
    }
    if ctx
        .state
        .keybindings
        .as_ref()
        .is_some_and(|keybindings| keybindings.stage != crate::state::KeybindingEditorStage::List)
    {
        root = root.child(keybinding_editor_dialog_overlay(ctx));
    }
    if ctx.state.show_theme_picker {
        root = root.child(theme_picker_overlay(ctx));
    }
    if ctx.state.show_layout_picker {
        root = root.child(layout_picker_overlay(ctx));
    }
    // The pick list is not drawn while a stacked prompt is up; cancelling restores it from
    // `restore_query`.
    if ctx.state.show_pick
        && ctx
            .state
            .pick
            .as_ref()
            .is_none_or(|pick| pick.prompt.is_none())
    {
        root = root.child(pick_overlay(ctx));
    }
    // Above the picker, which stays mounted underneath so cancelling returns to its list.
    if ctx
        .state
        .pick
        .as_ref()
        .is_some_and(|pick| pick.prompt.is_some())
    {
        root = root.child(pick_prompt_overlay(ctx));
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
    if ctx.state.remote_picker.is_some() {
        root = root.child(remote_picker_overlay(ctx));
    }
    if ctx.state.agent_picker.is_some() {
        root = root.child(agent_picker_overlay(ctx));
    }
    if ctx.state.collaboration.is_some() {
        root = root.child(collaboration_overlay(ctx));
    }
    if ctx.state.follow_prompt.is_some() {
        root = root.child(follow_prompt_overlay(ctx));
    }
    if show_reconnecting || show_offline {
        root = root.child(reconnecting_overlay(ctx));
    }
    // Last, so its own backdrop fades every dialog already on screen: an ssh prompt arrives on
    // ssh's schedule, over whatever the user had open, and returns them to it.
    if ctx.state.askpass.is_some() {
        root = root.child(askpass_overlay(ctx));
    }

    let content: Element = root.into();
    // Grows and shrinks with the slide, so the pane column resizes to make room rather than being
    // pushed off the far edge of the screen.
    let sidebar_width = ctx.state.effective_sidebar_width(viewport);
    // Nothing reserved, nothing to wrap - which covers both a hidden sidebar and either end of its
    // slide, where there is not yet enough width to put a panel in.
    let shell: Element = if sidebar_width == 0 {
        content
    } else {
        // A modal dialog covers the whole shell, so the sidebar dims with it exactly like the
        // workspace layer - otherwise a bright column stays beside the dialog. The wrapper is
        // always mounted so the sidebar's keyed splitter/tab state keeps the same parent as the
        // dim animates. The scratchpad drops out of this dim: it is a workspace-local layer that
        // never covers the sidebar.
        let sidebar_dim = crate::scratchpad::backdrop_dim(dialog_dim_progress);
        // The splitter spends one column on its own handle, so both the panel's settled width and
        // the window currently clipping it are one short of their reservations.
        let panel_width = ctx.state.sidebar_slide_width(viewport).saturating_sub(1);
        let sidebar_pane_width = sidebar_width.saturating_sub(1);
        let docked_right =
            ctx.state.config.sidebar.position == crate::config::SidebarPosition::Right;
        // The panel is laid out at its settled width and rides inside a clip window that grows with
        // the slide, anchored to the dock edge - so it arrives whole, sliding in, rather than being
        // re-laid-out narrower on every frame. Only the pane column beside it actually resizes.
        let sidebar: Element = Canvas::new()
            .child_at(
                FloatRect {
                    x: crate::layout::anim::sidebar_slide_offset(
                        sidebar_pane_width,
                        panel_width,
                        docked_right,
                    ),
                    y: 0.0,
                    w: f32::from(panel_width),
                    h: f32::from(viewport.h),
                }
                .to_rect(),
                sidebar::sidebar(ctx, panel_width),
            )
            .key("rozi-sidebar-clip");
        // The dim wraps the clip rather than the panel, so it applies to the sidebar column as it is
        // seen and leaves the panel's own allocation at its settled width.
        let sidebar: Element = Animated::new(sidebar)
            .height(Length::Flex(1))
            .opacity(sidebar_dim)
            .opacity_target(theme.surface.backdrop)
            .transition(crate::layout::anim::instant_transition())
            .into();
        // Whatever the sidebar has not reserved. It shrinks as the panel arrives, which is what
        // keeps the pane column's far edge pinned to the far edge of the screen while its near edge
        // travels: the column gives up the space rather than sliding out of it.
        let content_width = viewport.w.saturating_sub(sidebar_width);
        // The splitter paints its own handle column outside both children, so it has to be dimmed
        // by hand to keep the seam from staying lit between two dimmed panes.
        let divider_bg =
            sidebar::fill_color(theme, ctx.state.config.sidebar.background_follows_canvas)
                .blend_toward(theme.surface.backdrop, 1.0 - sidebar_dim);
        let divider_style = Style::new().fg(divider_bg.elevate_by(0.15)).bg(divider_bg);
        // The same window `set_width` clamps to, handed to the splitter so the drag stops there
        // too. Without it the handle follows the pointer past the widest sidebar rozi will draw,
        // and the columns between the panel and the pane column belong to nobody: an empty strip
        // beside the sidebar, with the pane column clipped because it was laid out for the width
        // it was denied.
        let (min_pane, max_pane) = ctx.state.sidebar_pane_bounds(viewport);
        let sidebar_limits = SplitterPaneLimits::range(min_pane, max_pane);
        let pane_limits = if docked_right {
            vec![SplitterPaneLimits::UNBOUNDED, sidebar_limits]
        } else {
            vec![sidebar_limits, SplitterPaneLimits::UNBOUNDED]
        };
        let mut splitter = Splitter::vertical()
            .pane_limits(pane_limits)
            .split_id("rozi-sidebar-shell")
            .weights_nonce(ctx.state.sidebar.outer_splitter_nonce(
                viewport.w,
                sidebar_width,
                docked_right,
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
        splitter = if docked_right {
            splitter
                .weights(vec![content_width as f32, sidebar_pane_width as f32])
                .child(content)
                .child(sidebar)
        } else {
            splitter
                .weights(vec![sidebar_pane_width as f32, content_width as f32])
                .child(sidebar)
                .child(content)
        };
        splitter.into()
    };

    ThemeProvider::new(ctx.state.theme.clone())
        .child(shell)
        .into()
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
        // Terminal geometry has to share one whole-cell origin. An odd size difference produces a
        // half-cell mathematical centre; leaving that fraction here makes each pane's independent
        // `FloatRect::to_rect` round on a different side of zero. A split crossing the viewport
        // edge can then lose its border, while another split gains an extra gap cell.
        x: local.x + ((local.w - w) / 2.0).round(),
        y: local.y + ((local.h - h) / 2.0).round(),
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

pub(crate) fn picker_selection_cap_glyphs(
    config: &crate::config::Config,
) -> (&'static str, &'static str) {
    config
        .effective_cap_style(config.pane.picker_selection_style)
        .glyphs()
        .unwrap_or(("", ""))
}

pub(crate) fn picker_selection_cap_style(theme: &Theme, fill: Color) -> Style {
    Style::new().fg(fill).bg(theme.surface.element)
}

pub(crate) fn shared_search_palette<T: Clone + PartialEq>(
    ctx: &Context<AppRoot>,
    height: Length,
    highlight_matches: bool,
) -> SearchPalette<T> {
    let theme = &ctx.state.theme;
    let selection_fill = theme.border_active;
    let selection_style = Style::new()
        .fg(theme.surface.backdrop)
        .bg(selection_fill)
        .bold()
        .contrast_policy(ContrastPolicy::BlackOrWhite);
    let (selection_left, selection_right) = picker_selection_cap_glyphs(&ctx.state.config);
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
        .input_divider_style(fg_only(&theme.border))
        .input_divider_join_frame(false)
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
        .list_selection_symbol(selection_left)
        .list_selection_symbol_right(selection_right)
        .list_selection_symbol_style(picker_selection_cap_style(theme, selection_fill))
        .list_unselected_symbol("")
        .list_selection_style(selection_style)
        .list_unfocused_selection_style(selection_style)
        .list_item_hover_style(Style::new().bg(theme.surface.element.elevate_by(0.08)))
        .list_item_horizontal_padding((0, 1))
        .list_header_horizontal_padding((0, 1))
        .item_style(fg_only(&theme.primary))
        .active_item_style(search_palette_active_item_style())
        .active_description_style(fg_only(&theme.accent))
        .header_style(rozi_fg(theme).bold())
        .description_style(fg_only(&theme.muted))
        .description_placement(DescriptionPlacement::Right)
        .primary_truncate_description_first(true)
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

    #[test]
    fn reconnecting_with_retained_panes_shows_progress_modal() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
                backend.set_viewport(tui_lipan::prelude::Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30,
                });
                let state = backend.state_mut();
                state.current_mut().session_name = Some("dev".into());
                state.current_mut().connection = crate::state::ConnectionState::Reconnecting;
                state.current_mut().pending_session_attach =
                    Some(crate::state::PendingSessionAttach {
                        epoch: state.runtime_epoch,
                        name: "dev".into(),
                        client: None,
                        autostart: false,
                        read_only: false,
                        reconnect: true,
                        remote_host: None,
                        intent: crate::state::AttachIntent::Plain,
                        left: None,
                        parked_epoch: None,
                    });
                backend.render();

                let frame = backend.capture_frame().to_fixed_grid();
                assert!(frame.contains("Session · dev"));
                assert!(frame.contains("reconnecting"));
                assert!(
                    frame.contains("sessions"),
                    "reconnecting overlay must advertise Esc → Sessions, got:\n{frame}"
                );
            })
            .expect("spawn reconnect modal test")
            .join()
            .expect("reconnect modal test completes");
    }

    #[test]
    fn ssh_askpass_covers_the_reconnecting_overlay() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
                backend.set_viewport(tui_lipan::prelude::Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30,
                });
                let state = backend.state_mut();
                state.current_mut().session_name = Some("dev".into());
                state.current_mut().connection = crate::state::ConnectionState::Reconnecting;
                state.current_mut().pending_session_attach =
                    Some(crate::state::PendingSessionAttach {
                        epoch: state.runtime_epoch,
                        name: "dev".into(),
                        client: None,
                        autostart: false,
                        read_only: false,
                        reconnect: true,
                        remote_host: Some("workbox".into()),
                        intent: crate::state::AttachIntent::Plain,
                        left: None,
                        parked_epoch: None,
                    });
                state.askpass = Some(crate::state::AskpassState::new(
                    crate::state::AskpassPrompt {
                        id: 1,
                        session: "reconnect".into(),
                        attach_epoch: Some(state.runtime_epoch),
                        kind: crate::session::remote::AskpassKind::Secret,
                        prompt: "user@workbox's password:".into(),
                        error: None,
                    },
                ));
                backend.render();

                let frame = backend.capture_frame().to_fixed_grid();
                assert!(
                    frame.contains("SSH · user@workbox's password"),
                    "askpass must be visible above reconnect chrome, got:\n{frame}"
                );
                assert!(
                    !frame.contains("reconnecting"),
                    "reconnect overlay must yield to askpass, got:\n{frame}"
                );
            })
            .expect("spawn askpass-over-reconnect test")
            .join()
            .expect("askpass-over-reconnect test completes");
    }

    #[test]
    fn unreachable_remote_session_shows_offline_modal() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
                backend.set_viewport(tui_lipan::prelude::Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30,
                });
                let state = backend.state_mut();
                state.show_session_picker = false;
                state.session_picker = None;
                state.current_mut().session_name = Some("dev".into());
                state.current_mut().remote_host = Some("workbox".into());
                state.current_mut().pending_session_attach = None;
                state.current_mut().connection = crate::state::ConnectionState::Unreachable;
                backend.render();

                let frame = backend.capture_frame().to_fixed_grid();
                assert!(
                    frame.contains("Session · dev"),
                    "expected offline session modal, got:\n{frame}"
                );
                assert!(
                    frame.contains("offline"),
                    "expected offline token, got:\n{frame}"
                );
                assert!(
                    frame.contains("reconnect"),
                    "expected reconnect hint, got:\n{frame}"
                );
            })
            .expect("spawn offline modal test")
            .join()
            .expect("offline modal test completes");
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

/// Shared modal chrome for every overlay: a configured picker border, a chrome-colored title, and
/// the surface-element background fill so overlays read as solid panels over the workspace.
pub(crate) fn overlay_border_style(ctx: &Context<AppRoot>) -> BorderStyle {
    ctx.state.config.pane.picker_border_style.to_border_style()
}

pub(crate) fn styled_modal(ctx: &Context<AppRoot>, title: &str, width: u16) -> Modal {
    let theme = &ctx.state.theme;
    Modal::new()
        .title(title.to_string())
        .title_style(rozi_chrome(theme).bold())
        .width(Length::Px(width))
        .border_style(overlay_border_style(ctx))
        .frame_style(Style::new().bg(theme.surface.element))
}

/// Shared cap for command-palette-shaped overlays (`OverlayPalette`, theme picker, install
/// prompt). Nested cards pass this to [`nested_action_palette_modal`] so they sit one row below.
pub(crate) const ACTION_PALETTE_MAX_HEIGHT_PERCENT: u16 = 65;

/// The command palette / theme picker modal: shared chrome, content-sized, no inner padding
/// (the `SearchPalette` manages its own). The modal hugs its content so filtering to a few
/// matches shrinks it, but is capped at 65% of the viewport (the inner list scrolls past that);
/// `reserve_height` keeps the modal's top edge fixed as it shrinks below the cap instead of
/// re-centering, so the palette does not drift while you type.
pub(crate) fn action_palette_modal(ctx: &Context<AppRoot>, title: &str) -> Modal {
    action_palette_modal_with_width(ctx, title, 60)
}

/// A card stacked on a list overlay (Change keybinding / Reset all on Keybindings, Terminal
/// padding on Settings, Install extension on Extensions). Portals pin their top at
/// `(viewport - reserve_height) / 2`; two fewer reserved rows than the parent drops that edge
/// by one, so the card sits just below instead of sharing a top.
pub(crate) fn nested_action_palette_modal(
    ctx: &Context<AppRoot>,
    title: &str,
    parent_reserve_percent: u16,
) -> Modal {
    let parent_band = Length::Percent(parent_reserve_percent).resolve(ctx.viewport().h, 0);
    action_palette_modal(ctx, title).reserve_height(Length::Px(parent_band.saturating_sub(2)))
}

pub(crate) fn action_palette_modal_with_width(
    ctx: &Context<AppRoot>,
    title: &str,
    width: u16,
) -> Modal {
    styled_modal(ctx, title, width)
        .height(Length::Auto)
        .max_height(Length::Percent(ACTION_PALETTE_MAX_HEIGHT_PERCENT))
        .reserve_height(Length::Percent(ACTION_PALETTE_MAX_HEIGHT_PERCENT))
        .padding(0)
}

pub(crate) fn canvas_rect_to_root(rect: FloatRect, top_chrome: u16) -> FloatRect {
    FloatRect {
        y: rect.y + f32::from(top_chrome),
        ..rect
    }
}

/// Persistent Rozi chrome: headers, directory names, section titles.
pub(crate) fn rozi_chrome(theme: &Theme) -> Style {
    crate::state::rozi_style(theme)
}

/// [`rozi_chrome`] reduced to a foreground, for text drawn over a surface fill.
pub(crate) fn rozi_fg(theme: &Theme) -> Style {
    fg_only(&rozi_chrome(theme))
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
mod pane_layer_tests {
    use crate::AppRoot;
    use crate::layout::anim::PaneAnimationStyle;
    use crate::layout::tiling::DwindleTree;
    use crate::state::{LayoutKind, Pane, SharedSessionState, SplitAxis};
    use tui_lipan::TestBackend;
    use tui_lipan::prelude::{FloatRect, Rect};

    fn smaller_follower_backend(
        pane_count: u32,
        layout_kind: LayoutKind,
        tile_tree: Option<DwindleTree>,
        canonical_canvas: (u16, u16),
    ) -> TestBackend<AppRoot> {
        crate::test_support::isolate_user_dirs();
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 80,
            h: 24,
        });
        {
            let state = backend.state_mut();
            state.config.animations.enabled = false;
            state.config.pane.show_workbar = false;
            state.config.pane.show_titles = false;

            let workspace = &mut state.current_mut().workspaces[0];
            workspace.layout_kind = layout_kind;
            workspace.panes.clear();
            workspace.tile_tree = tile_tree;
            for id in 1..=pane_count {
                let mut pane = Pane::new(id, 100, FloatRect::default());
                pane.opening = false;
                pane.terminal_active = true;
                workspace.panes.push(pane);
            }
            workspace.focused_pane = Some(2);
            state.current_mut().focused_pane = Some(2);

            let mut shared = SharedSessionState::new(1);
            shared.controller = Some(2);
            shared.canonical_canvas = Some(canonical_canvas);
            state.current_mut().shared = Some(shared);
        }
        backend
    }

    fn border_columns(line: &str, glyph: char) -> Vec<usize> {
        line.chars()
            .enumerate()
            .filter_map(|(column, found)| (found == glyph).then_some(column))
            .collect()
    }

    /// A leaving pane is drawn *under* the tile taking its space, for every style.
    ///
    /// Terminal cells have no transparency: an effect that removes a cell paints a blank over it.
    /// A leaving pane on top therefore covers its whole rectangle with a solid square regardless of
    /// how much of the effect is left - Portal's ring sits inside an opaque box, and Scale's
    /// shrinking frame floats over space the neighbour has already taken. Both read as artifacts.
    /// Underneath, the neighbour paints what it has claimed and the leaving pane shows through the
    /// rest, which is what going away looks like.
    #[test]
    fn a_closing_pane_is_drawn_under_the_neighbour_taking_its_space() {
        for style in [
            PaneAnimationStyle::Scale,
            PaneAnimationStyle::Slide,
            PaneAnimationStyle::Portal,
            PaneAnimationStyle::Scan,
        ] {
            crate::test_support::isolate_user_dirs();
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            });
            let (closing, survivor) = {
                let state = backend.state_mut();
                state.config.animations.pane_style = style;
                state.config.animations.geometry_duration = std::time::Duration::from_millis(900);
                state.config.confirm.close_pane = false;
                let mut neighbour = Pane::new(2, 100, FloatRect::default());
                neighbour.opening = false;
                neighbour.opening_animation = None;
                let workspace = state.active_workspace_mut();
                workspace.panes.push(neighbour);
                crate::layout::tiling::append_tiled_window(workspace, 2);
                workspace.panes[0].opening = false;
                workspace.panes[0].opening_animation = None;
                let key = |pane: &Pane| super::pane_window_key(pane.id, pane.pty_generation);
                (key(&workspace.panes[0]), key(&workspace.panes[1]))
            };
            backend.render();
            backend.advance(std::time::Duration::from_millis(1000));
            backend.render();

            backend
                .dispatch(crate::Msg::RunAction(crate::input::Action::Close))
                .expect("close the focused pane");
            backend.advance(std::time::Duration::from_millis(300));
            backend.render();

            let order = |key: &str| {
                backend
                    .capture_ui_snapshot()
                    .widgets
                    .iter()
                    .position(|widget| widget.key.as_ref().is_some_and(|k| k.as_ref() == key))
            };
            let closing_at = order(&closing)
                .unwrap_or_else(|| panic!("{style:?}: the closing pane is still rendered"));
            let survivor_at = order(&survivor)
                .unwrap_or_else(|| panic!("{style:?}: the surviving pane is still rendered"));
            assert!(
                closing_at < survivor_at,
                "{style:?}: the closing pane must paint before the tile taking its space, \
                 got closing at {closing_at} and survivor at {survivor_at}"
            );
        }
    }

    #[test]
    fn a_smaller_follower_keeps_a_split_border_on_the_viewport_edge() {
        // The narrow first pane is clipped just left of the follower. With the canonical origin
        // snapped to -28, pane 2's left frame lands exactly on local column zero.
        let mut backend = smaller_follower_backend(
            2,
            LayoutKind::Dwindle,
            Some(DwindleTree::Split {
                axis: SplitAxis::Horizontal,
                ratio: 0.1,
                first: Box::new(DwindleTree::Leaf(1)),
                second: Box::new(DwindleTree::Leaf(2)),
            }),
            (135, 25),
        );

        backend.render();
        let lines = backend.capture_frame().to_fixed_grid_lines();
        assert_eq!(
            lines[12].chars().next(),
            Some('│'),
            "the clipped split border drifted off the viewport edge:\n{}",
            lines.join("\n")
        );

        let bounds = super::follower_letterbox_bounds(
            backend.state(),
            Rect {
                x: 0,
                y: 0,
                w: 80,
                h: 24,
            },
        );
        assert_eq!((bounds.x, bounds.y), (-28.0, -1.0));
    }

    #[test]
    fn a_smaller_follower_keeps_equal_gaps_between_visible_columns() {
        let mut backend = smaller_follower_backend(3, LayoutKind::Columns, None, (135, 25));

        backend.render();
        let lines = backend.capture_frame().to_fixed_grid_lines();
        let borders = border_columns(&lines[12], '│');
        let gap_widths = borders
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .filter(|distance| *distance <= 3)
            .collect::<Vec<_>>();
        assert_eq!(
            gap_widths,
            vec![2, 2],
            "expected two visible pane gaps:\n{}",
            lines.join("\n")
        );
    }

    #[test]
    fn a_smaller_follower_keeps_visible_border_segments_without_reframing_clipped_edges() {
        let mut backend = smaller_follower_backend(
            3,
            LayoutKind::Dwindle,
            Some(DwindleTree::Split {
                axis: SplitAxis::Horizontal,
                ratio: 0.5,
                first: Box::new(DwindleTree::Leaf(1)),
                second: Box::new(DwindleTree::Split {
                    axis: SplitAxis::Vertical,
                    ratio: 0.5,
                    first: Box::new(DwindleTree::Leaf(2)),
                    second: Box::new(DwindleTree::Leaf(3)),
                }),
            }),
            (95, 31),
        );

        backend.render();
        let lines = backend.capture_frame().to_fixed_grid_lines();
        let message = || {
            format!(
                "visible border segments were erased or clipped panes were reframed:\n{}",
                lines.join("\n")
            )
        };
        assert!(
            lines.iter().all(|line| line.starts_with(' ')),
            "{}",
            message()
        );
        assert!(lines[11].contains("│ ╰"), "{}", message());
        assert!(lines[11].ends_with('─'), "{}", message());
        assert!(lines[12].contains("│ ╭"), "{}", message());
        assert!(lines[12].ends_with('─'), "{}", message());
        let verticals = border_columns(&lines[13], '│');
        assert_eq!(verticals, vec![38, 40], "{}", message());
    }
}
