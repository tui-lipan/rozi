use super::*;
use crate::Msg;
use crate::session::client::SessionClient;
use crate::session::protocol::{ClientInfo, PaneRuntimeState};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use tui_lipan::TestBackend;

fn agent_pane(status: &str) -> crate::pane::TerminalPane {
    let mut pane = crate::pane::TerminalPane::new(100);
    pane.detected_agent = Some(crate::session::protocol::DetectedAgent {
        kind: crate::session::protocol::AgentKind::Claude,
        state: crate::session::protocol::DetectedAgentState::Idle,
    });
    pane.reported_status = Some(crate::session::protocol::PaneStatus {
        value: status.to_string(),
        reason: None,
        set_at: 1,
    });
    pane
}

fn install_search_scan(
    backend: &mut TestBackend<crate::AppRoot>,
    target: crate::state::PaneId,
    query: &str,
) -> u64 {
    let pane_end = crate::pane_lifecycle::find_pane(backend.state(), target)
        .expect("search target")
        .terminal
        .search_line_count();
    let state = backend.state_mut();
    let epoch = state.search_scan_epoch.wrapping_add(1);
    state.search_scan_epoch = epoch;
    state.search_scan_scheduled_epoch = Some(epoch);
    let mut search = crate::state::ScrollbackSearchState::new(target);
    search.input.set_text(query);
    search.scan = Some(crate::state::ScrollbackSearchScan {
        epoch,
        query: Arc::from(query),
        panes: Arc::from([target]),
        pane_ends: Arc::from([pane_end]),
        pane_index: 0,
        line_cursor: 0,
        first_jump_done: false,
    });
    search.refresh_match_status();
    state.search = Some(search);
    epoch
}

fn search_output_lines(count: usize) -> Vec<u8> {
    (0..count)
        .map(|index| format!("needle-{index}\r\n"))
        .collect::<String>()
        .into_bytes()
}

#[test]
fn pane_output_restarts_partial_search_and_rejects_its_stale_chunk() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(crate::AppRoot::default());
            let epoch = backend.state().runtime_epoch;
            let target = backend
                .state()
                .current()
                .focused_pane
                .expect("focused pane");
            let generation = 7;
            {
                let pane =
                    crate::pane_lifecycle::find_pane_mut(backend.state_mut(), target).unwrap();
                pane.pty_generation = generation;
                pane.terminal.bind_session(target, generation);
                pane.terminal
                    .process_server_output(&search_output_lines(700));
            }
            let old_epoch = install_search_scan(&mut backend, target, "needle");
            assert!(matches!(
                crate::ops::search::advance_search_scan(backend.state_mut(), old_epoch, 100),
                crate::ops::search::SearchScanAdvance::Running { .. }
            ));
            assert!(
                !backend
                    .state()
                    .search
                    .as_ref()
                    .expect("partial search")
                    .matches
                    .is_empty()
            );

            let level = backend
                .update_level(Msg::SessionOutput {
                    epoch,
                    pane_id: target,
                    local: false,
                    generation,
                    bytes: b"live needle\r\n".to_vec(),
                })
                .expect("apply live output");
            assert_eq!(level, tui_lipan::UpdateLevel::Full);
            let restarted_epoch = backend.state().search_scan_epoch;
            assert_ne!(old_epoch, restarted_epoch);
            let search = backend.state().search.as_ref().expect("restarted search");
            assert!(search.matches.is_empty());
            assert_eq!(search.scan.as_ref().expect("scan").epoch, restarted_epoch);
            assert_eq!(
                search.scan.as_ref().expect("scan").panes.as_ref(),
                &[target]
            );
            assert_eq!(
                crate::ops::search::advance_search_scan(backend.state_mut(), old_epoch, 1),
                crate::ops::search::SearchScanAdvance::Stale
            );

            let stale_level = backend
                .update_level(Msg::SearchScanChunk { epoch: old_epoch })
                .expect("deliver stale queued chunk");
            assert_eq!(stale_level, tui_lipan::UpdateLevel::None);
            assert_eq!(
                backend.state().search_scan_scheduled_epoch,
                Some(restarted_epoch)
            );
            backend
                .update_level(Msg::SearchScanChunk { epoch: old_epoch })
                .expect("reject duplicate stale chunk");
            assert_eq!(
                backend.state().search_scan_scheduled_epoch,
                Some(restarted_epoch)
            );

            let offset_before_activation =
                crate::pane_lifecycle::find_pane(backend.state(), target)
                    .unwrap()
                    .terminal
                    .scrollback_offset();
            backend
                .update_level(Msg::SearchActivate(0))
                .expect("empty restarted result is not actionable");
            assert_eq!(
                crate::pane_lifecycle::find_pane(backend.state(), target)
                    .unwrap()
                    .terminal
                    .scrollback_offset(),
                offset_before_activation
            );
        })
        .expect("spawn partial live-output search test")
        .join()
        .expect("partial live-output search test completes");
}

