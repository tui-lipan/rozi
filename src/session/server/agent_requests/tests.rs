use super::super::tests::{add_client, test_pane};
use super::*;
use crate::control::{ControlCommand, ControlRequest};
use crate::session::protocol::DetectedAgentState;

/// A session with an agent in each of `panes`, every one running a real PTY that echoes back
/// whatever is typed into it.
struct Rig {
    server: SessionServer,
    typed: HashMap<PaneId, mpsc::Receiver<TerminalPtyEvent>>,
    /// Integration reports sent, for their sequence numbers.
    reports: u64,
}

impl Rig {
    fn new(panes: &[PaneId]) -> Self {
        let mut server = SessionServer::new_named("requests-test");
        server.agent_requests.quiet = Duration::ZERO;
        let mut typed = HashMap::new();
        for &id in panes {
            let (events, received) = mpsc::channel();
            let pty = TerminalPty::spawn(
                TerminalPtyConfig::new("/bin/sh")
                    .arg("-c")
                    .arg("stty raw -echo; printf ready; cat"),
                move |event| {
                    let _ = events.send(event);
                },
            )
            .unwrap();
            let mut pane = test_pane(1);
            pane.pty = Some(pty);
            pane.runtime.detected_agent = Some(protocol::DetectedAgent {
                agent: protocol::AgentIdentity::new("claude", "Claude Code").into(),
                state: DetectedAgentState::Idle,
            });
            pane.agent.sync_references(&pane.runtime);
            server.panes.insert(id, pane);
            assert!(read_until(&received, "ready"));
            typed.insert(id, received);
        }
        Self {
            server,
            typed,
            reports: 0,
        }
    }

    fn agent(&self, pane: PaneId) -> protocol::AgentRef {
        self.server
            .resolve_agent_wait_target(AgentTarget::Pane(pane))
            .unwrap()
    }

    fn set_state(&mut self, pane: PaneId, state: DetectedAgentState) {
        let pane = self.server.panes.get_mut(&pane).unwrap();
        let before = pane.agent.agent_states(&pane.runtime);
        pane.runtime.detected_agent.as_mut().unwrap().state = state;
        pane.agent.observe(before, &pane.runtime);
        self.server.resolve_agent_waits();
    }

    /// Run pane `pane`'s agent through one turn: working, then idle.
    fn run_turn(&mut self, pane: PaneId) {
        self.set_state(pane, DetectedAgentState::Working);
        self.set_state(pane, DetectedAgentState::Idle);
        self.server.pump_agent_requests();
    }

    fn runs(&self, pane: PaneId) -> (u64, u64) {
        let agent = self
            .server
            .current_agent_runtime(&self.agent(pane))
            .unwrap();
        (agent.completions, agent.answered)
    }

    fn ask(&mut self, from: Option<PaneId>, to: PaneId, text: &str) -> AgentRequestInfo {
        let from = from.map(|pane| self.agent(pane));
        info(
            self.server
                .ask_agent(from, AgentTarget::Pane(to), text.into(), None, None, None),
        )
    }

    fn try_ask(&mut self, from: PaneId, to: PaneId) -> ControlResponse {
        let from = self.agent(from);
        self.server.ask_agent(
            Some(from),
            AgentTarget::Pane(to),
            "another".into(),
            None,
            None,
            None,
        )
    }

    /// Pane `pane`'s agent reports that it is in native conversation `native_session` now, as
    /// after a `/clear`.
    fn move_to(&mut self, pane: PaneId, native_session: &str) {
        self.reports += 1;
        let update = super::super::runtime::AgentIntegrationUpdate {
            agent: Some("claude".into()),
            integration: "hooks".into(),
            state: Some(protocol::AgentState::Idle),
            reason: None,
            native_session: Some(native_session.into()),
            unless_quiet: None,
            ended: false,
            seq: self.reports,
            response: None,
        };
        let generation = self.server.panes[&pane].generation;
        self.server
            .apply_agent_integration(None, pane, generation, update)
            .unwrap();
    }

    fn request(&self, id: &str) -> AgentRequestInfo {
        info(self.server.agent_request(id))
    }

    fn reply(&mut self, from: PaneId, id: &str, text: &str) -> ControlResponse {
        let from = self.agent(from);
        self.server.reply_to_request(Some(from), id, text.into())
    }

    /// Whether `text` reaches pane `pane`'s program within a few seconds.
    fn typed_into(&self, pane: PaneId, text: &str) -> bool {
        read_until(&self.typed[&pane], text)
    }
}

fn read_until(events: &mpsc::Receiver<TerminalPtyEvent>, text: &str) -> bool {
    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(TerminalPtyEvent::Output(bytes)) = events.recv_timeout(Duration::from_millis(100))
        {
            output.extend_from_slice(&bytes);
            if String::from_utf8_lossy(&output).contains(text) {
                return true;
            }
        }
    }
    false
}

fn info(response: ControlResponse) -> AgentRequestInfo {
    assert!(response.ok, "{response:?}");
    serde_json::from_value(response.data.unwrap()).unwrap()
}

fn code(response: ControlResponse) -> Option<ControlErrorCode> {
    assert!(!response.ok, "{response:?}");
    response.code
}

