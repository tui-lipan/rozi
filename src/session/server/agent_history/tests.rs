use super::super::tests::{attach_client, catalog_from, decode_outbox_controls, test_pane};
use super::*;
use crate::control::{AgentResponseFidelity, AgentTarget, AgentWaitCondition, CaptureRender};
use crate::session::protocol::AttachmentOperation;
use std::os::unix::net::UnixStream;

// An external TUI boundary: real stdin, PTY mouse reports and redraws. All traversal, lease,
// lifecycle and reply behavior under test belongs to the production SessionServer.
#[test]
#[ignore = "child process fixture, launched by the history tests"]
fn tui_process() {
    use std::process::{Command, Stdio};
    assert!(std::env::var_os("ROZI_HISTORY_FIXTURE").is_some());
    assert!(
        Command::new("stty")
            .args(["raw", "-echo"])
            .stdin(Stdio::inherit())
            .status()
            .unwrap()
            .success()
    );
    let mode = std::env::var("ROZI_HISTORY_FIXTURE").unwrap();
    let mut top = if mode == "scrolled" { 4 } else { 6 };
    let draw = |top: usize| {
        let body = (top..top + 6)
            .map(|line| format!("line {line:02} 日本語"))
            .collect::<Vec<_>>()
            .join("\r\n");
        print!(
            "\x1b[?2026h\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[Hheader\x1b[K\r\n{body}\r\nfooter\x1b[K\x1b[?2026l"
        );
        std::io::stdout().flush().unwrap();
    };
    draw(top);
    let mut sequence = Vec::new();
    let mut input = std::io::stdin();
    let mut byte = [0];
    while input.read_exact(&mut byte).is_ok() {
        if byte[0] == 0x1b || !sequence.is_empty() {
            sequence.push(byte[0]);
            if byte[0] != b'M' {
                continue;
            }
            let up = sequence.starts_with(b"\x1b[<64;");
            sequence.clear();
            if up {
                top = top.saturating_sub(1);
            } else if mode != "restore-failure" {
                top = (top + 1).min(6);
            }
            draw(top);
        } else {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(std::env::var_os("ROZI_HISTORY_INPUT").unwrap())
                .unwrap()
                .write_all(&byte)
                .unwrap();
        }
    }
}

struct Rig {
    server: SessionServer,
    events: mpsc::Receiver<TerminalPtyEvent>,
    client: ClientId,
    _stream: UnixStream,
    reference: protocol::AgentRef,
    scratch: tempfile::TempDir,
    /// Run runtime detection each tick, as the production loop does. Off for rigs that set the
    /// agent state by hand, which detection would overwrite.
    poll_runtime: bool,
}

/// A definition that says nothing about history, so it reads history by default.
const CLAUDE: &str = r#"
[[agents]]
id = "claude"
match = { names = ["claude"] }
"#;

impl Rig {
    fn new(mode: &str) -> Self {
        Self::with_agents(mode, CLAUDE)
    }

    fn with_agents(mode: &str, agents: &str) -> Self {
        let scratch = tempfile::tempdir().unwrap();
        let (tx, events) = mpsc::channel();
        let pty = TerminalPty::spawn(
            TerminalPtyConfig::new(
                std::env::current_exe()
                    .unwrap()
                    .to_string_lossy()
                    .to_string(),
            )
            .arg("--exact")
            .arg("session::server::agent_history::tests::tui_process")
            .arg("--ignored")
            .arg("--nocapture")
            .env("ROZI_HISTORY_FIXTURE", mode)
            .env(
                "ROZI_HISTORY_INPUT",
                scratch.path().join("input").to_string_lossy().to_string(),
            )
            .size(30, 8),
            move |event| {
                let _ = tx.send(event);
            },
        )
        .unwrap();
        let mut server = SessionServer::new_named("history-test");
        server.settings.agents = catalog_from(agents);
        let mut pane = test_pane(7);
        pane.pty = Some(pty);
        pane.cols = 30;
        pane.rows = 8;
        pane.terminal.resize(8, 30);
        pane.runtime.detected_agent = Some(protocol::DetectedAgent {
            agent: protocol::AgentIdentity::new("claude", "Claude Code").into(),
            state: protocol::DetectedAgentState::Idle,
        });
        pane.agent.sync_references(&pane.runtime);
        server.panes.insert(3, pane);
        let reference = server
            .resolve_agent_wait_target(AgentTarget::Pane(3))
            .unwrap();
        let (client, stream) = attach_client(&mut server);
        let mut rig = Self {
            server,
            events,
            client,
            _stream: stream,
            reference,
            scratch,
            poll_runtime: false,
        };
        rig.until(|rig| rig.screen().contains("footer"));
        rig
    }

