use super::namespace::*;
use super::spawn::*;
use crate::anim::GeometryAnimation;
use crate::geometry::canvas_local_point_from_mouse;
use crate::state::{Pane, PaneId, PaneIdentity, ScrollableRevealEdge, State};
use tui_lipan::Theme;
use tui_lipan::prelude::*;

fn rule(matches: &str) -> crate::config::RuleConfig {
    crate::config::RuleConfig {
        matcher: crate::config::RuleMatcher::Substring(matches.to_string()),
        float: false,
        width: None,
        height: None,
        workspace: None,
        focus: true,
        fullscreen: false,
        position: crate::config::FloatPosition::Center,
    }
}

/// A queued spawn request carrying `identity`, with the boilerplate a test does not care about.
fn spawn_request(pane_id: PaneId, generation: u64, identity: PaneIdentity) -> PaneSpawnRequest {
    PaneSpawnRequest {
        pane_id,
        local: false,
        generation,
        identity,
        cols: 80,
        rows: 24,
        env: Vec::new(),
        palette: TerminalColorPalette::default(),
    }
}

pub(crate) fn in_stack<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(body)
        .expect("spawn test thread")
        .join()
        .expect("join test thread")
}

fn scrollable_close_backend(focus: PaneId) -> tui_lipan::TestBackend<crate::AppRoot> {
    let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
    backend.set_viewport(tui_lipan::prelude::Rect {
        x: 0,
        y: 0,
        w: 80,
        h: 24,
    });
    {
        let state = backend.state_mut();
        state.config.confirm.close_pane = false;
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.layout_kind = crate::state::LayoutKind::Scrollable;
        workspace.panes.clear();
        workspace.tile_tree = crate::tiling::build_dwindle_tree(
            &[10, 30, 20],
            crate::state::SplitAxis::Horizontal,
            &[0.5, 0.5],
        );
        // Storage order intentionally differs from the tree order.
        for id in [20, 10, 30] {
            let mut pane = Pane::new(
                id,
                100,
                FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 80.0,
                    h: 24.0,
                },
            );
            pane.opening = false;
            pane.terminal_active = true;
            // Keep the post-close strip overflowing so focus synchronization must reveal the
            // selected neighbor rather than merely preserving the first remaining column.
            pane.scrollable_width = 0.80;
            workspace.panes.push(pane);
        }
        workspace.focused_pane = Some(focus);
        workspace.scrollable_anchor = Some(focus);
        state.current_mut().focused_pane = Some(focus);
    }
    backend.render();
    backend
}

#[test]
fn replay_spawn_queues_the_command_as_input_instead_of_a_wire_command() {
    let mut state = State::new(crate::config::Config::default(), Theme::default());
    request_pane_spawn(
        &mut state,
        spawn_request(
            7,
            3,
            PaneIdentity {
                launch: Some(crate::pane_launch::PaneLaunch::shell("n")),
                replay: true,
                ..PaneIdentity::default()
            },
        ),
    );
    // No client yet: the spawn is queued, with the replay command stripped from the wire
    // request and parked for post-spawn injection instead.
    assert_eq!(state.current().pending_spawns.len(), 1);
    assert_eq!(state.current().pending_spawns[0].launch, None);
    assert_eq!(
        state
            .current()
            .pending_replay_inputs
            .get(&(7, 3))
            .map(String::as_str),
        Some("n")
    );

    // A non-replay command rides the wire request as before.
    request_pane_spawn(
        &mut state,
        spawn_request(
            8,
            4,
            PaneIdentity {
                launch: Some(crate::pane_launch::PaneLaunch::shell("htop")),
                ..PaneIdentity::default()
            },
        ),
    );
    assert_eq!(
        state.current().pending_spawns[1].launch,
        Some(crate::pane_launch::PaneLaunch::shell("htop")),
        "deterministic command panes must keep command-shell semantics"
    );
    assert!(!state.current().pending_replay_inputs.contains_key(&(8, 4)));
}

/// A `--remote` pane must not carry the client's locally resolved shell argv to the server: a
/// Linux client's `/usr/bin/bash` (with a local rc-file path) cannot run on a Windows remote.
/// Empty argv makes the server resolve its own default shell.
#[test]
fn remote_spawn_sends_no_local_shell_argv() {
    // Local session: the resolved interactive shell rides the request as before.
    let mut local = State::new(crate::config::Config::default(), Theme::default());
    request_pane_spawn(&mut local, spawn_request(1, 1, PaneIdentity::default()));
    assert!(
        !local.current().pending_spawns[0].shell.is_empty(),
        "a local pane keeps its resolved interactive-shell argv"
    );

    // Remote session: shell and command_shell are emptied for server-side resolution.
    let mut remote = State::new(crate::config::Config::default(), Theme::default());
    remote.current_mut().remote_host = Some("winvm".to_string());
    request_pane_spawn(&mut remote, spawn_request(1, 1, PaneIdentity::default()));
    assert!(
        remote.current().pending_spawns[0].shell.is_empty(),
        "a --remote pane must send an empty shell argv"
    );
    assert!(
        remote.current().pending_spawns[0].command_shell.is_empty(),
        "a --remote pane must send an empty command-shell argv"
    );
}

