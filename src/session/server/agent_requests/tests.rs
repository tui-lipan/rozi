use super::super::tests::{add_client, test_pane};
use super::*;
use crate::session::protocol::DetectedAgentState;

/// A session with an agent in each of `panes`, every one running a real PTY that echoes back
/// whatever is typed into it.
struct Rig {
    server: SessionServer,
    typed: HashMap<PaneId, mpsc::Receiver<TerminalPtyEvent>>,
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
        Self { server, typed }
    }

    fn agent(&self, pane: PaneId) -> protocol::AgentRef {
        self.server
            .resolve_agent_wait_target(AgentTarget::Pane(pane))
            .unwrap()
    }

    fn set_state(&mut self, pane: PaneId, state: DetectedAgentState) {
        let pane = self.server.panes.get_mut(&pane).unwrap();
        pane.runtime.detected_agent.as_mut().unwrap().state = state;
        self.server.resolve_agent_waits();
    }

    fn ask(&mut self, from: Option<PaneId>, to: PaneId, text: &str) -> AgentRequestInfo {
        let from = from.map(|pane| self.agent(pane));
        info(
            self.server
                .ask_agent(from, AgentTarget::Pane(to), text.into()),
        )
    }

    fn try_ask(&mut self, from: PaneId, to: PaneId) -> ControlResponse {
        let from = self.agent(from);
        self.server
            .ask_agent(Some(from), AgentTarget::Pane(to), "another".into())
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
        if !follows {
            assert_eq!(
                rig.request(&request.id).state,
                AgentRequestState::Failed,
                "{scenario}"
            );
            continue;
        }
        let published = rig.agent(3);
        assert!(published.slot.is_some());
        assert_eq!(published.incarnation, detected.incarnation);
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

    rig.set_state(3, DetectedAgentState::Idle);
    assert_eq!(rig.request(&first.id).state, AgentRequestState::Active);
    assert_eq!(rig.request(&second.id).state, AgentRequestState::Queued);
    assert!(rig.typed_into(3, &format!("rozi agents reply {} --stdin", first.id)));

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
                from: None,
                from_native_session: None,
                to: to.clone(),
                to_native_session: None,
                prompt: String::new(),
                state: AgentRequestState::Answered,
                reason: None,
                response: None,
                created_at_unix_ms: 0,
                updated_at_unix_ms: 0,
            },
            depth: 1,
        });
        store.slots.insert(
            to,
            DeliverySlot {
                request: id,
                tracker: AgentTurnTracker::new(true, Some(protocol::AgentState::Idle), false, None),
                delivered: Instant::now(),
                departed: true,
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