#[test]
fn a_request_follows_a_detected_agent_into_its_published_row() {
    // Each publishes the conversation on screen beside a background one; those marked `true`
    // keep the request, the rest are a replacement. A claim on the pane is in place before the
    // request, so the request knows the conversation by the id the claim reports.
    for (scenario, follows) in [
        ("handoff", true),
        ("program status", true),
        ("hook on the conversation", true),
        ("hook on a moved conversation", true),
        ("new conversation", true),
        ("hook on another conversation", false),
        ("nothing on screen", false),
        ("different agent", false),
        ("new launch", false),
    ] {
        let mut rig = Rig::new(&[3]);
        let (client, _stream) = super::super::tests::attach_client(&mut rig.server);
        let detected = rig.agent(3);
        let claim = |name: &str, native_session: Option<&str>| {
            Some(Box::new(protocol::AgentIntegrationReport {
                integration: name.into(),
                identity: protocol::AgentIdentity::new("claude", "Claude Code"),
                reference: detected.clone(),
                state: protocol::AgentState::Idle,
                reason: None,
                native_session: native_session.map(Into::into),
                unless_quiet: None,
                seq: 1,
                reported_at_unix_ms: 0,
            }))
        };
        let pane = rig.server.panes.get_mut(&3).unwrap();
        pane.runtime.integration = match scenario {
            "program status" => claim(super::super::runtime::PROGRAM_STATUS_INTEGRATION, None),
            "hook on the conversation" => claim("hook-1", Some("first-thread")),
            // A conversation moved to another process: its hooks report under the new process's
            // session, while the row keeps the one it resumes.
            "hook on a moved conversation" => claim("hook-1", Some("forked")),
            "hook on another conversation" => claim("hook-1", Some("background")),
            _ => None,
        };
        let request = rig.ask(None, 3, "What is 2+3?");
        assert_eq!(request.state, AgentRequestState::Active, "{scenario}");
        assert!(rig.typed_into(3, "What is 2+3?"));
        let set_claim_state = |rig: &mut Rig, state| {
            let pane = rig.server.panes.get_mut(&3).unwrap();
            if let Some(report) = pane.runtime.integration.as_mut() {
                report.state = state;
            }
        };
        set_claim_state(&mut rig, protocol::AgentState::Working);
        rig.set_state(3, DetectedAgentState::Working);

        let mut row = protocol::PublishedRow {
            publisher: String::new(),
            kind: protocol::PublisherKind::Agent,
            id: "first-thread".into(),
            title: "First question".into(),
            status: "working".into(),
            active: true,
            native_session: Some("first-thread".into()),
            live_session: None,
            reported: false,
            reason: None,
            work_started_at: None,
            cwd: None,
            project: None,
        };
        if scenario == "hook on a moved conversation" {
            row.live_session = Some("forked".into());
        }
        let publisher = || super::super::runtime::PanePublisher {
            id: "sessions".into(),
            kind: protocol::PublisherKind::Agent,
            epoch: 1,
            responses: Vec::new(),
        };
        let background = protocol::PublishedRow {
            id: "background".into(),
            active: false,
            native_session: Some("background".into()),
            live_session: None,
            ..row.clone()
        };
        let mut rows = vec![row.clone(), background.clone()];
        let pane = rig.server.panes.get_mut(&3).unwrap();
        match scenario {
            "nothing on screen" => rows[0].active = false,
            "different agent" => {
                pane.runtime.detected_agent.as_mut().unwrap().agent =
                    protocol::AgentIdentity::new("codex", "Codex").into();
            }
            "new launch" => pane.runtime.agent_launch += 1,
            _ => {}
        }
        rig.server
            .report_pane_rows(client, 3, 1, false, publisher(), rows)
            .unwrap();
        // A reference read before the row appeared, such as one a script saved from
        // `agents list`, names the row's agent only when it took the detected one over.
        let followed = rig
            .server
            .resolve_agent_wait_target(crate::control::AgentTarget::Ref(detected.clone()))
            .ok();
        if !follows {
            assert_eq!(
                rig.request(&request.id).state,
                AgentRequestState::Failed,
                "{scenario}"
            );
            assert!(
                followed.is_none_or(|reference| reference.slot.is_none()),
                "{scenario}"
            );
            continue;
        }
        let published = rig.agent(3);
        assert!(published.slot.is_some());
        assert_eq!(published.incarnation, detected.incarnation);
        assert_eq!(followed.as_ref(), Some(&published), "{scenario}");
        let moved = rig.request(&request.id);
        assert_eq!(moved.state, AgentRequestState::Active, "{scenario}");
        assert_eq!(moved.to, published);

        if scenario == "new conversation" {
            // The same row now shows another conversation, which cannot answer this request.
            let next = protocol::PublishedRow {
                native_session: Some("second-thread".into()),
                ..row
            };
            rig.server
                .report_pane_rows(client, 3, 1, false, publisher(), vec![next, background])
                .unwrap();
            assert_eq!(
                code(rig.reply(3, &request.id, "5")),
                Some(ControlErrorCode::NotPermitted)
            );
            continue;
        }
        rig.server
            .panes
            .get_mut(&3)
            .unwrap()
            .screen_mut()
            .process_bytes(b"5");
        set_claim_state(&mut rig, protocol::AgentState::Idle);
        let idle = protocol::PublishedRow {
            status: "idle".into(),
            ..row
        };
        rig.server
            .report_pane_rows(client, 3, 1, false, publisher(), vec![idle, background])
            .unwrap();
        let answered = rig.request(&request.id);
        assert_eq!(answered.state, AgentRequestState::Answered, "{scenario}");
        assert!(answered.response.unwrap().content.text.contains('5'));
    }
}

/// A reference read while the agent was only screen-detected keeps naming it as rows speak for
/// it: one window's publisher takes it over, another window's copy takes over when the first
/// leaves, and the rows go away while the agent runs on. A request it is answering moves with it
/// and can still be answered. A new launch is another agent.
#[test]
fn a_detected_agents_reference_outlives_the_rows_that_speak_for_it() {
    let mut rig = Rig::new(&[3]);
    let (first, _first_stream) = super::super::tests::attach_client(&mut rig.server);
    let (second, _second_stream) = super::super::tests::attach_client(&mut rig.server);
    let detected = rig.agent(3);
    let resolve = |rig: &Rig, reference: &protocol::AgentRef| {
        rig.server
            .resolve_agent_wait_target(AgentTarget::Ref(reference.clone()))
            .ok()
    };
    let publisher = || super::super::runtime::PanePublisher {
        id: "sessions".into(),
        kind: protocol::PublisherKind::Agent,
        epoch: 1,
        responses: Vec::new(),
    };
    let row = protocol::PublishedRow {
        publisher: String::new(),
        kind: protocol::PublisherKind::Agent,
        id: "thread".into(),
        title: "Question".into(),
        status: "idle".into(),
        active: true,
        native_session: Some("thread".into()),
        live_session: None,
        reported: false,
        reason: None,
        work_started_at: None,
        cwd: None,
        project: None,
    };

    rig.server
        .report_pane_rows(first, 3, 1, false, publisher(), vec![row.clone()])
        .unwrap();
    let published = rig.agent(3);
    assert!(published.slot.is_some());
    assert_eq!(resolve(&rig, &detected), Some(published.clone()));

    let request = rig.ask(None, 3, "What is 2+3?");
    assert_eq!(request.state, AgentRequestState::Active);
    assert_eq!(request.to, published);
    let working = protocol::PublishedRow {
        status: "working".into(),
        ..row
    };
    rig.server
        .report_pane_rows(first, 3, 1, false, publisher(), vec![working.clone()])
        .unwrap();
    rig.set_state(3, DetectedAgentState::Working);
    let still_answering = |rig: &mut Rig, to: &protocol::AgentRef| {
        rig.server.pump_agent_requests();
        let request = rig.request(&request.id);
        assert_eq!(
            request.state,
            AgentRequestState::Active,
            "{:?}",
            request.reason
        );
        assert_eq!(request.to, *to);
    };

    // The second window's copy waits while the first speaks, and takes over when it leaves.
    rig.server
        .report_pane_rows(second, 3, 1, false, publisher(), vec![working])
        .unwrap();
    rig.server.withdraw_client_rows(first);
    let promoted = rig.agent(3);
    assert!(promoted.slot.is_some() && promoted.slot != published.slot);
    assert_eq!(resolve(&rig, &detected), Some(promoted.clone()));
    assert_eq!(resolve(&rig, &published), Some(promoted.clone()));
    still_answering(&mut rig, &promoted);

    // Its rows withdrawn, the detected agent speaks for the pane again.
    rig.server
        .report_pane_rows(second, 3, 1, false, publisher(), Vec::new())
        .unwrap();
    let again = rig.agent(3);
    assert_eq!(again, detected);
    assert_eq!(resolve(&rig, &promoted), Some(detected.clone()));
    still_answering(&mut rig, &detected);
    assert!(rig.reply(3, &request.id, "5").ok);
    assert_eq!(rig.request(&request.id).state, AgentRequestState::Answered);

    let pane = rig.server.panes.get_mut(&3).unwrap();
    pane.runtime.agent_launch += 1;
    pane.agent.sync_references(&pane.runtime);
    assert_ne!(rig.agent(3), detected);
    assert_eq!(resolve(&rig, &detected), None);
}

