//! Restart rozi through the production update path: the plan the dialog shows, the transaction that
//! reserves and commits each slot before letting go, the handoff written for the new build, and the
//! restore that build runs.

use std::sync::mpsc::Receiver;

use tui_lipan::TestBackend;

use super::*;
use crate::session::client::{ClientOutbound, SessionClient};
use crate::session::protocol::{ClientMessage, PROTOCOL_VERSION};
use crate::state::{Attachment, ConnectionState, SharedSessionState};

/// The release every test server runs: older than the candidate.
const SERVER_RELEASE: &str = "0.0.40";

/// Make `attachment` a live connection to `name`, returning what its client sends.
fn connect(
    attachment: &mut Attachment,
    name: &str,
    remote: Option<&str>,
) -> Receiver<ClientOutbound> {
    let (mut client, sent) = SessionClient::test_channel();
    client.set_server_version(SERVER_RELEASE);
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

fn background(
    state: &mut State,
    epoch: AttachmentId,
    name: &str,
    remote: Option<&str>,
) -> Receiver<ClientOutbound> {
    let mut attachment = Attachment::new();
    let sent = connect(&mut attachment, name, remote);
    attachment.epoch = epoch;
    attachment.parked_seq = epoch;
    state.background.insert(epoch, attachment);
    sent
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

fn stage(backend: &TestBackend<AppRoot>) -> &RelaunchStage {
    &backend
        .state()
        .relaunch
        .dialog
        .as_ref()
        .expect("the dialog is open")
        .stage
}

fn ready_plan(backend: &TestBackend<AppRoot>) -> &RelaunchPlan {
    match stage(backend) {
        RelaunchStage::Ready(plan) => plan,
        other => panic!("the dialog has a plan, not {other:?}"),
    }
}

fn transitions(plan: &RelaunchPlan) -> Vec<(String, Transition)> {
    plan.sessions
        .iter()
        .map(|session| (session.facts.label(), session.transition.clone()))
        .collect()
}

/// The one request of `kind` the session sent, by its request id.
fn only_request(sent: &Receiver<ClientOutbound>) -> ClientMessage {
    let messages = sent_messages(sent);
    match messages.as_slice() {
        [message] => message.clone(),
        other => panic!("expected one request, got {other:?}"),
    }
}

fn prepared(backend: &mut TestBackend<AppRoot>, epoch: u64, message: ClientMessage) {
    let ClientMessage::PrepareHandoff { request_id } = message else {
        panic!("expected a reservation request, got {message:?}");
    };
    backend
        .dispatch(Msg::SessionHandoffPrepared {
            epoch,
            request_id,
            token: format!("token-{epoch}"),
            error: None,
        })
        .unwrap();
}

fn committed(backend: &mut TestBackend<AppRoot>, epoch: u64, message: ClientMessage) {
    let ClientMessage::CommitHandoff { request_id, token } = message else {
        panic!("expected a commit, got {message:?}");
    };
    assert_eq!(token, format!("token-{epoch}"));
    backend
        .dispatch(Msg::SessionHandoffCommitted {
            epoch,
            request_id,
            error: None,
        })
        .unwrap();
}

#[test]
fn a_session_is_carried_over_whenever_its_server_shares_a_generation_with_the_new_build() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        {
            let state = backend.state_mut();
            let _ = connect(state.current_mut(), "dev", None);
            // This client agreed on the older generation, but the server speaks the newer one too.
            if let Some(client) = state.current_mut().session_client.as_mut() {
                client.set_server_protocol(PROTOCOL_VERSION..=PROTOCOL_VERSION + 1);
            }
            let _ = background(state, 7, "api", Some("workbox"));
            let _ = background(state, 8, "eph-0123456789abcdef01234567", None);
            let _ = background(state, 9, "far", None);
            state.background.get_mut(&9).unwrap().connection = ConnectionState::Unreachable;
        }

        open_ready(&mut backend, PROTOCOL_VERSION + 1..=PROTOCOL_VERSION + 1);
        let plan = ready_plan(&backend);
        assert_eq!(
            transitions(plan),
            vec![
                ("dev".to_string(), Transition::Carry { claim: true }),
                ("api@workbox".to_string(), Transition::Upgrade),
                (
                    "eph-0123456789abcdef01234567".to_string(),
                    plan.sessions[2].transition.clone()
                ),
                ("far".to_string(), Transition::Reconnect),
            ],
            "the session on screen first, then the background by name"
        );
        assert!(
            matches!(plan.sessions[2].transition, Transition::Blocked { .. }),
            "a temporary session cannot be upgraded"
        );
        assert!(plan.blocked() && plan.destructive());
    });
}

