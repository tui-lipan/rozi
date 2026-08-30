//! Server-authoritative shared layout: the structured document that describes a named session's
//! window-manager state (workspace membership + order, tiling trees + ratios, layout kind, start
//! axis, floating/fullscreen geometry, workspace names, sync flag, and pane identity). It is the
//! only layout representation on the wire - profile TOML remains the on-disk format for
//! profiles/autosave, but nothing in the session protocol uses TOML.
//!
//! A controller client commits a [`SharedLayout`] on every layout change; the server bumps a
//! revision and broadcasts it, and every follower reconciles its local `State` toward the document
//! via [`apply_shared_layout`] without touching live terminal screens.

use serde::{Deserialize, Serialize};
use tui_lipan::prelude::*;

use crate::layout_tree_ser::{
    SerializedLayoutKind, SerializedSplitAxis, SerializedTree, from_dwindle, to_dwindle,
};
use crate::state::{PaneId, State};
use crate::tiling::DwindleTree;

/// Stable identity for an attached client, assigned by the server on attach.
pub type ClientId = u64;

/// Wire-format version for [`SharedLayout`]. Bumped if the document shape changes.
/// Version 3 represents pane launch intent without collapsing direct argv into a shell string.
pub const SHARED_LAYOUT_VERSION: u32 = 3;

/// The complete shared window-manager document for a session. Fractions in [`FracRect`] are
/// relative to the controller's canonical pane canvas (`canvas_cols` × `canvas_rows`, excluding
/// the workbar) so followers can rescale floating geometry to their own viewport.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SharedLayout {
    pub version: u32,
    pub canvas_cols: u16,
    pub canvas_rows: u16,
    pub workspaces: Vec<SharedWorkspace>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SharedWorkspace {
    pub index: usize,
    pub name: Option<String>,
    pub synchronized: bool,
    pub layout: SharedLayoutKind,
    pub start_axis: SharedSplitAxis,
    pub split_ratios: Vec<f32>,
    pub tree: Option<SharedTree>,
    pub panes: Vec<SharedPane>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SharedPane {
    pub pane_id: PaneId,
    pub generation: u64,
    pub title: Option<String>,
    pub profile_name: Option<String>,
    pub cwd: Option<String>,
    pub launch: Option<crate::pane_launch::PaneLaunch>,
    /// See [`crate::state::PaneIdentity::replay`]. Shared so a follower that takes control
    /// respawns an exited profile pane through the interactive shell, not the command runner.
    /// Defaulted so layout documents committed before this field existed still parse.
    #[serde(default)]
    pub replay: bool,
    pub keep_open: bool,
    pub floating: bool,
    pub fullscreen: bool,
    /// Fractions of the canonical canvas; `Some` only for floating panes.
    pub rect: Option<FracRect>,
    /// Scrollable column width as a fraction of the tile viewport.
    #[serde(default = "default_shared_scrollable_width")]
    pub scrollable_width: f32,
}

fn default_shared_scrollable_width() -> f32 {
    crate::state::DEFAULT_SCROLLABLE_WIDTH
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FracRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SharedLayoutValidationError {
    UnsupportedVersion(u32),
    ZeroCanvasDimensions,
    DuplicateWorkspaceIndex(usize),
    InvalidWorkspaceIndex(usize),
    ReservedPaneId(PaneId),
    DuplicatePaneId(PaneId),
    TreeLeafNotInWorkspace {
        workspace: usize,
        pane_id: PaneId,
    },
    DuplicateTreeLeaf {
        workspace: usize,
        pane_id: PaneId,
    },
    InvalidSplitRatio {
        workspace: usize,
    },
    InvalidTreeSplitRatio {
        workspace: usize,
    },
    MissingFloatingRect(PaneId),
    InvalidFloatingRect {
        pane_id: PaneId,
        reason: &'static str,
    },
    InvalidScrollableWidth(PaneId),
    InvalidGeneration {
        pane_id: PaneId,
        generation: u64,
    },
}

impl std::fmt::Display for SharedLayoutValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion(v) => write!(f, "unsupported shared layout version {v}"),
            Self::ZeroCanvasDimensions => write!(f, "canvas dimensions must be positive"),
            Self::DuplicateWorkspaceIndex(i) => write!(f, "duplicate workspace index {i}"),
            Self::InvalidWorkspaceIndex(i) => write!(f, "workspace index {i} out of bounds"),
            Self::ReservedPaneId(id) => {
                write!(f, "reserved pane id {id} not allowed in shared layout")
            }
            Self::DuplicatePaneId(id) => write!(f, "duplicate pane id {id}"),
            Self::TreeLeafNotInWorkspace { workspace, pane_id } => {
                write!(
                    f,
                    "tree leaf {pane_id} not found in workspace {workspace} panes"
                )
            }
            Self::DuplicateTreeLeaf { workspace, pane_id } => {
                write!(f, "duplicate tree leaf {pane_id} in workspace {workspace}")
            }
            Self::InvalidSplitRatio { workspace } => {
                write!(f, "invalid split ratio in workspace {workspace}")
            }
            Self::InvalidTreeSplitRatio { workspace } => {
                write!(f, "invalid tree split ratio in workspace {workspace}")
            }
            Self::MissingFloatingRect(id) => write!(f, "floating pane {id} missing rect"),
            Self::InvalidFloatingRect { pane_id, reason } => {
                write!(f, "floating pane {pane_id} has invalid rect: {reason}")
            }
            Self::InvalidScrollableWidth(id) => {
                write!(
                    f,
                    "pane {id} has non-positive or non-finite scrollable width"
                )
            }
            Self::InvalidGeneration {
                pane_id,
                generation,
            } => {
                write!(f, "pane {pane_id} has invalid generation {generation}")
            }
        }
    }
}

