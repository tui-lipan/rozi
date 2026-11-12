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
    let latest = rig
        .server
        .agent_responses
        .latest(&rig.agent(3), &None)
        .unwrap();
    assert_eq!(latest.content.source, AgentResponseSource::AgentCli);
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
            .register_request_await(7, None, &request.id, Some(60_000), reply)
            .is_none()
    );
    rig.server.remove_agent_waits(7);
    assert!(!rig.server.holds_agent_wait(7));
    assert_eq!(rig.request(&request.id).state, AgentRequestState::Active);
}