#[test]
fn replay_inputs_survive_teardown_only_while_their_spawn_is_still_queued() {
    let mut state = State::new(crate::config::Config::default(), Theme::default());
    request_pane_spawn(
        &mut state,
        spawn_request(
            7,
            3,
            PaneIdentity {
                launch: Some(crate::pane_launch::PaneLaunch::shell("n")),
                replay: true,
                ..PaneIdentity::default()
            },
        ),
    );
    // An entry whose spawn already went out (not queued) can never complete after a
    // disconnect, and its key could be minted again once the generation counter restarts.
    state
        .current_mut()
        .pending_replay_inputs
        .insert((9, 1), "stale".to_string());

    state.prune_replay_inputs_to_pending_spawns();

    assert!(state.current().pending_replay_inputs.contains_key(&(7, 3)));
    assert!(!state.current().pending_replay_inputs.contains_key(&(9, 1)));
}

#[test]
fn close_keeps_the_pane_described_while_it_animates_out() {
    in_stack(|| {
        let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
        backend.set_viewport(tui_lipan::prelude::Rect {
            x: 0,
            y: 0,
            w: 80,
            h: 24,
        });
        {
            let state = backend.state_mut();
            state.config.confirm.close_pane = false;
            let pane = &mut state.current_mut().workspaces[0].panes[0];
            pane.opening = false;
            pane.terminal_active = true;
        }
        backend.render();

        backend
            .dispatch(crate::Msg::RunAction(crate::input::Action::Close))
            .expect("close pane");
        // The pane has to stay described for the close scale to have anything to lay out.
        // It leaves the tiling layout at once so neighbours expand, but is still rendered.
        let pane = &backend.state().current().workspaces[0].panes[0];
        assert!(pane.closing, "the pane should be animating out, not gone");
        assert_eq!(backend.state().current().workspaces[0].visible_count(), 0);
        assert_eq!(backend.state().current().focused_pane, None);
        assert!(
            backend
                .capture_ui_snapshot()
                .widgets
                .iter()
                .any(|widget| widget
                    .key
                    .as_ref()
                    .is_some_and(|key| key.as_ref() == "rozi-pane-1-0")),
            "the closing pane still renders while it scales down"
        );

        // Prune drops it once the animation has run.
        backend
            .dispatch(crate::Msg::PruneClosed(
                backend.state().runtime_epoch,
                1,
                backend.state().current().workspaces[0].panes[0].pty_generation,
            ))
            .expect("prune closed pane");
        assert!(backend.state().current().workspaces[0].panes.is_empty());
    });
}

#[test]
fn closing_middle_scrollable_pane_focuses_next_tree_neighbor_and_prunes_cleanly() {
    in_stack(|| {
        let mut backend = scrollable_close_backend(30);
        backend
            .dispatch(crate::Msg::RunAction(crate::input::Action::Close))
            .expect("close middle pane");

        let workspace = &backend.state().current().workspaces[0];
        assert!(
            workspace
                .panes
                .iter()
                .any(|pane| pane.id == 30 && pane.closing)
        );
        assert_eq!(backend.state().current().focused_pane, Some(20));
        assert_eq!(workspace.focused_pane, Some(20));
        assert_eq!(workspace.scrollable_anchor, Some(20));
        assert_eq!(backend.state().animation, GeometryAnimation::Close);
        assert_eq!(
            workspace.scrollable_reveal_edge,
            ScrollableRevealEdge::Right
        );
        assert_eq!(workspace.tiled_ids(), [10, 20]);

        let generation = workspace
            .panes
            .iter()
            .find(|pane| pane.id == 30)
            .expect("closing pane")
            .pty_generation;
        backend
            .dispatch(crate::Msg::PruneClosed(
                backend.state().runtime_epoch,
                30,
                generation,
            ))
            .expect("prune closed middle pane");
        let workspace = &backend.state().current().workspaces[0];
        assert!(workspace.panes.iter().all(|pane| pane.id != 30));
        assert_eq!(backend.state().current().focused_pane, Some(20));
        assert_eq!(workspace.scrollable_anchor, Some(20));
    });
}

#[test]
fn closing_final_scrollable_pane_focuses_previous_tree_neighbor() {
    in_stack(|| {
        let mut backend = scrollable_close_backend(20);
        backend
            .dispatch(crate::Msg::RunAction(crate::input::Action::Close))
            .expect("close final pane");

        let workspace = &backend.state().current().workspaces[0];
        assert!(
            workspace
                .panes
                .iter()
                .any(|pane| pane.id == 20 && pane.closing)
        );
        assert_eq!(backend.state().current().focused_pane, Some(30));
        assert_eq!(workspace.focused_pane, Some(30));
        assert_eq!(workspace.scrollable_anchor, Some(30));
        assert_eq!(workspace.tiled_ids(), [10, 30]);
    });
}