impl std::error::Error for SharedLayoutValidationError {}

impl SharedLayout {
    /// Validate that the shared layout document satisfies all structural and security invariants:
    /// - Protocol/layout version matches [`SHARED_LAYOUT_VERSION`].
    /// - Positive canvas dimensions.
    /// - Unique workspace indexes within `0..WORKSPACE_COUNT` and pane identities across all workspaces.
    /// - Tree leaves belong to their workspace's pane list and do not repeat.
    /// - Split ratios in workspace lists and tree split nodes are finite and within `(0.0, 1.0)`.
    /// - Floating panes carry finite, positive fractional bounds; tiled panes carry no rect.
    /// - Scrollable column widths are finite and positive.
    /// - Pane generations are non-zero.
    pub fn validate(&self) -> std::result::Result<(), SharedLayoutValidationError> {
        if self.version != SHARED_LAYOUT_VERSION {
            return Err(SharedLayoutValidationError::UnsupportedVersion(
                self.version,
            ));
        }
        if self.canvas_cols == 0 || self.canvas_rows == 0 {
            return Err(SharedLayoutValidationError::ZeroCanvasDimensions);
        }

        let mut seen_workspaces = std::collections::HashSet::new();
        let mut seen_panes = std::collections::HashSet::new();

        for ws in &self.workspaces {
            if ws.index >= crate::state::WORKSPACE_COUNT {
                return Err(SharedLayoutValidationError::InvalidWorkspaceIndex(ws.index));
            }
            if !seen_workspaces.insert(ws.index) {
                return Err(SharedLayoutValidationError::DuplicateWorkspaceIndex(
                    ws.index,
                ));
            }

            let mut ws_pane_ids = std::collections::HashSet::new();
            for pane in &ws.panes {
                if pane.pane_id == crate::state::POPUP_PANE_ID {
                    return Err(SharedLayoutValidationError::ReservedPaneId(pane.pane_id));
                }
                if !seen_panes.insert(pane.pane_id) {
                    return Err(SharedLayoutValidationError::DuplicatePaneId(pane.pane_id));
                }
                ws_pane_ids.insert(pane.pane_id);

                if pane.generation == 0 {
                    return Err(SharedLayoutValidationError::InvalidGeneration {
                        pane_id: pane.pane_id,
                        generation: pane.generation,
                    });
                }

                if !pane.scrollable_width.is_finite() || pane.scrollable_width <= 0.0 {
                    return Err(SharedLayoutValidationError::InvalidScrollableWidth(
                        pane.pane_id,
                    ));
                }

                if pane.floating {
                    let Some(rect) = pane.rect else {
                        return Err(SharedLayoutValidationError::MissingFloatingRect(
                            pane.pane_id,
                        ));
                    };
                    if !rect.x.is_finite()
                        || !rect.y.is_finite()
                        || !rect.w.is_finite()
                        || !rect.h.is_finite()
                    {
                        return Err(SharedLayoutValidationError::InvalidFloatingRect {
                            pane_id: pane.pane_id,
                            reason: "coordinates must be finite",
                        });
                    }
                    if rect.w <= 0.0 || rect.h <= 0.0 {
                        return Err(SharedLayoutValidationError::InvalidFloatingRect {
                            pane_id: pane.pane_id,
                            reason: "dimensions must be positive",
                        });
                    }
                } else if pane.rect.is_some() {
                    return Err(SharedLayoutValidationError::InvalidFloatingRect {
                        pane_id: pane.pane_id,
                        reason: "non-floating pane must not carry a floating rect",
                    });
                }
            }

            for &ratio in &ws.split_ratios {
                if !ratio.is_finite() || ratio <= 0.0 || ratio >= 1.0 {
                    return Err(SharedLayoutValidationError::InvalidSplitRatio {
                        workspace: ws.index,
                    });
                }
            }

            if let Some(tree) = &ws.tree {
                let mut seen_leaves = std::collections::HashSet::new();
                validate_shared_tree(tree, ws.index, &ws_pane_ids, &mut seen_leaves)?;
            }
        }

        Ok(())
    }
}

