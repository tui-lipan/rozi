//! Restart rozi through the production update path: the plan the dialog shows, the claims gathered
//! before letting go, the handoff written for the new build, and the restore that build runs.

use std::sync::mpsc::Receiver;

use tui_lipan::TestBackend;

use super::*;
use crate::session::client::{ClientOutbound, SessionClient};
use crate::session::protocol::{ClientMessage, PROTOCOL_VERSION};
use crate::state::{Attachment, ConnectionState, SharedSessionState};

/// Make `attachment` a live connection to `name`, returning what its client sends.
fn connect(
    attachment: &mut Attachment,
    name: &str,
    remote: Option<&str>,
) -> Receiver<ClientOutbound> {
    let (client, sent) = SessionClient::test_channel();
    attachment.session_name = Some(name.to_string());
    attachment.remote_target =
        remote.map(|host| crate::session::remote::RemoteTarget::Alias(host.into()));
    attachment.session_client = Some(client);
    attachment.session_attached = true;
    attachment.connection = ConnectionState::Connected;
    // A test app starts with its startup attach pending; this attachment has already landed.
    attachment.pending_session_attach = None;
    attachment.shared = Some(SharedSessionState::new(1));
    sent
}

fn background(state: &mut State, epoch: AttachmentId, name: &str, remote: Option<&str>) {
    let mut attachment = Attachment::new();
    connect(&mut attachment, name, remote);
    attachment.epoch = epoch;
    attachment.parked_seq = epoch;
    state.background.insert(epoch, attachment);
}

fn sent_messages(sent: &Receiver<ClientOutbound>) -> Vec<ClientMessage> {
    sent.try_iter()
        .filter_map(|outbound| match outbound {
            ClientOutbound::Control(message) => Some(message),
            _ => None,
        })
        .collect()
}

fn open_ready(backend: &mut TestBackend<AppRoot>, protocol: std::ops::RangeInclusive<u32>) {
    backend.state_mut().relaunch.generation = 1;
    backend.state_mut().relaunch.dialog = Some(RelaunchDialog {
        generation: 1,
        expected: None,
        note: None,
        stage: RelaunchStage::Checking,
    });
    backend
        .dispatch(Msg::RelaunchCandidate {
            generation: 1,
            result: Ok(crate::relaunch::candidate::Candidate::for_test(
                "9.9.9", protocol,
            )),
        })
        .expect("the candidate arrives");
}

fn ready_plan(backend: &TestBackend<AppRoot>) -> &RelaunchPlan {
    match backend
        .state()
        .relaunch
        .dialog
        .as_ref()
        .map(|dialog| &dialog.stage)
    {
        Some(RelaunchStage::Ready(plan)) => plan,
        _ => panic!("the dialog has a plan"),
    }
}

fn transitions(plan: &RelaunchPlan) -> Vec<(String, Transition)> {
    plan.sessions
        .iter()
        .map(|session| (session.facts.label(), session.transition.clone()))
        .collect()
}

#[test]
fn every_session_the_new_build_speaks_is_carried_over_and_only_the_rest_upgraded() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        {
            let state = backend.state_mut();
            let _ = connect(state.current_mut(), "dev", None);
            background(state, 7, "api", Some("workbox"));
            background(state, 8, "eph-0123456789abcdef01234567", None);
            background(state, 9, "far", None);
            let far = state.background.get_mut(&9).unwrap();
            far.connection = ConnectionState::Unreachable;
        }

        open_ready(&mut backend, PROTOCOL_VERSION..=PROTOCOL_VERSION + 1);
        assert_eq!(
            transitions(ready_plan(&backend)),
            vec![
                ("dev".to_string(), Transition::Carry { claim: true }),
                ("api@workbox".to_string(), Transition::Carry { claim: true }),
                (
                    "eph-0123456789abcdef01234567".to_string(),
                    Transition::Carry { claim: true }
                ),
                ("far".to_string(), Transition::Reconnect),
            ],
            "the session on screen first, then the background by name"
        );
        assert!(!ready_plan(&backend).destructive());

        backend.dispatch(Msg::RelaunchCancel).unwrap();
        open_ready(&mut backend, PROTOCOL_VERSION + 1..=PROTOCOL_VERSION + 1);
        let plan = ready_plan(&backend);
        assert_eq!(plan.sessions[0].transition, Transition::Upgrade);
        assert_eq!(plan.sessions[1].transition, Transition::Upgrade);
        assert!(matches!(
            plan.sessions[2].transition,
            Transition::Blocked { .. }
        ));
        assert!(plan.blocked() && plan.destructive());
    });
}

