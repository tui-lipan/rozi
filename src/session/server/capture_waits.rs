//! Pane waits on a session server: `capture-pane`, `send-text`, and `send-keys` holding their
//! reply until the pane's screen shows some text or settles.
//!
//! Like an agent wait, a pane wait is registered against the headless connection that asked for it
//! and answered from the pump, never by blocking it. Each iteration looks again at the panes whose
//! screen changed since the last look, then at every wait's clock.

use super::headless::{session_control_reply, session_control_unsupported};
use super::*;
use crate::control::{
    ControlCommand, ControlErrorCode, ControlRequest, ControlResponse, PaneCapture,
};
use crate::pane::capture_wait::{ScreenWait, WaitEnd, WaitPlan, WaitReply, WaitStatus};

pub(super) struct PendingCaptureWait {
    pane_id: PaneId,
    generation: u64,
    /// The pane's `content_generation` when the wait last looked at its screen.
    seen: u64,
    screen: ScreenWait,
    plan: WaitPlan,
    capabilities: protocol::Capabilities,
    effective_protocol: u32,
}

impl SessionServer {
    /// Start a pane wait for `request`, writing a send's input first. `Some` is the reply now: a
    /// refusal, or a wait that was already satisfied. `None` means the wait is registered and the
    /// reply comes from [`Self::resolve_capture_waits`].
    pub(super) fn register_capture_wait(
        &mut self,
        client_id: ClientId,
        request: ControlRequest,
        capabilities: protocol::Capabilities,
        effective_protocol: u32,
    ) -> Option<ControlResponse> {
        let plan = match WaitPlan::for_command(&request.command) {
            Ok(Some(plan)) => plan,
            Ok(None) => return Some(ControlResponse::error("the request holds no pane wait")),
            Err(response) => return Some(response),
        };
        if let Some(reason) = session_control_unsupported(&request.command) {
            return Some(ControlResponse::error_with(
                ControlErrorCode::Unsupported,
                reason,
            ));
        }
        let pane_id = match self.session_target_pane(plan.target) {
            Ok(id) => id,
            Err(response) => return Some(response),
        };
        let sent = match &request.command {
            ControlCommand::SendText { text, .. } => {
                self.session_write_input(pane_id, text.clone().into_bytes())
            }
            ControlCommand::SendKeys { keys, literal, .. } => self
                .session_key_bytes(pane_id, keys, *literal)
                .and_then(|bytes| self.session_write_input(pane_id, bytes)),
            _ => Ok(()),
        };
        if let Err(response) = sent {
            return Some(response);
        }
        let Some(pane) = self.panes.get_mut(&pane_id) else {
            return Some(ControlResponse::error_with(
                ControlErrorCode::PaneNotFound,
                format!("pane {pane_id} not found"),
            ));
        };
        let screen = ScreenWait::start(
            &plan.wait,
            pane.screen_without_change(),
            plan.after_input,
            Instant::now(),
        );
        if screen.status(Instant::now()) == WaitStatus::Ready {
            return Some(self.capture_wait_response(pane_id, &plan, WaitEnd::Ready));
        }
        let (generation, seen) = (pane.generation, pane.content_generation);
        self.capture_waits.insert(
            client_id,
            PendingCaptureWait {
                pane_id,
                generation,
                seen,
                screen,
                plan,
                capabilities,
                effective_protocol,
            },
        );
        None
    }

    /// Look again at every pane wait: at the screens that changed, and at every clock.
    ///
    /// A pane whose program has exited is still looked at first, since its last output can be the
    /// very text the wait was for; only a wait that is still unsatisfied after that fails.
    pub(super) fn resolve_capture_waits(&mut self) {
        if self.capture_waits.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut ended = Vec::new();
        for (&client_id, wait) in &mut self.capture_waits {
            let pane = self
                .panes
                .get_mut(&wait.pane_id)
                .filter(|pane| pane.generation == wait.generation);
            let Some(pane) = pane else {
                ended.push((client_id, WaitEnd::Closed));
                continue;
            };
            if pane.content_generation != wait.seen {
                wait.seen = pane.content_generation;
                wait.screen.observe(pane.screen_without_change(), now);
            }
            match wait.screen.status(now) {
                WaitStatus::Ready => ended.push((client_id, WaitEnd::Ready)),
                WaitStatus::TimedOut => ended.push((client_id, WaitEnd::TimedOut)),
                WaitStatus::Pending if pane.exited.is_some() => {
                    ended.push((client_id, WaitEnd::Exited));
                }
                WaitStatus::Pending => {}
            }
        }
        for (client_id, end) in ended {
            let Some(wait) = self.capture_waits.remove(&client_id) else {
                continue;
            };
            let response = self.capture_wait_response(wait.pane_id, &wait.plan, end);
            self.enqueue(
                client_id,
                Target::Sender,
                session_control_reply(wait.capabilities, wait.effective_protocol, response),
            );
            self.set_close_after_flush(client_id);
        }
    }

