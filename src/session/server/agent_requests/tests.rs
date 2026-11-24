use super::super::tests::{add_client, attach_client, test_pane};
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
        // The fake agents never draw, so a held Enter goes out on the next pump.
        server.submit_max_delay = Duration::ZERO;
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
                blocker: None,
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
        info(self.server.ask_agent(
            from,
            AgentTarget::Pane(to),
            text.into(),
            AskOptions {
                parent: None,
                follows: None,
                asker_label: None,
                now: false,
                no_reply: false,
            },
        ))
    }

    fn ask_now(&mut self, to: PaneId, text: &str) -> ControlResponse {
        let now = AskOptions {
            now: true,
            ..AskOptions::default()
        };
        self.server
            .ask_agent(None, AgentTarget::Pane(to), text.into(), now)
    }

    /// Pane `pane`'s integration publishes `text` as the answer to a turn.
    fn publish(&mut self, pane: PaneId, text: &str) {
        let agent = self.agent(pane);
        self.server.agent_responses.insert(
            agent,
            None,
            AgentResponsePayload {
                text: text.into(),
                turn_id: None,
                source: AgentResponseSource::NativeHook,
                fidelity: AgentResponseFidelity::Semantic,
                complete: true,
                truncated: false,
            },
        );
    }

    fn try_ask(&mut self, from: PaneId, to: PaneId) -> ControlResponse {
        let from = self.agent(from);
        self.server.ask_agent(
            Some(from),
            AgentTarget::Pane(to),
            "another".into(),
            AskOptions {
                parent: None,
                follows: None,
                asker_label: None,
                now: false,
                no_reply: false,
            },
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
            let left = rig.request(&request.id);
            if scenario == "hook on another conversation" {
                // The agent still reports the conversation it took the request in, so the
                // request waits for that one to come back on screen.
                assert_eq!(left.state, AgentRequestState::Active, "{scenario}");
                assert_eq!(left.reason.as_deref(), Some(WAITING_FOR_RESUME));
                assert!(!rig.reply(3, &request.id, "5").ok, "{scenario}");
            } else {
                assert_eq!(left.state, AgentRequestState::Failed, "{scenario}");
            }
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
            if let Some((_, since)) = rig.server.agent_requests.screens.get_mut(&3) {
                *since = before_quiet;
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
    // Typing it again was the request's own input, not someone's draft.
    assert_eq!(
        info(rig.ask_now(3, "and the lexer")).state,
        AgentRequestState::Active
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
fn a_message_exits_the_queue_as_delivered_and_keeps_no_answer() {
    let mut rig = Rig::new(&[2, 3]);
    rig.set_state(3, DetectedAgentState::Working);
    let sender = rig.agent(2);
    let queued = info(rig.server.ask_agent(
        Some(sender),
        AgentTarget::Pane(3),
        "The base branch moved.".into(),
        AskOptions {
            parent: None,
            follows: None,
            asker_label: None,
            now: false,
            no_reply: true,
        },
    ));
    assert_eq!(queued.state, AgentRequestState::Queued);
    rig.set_state(3, DetectedAgentState::Idle);
    let delivered = rig.request(&queued.id);
    assert_eq!(delivered.state, AgentRequestState::Delivered);
    assert!(delivered.delivered_at_unix_ms.is_some());
    assert!(delivered.response.is_none());
    assert!(!rig.reply(3, &queued.id, "unsolicited answer").ok);
    rig.run_turn(3);
    assert_eq!(rig.request(&queued.id).state, AgentRequestState::Delivered);
    let protocol::ServerMessage::AgentRequestsChanged { requests, .. } =
        rig.server.agent_requests_message(false)
    else {
        panic!("expected requests");
    };
    assert!(
        requests.is_empty(),
        "old clients cannot decode the new state"
    );
}

#[test]
fn a_queued_message_names_its_sender_after_that_agent_leaves() {
    let mut rig = Rig::new(&[2, 3]);
    rig.set_state(3, DetectedAgentState::Working);
    let sender = rig.agent(2);
    let queued = info(rig.server.ask_agent(
        Some(sender.clone()),
        AgentTarget::Pane(3),
        "The base branch moved.".into(),
        AskOptions {
            parent: None,
            follows: None,
            asker_label: None,
            now: false,
            no_reply: true,
        },
    ));
    assert_eq!(queued.state, AgentRequestState::Queued);
    assert_eq!(queued.from_label.as_deref(), Some("Claude Code"));
    rig.server.panes.get_mut(&2).unwrap().runtime.detected_agent = None;
    assert!(rig.server.current_agent_runtime(&sender).is_err());
    rig.set_state(3, DetectedAgentState::Idle);
    assert_eq!(rig.request(&queued.id).state, AgentRequestState::Delivered);
    assert!(
        rig.typed_into(3, "Message from Claude Code in pane 2 (no reply needed)"),
        "a departed sender is still named from the request"
    );
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
    let waiting = rig.request(&second.id);
    assert_eq!(waiting.state, AgentRequestState::Queued);
    assert_eq!(
        waiting.reason,
        Some(format!(
            "waiting for the agent to finish request `{}`",
            first.id
        ))
    );
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
    rig.publish(3, "the lexer is fine");
    rig.server.pump_agent_requests();
    let answered = rig.request(&second.id);
    assert_eq!(answered.state, AgentRequestState::Answered);
    assert_eq!(answered.response.unwrap().content.text, "the lexer is fine");
}

#[test]
fn a_request_asked_now_is_typed_in_mid_turn_unless_that_is_unsafe() {
    let mut rig = Rig::new(&[3]);
    let ask_now = |rig: &mut Rig, text: &str| rig.ask_now(3, text);
    // A dialog, or a draft someone is typing, refuses it, and nothing is asked.
    rig.set_state(3, DetectedAgentState::Blocked);
    assert_eq!(
        code(ask_now(&mut rig, "review")),
        Some(ControlErrorCode::AgentBlocked)
    );
    rig.set_state(3, DetectedAgentState::Working);
    let quiet = Duration::from_millis(100);
    rig.server.agent_requests.quiet = quiet;
    rig.server
        .session_write_input(3, b"draft".to_vec())
        .unwrap();
    assert_eq!(
        code(ask_now(&mut rig, "review")),
        Some(ControlErrorCode::Conflict)
    );
    assert!(rig.server.agent_requests.requests.is_empty());
    std::thread::sleep(quiet);

    // Typed in while the agent works, and a second one joins the run the first is answered in:
    // a request's own typing is not someone else's draft.
    let first = info(ask_now(&mut rig, "review the parser"));
    assert_eq!(first.state, AgentRequestState::Active);
    assert_eq!(first.reason.as_deref(), Some(DELIVERED_MID_TURN));
    assert!(first.delivered_at_unix_ms.is_some());
    assert!(rig.typed_into(3, &format!("Request {}", first.id)));
    let second = info(ask_now(&mut rig, "and the lexer"));
    assert_eq!(second.state, AgentRequestState::Active);

    // The turn under way ends and the agent goes straight on to what was typed in, so nothing is
    // answered from the screen until it rests.
    rig.set_state(3, DetectedAgentState::Idle);
    rig.set_state(3, DetectedAgentState::Working);
    assert!(rig.reply(3, &first.id, "the parser is fine").ok);
    rig.server
        .panes
        .get_mut(&3)
        .unwrap()
        .screen_mut()
        .process_bytes(b"the lexer is fine");
    rig.set_state(3, DetectedAgentState::Idle);
    assert_eq!(rig.request(&second.id).state, AgentRequestState::Active);
    std::thread::sleep(quiet);
    rig.server.pump_agent_requests();
    let answered = rig.request(&second.id);
    assert_eq!(answered.state, AgentRequestState::Answered);
    assert!(
        answered
            .response
            .unwrap()
            .content
            .text
            .contains("the lexer is fine")
    );

    // Typed in within the quiet window a queued request waits out; cancelling it then says the
    // agent already has it.
    let third = info(ask_now(&mut rig, "and the docs"));
    assert_eq!(third.reason.as_deref(), Some(DELIVERED));
    let cancelled = info(rig.server.cancel_request(None, &third.id));
    assert!(
        cancelled
            .reason
            .unwrap()
            .contains("after it was typed into the agent")
    );
}

#[test]
fn a_submitted_agent_prompt_does_not_count_as_someone_typing() {
    let mut rig = Rig::new(&[3, 4]);
    rig.server.agent_requests.quiet = QUIET_WINDOW;
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
    let prompt_cmd = |pane: PaneId, text: &str, allow_working: bool| ControlCommand::AgentPrompt {
        target: AgentTarget::Pane(pane),
        prompt: text.into(),
        wait: None,
        timeout_ms: None,
        allow_working,
        capture: None,
        result: false,
    };

    // Repro: `agents prompt` submits text + Enter, agent starts working, then `ask --now`
    // within the quiet window must succeed instead of being refused as someone typing.
    assert!(control(&mut rig, prompt_cmd(3, "long task", false)).ok);
    rig.set_state(3, DetectedAgentState::Working);
    let pong = info(rig.ask_now(3, "PONG"));
    assert_eq!(pong.state, AgentRequestState::Active);
    assert!(rig.typed_into(3, "PONG"));

    // An attached UI forwarding `agents prompt` leaves no draft either.
    let (client, _stream) = attach_client(&mut rig.server);
    let attached_messages = rig.server.handle_attached_control(
        client,
        1,
        ControlRequest {
            command: prompt_cmd(3, "attached prompt", true),
            source_pane: None,
            source_session: None,
            extension: None,
            caller_context: None,
            credential: None,
        },
    );
    assert!(matches!(
        attached_messages.as_slice(),
        [(_, ServerMessage::AttachedControlResult { response, .. })] if response.ok
    ));
    let followup = info(rig.ask_now(3, "after attached prompt"));
    assert_eq!(followup.state, AgentRequestState::Active);

    // Real typing — keyboard input, `send-text`, and `send-keys` — still refuses `--now`.
    let generation = rig.server.panes[&3].generation;
    rig.server
        .accept_pane_input(client, 3, false, generation, b"draft", true);
    assert_eq!(
        code(rig.ask_now(3, "blocked by keyboard")),
        Some(ControlErrorCode::Conflict)
    );
    // Submitting a prompt clears any prior draft mark; subsequent `send-text` marks typing again.
    assert!(control(&mut rig, prompt_cmd(3, "submit over draft", true)).ok);
    assert_eq!(
        info(rig.ask_now(3, "after submit")).state,
        AgentRequestState::Active
    );
    assert!(
        control(
            &mut rig,
            ControlCommand::SendText {
                target: Some(3),
                text: "partial".into(),
                wait: None,
                capture: None,
                scale: None,
            }
        )
        .ok
    );
    assert_eq!(
        code(rig.ask_now(3, "blocked by send-text")),
        Some(ControlErrorCode::Conflict)
    );
    rig.set_state(4, DetectedAgentState::Working);
    assert!(
        control(
            &mut rig,
            ControlCommand::SendKeys {
                target: Some(4),
                keys: vec!["x".into()],
                literal: true,
                wait: None,
                capture: None,
                scale: None,
            }
        )
        .ok
    );
    assert_eq!(
        code(rig.ask_now(4, "blocked by send-keys")),
        Some(ControlErrorCode::Conflict)
    );

    // Ordinary queued delivery after an `agents prompt` is not delayed by a bogus `pane_input`
    // entry, while an idle pane that just received `agents prompt` resets its stopped timer so a
    // waiting queued request does not barge in before the agent reacts.
    rig.set_state(4, DetectedAgentState::Idle);
    assert!(control(&mut rig, prompt_cmd(4, "quick prompt", false)).ok);
    assert!(!rig.server.agent_requests.pane_input.contains_key(&4));
    let target4 = rig.agent(4);
    let queued = rig.ask(None, 4, "queued after prompt");
    assert_eq!(queued.state, AgentRequestState::Queued);
    let before_quiet = Instant::now() - QUIET_WINDOW;
    *rig.server
        .agent_requests
        .stopped_since
        .get_mut(&target4)
        .unwrap() = before_quiet;
    if let Some((_, since)) = rig.server.agent_requests.screens.get_mut(&4) {
        *since = before_quiet;
    }
    // Another `agents prompt` while idle clears `stopped_since`, so `queued` waits a fresh window.
    assert!(control(&mut rig, prompt_cmd(4, "second idle prompt", false)).ok);
    assert!(
        !rig.server
            .agent_requests
            .stopped_since
            .contains_key(&target4)
    );
    rig.server.pump_agent_requests();
    assert_eq!(rig.request(&queued.id).state, AgentRequestState::Queued);
    // Once the agent finishes and stays quiet, delivery proceeds without expiring `pane_input`.
    *rig.server
        .agent_requests
        .stopped_since
        .get_mut(&target4)
        .unwrap() = Instant::now() - QUIET_WINDOW;
    if let Some((_, since)) = rig.server.agent_requests.screens.get_mut(&4) {
        *since = Instant::now() - QUIET_WINDOW;
    }
    rig.server.pump_agent_requests();
    assert_eq!(rig.request(&queued.id).state, AgentRequestState::Active);
}

#[test]
fn requests_typed_into_one_run_keep_their_own_answers() {
    let mut rig = Rig::new(&[3]);
    let quiet = Duration::from_millis(100);
    rig.server.agent_requests.quiet = quiet;
    rig.set_state(3, DetectedAgentState::Working);
    let first = info(rig.ask_now(3, "review the parser"));
    let second = info(rig.ask_now(3, "start the benchmarks"));
    let third = info(rig.ask_now(3, "and the lexer"));

    // The agent answers each in a turn of its own, through its integration alone; the second
    // promises a later reply, and its turn's answer is still its own.
    let turn = |rig: &mut Rig, answer: &str| {
        rig.publish(3, answer);
        rig.set_state(3, DetectedAgentState::Idle);
        rig.set_state(3, DetectedAgentState::Working);
        rig.server.pump_agent_requests();
    };
    turn(&mut rig, "the parser is fine");
    let agent = rig.agent(3);
    assert!(rig.server.reply_later(Some(agent), &second.id).ok);
    turn(&mut rig, "started the benchmarks");
    rig.publish(3, "the lexer is fine");
    rig.set_state(3, DetectedAgentState::Idle);
    std::thread::sleep(quiet);
    rig.server.pump_agent_requests();
    let answer = |rig: &Rig, id: &str| {
        let request = rig.request(id);
        assert_eq!(request.state, AgentRequestState::Answered);
        request.response.unwrap().content.text
    };
    assert_eq!(answer(&rig, &first.id), "the parser is fine");
    assert_eq!(answer(&rig, &third.id), "the lexer is fine");
    let promised = rig.request(&second.id);
    assert_eq!(promised.state, AgentRequestState::Active);
    assert!(promised.response.is_none());

    // A run takes only so many requests typed in after its first.
    for index in 0..=MAX_RIDERS {
        let asked = info(rig.ask_now(3, &format!("question {index}")));
        assert_eq!(asked.state, AgentRequestState::Active);
    }
    assert_eq!(
        code(rig.ask_now(3, "one too many")),
        Some(ControlErrorCode::Conflict)
    );
}

#[test]
fn a_turn_is_answered_from_the_screen_only_once_the_screen_stops_changing() {
    let mut rig = Rig::new(&[3]);
    let request = rig.ask(None, 3, "summarize");
    let quiet = Duration::from_millis(100);
    rig.server.agent_requests.quiet = quiet;
    let write = |rig: &mut Rig, text: &[u8]| {
        rig.server
            .panes
            .get_mut(&3)
            .unwrap()
            .screen_mut()
            .process_bytes(text);
    };
    rig.set_state(3, DetectedAgentState::Working);
    write(&mut rig, b"the summary so far");
    // The agent reports the end of its turn while it is still writing its answer out.
    rig.set_state(3, DetectedAgentState::Idle);
    write(&mut rig, b" and the rest");
    rig.server.pump_agent_requests();
    assert_eq!(rig.request(&request.id).state, AgentRequestState::Active);
    std::thread::sleep(quiet);
    rig.server.pump_agent_requests();
    let answered = rig.request(&request.id);
    assert_eq!(answered.state, AgentRequestState::Answered);
    let text = answered.response.unwrap().content.text;
    assert!(text.ends_with("the rest"), "{text:?}");
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

    // A promised answer in no known conversation fails once its agent exits.
    let target = rig.agent(3);
    rig.server.reply_later(Some(target), &second.id);
    let pane = rig.server.panes.get_mut(&3).unwrap();
    pane.runtime.detected_agent = None;
    pane.agent.sync_references(&pane.runtime);
    rig.server.pump_agent_requests();
    let failed = rig.request(&second.id);
    assert_eq!(failed.state, AgentRequestState::Failed);
    assert_eq!(failed.reason.as_deref(), Some("the agent exited"));
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
            AskOptions {
                parent,
                follows: None,
                asker_label: None,
                now: false,
                no_reply: false,
            },
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
            .await_cycle(Some(&asker), &first.id)
            .map(|info| info.id.as_str()),
        Some(follow_up.id.as_str())
    );
}

/// An agent exits in its pane: its hooks, its screen detection, and the row a publisher lists it
/// under all go, the row before or after the agent. A request it owes, whether it promised to
/// reply later or not, waits for the agent to resume, and only the same conversation answers it.
#[test]
fn an_open_request_survives_an_agent_restart_only_in_its_original_conversation() {
    for deferred in [false, true] {
        for rows_first in [false, true] {
            for resumed_session in ["original", "different"] {
                let case = format!("deferred={deferred} rows_first={rows_first} {resumed_session}");
                let mut rig = Rig::new(&[3]);
                let (client, _stream) = super::super::tests::attach_client(&mut rig.server);
                let publish_row = |rig: &mut Rig, native_session: Option<&str>| {
                    let rows = native_session
                        .map_or_else(Vec::new, |session| vec![agent_row(session, true)]);
                    publish_rows(rig, client, rows);
                    rig.server.pump_agent_requests();
                };
                let set_agent = |rig: &mut Rig, detected: Option<protocol::DetectedAgent>| {
                    let pane = rig.server.panes.get_mut(&3).unwrap();
                    let before = pane.agent.agent_states(&pane.runtime);
                    pane.runtime.integration = None;
                    // An agent coming back is a new launch of it.
                    if detected.is_some() {
                        pane.runtime.agent_launch += 1;
                    }
                    pane.runtime.detected_agent = detected;
                    pane.agent.observe(before, &pane.runtime);
                    rig.server.pump_agent_requests();
                };
                rig.move_to(3, "original");
                publish_row(&mut rig, Some("original"));
                let first = rig.agent(3);
                let request = rig.ask(None, 3, "Wait for the long check and reply.");
                assert_eq!(request.state, AgentRequestState::Active, "{case}");
                assert!(rig.typed_into(3, "Wait for the long check and reply."));
                if deferred {
                    assert!(rig.server.reply_later(Some(first.clone()), &request.id).ok);
                }

                let detected = rig.server.panes[&3].runtime.detected_agent.clone();
                if rows_first {
                    rig.server.panes.get_mut(&3).unwrap().runtime.integration = None;
                    publish_row(&mut rig, None);
                }
                set_agent(&mut rig, None);
                publish_row(&mut rig, None);
                let waiting = rig.request(&request.id);
                assert_eq!(waiting.state, AgentRequestState::Active, "{case}");
                assert_eq!(
                    waiting.reason.as_deref(),
                    Some(WAITING_FOR_RESUME),
                    "{case}"
                );

                set_agent(&mut rig, detected);
                rig.move_to(3, resumed_session);
                publish_row(&mut rig, Some(resumed_session));
                assert_ne!(rig.agent(3), first);
                if resumed_session == "original" {
                    assert!(
                        !rig.server
                            .reply_to_request(Some(first), &request.id, "stale answer".into())
                            .ok
                    );
                    assert!(rig.reply(3, &request.id, "The check passed.").ok, "{case}");
                    assert_eq!(rig.request(&request.id).state, AgentRequestState::Answered);
                } else {
                    let failed = rig.request(&request.id);
                    assert_eq!(failed.state, AgentRequestState::Failed, "{case}");
                    assert_eq!(
                        failed.reason.as_deref(),
                        Some("the agent moved to another conversation"),
                        "{case}"
                    );
                    assert!(!rig.reply(3, &request.id, "wrong conversation").ok);
                }
            }
        }
    }
}

#[test]
fn replacing_the_pane_generation_fails_requests_even_for_the_same_conversation() {
    for deferred in [false, true] {
        let mut rig = Rig::new(&[3]);
        rig.move_to(3, "original");
        let target = rig.agent(3);
        let request = rig.ask(None, 3, "Wait for the check and reply.");
        assert_eq!(request.state, AgentRequestState::Active);
        if deferred {
            assert!(rig.server.reply_later(Some(target.clone()), &request.id).ok);
        }

        let pane = rig.server.panes.get_mut(&3).unwrap();
        pane.generation += 1;
        pane.runtime.integration = None;
        pane.agent.sync_references(&pane.runtime);
        rig.move_to(3, "original");
        let replacement = rig.agent(3);
        assert_ne!(replacement.pane.generation, target.pane.generation);

        rig.server.pump_agent_requests();
        let failed = rig.request(&request.id);
        assert_eq!(
            failed.state,
            AgentRequestState::Failed,
            "deferred={deferred}"
        );
        assert_eq!(failed.to, target);
        assert!(
            failed
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("pane 3 has been replaced")
        );
        assert!(
            !rig.reply(3, &request.id, "reply from the replacement pane")
                .ok
        );
    }
}

/// Within one launch, an agent that owes a promised answer can show another conversation and come
/// back. The request waits meanwhile and only the conversation it is bound to answers it.
#[test]
fn a_promised_answer_waits_while_its_agent_shows_another_conversation() {
    let mut rig = Rig::new(&[3]);
    let request = rig.ask(None, 3, "run the slow suite");
    assert!(rig.typed_into(
        3,
        &format!("rozi agents reply {} --format text --later", request.id)
    ));
    let target = rig.agent(3);
    info(rig.server.reply_later(Some(target.clone()), &request.id));

    // Delivered before the agent named its conversation, the request binds to the first one. Two
    // reports read in one server iteration still see it bound in between.
    rig.move_to(3, "conversation a");
    rig.move_to(3, "conversation b");
    rig.server.pump_agent_requests();
    let waiting = rig.request(&request.id);
    assert_eq!(waiting.state, AgentRequestState::Active);
    assert_eq!(waiting.to_native_session.as_deref(), Some("conversation a"));
    assert!(!rig.reply(3, &request.id, "From the wrong conversation.").ok);

    rig.move_to(3, "conversation a");
    rig.server.pump_agent_requests();
    assert!(rig.reply(3, &request.id, "The suite passed.").ok);
    assert_eq!(rig.request(&request.id).state, AgentRequestState::Answered);
}

/// A publisher and the agent's own report can each be first to say the agent switched
/// conversations. Switching away and back within one launch keeps a promised answer open.
#[test]
fn a_promised_answer_survives_a_switch_back_in_either_update_order() {
    for rows_first in [false, true] {
        let case = format!("rows_first={rows_first}");
        let mut rig = Rig::new(&[3]);
        let (client, _stream) = super::super::tests::attach_client(&mut rig.server);
        let switch = |rig: &mut Rig, to: &str, from: &str| {
            let rows = vec![agent_row(to, true), agent_row(from, false)];
            if rows_first {
                publish_rows(rig, client, rows);
                rig.server.pump_agent_requests();
                rig.move_to(3, to);
            } else {
                rig.move_to(3, to);
                rig.server.pump_agent_requests();
                publish_rows(rig, client, rows);
            }
            rig.server.pump_agent_requests();
        };
        rig.move_to(3, "a");
        publish_rows(
            &mut rig,
            client,
            vec![agent_row("a", true), agent_row("b", false)],
        );
        let request = rig.ask(None, 3, "Check and answer later.");
        assert_eq!(request.state, AgentRequestState::Active, "{case}");
        assert!(rig.server.reply_later(Some(rig.agent(3)), &request.id).ok);

        switch(&mut rig, "b", "a");
        assert_eq!(
            rig.request(&request.id).state,
            AgentRequestState::Active,
            "{case}"
        );
        switch(&mut rig, "a", "b");
        let open = rig.request(&request.id);
        assert_eq!(open.state, AgentRequestState::Active, "{case}: {open:?}");
        assert!(rig.reply(3, &request.id, "The check passed.").ok, "{case}");
        assert_eq!(
            rig.request(&request.id).state,
            AgentRequestState::Answered,
            "{case}"
        );
    }
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
        now: false,
        no_reply: false,
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
        AskOptions {
            parent: None,
            follows: Some(&delegated.id),
            asker_label: None,
            now: false,
            no_reply: false,
        },
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
            AskOptions {
                parent: None,
                follows: Some(&outside.id),
                asker_label: Some("Claude \"Code\"\u{1b}[31m on\nfar-host\u{202e}\u{e0041}".into()),
                now: false,
                no_reply: false,
            },
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
        AskOptions {
            parent: None,
            follows: Some(&first.id),
            asker_label: None,
            now: false,
            no_reply: false,
        },
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
            AskOptions {
                parent: None,
                follows: Some(id),
                asker_label: None,
                now: false,
                no_reply: false,
            },
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
        AskOptions {
            parent: None,
            follows: Some(&delegated.id),
            asker_label: None,
            now: false,
            no_reply: false,
        },
    );
    assert_eq!(code(follows), Some(ControlErrorCode::InvalidArgument));
}

#[test]
fn an_agent_task_changes_when_the_next_request_is_delivered() {
    let mut rig = Rig::new(&[2, 3]);
    let task = |rig: &Rig| {
        let agent = rig
            .server
            .agent_runtimes_for(3, &rig.server.panes[&3])
            .remove(0);
        (agent.task, agent.task_updated_at_unix_ms)
    };
    let first = rig.ask(Some(2), 3, "Plan the release\nCheck the changelog too.");
    assert_eq!(
        task(&rig),
        (Some("Plan the release".into()), first.delivered_at_unix_ms)
    );
    let second = rig.ask(Some(2), 3, "Review the version bump");
    assert_eq!(second.state, AgentRequestState::Queued);
    assert_eq!(task(&rig).0.as_deref(), Some("Plan the release"));

    assert!(rig.reply(3, &first.id, "planned").ok);
    rig.run_turn(3);
    let second = rig.request(&second.id);
    assert!(second.delivered_at_unix_ms.is_some());
    assert_eq!(
        task(&rig),
        (
            Some("Review the version bump".into()),
            second.delivered_at_unix_ms
        )
    );
}

#[test]
fn a_newer_published_row_title_supersedes_a_delivered_request_task() {
    let mut rig = Rig::new(&[2, 3]);
    let row = |title: &str, status: &str| protocol::PublishedRow {
        id: "conv-1".into(),
        publisher: "1/sessions".into(),
        kind: protocol::PublisherKind::Agent,
        title: title.into(),
        status: status.into(),
        reason: None,
        active: true,
        work_started_at: None,
        cwd: None,
        project: None,
        native_session: Some("session-x".into()),
        live_session: None,
        reported: false,
    };
    let task = |rig: &Rig| {
        let agent = rig
            .server
            .agent_runtimes_for(3, &rig.server.panes[&3])
            .remove(0);
        (agent.task, agent.task_updated_at_unix_ms)
    };
    super::super::runtime::publish_rows_for_test(
        rig.server.panes.get_mut(&3).unwrap(),
        vec![row("✳ Implement parser", "idle")],
    );
    assert_eq!(task(&rig).0.as_deref(), Some("✳ Implement parser"));

    let first = rig.ask(Some(2), 3, "Fix parser tests");
    assert_eq!(
        task(&rig),
        (Some("Fix parser tests".into()), first.delivered_at_unix_ms)
    );

    // Republishing the row with only status decoration changed keeps the delivered request label.
    super::super::runtime::replace_pane_rows(
        rig.server.panes.get_mut(&3).unwrap(),
        vec![row("⏺ Implement parser", "working")],
    );
    rig.server.pump_agent_requests();
    assert_eq!(
        task(&rig),
        (Some("Fix parser tests".into()), first.delivered_at_unix_ms)
    );

    // Updating the semantic row task reports a newer task from the publisher.
    assert!(rig.reply(3, &first.id, "done").ok);
    super::super::runtime::replace_pane_rows(
        rig.server.panes.get_mut(&3).unwrap(),
        vec![row("⏺ Review parser cleanup", "idle")],
    );
    rig.server.pump_agent_requests();
    let (label, updated_at) = task(&rig);
    assert_eq!(label.as_deref(), Some("⏺ Review parser cleanup"));
    assert!(updated_at >= first.delivered_at_unix_ms);

    // A subsequent delivered request supersedes the publisher title again.
    let second = rig.ask(Some(2), 3, "Verify edge cases");
    assert!(second.delivered_at_unix_ms.is_some());
    assert_eq!(
        task(&rig),
        (
            Some("Verify edge cases".into()),
            second.delivered_at_unix_ms
        )
    );
}

#[test]
fn withdrawing_a_published_row_carries_its_request_task_to_its_integration_claim() {
    let mut rig = Rig::new(&[2, 3]);
    let pane_ref = protocol::PaneRef {
        session_instance: rig.server.instance_id.clone(),
        pane_id: 3,
        generation: rig.server.panes[&3].generation,
    };
    let row = |title: &str| protocol::PublishedRow {
        id: "conv-1".into(),
        publisher: "1/sessions".into(),
        kind: protocol::PublisherKind::Agent,
        title: title.into(),
        status: "idle".into(),
        reason: None,
        active: true,
        work_started_at: None,
        cwd: None,
        project: None,
        native_session: Some("session-x".into()),
        live_session: None,
        reported: false,
    };
    {
        let pane = rig.server.panes.get_mut(&3).unwrap();
        pane.runtime.integration = Some(Box::new(protocol::AgentIntegrationReport {
            integration: "hook-x".into(),
            identity: protocol::AgentIdentity::new("claude", "Claude Code"),
            reference: protocol::AgentRef {
                pane: pane_ref.clone(),
                slot: None,
                incarnation: 50,
            },
            state: protocol::AgentState::Idle,
            reason: None,
            native_session: Some("session-x".into()),
            unless_quiet: None,
            seq: 1,
            reported_at_unix_ms: 10,
        }));
        super::super::runtime::publish_rows_for_test(pane, vec![row("Implement parser")]);
    }

    let request = rig.ask(Some(2), 3, "Fix parser tests");
    super::super::runtime::replace_pane_rows(rig.server.panes.get_mut(&3).unwrap(), Vec::new());

    let agent = rig
        .server
        .agent_runtimes_for(3, &rig.server.panes[&3])
        .remove(0);
    assert_eq!(agent.reference.slot, None);
    assert_eq!(agent.task.as_deref(), Some("Fix parser tests"));
    assert_eq!(agent.task_updated_at_unix_ms, request.delivered_at_unix_ms);

    // If a row later updates its title after the claim already held a request task, withdrawing
    // the row must not resurrect that older request task.
    {
        let pane = rig.server.panes.get_mut(&3).unwrap();
        super::super::runtime::replace_pane_rows(pane, vec![row("Implement parser")]);
        super::super::runtime::replace_pane_rows(pane, vec![row("Review parser cleanup")]);
        super::super::runtime::replace_pane_rows(pane, Vec::new());
    }
    let agent = rig
        .server
        .agent_runtimes_for(3, &rig.server.panes[&3])
        .remove(0);
    assert_eq!(agent.reference.slot, None);
    assert_eq!(agent.task, None);
    assert_eq!(agent.task_updated_at_unix_ms, None);
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
    let incoming = &response.data.unwrap()["incoming"];
    assert_eq!(incoming["id"], inbound.id);
    assert_eq!(
        incoming["from"],
        serde_json::to_value(rig.agent(3)).unwrap()
    );
    assert_eq!(incoming["prompt"], "which version?");

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

#[test]
fn an_await_ends_at_a_prompt_that_stops_its_undelivered_request() {
    let mut rig = Rig::new(&[3]);
    let detected = rig.server.panes.get_mut(&3).unwrap();
    let detected = detected.runtime.detected_agent.as_mut().unwrap();
    detected.blocker = Some(
        protocol::AgentBlocker {
            kind: "trust".into(),
            reset: None,
        }
        .into(),
    );
    rig.set_state(3, DetectedAgentState::Blocked);
    let request = rig.ask(None, 3, "plan the release");
    let reply = ReplyTo::Headless {
        capabilities: protocol::Capabilities::current(),
        effective_protocol: PROTOCOL_VERSION,
        permission: crate::session::protocol::Permission::View,
    };
    let refused = rig
        .server
        .register_request_await(7, None, &request.id, Some(60_000), reply.clone())
        .unwrap();
    assert_eq!(code(refused), Some(ControlErrorCode::AgentBlocked));

    // It stays queued, and is delivered once the prompt is answered.
    assert_eq!(rig.request(&request.id).state, AgentRequestState::Queued);
    rig.set_state(3, DetectedAgentState::Idle);
    assert_eq!(rig.request(&request.id).state, AgentRequestState::Active);
    assert!(
        rig.server
            .register_request_await(7, None, &request.id, Some(60_000), reply)
            .is_none()
    );
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
                delivered_at_unix_ms: Some(0),
                no_reply: false,
            },
            target_agent: "claude".into(),
            resumable_session: None,
            launch: 0,
            depth: 1,
            later: false,
        });
        store.slots.insert(
            to,
            DeliverySlot {
                request: id,
                riders: Vec::new(),
                mid_turn: false,
                answers: Vec::new(),
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

/// An agent row a publisher lists for conversation `id`.
fn agent_row(id: &str, active: bool) -> protocol::PublishedRow {
    protocol::PublishedRow {
        publisher: String::new(),
        kind: protocol::PublisherKind::Agent,
        id: id.into(),
        title: id.into(),
        status: "idle".into(),
        active,
        native_session: Some(id.into()),
        live_session: None,
        reported: false,
        reason: None,
        work_started_at: None,
        cwd: None,
        project: None,
    }
}

fn publish_rows(rig: &mut Rig, client: ClientId, rows: Vec<protocol::PublishedRow>) {
    rig.server
        .report_pane_rows(
            client,
            3,
            1,
            false,
            super::super::runtime::PanePublisher {
                id: "sessions".into(),
                kind: protocol::PublisherKind::Agent,
                epoch: 1,
                responses: Vec::new(),
            },
            rows,
        )
        .unwrap();
}

/// Another kind of agent taking the pane over ends a wait for a resume, even with no conversation on
/// screen.
#[test]
fn another_agent_in_the_pane_fails_a_request_waiting_for_a_resume() {
    let mut rig = Rig::new(&[3]);
    let (client, _stream) = super::super::tests::attach_client(&mut rig.server);
    rig.move_to(3, "original");
    let request = rig.ask(None, 3, "Check and answer later.");
    assert_eq!(request.state, AgentRequestState::Active);
    assert!(rig.server.reply_later(Some(rig.agent(3)), &request.id).ok);
    let pane = rig.server.panes.get_mut(&3).unwrap();
    pane.runtime.integration = None;
    pane.runtime.detected_agent = None;
    pane.agent.sync_references(&pane.runtime);
    rig.server.pump_agent_requests();
    assert_eq!(
        rig.request(&request.id).reason.as_deref(),
        Some(WAITING_FOR_RESUME)
    );

    let pane = rig.server.panes.get_mut(&3).unwrap();
    pane.runtime.agent_launch += 1;
    pane.runtime.detected_agent = Some(protocol::DetectedAgent {
        agent: protocol::AgentIdentity::new("codex", "Codex").into(),
        state: DetectedAgentState::Idle,
        blocker: None,
    });
    publish_rows(
        &mut rig,
        client,
        vec![agent_row("background-thread", false)],
    );
    assert_eq!(rig.request(&request.id).state, AgentRequestState::Failed);
}

/// A row the publisher still lists after its agent exits does not speak for the agent launched
/// next, which reports another conversation.
#[test]
fn a_row_left_from_before_an_exit_cannot_answer_for_a_new_conversation() {
    let mut rig = Rig::new(&[3]);
    let (client, _stream) = super::super::tests::attach_client(&mut rig.server);
    rig.move_to(3, "original");
    publish_rows(&mut rig, client, vec![agent_row("original", true)]);
    let request = rig.ask(None, 3, "Check and answer later.");
    assert_eq!(request.state, AgentRequestState::Active);
    assert!(rig.server.reply_later(Some(rig.agent(3)), &request.id).ok);

    let pane = rig.server.panes.get_mut(&3).unwrap();
    let detected = pane.runtime.detected_agent.take().unwrap();
    pane.runtime.integration = None;
    pane.agent.sync_references(&pane.runtime);
    rig.server.pump_agent_requests();

    let pane = rig.server.panes.get_mut(&3).unwrap();
    pane.runtime.agent_launch += 1;
    pane.runtime.detected_agent = Some(detected);
    pane.agent.sync_references(&pane.runtime);
    rig.move_to(3, "different");
    assert!(
        !rig.reply(3, &request.id, "Answer from the wrong conversation.")
            .ok
    );
}

/// A conversation sent to the background and then dropped from the list is still running, so the
/// conversation on screen does not decide its request.
#[test]
fn a_request_to_a_background_row_waits_while_its_agent_runs() {
    let mut rig = Rig::new(&[3]);
    let (client, _stream) = super::super::tests::attach_client(&mut rig.server);
    rig.move_to(3, "original");
    publish_rows(
        &mut rig,
        client,
        vec![agent_row("original", true), agent_row("other", false)],
    );
    let request = rig.ask(None, 3, "Check and answer later.");
    assert_eq!(request.state, AgentRequestState::Active);
    assert!(rig.server.reply_later(Some(rig.agent(3)), &request.id).ok);
    publish_rows(
        &mut rig,
        client,
        vec![agent_row("original", false), agent_row("other", true)],
    );
    assert_eq!(rig.request(&request.id).state, AgentRequestState::Active);
    publish_rows(&mut rig, client, vec![agent_row("other", true)]);
    assert_eq!(rig.request(&request.id).state, AgentRequestState::Active);
}

/// A launch can replace the process behind a published row without the row changing. Neither a
/// reply in its turn nor a promised one is taken from the new agent while it reports another
/// conversation.
#[test]
fn a_row_kept_across_a_relaunch_cannot_answer_for_another_conversation() {
    let accepted = [false, true].map(|deferred| {
        let mut rig = Rig::new(&[3]);
        let (client, _stream) = super::super::tests::attach_client(&mut rig.server);
        rig.move_to(3, "original");
        publish_rows(&mut rig, client, vec![agent_row("original", true)]);
        let request = rig.ask(None, 3, "Check and answer.");
        assert_eq!(request.state, AgentRequestState::Active);
        let original_ref = rig.agent(3);
        if deferred {
            assert!(
                rig.server
                    .reply_later(Some(original_ref.clone()), &request.id)
                    .ok
            );
        }

        // A real launch can replace the process behind a published row without an intermediate
        // sample of the shell. The published reference is intentionally retained.
        let pane = rig.server.panes.get_mut(&3).unwrap();
        pane.runtime.agent_launch += 1;
        pane.runtime.integration = None;
        pane.agent
            .row_resume
            .mark_stale(pane.agent.publisher_epochs.clone());
        pane.agent.sync_references(&pane.runtime);
        rig.move_to(3, "different");
        assert_eq!(rig.agent(3), original_ref);
        rig.reply(3, &request.id, "Reply from the wrong conversation.")
            .ok
    });
    assert_eq!(accepted, [false, false], "ordinary and deferred replies");
}

/// A request queued before a launch belongs to the launch that took it, so losing its row while
/// that launch runs is not a resume.
#[test]
fn a_request_belongs_to_the_launch_it_was_delivered_to() {
    let mut rig = Rig::new(&[3]);
    let (client, _stream) = super::super::tests::attach_client(&mut rig.server);
    rig.move_to(3, "original");
    publish_rows(
        &mut rig,
        client,
        vec![agent_row("original", true), agent_row("other", false)],
    );
    rig.server.agent_requests.quiet = Duration::from_secs(60);
    let request = rig.ask(None, 3, "Queue until the process is ready.");
    assert_eq!(request.state, AgentRequestState::Queued);

    let pane = rig.server.panes.get_mut(&3).unwrap();
    pane.runtime.agent_launch += 1;
    pane.agent.sync_references(&pane.runtime);
    rig.server.agent_requests.quiet = Duration::ZERO;
    rig.server.pump_agent_requests();
    assert_eq!(rig.request(&request.id).state, AgentRequestState::Active);
    assert!(rig.server.reply_later(Some(rig.agent(3)), &request.id).ok);

    publish_rows(
        &mut rig,
        client,
        vec![agent_row("original", false), agent_row("other", true)],
    );
    publish_rows(&mut rig, client, vec![agent_row("other", true)]);
    assert_eq!(rig.request(&request.id).state, AgentRequestState::Active);
}

/// A fresh request delivered to an agent running behind a stale or contradicted published row
/// must fence on `answering_session`, not the stale row's raw `response_native_session`.
#[test]
fn a_new_request_behind_a_stale_or_contradicted_row_does_not_inherit_the_old_session() {
    for contradicted in [false, true] {
        let mut rig = Rig::new(&[3]);
        let (client, _stream) = super::super::tests::attach_client(&mut rig.server);
        rig.move_to(3, "original");
        publish_rows(&mut rig, client, vec![agent_row("original", true)]);

        let pane = rig.server.panes.get_mut(&3).unwrap();
        pane.runtime.agent_launch += 1;
        pane.runtime.integration = None;
        pane.agent
            .row_resume
            .mark_stale(pane.agent.publisher_epochs.clone());
        pane.agent.sync_references(&pane.runtime);
        if contradicted {
            rig.move_to(3, "different");
        }

        let request = rig.ask(None, 3, "Fresh request after relaunch.");
        rig.server.pump_agent_requests();
        let after = rig.request(&request.id);
        assert_eq!(
            after.state,
            AgentRequestState::Active,
            "contradicted={contradicted}: {after:?}"
        );
        assert_eq!(after.to_native_session, None);
        assert!(rig.reply(3, &request.id, "Answered by the new agent.").ok);
        assert_eq!(rig.request(&request.id).state, AgentRequestState::Answered);
    }
}

/// A row that names no conversation holds its requests to none, even while the agent reports one,
/// so the answer its turn publishes still answers the request.
#[test]
fn a_row_without_a_conversation_is_answered_by_its_turn() {
    let mut rig = Rig::new(&[3]);
    let (client, _stream) = super::super::tests::attach_client(&mut rig.server);
    rig.move_to(3, "original");
    let row = |status: &str| protocol::PublishedRow {
        native_session: None,
        status: status.into(),
        ..agent_row("original", true)
    };
    publish_rows(&mut rig, client, vec![row("idle")]);
    let request = rig.ask(None, 3, "Check and answer.");
    assert!(rig.typed_into(3, "Check and answer."));
    publish_rows(&mut rig, client, vec![row("working")]);
    rig.server.pump_agent_requests();
    rig.publish(3, "Checked.");
    publish_rows(&mut rig, client, vec![row("idle")]);
    rig.server.pump_agent_requests();
    let answered = rig.request(&request.id);
    assert_eq!(answered.state, AgentRequestState::Answered, "{answered:?}");
    assert_eq!(answered.response.unwrap().content.text, "Checked.");
}