#[test]
fn restarting_reserves_each_slot_writes_the_handoff_and_lets_go_without_closing_anything() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        let (on_screen, parked) = {
            let state = backend.state_mut();
            let on_screen = connect(state.current_mut(), "dev", None);
            state.current_mut().active_workspace = 1;
            state.current_mut().engaged = true;
            let mut attachment = Attachment::new();
            let parked = connect(&mut attachment, "eph-0123456789abcdef01234567", None);
            attachment.auto_created = true;
            attachment.parked_seq = 3;
            state.background.insert(42, attachment);
            (on_screen, parked)
        };
        let epoch = backend.state().runtime_epoch;
        open_ready(&mut backend, PROTOCOL_VERSION..=PROTOCOL_VERSION);

        backend.dispatch(Msg::RelaunchConfirm).unwrap();
        let requests: Vec<(AttachmentId, u64)> = [(epoch, &on_screen), (42, &parked)]
            .into_iter()
            .map(|(epoch, sent)| {
                let messages = sent_messages(sent);
                match messages.as_slice() {
                    [ClientMessage::PrepareHandoff { request_id }] => (epoch, *request_id),
                    other => panic!("expected one reservation request, got {other:?}"),
                }
            })
            .collect();
        assert!(backend.state().relaunch.handover.take().is_none());

        for (epoch, request_id) in requests {
            backend
                .dispatch(Msg::SessionHandoffPrepared {
                    epoch,
                    request_id,
                    token: format!("token-{epoch}"),
                    error: None,
                })
                .unwrap();
        }

        let handover = backend
            .state()
            .relaunch
            .handover
            .take()
            .expect("the runner is handed the new build");
        assert!(backend.state().relaunch.dialog.is_none());
        for sent in [&on_screen, &parked] {
            assert_eq!(
                sent_messages(sent),
                vec![ClientMessage::Detach],
                "every session is detached, none shut down"
            );
        }
        let handoff = crate::relaunch::document::take(&handover.handoff_id).unwrap();
        assert_eq!(handoff.foreground, Some(0));
        let dev = &handoff.sessions[0];
        assert_eq!(dev.name, "dev");
        assert_eq!(
            dev.token.as_deref(),
            Some(format!("token-{epoch}").as_str())
        );
        assert_eq!(dev.active_workspace, 1);
        assert!(dev.engaged);
        let temporary = &handoff.sessions[1];
        assert_eq!(temporary.token.as_deref(), Some("token-42"));
        assert!(
            temporary.auto_created,
            "an untouched temporary session comes back as one"
        );
        assert_eq!(temporary.parked_seq, 3);
    });
}

#[test]
fn a_restart_that_ends_programs_asks_twice_and_a_blocked_one_never_starts() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        let sent = connect(backend.state_mut().current_mut(), "dev", None);
        open_ready(&mut backend, PROTOCOL_VERSION + 1..=PROTOCOL_VERSION + 1);

        backend.dispatch(Msg::RelaunchConfirm).unwrap();
        assert!(ready_plan(&backend).armed);
        assert!(sent_messages(&sent).is_empty());
        backend.dispatch(Msg::RelaunchConfirm).unwrap();
        assert!(
            backend.state().relaunch.handover.take().is_some(),
            "an upgrade claims nothing, so the second press hands over at once"
        );

        let mut blocked = TestBackend::new(AppRoot::default());
        let sent = connect(
            blocked.state_mut().current_mut(),
            "eph-0123456789abcdef01234567",
            None,
        );
        open_ready(&mut blocked, PROTOCOL_VERSION + 1..=PROTOCOL_VERSION + 1);
        blocked.dispatch(Msg::RelaunchConfirm).unwrap();
        blocked.dispatch(Msg::RelaunchConfirm).unwrap();
        assert!(ready_plan(&blocked).blocked());
        assert!(blocked.state().relaunch.handover.take().is_none());
        assert!(sent_messages(&sent).is_empty());
    });
}

#[test]
fn a_server_that_never_answers_does_not_hold_the_restart_up() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        let _sent = connect(backend.state_mut().current_mut(), "dev", None);
        open_ready(&mut backend, PROTOCOL_VERSION..=PROTOCOL_VERSION);
        backend.dispatch(Msg::RelaunchConfirm).unwrap();
        assert!(backend.state().relaunch.handover.take().is_none());

        backend
            .dispatch(Msg::RelaunchPrepareTimeout { generation: 1 })
            .unwrap();
        let handover = backend
            .state()
            .relaunch
            .handover
            .take()
            .expect("handed over");
        let handoff = crate::relaunch::document::take(&handover.handoff_id).unwrap();
        assert_eq!(handoff.sessions[0].token, None, "rejoined without a claim");
    });
}