#[test]
fn pane_output_restarts_a_completed_search() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(crate::AppRoot::default());
            let epoch = backend.state().runtime_epoch;
            let target = backend
                .state()
                .current()
                .focused_pane
                .expect("focused pane");
            let generation = 9;
            {
                let pane =
                    crate::pane_lifecycle::find_pane_mut(backend.state_mut(), target).unwrap();
                pane.pty_generation = generation;
                pane.terminal.bind_session(target, generation);
                pane.terminal
                    .process_server_output(&search_output_lines(40));
            }
            let old_epoch = install_search_scan(&mut backend, target, "needle");
            loop {
                if matches!(
                    crate::ops::search::advance_search_scan(backend.state_mut(), old_epoch, 17),
                    crate::ops::search::SearchScanAdvance::Complete { .. }
                ) {
                    break;
                }
            }
            backend.state_mut().search_scan_scheduled_epoch = None;
            assert!(
                !backend
                    .state()
                    .search
                    .as_ref()
                    .expect("completed search")
                    .matches
                    .is_empty()
            );

            backend
                .update_level(Msg::SessionOutput {
                    epoch,
                    pane_id: target,
                    local: false,
                    generation,
                    bytes: b"post-completion\r\n".to_vec(),
                })
                .expect("apply output after completion");
            let restarted_epoch = backend.state().search_scan_epoch;
            assert_ne!(old_epoch, restarted_epoch);
            let search = backend.state().search.as_ref().expect("restarted search");
            assert!(search.matches.is_empty());
            assert_eq!(search.scan.as_ref().expect("scan").epoch, restarted_epoch);
            assert_eq!(
                backend.state().search_scan_scheduled_epoch,
                Some(restarted_epoch)
            );
        })
        .expect("spawn completed live-output search test")
        .join()
        .expect("completed live-output search test completes");
}

#[test]
fn bell_events_include_focused_for_current_panes() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(crate::AppRoot::default());
            let epoch = backend.state().runtime_epoch;
            let pane_id = backend.state().current().focused_pane.unwrap();
            let generation = 42;
            {
                let pane =
                    crate::pane_lifecycle::find_pane_mut(backend.state_mut(), pane_id).unwrap();
                pane.pty_generation = generation;
                pane.terminal.bind_session(pane_id, generation);
            }
            let events = backend
                .state()
                .event_hub
                .subscribe(Some(HashSet::from([crate::events::EventKind::Bell])));
            backend
                .update_level(Msg::SessionOutput {
                    epoch,
                    pane_id,
                    local: false,
                    generation,
                    bytes: vec![7],
                })
                .unwrap();
            assert!(events.recv().unwrap().contains("\"focused\":\"true\""));
            backend.state_mut().current_mut().focused_pane = None;
            backend
                .update_level(Msg::SessionOutput {
                    epoch,
                    pane_id,
                    local: false,
                    generation,
                    bytes: vec![7],
                })
                .unwrap();
            assert!(events.recv().unwrap().contains("\"focused\":\"false\""));
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn focused_pane_bell_raises_background_attention_and_focus_gain_clears_it() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(crate::AppRoot::default());
            let epoch = backend.state().runtime_epoch;
            let pane_id = backend
                .state()
                .current()
                .focused_pane
                .expect("fresh pane focus");
            let generation = 42;
            {
                let pane =
                    crate::pane_lifecycle::find_pane_mut(backend.state_mut(), pane_id).unwrap();
                pane.pty_generation = generation;
                pane.terminal.bind_session(pane_id, generation);
            }
            let events = backend
                .state()
                .event_hub
                .subscribe(Some(HashSet::from([crate::events::EventKind::Bell])));

            backend
                .set_window_focused(false)
                .expect("lose host-window focus");
            backend
                .dispatch(Msg::SessionOutput {
                    epoch,
                    pane_id,
                    local: false,
                    generation,
                    bytes: vec![7],
                })
                .expect("deliver BEL while unfocused");
            assert!(events.recv().unwrap().contains("\"focused\":\"true\""));
            let pane = crate::pane_lifecycle::find_pane(backend.state(), pane_id).unwrap();
            assert!(pane.activity.has_unseen_output);
            assert!(pane.activity.bell);

            backend
                .set_window_focused(true)
                .expect("gain host-window focus");
            let pane = crate::pane_lifecycle::find_pane(backend.state(), pane_id).unwrap();
            assert!(!pane.activity.has_unseen_output);
            assert!(!pane.activity.bell);
        })
        .expect("spawn bell focus lifecycle test")
        .join()
        .expect("bell focus lifecycle test completes");
}

