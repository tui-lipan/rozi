//! Reconciling a client's live `State` with the shared layout document.

use super::*;

use crate::state::State;

/// Bridge for interactive operations on a workspace, including client-local scratch panes.
/// Absolute edits share the document implementation; geometry selection and focus stay with the UI.
pub(crate) fn edit_live_workspace(
    workspace: &mut crate::state::Workspace,
    edit: impl FnOnce(&mut SharedLayout) -> bool,
) -> bool {
    // Unit canvas preserves explicit cell rectangles exactly through the fractional representation.
    let mut layout = SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        canvas_cols: 1,
        canvas_rows: 1,
        pane_geometry: Default::default(),
        workspaces: vec![shared_workspace_from_state(0, workspace, 1, 1)],
    };
    let changed = edit(&mut layout);
    let shared = &layout.workspaces[0];
    workspace.layout_kind = shared.layout.into();
    workspace.split_ratios.clone_from(&shared.split_ratios);
    let known = shared.panes.iter().map(|pane| pane.pane_id).collect();
    workspace.tile_tree = shared
        .tree
        .as_ref()
        .and_then(|tree| dwindle_from_shared(tree, &known));
    for placement in &shared.panes {
        if let Some(pane) = workspace
            .panes
            .iter_mut()
            .find(|pane| pane.id == placement.pane_id)
        {
            let rect = placement
                .rect
                .map(|rect| frac_rect_to_float(rect, 1, 1))
                .unwrap_or(pane.floating_rect);
            apply_shared_pane_fields(pane, placement, rect);
        }
    }
    changed
}

pub(crate) fn edit_live_pane(
    workspace: &mut crate::state::Workspace,
    id: PaneId,
    edit: crate::control::PaneEdit,
) -> bool {
    edit_live_workspace(workspace, |layout| {
        layout.edit_pane_fields(id, edit).unwrap_or(false)
    })
}

pub(crate) fn transfer_live_pane(state: &mut State, id: PaneId, target: usize) -> bool {
    let mut layout = shared_layout_from_state(state, (1, 1));
    let changed = layout.move_pane_fields(id, target).unwrap_or(false);
    if changed {
        reconcile_controller_layout(state, &layout);
    }
    changed
}

pub(crate) fn swap_live_panes(
    workspace: &mut crate::state::Workspace,
    a: PaneId,
    b: PaneId,
) -> bool {
    edit_live_workspace(workspace, |layout| {
        layout.swap_pane_fields(a, b).unwrap_or(false)
    })
}

/// Apply an edit made by this controller without the follower lifecycle or lease machinery.
/// The document owns placement; existing panes retain their parsers, gestures, focus, anchors,
/// and animation state. The operation's caller handles its local presentation consequences.
pub(crate) fn reconcile_controller_layout(state: &mut State, layout: &SharedLayout) {
    let mut panes = std::collections::HashMap::new();
    let mut closing = Vec::new();
    for workspace in &mut state.current_mut().workspaces {
        let mut retained = Vec::new();
        for pane in std::mem::take(&mut workspace.panes) {
            if pane.closing {
                retained.push(pane);
            } else {
                panes.insert(pane.id, pane);
            }
        }
        closing.push(retained);
    }
    for shared in &layout.workspaces {
        let workspace = &mut state.current_mut().workspaces[shared.index];
        workspace.layout_kind = shared.layout.into();
        workspace.start_axis = shared.start_axis.into();
        workspace.split_ratios = shared.split_ratios.clone();
        workspace.name = shared.name.clone();
        workspace.synchronized = shared.synchronized;
        for placement in &shared.panes {
            let mut pane = panes
                .remove(&placement.pane_id)
                .expect("controller edits preserve live pane identities");
            let rect = placement
                .rect
                .map(|rect| frac_rect_to_float(rect, layout.canvas_cols, layout.canvas_rows))
                .unwrap_or(pane.floating_rect);
            apply_shared_pane_fields(&mut pane, placement, rect);
            workspace.panes.push(pane);
        }
        let known = workspace.panes.iter().map(|pane| pane.id).collect();
        workspace.tile_tree = shared
            .tree
            .as_ref()
            .and_then(|tree| dwindle_from_shared(tree, &known));
    }
    for (workspace, closing) in state.current_mut().workspaces.iter_mut().zip(closing) {
        workspace.panes.extend(closing);
    }
    assert!(
        panes.is_empty(),
        "controller removal must first run the close lifecycle"
    );
}

struct DrainedPanes {
    reusable: std::collections::HashMap<PaneId, crate::state::Pane>,
    closing_by_workspace: Vec<Vec<crate::state::Pane>>,
    pruned: Vec<(PaneId, u64)>,
}

struct RebuiltWorkspaces {
    next_pane_id: PaneId,
    next_generation: u64,
    orphan_clipboard_events: Vec<tui_lipan::prelude::TerminalClipboardEvent>,
    opened: Vec<(PaneId, u64)>,
}

fn index_incoming_panes(layout: &SharedLayout) -> std::collections::HashMap<PaneId, usize> {
    use crate::layout::model::WORKSPACE_COUNT;

    layout
        .workspaces
        .iter()
        .filter(|workspace| workspace.index < WORKSPACE_COUNT)
        .flat_map(|workspace| {
            workspace
                .panes
                .iter()
                .map(move |pane| (pane.pane_id, workspace.index))
        })
        .collect()
}

fn panes_move_between_workspaces(
    state: &State,
    incoming: &std::collections::HashMap<PaneId, usize>,
) -> bool {
    state
        .current()
        .workspaces
        .iter()
        .enumerate()
        .flat_map(|(workspace, state)| state.panes.iter().map(move |pane| (pane.id, workspace)))
        .any(|(id, workspace)| incoming.get(&id).is_some_and(|target| *target != workspace))
}

fn drain_existing_panes(
    state: &mut State,
    incoming: &std::collections::HashMap<PaneId, usize>,
    bounds: FloatRect,
    visible_bounds: FloatRect,
) -> DrainedPanes {
    use crate::layout::model::WORKSPACE_COUNT;

    let animations = state.config.animations;
    let top_gap = state.workspace_top_gap();
    let tile_gap = state.tile_gap_for(false);
    let mut reusable = std::collections::HashMap::new();
    let mut closing_by_workspace = Vec::with_capacity(WORKSPACE_COUNT);
    let mut pruned = Vec::new();
    for workspace in &mut state.current_mut().workspaces {
        let placements = crate::layout::workspace_target_rects_with_visible_bounds(
            workspace,
            bounds,
            visible_bounds,
            top_gap,
            tile_gap,
        );
        let mut closing = Vec::new();
        for mut pane in workspace.panes.drain(..) {
            if incoming.contains_key(&pane.id) {
                // A commit that re-adds a pane mid-close cancels the close and hands the live
                // pane back with its terminal screen and scrollback intact.
                pane.closing = false;
                pane.closing_animation = None;
                reusable.insert(pane.id, pane);
            } else if pane.closing {
                closing.push(pane);
            } else {
                // The server already dropped this pane. Re-killing it could race a reused id.
                if let Some(rect) = crate::layout::placement_for(&placements, pane.id) {
                    pane.floating_rect = FloatRect {
                        x: rect.x - bounds.x,
                        y: rect.y - bounds.y,
                        ..rect
                    };
                }
                pane.opening = false;
                pane.opening_animation = None;
                pane.closing = true;
                pane.begin_close_animation(animations);
                pane.terminal.kill();
                pruned.push((pane.id, pane.pty_generation));
                closing.push(pane);
            }
        }
        closing_by_workspace.push(closing);
    }
    DrainedPanes {
        reusable,
        closing_by_workspace,
        pruned,
    }
}

fn bind_shared_generation(
    state: &State,
    existing: &mut crate::state::Pane,
    shared_pane: &SharedPane,
) {
    if existing.pty_generation != shared_pane.generation {
        if let Some(capacity) = state.current().shared.as_ref().and_then(|shared| {
            shared
                .pane_scrollback
                .get(&(shared_pane.pane_id, shared_pane.generation))
        }) {
            existing.terminal.use_server_scrollback(*capacity);
        }
        existing.terminal.cols = existing.terminal.cols.max(1);
        existing.terminal.rows = existing.terminal.rows.max(1);
        existing
            .terminal
            .bind_server_backend(shared_pane.pane_id, shared_pane.generation);
        existing.pty_generation = shared_pane.generation;
    }
}

fn begin_shared_pane_open(
    state: &State,
    pane: &mut crate::state::Pane,
    added: bool,
    animate: bool,
) -> Option<(PaneId, u64)> {
    if !added || !animate {
        return None;
    }
    pane.opening = true;
    pane.terminal_active = false;
    pane.begin_open_animation(state.config.animations);
    Some((pane.id, pane.pty_generation))
}