#[test]
fn restarting_reserves_writes_commits_and_only_then_lets_go() {
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
        for (epoch, sent) in [(epoch, &on_screen), (42, &parked)] {
            let request = only_request(sent);
            prepared(&mut backend, epoch, request);
        }
        assert!(matches!(stage(&backend), RelaunchStage::Committing(_)));
        assert!(
            backend.state().relaunch.handover.take().is_none(),
            "nothing is handed over before every commit is answered"
        );

        let commit = only_request(&on_screen);
        committed(&mut backend, epoch, commit);
        assert!(backend.state().relaunch.handover.take().is_none());
        let commit = only_request(&parked);
        committed(&mut backend, 42, commit);

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
fn a_server_that_does_not_answer_stops_the_restart_and_the_client_stays() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        let (dev, api) = {
            let state = backend.state_mut();
            let dev = connect(state.current_mut(), "dev", None);
            let api = background(state, 7, "api", None);
            (dev, api)
        };
        let epoch = backend.state().runtime_epoch;
        open_ready(&mut backend, PROTOCOL_VERSION..=PROTOCOL_VERSION);
        backend.dispatch(Msg::RelaunchConfirm).unwrap();
        // `dev` answers, `api` never does.
        let request = only_request(&dev);
        prepared(&mut backend, epoch, request);
        let _ = only_request(&api);
        let attempt = backend.state().relaunch.attempt;

        backend
            .dispatch(Msg::RelaunchAnswerTimeout { attempt })
            .unwrap();
        assert!(backend.state().relaunch.handover.take().is_none());
        let plan = ready_plan(&backend);
        assert_eq!(plan.notice.as_deref(), Some("`api` did not answer"));
        assert_eq!(
            sent_messages(&dev),
            vec![ClientMessage::AbortHandoff {
                token: format!("token-{epoch}")
            }],
            "the reservation already granted is taken back"
        );
        assert!(sent_messages(&api).is_empty(), "nothing is detached");
    });
}

#[test]
fn a_refused_commit_takes_every_commit_back_and_discards_the_handoff() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        let dev = connect(backend.state_mut().current_mut(), "dev", None);
        let epoch = backend.state().runtime_epoch;
        open_ready(&mut backend, PROTOCOL_VERSION..=PROTOCOL_VERSION);
        backend.dispatch(Msg::RelaunchConfirm).unwrap();
        let request = only_request(&dev);
        prepared(&mut backend, epoch, request);
        let ClientMessage::CommitHandoff { request_id, .. } = only_request(&dev) else {
            panic!("expected a commit");
        };

        backend
            .dispatch(Msg::SessionHandoffCommitted {
                epoch,
                request_id,
                error: Some("no reservation for that token".to_string()),
            })
            .unwrap();
        assert!(backend.state().relaunch.handover.take().is_none());
        assert!(
            ready_plan(&backend)
                .notice
                .as_deref()
                .is_some_and(|notice| notice.contains("could not hold its place"))
        );
        assert_eq!(
            sent_messages(&dev),
            vec![ClientMessage::AbortHandoff {
                token: format!("token-{epoch}")
            }]
        );
    });
}

