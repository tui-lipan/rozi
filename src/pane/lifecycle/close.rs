use std::time::Duration;
use tui_lipan::prelude::*;

use crate::layout::anim::{self, GeometryAnimation};
use crate::layout::tiling::remove_tiled_window;
use crate::ops::focus::{
    choose_fallback_focus, choose_fallback_focus_near, focus_pane, request_current_pane_focus,
    scrollable_close_neighbor,
};
use crate::pane::lifecycle::namespace::{
    clear_pane_local_state, find_pane, find_pane_in_namespace_mut, find_pane_mut, pane_is_local,
    remove_pane,
};
use crate::state::PaneId;
use crate::{AppRoot, Msg};

/// Kill a live workspace pane and start its close animation.
///
/// The pane leaves the tiling layout immediately, so its neighbours begin expanding at once, but
/// stays in `panes` marked [`Pane::closing`] until [`Msg::PruneClosed`] drops it. It has to stay
/// described for the close animation to exist at all: the pane scales toward its centre, which
/// means the whole subtree is re-laid out every frame at a shrinking rectangle. Framework-side
/// retention (`Animated::auto_exit`) cannot do this, because it freezes the already reconciled
/// subtree and only clips it.
pub(crate) fn close_pane(ctx: &mut Context<AppRoot>, id: PaneId) -> Update {
    let scratch = crate::scratchpad::contains(&ctx.state, id);
    match close_pane_inner(ctx, id, true) {
        Some(generation) => Update::with_command(prune_closed_command(
            ctx.state.runtime_epoch,
            id,
            generation,
            if scratch && ctx.state.scratch.panes.iter().all(|pane| pane.closing) {
                anim::retained_pane_timeout(ctx.state.config.animations).max(
                    anim::scratch_transition_duration(
                        ctx.state.config.animations.geometry_duration,
                    ),
                )
            } else {
                anim::retained_pane_timeout(ctx.state.config.animations)
            },
        )),
        None => Update::full(),
    }
}

/// Start the close animation for a pane whose server-side process has already exited.
pub(crate) fn remove_pane_after_exit(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    local: bool,
) -> Update {
    let scratch = local && crate::scratchpad::contains(&ctx.state, id);
    match close_pane_inner_with_focus(ctx, id, false, true, Some(local)) {
        Some(generation) => Update::with_command(prune_closed_command(
            ctx.state.runtime_epoch,
            id,
            generation,
            if scratch && ctx.state.scratch.panes.iter().all(|pane| pane.closing) {
                anim::retained_pane_timeout(ctx.state.config.animations).max(
                    anim::scratch_transition_duration(
                        ctx.state.config.animations.geometry_duration,
                    ),
                )
            } else {
                anim::retained_pane_timeout(ctx.state.config.animations)
            },
        )),
        None => Update::full(),
    }
}

/// Mark a pane closing without scheduling its prune. Callers closing one pane wrap the returned
/// generation in [`prune_closed_command`]; callers closing several at once collect generations and
/// schedule one [`prune_closed_batch_command`], since an [`Update`] carries only one [`Command`].
/// Returns `None` when the pane is unknown or already closing.
pub(crate) fn close_pane_inner(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    kill_server_pane: bool,
) -> Option<u64> {
    close_pane_inner_with_focus(ctx, id, kill_server_pane, true, None)
}

/// Mark and kill one pane for a batch teardown without resolving focus. The caller must resolve
/// focus after all panes in the batch have been marked closing; otherwise each pane can select a
/// neighbour that the next iteration immediately closes.
pub(crate) fn close_pane_inner_without_focus(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    kill_server_pane: bool,
) -> Option<u64> {
    close_pane_inner_with_focus(ctx, id, kill_server_pane, false, None)
}