fn rebuild_shared_workspaces(
    ctx: &mut Context<crate::AppRoot>,
    layout: &SharedLayout,
    canonical_canvas: (u16, u16),
    bounds: FloatRect,
    reusable: &mut std::collections::HashMap<PaneId, crate::state::Pane>,
    closing_by_workspace: &mut [Vec<crate::state::Pane>],
) -> RebuiltWorkspaces {
    use crate::layout::model::WORKSPACE_COUNT;
    use crate::state::Pane;

    let (canvas_cols, canvas_rows) = canonical_canvas;
    let mut next_pane_id = ctx.state.current().next_pane_id;
    let mut next_generation = ctx.state.current().next_pty_generation;
    let mut orphan_clipboard_events = Vec::new();
    let mut opened = Vec::new();
    let animate_additions = ctx
        .state
        .current()
        .shared
        .as_ref()
        .is_some_and(|shared| shared.canonical_canvas.is_some());
    let mut seen_ids = std::collections::HashSet::new();

    for shared_workspace in &layout.workspaces {
        if shared_workspace.index >= WORKSPACE_COUNT {
            continue;
        }
        let mut rebuilt = Vec::with_capacity(shared_workspace.panes.len());
        for shared_pane in &shared_workspace.panes {
            if !seen_ids.insert(shared_pane.pane_id) {
                continue;
            }
            next_pane_id = next_pane_id.max(shared_pane.pane_id.saturating_add(1));
            next_generation = next_generation.max(shared_pane.generation.saturating_add(1));

            let float_rect = shared_pane
                .rect
                .map(|rect| frac_rect_to_float(rect, canvas_cols, canvas_rows))
                .unwrap_or_else(|| {
                    crate::layout::geometry::default_floating_rect(bounds, shared_pane.pane_id)
                });
            let existing = reusable.remove(&shared_pane.pane_id).or_else(|| {
                ctx.state
                    .current_mut()
                    .take_retired_pane(shared_pane.pane_id, shared_pane.generation)
            });
            let added = existing.is_none();
            let mut pane = match existing {
                Some(mut existing) => {
                    // A new generation needs a fresh server backend, but keeps local dimensions.
                    bind_shared_generation(&ctx.state, &mut existing, shared_pane);
                    orphan_clipboard_events.extend(drain_orphan_output(
                        ctx_shared_mut(ctx),
                        &mut existing,
                        shared_pane,
                    ));
                    end_invalid_orphan_transport(ctx);
                    existing
                }
                None => {
                    let scrollback = ctx
                        .state
                        .current()
                        .shared
                        .as_ref()
                        .and_then(|shared| {
                            shared
                                .pane_scrollback
                                .get(&(shared_pane.pane_id, shared_pane.generation))
                        })
                        .copied()
                        .expect("server metadata must precede shared pane placement");
                    let mut pane = Pane::new(shared_pane.pane_id, scrollback, float_rect);
                    pane.pty_generation = shared_pane.generation;
                    pane.terminal
                        .bind_server_backend(shared_pane.pane_id, shared_pane.generation);
                    pane.opening = false;
                    pane.terminal_active = true;
                    pane.terminal.status = ManagedTerminalStatus::Ready;
                    orphan_clipboard_events.extend(drain_orphan_output(
                        ctx_shared_mut(ctx),
                        &mut pane,
                        shared_pane,
                    ));
                    end_invalid_orphan_transport(ctx);
                    pane
                }
            };

            apply_shared_pane_fields(&mut pane, shared_pane, float_rect);
            if let Some(opened_pane) =
                begin_shared_pane_open(&ctx.state, &mut pane, added, animate_additions)
            {
                opened.push(opened_pane);
            }
            rebuilt.push(pane);
        }

        let workspace = &mut ctx.state.current_mut().workspaces[shared_workspace.index];
        workspace.name = shared_workspace
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        workspace.synchronized = shared_workspace.synchronized;
        workspace.layout_kind = shared_workspace.layout.into();
        workspace.start_axis = shared_workspace.start_axis.into();
        if !shared_workspace.split_ratios.is_empty() {
            workspace.split_ratios = shared_workspace.split_ratios.clone();
        }
        let known = rebuilt
            .iter()
            .filter(|pane| !pane.floating)
            .map(|pane| pane.id)
            .collect();
        workspace.tile_tree = shared_workspace
            .tree
            .as_ref()
            .and_then(|tree| dwindle_from_shared(tree, &known));
        workspace.last_move_swap = None;

        // Closing panes keep rendering in their old workspace until their animation is pruned.
        rebuilt.extend(std::mem::take(
            &mut closing_by_workspace[shared_workspace.index],
        ));
        workspace.panes = rebuilt;
    }

    RebuiltWorkspaces {
        next_pane_id,
        next_generation,
        orphan_clipboard_events,
        opened,
    }
}

fn restore_closing_panes_in_omitted_workspaces(
    state: &mut State,
    layout: &SharedLayout,
    closing_by_workspace: Vec<Vec<crate::state::Pane>>,
) {
    use crate::layout::model::WORKSPACE_COUNT;

    for (index, closing) in closing_by_workspace.into_iter().enumerate() {
        let omitted = !layout
            .workspaces
            .iter()
            .any(|workspace| workspace.index == index && workspace.index < WORKSPACE_COUNT);
        if omitted {
            state.current_mut().workspaces[index].panes.clear();
        }
        state.current_mut().workspaces[index].panes.extend(closing);
    }
}

fn repair_workspace_focus_and_anchors(
    state: &mut State,
) -> [bool; crate::layout::model::WORKSPACE_COUNT] {
    use crate::layout::model::{LayoutKind, ScrollableRevealEdge, WORKSPACE_COUNT};

    let mut reveal_after_focus_fallback = [false; WORKSPACE_COUNT];
    for (index, workspace) in state.current_mut().workspaces.iter_mut().enumerate() {
        let focus_valid = workspace.focused_pane.is_some_and(|id| {
            workspace
                .panes
                .iter()
                .any(|pane| pane.id == id && !pane.closing)
        });
        let focus_invalid = !focus_valid;
        if focus_invalid {
            workspace.focused_pane = workspace
                .panes
                .iter()
                .find(|pane| !pane.closing)
                .map(|pane| pane.id);
        }
        let anchor_valid = workspace.scrollable_anchor.is_some_and(|id| {
            workspace
                .panes
                .iter()
                .any(|pane| pane.id == id && !pane.closing && !pane.floating)
        });
        if !anchor_valid {
            workspace.set_scrollable_viewport(None, ScrollableRevealEdge::Left);
        } else if focus_invalid && workspace.layout_kind == LayoutKind::Scrollable {
            reveal_after_focus_fallback[index] = true;
        }
    }
    reveal_after_focus_fallback
}

/// Reconcile the client's local `State` toward an authoritative shared layout at revision `rev`.
///
/// This is the follower's read path (and the seed path on attach). It moves, adds, removes, and
/// reorders `Pane` structs and rewrites workspace metadata, but never touches a surviving pane's
/// terminal screen, scrollback, or snapshot - only brand-new panes get a fresh backend, and only
/// their buffered orphan output is replayed. Local-only state (focus, active workspace, overlays,
/// mode, theme) is preserved. Removed panes retain their last placement until their close animation
/// finishes and the prune timer drops them.
pub(crate) fn apply_shared_layout(
    ctx: &mut Context<crate::AppRoot>,
    layout: &SharedLayout,
    rev: u64,
) -> Update {
    use crate::layout::model::WORKSPACE_COUNT;

    // A foreign commit can only land mid-drag right after this client lost the lease; cancel any
    // in-flight move/resize so it does not fight the incoming geometry.
    ctx.state.moving_pane = None;
    ctx.state.resizing_pane = None;
    ctx.state.split_drag = None;
    let canvas_cols = layout.canvas_cols.max(1);
    let canvas_rows = layout.canvas_rows.max(1);
    let bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());

    let incoming = index_incoming_panes(layout);
    if panes_move_between_workspaces(&ctx.state, &incoming) {
        ctx.state.pane_canvas_epoch = ctx.state.pane_canvas_epoch.wrapping_add(1);
    }

    let previous_bounds = crate::view::follower_letterbox_bounds(&ctx.state, ctx.viewport());
    let visible_bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());

    // Missing panes remain in their workspace until their close animation is pruned.
    let DrainedPanes {
        reusable: mut pool,
        closing_by_workspace: mut closing_by_ws,
        pruned,
    } = drain_existing_panes(&mut ctx.state, &incoming, previous_bounds, visible_bounds);

    let RebuiltWorkspaces {
        next_pane_id,
        next_generation,
        orphan_clipboard_events,
        opened,
    } = rebuild_shared_workspaces(
        ctx,
        layout,
        (canvas_cols, canvas_rows),
        bounds,
        &mut pool,
        &mut closing_by_ws,
    );

    restore_closing_panes_in_omitted_workspaces(&mut ctx.state, layout, closing_by_ws);

    // Fix up focus per workspace: keep the current focus when it survived, else fall back to the
    // first live pane. Local scrollable anchors survive only while their tiled pane still exists.
    // When focus falls back but a different Scrollable anchor remains, the fallback may sit under
    // a right-scrolled viewport — remember to sync reveal for that workspace after active is known.
    let scrollable_reveal_after_focus_fallback = repair_workspace_focus_and_anchors(&mut ctx.state);
    let active = ctx
        .state
        .current()
        .active_workspace
        .min(WORKSPACE_COUNT - 1);
    ctx.state.current_mut().active_workspace = active;
    ctx.state.current_mut().focused_pane = ctx.state.current_mut().workspaces[active].focused_pane;
    // Local-only reveal for a focus fallback under a surviving Scrollable anchor. Does not arm
    // AxisChange — reconciler keeps Close/TileFloat below.
    if scrollable_reveal_after_focus_fallback[active]
        && let Some(focus) = ctx.state.current().focused_pane
    {
        crate::ops::focus::sync_scrollable_reveal(&mut ctx.state, focus, false);
    }

    ctx.state.current_mut().next_pane_id = ctx.state.current_mut().next_pane_id.max(next_pane_id);
    ctx.state.current_mut().next_pty_generation = ctx
        .state
        .current_mut()
        .next_pty_generation
        .max(next_generation);
    if let Some(shared) = ctx.state.current_mut().shared.as_mut() {
        shared.layout_rev = rev;
        shared.canonical_canvas = Some((canvas_cols, canvas_rows));
        shared.last_committed_layout = Some(layout.clone());
    }
    if ctx.state.config.clipboard.enable_osc52 {
        for event in orphan_clipboard_events {
            if matches!(
                event.target,
                tui_lipan::prelude::TerminalClipboardTarget::Clipboard
            ) {
                ctx.clipboard().relay_osc52(&event.text);
            }
        }
    }
    if !pruned.is_empty() {
        ctx.state
            .begin_pane_event(crate::layout::anim::GeometryAnimation::Close);
        let timeout = pruned
            .iter()
            .filter_map(|(id, _)| {
                crate::pane::lifecycle::find_pane(&ctx.state, *id).map(|pane| {
                    crate::layout::anim::retained_pane_timeout_for_pane(
                        ctx.state.config.animations,
                        pane,
                        ctx.state.runtime_frame_rate(),
                    )
                })
            })
            .max()
            .unwrap_or_else(|| {
                crate::layout::anim::retained_pane_timeout(
                    ctx.state.config.animations,
                    ctx.state.runtime_frame_rate(),
                )
            });
        return shared_lifecycle_update(ctx, opened, pruned, timeout);
    }
    ctx.state.animation = crate::layout::anim::GeometryAnimation::TileFloat;

    shared_lifecycle_update(ctx, opened, pruned, std::time::Duration::ZERO)
}