#[test]
fn finished_unseen_survives_background_updates_until_focus_gain() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(crate::AppRoot::default());
            let epoch = backend.state().runtime_epoch;
            let pane_id = backend
                .state()
                .current()
                .focused_pane
                .expect("fresh pane focus");
            let generation = 42;
            {
                let pane =
                    crate::pane_lifecycle::find_pane_mut(backend.state_mut(), pane_id).unwrap();
                pane.pty_generation = generation;
                pane.terminal.bind_session(pane_id, generation);
                pane.terminal.reported_status = Some(crate::session::protocol::PaneStatus {
                    value: crate::session::protocol::pane_status::WORKING.into(),
                    reason: None,
                    set_at: 1,
                });
                pane.terminal.detected_agent = Some(crate::session::protocol::DetectedAgent {
                    kind: crate::session::protocol::AgentKind::Claude,
                    state: crate::session::protocol::DetectedAgentState::Idle,
                });
            }

            backend
                .set_window_focused(false)
                .expect("lose host-window focus");
            backend
                .dispatch(Msg::SessionPaneRuntimeChanged {
                    epoch,
                    pane_id,
                    local: false,
                    generation,
                    state: PaneRuntimeState {
                        sequence: 1,
                        status: Some(crate::session::protocol::PaneStatus {
                            value: crate::session::protocol::pane_status::IDLE.into(),
                            reason: None,
                            set_at: 2,
                        }),
                        detected_agent: Some(crate::session::protocol::DetectedAgent {
                            kind: crate::session::protocol::AgentKind::Claude,
                            state: crate::session::protocol::DetectedAgentState::Idle,
                        }),
                        ..PaneRuntimeState::default()
                    },
                })
                .expect("deliver finished edge while unfocused");
            backend
                .dispatch(Msg::WorkbarTick)
                .expect("ordinary post-update pass");
            assert!(
                crate::pane_lifecycle::find_pane(backend.state(), pane_id)
                    .expect("finished pane")
                    .terminal
                    .finished_unseen
            );

            backend
                .set_window_focused(true)
                .expect("gain host-window focus");
            assert!(
                !crate::pane_lifecycle::find_pane(backend.state(), pane_id)
                    .expect("finished pane")
                    .terminal
                    .finished_unseen
            );
        })
        .expect("spawn finished focus lifecycle test")
        .join()
        .expect("finished focus lifecycle test completes");
}

#[test]
fn parked_disconnect_preserves_identity_and_marks_attachment_offline() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(crate::AppRoot::default());
            let (client, _outbound) = SessionClient::test_channel();
            let target = crate::session::remote::RemoteTarget::Alias("workbox".to_string());
            {
                let state = backend.state_mut();
                state.runtime_epoch = 4;
                state.current_mut().epoch = 4;
                state.current_mut().session_name = Some("dev".to_string());
                state.current_mut().session_client = Some(client);
                state.current_mut().session_attached = true;
                state.current_mut().pending_session_attach = None;
                state.current_mut().connection = crate::state::ConnectionState::Connected;
                state.current_mut().remote_host = Some("workbox".to_string());
                state.current_mut().remote_target = Some(target.clone());
                state.park_current(4, crate::state::Attachment::new());
                state.runtime_epoch = 5;
            }

            assert_eq!(backend.state().runtime_epoch, 5);
            let before = backend
                .state()
                .background
                .get(&4)
                .expect("parked before drop");
            assert_eq!(before.session_name.as_deref(), Some("dev"));
            assert!(before.pending_session_attach.is_none());

            backend
                .dispatch(Msg::SessionDisconnected {
                    epoch: 4,
                    name: "dev".to_string(),
                })
                .expect("dispatch parked disconnect");

            let parked = backend
                .state()
                .background
                .get(&4)
                .expect("retained session");
            assert_eq!(
                parked.connection,
                crate::state::ConnectionState::Disconnected
            );
            assert!(!parked.session_attached);
            assert!(parked.session_client.is_none());
            assert_eq!(parked.remote_target.as_ref(), Some(&target));
            assert_eq!(parked.session_name.as_deref(), Some("dev"));
        })
        .expect("spawn parked-disconnect test")
        .join()
        .expect("parked-disconnect test completes");
}

#[test]
fn parked_rename_updates_retained_identity() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(crate::AppRoot::default());
            {
                let state = backend.state_mut();
                state.runtime_epoch = 8;
                state.current_mut().session_name = Some("before".to_string());
                state.park_current(8, crate::state::Attachment::new());
                state.runtime_epoch = 9;
            }

            backend
                .dispatch(Msg::SessionRenamed {
                    epoch: 8,
                    session: "after".to_string(),
                })
                .expect("dispatch parked rename");

            assert_eq!(
                backend
                    .state()
                    .background
                    .get(&8)
                    .and_then(|attachment| attachment.session_name.as_deref()),
                Some("after")
            );
        })
        .expect("spawn parked-rename test")
        .join()
        .expect("parked-rename test completes");
}

#[test]
fn finished_unseen_arms_on_working_to_quiescent_and_disarms_on_resume() {
    // working -> idle arms the pulse.
    let mut pane = agent_pane("idle");
    update_agent_status_edge(&mut pane, Some("working"), None, false);
    assert!(pane.finished_unseen);

    // A later idle -> idle poll leaves it armed until the pane is looked at.
    update_agent_status_edge(&mut pane, Some("idle"), None, false);
    assert!(pane.finished_unseen);

    // Resuming work disarms it: a spinning agent must not wear a completed dot.
    pane.reported_status = Some(crate::session::protocol::PaneStatus {
        value: "working".into(),
        reason: None,
        set_at: 2,
    });
    update_agent_status_edge(&mut pane, Some("idle"), None, false);
    assert!(!pane.finished_unseen);
}

/// The duration column is only honest if the stamp moves on a real state change and holds
/// still across the repeated polls that report the same state.
#[test]
fn status_since_stamps_transitions_and_survives_unchanged_polls() {
    let mut pane = agent_pane("working");
    assert!(pane.status_since.is_none());
    update_agent_status_edge(&mut pane, Some("idle"), None, false);
    let stamped = pane.status_since.expect("a transition stamps the pane");

    update_agent_status_edge(&mut pane, Some("working"), None, false);
    assert_eq!(pane.status_since, Some(stamped));

    pane.reported_status = Some(crate::session::protocol::PaneStatus {
        value: "idle".into(),
        reason: None,
        set_at: 2,
    });
    update_agent_status_edge(&mut pane, Some("working"), None, false);
    assert!(pane.status_since.expect("re-stamped") > stamped);
}