fn close_scratch_pane(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    kill_server_pane: bool,
    resolve_focus: bool,
) -> Option<u64> {
    let bounds = crate::scratchpad::deployed_rect(&ctx.state, ctx.viewport());
    let placements = crate::layout::workspace_target_rects(
        &ctx.state.scratch,
        bounds,
        0.0,
        ctx.state.tile_gap(),
    );
    let client = ctx.state.scratch_client();
    let was_focused = ctx.state.scratch.focused_pane == Some(id);
    let scrollable_neighbor = (ctx.state.scratch.layout_kind
        == crate::state::LayoutKind::Scrollable)
        .then(|| scrollable_close_neighbor(&ctx.state.scratch, id))
        .flatten();
    let pane = ctx
        .state
        .scratch
        .panes
        .iter_mut()
        .find(|pane| pane.id == id && !pane.closing)?;
    let generation = pane.pty_generation;
    if kill_server_pane && let Some(client) = client {
        client.kill(id, generation, true);
    }
    pane.floating_rect = crate::layout::placement_for(&placements, id).unwrap_or(bounds);
    pane.opening = false;
    pane.closing = true;
    pane.terminal.kill();
    remove_tiled_window(&mut ctx.state.scratch, id);

    if was_focused {
        match scrollable_neighbor {
            Some(target) => focus_pane(&mut ctx.state, target),
            None => choose_fallback_focus_near(&mut ctx.state, Some(id), None),
        }
    }
    ctx.state.animation = GeometryAnimation::Close;
    if ctx.state.scratch.focused_pane.is_none() {
        crate::scratchpad::after_pane_removed(ctx);
    } else if resolve_focus {
        request_current_pane_focus(ctx);
    }
    Some(generation)
}

#[derive(Default)]
struct WorkspaceCloseFocus {
    active_pane: bool,
    neighbor: Option<PaneId>,
    anchor_remap: Option<(usize, Option<PaneId>, crate::state::ScrollableRevealEdge)>,
}

fn plan_workspace_close_focus(
    state: &crate::state::State,
    id: PaneId,
    resolve_focus: bool,
) -> WorkspaceCloseFocus {
    if !resolve_focus {
        return WorkspaceCloseFocus::default();
    }
    let attachment = state.current();
    let active_workspace = attachment.active_workspace;
    let owner_workspace = attachment
        .workspaces
        .iter()
        .position(|workspace| workspace.panes.iter().any(|pane| pane.id == id));
    let active_pane =
        owner_workspace == Some(active_workspace) && attachment.focused_pane == Some(id);
    let neighbor = owner_workspace
        .and_then(|workspace| scrollable_close_neighbor(&attachment.workspaces[workspace], id));
    let anchor_remap = owner_workspace.and_then(|workspace_index| {
        let workspace = &attachment.workspaces[workspace_index];
        (workspace.layout_kind == crate::state::LayoutKind::Scrollable
            && workspace.scrollable_anchor == Some(id))
        .then_some((workspace_index, neighbor, workspace.scrollable_reveal_edge))
    });
    WorkspaceCloseFocus {
        active_pane,
        neighbor,
        anchor_remap,
    }
}

fn mark_workspace_pane_closing(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    kill_server_pane: bool,
    namespace: Option<bool>,
) -> Option<u64> {
    let bounds = ctx
        .state
        .canvas_bounds_from_terminal_viewport(ctx.viewport());
    let placements = {
        let attachment = ctx.state.current();
        let workspace = &attachment.workspaces[attachment.active_workspace];
        crate::layout::workspace_target_rects(
            workspace,
            bounds,
            ctx.state.workspace_top_gap(),
            ctx.state.tile_gap(),
        )
    };
    let client = ctx.state.current().session_client.clone();
    let wire_local = namespace.unwrap_or_else(|| pane_is_local(&ctx.state, id));
    let pane = match namespace {
        Some(false) => find_pane_in_namespace_mut(&mut ctx.state, id, false),
        _ => find_pane_mut(&mut ctx.state, id),
    }?;
    if pane.closing {
        return None;
    }
    let generation = pane.pty_generation;
    if kill_server_pane && let Some(client) = client {
        client.kill(id, generation, wire_local);
    }
    pane.floating_rect =
        crate::layout::placement_for(&placements, id).unwrap_or(pane.floating_rect);
    pane.opening = false;
    pane.closing = true;
    pane.terminal.kill();
    Some(generation)
}