    fn screen(&mut self) -> String {
        let screen = self
            .server
            .panes
            .get_mut(&3)
            .unwrap()
            .screen_without_change();
        screen.export_text(0, screen.total_text_lines())
    }

    /// One production loop turn: drain output, poll runtime detection, then pump the reader.
    fn tick(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            if let Some(outbound) = self
                .server
                .handle_event(ServerEvent::Pty(None, 3, 7, event))
            {
                self.server.send_outbound(None, &outbound);
            }
        }
        if self.poll_runtime {
            self.server.last_runtime_poll = Instant::now() - Duration::from_secs(1);
            self.server.poll_pane_runtime();
        }
        self.server.pump_agent_history();
    }

    fn until(&mut self, predicate: impl Fn(&mut Self) -> bool) {
        let end = Instant::now() + Duration::from_secs(15);
        while !predicate(self) {
            assert!(Instant::now() < end, "timed out; screen: {}", self.screen());
            self.tick();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn begin(&mut self, attached: bool) {
        self.server
            .register_post_prompt_wait(
                self.client,
                self.reference.clone(),
                AgentWaitCondition::Turn,
                Some(12_000),
                Some(CaptureRender::Text),
                true,
                if attached {
                    ReplyTo::Attached { request_id: 42 }
                } else {
                    ReplyTo::Headless {
                        capabilities: protocol::Capabilities::default(),
                        effective_protocol: PROTOCOL_VERSION,
                    }
                },
            )
            .unwrap();
        self.state(protocol::DetectedAgentState::Working);
        self.state(protocol::DetectedAgentState::Idle);
        assert!(self.server.agent_waits.is_empty());
    }

    fn state(&mut self, state: protocol::DetectedAgentState) {
        self.server
            .panes
            .get_mut(&3)
            .unwrap()
            .runtime
            .detected_agent
            .as_mut()
            .unwrap()
            .state = state;
        self.server.resolve_agent_waits();
    }

    fn result(&self) -> serde_json::Value {
        decode_outbox_controls(self.server.client(self.client).unwrap())
            .into_iter()
            .find_map(|message| match message {
                ServerMessage::AttachedControlResult {
                    request_id: 42,
                    response,
                }
                | ServerMessage::SessionControlResult { response, .. } => {
                    Some(serde_json::to_value(response).unwrap())
                }
                _ => None,
            })
            .expect("completed wait reply")
    }

    fn finish(&mut self) -> serde_json::Value {
        self.until(|rig| rig.server.agent_history.is_empty());
        self.result()
    }
}

/// No reconstruction: the reply carries the screen as it was when the turn resolved.
fn assert_visible_fallback(result: &serde_json::Value, bottom: usize) {
    let response = &result["data"]["response"];
    assert_eq!(response["source"], "terminal", "{result}");
    assert_eq!(response["fidelity"], "rendered", "{result}");
    assert_eq!(response["complete"], false, "{result}");
    let text = response["text"].as_str().unwrap();
    for line in bottom..bottom + 6 {
        assert!(text.contains(&format!("line {line:02} 日本語")), "{text}");
    }
    assert!(!text.contains(&format!("line {:02}", bottom - 1)), "{text}");
    assert!(text.ends_with("footer"), "{text:?}");
}

#[test]
fn history_returns_full_rendered_text_and_restores_for_both_transports() {
    for attached in [false, true] {
        let mut rig = Rig::new("bottom");
        let original = rig.screen();
        rig.begin(attached);
        assert!(rig.server.holds_agent_history(rig.client));
        assert!(rig.server.holds_agent_wait(rig.client));
        let result = rig.finish();
        assert_eq!(rig.screen(), original);
        let response = &result["data"]["response"];
        assert_eq!(response["source"], "alternate-screen-history", "{result}");
        assert_eq!(response["fidelity"], "rendered");
        assert_eq!(response["complete"], true);
        let text = response["text"].as_str().unwrap();
        for line in 0..12 {
            assert!(text.contains(&format!("line {line:02} 日本語")), "{text}");
        }
        let capture = result["data"]["result"]["capture"].to_string();
        assert!(
            !capture.contains("line 00"),
            "capture must precede traversal: {capture}"
        );
        assert!(capture.contains("line 11"), "{result}");
    }
}

#[test]
fn scrolled_tui_and_failed_restoration_return_only_the_visible_screen() {
    for mode in ["scrolled", "restore-failure"] {
        let mut rig = Rig::new(mode);
        let original = rig.screen();
        rig.begin(true);
        let result = rig.finish();
        assert_visible_fallback(&result, if mode == "scrolled" { 4 } else { 6 });
        if mode == "scrolled" {
            assert_eq!(rig.screen(), original);
        }
    }
}

#[test]
fn marked_input_and_resize_wait_for_restoration_in_order() {
    let mut rig = Rig::new("bottom");
    let (other, _stream) = attach_client(&mut rig.server);
    rig.begin(true);
    rig.until(|rig| rig.screen().contains("line 04"));
    let original_size = (rig.server.panes[&3].cols, rig.server.panes[&3].rows);
    rig.server.process_client_control_now(
        other,
        ClientMessage::MarkedInput {
            pane_id: 3,
            local: false,
            generation: 7,
            bytes: b"x".to_vec(),
            token: 90,
        },
    );
    assert!(
        !decode_outbox_controls(rig.server.client(other).unwrap())
            .iter()
            .any(|message| matches!(message, ServerMessage::InputMarked { token: 90 }))
    );
    assert!(!rig.scratch.path().join("input").exists());
    let replies = rig.server.handle_resize_with(
        rig.client,
        PaneResizeRequest {
            pane_id: 3,
            local: false,
            generation: 7,
            cols: 40,
            rows: 10,
            cell_width: 10,
            cell_height: 20,
        },
        |_| panic!("resize during restoration"),
    );
    assert!(replies.is_empty());
    assert_eq!(
        (rig.server.panes[&3].cols, rig.server.panes[&3].rows),
        original_size
    );
    let result = rig.finish();
    assert_visible_fallback(&result, 6);
    assert_eq!(
        (rig.server.panes[&3].cols, rig.server.panes[&3].rows),
        (40, 10)
    );
    rig.until(|rig| {
        std::fs::read(rig.scratch.path().join("input")).is_ok_and(|bytes| bytes == b"x")
    });
    assert!(
        decode_outbox_controls(rig.server.client(other).unwrap())
            .iter()
            .any(|message| matches!(message, ServerMessage::InputMarked { token: 90 }))
    );
}

#[test]
fn semantic_response_arriving_during_traversal_wins() {
    let mut rig = Rig::new("bottom");
    let original = rig.screen();
    rig.begin(true);
    rig.until(|rig| rig.screen().contains("line 04"));
    rig.server.agent_responses.insert(
        rig.reference.clone(),
        None,
        AgentResponsePayload {
            text: "exact reply".into(),
            turn_id: Some("turn".into()),
            source: AgentResponseSource::NativeApi,
            fidelity: AgentResponseFidelity::Semantic,
            complete: true,
            truncated: false,
        },
    );
    let result = rig.finish();
    assert_eq!(rig.screen(), original);
    assert_eq!(result["data"]["response"]["text"], "exact reply");
    assert_eq!(result["data"]["response"]["fidelity"], "semantic");
}

#[test]
fn normal_history_is_rendered_bounded_and_never_claimed_complete() {
    let mut rig = Rig::new("bottom");
    rig.server
        .panes
        .get_mut(&3)
        .unwrap()
        .screen_mut()
        .process_bytes(b"\x1b[?1049l\x1b[2J\x1b[Hprevious\r\nanswer");
    rig.begin(true);
    let result = rig.finish();
    assert_eq!(result["data"]["response"]["source"], "terminal");
    assert_eq!(result["data"]["response"]["complete"], false);
    assert!(
        result["data"]["response"]["text"]
            .as_str()
            .unwrap()
            .contains("previous")
    );
    let payload = rendered_payload(
        "🌷".repeat(MAX_RESPONSE_BYTES),
        AgentResponseSource::Terminal,
        true,
    );
    assert!(payload.text.len() <= MAX_RESPONSE_BYTES);
    assert!(payload.truncated);
    assert!(!payload.complete);
    let history = format!("oldest{}newest", "🌷".repeat(MAX_RESPONSE_BYTES / 4));
    let payload = rendered_payload(history, AgentResponseSource::AlternateScreenHistory, true);
    assert!(payload.text.len() <= MAX_RESPONSE_BYTES);
    assert!(payload.text.ends_with("newest") && !payload.text.starts_with("oldest"));
    assert!(payload.truncated);
    assert!(!payload.complete);
}

#[test]
fn eligibility_refuses_mouse_disabled_and_missing_pty() {
    for mouse_disabled in [false, true] {
        let mut rig = Rig::new("bottom");
        let pane = rig.server.panes.get_mut(&3).unwrap();
        if mouse_disabled {
            pane.screen_mut().process_bytes(b"\x1b[?1000l");
        } else {
            pane.pty = None;
        }
        rig.begin(true);
        assert!(rig.server.agent_history.is_empty());
        assert_visible_fallback(&rig.result(), 6);
    }
}

#[test]
fn timeout_and_agent_resuming_restore_without_reconstructing() {
    for timeout in [false, true] {
        let mut rig = Rig::new("bottom");
        let original = rig.screen();
        rig.begin(true);
        rig.until(|rig| rig.screen().contains("line 04"));
        if timeout {
            rig.server.agent_history.get_mut(&3).unwrap().wait.deadline = Some(Instant::now());
        } else {
            rig.state(protocol::DetectedAgentState::Working);
        }
        let result = rig.finish();
        assert_eq!(rig.screen(), original);
        if timeout {
            assert_eq!(result["code"], "timeout", "{result}");
            assert!(result["data"]["response"].is_null(), "{result}");
        } else {
            assert_visible_fallback(&result, 6);
        }
    }
}

#[test]
fn drag_and_drop_waits_for_history_restoration() {
    let mut rig = Rig::new("bottom");
    rig.server
        .panes
        .get_mut(&3)
        .unwrap()
        .screen_mut()
        .set_drag_and_drop_enabled(true);
    rig.begin(true);
    let mut codes = Vec::new();
    tui_lipan::dnd::TerminalEvent::Drop {
        position: Default::default(),
        operations: tui_lipan::dnd::DropOperations::COPY,
        mimes: vec!["text/plain".into()],
    }
    .encode(Some(9), &mut codes);
    rig.server.handle_message(
        rig.client,
        ClientMessage::DragAndDrop {
            pane_id: 3,
            local: false,
            generation: 7,
            escape_codes: String::from_utf8(codes).unwrap(),
        },
    );
    assert!(rig.server.agent_history[&3].cancelled);
    assert!(matches!(
        &rig.server.agent_history[&3].mutations[..],
        [DeferredMutation::Control(_, message)] if matches!(message.as_ref(), ClientMessage::DragAndDrop { .. })
    ));
    rig.finish();
    rig.server.handle_event(ServerEvent::Pty(
        None,
        3,
        7,
        TerminalPtyEvent::Output(
            b"\x1b]72;t=a:i=9;\x1b\\\x1b]72;t=r:x=1:i=9\x1b\\"
                .to_vec()
                .into(),
        ),
    ));
    assert!(decode_outbox_controls(rig.server.client(rig.client).unwrap()).iter().any(|message|
        matches!(message, ServerMessage::PaneDragAndDrop { escape_codes, .. } if escape_codes.contains("t=r"))
    ), "the replayed drop owns the program's data request");
}

#[test]
fn revoked_drop_cleanup_waits_for_history_restoration() {
    let mut rig = Rig::new("bottom");
    rig.server
        .panes
        .get_mut(&3)
        .unwrap()
        .screen_mut()
        .set_drag_and_drop_enabled(true);
    let mut codes = Vec::new();
    tui_lipan::dnd::TerminalEvent::Drop {
        position: Default::default(),
        operations: tui_lipan::dnd::DropOperations::COPY,
        mimes: vec!["text/plain".into()],
    }
    .encode(Some(9), &mut codes);
    rig.server
        .accept_drag_and_drop(rig.client, 3, false, 7, String::from_utf8(codes).unwrap());
    rig.begin(true);
    let (second, _stream) = attach_client(&mut rig.server);
    rig.server
        .handle_message(rig.client, ClientMessage::SetInputLock { locked: true });
    rig.server
        .handle_message(second, ClientMessage::RequestControl);
    assert!(rig.server.agent_history[&3].cancelled);
    assert!(
        matches!(
            &rig.server.agent_history[&3].mutations[..],
            [DeferredMutation::DragReply { generation: 7, .. }]
        ),
        "server cleanup must wait behind restoration"
    );
    // A request already in flight when authority changed must also be refused after restoration.
    rig.server.handle_event(ServerEvent::Pty(
        None,
        3,
        7,
        TerminalPtyEvent::Output(b"\x1b]72;t=r:x=1:i=9\x1b\\".to_vec().into()),
    ));
    assert!(
        matches!(
            &rig.server.agent_history[&3].mutations[..],
            [
                DeferredMutation::DragReply { .. },
                DeferredMutation::DragReply { .. }
            ]
        ),
        "the unowned data-request refusal must also wait behind restoration"
    );
    rig.finish();
}

#[test]
fn unexpected_output_before_probe_never_scrolls() {
    let mut rig = Rig::new("bottom");
    rig.begin(true);
    rig.server.handle_event(ServerEvent::Pty(
        None,
        3,
        7,
        TerminalPtyEvent::Output(b"\x1b[Hchanged".to_vec().into()),
    ));
    assert!(rig.server.agent_history[&3].cancelled);
    let result = rig.finish();
    assert_visible_fallback(&result, 6);
}

#[test]
fn replacement_and_disconnected_requester_release_the_lease() {
    for replaced in [false, true] {
        let mut rig = Rig::new("bottom");
        let original = rig.screen();
        rig.begin(true);
        rig.until(|rig| rig.screen().contains("line 04"));
        if replaced {
            rig.server.panes.get_mut(&3).unwrap().generation = 8;
        } else {
            rig.server.remove_agent_waits(rig.client);
        }
        let result = rig.finish();
        if replaced {
            assert_eq!(result["ok"], false);
            assert!(result["data"]["response"].is_null(), "{result}");
            assert!(
                rig.server
                    .agent_responses
                    .latest(&rig.reference, &None)
                    .is_none()
            );
        } else {
            assert_eq!(rig.screen(), original);
        }
        assert!(!rig.server.holds_agent_wait(rig.client));
    }
}

#[test]
fn lease_rejects_headless_input_and_ignores_unchanged_geometry() {
    let mut rig = Rig::new("bottom");
    rig.begin(true);
    let replies = rig.server.handle_resize_with(
        rig.client,
        PaneResizeRequest {
            pane_id: 3,
            local: false,
            generation: 7,
            cols: 30,
            rows: 8,
            cell_width: 10,
            cell_height: 20,
        },
        |_| Ok(()),
    );
    assert!(replies.is_empty());
    assert!(!rig.server.agent_history[&3].cancelled);
    assert!(rig.server.has_conflicting_prompt(&rig.reference, false));
    let messages = rig.server.handle_session_control(
        999,
        "history-test".into(),
        PROTOCOL_VERSION,
        PROTOCOL_VERSION,
        None,
        crate::control::ControlRequest {
            command: crate::control::ControlCommand::SendText {
                target: Some(3),
                text: "bad".into(),
                wait: None,
                capture: None,
                scale: None,
            },
            source_pane: None,
            source_session: None,
            caller_context: None,
            extension: None,
            credential: None,
        },
    );
    assert!(messages.iter().any(|(_, message)| matches!(message,
        ServerMessage::SessionControlResult { response, .. }
            if response.code == Some(crate::control::ControlErrorCode::Conflict))));
    assert!(!rig.scratch.path().join("input").exists());
    rig.server.cancel_client_history(rig.client);
    rig.finish();
}

fn resize(cols: u16, rows: u16) -> PaneResizeRequest {
    PaneResizeRequest {
        pane_id: 3,
        local: false,
        generation: 7,
        cols,
        rows,
        cell_width: 10,
        cell_height: 20,
    }
}

#[test]
fn a_to_b_to_a_resizes_replay_in_order_and_end_at_a() {
    let mut rig = Rig::new("bottom");
    rig.begin(true);
    assert!(rig.server.agent_history.contains_key(&3));
    for request in [resize(40, 10), resize(30, 8)] {
        let replies = rig
            .server
            .handle_resize_with(rig.client, request, |_| panic!("resize during restoration"));
        assert!(replies.is_empty());
    }
    assert_eq!(rig.server.agent_history[&3].mutations.len(), 2);
    assert_eq!(
        (rig.server.panes[&3].cols, rig.server.panes[&3].rows),
        (30, 8)
    );
    rig.finish();
    assert_eq!(
        (rig.server.panes[&3].cols, rig.server.panes[&3].rows),
        (30, 8)
    );
    let resized = decode_outbox_controls(rig.server.client(rig.client).unwrap())
        .into_iter()
        .filter_map(|message| match message {
            ServerMessage::Resized { cols, rows, .. } => Some((cols, rows)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(resized, [(40, 10), (30, 8)]);
}

#[test]
fn pending_pty_resize_prevents_history_and_its_retry_runs_normally() {
    let mut rig = Rig::new("bottom");
    rig.server.panes.get_mut(&3).unwrap().pty_resize_pending = true;
    rig.begin(true);
    assert!(rig.server.agent_history.is_empty());
    assert_visible_fallback(&rig.result(), 6);
    let retried = std::cell::Cell::new(false);
    rig.server
        .handle_resize_with(rig.client, resize(30, 8), |_| {
            retried.set(true);
            Ok(())
        });
    assert!(retried.get());
    assert!(!rig.server.panes[&3].pty_resize_pending);
}

#[test]
fn history_page_matching_a_working_rule_never_changes_agent_state() {
    let agents = format!(
        "{CLAUDE}\n[[agents.states]]\nstate = \"working\"\nscreen = {{ any_of = [\"line 03\"] }}\n"
    );
    let mut rig = Rig::with_agents("bottom", &agents);
    rig.server
        .panes
        .get_mut(&3)
        .unwrap()
        .env
        .push(("ROZI_AGENT".into(), "claude".into()));
    // Settle detection on the bottom page, outside any startup grace, before traversing.
    rig.poll_runtime = true;
    for _ in 0..3 {
        rig.tick();
    }
    rig.server.panes.get_mut(&3).unwrap().agent.startup = None;
    rig.tick();
    // Detection named the agent for real, so the reference the rig resolved earlier is stale.
    rig.reference = rig
        .server
        .resolve_agent_wait_target(AgentTarget::Pane(3))
        .unwrap();
    let state = |rig: &Rig| {
        rig.server.panes[&3]
            .runtime
            .detected_agent
            .as_ref()
            .map(|agent| agent.state)
    };
    assert_ne!(state(&rig), Some(protocol::DetectedAgentState::Working));
    let completions = |rig: &Rig| {
        rig.server
            .agent_runtimes_for(3, &rig.server.panes[&3])
            .iter()
            .map(|agent| agent.completions)
            .sum::<u64>()
    };
    rig.begin(true);
    let before = (state(&rig), completions(&rig));
    rig.until(|rig| rig.screen().contains("line 03"));
    // Several polls over the historical page, as the production loop makes.
    for _ in 0..3 {
        rig.tick();
    }
    assert_eq!((state(&rig), completions(&rig)), before);
    let result = rig.finish();
    assert_eq!(
        result["data"]["response"]["source"], "alternate-screen-history",
        "{result}"
    );
    assert_eq!((state(&rig), completions(&rig)), before);
}

#[test]
fn opting_out_of_history_never_scrolls_and_returns_the_visible_screen() {
    let agents = format!("{CLAUDE}history = \"none\"\n");
    let mut rig = Rig::with_agents("bottom", &agents);
    let original = rig.screen();
    rig.begin(true);
    assert!(rig.server.agent_history.is_empty());
    assert_visible_fallback(&rig.result(), 6);
    assert_eq!(rig.screen(), original);
    assert!(!rig.scratch.path().join("input").exists());
}

#[test]
fn inline_image_in_a_history_page_still_restores_the_viewport() {
    let mut rig = Rig::new("bottom");
    let original = rig.screen();
    rig.begin(true);
    rig.until(|rig| rig.screen().contains("line 04"));
    // The older page now carries a terminal image: a Kitty graphics placement.
    rig.server
        .panes
        .get_mut(&3)
        .unwrap()
        .screen_mut()
        .process_bytes(b"\x1b_Ga=T,f=24,s=1,v=1,t=d;AAAA\x1b\\");
    assert!(
        !rig.server
            .panes
            .get_mut(&3)
            .unwrap()
            .screen_without_change()
            .render_snapshot()
            .images
            .is_empty()
    );
    rig.tick();
    assert!(
        rig.server.agent_history.contains_key(&3),
        "lease released without restoring"
    );
    assert!(rig.server.agent_history[&3].cancelled);
    let result = rig.finish();
    assert_visible_fallback(&result, 6);
    assert_eq!(rig.screen(), original);
}

fn stage_upload(rig: &mut Rig, transfer: u64) {
    use base64::{Engine, engine::general_purpose::STANDARD};
    for operation in [
        AttachmentOperation::Begin {
            pane_id: 3,
            generation: 7,
            local: false,
            name: "photo.png".into(),
            size: 4,
        },
        AttachmentOperation::Chunk {
            data: STANDARD.encode(b"data"),
        },
    ] {
        assert!(
            rig.server
                .attachment_step(rig.client, transfer, operation)
                .is_empty()
        );
        let end = Instant::now() + Duration::from_secs(5);
        while rig.server.attachments.is_busy((rig.client, transfer)) {
            assert!(Instant::now() < end);
            rig.server.poll_attachment_work();
            std::thread::yield_now();
        }
    }
}

fn attachment_error(rig: &Rig) -> Option<String> {
    decode_outbox_controls(rig.server.client(rig.client).unwrap())
        .into_iter()
        .find_map(|message| match message {
            ServerMessage::AttachmentAck { error, .. } => error,
            _ => None,
        })
}

#[test]
fn attachment_begin_and_completion_never_write_under_the_lease() {
    // Begin during traversal is refused.
    let mut rig = Rig::new("bottom");
    rig.begin(true);
    let replies = rig.server.attachment_step(
        rig.client,
        1,
        AttachmentOperation::Begin {
            pane_id: 3,
            generation: 7,
            local: false,
            name: "photo.png".into(),
            size: 4,
        },
    );
    assert!(matches!(
        replies.as_slice(),
        [(_, ServerMessage::AttachmentAck { error: Some(_), .. })]
    ));
    rig.finish();

    // An upload whose final step is already with the worker completes during traversal.
    let mut rig = Rig::new("bottom");
    stage_upload(&mut rig, 2);
    assert!(
        rig.server
            .attachment_step(rig.client, 2, AttachmentOperation::Finish)
            .is_empty()
    );
    rig.begin(true);
    assert!(rig.server.agent_history.contains_key(&3));
    let end = Instant::now() + Duration::from_secs(5);
    while rig.server.attachments.has_pending() {
        assert!(Instant::now() < end);
        rig.server.poll_attachment_work();
        std::thread::yield_now();
    }
    assert!(
        attachment_error(&rig).is_some(),
        "completion must be refused"
    );
    rig.finish();
    assert!(!rig.scratch.path().join("input").exists());
}

#[test]
fn an_explicit_reply_during_a_requests_history_read_wins_and_restores() {
    let mut rig = Rig::new("bottom");
    rig.server.agent_requests.quiet = Duration::ZERO;
    let original = rig.screen();
    let asked = rig
        .server
        .ask_agent(None, AgentTarget::Pane(3), "summarize".into(), None);
    let id = asked.data.unwrap()["id"].as_str().unwrap().to_owned();
    rig.state(protocol::DetectedAgentState::Working);
    rig.state(protocol::DetectedAgentState::Idle);
    assert!(rig.server.agent_history.contains_key(&3));
    rig.until(|rig| rig.screen().contains("line 04"));

    let reply = rig
        .server
        .reply_to_request(Some(rig.reference.clone()), &id, "explicit".into());
    assert!(reply.ok, "{reply:?}");
    rig.until(|rig| rig.server.agent_history.is_empty());
    assert_eq!(rig.screen(), original);
    let request = rig.server.agent_request(&id).data.unwrap();
    assert_eq!(request["state"], "answered", "{request}");
    assert_eq!(request["response"]["text"], "explicit", "{request}");
    assert!(
        rig.server
            .agent_requests
            .answering(&rig.reference)
            .is_none()
    );
}