#[test]
fn closing_the_last_scrollable_tile_clears_its_anchor() {
    in_stack(|| {
        let mut backend = scrollable_close_backend(30);
        {
            let state = backend.state_mut();
            let workspace = &mut state.current_mut().workspaces[0];
            workspace.panes.retain(|pane| pane.id == 30);
            workspace.tile_tree = Some(crate::tiling::DwindleTree::Leaf(30));
            workspace.focused_pane = Some(30);
            workspace.scrollable_anchor = Some(30);
            workspace.scrollable_reveal_edge = ScrollableRevealEdge::Right;
            state.current_mut().focused_pane = Some(30);
        }
        backend.render();
        backend
            .dispatch(crate::Msg::RunAction(crate::input::Action::Close))
            .expect("close last Scrollable tile");

        let workspace = &backend.state().current().workspaces[0];
        assert_eq!(backend.state().current().focused_pane, None);
        assert_eq!(workspace.focused_pane, None);
        assert_eq!(workspace.scrollable_anchor, None);
        assert_eq!(workspace.scrollable_reveal_edge, ScrollableRevealEdge::Left);
        assert!(
            workspace
                .panes
                .iter()
                .any(|pane| pane.id == 30 && pane.closing)
        );
    });
}

#[test]
fn closing_nonfocused_scrollable_pane_preserves_focus_and_anchor() {
    in_stack(|| {
        let mut backend = scrollable_close_backend(30);
        backend.state_mut().sidebar.panels[0].active_tab =
            Some(crate::config::SidebarTabId::new("panes"));
        // Tree order is [10, 30, 20], so row 1 is pane 10; pane 30 remains focused.
        backend
            .dispatch(crate::Msg::SidebarRowClose { panel: 0, index: 1 })
            .expect("arm nonfocused close");
        backend
            .dispatch(crate::Msg::SidebarRowClose { panel: 0, index: 1 })
            .expect("close nonfocused pane");

        let workspace = &backend.state().current().workspaces[0];
        assert!(
            workspace
                .panes
                .iter()
                .any(|pane| pane.id == 10 && pane.closing)
        );
        assert_eq!(backend.state().current().focused_pane, Some(30));
        assert_eq!(workspace.focused_pane, Some(30));
        assert_eq!(workspace.scrollable_anchor, Some(30));
    });
}

#[test]
fn closing_a_nonfocused_scrollable_anchor_remaps_without_changing_focus() {
    in_stack(|| {
        let mut backend = scrollable_close_backend(30);
        {
            let state = backend.state_mut();
            let mut floating = Pane::new(
                99,
                100,
                FloatRect {
                    x: 5.0,
                    y: 5.0,
                    w: 20.0,
                    h: 10.0,
                },
            );
            floating.floating = true;
            floating.opening = false;
            floating.terminal_active = true;
            let workspace = &mut state.current_mut().workspaces[0];
            workspace.panes.push(floating);
            workspace.focused_pane = Some(99);
            workspace.scrollable_anchor = Some(30);
            workspace.scrollable_reveal_edge = ScrollableRevealEdge::Right;
            state.current_mut().focused_pane = Some(99);
            state.sidebar.panels[0].active_tab = Some(crate::config::SidebarTabId::new("panes"));
        }
        backend.render();
        let focus_events =
            backend
                .state()
                .event_hub
                .subscribe(Some(std::collections::HashSet::from([
                    crate::events::EventKind::FocusChanged,
                ])));

        // Tree order is [10, 30, 20], so row 2 closes the anchored middle tile.
        backend
            .dispatch(crate::Msg::SidebarRowClose { panel: 0, index: 2 })
            .expect("arm anchored close");
        backend
            .dispatch(crate::Msg::SidebarRowClose { panel: 0, index: 2 })
            .expect("close anchored tile");

        let workspace = &backend.state().current().workspaces[0];
        assert!(
            workspace
                .panes
                .iter()
                .any(|pane| pane.id == 30 && pane.closing)
        );
        assert_eq!(backend.state().current().focused_pane, Some(99));
        assert_eq!(workspace.focused_pane, Some(99));
        assert_eq!(workspace.scrollable_anchor, Some(20));
        assert_eq!(
            workspace.scrollable_reveal_edge,
            ScrollableRevealEdge::Right
        );
        assert!(focus_events.try_recv().is_err());
    });
}

#[test]
fn closing_an_inactive_scrollable_anchor_remaps_its_workspace_only() {
    in_stack(|| {
        let mut backend = scrollable_close_backend(30);
        {
            let state = backend.state_mut();
            let rect = FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            };
            let workspace = &mut state.current_mut().workspaces[1];
            workspace.layout_kind = crate::state::LayoutKind::Scrollable;
            workspace.panes.clear();
            for id in [120, 110, 130] {
                let mut pane = Pane::new(id, 100, rect);
                pane.opening = false;
                pane.terminal_active = true;
                workspace.panes.push(pane);
            }
            workspace.tile_tree = crate::tiling::build_dwindle_tree(
                &[110, 130, 120],
                crate::state::SplitAxis::Horizontal,
                &[0.5, 0.5],
            );
            workspace.focused_pane = Some(130);
            workspace.scrollable_anchor = Some(130);
            workspace.scrollable_reveal_edge = ScrollableRevealEdge::Right;
            state.sidebar.panels[0].active_tab = Some(crate::config::SidebarTabId::new("panes"));
        }
        backend.render();
        let focus_events =
            backend
                .state()
                .event_hub
                .subscribe(Some(std::collections::HashSet::from([
                    crate::events::EventKind::FocusChanged,
                ])));

        // Active rows 0–3, spacer 4, inactive header 5; row 7 is pane 130 in [110, 130, 120].
        backend
            .dispatch(crate::Msg::SidebarRowClose { panel: 0, index: 7 })
            .expect("arm inactive anchored close");
        backend
            .dispatch(crate::Msg::SidebarRowClose { panel: 0, index: 7 })
            .expect("close inactive anchored tile");

        let current = backend.state().current();
        let active = &current.workspaces[0];
        let inactive = &current.workspaces[1];
        assert_eq!(current.focused_pane, Some(30));
        assert_eq!(active.focused_pane, Some(30));
        assert_eq!(active.scrollable_anchor, Some(30));
        assert!(
            inactive
                .panes
                .iter()
                .any(|pane| pane.id == 130 && pane.closing)
        );
        assert_eq!(inactive.scrollable_anchor, Some(120));
        assert_eq!(inactive.scrollable_reveal_edge, ScrollableRevealEdge::Right);
        assert!(focus_events.try_recv().is_err());
    });
}