fn repair_workspace_focus(ctx: &mut Context<AppRoot>, plan: WorkspaceCloseFocus) {
    if plan.active_pane {
        if let Some(target) = plan.neighbor {
            focus_pane(&mut ctx.state, target);
        } else {
            choose_fallback_focus(&mut ctx.state);
        }
    }
    if let Some((workspace, anchor, edge)) = plan.anchor_remap
        && (!plan.active_pane || anchor.is_none())
    {
        ctx.state.current_mut().workspaces[workspace].set_scrollable_viewport(anchor, edge);
    }
    // Focus synchronization may arm AxisChange, but the retained pane needs the close transition.
    ctx.state.animation = GeometryAnimation::Close;
    request_current_pane_focus(ctx);
}

pub(crate) fn close_pane_inner_with_focus(
    ctx: &mut Context<AppRoot>,
    id: PaneId,
    kill_server_pane: bool,
    resolve_focus: bool,
    namespace: Option<bool>,
) -> Option<u64> {
    let in_scratch = crate::scratchpad::contains(&ctx.state, id);
    if namespace != Some(false) && in_scratch {
        return close_scratch_pane(ctx, id, kill_server_pane, resolve_focus);
    }
    if namespace == Some(true) {
        return None;
    }
    // Plan before marking the pane closing, which removes it from Scrollable's visual order.
    let focus = plan_workspace_close_focus(&ctx.state, id, resolve_focus);
    let generation = mark_workspace_pane_closing(ctx, id, kill_server_pane, namespace)?;
    ctx.state.animation = GeometryAnimation::Close;
    if resolve_focus {
        repair_workspace_focus(ctx, focus);
    }
    Some(generation)
}

/// Drop a pane once its close animation has run, if it is still the same closing pane.
pub(crate) fn prune_closed_pane(
    ctx: &mut Context<AppRoot>,
    epoch: u64,
    id: PaneId,
    generation: u64,
) -> Update {
    if epoch != ctx.state.runtime_epoch {
        return Update::none();
    }
    let still_closing = find_pane(&ctx.state, id)
        .is_some_and(|pane| pane.pty_generation == generation && pane.closing);
    if !still_closing {
        return Update::none();
    }
    if ctx.state.popup.as_ref().is_some_and(|pane| pane.id == id) {
        ctx.state.popup = None;
    } else if let Some(index) = ctx
        .state
        .scratch
        .panes
        .iter()
        .position(|pane| pane.id == id)
    {
        ctx.state.scratch.panes.remove(index);
        remove_tiled_window(&mut ctx.state.scratch, id);
        crate::scratchpad::after_pane_removed(ctx);
    } else {
        let timeout = crate::layout::anim::retained_pane_timeout(ctx.state.config.animations);
        // Take the pane out first so its terminal screen can be retired: a same-generation
        // reintroduction (a layout correction) restores its scrollback instead of starting blank.
        let removed = ctx
            .state
            .current_mut()
            .workspaces
            .iter_mut()
            .find_map(|ws| {
                ws.panes
                    .iter()
                    .position(|pane| pane.id == id)
                    .map(|index| ws.panes.remove(index))
            });
        remove_pane(&mut ctx.state, id);
        if let Some(pane) = removed {
            clear_pane_local_state(&mut ctx.state, id);
            ctx.state.current_mut().retire_pane(pane, timeout);
        }
    }
    Update::full()
}

pub(crate) fn prune_closed_command(
    epoch: u64,
    id: PaneId,
    generation: u64,
    delay: Duration,
) -> Command {
    Command::after(delay, move |link: CommandLink<Msg>| {
        link.send(Msg::PruneClosed(epoch, id, generation));
    })
}

/// Prune several panes closed in the same batch (e.g. [`crate::ops::exit::kill_workspace`]) after
/// one shared delay, since an [`Update`] can only carry a single [`Command`].
pub(crate) fn prune_closed_batch_command(
    epoch: u64,
    targets: Vec<(PaneId, u64)>,
    delay: Duration,
) -> Command {
    Command::after(delay, move |link: CommandLink<Msg>| {
        for (id, generation) in targets {
            link.send(Msg::PruneClosed(epoch, id, generation));
        }
    })
}