fn validate_shared_tree(
    tree: &SharedTree,
    workspace_idx: usize,
    workspace_panes: &std::collections::HashSet<PaneId>,
    seen_leaves: &mut std::collections::HashSet<PaneId>,
) -> std::result::Result<(), SharedLayoutValidationError> {
    match tree {
        SerializedTree::Leaf { pane } => {
            if !workspace_panes.contains(pane) {
                return Err(SharedLayoutValidationError::TreeLeafNotInWorkspace {
                    workspace: workspace_idx,
                    pane_id: *pane,
                });
            }
            if !seen_leaves.insert(*pane) {
                return Err(SharedLayoutValidationError::DuplicateTreeLeaf {
                    workspace: workspace_idx,
                    pane_id: *pane,
                });
            }
            Ok(())
        }
        SerializedTree::Split {
            ratio,
            first,
            second,
            ..
        } => {
            if !ratio.is_finite() || *ratio <= 0.0 || *ratio >= 1.0 {
                return Err(SharedLayoutValidationError::InvalidTreeSplitRatio {
                    workspace: workspace_idx,
                });
            }
            validate_shared_tree(first, workspace_idx, workspace_panes, seen_leaves)?;
            validate_shared_tree(second, workspace_idx, workspace_panes, seen_leaves)?;
            Ok(())
        }
    }
}

/// The tiling tree with direct pane ids (no positional indirection like profile TOML).
pub type SharedTree = SerializedTree<PaneId>;
pub type SharedLayoutKind = SerializedLayoutKind;
pub type SharedSplitAxis = SerializedSplitAxis;

/// Build the shared document from the client's live `State` for the given canonical canvas size
/// (in cells, excluding the workbar). The scratchpad is excluded because it is
/// local-only lifecycle, never shared.
pub fn shared_layout_from_state(state: &State, canvas: (u16, u16)) -> SharedLayout {
    let canvas_cols = canvas.0.max(1);
    let canvas_rows = canvas.1.max(1);
    SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        canvas_cols,
        canvas_rows,
        workspaces: state
            .current()
            .workspaces
            .iter()
            .enumerate()
            .map(|(index, workspace)| {
                shared_workspace_from_state(index, workspace, canvas_cols, canvas_rows)
            })
            .collect(),
    }
}

fn shared_workspace_from_state(
    index: usize,
    workspace: &crate::state::Workspace,
    canvas_cols: u16,
    canvas_rows: u16,
) -> SharedWorkspace {
    let cols = f32::from(canvas_cols.max(1));
    let rows = f32::from(canvas_rows.max(1));
    // The effective tree prunes to live tiled panes and appends any stragglers, so it matches the
    // layout engine's live pane set.
    let tree = crate::layout::effective_tile_tree(workspace, None)
        .as_ref()
        .and_then(|tree| from_dwindle(tree, &|id| Some(id)));
    SharedWorkspace {
        index,
        name: workspace.name.clone(),
        synchronized: workspace.synchronized,
        layout: workspace.layout_kind.into(),
        start_axis: workspace.start_axis.into(),
        split_ratios: workspace.split_ratios.clone(),
        tree,
        panes: workspace
            .panes
            .iter()
            .filter(|pane| !pane.closing)
            .map(|pane| SharedPane {
                pane_id: pane.id,
                generation: pane.pty_generation,
                title: pane.identity.custom_title.clone(),
                profile_name: pane.identity.profile_name.clone(),
                cwd: pane.identity.cwd.clone(),
                launch: pane.identity.launch.clone(),
                replay: pane.identity.replay,
                keep_open: pane.identity.keep_open,
                floating: pane.floating,
                fullscreen: pane.fullscreen,
                rect: pane.floating.then(|| FracRect {
                    x: pane.floating_rect.x / cols,
                    y: pane.floating_rect.y / rows,
                    w: pane.floating_rect.w / cols,
                    h: pane.floating_rect.h / rows,
                }),
                scrollable_width: crate::tiling::sanitize_scrollable_width(pane.scrollable_width),
            })
            .collect(),
    }
}

/// Rebuild a [`DwindleTree`] from a shared tree, keeping only leaves whose pane id is present
/// locally (`known`). Returns `None` if nothing survives, so the caller can rebuild from order.
pub(crate) fn dwindle_from_shared(
    tree: &SharedTree,
    known: &std::collections::HashSet<PaneId>,
) -> Option<DwindleTree> {
    to_dwindle(tree, &|pane| known.contains(pane).then_some(*pane), true)
}

pub(crate) fn frac_rect_to_float(rect: FracRect, canvas_cols: u16, canvas_rows: u16) -> FloatRect {
    FloatRect {
        x: rect.x * f32::from(canvas_cols.max(1)),
        y: rect.y * f32::from(canvas_rows.max(1)),
        w: rect.w * f32::from(canvas_cols.max(1)),
        h: rect.h * f32::from(canvas_rows.max(1)),
    }
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
}