fn shared_lifecycle_update(
    ctx: &Context<crate::AppRoot>,
    opened: Vec<(PaneId, u64)>,
    closed: Vec<(PaneId, u64)>,
    close_delay: std::time::Duration,
) -> Update {
    if opened.is_empty() && closed.is_empty() {
        return Update::full();
    }
    let epoch = ctx.state.runtime_epoch;
    if opened.is_empty() {
        return Update::with_command(crate::pane::lifecycle::prune_closed_batch_command(
            epoch,
            closed,
            close_delay,
        ));
    }
    let animations = ctx.state.config.animations;
    let open_delay = crate::layout::anim::open_delay(animations);
    let delay = if closed.is_empty() {
        open_delay
    } else {
        open_delay.min(close_delay)
    };
    Update::with_command(Command::after(
        delay,
        move |link: CommandLink<crate::Msg>| {
            for (id, generation) in opened {
                send_lifecycle_after(
                    &link,
                    open_delay.saturating_sub(delay),
                    crate::Msg::FinishOpen(epoch, id, generation),
                );
                send_lifecycle_after(
                    &link,
                    crate::layout::anim::activation_delay(animations).saturating_sub(delay),
                    crate::Msg::ActivatePane(epoch, id, generation),
                );
            }
            for (id, generation) in closed {
                send_lifecycle_after(
                    &link,
                    close_delay.saturating_sub(delay),
                    crate::Msg::PruneClosed(epoch, id, generation),
                );
            }
        },
    ))
}

fn send_lifecycle_after(
    link: &CommandLink<crate::Msg>,
    delay: std::time::Duration,
    message: crate::Msg,
) {
    if delay.is_zero() {
        link.send(message);
    } else {
        link.send_after(delay, message);
    }
}

/// Reborrow the shared-session bookkeeping mutably; used by the reconciler's orphan drain so it can
/// touch `orphan_output` while also mutating a pane.
fn ctx_shared_mut(
    ctx: &mut Context<crate::AppRoot>,
) -> Option<&mut crate::state::SharedSessionState> {
    ctx.state.current_mut().shared.as_mut()
}

fn end_invalid_orphan_transport(ctx: &mut Context<crate::AppRoot>) {
    if ctx
        .state
        .current()
        .shared
        .as_ref()
        .is_some_and(|s| s.orphan_replay_required)
        && let Some(client) = &ctx.state.current().session_client
    {
        client.end_for_replay();
    }
}

fn drain_orphan_output(
    shared: Option<&mut crate::state::SharedSessionState>,
    pane: &mut crate::state::Pane,
    shared_pane: &SharedPane,
) -> Vec<tui_lipan::prelude::TerminalClipboardEvent> {
    let Some(shared) = shared else {
        return Vec::new();
    };
    // Geometry the server reported before this layout created the pane comes first: the buffered
    // output was written against it, pictures included.
    if let Some(geometry) = shared.take_orphan_geometry(shared_pane.pane_id, shared_pane.generation)
    {
        pane.terminal
            .apply_server_geometry(geometry.cols, geometry.rows, geometry.cell);
    }
    if let Some((bytes, media)) =
        shared.take_owned_orphan_output(shared_pane.pane_id, shared_pane.generation)
    {
        match pane.terminal.process_server_output_media(&bytes, media) {
            Ok(output) => return output.clipboard_events,
            Err(_) => {
                shared.orphan_replay_required = true;
            }
        }
    }
    Vec::new()
}

fn apply_shared_pane_fields(
    pane: &mut crate::state::Pane,
    shared_pane: &SharedPane,
    float_rect: FloatRect,
) {
    pane.floating = shared_pane.floating;
    pane.pinned = shared_pane.pinned;
    pane.fullscreen = shared_pane.fullscreen;
    if shared_pane.floating {
        pane.floating_rect = float_rect;
        pane.floating_rect_initialized = true;
    }
    pane.identity.custom_title = shared_pane.title.clone();
    pane.identity.profile_name = shared_pane.profile_name.clone();
    pane.identity.cwd = shared_pane.cwd.clone();
    pane.identity.launch = shared_pane.launch.clone();
    pane.identity.replay = shared_pane.replay
        && matches!(
            pane.identity.launch.as_ref(),
            Some(crate::pane::launch::PaneLaunch::Shell { .. })
        );
    pane.identity.keep_open = shared_pane.keep_open;
    pane.scrollable_width =
        crate::layout::tiling::sanitize_scrollable_width(shared_pane.scrollable_width);
}

/// Reconciler behavior driven through the real runtime (a follower/controller applying commits).
#[cfg(test)]
mod reconciler_tests {
    use super::*;
    use crate::AppRoot;
    use crate::Msg;
    use crate::input::Action;
    use crate::layout::model::Direction;
    use crate::layout::shared::test_support::{layout_with_panes, state_with_split};
    use crate::ops::focus::focus_pane;
    use crate::pane::lifecycle::{find_pane, find_pane_mut};
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::ClientMessage;
    use crate::state::Pane;
    use crate::state::{DirectionalFocusHint, SharedSessionState};
    use tui_lipan::TestBackend;

    // Seed capacity metadata for synthetic layout fixtures. Explicit resize/output fixtures keep
    // their own message ordering; the metadata tests below drive the production handler.
    trait ServerDispatch {
        fn dispatch_server(&mut self, message: Msg) -> tui_lipan::Result<bool>;
    }
    impl ServerDispatch for TestBackend<AppRoot> {
        fn dispatch_server(&mut self, mut message: Msg) -> tui_lipan::Result<bool> {
            if let Msg::SessionAttached {
                layout: Some(layout),
                panes,
                ..
            } = &mut message
            {
                *panes = layout
                    .workspaces
                    .iter()
                    .flat_map(|ws| &ws.panes)
                    .map(|pane| {
                        crate::session::protocol::PaneMeta::for_test(
                            pane.pane_id,
                            pane.generation,
                            100,
                        )
                    })
                    .collect();
            }
            if let Msg::SessionLayoutCommitted { layout, .. } = &message {
                for pane in layout.workspaces.iter().flat_map(|ws| &ws.panes) {
                    let capacity = self
                        .state()
                        .current()
                        .shared
                        .as_ref()
                        .and_then(|shared| {
                            shared.pane_scrollback.get(&(pane.pane_id, pane.generation))
                        })
                        .copied()
                        .or_else(|| {
                            find_pane(self.state(), pane.pane_id)
                                .map(|pane| pane.terminal.scrollback_capacity())
                        })
                        .unwrap_or(100);
                    self.state_mut()
                        .current_mut()
                        .shared
                        .as_mut()
                        .unwrap()
                        .pane_scrollback
                        .insert((pane.pane_id, pane.generation), capacity);
                }
            }
            self.dispatch(message)
        }
    }

    const VIEWPORT: Rect = Rect {
        x: 0,
        y: 0,
        w: 100,
        h: 30,
    };