/// A run a request started answers another agent, which gets the answer; the user's own runs
/// are the ones a client asks them to look at. A request whose turn ends without the agent ever
/// working started no run, so the agent's next run is the user's.
#[test]
fn a_run_that_answers_a_request_counts_apart() {
    let mut rig = Rig::new(&[3]);
    let asked = rig.ask(None, 3, "What is 2+3?");
    assert_eq!(asked.state, AgentRequestState::Active);
    rig.run_turn(3);
    assert_eq!(rig.runs(3), (1, 1));

    rig.run_turn(3);
    assert_eq!(rig.runs(3), (2, 1));

    let early = rig.ask(None, 3, "Still there?");
    assert_eq!(early.state, AgentRequestState::Active);
    assert!(rig.reply(3, &early.id, "yes").ok);
    // The agent never left its prompt, so the turn ends once the server stops waiting for it.
    rig.server.agent_requests.undetected_turn = Duration::ZERO;
    rig.server.pump_agent_requests();
    assert!(rig.server.agent_requests.slots.is_empty());
    rig.run_turn(3);
    assert_eq!(rig.runs(3), (3, 1));
}

#[test]
fn a_claude_usage_limit_pause_requires_fresh_quiet_after_the_screen_clears() {
    for (observed_work, auto_resumes) in [(true, true), (false, true), (true, false)] {
        let mut rig = Rig::new(&[3]);
        rig.server.agent_requests.quiet = QUIET_WINDOW;
        rig.move_to(3, "conversation");
        let target = rig.agent(3);
        let pane = rig.server.panes.get_mut(&3).unwrap();
        pane.agent.identity = Some("claude".into());
        pane.screen_mut().resize(10, 100);
        let show = |rig: &mut Rig, state, footer: &str| {
            let pane = rig.server.panes.get_mut(&3).unwrap();
            pane.runtime.integration.as_mut().unwrap().state = state;
            pane.screen_mut().process_bytes(b"\x1b[2J\x1b[H");
            pane.screen_mut().process_bytes(footer.as_bytes());
            rig.server.pump_agent_requests();
        };
        let expire_quiet = |rig: &mut Rig| {
            let before_quiet = Instant::now() - QUIET_WINDOW;
            *rig.server
                .agent_requests
                .stopped_since
                .get_mut(&target)
                .unwrap() = before_quiet;
            if let Some(input) = rig.server.agent_requests.pane_input.get_mut(&3) {
                *input = before_quiet;
            }
            rig.server.pump_agent_requests();
        };
        let quota = "────────────────\r\n  ⚠ Usage limit reached · limit resets 2pm";
        let cleared =
            "● Usage limit reached · continuing automatically at 2pm · esc to cancel\r\n❯";
        show(&mut rig, protocol::AgentState::Error, quota);
        let request = rig.ask(None, 3, "Review the fix");
        assert_eq!(request.state, AgentRequestState::Queued);
        assert_eq!(
            request.reason.as_deref(),
            Some("the agent's screen is blocked")
        );
        assert!(
            !rig.server
                .agent_requests
                .stopped_since
                .contains_key(&target)
        );

        // A redraw can clear the warning while the integration still reports the old error.
        show(&mut rig, protocol::AgentState::Error, cleared);
        let queued = rig.request(&request.id);
        assert_eq!(queued.state, AgentRequestState::Queued);
        assert_eq!(
            queued.reason.as_deref(),
            Some("waiting for the agent's pane to settle")
        );
        show(&mut rig, protocol::AgentState::Working, "");
        assert_eq!(rig.request(&request.id).state, AgentRequestState::Queued);
        show(&mut rig, protocol::AgentState::Idle, cleared);
        expire_quiet(&mut rig);
        assert_eq!(rig.request(&request.id).state, AgentRequestState::Active);
        assert!(rig.typed_into(3, &format!("Request {}", request.id)));

        if observed_work {
            show(&mut rig, protocol::AgentState::Working, "");
        }
        show(&mut rig, protocol::AgentState::Error, quota);
        let followup = rig.ask(None, 3, "One more check");
        assert_eq!(followup.state, AgentRequestState::Queued);
        assert!(
            !rig.server
                .agent_requests
                .stopped_since
                .contains_key(&target)
        );
        // Even a pause that outlasts the delivery timeout has to wait for the agent to resume.
        rig.server
            .agent_requests
            .slots
            .get_mut(&target)
            .unwrap()
            .delivered = Instant::now() - UNDETECTED_TURN;
        show(&mut rig, protocol::AgentState::Error, cleared);
        let waiting = rig.request(&request.id);
        assert_eq!(waiting.state, AgentRequestState::Active);
        assert!(waiting.response.is_none());

        if auto_resumes {
            show(&mut rig, protocol::AgentState::Working, "");
            assert!(rig.reply(3, &request.id, "Reviewed").ok);
            assert_eq!(
                rig.request(&request.id).response.unwrap().content.source,
                AgentResponseSource::AgentCli
            );
        } else {
            // Cancelling the pause leaves the agent stopped. The terminal is available as a
            // fallback once the quiet window elapses.
            expire_quiet(&mut rig);
            assert_eq!(
                rig.request(&request.id).response.unwrap().content.source,
                AgentResponseSource::Terminal
            );
        }
    }
}