/// A finished run reports what it cost. The number is banked as the run ends and never moves
/// again, so it cannot drift into meaning "how long ago it stopped".
#[test]
fn finishing_a_run_banks_its_length_and_freezes_it() {
    let run = std::time::Duration::from_secs(12 * 60);
    let mut pane = agent_pane("idle");
    update_agent_status_edge(&mut pane, Some("working"), Some(run), false);
    assert_eq!(pane.last_run, Some(run));

    // Repeated idle polls afterwards leave the banked run alone.
    update_agent_status_edge(
        &mut pane,
        Some("idle"),
        Some(std::time::Duration::from_secs(1)),
        false,
    );
    assert_eq!(pane.last_run, Some(run));

    // Only a `working` stretch is banked; leaving any other state does not overwrite it.
    pane.reported_status = Some(crate::session::protocol::PaneStatus {
        value: "working".into(),
        reason: None,
        set_at: 2,
    });
    update_agent_status_edge(
        &mut pane,
        Some("blocked"),
        Some(std::time::Duration::from_secs(3)),
        false,
    );
    assert_eq!(pane.last_run, Some(run));
}

#[test]
fn finished_unseen_ignores_working_to_blocked() {
    let mut pane = agent_pane("blocked");
    update_agent_status_edge(&mut pane, Some("working"), None, false);
    assert!(!pane.finished_unseen);
}

#[test]
fn detected_blocked_over_stale_idle_does_not_arm_finished_unseen() {
    let mut pane = agent_pane("idle");
    pane.detected_agent.as_mut().expect("agent").state =
        crate::session::protocol::DetectedAgentState::Blocked;
    update_agent_status_edge(&mut pane, Some("working"), None, false);
    assert_eq!(pane.agent_status().as_deref(), Some("blocked"));
    assert!(!pane.finished_unseen);
}

#[test]
fn agent_edges_report_detected_only_blocked() {
    let mut pane = crate::pane::TerminalPane::new(100);
    pane.detected_agent = Some(crate::session::protocol::DetectedAgent {
        kind: crate::session::protocol::AgentKind::Claude,
        state: crate::session::protocol::DetectedAgentState::Blocked,
    });

    let edges = update_agent_status_edge(&mut pane, Some("idle"), None, false);

    assert!(edges.became_blocked);
    assert!(!edges.finished);
    assert!(pane.reported_status.is_none());
}

#[test]
fn agent_edges_report_reported_only_blocked_once() {
    let mut pane = crate::pane::TerminalPane::new(100);
    pane.reported_status = Some(crate::session::protocol::PaneStatus {
        value: crate::session::protocol::pane_status::BLOCKED.into(),
        reason: None,
        set_at: 1,
    });

    let first = update_agent_status_edge(&mut pane, None, None, false);
    let repeated = update_agent_status_edge(&mut pane, None, None, true);

    assert!(first.became_blocked);
    assert!(!repeated.became_blocked);
    assert!(pane.detected_agent.is_none());
}

#[test]
fn hold_on_exit_excludes_disabled_and_closing_panes() {
    assert!(should_hold_on_exit(true, false));
    assert!(!should_hold_on_exit(false, false));
    assert!(
        !should_hold_on_exit(true, true),
        "a pane the user closed must not hold, or its own close would keep it alive"
    );
}

#[test]
fn parked_non_controller_defers_exit_until_control_returns() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(crate::AppRoot::default());
            let mut attachment = crate::state::Attachment::new();
            attachment.epoch = 9;
            let mut pane =
                crate::state::Pane::new(4, 100, tui_lipan::prelude::FloatRect::default());
            pane.pty_generation = 7;
            attachment.workspaces[0].panes.push(pane);
            let mut shared = crate::state::SharedSessionState::new(1);
            shared.controller = Some(2);
            attachment.shared = Some(shared);
            backend.state_mut().background.insert(9, attachment);

            backend
                .dispatch(Msg::SessionExited {
                    epoch: 9,
                    pane_id: 4,
                    local: false,
                    generation: 7,
                    code: 0,
                })
                .expect("parked exit");

            assert_eq!(
                backend.state().background[&9].pending_background_closes,
                vec![(4, 7)]
            );
        })
        .expect("spawn test")
        .join()
        .expect("join test");
}

#[test]
fn roster_diff_emits_joins_and_leaves_with_the_new_count() {
    let client = |id, label: &str| ClientInfo {
        id,
        label: label.to_string(),
        read_only: false,
        requesting_control: false,
        parked: false,
    };
    let events = roster_diff_events(
        &[client(1, "one"), client(2, "two")],
        &[client(2, "renamed"), client(3, "three")],
    );

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, crate::events::EventKind::ClientJoined);
    assert_eq!(
        events[0].fields,
        vec![
            ("client_id", "3".into()),
            ("client_name", "three".into()),
            ("count", "2".into()),
        ]
    );
    assert_eq!(events[1].kind, crate::events::EventKind::ClientLeft);
    assert_eq!(
        events[1].fields,
        vec![
            ("client_id", "1".into()),
            ("client_name", "one".into()),
            ("count", "2".into()),
        ]
    );
}