#[test]
fn the_dialog_names_each_session_and_what_becomes_of_it() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(tui_lipan::prelude::Rect {
            x: 0,
            y: 0,
            w: 120,
            h: 40,
        });
        {
            let state = backend.state_mut();
            let _ = connect(state.current_mut(), "dev", None);
            background(state, 7, "api", Some("workbox"));
        }
        // Drawn at rest: a modal opening with animations on is still transparent here.
        backend.state_mut().config.animations.enabled = false;
        open_ready(&mut backend, PROTOCOL_VERSION..=PROTOCOL_VERSION);
        backend.render();
        let screen = backend.capture_frame().plain_text();
        for expected in [
            "Restart rozi",
            "v9.9.9",
            "dev",
            "api@workbox",
            "Keeps running",
            "Restart",
        ] {
            assert!(
                screen.contains(expected),
                "missing {expected:?} in\n{screen}"
            );
        }
    });
}

#[test]
fn a_session_that_lands_gets_its_view_back_and_the_restore_finishes() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.render();
        let epoch = backend.state().runtime_epoch;
        let (client, _sent) = SessionClient::test_channel();
        let session = HandoffSession {
            name: "dev".to_string(),
            remote: None,
            instance: Some("dev".to_string()),
            transition: Transition::Carry { claim: true },
            token: Some("token".to_string()),
            read_only: false,
            auto_created: false,
            engaged: true,
            active_workspace: 0,
            focused_pane: Some(9),
            parked_seq: 0,
        };
        {
            let state = backend.state_mut();
            state.current_mut().pending_session_attach = Some(crate::state::PendingSessionAttach {
                epoch,
                request: crate::state::AttachRequest::new("dev", crate::state::AttachMode::Resume)
                    .with_handoff_token(Some("token".to_string())),
                client: Some(client),
                left: None,
                parked_epoch: None,
            });
            state.relaunch.restore = Some(Restore {
                queue: VecDeque::new(),
                total: 1,
                current: Some((epoch, session)),
                launcher: false,
                problems: Vec::new(),
            });
        }
        assert_eq!(
            covering_progress(backend.state()),
            None,
            "the last session is not covered"
        );

        backend
            .dispatch(Msg::SessionAttached {
                epoch,
                session_instance: crate::session::protocol::SessionInstanceId::for_test("dev"),
                session: "dev".to_string(),
                client_id: 1,
                panes: vec![
                    crate::session::protocol::PaneMeta::for_test(7, 0, 100),
                    crate::session::protocol::PaneMeta::for_test(9, 0, 100),
                ],
                layout_rev: 1,
                layout: Some(layout(&[7, 9])),
                controller: Some(1),
                clients: Vec::new(),
                input_locked: false,
                allow_takeover: true,
                read_only: false,
                origin: crate::session::origin::SessionOrigin::default(),
            })
            .unwrap();

        assert!(
            backend.state().relaunch.restore.is_none(),
            "the restore is done"
        );
        assert_eq!(backend.state().current().focused_pane, Some(9));
        assert!(backend.state().current().engaged);
    });
}

#[test]
fn background_sessions_stay_covered_until_the_one_on_screen_is_attaching() {
    let mut state = State::new(
        crate::config::Config::default(),
        crate::state::ThemePreset::Lipan.theme(),
    );
    let handoff = ClientHandoff::new(
        Some(1),
        vec![
            HandoffSession {
                name: "bg".to_string(),
                remote: None,
                instance: None,
                transition: Transition::Carry { claim: false },
                token: None,
                read_only: false,
                auto_created: false,
                engaged: false,
                active_workspace: 0,
                focused_pane: None,
                parked_seq: 1,
            },
            HandoffSession {
                name: "dev".to_string(),
                remote: None,
                instance: None,
                transition: Transition::Carry { claim: false },
                token: None,
                read_only: false,
                auto_created: false,
                engaged: false,
                active_workspace: 0,
                focused_pane: None,
                parked_seq: 0,
            },
        ],
    );
    let queue: VecDeque<_> = handoff
        .restore_order()
        .into_iter()
        .map(|index| handoff.sessions[index].clone())
        .collect();
    state.relaunch.restore = Some(Restore {
        total: queue.len(),
        queue,
        current: None,
        launcher: false,
        problems: Vec::new(),
    });
    assert_eq!(covering_progress(&state), Some((0, 2)));
    let restore = state.relaunch.restore.as_mut().unwrap();
    assert_eq!(restore.queue.pop_front().unwrap().name, "bg");
    assert_eq!(covering_progress(&state), Some((1, 2)));
    let restore = state.relaunch.restore.as_mut().unwrap();
    assert_eq!(restore.queue.pop_front().unwrap().name, "dev");
    assert_eq!(
        covering_progress(&state),
        None,
        "the session on screen is shown as it attaches"
    );
}

fn layout(panes: &[crate::pane::id::PaneId]) -> crate::layout::shared::SharedLayout {
    use crate::layout::shared::{
        SHARED_LAYOUT_VERSION, SharedLayout, SharedLayoutKind, SharedPane, SharedSplitAxis,
        SharedWorkspace,
    };
    SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        pane_geometry: Default::default(),
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
                .map(|id| SharedPane {
                    pane_id: *id,
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
        }],
    }
}