#[test]
fn a_request_that_leaves_no_trace_is_typed_once_more_then_fails() {
    let mut rig = Rig::new(&[3]);
    rig.server.agent_requests.delivery_check = Duration::ZERO;
    // The server's copy of the screen never shows what the fake agent echoes: as far as it can
    // tell, the program dropped the request without a trace, like one still starting up.
    let request = rig.ask(None, 3, "What is 2+3?");
    let typed = format!("Request {}", request.id);
    assert!(rig.typed_into(3, &typed));
    rig.server.pump_agent_requests();
    assert_eq!(rig.request(&request.id).state, AgentRequestState::Active);
    assert!(rig.typed_into(3, &typed), "typed again");

    rig.server.pump_agent_requests();
    let failed = rig.request(&request.id);
    assert_eq!(failed.state, AgentRequestState::Failed);
    assert_eq!(failed.reason.as_deref(), Some(NOT_TAKEN));
    assert!(rig.server.agent_requests.slots.is_empty());
}

#[test]
fn a_dropped_request_is_never_typed_over_fresh_input() {
    let mut rig = Rig::new(&[3]);
    let request = rig.ask(None, 3, "What is 2+3?");
    let typed = format!("Request {}", request.id);
    assert!(rig.typed_into(3, &typed));
    // The agent is seen stopped long enough to be ready for typing, before anyone types.
    rig.server.pump_agent_requests();
    let quiet = Duration::from_millis(150);
    rig.server.agent_requests.quiet = quiet;
    std::thread::sleep(quiet);

    // Someone starts a draft just as the dropped request is checked.
    rig.server.agent_requests.delivery_check = Duration::ZERO;
    rig.server
        .session_write_input(3, b"draft".to_vec())
        .unwrap();
    assert!(rig.typed_into(3, "draft"));
    rig.server.pump_agent_requests();
    assert_eq!(rig.server.agent_requests.slots[&rig.agent(3)].attempts, 1);

    std::thread::sleep(quiet);
    rig.server.pump_agent_requests();
    assert_eq!(rig.server.agent_requests.slots[&rig.agent(3)].attempts, 2);
    assert!(
        rig.typed_into(3, &typed),
        "typed again once the pane was quiet"
    );
}

#[test]
fn only_a_pane_showing_the_request_counts_as_having_taken_it() {
    let mut rig = Rig::new(&[3, 4]);
    rig.server.agent_requests.delivery_check = Duration::ZERO;
    let show = |rig: &mut Rig, pane: PaneId, text: &str| {
        rig.server
            .panes
            .get_mut(&pane)
            .unwrap()
            .screen_mut()
            .process_bytes(text.as_bytes());
    };

    // Shown, here wrapped across two rows: it arrived, so it waits for its turn as before.
    let shown = rig.ask(None, 3, "What is 2+3?");
    let (head, tail) = shown.id.split_at(6);
    show(&mut rig, 3, &format!("Request {head}\r\n{tail}:\r\n"));
    rig.server.pump_agent_requests();
    assert_eq!(rig.request(&shown.id).state, AgentRequestState::Active);
    let slot = &rig.server.agent_requests.slots[&rig.agent(3)];
    assert!(slot.shown);
    assert_eq!(slot.attempts, 1);

    // The pane moved on to something else without showing it: typing it again could land
    // anywhere, so it fails instead.
    let missing = rig.ask(None, 4, "What is 2+3?");
    show(&mut rig, 4, "Do you trust this folder?\r\n");
    rig.server.pump_agent_requests();
    let failed = rig.request(&missing.id);
    assert_eq!(failed.state, AgentRequestState::Failed);
    assert_eq!(failed.reason.as_deref(), Some(NOT_TAKEN));
}

#[test]
fn requests_reach_a_target_one_turn_at_a_time() {
    let mut rig = Rig::new(&[2, 3, 4]);

    // Input a headless command writes keeps the pane from settling, so nothing is typed over it;
    // the delivery itself counts as input.
    rig.server.agent_requests.quiet = Duration::from_millis(50);
    let quiet = rig.ask(None, 4, "settle first");
    std::thread::sleep(Duration::from_millis(60));
    rig.server
        .session_write_input(4, b"draft".to_vec())
        .unwrap();
    rig.server.pump_agent_requests();
    assert_eq!(rig.request(&quiet.id).state, AgentRequestState::Queued);
    std::thread::sleep(Duration::from_millis(60));
    rig.server.pump_agent_requests();
    assert_eq!(rig.request(&quiet.id).state, AgentRequestState::Active);
    assert!(
        !rig.server
            .agent_requests
            .stopped_since
            .contains_key(&rig.agent(4))
    );
    rig.server.agent_requests.quiet = Duration::ZERO;

    rig.set_state(3, DetectedAgentState::Working);
    let first = rig.ask(Some(2), 3, "review the parser");
    let second = rig.ask(Some(2), 3, "and the lexer");
    assert_eq!(first.state, AgentRequestState::Queued);
    assert_eq!(first.reason.as_deref(), Some("the agent is working"));
    // Both sides are named as they were when it was asked, so the request keeps naming them after
    // either agent is gone.
    assert_eq!(first.from_label.as_deref(), Some("Claude Code"));
    assert_eq!(first.to_label.as_deref(), Some("Claude Code"));

    rig.set_state(3, DetectedAgentState::Idle);
    assert_eq!(rig.request(&first.id).state, AgentRequestState::Active);
    assert_eq!(rig.request(&second.id).state, AgentRequestState::Queued);
    assert!(rig.typed_into(
        3,
        &format!("rozi agents reply {} --format text --stdin", first.id)
    ));

    // An approval dialog is not the end of the turn.
    rig.set_state(3, DetectedAgentState::Working);
    rig.set_state(3, DetectedAgentState::Blocked);
    assert_eq!(rig.request(&first.id).state, AgentRequestState::Active);
    assert_eq!(rig.request(&second.id).state, AgentRequestState::Queued);

    rig.server
        .panes
        .get_mut(&3)
        .unwrap()
        .screen_mut()
        .process_bytes(b"parser is fine");
    rig.set_state(3, DetectedAgentState::Idle);
    let answered = rig.request(&first.id);
    assert_eq!(answered.state, AgentRequestState::Answered);
    let response = answered.response.unwrap();
    assert_eq!(response.content.source, AgentResponseSource::Terminal);
    assert!(response.content.text.contains("parser is fine"));
    assert_eq!(rig.request(&second.id).state, AgentRequestState::Active);

    // An integration's answer published after delivery beats the screen, without the turn ending.
    rig.server.agent_responses.insert(
        rig.agent(3),
        None,
        AgentResponsePayload {
            text: "the lexer is fine".into(),
            turn_id: None,
            source: AgentResponseSource::NativeHook,
            fidelity: AgentResponseFidelity::Semantic,
            complete: true,
            truncated: false,
        },
    );
    rig.server.pump_agent_requests();
    let answered = rig.request(&second.id);
    assert_eq!(answered.state, AgentRequestState::Answered);
    assert_eq!(answered.response.unwrap().content.text, "the lexer is fine");
}