#[test]
fn runtime_status_transitions_emit_once_and_stale_updates_are_ignored() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(crate::AppRoot::default());
            let epoch = backend.state().runtime_epoch;
            let pane = &mut backend.state_mut().current_mut().workspaces[0].panes[0];
            pane.pty_generation = 7;
            pane.terminal.bind_session(pane.id, 7);
            let events = backend.state().event_hub.subscribe(Some(HashSet::from([
                crate::events::EventKind::PaneStatusChanged,
            ])));
            let status = crate::session::protocol::PaneStatus {
                value: "blocked".into(),
                reason: Some("needs approval".into()),
                set_at: 1,
            };
            let runtime = PaneRuntimeState {
                status: Some(status.clone()),
                detected_agent: Some(crate::session::protocol::DetectedAgent {
                    kind: crate::session::protocol::AgentKind::OpenCode,
                    state: crate::session::protocol::DetectedAgentState::Blocked,
                }),
                sequence: 1,
                ..PaneRuntimeState::default()
            };

            backend
                .dispatch(Msg::SessionPaneRuntimeChanged {
                    epoch,
                    pane_id: 1,
                    local: false,
                    generation: 7,
                    state: runtime.clone(),
                })
                .expect("dispatch status transition");
            let event: serde_json::Value =
                serde_json::from_str(&events.try_recv().expect("transition event")).unwrap();
            assert_eq!(event["event"], "pane-status-changed");
            assert_eq!(event["data"]["pane"], "1");
            assert_eq!(event["data"]["status"], "blocked");
            assert_eq!(event["data"]["reason"], "needs approval");
            assert_eq!(event["data"]["previous_status"], "");
            assert_eq!(
                backend.state().current().workspaces[0].panes[0]
                    .terminal
                    .detected_agent
                    .as_ref()
                    .map(|agent| agent.kind),
                Some(crate::session::protocol::AgentKind::OpenCode)
            );

            backend
                .dispatch(Msg::SessionPaneRuntimeChanged {
                    epoch,
                    pane_id: 1,
                    local: false,
                    generation: 7,
                    state: runtime,
                })
                .expect("dispatch duplicate status");
            assert!(events.try_recv().is_err());

            backend
                .dispatch(Msg::SessionPaneRuntimeChanged {
                    epoch,
                    pane_id: 1,
                    local: false,
                    generation: 7,
                    state: PaneRuntimeState {
                        status: None,
                        sequence: 0,
                        ..PaneRuntimeState::default()
                    },
                })
                .expect("dispatch stale status");
            assert_eq!(
                backend.state().current().workspaces[0].panes[0]
                    .terminal
                    .reported_status,
                Some(status)
            );
            assert!(
                backend.state().current().workspaces[0].panes[0]
                    .terminal
                    .detected_agent
                    .is_some()
            );
            assert!(events.try_recv().is_err());
        })
        .expect("spawn runtime status test thread")
        .join()
        .expect("runtime status test thread completes");
}

#[test]
fn parked_runtime_updates_keep_background_metadata_current() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(crate::AppRoot::default());
            {
                let state = backend.state_mut();
                state.runtime_epoch = 4;
                let pane = &mut state.current_mut().workspaces[0].panes[0];
                pane.pty_generation = 7;
                pane.terminal.bind_session(pane.id, 7);
                state.park_current(4, crate::state::Attachment::new());
                state.runtime_epoch = 5;
            }

            backend
                .dispatch(Msg::SessionPaneRuntimeChanged {
                    epoch: 4,
                    pane_id: 1,
                    local: false,
                    generation: 7,
                    state: PaneRuntimeState {
                        cwd: Some("/remote/project".to_string()),
                        foreground_program: Some("cargo".to_string()),
                        sequence: 1,
                        ..PaneRuntimeState::default()
                    },
                })
                .expect("dispatch parked runtime update");

            let pane = &backend.state().background[&4].workspaces[0].panes[0];
            assert_eq!(pane.terminal.cwd.as_deref(), Some("/remote/project"));
            assert_eq!(pane.terminal.foreground_program.as_deref(), Some("cargo"));
        })
        .expect("spawn parked runtime test")
        .join()
        .expect("parked runtime test completes");
}

/// A failed *local* attach must not install another pending attach: a local ephemeral is
/// itself the fallback, so retrying it on failure would spin forever (fail → fall back → fail →
/// …). Only a remote failure falls back, which is verified live. Side-effect-free: the local
/// path returns no command, so nothing is spawned.
#[test]
fn local_attach_failure_does_not_retry_into_an_ephemeral_loop() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(crate::AppRoot::default());
            backend.state_mut().current_mut().pending_session_attach =
                Some(crate::state::PendingSessionAttach {
                    epoch: 42,
                    name: "eph-local".into(),
                    client: None,
                    autostart: true,
                    read_only: false,
                    reconnect: false,
                    remote_host: None,
                    intent: crate::state::AttachIntent::Plain,
                    left: None,
                    parked_epoch: None,
                });
            backend
                .dispatch(Msg::SessionAttachFailed {
                    epoch: 42,
                    message: "no local server".into(),
                })
                .expect("dispatch local attach failure");
            assert!(
                backend.state().current().pending_session_attach.is_none(),
                "a local ephemeral failure must clear the pending attach and not re-arm one"
            );
        })
        .expect("spawn no-loop test thread")
        .join()
        .expect("no-loop test thread completes");
}