#[test]
fn a_plan_that_changed_since_it_was_shown_is_shown_again_before_anything_is_reserved() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        let dev = connect(backend.state_mut().current_mut(), "dev", None);
        let epoch = backend.state().runtime_epoch;
        open_ready(&mut backend, PROTOCOL_VERSION..=PROTOCOL_VERSION);

        // Renamed while the dialog was open.
        backend.state_mut().current_mut().session_name = Some("renamed".to_string());
        backend.dispatch(Msg::RelaunchConfirm).unwrap();
        assert!(sent_messages(&dev).is_empty(), "nothing was reserved");
        let plan = ready_plan(&backend);
        assert_eq!(plan.sessions[0].facts.name, "renamed");
        assert!(plan.notice.is_some());

        // Changed again after the reservation: taken back before anything is committed.
        backend.dispatch(Msg::RelaunchConfirm).unwrap();
        let request = only_request(&dev);
        backend.state_mut().current_mut().session_name = Some("again".to_string());
        prepared(&mut backend, epoch, request);
        assert_eq!(
            sent_messages(&dev),
            vec![ClientMessage::AbortHandoff {
                token: format!("token-{epoch}")
            }]
        );
        assert_eq!(ready_plan(&backend).sessions[0].facts.name, "again");
    });
}

#[test]
fn closing_the_dialog_mid_restart_takes_back_what_was_reserved() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        let (dev, api) = {
            let state = backend.state_mut();
            (
                connect(state.current_mut(), "dev", None),
                background(state, 7, "api", None),
            )
        };
        let epoch = backend.state().runtime_epoch;
        open_ready(&mut backend, PROTOCOL_VERSION..=PROTOCOL_VERSION);
        backend.dispatch(Msg::RelaunchConfirm).unwrap();
        let request = only_request(&dev);
        prepared(&mut backend, epoch, request);
        let _ = only_request(&api);

        backend.dispatch(Msg::RelaunchCancel).unwrap();
        assert!(backend.state().relaunch.dialog.is_none());
        assert_eq!(
            sent_messages(&dev),
            vec![ClientMessage::AbortHandoff {
                token: format!("token-{epoch}")
            }]
        );
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
            let _ = background(state, 7, "api", Some("workbox"));
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

fn session(name: &str, transition: Transition, parked_seq: u64) -> HandoffSession {
    HandoffSession {
        name: name.to_string(),
        remote: None,
        instance: None,
        transition,
        token: Some(format!("token-{name}")),
        read_only: false,
        auto_created: false,
        engaged: true,
        active_workspace: 0,
        focused_pane: None,
        parked_seq,
    }
}

fn step_names(restore: &Restore) -> Vec<String> {
    restore
        .steps
        .iter()
        .map(|step| match step {
            RestoreStep::Attach(session) => format!("attach {}", session.name),
            RestoreStep::Upgrade(session) => format!("upgrade {}", session.name),
            RestoreStep::Land => "land".to_string(),
        })
        .collect()
}

#[test]
fn every_slot_is_claimed_before_any_upgrade_starts() {
    let handoff = ClientHandoff::new(
        Some(1),
        vec![
            session("old", Transition::Upgrade, 1),
            session("dev", Transition::Carry { claim: true }, 0),
            session("api", Transition::Carry { claim: true }, 2),
            session("also-old", Transition::Upgrade, 3),
            session("far", Transition::Reconnect, 4),
        ],
    );
    let restore = restore_for(&handoff);
    assert_eq!(
        step_names(&restore),
        vec![
            "attach api",
            "attach far",
            "attach dev",
            "upgrade old",
            "upgrade also-old",
            "land",
        ],
        "carried sessions by recency with the one on screen last, then upgrades, then landing"
    );
    assert_eq!(restore.foreground, Some(("dev".to_string(), None)));
    assert_eq!(restore.total, 5);
}