#[test]
fn a_request_answered_later_outlives_its_turn_until_the_reply() {
    let mut rig = Rig::new(&[2, 3]);
    let first = rig.ask(Some(2), 3, "run the slow suite");
    let second = rig.ask(Some(2), 3, "and then this");
    assert!(rig.typed_into(
        3,
        &format!("rozi agents reply {} --format text --later", first.id)
    ));
    rig.set_state(3, DetectedAgentState::Working);

    // Only the agent the request went to can promise a later answer.
    let caller = rig.agent(2);
    assert_eq!(
        code(rig.server.reply_later(Some(caller), &first.id)),
        Some(ControlErrorCode::NotPermitted)
    );
    let target = rig.agent(3);
    let deferred = info(rig.server.reply_later(Some(target.clone()), &first.id));
    assert_eq!(deferred.state, AgentRequestState::Active);
    assert_eq!(
        code(rig.server.reply_later(Some(target), &second.id)),
        Some(ControlErrorCode::Conflict),
        "a queued request is not delivered yet"
    );
    // What the agent asks while it owes the answer belongs to the request it owes.
    let follow_up = rig.ask(Some(3), 2, "which tests are slow?");
    assert_eq!(follow_up.parent.as_deref(), Some(first.id.as_str()));

    // The turn ends on a background command: nothing is read from the screen, and the agent is
    // free to take the next request.
    rig.server
        .panes
        .get_mut(&3)
        .unwrap()
        .screen_mut()
        .process_bytes(b"started the suite in the background");
    rig.set_state(3, DetectedAgentState::Idle);
    assert_eq!(
        rig.runs(3),
        (1, 1),
        "the deferred request's run stays quiet"
    );
    let waiting = rig.request(&first.id);
    assert_eq!(waiting.state, AgentRequestState::Active);
    assert!(waiting.response.is_none());
    assert_eq!(rig.request(&second.id).state, AgentRequestState::Active);

    let answered = info(rig.reply(3, &first.id, "the suite passed"));
    assert_eq!(answered.state, AgentRequestState::Answered);
    assert_eq!(answered.response.unwrap().content.text, "the suite passed");

    // A promised answer fails once its agent is replaced.
    let target = rig.agent(3);
    rig.server.reply_later(Some(target), &second.id);
    let pane = rig.server.panes.get_mut(&3).unwrap();
    pane.runtime.agent_launch += 1;
    pane.agent.sync_references(&pane.runtime);
    rig.server.pump_agent_requests();
    assert_eq!(rig.request(&second.id).state, AgentRequestState::Failed);
}

#[test]
fn an_agent_owing_several_requests_names_the_one_it_asks_for() {
    let mut rig = Rig::new(&[2, 3, 4]);
    let first = rig.ask(Some(2), 3, "run the slow suite");
    assert!(rig.typed_into(
        3,
        &format!("rozi agents reply {} --format text --later", first.id)
    ));
    info(rig.server.reply_later(Some(rig.agent(3)), &first.id));
    let second = rig.ask(Some(4), 3, "run the fuzzer");
    assert!(rig.typed_into(
        3,
        &format!("rozi agents reply {} --format text --later", second.id)
    ));
    info(rig.server.reply_later(Some(rig.agent(3)), &second.id));

    // Neither delivery was observed working. Deferring must not hide the next user run.
    rig.run_turn(3);
    assert_eq!(rig.runs(3), (1, 0));

    // Either promise could be the one asking, so the ask must say which.
    let from = rig.agent(3);
    let mut ask = |parent: Option<&str>| {
        rig.server.ask_agent(
            Some(from.clone()),
            AgentTarget::Pane(2),
            "which tests are slow?".into(),
            parent,
            None,
            None,
        )
    };
    assert_eq!(code(ask(None)), Some(ControlErrorCode::InvalidArgument));
    assert_eq!(
        code(ask(Some("elsewhere"))),
        Some(ControlErrorCode::NotPermitted)
    );
    let follow_up = info(ask(Some(&first.id)));
    assert_eq!(follow_up.parent.as_deref(), Some(first.id.as_str()));
    // So the asker of the first request cannot wait for it while it blocks the answer.
    let asker = rig.agent(2);
    assert_eq!(
        rig.server
            .agent_requests
            .await_cycle(Some(&asker), &first.id),
        Some(follow_up.id.as_str())
    );
}

#[test]
fn a_promised_answer_fails_when_its_agent_changes_conversation() {
    let mut rig = Rig::new(&[3]);
    let request = rig.ask(None, 3, "run the slow suite");
    assert!(rig.typed_into(
        3,
        &format!("rozi agents reply {} --format text --later", request.id)
    ));
    let target = rig.agent(3);
    info(rig.server.reply_later(Some(target.clone()), &request.id));

    let generation = rig.server.panes[&3].generation;
    let mut report = |seq, native_session: &str| {
        let update = super::super::runtime::AgentIntegrationUpdate {
            agent: Some("claude".into()),
            integration: "hooks".into(),
            state: Some(protocol::AgentState::Working),
            reason: None,
            native_session: Some(native_session.into()),
            unless_quiet: None,
            ended: false,
            seq,
            response: None,
        };
        rig.server
            .apply_agent_integration(None, 3, generation, update)
            .unwrap();
    };
    // Delivered before the agent named its conversation, the request binds to the first one. Two
    // reports read in one server iteration still see it bound in between.
    report(1, "conversation a");
    report(2, "conversation b");
    rig.server.pump_agent_requests();
    let failed = rig.request(&request.id);
    assert_eq!(failed.state, AgentRequestState::Failed);
    assert_eq!(
        failed.reason.as_deref(),
        Some("the agent moved to another conversation")
    );
}