#[test]
fn retained_remote_reconnect_failure_stays_offline_and_remote() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(crate::AppRoot::default());
            let target = crate::session::remote::RemoteTarget::Alias("workbox".to_string());
            {
                let state = backend.state_mut();
                state.current_mut().session_name = Some("dev".to_string());
                state.current_mut().remote_host = Some("workbox".to_string());
                state.current_mut().remote_target = Some(target.clone());
                state.current_mut().pending_session_attach =
                    Some(crate::state::PendingSessionAttach {
                        epoch: 42,
                        name: "dev".to_string(),
                        client: None,
                        autostart: false,
                        read_only: false,
                        reconnect: true,
                        remote_host: Some("workbox".to_string()),
                        intent: crate::state::AttachIntent::Plain,
                        left: None,
                        parked_epoch: None,
                    });
            }

            backend
                .dispatch(Msg::SessionAttachFailed {
                    epoch: 42,
                    message: "offline".to_string(),
                })
                .expect("dispatch reconnect failure");

            assert!(backend.state().current().pending_session_attach.is_none());
            assert_eq!(
                backend.state().current().connection,
                crate::state::ConnectionState::Unreachable
            );
            assert_eq!(
                backend.state().current().remote_target.as_ref(),
                Some(&target)
            );
            assert_eq!(
                backend.state().current().session_name.as_deref(),
                Some("dev")
            );
        })
        .expect("spawn retained reconnect test")
        .join()
        .expect("retained reconnect test completes");
}

/// A failed *remote* connect that had parked a live session restores that session rather than
/// falling back to a fresh local ephemeral. The ephemeral fallback would re-attach to this
/// process's own `eph-<pid>` server — still controlled by the parked client — and come back as a
/// follower of itself. Restoring the parked attachment keeps the user on their real session, and
/// the dead empty connect attachment is discarded.
#[test]
fn failed_remote_connect_restores_the_parked_session_not_a_follower_ephemeral() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(crate::AppRoot::default());
            let target = crate::session::remote::RemoteTarget::Alias("windev".to_string());
            {
                let state = backend.state_mut();
                // The live local session, parked into the background under epoch 1 when the
                // connect started.
                let mut parked = crate::state::Attachment::new();
                parked.epoch = 1;
                parked.session_name = Some("eph-local".to_string());
                parked.session_attached = true;
                parked.connection = crate::state::ConnectionState::Connected;
                state.background.insert(1, parked);

                // The current attachment is the fresh empty one the connect installed; it never
                // attached.
                state.runtime_epoch = 2;
                state.current_mut().epoch = 2;
                state.current_mut().remote_host = Some("windev".to_string());
                state.current_mut().remote_target = Some(target.clone());
                state.current_mut().pending_session_attach =
                    Some(crate::state::PendingSessionAttach {
                        epoch: 2,
                        name: "eph-windev".to_string(),
                        client: None,
                        autostart: true,
                        read_only: false,
                        reconnect: false,
                        remote_host: Some("windev".to_string()),
                        intent: crate::state::AttachIntent::Plain,
                        left: None,
                        parked_epoch: Some(1),
                    });
            }

            backend
                .dispatch(Msg::SessionAttachFailed {
                    epoch: 2,
                    message: "could not resolve hostname windev".to_string(),
                })
                .expect("dispatch remote connect failure");

            // Restored onto the parked local session, not a fresh ephemeral, and no longer
            // pointed at the remote host.
            assert_eq!(
                backend.state().current().session_name.as_deref(),
                Some("eph-local")
            );
            assert!(backend.state().current().session_attached);
            assert_eq!(backend.state().current().remote_target, None);
            assert!(backend.state().current().pending_session_attach.is_none());
            // The parked entry is gone (now current) and the dead connect attachment was not
            // retained in its place.
            assert!(!backend.state().background.contains_key(&1));
            assert!(!backend.state().background.contains_key(&2));
        })
        .expect("spawn failed-connect-restore test")
        .join()
        .expect("failed-connect-restore test completes");
}

/// A failed *local* create also restores the parked session — creating now parks the current
/// session like a switch, so a create that can't start its server must not strand the user on
/// the broken empty attachment either. (The remote-only ephemeral fallback stays remote-only.)
#[test]
fn failed_local_create_restores_the_parked_session() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(crate::AppRoot::default());
            {
                let state = backend.state_mut();
                let mut parked = crate::state::Attachment::new();
                parked.epoch = 1;
                parked.session_name = Some("eph-local".to_string());
                parked.session_attached = true;
                parked.connection = crate::state::ConnectionState::Connected;
                state.background.insert(1, parked);

                state.runtime_epoch = 2;
                state.current_mut().epoch = 2;
                state.current_mut().pending_session_attach =
                    Some(crate::state::PendingSessionAttach {
                        epoch: 2,
                        name: "work".to_string(),
                        client: None,
                        autostart: true,
                        read_only: false,
                        reconnect: false,
                        // Local create: no remote host.
                        remote_host: None,
                        intent: crate::state::AttachIntent::Plain,
                        left: None,
                        parked_epoch: Some(1),
                    });
            }

            backend
                .dispatch(Msg::SessionAttachFailed {
                    epoch: 2,
                    message: "could not start session server".to_string(),
                })
                .expect("dispatch local create failure");

            assert_eq!(
                backend.state().current().session_name.as_deref(),
                Some("eph-local")
            );
            assert!(backend.state().current().session_attached);
            assert!(backend.state().current().pending_session_attach.is_none());
            assert!(!backend.state().background.contains_key(&1));
            assert!(!backend.state().background.contains_key(&2));
        })
        .expect("spawn failed-local-create test")
        .join()
        .expect("failed-local-create test completes");
}