#[test]
fn close_popup_keeps_the_popup_described_until_it_is_pruned() {
    in_stack(|| {
        let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
        backend.set_viewport(tui_lipan::prelude::Rect {
            x: 0,
            y: 0,
            w: 80,
            h: 24,
        });
        {
            let state = backend.state_mut();
            let mut popup = Pane::new(
                crate::state::POPUP_PANE_ID,
                state.config.scrollback,
                FloatRect {
                    x: 10.0,
                    y: 5.0,
                    w: 40.0,
                    h: 12.0,
                },
            );
            popup.opening = false;
            popup.terminal_active = true;
            state.popup = Some(popup);
        }
        backend.render();

        backend
            .dispatch(crate::Msg::ClosePopup)
            .expect("close popup");
        let popup = backend
            .state()
            .popup
            .as_ref()
            .expect("popup still described");
        assert!(popup.closing);
        let generation = popup.pty_generation;
        assert!(
            backend
                .capture_ui_snapshot()
                .widgets
                .iter()
                .any(|widget| widget
                    .key
                    .as_ref()
                    .is_some_and(|key| { key.as_ref() == "rozi-pane-4294967295-0" })),
            "the closing popup still renders while it scales down"
        );

        backend
            .dispatch(crate::Msg::PruneClosed(
                backend.state().runtime_epoch,
                crate::state::POPUP_PANE_ID,
                generation,
            ))
            .expect("prune closed popup");
        assert!(backend.state().popup.is_none());
    });
}

#[test]
fn disabled_close_animation_still_prunes_the_pane() {
    in_stack(|| {
        let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
        backend.set_viewport(tui_lipan::prelude::Rect {
            x: 0,
            y: 0,
            w: 80,
            h: 24,
        });
        {
            let state = backend.state_mut();
            state.config.confirm.close_pane = false;
            state.config.animations.enabled = false;
            state.current_mut().workspaces[0].panes[0].opening = false;
        }
        backend.render();
        // Read the generation before closing: with animations off the prune delay is zero, so
        // the scheduled `PruneClosed` can land on its own timer at any point after the close
        // and take the pane out from under a later read.
        let generation = backend.state().current().workspaces[0].panes[0].pty_generation;
        backend
            .dispatch(crate::Msg::RunAction(crate::input::Action::Close))
            .expect("close pane");

        // Whether the timer got there first or not, the message drives removal and the pane
        // is gone either way.
        backend
            .dispatch(crate::Msg::PruneClosed(
                backend.state().runtime_epoch,
                1,
                generation,
            ))
            .expect("prune closed pane");
        assert!(backend.state().current().workspaces[0].panes.is_empty());
        assert!(backend.capture_ui_snapshot().widgets.iter().all(|widget| {
            widget
                .key
                .as_ref()
                .is_none_or(|key| key.as_ref() != "rozi-pane-1-0")
        }));
    });
}

#[test]
fn workspace_switch_replaces_the_canvas_host_without_retaining_old_panes() {
    in_stack(|| {
        let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
        backend.set_viewport(tui_lipan::prelude::Rect {
            x: 0,
            y: 0,
            w: 80,
            h: 24,
        });
        let mut pane = Pane::new(2, 100, FloatRect::default());
        pane.opening = false;
        backend.state_mut().current_mut().workspaces[1]
            .panes
            .push(pane);
        crate::tiling::append_tiled_window(&mut backend.state_mut().current_mut().workspaces[1], 2);
        backend.render();

        backend
            .dispatch(crate::Msg::RunAction(
                crate::input::Action::SwitchWorkspace(1),
            ))
            .expect("switch workspace");
        let snapshot = backend.capture_ui_snapshot();
        assert!(snapshot.widgets.iter().any(|widget| {
            widget
                .key
                .as_ref()
                .is_some_and(|key| key.as_ref() == "rozi-pane-2-0")
        }));
        assert!(snapshot.widgets.iter().all(|widget| {
            widget
                .key
                .as_ref()
                .is_none_or(|key| key.as_ref() != "rozi-pane-1-0")
        }));
    });
}