/// The request lists queued for `client` and not yet sent, in order.
fn request_lists(server: &SessionServer, client: ClientId) -> Vec<Vec<String>> {
    use super::super::tests::{DecodedOutboxFrame, decode_outbox_frames};
    decode_outbox_frames(server.client(client).expect("attached client"))
        .into_iter()
        .filter_map(|frame| match frame {
            DecodedOutboxFrame::Control(message) => match *message {
                protocol::ServerMessage::AgentRequestsChanged { requests, .. } => {
                    Some(requests.into_iter().map(|request| request.id).collect())
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[test]
fn attached_clients_hear_of_each_change_and_a_new_one_gets_the_list() {
    let mut rig = Rig::new(&[2, 3]);
    let (first, _first_stream) = super::super::tests::attach_client(&mut rig.server);

    let asked = rig.ask(Some(2), 3, "review the parser");
    rig.server.broadcast_agent_request_changes();
    rig.server.broadcast_agent_request_changes();
    assert_eq!(
        request_lists(&rig.server, first),
        [vec![asked.id.clone()]],
        "one change is one message"
    );

    let (second, _second_stream) = super::super::tests::add_client(&mut rig.server);
    rig.server.handle_message(
        second,
        protocol::ClientMessage::Attach {
            capabilities: Some(protocol::Capabilities::current()),
            session: rig.server.session_name.clone(),
            protocol_version: protocol::PROTOCOL_VERSION,
            min_protocol_version: protocol::PROTOCOL_VERSION,
            label: "second".into(),
            read_only: false,
            shares_filesystem: true,
            expected_server_nonce: None,
            handoff_token: None,
        },
    );
    assert_eq!(request_lists(&rig.server, second), [vec![asked.id]]);
}

#[test]
fn a_locked_or_replaced_target_is_never_typed_into() {
    let mut rig = Rig::new(&[2, 3]);
    rig.server.input_locked = true;
    let first = rig.ask(None, 3, "first");
    assert_eq!(first.state, AgentRequestState::Queued);
    rig.server.input_locked = false;
    rig.server.pump_agent_requests();
    assert_eq!(rig.request(&first.id).state, AgentRequestState::Active);

    let second = rig.ask(None, 3, "second");
    rig.set_state(2, DetectedAgentState::Working);
    let third = rig.ask(None, 2, "third");
    rig.server
        .agent_requests
        .info_mut(&third.id)
        .unwrap()
        .to_native_session = Some("another conversation".into());
    // Asked by the agent about to be replaced, so nobody is left to receive the answer.
    let orphan = rig.ask(Some(3), 2, "orphan");
    assert_eq!(orphan.state, AgentRequestState::Queued);
    // Asked from a conversation its agent has since left.
    let moved = rig.ask(Some(3), 2, "moved");
    rig.server
        .agent_requests
        .info_mut(&moved.id)
        .unwrap()
        .from_native_session = Some("another conversation".into());
    rig.server.pump_agent_requests();
    assert_eq!(rig.request(&moved.id).state, AgentRequestState::Cancelled);
    assert_eq!(rig.request(&orphan.id).state, AgentRequestState::Queued);
    let pane = rig.server.panes.get_mut(&3).unwrap();
    pane.runtime.agent_launch += 1;
    pane.agent.sync_references(&pane.runtime);
    rig.server.pump_agent_requests();
    for id in [&first.id, &second.id, &third.id] {
        assert_eq!(rig.request(id).state, AgentRequestState::Failed, "{id}");
    }
    assert_eq!(rig.request(&orphan.id).state, AgentRequestState::Cancelled);
}

#[test]
fn only_the_target_answers_and_its_first_answer_stands() {
    let mut rig = Rig::new(&[2, 3]);
    rig.set_state(3, DetectedAgentState::Working);
    let request = rig.ask(Some(2), 3, "what changed?");

    assert_eq!(
        code(rig.reply(2, &request.id, "not mine to answer")),
        Some(ControlErrorCode::NotPermitted)
    );
    let answered = info(rig.reply(3, &request.id, "the parser"));
    assert_eq!(answered.state, AgentRequestState::Answered);
    assert!(
        info(rig.reply(3, &request.id, "the parser"))
            .response
            .is_some()
    );
    assert_eq!(
        code(rig.reply(3, &request.id, "something else")),
        Some(ControlErrorCode::Conflict)
    );

    // Answered while queued, so it is never typed into the agent.
    rig.set_state(3, DetectedAgentState::Idle);
    assert!(rig.server.agent_requests.answering(&rig.agent(3)).is_none());
    // The reply stays on the request: `agents response` also answers clients that predate
    // requests and cannot decode its source, and the list carries no prompt or answer text.
    assert!(
        rig.server
            .agent_responses
            .latest(&rig.agent(3), &None)
            .is_none()
    );
    let list = rig.server.agent_request_list(None, None).data.unwrap();
    assert_eq!(list[0]["id"], request.id.as_str());
    assert!(list[0].get("prompt").is_none() && list[0].get("response").is_none());
}

#[test]
fn lineage_follows_the_request_being_answered_and_is_bounded() {
    let mut rig = Rig::new(&[2, 3, 4]);
    let parent = rig.ask(Some(2), 3, "plan the release");
    assert_eq!(parent.state, AgentRequestState::Active);
    rig.set_state(4, DetectedAgentState::Working);
    let child = rig.ask(Some(3), 4, "check the changelog");
    assert_eq!(child.parent.as_deref(), Some(parent.id.as_str()));
    assert_eq!(child.root, parent.id);

    rig.server.agent_requests.requests[0].depth = MAX_DEPTH;
    assert_eq!(code(rig.try_ask(3, 4)), Some(ControlErrorCode::Conflict));
    rig.server.agent_requests.requests[0].depth = 1;

    for _ in 1..MAX_QUEUED_PER_TARGET {
        rig.try_ask(3, 4);
    }
    assert_eq!(code(rig.try_ask(3, 4)), Some(ControlErrorCode::Conflict));
}

#[test]
fn an_ask_keeps_its_uploads_only_when_the_request_is_created() {
    let mut rig = Rig::new(&[3]);
    let first = rig.server.upload_for_test(1, b"one");
    let second = rig.server.upload_for_test(2, b"two");
    let control = |rig: &mut Rig, command: ControlCommand| {
        let messages = rig.server.handle_session_control(
            0,
            "requests-test".into(),
            PROTOCOL_VERSION,
            protocol::MIN_SUPPORTED_PROTOCOL,
            Some(protocol::Capabilities::current()),
            ControlRequest {
                command,
                source_pane: None,
                source_session: None,
                extension: None,
                caller_context: None,
                credential: None,
            },
        );
        let Some((_, ServerMessage::SessionControlResult { response, .. })) =
            messages.into_iter().next()
        else {
            panic!("a control result")
        };
        response
    };
    let ask = |attachments: Vec<u64>| ControlCommand::AgentAsk {
        target: AgentTarget::Pane(3),
        prompt: "look at these".into(),
        parent: None,
        follows: None,
        asker_label: None,
        attachments,
    };

    // One missing upload refuses the whole ask, and the others stay unclaimed.
    let refused = control(&mut rig, ask(vec![1, 99]));
    assert_eq!(code(refused), Some(ControlErrorCode::InvalidArgument));
    assert_eq!(rig.server.attachment_counts(), (0, 2, 0));
    // An asker that gives up drops its uploads, and their files go at once.
    let cancel = ControlCommand::AgentAttachCancel { transfers: vec![2] };
    assert!(control(&mut rig, cancel).ok);
    assert!(!second.exists());
    assert_eq!(rig.server.attachment_counts(), (0, 1, 0));

    let asked = info(control(&mut rig, ask(vec![1])));
    assert!(
        asked
            .prompt
            .ends_with(&format!("\n\nAttached files:\n- {}", first.display())),
        "{}",
        asked.prompt
    );
    assert_eq!(rig.server.attachment_counts(), (0, 0, 1));
    assert!(first.exists());
}

#[test]
fn exchanges_between_the_same_two_agents_make_one_conversation() {
    let mut rig = Rig::new(&[2, 3, 4]);
    let first = rig.ask(Some(2), 3, "plan the release");
    assert_eq!(first.conversation.as_deref(), Some(first.id.as_str()));
    let again = rig.ask(Some(2), 3, "and the changelog?");
    assert_eq!(again.conversation, first.conversation);
    assert_eq!(again.parent, None);
    let header = |request: &AgentRequestInfo| {
        let frame = rig.server.request_frame(request);
        frame.lines().next().unwrap_or_default().to_string()
    };
    assert!(
        header(&again).ends_with(&format!(", following {}:", first.id)),
        "{}",
        header(&again)
    );
    assert!(
        header(&first).ends_with(" in pane 2:"),
        "{}",
        header(&first)
    );
    // What 3 asks while answering belongs to the request it answers, and cannot follow another.
    rig.set_state(4, DetectedAgentState::Working);
    let delegated = rig.ask(Some(3), 4, "check the changelog");
    assert_eq!(delegated.conversation, first.conversation);
    let owing = rig.agent(3);
    let follows_while_owing = rig.server.ask_agent(
        Some(owing),
        AgentTarget::Pane(4),
        "more".into(),
        None,
        Some(&delegated.id),
        None,
    );
    assert_eq!(
        code(follows_while_owing),
        Some(ControlErrorCode::InvalidArgument)
    );
    // That delegated request is not something 3 and 4 discussed themselves.
    let other_pair = rig.ask(Some(4), 3, "status?");
    assert_eq!(
        other_pair.conversation.as_deref(),
        Some(other_pair.id.as_str())
    );

    // Askers outside the session all look alike, so only `follows` joins one of them.
    let outside = rig.ask(None, 3, "who are you?");
    assert_eq!(outside.conversation.as_deref(), Some(outside.id.as_str()));
    let follows = |rig: &mut Rig, to: PaneId| {
        rig.server.ask_agent(
            None,
            AgentTarget::Pane(to),
            "follow-up".into(),
            None,
            Some(&outside.id),
            Some("Claude \"Code\"\u{1b}[31m on\nfar-host\u{202e}\u{e0041}".into()),
        )
    };
    let follow_up = info(follows(&mut rig, 3));
    assert_eq!(follow_up.conversation.as_deref(), Some(outside.id.as_str()));
    assert_eq!(
        follow_up.from_label.as_deref(),
        Some("Claude 'Code' [31m on far-host")
    );
    assert_eq!(
        code(follows(&mut rig, 4)),
        Some(ControlErrorCode::InvalidArgument)
    );
    // Only whoever asked a request continues its conversation.
    let someone_elses = rig.server.ask_agent(
        None,
        AgentTarget::Pane(3),
        "me too".into(),
        None,
        Some(&first.id),
        None,
    );
    assert_eq!(code(someone_elses), Some(ControlErrorCode::InvalidArgument));
}

#[test]
fn a_new_native_conversation_ends_the_conversation_for_good() {
    let mut rig = Rig::new(&[2, 3]);
    let move_to = |rig: &mut Rig, native_session: &str| rig.move_to(3, native_session);
    let follows = |rig: &mut Rig, id: &str| {
        rig.server.ask_agent(
            None,
            AgentTarget::Pane(3),
            "and then?".into(),
            None,
            Some(id),
            None,
        )
    };
    // Asked before 3 named its native conversation, then again once it had.
    let first = rig.ask(Some(2), 3, "plan the release");
    let outside = rig.ask(None, 3, "who are you?");
    assert_eq!(first.to_native_session, None);
    assert_eq!(outside.to_native_session, None);
    move_to(&mut rig, "x");
    let second = rig.ask(Some(2), 3, "and the changelog?");
    assert_eq!(second.conversation, first.conversation);
    let followed = info(follows(&mut rig, &outside.id));
    assert_eq!(followed.conversation.as_deref(), Some(outside.id.as_str()));

    // A `/clear`: the unknown first exchange must not stand in for the conversation it left.
    move_to(&mut rig, "y");
    let after_clear = rig.ask(Some(2), 3, "start over");
    assert_eq!(
        after_clear.conversation.as_deref(),
        Some(after_clear.id.as_str())
    );
    // Following the request asked before "x" was known, not the later one held in "x".
    assert_eq!(
        code(follows(&mut rig, &outside.id)),
        Some(ControlErrorCode::InvalidArgument)
    );
}

#[test]
fn following_a_delegated_request_respects_a_new_native_conversation() {
    let mut rig = Rig::new(&[2, 3, 4]);
    rig.move_to(4, "x");
    // 3, answering 2, asks 4 on that request's behalf; then both finish.
    let request = rig.ask(Some(2), 3, "plan the release");
    let delegated = rig.ask(Some(3), 4, "check the changelog");
    assert_eq!(delegated.parent.as_deref(), Some(request.id.as_str()));
    assert!(rig.reply(4, &delegated.id, "fine").ok);
    assert!(rig.reply(3, &request.id, "done").ok);
    rig.run_turn(4);
    rig.run_turn(3);

    rig.move_to(4, "y");
    let asker = rig.agent(3);
    let follows = rig.server.ask_agent(
        Some(asker),
        AgentTarget::Pane(4),
        "and the release notes?".into(),
        None,
        Some(&delegated.id),
        None,
    );
    assert_eq!(code(follows), Some(ControlErrorCode::InvalidArgument));
}

#[test]
fn an_await_answers_once_and_never_holds_up_its_request() {
    let mut rig = Rig::new(&[2, 3]);
    let (client, mut stream) = add_client(&mut rig.server);
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let request = rig.ask(Some(2), 3, "plan the release");
    rig.set_state(2, DetectedAgentState::Working);
    let reply = ReplyTo::Headless {
        capabilities: protocol::Capabilities::current(),
        effective_protocol: PROTOCOL_VERSION,
        permission: crate::session::protocol::Permission::View,
    };
    assert!(
        rig.server
            .register_request_await(client, Some(rig.agent(2)), &request.id, None, reply.clone())
            .is_none()
    );

    // The agent being awaited asks the awaiting one: waiting on would deadlock both.
    let inbound = rig.ask(Some(3), 2, "which version?");
    assert!(!rig.server.holds_agent_wait(client));
    rig.server.flush_clients();
    let ServerMessage::SessionControlResult { response, .. } =
        protocol::read_frame(&mut stream).unwrap()
    else {
        panic!("a control result")
    };
    assert_eq!(response.code, Some(ControlErrorCode::Conflict));
    assert!(response.error.unwrap().contains(&inbound.id));

    // A script's await ends with its connection; the request carries on.
    assert!(
        rig.server
            .register_request_await(7, None, &request.id, Some(60_000), reply.clone())
            .is_none()
    );
    rig.server.remove_agent_waits(7);
    assert!(!rig.server.holds_agent_wait(7));
    assert_eq!(rig.request(&request.id).state, AgentRequestState::Active);

    // Once the inbound request is typed into the awaiting agent, it is no less of a cycle.
    rig.set_state(2, DetectedAgentState::Idle);
    assert_eq!(rig.request(&inbound.id).state, AgentRequestState::Active);
    let refused = rig
        .server
        .register_request_await(client, Some(rig.agent(2)), &request.id, None, reply)
        .unwrap();
    assert_eq!(code(refused), Some(ControlErrorCode::Conflict));
}

fn answered_requests_with_slots(count: usize) -> AgentRequestStore {
    let mut store = AgentRequestStore::default();
    for pane in 0..count {
        let to = protocol::AgentRef::for_test(pane as PaneId, 1, None);
        let id = format!("request-{pane}");
        store.requests.push(StoredRequest {
            info: AgentRequestInfo {
                id: id.clone(),
                root: id.clone(),
                parent: None,
                conversation: None,
                from: None,
                from_native_session: None,
                to: to.clone(),
                to_native_session: None,
                from_label: None,
                to_label: None,
                prompt: String::new(),
                state: AgentRequestState::Answered,
                reason: None,
                response: None,
                created_at_unix_ms: 0,
                updated_at_unix_ms: 0,
            },
            depth: 1,
            later: false,
        });
        store.slots.insert(
            to,
            DeliverySlot {
                request: id,
                tracker: AgentTurnTracker::new(true, Some(protocol::AgentState::Idle), false, None),
                delivered: Instant::now(),
                departed: true,
                waiting_after_screen_block: false,
                shown: true,
                attempts: 1,
                before: 0,
            },
        );
    }
    store
}

#[test]
fn answered_requests_hold_admission_capacity_until_their_delivery_turn_ends() {
    let mut store = answered_requests_with_slots(MAX_LIVE);
    let target = protocol::AgentRef::for_test(MAX_LIVE as PaneId, 1, None);
    store.prune();
    assert_eq!(
        code(store.admit(&target, None).unwrap_err()),
        Some(ControlErrorCode::Conflict)
    );

    // A retained answer stops consuming live capacity only after its delivery slot is released.
    let first = store.requests[0].info.to.clone();
    store.slots.remove(&first);
    assert!(store.admit(&target, None).is_ok());
    // Unresolved requests consume the same capacity even without a delivery slot.
    store.requests[0].info.state = AgentRequestState::Queued;
    assert_eq!(
        code(store.admit(&target, None).unwrap_err()),
        Some(ControlErrorCode::Conflict)
    );
}

#[test]
fn protected_answers_cannot_exceed_the_session_storage_budget() {
    let mut store = answered_requests_with_slots(16);
    let response = AgentResponse {
        response_id: 1,
        content: AgentResponsePayload {
            text: "x".repeat(MAX_RESPONSE_BYTES),
            turn_id: None,
            source: AgentResponseSource::AgentCli,
            fidelity: AgentResponseFidelity::Semantic,
            complete: true,
            truncated: false,
        },
    };
    for stored in &mut store.requests[..15] {
        stored.info.response = Some(response.clone());
    }
    store.requests[15].info.state = AgentRequestState::Active;
    store.requests[15].info.prompt = "p".repeat(512);
    let request = store.requests[15].info.clone();
    // Fifteen 1 MiB answers, sixteen metadata allowances, and the 512-byte prompt leave this room.
    assert!(store.admit_reply(&request, &"x".repeat(1_039_872)).is_ok());
    assert_eq!(
        code(
            store
                .admit_reply(&request, &"x".repeat(1_039_873))
                .unwrap_err()
        ),
        Some(ControlErrorCode::Conflict)
    );

    // An answer without a delivery slot is evictable and needs no protected capacity.
    let slot = store.slots.remove(&request.to).unwrap();
    assert!(store.admit_reply(&request, &response.content.text).is_ok());
    store.slots.insert(request.to.clone(), slot);
    // Retaining an old answer after releasing its slot frees protected capacity.
    store.slots.remove(&store.requests[0].info.to);
    assert!(store.admit_reply(&request, &response.content.text).is_ok());
}