/// Attaching where another client already holds the lease: the client must ask what to do
/// instead of quietly becoming a follower. `controller` is who the server says holds it.
fn attach_with_controller(controller: crate::shared_layout::ClientId) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            let mut backend = TestBackend::new(crate::AppRoot::default());
            let (client, _rx) = SessionClient::test_channel();
            backend.state_mut().current_mut().pending_session_attach =
                Some(crate::state::PendingSessionAttach {
                    epoch: 1,
                    name: "dev".into(),
                    client: Some(client),
                    autostart: false,
                    read_only: false,
                    reconnect: false,
                    remote_host: None,
                    intent: crate::state::AttachIntent::Plain,
                    left: None,
                    parked_epoch: None,
                });
            backend
                .dispatch(Msg::SessionAttached {
                    epoch: 1,
                    session: "dev".into(),
                    client_id: 2,
                    panes: Vec::new(),
                    layout_rev: 0,
                    layout: None,
                    controller: Some(controller),
                    clients: Vec::new(),
                    input_locked: false,
                    allow_takeover: false,
                    read_only: false,
                    created_from_profile: None,
                })
                .expect("dispatch attach");
            tx.send(backend.state().follow_prompt.is_some())
                .expect("report result");
        })
        .expect("spawn follow-prompt test")
        .join()
        .expect("follow-prompt test completes");
    rx.recv().expect("test result")
}

#[test]
fn attaching_to_an_occupied_session_asks_before_following() {
    assert!(attach_with_controller(1));
}

/// Getting the lease on attach — which is what happens when the only other client is parked —
/// is not a fork in the road, so nothing is asked.
#[test]
fn attaching_with_the_lease_in_hand_asks_nothing() {
    assert!(!attach_with_controller(2));
}

#[test]
fn empty_ephemeral_profile_seed_emits_profile_loaded_after_attach() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(crate::AppRoot::default());
            let (client, rx) = SessionClient::test_channel();
            let path = PathBuf::from("legacy-profile.toml");
            backend.state_mut().current_mut().pending_session_attach =
                Some(crate::state::PendingSessionAttach {
                    epoch: 1,
                    name: "eph-test".into(),
                    client: Some(client),
                    autostart: true,
                    read_only: false,
                    reconnect: false,
                    remote_host: None,
                    intent: crate::state::AttachIntent::ProfileSeed {
                        profile: "legacy-profile".into(),
                        path: path.clone(),
                    },
                    left: None,
                    parked_epoch: None,
                });
            backend.state_mut().show_profile_picker = true;
            backend.state_mut().profile_picker =
                Some(crate::state::ProfilePickerState::new(Vec::new()));
            let events = backend.state().event_hub.subscribe(Some(HashSet::from([
                crate::events::EventKind::ProfileLoaded,
            ])));

            backend
                .dispatch(Msg::SessionAttached {
                    epoch: 1,
                    session: "eph-test".into(),
                    client_id: 1,
                    panes: Vec::new(),
                    layout_rev: 0,
                    layout: None,
                    controller: Some(1),
                    clients: Vec::new(),
                    input_locked: false,
                    allow_takeover: false,
                    read_only: false,
                    created_from_profile: None,
                })
                .expect("dispatch attach");

            assert_eq!(backend.state().current().created_from_profile, None);
            assert!(!backend.state().show_profile_picker);
            assert!(backend.state().profile_picker.is_none());
            assert!(rx.try_iter().any(|message| matches!(
                message,
                crate::session::client::ClientOutbound::Control(
                    crate::session::protocol::ClientMessage::SetSessionOrigin { profile }
                ) if profile == "legacy-profile"
            )));
            assert!(events.try_recv().is_err());
            backend
                .dispatch(Msg::SessionOriginSet {
                    epoch: 1,
                    created_from_profile: "legacy-profile".to_string(),
                })
                .expect("acknowledge session origin");
            assert_eq!(
                backend.state().current().created_from_profile.as_deref(),
                Some("legacy-profile")
            );

            let event: serde_json::Value =
                serde_json::from_str(&events.try_recv().expect("profile-loaded event"))
                    .expect("event json");
            assert_eq!(
                event,
                serde_json::json!({
                    "event": "profile-loaded",
                    "data": {
                        "profile": "legacy-profile",
                        "path": path.display().to_string(),
                        "session": "eph-test"
                    }
                })
            );
        })
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
}

fn colliding_namespace_backend() -> TestBackend<crate::AppRoot> {
    let mut backend = TestBackend::new(crate::AppRoot::default());
    let rect = tui_lipan::prelude::FloatRect::default();
    let mut shared = crate::state::Pane::new(7, 100, rect);
    shared.pty_generation = 1;
    shared.terminal.bind_session(7, 1);
    backend.state_mut().current_mut().workspaces[0]
        .panes
        .push(shared);
    let mut local = crate::state::Pane::new(7, 100, rect);
    local.pty_generation = 1;
    local.terminal.bind_session(7, 1);
    backend.state_mut().scratch.panes.push(local);
    backend.state_mut().scratch.focused_pane = Some(7);
    backend.state_mut().scratch_visible = true;
    backend
}

