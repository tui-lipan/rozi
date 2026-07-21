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

/// Wire-format version for [`SharedLayout`]. Bumped if the document shape changes; protocol v6
/// carries version 1.
pub const SHARED_LAYOUT_VERSION: u32 = 1;

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
    pub command: Option<String>,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FracRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The tiling tree with direct pane ids (no positional indirection like profile TOML).
pub type SharedTree = SerializedTree<PaneId>;
pub type SharedLayoutKind = SerializedLayoutKind;
pub type SharedSplitAxis = SerializedSplitAxis;

/// Build the shared document from the client's live `State` for the given canonical canvas size
/// (in cells, excluding the workbar). Closing panes and the scratchpad are excluded - they are
/// local-only lifecycle, never shared.
pub fn shared_layout_from_state(state: &State, canvas: (u16, u16)) -> SharedLayout {
    let canvas_cols = canvas.0.max(1);
    let canvas_rows = canvas.1.max(1);
    SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        canvas_cols,
        canvas_rows,
        workspaces: state
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
    // The effective tree already prunes to active (non-closing, tiled) panes and appends any
    // stragglers, so it matches exactly what the layout engine renders.
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
                command: pane.identity.command.clone(),
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

/// Reconcile the client's local `State` toward an authoritative shared layout at revision `rev`.
///
/// This is the follower's read path (and the seed path on attach). It moves, adds, removes, and
/// reorders `Pane` structs and rewrites workspace metadata, but never touches a surviving pane's
/// terminal screen, scrollback, or snapshot - only brand-new panes get a fresh backend, and only
/// their buffered orphan output is replayed. Local-only state (focus, active workspace, overlays,
/// mode, theme) is preserved. Returns an `Update` carrying the batched prune command for any panes
/// the commit removed.
pub(crate) fn apply_shared_layout(
    ctx: &mut Context<crate::HyprmuxApp>,
    layout: &SharedLayout,
    rev: u64,
) -> Update {
    use crate::pane_lifecycle::{close_pane_remote, prune_closed_batch_command};
    use crate::state::{Pane, WORKSPACE_COUNT};

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

    // Index the incoming panes: id -> (workspace index, order within workspace, pane).
    let mut incoming: std::collections::HashMap<PaneId, (usize, usize, &SharedPane)> =
        std::collections::HashMap::new();
    for shared_ws in &layout.workspaces {
        if shared_ws.index >= WORKSPACE_COUNT {
            continue;
        }
        for (order, pane) in shared_ws.panes.iter().enumerate() {
            incoming.insert(pane.pane_id, (shared_ws.index, order, pane));
        }
    }

    // Removals: live local panes absent from the incoming set close out (no `client.kill`).
    let removed_ids: Vec<PaneId> = ctx
        .state
        .workspaces
        .iter()
        .flat_map(|ws| ws.panes.iter())
        .filter(|pane| !pane.closing && !incoming.contains_key(&pane.id))
        .map(|pane| pane.id)
        .collect();
    let mut pruned: Vec<(PaneId, u64)> = Vec::new();
    for id in removed_ids {
        if let Some(generation) = close_pane_remote(ctx, id) {
            pruned.push((id, generation));
        }
    }

    // Drain every surviving (non-closing) pane into a pool keyed by id; closing panes stay in place
    // so their exit animation is undisturbed.
    let mut pool: std::collections::HashMap<PaneId, Pane> = std::collections::HashMap::new();
    let mut closing_by_ws: Vec<Vec<Pane>> = Vec::with_capacity(WORKSPACE_COUNT);
    for ws in &mut ctx.state.workspaces {
        let mut closing = Vec::new();
        for pane in ws.panes.drain(..) {
            if pane.closing {
                closing.push(pane);
            } else {
                pool.insert(pane.id, pane);
            }
        }
        closing_by_ws.push(closing);
    }

    let mut max_pane_id = ctx.state.next_pane_id;
    let mut max_generation = ctx.state.next_pty_generation;
    let scrollback = ctx.state.config.scrollback;

    // Rebuild each workspace from the incoming order, reusing pooled panes (survivors + moves) and
    // creating brand-new ones as needed.
    for shared_ws in &layout.workspaces {
        if shared_ws.index >= WORKSPACE_COUNT {
            continue;
        }
        let mut rebuilt: Vec<Pane> = Vec::with_capacity(shared_ws.panes.len());
        for shared_pane in &shared_ws.panes {
            max_pane_id = max_pane_id.max(shared_pane.pane_id.saturating_add(1));
            max_generation = max_generation.max(shared_pane.generation.saturating_add(1));

            let float_rect = shared_pane
                .rect
                .map(|rect| frac_rect_to_float(rect, canvas_cols, canvas_rows))
                .unwrap_or_else(|| {
                    crate::geometry::default_floating_rect(bounds, shared_pane.pane_id)
                });

            let mut pane = match pool.remove(&shared_pane.pane_id) {
                Some(mut existing) => {
                    // Surviving pane (possibly moved workspace): keep its terminal untouched unless
                    // the generation changed (a respawn), which requires a fresh backend.
                    if existing.pty_generation != shared_pane.generation {
                        existing.terminal.cols = existing.terminal.cols.max(1);
                        existing.terminal.rows = existing.terminal.rows.max(1);
                        existing
                            .terminal
                            .bind_server_backend(shared_pane.pane_id, shared_pane.generation);
                        existing.pty_generation = shared_pane.generation;
                        drain_orphan_output(ctx_shared_mut(ctx), &mut existing, shared_pane);
                    }
                    existing
                }
                None => {
                    // Brand-new pane: a fresh backend is legal here only because it has no local
                    // screen yet. Replay any output buffered before this commit created it.
                    let mut pane = Pane::new(shared_pane.pane_id, scrollback, float_rect);
                    pane.pty_generation = shared_pane.generation;
                    pane.terminal
                        .bind_server_backend(shared_pane.pane_id, shared_pane.generation);
                    pane.opening = false;
                    pane.terminal_active = true;
                    pane.terminal.status = ManagedTerminalStatus::Ready;
                    drain_orphan_output(ctx_shared_mut(ctx), &mut pane, shared_pane);
                    pane
                }
            };

            apply_shared_pane_fields(&mut pane, shared_pane, float_rect);
            rebuilt.push(pane);
        }

        let ws = &mut ctx.state.workspaces[shared_ws.index];
        ws.name = shared_ws
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        ws.synchronized = shared_ws.synchronized;
        ws.layout_kind = shared_ws.layout.into();
        ws.start_axis = shared_ws.start_axis.into();
        if !shared_ws.split_ratios.is_empty() {
            ws.split_ratios = shared_ws.split_ratios.clone();
        }
        let known: std::collections::HashSet<PaneId> = rebuilt
            .iter()
            .filter(|pane| !pane.floating)
            .map(|pane| pane.id)
            .collect();
        ws.tile_tree = shared_ws
            .tree
            .as_ref()
            .and_then(|tree| dwindle_from_shared(tree, &known));
        ws.last_move_swap = None;
        ws.last_directional_focus = None;

        // Reattach the workspace's still-closing panes after the rebuilt live set.
        let mut panes = rebuilt;
        panes.extend(std::mem::take(&mut closing_by_ws[shared_ws.index]));
        ws.panes = panes;
    }

    // Any workspace the layout did not mention keeps only its closing panes.
    for (index, closing) in closing_by_ws.into_iter().enumerate() {
        if layout
            .workspaces
            .iter()
            .any(|ws| ws.index == index && ws.index < WORKSPACE_COUNT)
        {
            continue;
        }
        if !closing.is_empty() {
            ctx.state.workspaces[index].panes.extend(closing);
        }
    }

    // Fix up focus per workspace: keep the current focus when it survived, else fall back to the
    // first live pane.
    for ws in &mut ctx.state.workspaces {
        let focus_valid = ws
            .focused_pane
            .is_some_and(|id| ws.panes.iter().any(|pane| pane.id == id && !pane.closing));
        if !focus_valid {
            ws.focused_pane = ws
                .panes
                .iter()
                .find(|pane| !pane.closing)
                .map(|pane| pane.id);
        }
    }
    let active = ctx.state.active_workspace.min(WORKSPACE_COUNT - 1);
    ctx.state.active_workspace = active;
    ctx.state.focused_pane = ctx.state.workspaces[active].focused_pane;

    ctx.state.next_pane_id = ctx.state.next_pane_id.max(max_pane_id);
    ctx.state.next_pty_generation = ctx.state.next_pty_generation.max(max_generation);
    if let Some(shared) = ctx.state.shared.as_mut() {
        shared.layout_rev = rev;
        shared.canonical_canvas = Some((canvas_cols, canvas_rows));
        shared.last_committed_layout = Some(layout.clone());
    }
    ctx.state.animation = crate::anim::GeometryAnimation::TileFloat;

    if pruned.is_empty() {
        Update::full()
    } else {
        Update::with_command(prune_closed_batch_command(
            ctx.state.runtime_epoch,
            pruned,
            crate::anim::close_delay(ctx.state.config.animations),
        ))
    }
}

/// Reborrow the shared-session bookkeeping mutably; used by the reconciler's orphan drain so it can
/// touch `orphan_output` while also mutating a pane.
fn ctx_shared_mut(
    ctx: &mut Context<crate::HyprmuxApp>,
) -> Option<&mut crate::state::SharedSessionState> {
    ctx.state.shared.as_mut()
}

fn drain_orphan_output(
    shared: Option<&mut crate::state::SharedSessionState>,
    pane: &mut crate::state::Pane,
    shared_pane: &SharedPane,
) {
    let Some(shared) = shared else {
        return;
    };
    if let Some(bytes) = shared
        .orphan_output
        .remove(&(shared_pane.pane_id, shared_pane.generation))
    {
        pane.terminal.process_server_output(&bytes);
    }
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
    pane.identity.command = shared_pane.command.clone();
    pane.identity.replay = shared_pane.replay;
    pane.identity.keep_open = shared_pane.keep_open;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HyprmuxConfig;
    use crate::state::{Pane, State};

    #[test]
    fn shared_pane_without_replay_field_parses_as_non_replay() {
        // Layout documents committed by builds predating the `replay` field must still parse.
        let pane: SharedPane = serde_json::from_value(serde_json::json!({
            "pane_id": 2,
            "generation": 7,
            "title": null,
            "profile_name": null,
            "cwd": null,
            "command": "nvim",
            "keep_open": false,
            "floating": false,
            "fullscreen": false,
            "rect": null
        }))
        .expect("pre-replay shared pane parses");
        assert!(!pane.replay);
    }

    fn state_with_split() -> State {
        let mut state = State::new(HyprmuxConfig::default(), Theme::default());
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 24.0,
        };
        let previous = state.workspaces[0].focused_pane;
        let mut pane = Pane::new(2, state.config.scrollback, rect);
        pane.pty_generation = 7;
        state.workspaces[0].panes.push(pane);
        let bounds = state.canvas_bounds_from_terminal_viewport(Rect {
            x: 0,
            y: 0,
            w: 80,
            h: 25,
        });
        crate::layout::place_spawned_pane(
            &mut state.workspaces[0],
            2,
            previous,
            bounds,
            0.0,
            crate::state::TileGap::DEFAULT,
            state.config.layout.split_width_multiplier,
        );
        state.next_pane_id = 3;
        state.next_pty_generation = 8;
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
        let mut state = State::new(HyprmuxConfig::default(), Theme::default());
        state.workspaces[0].panes[0].floating = true;
        state.workspaces[0].panes[0].floating_rect = FloatRect {
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
}

/// Reconciler behavior driven through the real runtime (a follower/controller applying commits).
#[cfg(test)]
mod reconciler_tests {
    use super::*;
    use crate::HyprmuxApp;
    use crate::Msg;
    use crate::pane_lifecycle::find_pane;
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::ClientMessage;
    use crate::state::SharedSessionState;
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
                        command: None,
                        replay: false,
                        keep_open: false,
                        floating: false,
                        fullscreen: false,
                        rect: None,
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

    fn attach_follower(backend: &mut TestBackend<HyprmuxApp>, client: SessionClient) {
        let state = backend.state_mut();
        state.session_attached = true;
        state.session_client = Some(client);
        let mut shared = SharedSessionState::new(1);
        shared.controller = Some(2); // another client controls the layout; we follow.
        shared.clients = vec![
            crate::session::protocol::ClientInfo {
                id: 1,
                label: "a".into(),
                read_only: false,
                requesting_control: false,
            },
            crate::session::protocol::ClientInfo {
                id: 2,
                label: "b".into(),
                read_only: false,
                requesting_control: false,
            },
        ];
        state.shared = Some(shared);
    }

    #[test]
    fn remote_removal_closes_pane_without_sending_kill() {
        in_stack(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
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
        });
    }

    #[test]
    fn remote_addition_creates_a_ready_pane() {
        in_stack(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
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
    fn initial_session_layout_is_applied_without_geometry_transition() {
        in_stack(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.set_viewport(VIEWPORT);
            let (client, _rx) = SessionClient::test_channel();
            backend.state_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
                epoch: 1,
                name: "live".into(),
                client: Some(client),
                autostart: false,
                read_only: false,
                intent: crate::state::AttachIntent::Plain,
                left: None,
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
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.set_viewport(VIEWPORT);
            let (client, _rx) = SessionClient::test_channel();
            {
                let state = backend.state_mut();
                state.session_attached = true;
                state.session_client = Some(client);
                let mut shared = SharedSessionState::new(1);
                shared.controller = Some(1); // we are the controller: our own echoes must not apply.
                shared.clients = vec![
                    crate::session::protocol::ClientInfo {
                        id: 1,
                        label: "a".into(),
                        read_only: false,
                        requesting_control: false,
                    },
                    crate::session::protocol::ClientInfo {
                        id: 2,
                        label: "b".into(),
                        read_only: false,
                        requesting_control: false,
                    },
                ];
                state.shared = Some(shared);
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
                backend.state_mut().shared.as_ref().unwrap().layout_rev,
                7,
                "echo confirms the committed revision"
            );
        });
    }
}