fn index_incoming_panes(layout: &SharedLayout) -> std::collections::HashMap<PaneId, usize> {
    use crate::state::WORKSPACE_COUNT;

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
) -> DrainedPanes {
    use crate::state::WORKSPACE_COUNT;

    let mut reusable = std::collections::HashMap::new();
    let mut closing_by_workspace = Vec::with_capacity(WORKSPACE_COUNT);
    let mut pruned = Vec::new();
    for workspace in &mut state.current_mut().workspaces {
        let mut closing = Vec::new();
        for mut pane in workspace.panes.drain(..) {
            if incoming.contains_key(&pane.id) {
                // A commit that re-adds a pane mid-close cancels the close and hands the live
                // pane back with its terminal screen and scrollback intact.
                pane.closing = false;
                reusable.insert(pane.id, pane);
            } else if pane.closing {
                closing.push(pane);
            } else {
                // The server already dropped this pane. Re-killing it could race a reused id.
                pane.opening = false;
                pane.closing = true;
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

fn rebuild_shared_workspaces(
    ctx: &mut Context<crate::AppRoot>,
    layout: &SharedLayout,
    canonical_canvas: (u16, u16),
    bounds: FloatRect,
    reusable: &mut std::collections::HashMap<PaneId, crate::state::Pane>,
    closing_by_workspace: &mut [Vec<crate::state::Pane>],
) -> RebuiltWorkspaces {
    use crate::state::{Pane, WORKSPACE_COUNT};

    let (canvas_cols, canvas_rows) = canonical_canvas;
    let mut next_pane_id = ctx.state.current().next_pane_id;
    let mut next_generation = ctx.state.current().next_pty_generation;
    let scrollback = ctx.state.config.scrollback;
    let mut orphan_clipboard_events = Vec::new();
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
                    crate::geometry::default_floating_rect(bounds, shared_pane.pane_id)
                });
            let existing = reusable.remove(&shared_pane.pane_id).or_else(|| {
                ctx.state
                    .current_mut()
                    .take_retired_pane(shared_pane.pane_id, shared_pane.generation)
            });
            let mut pane = match existing {
                Some(mut existing) => {
                    // A new generation needs a fresh server backend, but keeps local dimensions.
                    if existing.pty_generation != shared_pane.generation {
                        existing.terminal.cols = existing.terminal.cols.max(1);
                        existing.terminal.rows = existing.terminal.rows.max(1);
                        existing
                            .terminal
                            .bind_server_backend(shared_pane.pane_id, shared_pane.generation);
                        existing.pty_generation = shared_pane.generation;
                    }
                    orphan_clipboard_events.extend(drain_orphan_output(
                        ctx_shared_mut(ctx),
                        &mut existing,
                        shared_pane,
                    ));
                    existing
                }
                None => {
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
                    pane
                }
            };

            apply_shared_pane_fields(&mut pane, shared_pane, float_rect);
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
    }
}

fn restore_closing_panes_in_omitted_workspaces(
    state: &mut State,
    layout: &SharedLayout,
    closing_by_workspace: Vec<Vec<crate::state::Pane>>,
) {
    use crate::state::WORKSPACE_COUNT;

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

fn repair_workspace_focus_and_anchors(state: &mut State) -> [bool; crate::state::WORKSPACE_COUNT] {
    use crate::state::{LayoutKind, ScrollableRevealEdge, WORKSPACE_COUNT};

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
/// mode, theme) is preserved. Removed panes are dropped from application state immediately; the
/// stable keyed Canvas retains their already-described visual subtree for its exit animation.
pub(crate) fn apply_shared_layout(
    ctx: &mut Context<crate::AppRoot>,
    layout: &SharedLayout,
    rev: u64,
) -> Update {
    use crate::state::WORKSPACE_COUNT;

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

    // Missing panes remain in their workspace until their close animation is pruned.
    let DrainedPanes {
        reusable: mut pool,
        closing_by_workspace: mut closing_by_ws,
        pruned,
    } = drain_existing_panes(&mut ctx.state, &incoming);

    let RebuiltWorkspaces {
        next_pane_id,
        next_generation,
        orphan_clipboard_events,
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
        ctx.state.animation = crate::anim::GeometryAnimation::Close;
        return Update::with_command(crate::pane_lifecycle::prune_closed_batch_command(
            ctx.state.runtime_epoch,
            pruned,
            crate::anim::retained_pane_timeout(ctx.state.config.animations),
        ));
    }
    ctx.state.animation = crate::anim::GeometryAnimation::TileFloat;

    Update::full()
}

/// Reborrow the shared-session bookkeeping mutably; used by the reconciler's orphan drain so it can
/// touch `orphan_output` while also mutating a pane.
fn ctx_shared_mut(
    ctx: &mut Context<crate::AppRoot>,
) -> Option<&mut crate::state::SharedSessionState> {
    ctx.state.current_mut().shared.as_mut()
}

fn drain_orphan_output(
    shared: Option<&mut crate::state::SharedSessionState>,
    pane: &mut crate::state::Pane,
    shared_pane: &SharedPane,
) -> Vec<tui_lipan::prelude::TerminalClipboardEvent> {
    let Some(shared) = shared else {
        return Vec::new();
    };
    if let Some(bytes) = shared.take_orphan_output(shared_pane.pane_id, shared_pane.generation) {
        return pane.terminal.process_server_output(&bytes).clipboard_events;
    }
    Vec::new()
}

fn apply_shared_pane_fields(
    pane: &mut crate::state::Pane,
    shared_pane: &SharedPane,
    float_rect: FloatRect,
) {
    pane.floating = shared_pane.floating;
    pane.fullscreen = shared_pane.fullscreen;
    if shared_pane.floating {
        pane.floating_rect = float_rect;
    }
    pane.identity.custom_title = shared_pane.title.clone();
    pane.identity.profile_name = shared_pane.profile_name.clone();
    pane.identity.cwd = shared_pane.cwd.clone();
    pane.identity.launch = shared_pane.launch.clone();
    pane.identity.replay = shared_pane.replay
        && matches!(
            pane.identity.launch.as_ref(),
            Some(crate::pane_launch::PaneLaunch::Shell { .. })
        );
    pane.identity.keep_open = shared_pane.keep_open;
    pane.scrollable_width = crate::tiling::sanitize_scrollable_width(shared_pane.scrollable_width);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::state::{Pane, State};

    #[test]
    fn client_scratchpad_is_excluded_from_shared_layout() {
        let mut state = State::new(Config::default(), Theme::default());
        let scratch_id = 1 << 31;
        state
            .scratch
            .panes
            .push(Pane::new(scratch_id, 100, FloatRect::default()));
        crate::tiling::append_tiled_window(&mut state.scratch, scratch_id);

        let layout = shared_layout_from_state(&state, (100, 30));
        assert!(layout.workspaces.iter().all(|workspace| {
            workspace
                .panes
                .iter()
                .all(|pane| pane.pane_id != scratch_id)
        }));
    }

    #[test]
    fn shared_pane_without_replay_field_parses_as_non_replay() {
        // `replay` remains defaulted independently of the launch representation.
        let pane: SharedPane = serde_json::from_value(serde_json::json!({
            "pane_id": 2,
            "generation": 7,
            "title": null,
            "profile_name": null,
            "cwd": null,
            "launch": {"kind": "shell", "command": "nvim"},
            "keep_open": false,
            "floating": false,
            "fullscreen": false,
            "rect": null
        }))
        .expect("pre-replay shared pane parses");
        assert!(!pane.replay);
        assert_eq!(
            pane.scrollable_width,
            crate::state::DEFAULT_SCROLLABLE_WIDTH
        );
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
            crate::state::DEFAULT_SCROLLABLE_WIDTH
        );
        assert_eq!(SHARED_LAYOUT_VERSION, 3);
    }

    fn state_with_split() -> State {
        let mut state = State::new(Config::default(), Theme::default());
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 24.0,
        };
        let previous = state.current().workspaces[0].focused_pane;
        let mut pane = Pane::new(2, state.config.scrollback, rect);
        pane.pty_generation = 7;
        state.current_mut().workspaces[0].panes.push(pane);
        let bounds = state.canvas_bounds_from_terminal_viewport(Rect {
            x: 0,
            y: 0,
            w: 80,
            h: 25,
        });
        let split_width_multiplier = state.config.layout.split_width_multiplier;
        crate::layout::place_spawned_pane(
            &mut state.current_mut().workspaces[0],
            2,
            previous,
            bounds,
            0.0,
            crate::state::TileGap::DEFAULT,
            split_width_multiplier,
        );
        state.current_mut().next_pane_id = 3;
        state.current_mut().next_pty_generation = 8;
        state
    }

    #[test]
    fn shared_layout_captures_panes_and_tree() {
        let state = state_with_split();
        let layout = shared_layout_from_state(&state, (80, 24));
        assert_eq!(layout.version, SHARED_LAYOUT_VERSION);
        assert_eq!(layout.canvas_cols, 80);
        let ws = &layout.workspaces[0];
        assert_eq!(ws.panes.len(), 2);
        assert!(ws.tree.is_some());
    }

    #[test]
    fn floating_rect_round_trips_through_fractions() {
        let mut state = State::new(Config::default(), Theme::default());
        state.current_mut().workspaces[0].panes[0].floating = true;
        state.current_mut().workspaces[0].panes[0].floating_rect = FloatRect {
            x: 20.0,
            y: 6.0,
            w: 40.0,
            h: 12.0,
        };
        let layout = shared_layout_from_state(&state, (80, 24));
        let rect = layout.workspaces[0].panes[0].rect.expect("floating rect");
        let restored = frac_rect_to_float(rect, 80, 24);
        assert!((restored.x - 20.0).abs() < 0.001);
        assert!((restored.w - 40.0).abs() < 0.001);
    }

    #[test]
    fn dwindle_from_shared_drops_unknown_leaves() {
        let tree = SharedTree::Split {
            axis: SharedSplitAxis::Horizontal,
            ratio: 0.5,
            first: Box::new(SharedTree::Leaf { pane: 1 }),
            second: Box::new(SharedTree::Leaf { pane: 2 }),
        };
        let known: std::collections::HashSet<PaneId> = [1].into_iter().collect();
        let rebuilt = dwindle_from_shared(&tree, &known);
        assert_eq!(rebuilt, Some(DwindleTree::Leaf(1)));
    }

    #[test]
    fn shared_layout_round_trips_columns_and_scrollable_kinds() {
        for kind in [
            SharedLayoutKind::Columns,
            SharedLayoutKind::Rows,
            SharedLayoutKind::Scrollable,
        ] {
            let layout = SharedLayout {
                version: SHARED_LAYOUT_VERSION,
                canvas_cols: 80,
                canvas_rows: 24,
                workspaces: vec![SharedWorkspace {
                    index: 0,
                    name: None,
                    synchronized: false,
                    layout: kind,
                    start_axis: SharedSplitAxis::Horizontal,
                    split_ratios: Vec::new(),
                    tree: None,
                    panes: vec![SharedPane {
                        pane_id: 1,
                        generation: 1,
                        title: None,
                        profile_name: None,
                        cwd: None,
                        launch: None,
                        replay: false,
                        keep_open: false,
                        floating: false,
                        fullscreen: false,
                        rect: None,
                        scrollable_width: crate::state::DEFAULT_SCROLLABLE_WIDTH,
                    }],
                }],
            };
            let encoded = serde_json::to_value(&layout).expect("encode");
            let label = match kind {
                SharedLayoutKind::Columns => "columns",
                SharedLayoutKind::Rows => "rows",
                SharedLayoutKind::Scrollable => "scrollable",
                _ => unreachable!(),
            };
            assert_eq!(encoded["workspaces"][0]["layout"], label);
            assert!(!encoded.to_string().contains("scrollable_anchor"));
            let decoded: SharedLayout = serde_json::from_value(encoded).expect("decode");
            assert_eq!(decoded.workspaces[0].layout, kind);
        }
    }
}

/// Reconciler behavior driven through the real runtime (a follower/controller applying commits).
#[cfg(test)]
mod reconciler_tests {
    use super::*;
    use crate::AppRoot;
    use crate::Msg;
    use crate::input::Action;
    use crate::ops::focus::focus_pane;
    use crate::pane_lifecycle::{find_pane, find_pane_mut};
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::ClientMessage;
    use crate::state::{Direction, DirectionalFocusHint, SharedSessionState};
    use tui_lipan::TestBackend;

    const VIEWPORT: Rect = Rect {
        x: 0,
        y: 0,
        w: 100,
        h: 30,
    };

    fn layout_with_panes(panes: &[(PaneId, u64)]) -> SharedLayout {
        SharedLayout {
            version: SHARED_LAYOUT_VERSION,
            canvas_cols: 100,
            canvas_rows: 28,
            workspaces: vec![SharedWorkspace {
                index: 0,
                name: None,
                synchronized: false,
                layout: SharedLayoutKind::Dwindle,
                start_axis: SharedSplitAxis::Horizontal,
                split_ratios: Vec::new(),
                tree: None,
                panes: panes
                    .iter()
                    .map(|(id, generation)| SharedPane {
                        pane_id: *id,
                        generation: *generation,
                        title: None,
                        profile_name: None,
                        cwd: None,
                        launch: None,
                        replay: false,
                        keep_open: false,
                        floating: false,
                        fullscreen: false,
                        rect: None,
                        scrollable_width: crate::state::DEFAULT_SCROLLABLE_WIDTH,
                    })
                    .collect(),
            }],
        }
    }

    /// Run in a generous stack: mounting the full app in `TestBackend` overflows the default test
    /// stack, matching the pattern used by the snapshot tests in `main.rs`.
    fn in_stack<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(body)
            .expect("spawn test thread")
            .join()
            .expect("join test thread")
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
    fn remote_removal_drops_pane_without_sending_kill() {
        in_stack(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            let (client, rx) = SessionClient::test_channel();
            attach_follower(&mut backend, client);
            backend.render();

            // A commit with no panes removes the local seed pane.
            backend
                .dispatch(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout: layout_with_panes(&[]),
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
                .dispatch(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout: layout_with_panes(&[]),
                })
                .expect("remove pane");
            backend
                .dispatch(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 2,
                    author: 2,
                    layout: layout_with_panes(&[(1, 0)]),
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
                .dispatch(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout,
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
                .dispatch(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout: layout_with_panes(&[(1, 0), (2, 5)]),
                })
                .expect("dispatch commit");

            let added = find_pane(backend.state_mut(), 2).expect("pane 2 created by reconciler");
            assert_eq!(added.pty_generation, 5);
            assert!(added.terminal.is_ready());
            assert!(find_pane(backend.state_mut(), 1).is_some(), "survivor kept");
            assert_eq!(
                backend.state_mut().animation,
                crate::anim::GeometryAnimation::TileFloat,
                "live layout commits should retain geometry transitions"
            );
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
                .dispatch(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout: layout_with_panes(&[(1, 0), (2, 5)]),
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
                .dispatch(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout: layout.clone(),
                })
                .expect("seed shared layout");

            backend.state_mut().current_mut().workspaces[0].scrollable_anchor = Some(2);
            backend
                .dispatch(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 2,
                    author: 2,
                    layout: layout.clone(),
                })
                .expect("reconcile with survivor");
            assert_eq!(
                backend.state().current().workspaces[0].scrollable_anchor,
                Some(2)
            );

            let mut without_anchor = layout_with_panes(&[(1, 0)]);
            without_anchor.workspaces[0].layout = SharedLayoutKind::Scrollable;
            backend
                .dispatch(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 3,
                    author: 2,
                    layout: without_anchor,
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
                .dispatch(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout: layout.clone(),
                })
                .expect("seed scrollable");
            {
                let state = backend.state_mut();
                focus_pane(state, 4);
                state.animation = crate::anim::GeometryAnimation::None;
            }
            backend.render();
            // Focus a still-visible pane so the right anchor survives when that focus is removed.
            {
                let state = backend.state_mut();
                focus_pane(state, 3);
                state.animation = crate::anim::GeometryAnimation::None;
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
                .dispatch(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 2,
                    author: 2,
                    layout: without_focus,
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
                crate::state::ScrollableRevealEdge::Left
            );
            assert_eq!(
                backend.state().animation,
                crate::anim::GeometryAnimation::Close,
                "removing a pane keeps reconciler Close animation"
            );
            backend.render();
            let visible = {
                let state = backend.state();
                let viewport = state.last_viewport.get().expect("viewport");
                let letterbox = crate::view::follower_letterbox_bounds(state, viewport);
                let local = state.canvas_bounds_from_terminal_viewport(viewport);
                let top_gap = state.workspace_top_gap();
                let tile_letterbox = crate::geometry::workspace_tile_bounds(letterbox, top_gap);
                let tile_local = crate::geometry::workspace_tile_bounds(local, top_gap);
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
            use crate::ops::focus::switch_workspace;
            use crate::state::LayoutKind;

            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            let (client, _rx) = SessionClient::test_channel();
            attach_follower(&mut backend, client);
            backend.render();

            // Seed scrollable content on workspace 1 while staying active on 0.
            let mut layout = SharedLayout {
                version: SHARED_LAYOUT_VERSION,
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
                            fullscreen: false,
                            rect: None,
                            scrollable_width: crate::state::DEFAULT_SCROLLABLE_WIDTH,
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
                                fullscreen: false,
                                rect: None,
                                scrollable_width: crate::state::DEFAULT_SCROLLABLE_WIDTH,
                            })
                            .collect(),
                    },
                ],
            };
            backend
                .dispatch(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout: layout.clone(),
                })
                .expect("seed multi-ws");
            assert_eq!(backend.state().current().active_workspace, 0);

            // Prepare ws1 viewport state as if it had been right-scrolled with focus on 3.
            {
                let state = backend.state_mut();
                state.current_mut().active_workspace = 1;
                focus_pane(state, 4);
                state.animation = crate::anim::GeometryAnimation::None;
            }
            backend.render();
            {
                let state = backend.state_mut();
                focus_pane(state, 3);
                state.animation = crate::anim::GeometryAnimation::None;
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
                .dispatch(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 2,
                    author: 2,
                    layout,
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
                crate::anim::GeometryAnimation::None
            );
            assert_eq!(backend.state().current().focused_pane, Some(1));
            assert_eq!(
                backend.state().current().workspaces[1].scrollable_anchor,
                Some(1)
            );
            assert_eq!(
                backend.state().current().workspaces[1].scrollable_reveal_edge,
                crate::state::ScrollableRevealEdge::Left
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
                let tile_letterbox = crate::geometry::workspace_tile_bounds(letterbox, top_gap);
                let tile_local = crate::geometry::workspace_tile_bounds(local, top_gap);
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
                .dispatch(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout,
                })
                .expect("commit scrollable widths");

            let ws = &backend.state().current().workspaces[0];
            assert_eq!(ws.layout_kind, crate::state::LayoutKind::Scrollable);
            assert!((ws.panes[0].scrollable_width - 0.62).abs() < 1e-6);
            assert_eq!(
                ws.panes[1].scrollable_width,
                crate::state::MAX_SPLIT_RATIO,
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
                crate::state::MAX_SPLIT_RATIO
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
                .dispatch(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout: layout.clone(),
                })
                .expect("seed shared layout");

            let hint = DirectionalFocusHint {
                pane: 1,
                entry_direction: Direction::Left,
                target: 2,
            };
            backend.state_mut().current_mut().workspaces[0].last_directional_focus = Some(hint);

            backend
                .dispatch(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 2,
                    author: 2,
                    layout,
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
                .dispatch(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 1,
                    author: 2,
                    layout: layout.clone(),
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
                    .dispatch(Msg::SessionLayoutCommitted {
                        epoch: 0,
                        rev,
                        author: 2,
                        layout: layout.clone(),
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
                    .dispatch(Msg::SessionLayoutCommitted {
                        epoch: 0,
                        rev,
                        author: 2,
                        layout: layout.clone(),
                    })
                    .expect("reconcile shared layout");
            }
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
                    name: "live".into(),
                    client: Some(client),
                    autostart: false,
                    read_only: false,
                    reconnect: false,
                    remote_host: None,
                    intent: crate::state::AttachIntent::Plain,
                    left: None,
                    parked_epoch: None,
                });
            backend.render();

            backend
                .dispatch(Msg::SessionAttached {
                    epoch: 1,
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
                    created_from_profile: None,
                })
                .expect("dispatch attach");

            let state = backend.state_mut();
            assert!(find_pane(state, 1).is_some());
            assert!(find_pane(state, 2).is_some());
            assert_eq!(state.animation, crate::anim::GeometryAnimation::None);
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
                .dispatch(Msg::SessionLayoutCommitted {
                    epoch: 0,
                    rev: 7,
                    author: 1,
                    layout: layout_with_panes(&[]),
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
    fn shared_layout_validation_enforces_invariants() {
        let valid = layout_with_panes(&[(1, 1), (2, 1)]);
        assert_eq!(valid.validate(), Ok(()));

        // Unsupported version
        let mut bad = valid.clone();
        bad.version = 999;
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::UnsupportedVersion(999))
        ));

        // Zero canvas
        let mut bad = valid.clone();
        bad.canvas_cols = 0;
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::ZeroCanvasDimensions)
        ));

        // Duplicate workspace index
        let mut bad = valid.clone();
        bad.workspaces.push(bad.workspaces[0].clone());
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::DuplicateWorkspaceIndex(0))
        ));

        // Duplicate pane id
        let mut bad = valid.clone();
        bad.workspaces[0].panes[1].pane_id = 1;
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::DuplicatePaneId(1))
        ));

        // Invalid generation
        let mut bad = valid.clone();
        bad.workspaces[0].panes[0].generation = 0;
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::InvalidGeneration {
                pane_id: 1,
                generation: 0
            })
        ));

        // Invalid scrollable width
        let mut bad = valid.clone();
        bad.workspaces[0].panes[0].scrollable_width = 0.0;
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::InvalidScrollableWidth(1))
        ));

        // Floating pane missing rect
        let mut bad = valid.clone();
        bad.workspaces[0].panes[0].floating = true;
        bad.workspaces[0].panes[0].rect = None;
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::MissingFloatingRect(1))
        ));

        // Floating pane invalid rect
        let mut bad = valid.clone();
        bad.workspaces[0].panes[0].floating = true;
        bad.workspaces[0].panes[0].rect = Some(FracRect {
            x: 0.1,
            y: 0.1,
            w: 0.0,
            h: 0.5,
        });
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::InvalidFloatingRect { pane_id: 1, .. })
        ));

        // Non-floating pane carrying rect
        let mut bad = valid.clone();
        bad.workspaces[0].panes[0].floating = false;
        bad.workspaces[0].panes[0].rect = Some(FracRect {
            x: 0.1,
            y: 0.1,
            w: 0.5,
            h: 0.5,
        });
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::InvalidFloatingRect { pane_id: 1, .. })
        ));

        // Invalid split ratio
        let mut bad = valid.clone();
        bad.workspaces[0].split_ratios.push(1.5);
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::InvalidSplitRatio { workspace: 0 })
        ));

        // Tree leaf not in workspace
        let mut bad = valid.clone();
        bad.workspaces[0].tree = Some(SerializedTree::Leaf { pane: 99 });
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::TreeLeafNotInWorkspace {
                workspace: 0,
                pane_id: 99
            })
        ));

        // Duplicate tree leaf
        let mut bad = valid.clone();
        bad.workspaces[0].tree = Some(SerializedTree::Split {
            axis: SharedSplitAxis::Horizontal,
            ratio: 0.5,
            first: Box::new(SerializedTree::Leaf { pane: 1 }),
            second: Box::new(SerializedTree::Leaf { pane: 1 }),
        });
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::DuplicateTreeLeaf {
                workspace: 0,
                pane_id: 1
            })
        ));

        // Invalid tree split ratio
        let mut bad = valid.clone();
        bad.workspaces[0].tree = Some(SerializedTree::Split {
            axis: SharedSplitAxis::Horizontal,
            ratio: 0.0,
            first: Box::new(SerializedTree::Leaf { pane: 1 }),
            second: Box::new(SerializedTree::Leaf { pane: 2 }),
        });
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::InvalidTreeSplitRatio { workspace: 0 })
        ));

        // Workspace index out of range
        let mut bad = valid.clone();
        bad.workspaces[0].index = crate::state::WORKSPACE_COUNT;
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::InvalidWorkspaceIndex(_))
        ));

        // Reserved popup pane id
        let mut bad = valid.clone();
        bad.workspaces[0].panes[0].pane_id = crate::state::POPUP_PANE_ID;
        bad.workspaces[0].tree = Some(SerializedTree::Split {
            axis: SharedSplitAxis::Horizontal,
            ratio: 0.5,
            first: Box::new(SerializedTree::Leaf {
                pane: crate::state::POPUP_PANE_ID,
            }),
            second: Box::new(SerializedTree::Leaf { pane: 2 }),
        });
        assert!(matches!(
            bad.validate(),
            Err(SharedLayoutValidationError::ReservedPaneId(_))
        ));
    }
}