fn namespace_text(backend: &TestBackend<crate::AppRoot>, local: bool) -> String {
    crate::pane_lifecycle::find_pane_in_namespace(backend.state(), 7, local)
        .expect("namespaced pane")
        .terminal
        .capture_text()
}

#[test]
fn colliding_pane_ids_route_session_events_by_namespace() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = colliding_namespace_backend();
            let epoch = backend.state().runtime_epoch;

            backend
                .dispatch(Msg::SessionOutput {
                    epoch,
                    pane_id: 7,
                    local: false,
                    generation: 1,
                    bytes: b"shared-out\r\n".to_vec(),
                })
                .expect("shared output");
            backend
                .dispatch(Msg::SessionOutput {
                    epoch,
                    pane_id: 7,
                    local: true,
                    generation: 1,
                    bytes: b"local-out\r\n".to_vec(),
                })
                .expect("local output");
            assert!(
                namespace_text(&backend, false).contains("shared-out"),
                "shared output must land on the attachment pane"
            );
            assert!(
                !namespace_text(&backend, false).contains("local-out"),
                "local output must not land on the attachment pane"
            );
            assert!(
                namespace_text(&backend, true).contains("local-out"),
                "local output must land on the scratch pane"
            );
            assert!(
                !namespace_text(&backend, true).contains("shared-out"),
                "shared output must not land on the scratch pane"
            );

            backend
                .dispatch(Msg::SessionResized {
                    epoch,
                    pane_id: 7,
                    local: false,
                    generation: 1,
                    cols: 40,
                    rows: 12,
                })
                .expect("shared resize");
            backend
                .dispatch(Msg::SessionResized {
                    epoch,
                    pane_id: 7,
                    local: true,
                    generation: 1,
                    cols: 20,
                    rows: 8,
                })
                .expect("local resize");
            assert_eq!(
                crate::pane_lifecycle::find_pane_in_namespace(backend.state(), 7, false)
                    .unwrap()
                    .terminal
                    .cols,
                40
            );
            assert_eq!(
                crate::pane_lifecycle::find_pane_in_namespace(backend.state(), 7, true)
                    .unwrap()
                    .terminal
                    .rows,
                8
            );

            backend.state_mut().config.pane.hold_on_exit = false;
            backend
                .dispatch(Msg::SessionExited {
                    epoch,
                    pane_id: 7,
                    local: false,
                    generation: 1,
                    code: 0,
                })
                .expect("shared exit");
            assert!(
                crate::pane_lifecycle::find_pane_in_namespace(backend.state(), 7, false)
                    .is_some_and(|pane| pane.closing),
                "shared exit must close the attachment pane"
            );
            assert!(
                crate::pane_lifecycle::find_pane_in_namespace(backend.state(), 7, true)
                    .is_some_and(|pane| !pane.closing),
                "shared exit must not close the scratch pane"
            );

            backend
                .dispatch(Msg::SessionExited {
                    epoch,
                    pane_id: 7,
                    local: true,
                    generation: 1,
                    code: 0,
                })
                .expect("local exit");
            assert!(
                crate::pane_lifecycle::find_pane_in_namespace(backend.state(), 7, true)
                    .is_some_and(|pane| pane.closing),
                "local exit must close the scratch pane"
            );
        })
        .expect("spawn colliding namespace test")
        .join()
        .expect("colliding namespace test completes");
}

/// A popup lives outside every workspace, so the generic teardown cannot find it: `local` with
/// no scratch membership falls straight through and marks nothing. Without an explicit
/// interception a failed popup spawn left a dead pane on screen for the rest of the session.
#[test]
fn a_failed_popup_spawn_tears_the_popup_down() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(crate::AppRoot::default());
            let generation = {
                let state = backend.state_mut();
                let mut pane = crate::state::Pane::new(
                    crate::state::POPUP_PANE_ID,
                    100,
                    tui_lipan::prelude::FloatRect::default(),
                );
                pane.opening = false;
                // A `keep_open` popup must still go: there is no process to keep open.
                pane.identity.keep_open = true;
                let generation = pane.pty_generation;
                state.popup = Some(pane);
                generation
            };
            backend.render();

            let epoch = backend.state().runtime_epoch;
            backend
                .dispatch(Msg::SessionSpawnResult {
                    epoch,
                    pane_id: crate::state::POPUP_PANE_ID,
                    local: true,
                    generation,
                    pid: None,
                    ok: false,
                    error: Some("no such command".to_string()),
                })
                .expect("failed popup spawn");

            assert!(
                backend
                    .state()
                    .popup
                    .as_ref()
                    .is_some_and(|pane| pane.closing),
                "the popup must be marked closing so its prune can drop it"
            );

            backend
                .dispatch(Msg::PruneClosed(
                    epoch,
                    crate::state::POPUP_PANE_ID,
                    generation,
                ))
                .expect("prune the closed popup");
            assert!(backend.state().popup.is_none(), "the popup must be dropped");
        })
        .expect("spawn popup teardown test thread")
        .join()
        .expect("popup teardown test thread panicked");
}