#[test]
fn removing_a_pane_clears_modes_that_target_it() {
    let mut state = State::new(crate::config::Config::default(), Theme::default());
    state.copy_mode = Some(crate::state::CopyModeState {
        target: 1,
        navigation: TerminalCopyMode::new(0, 0, 0),
        search_matches: Vec::new(),
        search_current: 0,
        search_truncated: false,
    });
    state.hint_mode = Some(crate::state::HintModeState {
        target: 1,
        matches: Vec::new(),
        labels: Vec::new(),
        input: String::new(),
        offset: 0,
    });
    state.rename = Some(crate::state::PaneRenameState::new(1, "pane"));
    state.copy_feedback_target = Some((state.runtime_epoch, 1));

    remove_pane(&mut state, 1);

    assert!(state.copy_mode.is_none());
    assert!(state.hint_mode.is_none());
    assert!(state.rename.is_none());
    assert!(state.copy_feedback_target.is_none());
    assert_eq!(state.mode, crate::state::Mode::Normal);
}

#[test]
fn spawn_focus_can_update_target_workspace_without_stealing_active_focus() {
    let mut state = State::new(crate::config::Config::default(), Theme::default());
    state.current_mut().active_workspace = 0;
    state.current_mut().focused_pane = Some(1);
    apply_spawn_focus(
        &mut state,
        2,
        7,
        SpawnPlacement {
            focus: false,
            ..Default::default()
        },
    );
    assert_eq!(state.current().workspaces[2].focused_pane, Some(7));
    assert_eq!(state.current().active_workspace, 0);
    assert_eq!(state.current().focused_pane, Some(1));
    apply_spawn_focus(&mut state, 2, 8, SpawnPlacement::default());
    assert_eq!(state.current().active_workspace, 2);
    assert_eq!(state.current().focused_pane, Some(8));
}

#[test]
fn non_focusing_spawn_on_active_scrollable_keeps_viewport_anchor() {
    let bounds = FloatRect {
        x: 0.0,
        y: 0.0,
        w: 100.0,
        h: 24.0,
    };
    let mut state = State::new(crate::config::Config::default(), Theme::default());
    {
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.layout_kind = crate::state::LayoutKind::Scrollable;
        for id in [1_u32, 2] {
            workspace.panes.push(Pane::new(id, 100, bounds));
            crate::tiling::append_tiled_window(workspace, id);
        }
        workspace.focused_pane = Some(1);
        workspace.scrollable_anchor = Some(1);
        workspace.panes.push(Pane::new(3, 100, bounds));
        crate::tiling::append_tiled_window(workspace, 3);
    }
    state.current_mut().active_workspace = 0;
    state.current_mut().focused_pane = Some(1);

    apply_spawn_focus(
        &mut state,
        0,
        3,
        SpawnPlacement {
            focus: false,
            ..Default::default()
        },
    );

    assert_eq!(state.current().focused_pane, Some(1));
    assert_eq!(state.current().workspaces[0].focused_pane, Some(1));
    assert_eq!(state.current().workspaces[0].scrollable_anchor, Some(1));
    let render_focus = state.current().workspaces[0]
        .focused_pane
        .or(state.current().focused_pane);
    assert_eq!(render_focus, Some(1));

    let placements = crate::layout::workspace_target_rects(
        &state.current().workspaces[0],
        bounds,
        0.0,
        crate::state::TileGap::DEFAULT,
    );
    let anchored = placements.iter().find(|p| p.id == 1).expect("pane A");
    assert!(
        (anchored.rect.x - bounds.x).abs() < f32::EPSILON,
        "viewport must stay on A after a non-focusing spawn"
    );
}

#[test]
fn non_focusing_tiled_spawn_with_floating_focus_anchors_existing_tiled() {
    let bounds = FloatRect {
        x: 0.0,
        y: 0.0,
        w: 100.0,
        h: 24.0,
    };
    let mut state = State::new(crate::config::Config::default(), Theme::default());
    {
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.layout_kind = crate::state::LayoutKind::Scrollable;
        workspace.panes.push(Pane::new(1, 100, bounds));
        crate::tiling::append_tiled_window(workspace, 1);
        let mut floating = Pane::new(2, 100, bounds);
        floating.floating = true;
        workspace.panes.push(floating);
        workspace.focused_pane = Some(2);
        workspace.scrollable_anchor = Some(99);
        workspace.panes.push(Pane::new(3, 100, bounds));
        crate::tiling::append_tiled_window(workspace, 3);
    }
    state.current_mut().active_workspace = 0;
    state.current_mut().focused_pane = Some(2);

    apply_spawn_focus(
        &mut state,
        0,
        3,
        SpawnPlacement {
            focus: false,
            ..Default::default()
        },
    );

    assert_eq!(state.current().focused_pane, Some(2));
    assert_eq!(state.current().workspaces[0].focused_pane, Some(2));
    assert_eq!(state.current().workspaces[0].scrollable_anchor, Some(1));
    assert_eq!(
        state.current().workspaces[0]
            .focused_pane
            .or(state.current().focused_pane),
        Some(2)
    );

    let placements = crate::layout::workspace_target_rects(
        &state.current().workspaces[0],
        bounds,
        0.0,
        crate::state::TileGap::DEFAULT,
    );
    let anchored = placements
        .iter()
        .find(|p| p.id == 1)
        .expect("existing tiled");
    assert!(
        (anchored.rect.x - bounds.x).abs() < f32::EPSILON,
        "stale/missing anchor must fall back to a pre-existing tiled pane, not the spawn"
    );
}