#[test]
fn an_upgrade_that_finished_is_attached_next_and_one_that_failed_is_reported() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        let upgrading = session("old", Transition::Upgrade, 1);
        backend.state_mut().relaunch.attempt = 7;
        backend.state_mut().relaunch.restore = Some(Restore {
            steps: VecDeque::from([RestoreStep::Upgrade(session(
                "next",
                Transition::Upgrade,
                2,
            ))]),
            current: Some(RestoreInFlight::Upgrade {
                attempt: 7,
                session: upgrading,
            }),
            total: 2,
            done: 0,
            foreground: None,
            foreground_epoch: None,
            recency: Vec::new(),
            problems: Vec::new(),
        });
        assert_eq!(
            covering_activity(backend.state()),
            Some(RestoreActivity::Upgrading {
                session: "old".to_string()
            })
        );

        // A stale answer from another attempt changes nothing.
        backend
            .update_level(Msg::RelaunchUpgraded {
                attempt: 6,
                result: Ok(()),
            })
            .unwrap();
        assert!(
            backend
                .state()
                .relaunch
                .restore
                .as_ref()
                .unwrap()
                .current
                .is_some()
        );

        backend
            .update_level(Msg::RelaunchUpgraded {
                attempt: 7,
                result: Ok(()),
            })
            .unwrap();
        let restore = backend.state().relaunch.restore.as_ref().unwrap();
        assert_eq!(step_names(restore), vec!["attach old", "upgrade next"]);
        match &restore.steps[0] {
            RestoreStep::Attach(session) => {
                assert_eq!(session.token, None, "an upgraded server holds no slot")
            }
            _ => unreachable!(),
        }

        backend
            .state_mut()
            .relaunch
            .restore
            .as_mut()
            .unwrap()
            .current = Some(RestoreInFlight::Upgrade {
            attempt: 8,
            session: session("next", Transition::Upgrade, 2),
        });
        backend
            .update_level(Msg::RelaunchUpgraded {
                attempt: 8,
                result: Err("host unreachable".to_string()),
            })
            .unwrap();
        let restore = backend.state().relaunch.restore.as_ref().unwrap();
        assert_eq!(
            restore.problems,
            vec!["`next` was not upgraded: host unreachable".to_string()]
        );
        assert_eq!(restore.done, 1);
    });
}

#[test]
fn a_session_that_lands_gets_its_view_back_and_marks_where_the_foreground_is() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.render();
        let epoch = backend.state().runtime_epoch;
        let (client, _sent) = SessionClient::test_channel();
        let mut landing = session("dev", Transition::Carry { claim: true }, 0);
        landing.instance = Some("dev".to_string());
        landing.focused_pane = Some(9);
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
                steps: VecDeque::new(),
                current: Some(RestoreInFlight::Attach {
                    epoch,
                    session: landing,
                }),
                total: 1,
                done: 0,
                foreground: Some(("dev".to_string(), None)),
                foreground_epoch: None,
                recency: Vec::new(),
                problems: Vec::new(),
            });
        }

        backend
            .update_level(Msg::SessionAttached {
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

        let restore = backend.state().relaunch.restore.as_ref().unwrap();
        assert_eq!(restore.foreground_epoch, Some(epoch));
        assert_eq!(restore.done, 1);
        assert!(restore.problems.is_empty());
        assert_eq!(backend.state().current().focused_pane, Some(9));
        assert!(backend.state().current().engaged);

        // Landing on it, with nothing left to do, finishes the restore and uncovers the view.
        backend.dispatch(Msg::RelaunchRestoreNext).unwrap();
        assert!(backend.state().relaunch.restore.is_none());
        assert_eq!(covering_activity(backend.state()), None);
    });
}

#[test]
fn landing_returns_to_the_session_that_was_on_screen_and_restores_background_recency() {
    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        let foreground = 50;
        let (foreground_sent, other_sent) = {
            let state = backend.state_mut();
            let foreground_sent = background(state, foreground, "dev", None);
            let other_sent = background(state, 51, "api", None);
            let _ = connect(state.current_mut(), "old", None);
            state.relaunch.restore = Some(Restore {
                steps: VecDeque::from([RestoreStep::Land]),
                current: None,
                total: 3,
                done: 3,
                foreground: Some(("dev".to_string(), None)),
                foreground_epoch: Some(foreground),
                recency: vec![
                    (("api".to_string(), None), 9),
                    (("old".to_string(), None), 2),
                    (("dev".to_string(), None), 0),
                ],
                problems: Vec::new(),
            });
            (foreground_sent, other_sent)
        };
        let _ = sent_messages(&foreground_sent);

        backend.dispatch(Msg::RelaunchRestoreNext).unwrap();
        let state = backend.state();
        assert!(state.relaunch.restore.is_none());
        assert_eq!(state.current().session_name.as_deref(), Some("dev"));
        let seq = |name: &str| {
            state
                .background
                .values()
                .find(|attachment| attachment.session_name.as_deref() == Some(name))
                .map(|attachment| attachment.parked_seq)
                .unwrap()
        };
        assert!(
            seq("old") < seq("api"),
            "the background keeps the recency the previous process had"
        );
        let _ = other_sent;
    });
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