    /// Run in a generous stack: mounting the full app in `TestBackend` overflows the default test
    /// stack, matching the pattern used by the snapshot tests in `main.rs`.
    fn in_stack<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
        crate::test_support::on_large_stack(body)
    }

    fn attach_follower(backend: &mut TestBackend<AppRoot>, client: SessionClient) {
        let state = backend.state_mut();
        state.current_mut().session_attached = true;
        state.current_mut().session_client = Some(client);
        let mut shared = SharedSessionState::new(1);
        shared.controller = Some(2); // another client controls the layout; we follow.
        shared.clients = vec![
            crate::session::protocol::ClientInfo {
                id: 1,
                label: "a".into(),
                read_only: false,
                requesting_control: false,
                parked: false,
            },
            crate::session::protocol::ClientInfo {
                id: 2,
                label: "b".into(),
                read_only: false,
                requesting_control: false,
                parked: false,
            },
        ];
        state.current_mut().shared = Some(shared);
    }

    #[test]
    fn server_metadata_controls_new_and_reconciled_parsers_across_clients_and_generations() {
        in_stack(|| {
            let mut histories = Vec::new();
            for local_capacity in [2, 1000] {
                let mut backend = TestBackend::new(AppRoot::default());
                backend.state_mut().config.scrollback = local_capacity;
                backend.set_viewport(VIEWPORT);
                let (client, _rx) = SessionClient::test_channel();
                attach_follower(&mut backend, client);
                for (id, generation, capacity) in [(7, 1, 7), (8, 1, 19), (7, 2, 11)] {
                    backend
                        .dispatch(Msg::SessionPaneMetadata {
                            cell_width: 9,
                            cell_height: 18,
                            epoch: 0,
                            local: false,
                            meta: crate::session::protocol::PaneMeta::for_test(
                                id, generation, capacity,
                            ),
                        })
                        .unwrap();
                    let panes = if id == 8 {
                        vec![(7, 1), (8, 1)]
                    } else if generation == 2 {
                        vec![(7, 2), (8, 1)]
                    } else {
                        vec![(7, 1)]
                    };
                    backend
                        .dispatch(Msg::SessionLayoutCommitted {
                            epoch: 0,
                            rev: generation,
                            author: 2,
                            layout: layout_with_panes(&panes),
                            live: false,
                        })
                        .unwrap();
                    let pane = find_pane_mut(backend.state_mut(), id).unwrap();
                    assert_eq!(pane.terminal.scrollback_capacity(), capacity);
                    assert_eq!((pane.terminal.cols, pane.terminal.rows), (80, 24));
                    assert_eq!(
                        pane.terminal.cell_size(),
                        tui_lipan::TerminalCellSize {
                            width: 9,
                            height: 18
                        }
                    );
                    let output: String =
                        (0..100).map(|line| format!("history-{line}\r\n")).collect();
                    pane.terminal.process_server_output(output.as_bytes());
                    assert!(pane.terminal.total_scrollback_rows() <= capacity);
                }
                assert_eq!(
                    find_pane(backend.state(), 8)
                        .unwrap()
                        .terminal
                        .scrollback_capacity(),
                    19
                );
                histories.push(
                    find_pane(backend.state(), 7)
                        .unwrap()
                        .terminal
                        .capture_scrollback_text(None),
                );
            }
            assert_eq!(histories[0], histories[1]);
        });
    }

    #[test]
    fn attach_and_reconnect_use_reported_capacity_instead_of_client_configuration() {
        in_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            backend.state_mut().config.scrollback = 1;
            for epoch in [1, 2] {
                pend_attach(&mut backend, epoch, "history");
                let mut message = attach_message(epoch, "history", &[(7, 1), (8, 1)]);
                if let Msg::SessionAttached { panes, .. } = &mut message {
                    *panes = vec![
                        crate::session::protocol::PaneMeta::for_test(7, 1, 7),
                        crate::session::protocol::PaneMeta::for_test(8, 1, 19),
                    ];
                }
                backend.dispatch(message).unwrap();
                assert_eq!(
                    find_pane(backend.state(), 7)
                        .unwrap()
                        .terminal
                        .scrollback_capacity(),
                    7
                );
                assert_eq!(
                    find_pane(backend.state(), 8)
                        .unwrap()
                        .terminal
                        .scrollback_capacity(),
                    19
                );
                backend.state_mut().config.scrollback = 999;
            }
        });
    }

    #[test]
    fn remote_removal_drops_pane_without_sending_kill() {
        in_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            let (client, rx) = SessionClient::test_channel();
            attach_follower(&mut backend, client);
            backend.render();

            // A commit with no panes removes the local seed pane.
            backend
                .dispatch_server(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout: layout_with_panes(&[]),
                    live: false,
                })
                .expect("dispatch commit");

            let kills = rx
                .try_iter()
                .filter(|msg| matches!(msg, ClientOutbound::Control(ClientMessage::Kill { .. })))
                .count();
            assert_eq!(kills, 0, "reconciler removal must not emit a Kill frame");
            // The pane animates out rather than vanishing, exactly as a local close does.
            let workspace = &backend.state().current().workspaces[0];
            assert!(workspace.panes.iter().all(|pane| pane.closing));
            assert_eq!(workspace.visible_count(), 0);

            let epoch = backend.state().runtime_epoch;
            let generation = backend.state().current().workspaces[0].panes[0].pty_generation;
            backend
                .dispatch(Msg::PruneClosed(epoch, 1, generation))
                .expect("prune");
            let workspace = &backend.state().current().workspaces[0];
            assert!(workspace.panes.is_empty());
        });
    }

    #[test]
    fn same_generation_readd_restores_the_retired_terminal_screen() {
        in_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            let (client, _rx) = SessionClient::test_channel();
            attach_follower(&mut backend, client);
            let output = (0..48)
                .map(|row| format!("retained {row}\r\n"))
                .collect::<String>();
            let pane = find_pane_mut(backend.state_mut(), 1).expect("seed pane");
            pane.terminal.process_server_output(output.as_bytes());
            assert!(pane.terminal.set_scrollback(2));
            let before = pane.terminal.capture_text();
            assert!(!before.trim().is_empty());
            backend.render();

            backend
                .dispatch_server(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout: layout_with_panes(&[]),
                    live: false,
                })
                .expect("remove pane");
            backend
                .dispatch_server(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 2,
                    author: 2,
                    layout: layout_with_panes(&[(1, 0)]),
                    live: false,
                })
                .expect("restore pane");

            let pane = find_pane(backend.state_mut(), 1).expect("restored pane");
            assert_eq!(pane.terminal.capture_text(), before);
            assert_eq!(pane.terminal.scrollback_offset(), 2);
        });
    }

    #[test]
    fn shared_layout_readd_does_not_duplicate_a_pane_id() {
        in_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            let (client, _rx) = SessionClient::test_channel();
            attach_follower(&mut backend, client);
            backend.render();

            let mut layout = layout_with_panes(&[(1, 0)]);
            let duplicate = layout.workspaces[0].panes[0].clone();
            layout.workspaces[0].panes.push(duplicate);
            backend
                .dispatch_server(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout,
                    live: false,
                })
                .expect("dispatch duplicate-id commit");

            assert_eq!(
                backend.state().current().workspaces[0].panes.len(),
                1,
                "a repeated shared id must rebuild one live pane"
            );
        });
    }

    #[test]
    fn remote_addition_creates_a_ready_pane() {
        in_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            let (client, _rx) = SessionClient::test_channel();
            attach_follower(&mut backend, client);
            backend.render();

            // Keep the seed pane (id 1, generation 0) and add a brand-new pane (id 2).
            backend
                .dispatch_server(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout: layout_with_panes(&[(1, 0), (2, 5)]),
                    live: false,
                })
                .expect("dispatch commit");

            let added = find_pane(backend.state_mut(), 2).expect("pane 2 created by reconciler");
            assert_eq!(added.pty_generation, 5);
            assert!(added.terminal.is_ready());
            assert!(find_pane(backend.state_mut(), 1).is_some(), "survivor kept");
            assert_eq!(
                backend.state_mut().animation,
                crate::layout::anim::GeometryAnimation::TileFloat,
                "live layout commits should retain geometry transitions"
            );
        });
    }

    /// The server can report a new pane's geometry and output before the layout commit that creates
    /// the pane reaches a follower. The output is buffered; the geometry must be too, so a picture
    /// in that early output is laid out against the session's cells rather than this client's.
    #[test]
    fn geometry_reported_before_the_layout_applies_to_buffered_output() {
        use base64::Engine as _;

        in_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            let (client, _rx) = SessionClient::test_channel();
            attach_follower(&mut backend, client);
            backend.render();
            let epoch = backend.state().runtime_epoch;
            let session_cell = tui_lipan::TerminalCellSize::new(14, 32);
            assert_ne!(tui_lipan::host_cell_size(), session_cell);

            backend
                .dispatch(Msg::SessionResized {
                    epoch,
                    pane_id: 2,
                    local: false,
                    generation: 5,
                    cols: 40,
                    rows: 12,
                    cell: Some(session_cell),
                })
                .expect("dispatch early resize");
            // 112x128 pixels is 8x4 of the session's 14x32 cells, and stays within the orphan
            // buffer's per-pane cap.
            let payload = base64::engine::general_purpose::STANDARD
                .encode([10u8, 20, 30, 255].repeat(112 * 128));
            backend
                .dispatch(Msg::SessionOutput {
                    media: None,
                    metadata: crate::session::protocol::OutputMetadata::default(),
                    epoch,
                    pane_id: 2,
                    local: false,
                    generation: 5,
                    bytes: format!("\x1b[H\x1b_Ga=T,f=32,s=112,v=128,i=9,q=2;{payload}\x1b\\")
                        .into_bytes(),
                })
                .expect("dispatch early output");
            backend
                .dispatch_server(Msg::SessionLayoutCommitted {
                    epoch,
                    rev: 1,
                    author: 2,
                    layout: layout_with_panes(&[(1, 0), (2, 5)]),
                    live: false,
                })
                .expect("dispatch commit");

            let pane = find_pane(backend.state_mut(), 2).expect("pane 2 created by reconciler");
            assert_eq!(pane.terminal.cell_size(), session_cell);
            assert_eq!((pane.terminal.cols, pane.terminal.rows), (40, 12));
            let snapshot = pane.terminal.snapshot();
            let placement = snapshot.images.first().expect("buffered image placed");
            assert_eq!((placement.cols, placement.rows), (8, 4));
        });
    }

    /// A respawn replaces a pane's generation, and the server can report the new generation's
    /// geometry and output before the layout commit that carries it. The geometry is held for the
    /// new screen, through any commit that still shows the old one, and never applied to the old.
    #[test]
    fn geometry_reported_for_a_new_generation_applies_to_its_buffered_output() {
        use base64::Engine as _;

        in_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            let (client, _rx) = SessionClient::test_channel();
            attach_follower(&mut backend, client);
            backend.render();
            let epoch = backend.state().runtime_epoch;
            let commit = |backend: &mut TestBackend<AppRoot>, rev, generation| {
                backend
                    .dispatch_server(Msg::SessionLayoutCommitted {
                        epoch,
                        rev,
                        author: 2,
                        layout: layout_with_panes(&[(1, 0), (2, generation)]),
                        live: false,
                    })
                    .expect("dispatch commit");
            };
            commit(&mut backend, 1, 5);
            let host_cell = find_pane(backend.state_mut(), 2)
                .expect("pane 2 at generation 5")
                .terminal
                .cell_size();
            let session_cell = tui_lipan::TerminalCellSize::new(14, 32);
            assert_ne!(host_cell, session_cell);

            backend
                .dispatch(Msg::SessionResized {
                    epoch,
                    pane_id: 2,
                    local: false,
                    generation: 6,
                    cols: 40,
                    rows: 12,
                    cell: Some(session_cell),
                })
                .expect("dispatch early resize");
            let old = find_pane(backend.state_mut(), 2).expect("pane 2 at generation 5");
            assert_eq!(old.pty_generation, 5);
            assert_eq!(
                old.terminal.cell_size(),
                host_cell,
                "the old screen keeps its cells"
            );
            // A revision that still shows the old generation must not drop the new one's geometry.
            commit(&mut backend, 2, 5);
            // 112x128 pixels is 8x4 of the session's 14x32 cells.
            let payload = base64::engine::general_purpose::STANDARD
                .encode([10u8, 20, 30, 255].repeat(112 * 128));
            backend
                .dispatch(Msg::SessionOutput {
                    media: None,
                    metadata: crate::session::protocol::OutputMetadata::default(),
                    epoch,
                    pane_id: 2,
                    local: false,
                    generation: 6,
                    bytes: format!("\x1b[H\x1b_Ga=T,f=32,s=112,v=128,i=9,q=2;{payload}\x1b\\")
                        .into_bytes(),
                })
                .expect("dispatch early output");
            commit(&mut backend, 3, 6);

            let pane = find_pane(backend.state_mut(), 2).expect("pane 2 at generation 6");
            assert_eq!(pane.pty_generation, 6);
            assert_eq!(pane.terminal.cell_size(), session_cell);
            assert_eq!((pane.terminal.cols, pane.terminal.rows), (40, 12));
            let snapshot = pane.terminal.snapshot();
            let placement = snapshot.images.first().expect("buffered image placed");
            assert_eq!((placement.cols, placement.rows), (8, 4));
        });
    }

    #[test]
    fn reconciliation_discards_older_drains_exact_and_retains_future_orphans() {
        in_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            let (client, _rx) = SessionClient::test_channel();
            attach_follower(&mut backend, client);
            {
                let shared = backend
                    .state_mut()
                    .current_mut()
                    .shared
                    .as_mut()
                    .expect("shared state");
                shared.buffer_orphan_output(2, 4, b"older\r\n");
                shared.buffer_orphan_output(2, 5, b"exact\r\n");
                shared.buffer_orphan_output(2, 6, b"future\r\n");
            }

            backend
                .dispatch_server(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout: layout_with_panes(&[(1, 0), (2, 5)]),
                    live: false,
                })
                .expect("reconcile generation");

            let screen = find_pane(backend.state_mut(), 2)
                .expect("reconciled pane")
                .terminal
                .capture_text();
            assert!(screen.contains("exact"));
            assert!(!screen.contains("older"));
            assert!(!screen.contains("future"));

            let shared = backend
                .state_mut()
                .current_mut()
                .shared
                .as_mut()
                .expect("shared state");
            assert_eq!(
                shared.orphan_output_stats(),
                crate::state::OrphanOutputStats {
                    retained: b"future\r\n".len(),
                    high_water: b"older\r\nexact\r\nfuture\r\n".len(),
                    keys: 1,
                }
            );
            assert_eq!(
                shared.take_orphan_output(2, 6),
                Some(b"future\r\n".to_vec())
            );
        });
    }

    #[test]
    fn shared_layout_reconcile_preserves_or_clears_scrollable_anchor() {
        in_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            let (client, _rx) = SessionClient::test_channel();
            attach_follower(&mut backend, client);
            backend.render();

            let layout = layout_with_panes(&[(1, 0), (2, 0)]);
            backend
                .dispatch_server(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout: layout.clone(),
                    live: false,
                })
                .expect("seed shared layout");

            backend.state_mut().current_mut().workspaces[0].scrollable_anchor = Some(2);
            backend
                .dispatch_server(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 2,
                    author: 2,
                    layout: layout.clone(),
                    live: false,
                })
                .expect("reconcile with survivor");
            assert_eq!(
                backend.state().current().workspaces[0].scrollable_anchor,
                Some(2)
            );

            let mut without_anchor = layout_with_panes(&[(1, 0)]);
            without_anchor.workspaces[0].layout = SharedLayoutKind::Scrollable;
            backend
                .dispatch_server(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 3,
                    author: 2,
                    layout: without_anchor,
                    live: false,
                })
                .expect("reconcile without survivor");
            assert_eq!(
                backend.state().current().workspaces[0].scrollable_anchor,
                None
            );
        });
    }

    #[test]
    fn shared_layout_focus_fallback_reveals_under_surviving_scrollable_anchor() {
        in_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            let (client, _rx) = SessionClient::test_channel();
            attach_follower(&mut backend, client);
            backend.render();

            let mut layout = layout_with_panes(&[(1, 0), (2, 0), (3, 0), (4, 0)]);
            layout.workspaces[0].layout = SharedLayoutKind::Scrollable;
            backend
                .dispatch_server(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout: layout.clone(),
                    live: false,
                })
                .expect("seed scrollable");
            {
                let state = backend.state_mut();
                focus_pane(state, 4);
                state.animation = crate::layout::anim::GeometryAnimation::None;
            }
            backend.render();
            // Focus a still-visible pane so the right anchor survives when that focus is removed.
            {
                let state = backend.state_mut();
                focus_pane(state, 3);
                state.animation = crate::layout::anim::GeometryAnimation::None;
            }
            backend.render();
            assert_eq!(
                backend.state().current().workspaces[0].scrollable_anchor,
                Some(4)
            );
            assert_eq!(backend.state().current().focused_pane, Some(3));

            let mut without_focus = layout_with_panes(&[(1, 0), (2, 0), (4, 0)]);
            without_focus.workspaces[0].layout = SharedLayoutKind::Scrollable;
            backend
                .dispatch_server(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 2,
                    author: 2,
                    layout: without_focus,
                    live: false,
                })
                .expect("remove focused pane");

            assert_eq!(backend.state().current().focused_pane, Some(1));
            assert_eq!(
                backend.state().current().workspaces[0].scrollable_anchor,
                Some(1),
                "fallback focus must become the Scrollable reveal anchor"
            );
            assert_eq!(
                backend.state().current().workspaces[0].scrollable_reveal_edge,
                crate::layout::model::ScrollableRevealEdge::Left
            );
            assert_eq!(
                backend.state().animation,
                crate::layout::anim::GeometryAnimation::Close,
                "removing a pane keeps reconciler Close animation"
            );
            backend.render();
            let visible = {
                let state = backend.state();
                let viewport = state.last_viewport.get().expect("viewport");
                let letterbox = crate::view::follower_letterbox_bounds(state, viewport);
                let local = state.canvas_bounds_from_terminal_viewport(viewport);
                let top_gap = state.workspace_top_gap();
                let tile_letterbox =
                    crate::layout::geometry::workspace_tile_bounds(letterbox, top_gap);
                let tile_local = crate::layout::geometry::workspace_tile_bounds(local, top_gap);
                let left = tile_letterbox.x.max(tile_local.x);
                let right = (tile_letterbox.x + tile_letterbox.w).min(tile_local.x + tile_local.w);
                FloatRect {
                    x: left,
                    y: tile_local.y,
                    w: (right - left).max(0.0),
                    h: tile_local.h,
                }
            };
            let placements = crate::layout::workspace_target_rects_with_visible_bounds(
                &backend.state().current().workspaces[0],
                crate::view::follower_letterbox_bounds(
                    backend.state(),
                    backend.state().last_viewport.get().unwrap(),
                ),
                backend.state().canvas_bounds_from_terminal_viewport(
                    backend.state().last_viewport.get().unwrap(),
                ),
                backend.state().workspace_top_gap(),
                backend.state().tile_gap(),
            );
            let rect = crate::layout::placement_for(&placements, 1).expect("fallback placement");
            assert!(
                (rect.x - visible.x).abs() < 0.5,
                "fallback pane must meet the left visible edge, got {rect:?} vs {visible:?}"
            );
        });
    }

    #[test]
    fn inactive_scrollable_focus_fallback_reveals_on_workspace_switch() {
        in_stack(|| {
            use crate::layout::model::LayoutKind;
            use crate::ops::focus::switch_workspace;

            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            let (client, _rx) = SessionClient::test_channel();
            attach_follower(&mut backend, client);
            backend.render();

            // Seed scrollable content on workspace 1 while staying active on 0.
            let mut layout = SharedLayout {
                version: SHARED_LAYOUT_VERSION,
                pane_geometry: Default::default(),
                canvas_cols: 100,
                canvas_rows: 28,
                workspaces: vec![
                    SharedWorkspace {
                        index: 0,
                        name: None,
                        synchronized: false,
                        layout: SharedLayoutKind::Dwindle,
                        start_axis: SharedSplitAxis::Horizontal,
                        split_ratios: Vec::new(),
                        tree: None,
                        panes: vec![SharedPane {
                            pane_id: 99,
                            generation: 0,
                            title: None,
                            profile_name: None,
                            cwd: None,
                            launch: None,
                            replay: false,
                            keep_open: false,
                            floating: false,
                            pinned: false,
                            fullscreen: false,
                            rect: None,
                            scrollable_width: crate::layout::model::DEFAULT_SCROLLABLE_WIDTH,
                        }],
                    },
                    SharedWorkspace {
                        index: 1,
                        name: None,
                        synchronized: false,
                        layout: SharedLayoutKind::Scrollable,
                        start_axis: SharedSplitAxis::Horizontal,
                        split_ratios: Vec::new(),
                        tree: None,
                        panes: [1, 2, 3, 4]
                            .into_iter()
                            .map(|pane_id| SharedPane {
                                pane_id,
                                generation: 0,
                                title: None,
                                profile_name: None,
                                cwd: None,
                                launch: None,
                                replay: false,
                                keep_open: false,
                                floating: false,
                                pinned: false,
                                fullscreen: false,
                                rect: None,
                                scrollable_width: crate::layout::model::DEFAULT_SCROLLABLE_WIDTH,
                            })
                            .collect(),
                    },
                ],
            };
            backend
                .dispatch_server(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout: layout.clone(),
                    live: false,
                })
                .expect("seed multi-ws");
            assert_eq!(backend.state().current().active_workspace, 0);

            // Prepare ws1 viewport state as if it had been right-scrolled with focus on 3.
            {
                let state = backend.state_mut();
                state.current_mut().active_workspace = 1;
                focus_pane(state, 4);
                state.animation = crate::layout::anim::GeometryAnimation::None;
            }
            backend.render();
            {
                let state = backend.state_mut();
                focus_pane(state, 3);
                state.animation = crate::layout::anim::GeometryAnimation::None;
                state.current_mut().active_workspace = 0;
                state.current_mut().focused_pane = Some(99);
                state.current_mut().workspaces[0].focused_pane = Some(99);
            }
            assert_eq!(
                backend.state().current().workspaces[1].scrollable_anchor,
                Some(4)
            );
            assert_eq!(
                backend.state().current().workspaces[1].focused_pane,
                Some(3)
            );

            // Remove focused pane 3 on inactive ws1; anchor 4 survives; active stays 0 so
            // reconcile does not sync reveal for ws1.
            layout.workspaces[1].panes.retain(|pane| pane.pane_id != 3);
            backend
                .dispatch_server(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 2,
                    author: 2,
                    layout,
                    live: false,
                })
                .expect("inactive focus fallback");
            assert_eq!(backend.state().current().active_workspace, 0);
            assert_eq!(
                backend.state().current().workspaces[1].focused_pane,
                Some(1)
            );
            assert_eq!(
                backend.state().current().workspaces[1].scrollable_anchor,
                Some(4),
                "inactive ws keeps surviving right anchor until activation"
            );

            switch_workspace(backend.state_mut(), 1);
            assert_eq!(backend.state().current().active_workspace, 1);
            assert_eq!(
                backend.state().animation,
                crate::layout::anim::GeometryAnimation::None
            );
            assert_eq!(backend.state().current().focused_pane, Some(1));
            assert_eq!(
                backend.state().current().workspaces[1].scrollable_anchor,
                Some(1)
            );
            assert_eq!(
                backend.state().current().workspaces[1].scrollable_reveal_edge,
                crate::layout::model::ScrollableRevealEdge::Left
            );
            assert_eq!(
                backend.state().current().workspaces[1].layout_kind,
                LayoutKind::Scrollable
            );
            backend.render();
            let visible = {
                let state = backend.state();
                let viewport = state.last_viewport.get().expect("viewport");
                let letterbox = crate::view::follower_letterbox_bounds(state, viewport);
                let local = state.canvas_bounds_from_terminal_viewport(viewport);
                let top_gap = state.workspace_top_gap();
                let tile_letterbox =
                    crate::layout::geometry::workspace_tile_bounds(letterbox, top_gap);
                let tile_local = crate::layout::geometry::workspace_tile_bounds(local, top_gap);
                let left = tile_letterbox.x.max(tile_local.x);
                let right = (tile_letterbox.x + tile_letterbox.w).min(tile_local.x + tile_local.w);
                FloatRect {
                    x: left,
                    y: tile_local.y,
                    w: (right - left).max(0.0),
                    h: tile_local.h,
                }
            };
            let placements = crate::layout::workspace_target_rects_with_visible_bounds(
                &backend.state().current().workspaces[1],
                crate::view::follower_letterbox_bounds(
                    backend.state(),
                    backend.state().last_viewport.get().unwrap(),
                ),
                backend.state().canvas_bounds_from_terminal_viewport(
                    backend.state().last_viewport.get().unwrap(),
                ),
                backend.state().workspace_top_gap(),
                backend.state().tile_gap(),
            );
            let rect = crate::layout::placement_for(&placements, 1).expect("fallback");
            assert!(
                (rect.x - visible.x).abs() < 0.5,
                "switch must left-align inactive fallback, got {rect:?} vs {visible:?}"
            );
        });
    }

    #[test]
    fn follower_reconcile_retains_scrollable_width_through_reemit() {
        in_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            let (client, _rx) = SessionClient::test_channel();
            attach_follower(&mut backend, client);
            backend.render();

            let mut layout = layout_with_panes(&[(1, 0), (2, 0)]);
            layout.workspaces[0].layout = SharedLayoutKind::Scrollable;
            layout.workspaces[0].panes[0].scrollable_width = 0.62;
            layout.workspaces[0].panes[1].scrollable_width = 9.0; // sanitize to MAX
            backend
                .dispatch_server(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout,
                    live: false,
                })
                .expect("commit scrollable widths");

            let ws = &backend.state().current().workspaces[0];
            assert_eq!(ws.layout_kind, crate::layout::model::LayoutKind::Scrollable);
            assert!((ws.panes[0].scrollable_width - 0.62).abs() < 1e-6);
            assert_eq!(
                ws.panes[1].scrollable_width,
                crate::layout::model::MAX_SPLIT_RATIO,
                "follower applies sanitize at the state boundary"
            );

            let reemitted = shared_layout_from_state(backend.state(), (100, 28));
            assert_eq!(reemitted.version, SHARED_LAYOUT_VERSION);
            assert_eq!(reemitted.workspaces[0].layout, SharedLayoutKind::Scrollable);
            assert!(
                (reemitted.workspaces[0].panes[0].scrollable_width - 0.62).abs() < 1e-6,
                "commit → follower → re-serialize must retain non-default width"
            );
            assert_eq!(
                reemitted.workspaces[0].panes[1].scrollable_width,
                crate::layout::model::MAX_SPLIT_RATIO
            );
        });
    }

    #[test]
    fn shared_layout_reconcile_preserves_directional_focus_hint() {
        in_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            let (client, _rx) = SessionClient::test_channel();
            attach_follower(&mut backend, client);
            backend.render();

            let layout = layout_with_panes(&[(1, 0), (2, 0)]);
            backend
                .dispatch_server(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout: layout.clone(),
                    live: false,
                })
                .expect("seed shared layout");

            let hint = DirectionalFocusHint {
                pane: 1,
                entry_direction: Direction::Left,
                target: 2,
            };
            backend.state_mut().current_mut().workspaces[0].last_directional_focus = Some(hint);

            backend
                .dispatch_server(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 2,
                    author: 2,
                    layout,
                    live: false,
                })
                .expect("reconcile shared layout");

            assert_eq!(
                backend.state().current().workspaces[0].last_directional_focus,
                Some(hint)
            );
        });
    }

    #[test]
    fn directional_focus_keeps_entry_row_across_shared_reconciles() {
        in_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            let (client, _rx) = SessionClient::test_channel();
            attach_follower(&mut backend, client);
            backend.render();

            let mut layout = layout_with_panes(&[(1, 0), (2, 0), (3, 0), (4, 0)]);
            layout.workspaces[0].tree = Some(SharedTree::Split {
                axis: SharedSplitAxis::Horizontal,
                ratio: 0.5,
                first: Box::new(SharedTree::Leaf { pane: 1 }),
                second: Box::new(SharedTree::Split {
                    axis: SharedSplitAxis::Vertical,
                    ratio: 0.5,
                    first: Box::new(SharedTree::Leaf { pane: 2 }),
                    second: Box::new(SharedTree::Split {
                        axis: SharedSplitAxis::Horizontal,
                        ratio: 0.5,
                        first: Box::new(SharedTree::Leaf { pane: 3 }),
                        second: Box::new(SharedTree::Leaf { pane: 4 }),
                    }),
                }),
            });
            backend
                .dispatch_server(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout: layout.clone(),
                    live: false,
                })
                .expect("seed shared layout");

            backend.state_mut().current_mut().focused_pane = Some(4);
            backend.state_mut().current_mut().workspaces[0].focused_pane = Some(4);
            for (rev, expected) in [(2, 3), (3, 1), (4, 4)] {
                backend
                    .dispatch(Msg::RunAction(Action::Focus(Direction::Left)))
                    .expect("focus left");
                assert_eq!(backend.state().current().focused_pane, Some(expected));
                backend
                    .dispatch_server(Msg::SessionLayoutCommitted {
                        epoch: 0,
                        rev,
                        author: 2,
                        layout: layout.clone(),
                        live: false,
                    })
                    .expect("reconcile shared layout");
                assert_eq!(
                    backend.state().current().workspaces[0].last_directional_focus,
                    Some(DirectionalFocusHint {
                        pane: expected,
                        entry_direction: Direction::Left,
                        target: match expected {
                            3 => 4,
                            1 => 3,
                            4 => 1,
                            _ => unreachable!(),
                        },
                    }),
                    "directional hint changed after revision {rev}"
                );
            }

            backend.state_mut().current_mut().focused_pane = Some(3);
            backend.state_mut().current_mut().workspaces[0].focused_pane = Some(3);
            backend.state_mut().current_mut().workspaces[0].last_directional_focus = None;
            for (rev, expected) in [(5, 4), (6, 1), (7, 3)] {
                backend
                    .dispatch(Msg::RunAction(Action::Focus(Direction::Right)))
                    .expect("focus right");
                assert_eq!(backend.state().current().focused_pane, Some(expected));
                backend
                    .dispatch_server(Msg::SessionLayoutCommitted {
                        epoch: 0,
                        rev,
                        author: 2,
                        layout: layout.clone(),
                        live: false,
                    })
                    .expect("reconcile shared layout");
            }
        });
    }

    fn commit(backend: &mut TestBackend<AppRoot>, rev: u64, layout: SharedLayout) {
        backend
            .dispatch_server(Msg::SessionLayoutCommitted {
                epoch: 0,
                rev,
                author: 2,
                layout,
                live: false,
            })
            .expect("commit shared layout");
    }

    fn terminal_rect(backend: &TestBackend<AppRoot>, id: PaneId) -> Rect {
        let pane = backend.state().current().workspaces[0]
            .panes
            .iter()
            .find(|pane| pane.id == id)
            .unwrap();
        backend
            .capture_ui_snapshot()
            .widgets
            .iter()
            .find(|widget| widget.key.as_ref() == Some(&pane.keys.terminal))
            .expect("terminal widget")
            .rect
    }

    #[test]
    fn follower_preserves_controller_terminal_heights() {
        in_stack(|| {
            let mut controller = TestBackend::new(AppRoot::default());
            controller.set_viewport(VIEWPORT);
            let (client, _rx) = SessionClient::test_channel();
            attach_follower(&mut controller, client);
            let mut layout = layout_with_panes(&[(1, 1), (2, 1), (3, 1)]);
            layout.canvas_rows = controller
                .state()
                .canvas_bounds_from_terminal_viewport(VIEWPORT)
                .h as u16;
            layout.workspaces[0].layout = SharedLayoutKind::Rows;
            commit(&mut controller, 1, layout.clone());
            controller.render();
            controller.advance(std::time::Duration::from_secs(1));
            let grids: Vec<_> = (1..=3).map(|id| terminal_rect(&controller, id)).collect();
            for viewport in [
                Rect {
                    w: 140,
                    h: 45,
                    ..VIEWPORT
                },
                Rect {
                    w: 141,
                    h: 46,
                    ..VIEWPORT
                },
                Rect {
                    w: 81,
                    h: 23,
                    ..VIEWPORT
                },
            ] {
                let mut follower = TestBackend::new(AppRoot::default());
                follower.set_viewport(viewport);
                follower.state_mut().config.pane.padding = (1, 2, 1, 2);
                follower.state_mut().config.pane.titlebar =
                    crate::pane::chrome::PaneTitlebarMode::Integrated;
                follower.state_mut().config.pane.border_mode =
                    crate::pane::chrome::PaneBorderMode::Merged;
                follower.state_mut().config.pane.workbar_gap = false;
                let (client, _rx) = SessionClient::test_channel();
                attach_follower(&mut follower, client);
                commit(&mut follower, 1, layout.clone());
                follower.render();
                follower.advance(std::time::Duration::from_secs(1));
                assert_eq!(follower.state().config.pane.padding, (1, 2, 1, 2));
                for (id, expected) in (1..=3).zip(&grids) {
                    let rect = terminal_rect(&follower, id);
                    assert_eq!(
                        (rect.w, rect.h),
                        (expected.w, expected.h),
                        "pane {id} at {viewport:?}"
                    );
                }
                follower
                    .state_mut()
                    .current_mut()
                    .shared
                    .as_mut()
                    .unwrap()
                    .controller = Some(1);
                assert_eq!(
                    follower.state().workspace_pane_config().padding,
                    (1, 2, 1, 2)
                );
            }
        });
    }

    #[test]
    fn follower_float_uses_letterbox_origin_and_close_keeps_tile_placement() {
        in_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            let viewport = Rect {
                w: 140,
                h: 45,
                ..VIEWPORT
            };
            backend.set_viewport(viewport);
            let (client, _rx) = SessionClient::test_channel();
            attach_follower(&mut backend, client);
            let mut layout = layout_with_panes(&[(1, 1), (2, 1), (3, 1)]);
            layout.workspaces[0].panes[2].floating = true;
            layout.workspaces[0].panes[2].rect = Some(FracRect {
                x: 0.0,
                y: 0.0,
                w: 0.4,
                h: 0.5,
            });
            commit(&mut backend, 1, layout.clone());
            backend.render();
            backend.advance(std::time::Duration::from_secs(1));
            let bounds = crate::view::follower_letterbox_bounds(backend.state(), viewport);
            let before = crate::view::settled_active_pane_rects(backend.state(), viewport);
            let float = before.iter().find(|(id, _)| *id == 3).unwrap().1;
            assert_eq!(float.x, bounds.x);
            assert_eq!(
                float.y,
                bounds.y + f32::from(backend.state().content_top_offset())
            );
            let tile = before.iter().find(|(id, _)| *id == 2).unwrap().1;
            layout.workspaces[0].panes.retain(|pane| pane.pane_id != 2);
            commit(&mut backend, 2, layout);
            let pane = find_pane(backend.state(), 2).unwrap();
            assert!(pane.closing);
            let saved = pane.floating_rect;
            assert_eq!(
                (
                    saved.x + bounds.x,
                    saved.y + bounds.y + f32::from(backend.state().content_top_offset()),
                    saved.w,
                    saved.h
                ),
                (tile.x, tile.y, tile.w, tile.h)
            );
        });
    }

    #[test]
    fn follower_additions_use_open_lifecycle_after_the_initial_seed() {
        in_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            let (client, _rx) = SessionClient::test_channel();
            attach_follower(&mut backend, client);
            backend.state_mut().current_mut().workspaces[0]
                .panes
                .clear();
            commit(&mut backend, 1, layout_with_panes(&[(1, 1)]));
            backend.render();
            assert!(!find_pane(backend.state(), 1).unwrap().opening);
            commit(&mut backend, 2, layout_with_panes(&[(1, 1), (2, 1)]));
            backend.render();
            let pane = find_pane(backend.state(), 2).unwrap();
            assert!(pane.opening && pane.opening_animation.is_some() && !pane.terminal_active);
            backend.dispatch(Msg::FinishOpen(0, 2, 1)).unwrap();
            assert!(!find_pane(backend.state(), 2).unwrap().opening);
            backend.dispatch(Msg::ActivatePane(0, 2, 1)).unwrap();
            assert!(find_pane(backend.state(), 2).unwrap().terminal_active);
        });
    }

    #[test]
    fn initial_session_layout_is_applied_without_geometry_transition() {
        in_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            let (client, _rx) = SessionClient::test_channel();
            backend.state_mut().current_mut().pending_session_attach =
                Some(crate::state::PendingSessionAttach {
                    epoch: 1,
                    request: crate::state::AttachRequest::new(
                        "live",
                        crate::state::AttachMode::Join,
                    ),
                    client: Some(client),
                    left: None,
                    parked_epoch: None,
                });
            backend.render();

            backend
                .dispatch_server(Msg::SessionAttached {
                    epoch: 1,
                    session_instance: crate::session::protocol::SessionInstanceId::for_test("live"),
                    session: "live".into(),
                    client_id: 1,
                    panes: Vec::new(),
                    layout_rev: 7,
                    layout: Some(layout_with_panes(&[(1, 4), (2, 9)])),
                    controller: Some(1),
                    clients: Vec::new(),
                    input_locked: false,
                    allow_takeover: false,
                    read_only: false,
                    origin: crate::session::origin::SessionOrigin::default(),
                })
                .expect("dispatch attach");

            let state = backend.state_mut();
            assert!(find_pane(state, 1).is_some());
            assert!(find_pane(state, 2).is_some());
            assert_eq!(
                state.animation,
                crate::layout::anim::GeometryAnimation::None
            );
        });
    }

    fn attach_message(epoch: u64, session: &str, panes: &[(PaneId, u64)]) -> Msg {
        Msg::SessionAttached {
            epoch,
            session_instance: crate::session::protocol::SessionInstanceId::for_test(session),
            session: session.into(),
            client_id: 1,
            panes: Vec::new(),
            layout_rev: 1,
            layout: Some(layout_with_panes(panes)),
            controller: Some(1),
            clients: Vec::new(),
            input_locked: false,
            allow_takeover: false,
            read_only: false,
            origin: crate::session::origin::SessionOrigin::default(),
        }
    }

    fn pend_attach(backend: &mut TestBackend<AppRoot>, epoch: u64, name: &str) {
        let (client, _rx) = SessionClient::test_channel();
        let state = backend.state_mut();
        state.runtime_epoch = epoch;
        state.current_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
            epoch,
            request: crate::state::AttachRequest::new(name, crate::state::AttachMode::Join),
            client: Some(client),
            left: None,
            parked_epoch: None,
        });
        state.current_mut().connection = crate::state::ConnectionState::Connecting;
    }

    fn frame_colors(
        backend: &TestBackend<AppRoot>,
    ) -> Vec<(tui_lipan::prelude::Color, tui_lipan::prelude::Color)> {
        backend
            .capture_frame()
            .cells
            .into_iter()
            .map(|cell| (cell.fg, cell.bg))
            .collect()
    }

    /// A cold attach goes session -> Connecting -> session. The session that lands reuses the
    /// outgoing session's pane ids, and so its chrome animation keys; its first frame must already
    /// wear its settled chrome, with nothing left to fade in afterwards.
    #[test]
    fn a_cold_attach_lands_with_settled_chrome() {
        in_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            backend.state_mut().config.animations.session =
                crate::layout::anim::SessionAnimationStyle::Off;
            pend_attach(&mut backend, 1, "a");
            backend.render();
            backend
                .dispatch_server(attach_message(1, "a", &[(1, 1), (2, 2)]))
                .expect("attach a");
            backend.state_mut().current_mut().workspaces[0].focused_pane = Some(2);
            backend.state_mut().current_mut().focused_pane = Some(2);
            backend.render();
            backend.advance(std::time::Duration::from_secs(1));

            let parked = backend.state().runtime_epoch;
            backend
                .state_mut()
                .park_current(parked, crate::state::Attachment::new());
            pend_attach(&mut backend, 2, "b");
            backend.render();
            backend.advance(std::time::Duration::from_millis(300));

            backend
                .dispatch_server(attach_message(2, "b", &[(1, 3), (2, 4)]))
                .expect("attach b");
            backend.render();
            let first = frame_colors(&backend);
            backend.advance(std::time::Duration::from_secs(1));
            let settled = frame_colors(&backend);
            let width = usize::from(VIEWPORT.w);
            let changed: Vec<_> = first
                .iter()
                .zip(&settled)
                .enumerate()
                .filter(|(_, (a, b))| a != b)
                .map(|(index, (a, b))| ((index % width, index / width), *a, *b))
                .collect();
            assert!(
                changed.is_empty(),
                "{} cells still settling after the attach: {:?}",
                changed.len(),
                changed.iter().take(12).collect::<Vec<_>>()
            );
        });
    }

    fn screen_text(backend: &TestBackend<AppRoot>) -> String {
        backend.capture_frame().to_fixed_grid_lines().join("\n")
    }

    /// A local attach lands in tens of milliseconds. For that long the previous session's picture
    /// stays up instead of a Connecting scene that would only flash; a slow attach still gets one.
    #[test]
    fn a_fresh_attach_holds_the_previous_picture_before_admitting_it_is_connecting() {
        in_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            pend_attach(&mut backend, 1, "a");
            backend.render();
            backend
                .dispatch_server(attach_message(1, "a", &[(1, 1), (2, 2)]))
                .expect("attach a");
            backend.render();
            backend.advance(std::time::Duration::from_secs(1));
            let settled = screen_text(&backend);
            assert!(!settled.contains("Connecting"), "{settled}");

            let parked = backend.state().runtime_epoch;
            backend
                .state_mut()
                .park_current(parked, crate::state::Attachment::new());
            pend_attach(&mut backend, 2, "b");
            backend.state_mut().connect_hold = Some(crate::state::ConnectHold {
                epoch: 2,
                active: true,
            });
            backend.render();
            let held = screen_text(&backend);
            assert!(!held.contains("Connecting"), "{held}");
            assert_eq!(
                held, settled,
                "the outgoing picture should stand in unchanged"
            );

            backend
                .dispatch(Msg::ConnectHoldElapsed(2))
                .expect("hold elapsed");
            backend.render();
            let connecting = screen_text(&backend);
            assert!(connecting.contains("Connecting"), "{connecting}");
        });
    }

    #[test]
    fn own_commit_echo_confirms_rev_without_reapplying() {
        in_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            let (client, _rx) = SessionClient::test_channel();
            {
                let state = backend.state_mut();
                state.current_mut().session_attached = true;
                state.current_mut().session_client = Some(client);
                let mut shared = SharedSessionState::new(1);
                shared.controller = Some(1); // we are the controller: our own echoes must not apply.
                shared.clients = vec![
                    crate::session::protocol::ClientInfo {
                        id: 1,
                        label: "a".into(),
                        read_only: false,
                        requesting_control: false,
                        parked: false,
                    },
                    crate::session::protocol::ClientInfo {
                        id: 2,
                        label: "b".into(),
                        read_only: false,
                        requesting_control: false,
                        parked: false,
                    },
                ];
                state.current_mut().shared = Some(shared);
            }
            backend.render();

            // An echo authored by us carries a layout with no panes; it must be ignored.
            backend
                .dispatch_server(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 7,
                    author: 1,
                    layout: layout_with_panes(&[]),
                    live: false,
                })
                .expect("dispatch echo");

            assert!(
                find_pane(backend.state_mut(), 1).is_some(),
                "own echo must not remove local panes"
            );
            assert_eq!(
                backend
                    .state_mut()
                    .current_mut()
                    .shared
                    .as_ref()
                    .unwrap()
                    .layout_rev,
                7,
                "echo confirms the committed revision"
            );
        });
    }

    #[test]
    fn a_shared_float_remembers_its_size_after_a_shared_return_to_tiling() {
        let mut state = state_with_split();
        state.current_mut().workspaces[0].panes[0].floating = true;
        let layout = shared_layout_from_state(&state, (80, 24));
        let mut shared = layout.workspaces[0].panes[0].clone();
        let remembered = FloatRect {
            x: 5.0,
            y: 3.0,
            w: 30.0,
            h: 10.0,
        };
        let mut pane = Pane::new(1, 100, FloatRect::default());
        apply_shared_pane_fields(&mut pane, &shared, remembered);
        shared.floating = false;
        apply_shared_pane_fields(&mut pane, &shared, FloatRect::default());
        assert!(pane.floating_rect_initialized);
        assert_eq!(pane.floating_rect, remembered);
    }

    #[test]
    fn shared_pane_scrollable_width_round_trips_and_sanitizes() {
        let mut pane = SharedPane {
            pane_id: 1,
            generation: 1,
            title: None,
            profile_name: None,
            cwd: None,
            launch: None,
            replay: false,
            keep_open: false,
            floating: false,
            pinned: false,
            fullscreen: false,
            rect: None,
            scrollable_width: 0.67,
        };
        let encoded = serde_json::to_value(&pane).unwrap();
        assert!((encoded["scrollable_width"].as_f64().unwrap() - 0.67).abs() < 1e-6);
        let decoded: SharedPane = serde_json::from_value(encoded).unwrap();
        assert!((decoded.scrollable_width - 0.67).abs() < 1e-6);

        pane.scrollable_width = f32::NAN;
        let mut runtime = Pane::new(
            1,
            100,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        );
        apply_shared_pane_fields(
            &mut runtime,
            &pane,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        );
        assert_eq!(
            runtime.scrollable_width,
            crate::layout::model::DEFAULT_SCROLLABLE_WIDTH
        );
        assert_eq!(SHARED_LAYOUT_VERSION, 4);
    }
}