#[test]
fn interactive_command_spawn_applies_configured_rule() {
    let mut config = crate::config::Config::default();
    let mut configured = rule("btop");
    configured.workspace = Some(2);
    configured.float = true;
    configured.width = Some(0.7);
    configured.height = Some(0.8);
    configured.fullscreen = true;
    configured.focus = false;
    config.rules.push(configured);
    let mut state = State::new(config, Theme::default());
    state.current_mut().workspaces[2].focused_pane = Some(7);

    let (workspace, previous_focused, placement) =
        interactive_spawn_target(&state, 0, None, Some("exec btop"), None, None);

    assert_eq!(workspace, 2);
    assert_eq!(previous_focused, Some(7));
    assert_eq!(
        placement,
        SpawnPlacement {
            float: Some(SpawnFloat {
                width: 0.7,
                height: 0.8,
                position: crate::config::FloatPosition::Center,
                pointer: None,
            }),
            fullscreen: true,
            focus: false,
        }
    );
}

#[test]
fn interactive_spawn_without_command_keeps_source_and_default_placement() {
    let mut config = crate::config::Config::default();
    config.rules.push(rule("btop"));
    let state = State::new(config, Theme::default());

    let target = interactive_spawn_target(&state, 0, Some(1), None, None, None);

    assert_eq!(target, (0, Some(1), SpawnPlacement::default()));
}

#[test]
fn focus_override_beats_the_matched_rule_without_touching_placement() {
    let mut config = crate::config::Config::default();
    let mut configured = rule("btop");
    configured.workspace = Some(3);
    configured.fullscreen = true;
    configured.focus = true;
    config.rules.push(configured);
    let state = State::new(config, Theme::default());

    // The control endpoint's default: never move focus, but keep the rule's placement.
    let (workspace, _, placement) =
        interactive_spawn_target(&state, 0, None, Some("exec btop"), Some(false), None);
    assert_eq!(workspace, 3);
    assert!(!placement.focus);
    assert!(placement.fullscreen);

    // `--focus` overrides a rule that asked for no focus.
    let mut config = crate::config::Config::default();
    let mut configured = rule("btop");
    configured.focus = false;
    config.rules.push(configured);
    let state = State::new(config, Theme::default());
    let (_, _, placement) =
        interactive_spawn_target(&state, 0, None, Some("exec btop"), Some(true), None);
    assert!(placement.focus);
}

#[test]
fn pane_env_skips_control_socket_when_remote_attached() {
    let pane = Pane::new(1, 100, FloatRect::default());
    let path = std::path::Path::new("/tmp/rozi-control.sock");
    let local = pane_env(Some(path), &pane, false, &[]);
    assert!(
        local
            .iter()
            .any(|(k, v)| k == "ROZI_SOCKET" && v.contains("rozi-control")),
        "local attach should inject ROZI_SOCKET: {local:?}"
    );
    let remote = pane_env(Some(path), &pane, true, &[]);
    assert!(
        remote.iter().all(|(k, _)| k != "ROZI_SOCKET"),
        "remote attach must not inject client ROZI_SOCKET: {remote:?}"
    );
    assert!(remote.iter().any(|(k, _)| k == "ROZI"));
    assert!(remote.iter().any(|(k, _)| k == "ROZI_PANE"));
}

/// A pane is told where rozi is so a script does not have to assume a `PATH` install; a remote
/// pane is not, because this client's path names nothing on the other host.
#[test]
fn pane_env_advertises_the_binary_locally_but_not_remotely() {
    let pane = Pane::new(1, 100, FloatRect::default());
    let local = pane_env(None, &pane, false, &[]);
    let advertised = local
        .iter()
        .find(|(k, _)| k == "ROZI_BIN")
        .map(|(_, v)| v.clone())
        .expect("local pane learns ROZI_BIN");
    assert_eq!(
        std::path::PathBuf::from(&advertised),
        std::env::current_exe().expect("current exe")
    );

    let remote = pane_env(None, &pane, true, &[]);
    assert!(
        remote.iter().all(|(k, _)| k != "ROZI_BIN"),
        "remote attach must not advertise the client binary: {remote:?}"
    );
}

/// A spawn parks the Scrollable viewport on the new pane. The scratch spawn skipped that, so
/// the anchor stayed on whatever was focused first - and switching the dropdown to Scrollable
/// later revealed that stale pane, leaving the actually-focused one off screen.
#[test]
fn spawning_into_the_scratchpad_parks_the_scrollable_anchor_on_the_new_pane() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            use crate::AppRoot;
            use crate::state::Pane;
            use tui_lipan::TestBackend;
            use tui_lipan::prelude::{FloatRect, Rect};

            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            });
            {
                let state = backend.state_mut();
                let mut pane = Pane::new(1, 100, FloatRect::default());
                pane.opening = false;
                state.scratch.panes.push(pane);
                crate::tiling::append_tiled_window(&mut state.scratch, 1);
                state.scratch.focused_pane = Some(1);
                state.scratch.scrollable_anchor = Some(1);
                state.scratch_visible = true;
            }
            backend.render();

            backend
                .dispatch(crate::Msg::RunAction(crate::input::Action::Spawn))
                .expect("spawn into the scratchpad");

            let spawned = backend
                .state()
                .scratch
                .focused_pane
                .expect("the spawn takes focus");
            assert_ne!(spawned, 1, "a second pane was created");
            assert_eq!(
                backend.state().scratch.scrollable_anchor,
                Some(spawned),
                "the strip must be parked on the pane that now has focus"
            );
        })
        .expect("spawn scratch anchor test thread")
        .join()
        .expect("scratch anchor test thread panicked");
}