    fn capture_wait_response(
        &mut self,
        pane_id: PaneId,
        plan: &WaitPlan,
        end: WaitEnd,
    ) -> ControlResponse {
        plan.reply.respond(pane_id, &plan.wait, end, |reply| {
            self.wait_capture(pane_id, reply)
        })
    }

    fn wait_capture(
        &mut self,
        pane_id: PaneId,
        reply: &WaitReply,
    ) -> std::result::Result<PaneCapture, ControlResponse> {
        let pane = self.panes.get_mut(&pane_id).ok_or_else(|| {
            ControlResponse::error_with(
                ControlErrorCode::PaneNotFound,
                format!("pane {pane_id} not found"),
            )
        })?;
        let content = crate::pane::capture_screen(
            pane.screen_without_change(),
            reply.scrollback.clone(),
            reply.render,
            reply.scale,
        )?;
        Ok(PaneCapture {
            id: pane_id,
            title: pane.screen().title(),
            content,
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::control::{CaptureContent, CaptureRender, PaneWait};
    use crate::session::server::tests::{add_client, decode_outbox_controls, test_pane};

    fn server() -> SessionServer {
        let mut server = SessionServer::new_named("dev");
        server.panes.insert(3, test_pane(1));
        server
    }

    fn capture_wait(text: Option<&str>, settle_ms: Option<u64>, timeout_ms: u64) -> ControlRequest {
        ControlRequest {
            command: ControlCommand::CapturePane {
                target: Some(3),
                scrollback: None,
                render: CaptureRender::Text,
                scale: None,
                wait: Some(PaneWait {
                    text: text.map(str::to_string),
                    settle_ms,
                    timeout_ms,
                }),
            },
            source_pane: None,
            extension: None,
        }
    }

    fn ask(
        server: &mut SessionServer,
        client: ClientId,
        request: ControlRequest,
    ) -> Vec<ControlResponse> {
        server
            .handle_session_control(
                client,
                "dev".to_string(),
                PROTOCOL_VERSION,
                PROTOCOL_VERSION,
                None,
                request,
            )
            .into_iter()
            .filter_map(|(_, message)| match message {
                ServerMessage::SessionControlResult { response, .. } => Some(response),
                _ => None,
            })
            .collect()
    }

    /// The replies a resolved wait queued for `client`.
    fn answered(server: &SessionServer, client: ClientId) -> Vec<ControlResponse> {
        let conn = server
            .clients
            .iter()
            .find(|conn| conn.id == client)
            .unwrap();
        decode_outbox_controls(conn)
            .into_iter()
            .filter_map(|message| match message {
                ServerMessage::SessionControlResult { response, .. } => Some(response),
                _ => None,
            })
            .collect()
    }

    fn captured_text(response: &ControlResponse) -> String {
        let capture: PaneCapture =
            serde_json::from_value(response.data.clone().expect("a capture")).unwrap();
        match capture.content {
            CaptureContent::Text { text } => text,
            other => panic!("expected text, got {other:?}"),
        }
    }

    fn print(server: &mut SessionServer, bytes: &[u8]) {
        server
            .panes
            .get_mut(&3)
            .unwrap()
            .screen_mut()
            .process_bytes(bytes);
    }

    #[test]
    fn a_wait_holds_the_reply_until_the_text_is_drawn() {
        let mut server = server();
        let (client, _stream) = add_client(&mut server);
        assert!(ask(&mut server, client, capture_wait(Some("done"), None, 5_000)).is_empty());
        assert!(server.capture_waits.contains_key(&client));

        print(&mut server, b"working\r\n");
        server.resolve_capture_waits();
        assert!(answered(&server, client).is_empty());

        print(&mut server, b"done\r\n");
        server.resolve_capture_waits();
        let replies = answered(&server, client);
        assert_eq!(replies.len(), 1);
        assert!(replies[0].ok);
        assert!(captured_text(&replies[0]).contains("done"));
        assert!(server.capture_waits.is_empty());
    }

    #[test]
    fn a_wait_already_satisfied_answers_at_once() {
        let mut server = server();
        let (client, _stream) = add_client(&mut server);
        print(&mut server, b"ready");
        let replies = ask(
            &mut server,
            client,
            capture_wait(Some("ready"), None, 5_000),
        );
        assert_eq!(replies.len(), 1);
        assert!(replies[0].ok);
        assert!(server.capture_waits.is_empty());
    }

    #[test]
    fn a_timed_out_wait_carries_the_screen_at_its_deadline() {
        let mut server = server();
        let (client, _stream) = add_client(&mut server);
        print(&mut server, b"still working");
        assert!(ask(&mut server, client, capture_wait(Some("done"), None, 20)).is_empty());
        std::thread::sleep(Duration::from_millis(30));
        server.resolve_capture_waits();

        let replies = answered(&server, client);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].code, Some(ControlErrorCode::Timeout));
        assert!(captured_text(&replies[0]).contains("still working"));
    }

    #[test]
    fn a_settle_wait_resolves_once_the_screen_is_quiet() {
        let mut server = server();
        let (client, _stream) = add_client(&mut server);
        assert!(ask(&mut server, client, capture_wait(None, Some(20), 5_000)).is_empty());
        server.resolve_capture_waits();
        assert!(answered(&server, client).is_empty());
        std::thread::sleep(Duration::from_millis(30));
        server.resolve_capture_waits();
        assert!(answered(&server, client)[0].ok);
    }

    #[test]
    fn an_exit_ends_the_wait_but_its_last_output_still_counts() {
        let mut server = server();
        let (client, _stream) = add_client(&mut server);
        let (other, _other_stream) = add_client(&mut server);
        assert!(ask(&mut server, client, capture_wait(Some("done"), None, 5_000)).is_empty());
        assert!(ask(&mut server, other, capture_wait(Some("never"), None, 5_000)).is_empty());

        // The program prints its last line and exits before the pump looks again.
        print(&mut server, b"done\r\n");
        server.panes.get_mut(&3).unwrap().exited = Some(0);
        server.resolve_capture_waits();

        assert!(answered(&server, client)[0].ok);
        let failed = &answered(&server, other)[0];
        assert_eq!(failed.code, Some(ControlErrorCode::PaneNotRunning));
        assert!(captured_text(failed).contains("done"));
    }

    #[test]
    fn a_closed_pane_ends_the_wait() {
        let mut server = server();
        let (client, _stream) = add_client(&mut server);
        assert!(ask(&mut server, client, capture_wait(Some("done"), None, 5_000)).is_empty());
        server.panes.remove(&3);
        server.resolve_capture_waits();
        let failed = &answered(&server, client)[0];
        assert_eq!(failed.code, Some(ControlErrorCode::PaneNotRunning));
        assert!(failed.data.is_none());
    }

    #[test]
    fn a_disconnected_caller_takes_its_wait_with_it() {
        let mut server = server();
        let (client, _stream) = add_client(&mut server);
        assert!(ask(&mut server, client, capture_wait(Some("done"), None, 5_000)).is_empty());
        server.remove_client(client);
        assert!(server.capture_waits.is_empty());
    }

    #[test]
    fn a_malformed_wait_is_refused_before_anything_waits() {
        let mut server = server();
        let (client, _stream) = add_client(&mut server);
        for request in [
            capture_wait(None, None, 5_000),
            capture_wait(Some(""), None, 5_000),
            capture_wait(Some("a\nb"), None, 5_000),
            capture_wait(Some("x"), None, 0),
            capture_wait(Some("x"), None, crate::control::MAX_PANE_WAIT_MS + 1),
            capture_wait(None, Some(5_000), 5_000),
        ] {
            let replies = ask(&mut server, client, request);
            assert_eq!(replies[0].code, Some(ControlErrorCode::InvalidArgument));
        }
        assert!(server.capture_waits.is_empty());
    }
}