#[test]
fn spawn_float_rect_places_pointer_and_edges() {
    let bounds = FloatRect {
        x: 0.0,
        y: 0.0,
        w: 100.0,
        h: 20.0,
    };
    let pointer = SpawnFloat {
        width: 0.42,
        height: 0.42,
        position: crate::config::FloatPosition::Cursor,
        pointer: Some((50.0, 10.0)),
    }
    .rect(bounds);
    assert!(
        (pointer.x - 29.0).abs() < 0.01,
        "centered on x, got {}",
        pointer.x
    );
    assert!(
        (pointer.y - 5.8).abs() < 0.01,
        "centered on y, got {}",
        pointer.y
    );
    assert!((pointer.w - 42.0).abs() < 0.01);
    assert!((pointer.h - 8.4).abs() < 0.01);

    let top_right = SpawnFloat {
        width: 0.5,
        height: 0.5,
        position: crate::config::FloatPosition::TopRight,
        pointer: None,
    }
    .rect(bounds);
    assert!((top_right.x - 50.0).abs() < f32::EPSILON);
    assert!((top_right.y).abs() < f32::EPSILON);

    let bottom = SpawnFloat {
        width: 0.4,
        height: 0.5,
        position: crate::config::FloatPosition::Bottom,
        pointer: None,
    }
    .rect(bounds);
    assert!((bottom.x - 30.0).abs() < f32::EPSILON);
    assert!((bottom.y - 10.0).abs() < f32::EPSILON);
}

#[test]
fn spawn_float_opens_a_floating_pane_at_the_mouse_pointer() {
    in_stack(|| {
        use crate::AppRoot;
        use tui_lipan::TestBackend;
        use tui_lipan::core::event::{MouseEvent, MouseKind};
        use tui_lipan::prelude::{KeyMods, Rect};

        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        });
        backend.render();
        {
            let pane = backend.state_mut().current_mut().workspaces[0]
                .panes
                .iter_mut()
                .find(|pane| pane.id == 1)
                .expect("initial pane");
            pane.opening = false;
            pane.terminal_active = true;
            // A caret elsewhere must not steal placement from the pointer.
            pane.terminal.process_server_output(b"\x1b[6;12H");
        }

        backend
            .send_mouse(MouseEvent {
                x: 40,
                y: 12,
                kind: MouseKind::Moved,
                mods: KeyMods::NONE,
            })
            .expect("record pointer");

        let bounds = backend
            .state()
            .canvas_bounds_from_terminal_viewport(backend.viewport());
        let expected = canvas_local_point_from_mouse(
            40,
            12,
            bounds,
            backend
                .state()
                .terminal_content_left_offset(backend.viewport()),
            backend.state().content_top_offset(),
        );

        backend
            .dispatch(crate::Msg::RunAction(crate::input::Action::SpawnFloat))
            .expect("spawn floating pane");

        let spawned_id = backend.state().focused_pane().expect("spawn takes focus");
        assert_ne!(spawned_id, 1);
        let spawned = backend.state().current().workspaces[0]
            .panes
            .iter()
            .find(|pane| pane.id == spawned_id)
            .expect("spawned pane");
        assert!(spawned.floating, "the new pane must be floating");
        assert!(
            !backend.state().current().workspaces[0]
                .tiled_ids()
                .contains(&spawned_id),
            "a pointer-spawned float is not in the tile tree"
        );
        assert!(
            (spawned.floating_rect.x + spawned.floating_rect.w / 2.0 - expected.0).abs() < 0.51
                && (spawned.floating_rect.y + spawned.floating_rect.h / 2.0 - expected.1).abs()
                    < 0.51,
            "pane center should sit on the pointer, got {:?} want {expected:?}",
            (
                spawned.floating_rect.x + spawned.floating_rect.w / 2.0,
                spawned.floating_rect.y + spawned.floating_rect.h / 2.0
            )
        );
    });
}

mod close_animation {
    use super::in_stack;
    use std::time::Duration;

    fn pane_rect(backend: &tui_lipan::TestBackend<crate::AppRoot>) -> Option<(i16, i16, u16, u16)> {
        backend
            .capture_ui_snapshot()
            .widgets
            .iter()
            .find(|w| {
                w.key
                    .as_ref()
                    .is_some_and(|k| k.as_ref() == "rozi-pane-1-0")
            })
            .map(|w| (w.rect.x, w.rect.y, w.rect.w, w.rect.h))
    }

    /// The close animation is the spawn animation in reverse: the pane scales toward its centre on
    /// **both** axes, so its border shrinks with it. A height-only collapse would clip the bottom
    /// border away instead. Floating panes animate exactly like tiled ones.
    #[test]
    fn a_closing_pane_scales_down_on_both_axes() {
        for floating in [false, true] {
            in_stack(move || {
                let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
                backend.set_viewport(tui_lipan::prelude::Rect {
                    x: 0,
                    y: 0,
                    w: 80,
                    h: 24,
                });
                {
                    let state = backend.state_mut();
                    state.config.confirm.close_pane = false;
                    let pane = &mut state.current_mut().workspaces[0].panes[0];
                    pane.opening = false;
                    pane.terminal_active = true;
                    pane.floating = floating;
                }
                backend.render();
                let (_, _, w0, h0) = pane_rect(&backend).expect("pane renders");

                backend
                    .dispatch(crate::Msg::RunAction(crate::input::Action::Close))
                    .expect("close");

                // Front-loaded: the shrink has to be visible before the fade hides it, so the very
                // first tick must already move. An EaseInOutCubic ramp would still be at full size.
                backend.advance(Duration::from_millis(25));
                let (x1, y1, w1, h1) = pane_rect(&backend).expect("closing pane still renders");
                assert!(
                    w1 < w0 && h1 < h0,
                    "closing={floating}: both axes should shrink on the first tick, \
                     got {w1}x{h1} from {w0}x{h0}"
                );
                assert!(
                    x1 > 0 && y1 > 0,
                    "the pane should pull in toward its centre"
                );

                // And it keeps shrinking rather than snapping.
                backend.advance(Duration::from_millis(25));
                let (_, _, w2, h2) = pane_rect(&backend).expect("still closing");
                assert!(w2 < w1 && h2 <= h1, "the scale should continue: {w2}x{h2}");
            });
        }
    }

    /// A pane the user closed exits by definition. Reporting that exit is noise, and worse, the
    /// `[exited]` title and failure toast appear over the pane while it is still animating out.
    #[test]
    fn a_user_closed_pane_does_not_report_its_own_exit() {
        in_stack(|| {
            let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
            backend.set_viewport(tui_lipan::prelude::Rect {
                x: 0,
                y: 0,
                w: 80,
                h: 24,
            });
            let generation = {
                let state = backend.state_mut();
                state.config.confirm.close_pane = false;
                state.config.pane.hold_on_exit = true;
                let pane = &mut state.current_mut().workspaces[0].panes[0];
                pane.opening = false;
                pane.terminal_active = true;
                pane.pty_generation
            };
            backend.render();

            backend
                .dispatch(crate::Msg::RunAction(crate::input::Action::Close))
                .expect("close");
            let epoch = backend.state().runtime_epoch;

            // The server reports the kill we asked for.
            backend
                .dispatch(crate::Msg::SessionExited {
                    epoch,
                    pane_id: 1,
                    local: false,
                    generation,
                    code: 1,
                })
                .expect("exit frame");

            let text = backend.capture_frame().plain_text();
            assert!(
                !text.contains("exited (1)"),
                "closing our own pane must not toast its exit: {text}"
            );
            // The other half of the same noise: the marker would rewrite the titlebar while the
            // pane is animating out from under it.
            assert!(
                !text.contains("[exited"),
                "a closing pane must not wear its exit code: {text}"
            );
            // `hold_on_exit` must not keep a pane the user explicitly closed.
            assert!(
                backend.state().current().workspaces[0].panes[0].closing,
                "the pane should still be closing, not held open"
            );
        });
    }

    /// The other side of the marker rule: a pane `hold_on_exit` keeps in the layout is staying, so
    /// its exit code is the only thing saying why it is inert and respawnable.
    #[test]
    fn a_held_exited_pane_still_wears_its_exit_code() {
        in_stack(|| {
            let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
            backend.set_viewport(tui_lipan::prelude::Rect {
                x: 0,
                y: 0,
                w: 80,
                h: 24,
            });
            let generation = {
                let state = backend.state_mut();
                state.config.pane.hold_on_exit = true;
                let pane = &mut state.current_mut().workspaces[0].panes[0];
                pane.opening = false;
                pane.terminal_active = true;
                pane.pty_generation
            };
            backend.render();
            let epoch = backend.state().runtime_epoch;

            // The shell exits on its own - nobody closed this pane.
            backend
                .dispatch(crate::Msg::SessionExited {
                    epoch,
                    pane_id: 1,
                    local: false,
                    generation,
                    code: 3,
                })
                .expect("exit frame");

            assert!(
                !backend.state().current().workspaces[0].panes[0].closing,
                "hold_on_exit should keep the pane in the layout"
            );
            let text = backend.capture_frame().plain_text();
            assert!(
                text.contains("[exited 3]"),
                "a held pane must say why it is inert: {text}"
            );
        });
    }

    #[test]
    fn find_pane_in_namespace_does_not_cross_local_and_shared_ids() {
        in_stack(|| {
            let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
            let rect = tui_lipan::prelude::FloatRect::default();
            let mut shared = crate::state::Pane::new(7, 100, rect);
            shared.title = "shared".into();
            backend.state_mut().current_mut().workspaces[0]
                .panes
                .push(shared);
            let mut local = crate::state::Pane::new(7, 100, rect);
            local.title = "local".into();
            backend.state_mut().scratch.panes.push(local);
            backend.state_mut().scratch_visible = true;

            assert_eq!(
                super::find_pane_in_namespace(backend.state(), 7, false)
                    .unwrap()
                    .title,
                "shared"
            );
            assert_eq!(
                super::find_pane_in_namespace(backend.state(), 7, true)
                    .unwrap()
                    .title,
                "local"
            );
            assert_eq!(
                super::find_pane(backend.state(), 7).unwrap().title,
                "local",
                "stacking lookup still prefers scratch; session events must not use it"
            );
        });
    }
}
