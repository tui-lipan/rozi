//! Typing a prompt into an agent: the text as a paste, then Enter once the agent has taken it.
//!
//! Some agents apply a paste asynchronously and handle a key that arrives in the same read before
//! the pasted text lands. Devin CLI does: an Enter written with the paste submitted whatever the
//! input held before it, so every prompt went in one prompt late. The Enter is therefore held back
//! until the program draws something after the paste, and sent on its own. Input for the pane
//! that arrives meanwhile, another prompt included, waits behind it, so nothing can pull the Enter
//! forward into the paste's read or land between the two.

use super::*;
use crate::control::ControlResponse;

/// The shortest gap between a paste and its Enter, so the two never share one read.
const SUBMIT_MIN_DELAY: Duration = Duration::from_millis(15);
/// How long the Enter waits for the program to draw the paste before it is sent anyway.
pub(super) const SUBMIT_MAX_DELAY: Duration = Duration::from_millis(500);

/// A prompt encoded for one pane: its text as the pane takes a paste, and its Enter.
pub(super) struct PromptInput {
    pub(super) paste: Vec<u8>,
    pub(super) enter: Vec<u8>,
}

/// An Enter held back from a prompt typed into a session pane, and the input that came after it.
pub(super) struct HeldSubmit {
    generation: u64,
    /// The pane's output sequence when the paste was written.
    output: u64,
    pasted: Instant,
    enter: Vec<u8>,
    queued: VecDeque<Queued>,
}

/// Input that arrived while an Enter was held, in the order it arrived.
enum Queued {
    Input(Vec<u8>),
    /// The Enter of a prompt whose paste is the input queued just before it.
    Submit(Vec<u8>),
}

impl SessionServer {
    pub(super) fn session_prompt_input(
        &self,
        id: PaneId,
        prompt: &str,
    ) -> std::result::Result<PromptInput, ControlResponse> {
        let enter = self.session_key_bytes(id, &["Enter".into()], false)?;
        let modes = self.panes[&id].screen().key_modes();
        // Enter stays outside the paste, so bulk-input detection cannot read it as part of the
        // prompt, and is encoded for the child's keyboard protocol.
        let paste = tui_lipan::encode_paste(prompt, modes);
        Ok(PromptInput { paste, enter })
    }

    /// Type `prompt` into pane `id`. With `attached`, an attached UI's authority to type was
    /// already settled; otherwise the session's input lock applies. Enter follows on its own.
    pub(super) fn write_prompt(
        &mut self,
        id: PaneId,
        prompt: PromptInput,
        attached: bool,
    ) -> std::result::Result<(), ControlResponse> {
        if !attached {
            self.ensure_headless_input_unlocked()?;
        }
        // The paste goes the ordinary way, and waits behind an Enter already held for the pane.
        // A submitted prompt leaves no unsent draft, so it does not count as someone typing.
        self.write_running_pane(id, prompt.paste)?;
        self.agent_requests.note_prompt_submitted(id);
        if let Some(held) = self.held_submits.get_mut(&id) {
            held.queued.push_back(Queued::Submit(prompt.enter));
            return Ok(());
        }
        let pane = &self.panes[&id];
        self.held_submits.insert(
            id,
            HeldSubmit {
                generation: pane.generation,
                output: pane.output_sequence,
                pasted: Instant::now(),
                enter: prompt.enter,
                queued: VecDeque::new(),
            },
        );
        Ok(())
    }

    /// Whether shared pane `pane_id` at `generation` has a prompt Enter held back. Any stale
    /// entry left over from an earlier generation is dropped.
    pub(super) fn has_live_held_submit(&mut self, pane_id: PaneId, generation: u64) -> bool {
        let Some(held) = self.held_submits.get(&pane_id) else {
            return false;
        };
        if held.generation != generation {
            self.held_submits.remove(&pane_id);
            return false;
        }
        true
    }

    /// Keep `bytes` for pane `pane_id` behind the Enter held for it, if there is one. An Enter held
    /// for a process the pane no longer runs is dropped, with what was queued behind it.
    pub(super) fn queue_behind_held_submit(
        &mut self,
        pane_id: PaneId,
        generation: u64,
        bytes: &[u8],
    ) -> bool {
        if !self.has_live_held_submit(pane_id, generation) {
            return false;
        }
        self.held_submits
            .get_mut(&pane_id)
            .expect("live held submit")
            .queued
            .push_back(Queued::Input(bytes.to_vec()));
        true
    }

    /// Send every held Enter whose paste the program has drawn, or has had long enough to, then
    /// what was queued behind it.
    pub(super) fn send_due_submits(&mut self) {
        let due: Vec<PaneId> = self
            .held_submits
            .iter()
            .filter(|(id, held)| {
                let max_delay = self.submit_max_delay;
                let elapsed = held.pasted.elapsed();
                let drawn = self
                    .panes
                    .get(id)
                    .is_none_or(|pane| pane.output_sequence != held.output);
                elapsed >= max_delay || (drawn && elapsed >= SUBMIT_MIN_DELAY)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            self.send_held_submit(id);
        }
    }

    /// Send pane `id`'s held Enter and the input queued behind it, up to the next prompt's paste,
    /// whose Enter is then held in turn.
    fn send_held_submit(&mut self, id: PaneId) {
        let Some(mut held) = self.held_submits.remove(&id) else {
            return;
        };
        let generation = held.generation;
        if self
            .panes
            .get(&id)
            .is_none_or(|pane| pane.generation != generation)
        {
            return;
        }
        self.write_pane_now(None, id, generation, &held.enter);
        while let Some(queued) = held.queued.pop_front() {
            match queued {
                Queued::Input(bytes) => self.write_pane_now(None, id, generation, &bytes),
                Queued::Submit(enter) => {
                    held.enter = enter;
                    held.pasted = Instant::now();
                    held.output = self.panes[&id].output_sequence;
                    self.held_submits.insert(id, held);
                    return;
                }
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// Collect what the pane echoes until `want` appears, or for `within` when `want` is `None`.
    fn echoed(
        received: &mpsc::Receiver<TerminalPtyEvent>,
        seen: &mut Vec<u8>,
        want: Option<&[u8]>,
        within: Duration,
    ) {
        let deadline = Instant::now() + within;
        while !want.is_some_and(|want| seen.windows(want.len()).any(|window| window == want)) {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return;
            };
            if let Ok(TerminalPtyEvent::Output(bytes)) = received.recv_timeout(left) {
                seen.extend_from_slice(&bytes);
            }
        }
    }

    /// Devin takes a paste and an Enter that reach it together as a paste alone. Input that
    /// arrives right after a prompt, another prompt included, must neither send the held Enter
    /// early nor land between a paste and its Enter.
    #[test]
    fn input_after_a_prompt_waits_behind_its_enter() {
        let mut server = SessionServer::new_named("prompt-input");
        let (events, received) = mpsc::channel();
        let pty = TerminalPty::spawn(
            TerminalPtyConfig::new("/bin/sh")
                .arg("-c")
                .arg("stty raw -echo; printf ready; cat -v"),
            move |event| {
                let _ = events.send(event);
            },
        )
        .unwrap();
        let mut pane = super::super::tests::test_pane(1);
        pane.pty = Some(pty);
        server.panes.insert(3, pane);
        let mut seen = Vec::new();
        echoed(&received, &mut seen, Some(b"ready"), Duration::from_secs(1));
        let prompt = |server: &mut SessionServer, text: &str| {
            let input = server.session_prompt_input(3, text).unwrap();
            server.write_prompt(3, input, false).unwrap();
        };
        let expire = |server: &mut SessionServer| {
            server.held_submits.get_mut(&3).unwrap().pasted -= SUBMIT_MAX_DELAY;
            server.send_due_submits();
        };

        prompt(&mut server, "fix it");
        server.session_write_input(3, b"x".to_vec()).unwrap();
        prompt(&mut server, "again");
        server.send_due_submits();
        // `cat -v` shows a carriage return as `^M`. Only the first paste has reached the pane.
        echoed(&received, &mut seen, None, Duration::from_millis(150));
        assert_eq!(String::from_utf8_lossy(&seen), "readyfix it");

        expire(&mut server);
        echoed(&received, &mut seen, Some(b"again"), Duration::from_secs(1));
        echoed(&received, &mut seen, None, Duration::from_millis(150));
        assert_eq!(String::from_utf8_lossy(&seen), "readyfix it^Mxagain");

        expire(&mut server);
        echoed(
            &received,
            &mut seen,
            Some(b"again^M"),
            Duration::from_secs(1),
        );
        assert_eq!(String::from_utf8_lossy(&seen), "readyfix it^Mxagain^M");
        assert!(server.held_submits.is_empty());
    }

    fn mouse_click() -> crate::control::MouseAction {
        crate::control::MouseAction::Click {
            position: crate::control::MousePoint { x: 4, y: 2 },
            button: crate::control::MouseButton::Left,
            count: 1,
            modifiers: crate::control::MouseModifiers::default(),
        }
    }

    fn mouse_server(
        name: &str,
    ) -> (
        SessionServer,
        mpsc::Receiver<TerminalPtyEvent>,
        ClientId,
        std::os::unix::net::UnixStream,
    ) {
        let mut server = SessionServer::new_named(name);
        let (events, received) = mpsc::channel();
        let pty = TerminalPty::spawn(
            TerminalPtyConfig::new("/bin/sh")
                .arg("-c")
                .arg("stty raw -echo; printf ready; cat -v"),
            move |event| {
                let _ = events.send(event);
            },
        )
        .unwrap();
        let mut pane = super::super::tests::test_pane(1);
        pane.screen_mut().process_bytes(b"\x1b[?1000h\x1b[?1006h");
        pane.pty = Some(pty);
        server.panes.insert(3, pane);
        let (attached_id, attached_stream) = super::super::tests::attach_client(&mut server);
        (server, received, attached_id, attached_stream)
    }

    fn headless_send_mouse(
        server: &mut SessionServer,
        wait: Option<crate::control::PaneWait>,
        capture: Option<crate::control::CaptureRender>,
    ) -> ControlResponse {
        let (client, _stream) = super::super::tests::add_client(server);
        let session = server.session_name.clone();
        let replies = server.handle_session_control(
            client,
            session,
            PROTOCOL_VERSION,
            PROTOCOL_VERSION,
            Some(protocol::Capabilities::current()),
            crate::control::ControlRequest {
                command: crate::control::ControlCommand::SendMouse {
                    target: Some(3),
                    action: mouse_click(),
                    wait,
                    capture,
                    scale: None,
                },
                source_pane: None,
                source_session: None,
                extension: None,
                caller_context: None,
                credential: None,
            },
        );
        replies
            .into_iter()
            .find_map(|(_, message)| match message {
                ServerMessage::SessionControlResult { response, .. } => Some(response),
                _ => None,
            })
            .expect("immediate SessionControlResult")
    }

    fn attached_send_mouse(
        server: &mut SessionServer,
        client_id: ClientId,
        request_id: u64,
    ) -> ControlResponse {
        let replies = server.handle_message(
            client_id,
            ClientMessage::SendMouse {
                request_id,
                pane_id: 3,
                local: false,
                generation: 1,
                action: mouse_click(),
                mark: true,
            },
        );
        replies
            .into_iter()
            .find_map(|(_, message)| match message {
                ServerMessage::SendMouseResult {
                    request_id: id,
                    response,
                } if id == request_id => Some(response),
                _ => None,
            })
            .expect("SendMouseResult")
    }

    /// Viewport-dependent mouse gestures must never queue behind a held prompt Enter on any path
    /// (attached, headless immediate, or headless waiting), and must work again once it flushes.
    #[test]
    fn send_mouse_conflicts_during_held_submit_and_succeeds_after_it_flushes() {
        let (mut server, received, attached_id, _attached_stream) = mouse_server("send-mouse-held");
        let mut seen = Vec::new();
        echoed(&received, &mut seen, Some(b"ready"), Duration::from_secs(1));

        let prompt = server.session_prompt_input(3, "fix it").unwrap();
        server.write_prompt(3, prompt, false).unwrap();

        let attached = attached_send_mouse(&mut server, attached_id, 1);
        assert!(!attached.ok, "{attached:?}");
        assert_eq!(
            attached.code,
            Some(crate::control::ControlErrorCode::Conflict)
        );

        let headless = headless_send_mouse(&mut server, None, None);
        assert!(!headless.ok, "{headless:?}");
        assert_eq!(
            headless.code,
            Some(crate::control::ControlErrorCode::Conflict)
        );

        let waiting = headless_send_mouse(
            &mut server,
            Some(crate::control::PaneWait {
                text: Some("clicked".to_string()),
                settle_ms: None,
                timeout_ms: 5_000,
            }),
            Some(crate::control::CaptureRender::Text),
        );
        assert!(!waiting.ok, "{waiting:?}");
        assert_eq!(
            waiting.code,
            Some(crate::control::ControlErrorCode::Conflict)
        );
        assert!(waiting.data.is_none(), "{waiting:?}");
        assert!(server.capture_waits.is_empty());
        assert!(server.held_submits[&3].queued.is_empty());

        // Flush the held prompt submission and confirm no stray mouse bytes were queued behind it.
        server.held_submits.get_mut(&3).unwrap().pasted -= SUBMIT_MAX_DELAY;
        server.send_due_submits();
        echoed(
            &received,
            &mut seen,
            Some(b"fix it^M"),
            Duration::from_secs(1),
        );
        echoed(&received, &mut seen, None, Duration::from_millis(100));
        assert_eq!(String::from_utf8_lossy(&seen), "readyfix it^M");
        assert!(server.held_submits.is_empty());

        // Once the pending submission finishes, ordinary mouse gestures deliver normally.
        let after = attached_send_mouse(&mut server, attached_id, 2);
        assert!(after.ok, "{after:?}");
        echoed(
            &received,
            &mut seen,
            Some(b"^[[<0;5;3m"),
            Duration::from_secs(1),
        );
        let text = String::from_utf8_lossy(&seen);
        assert!(text.contains("^[[<0;5;3M^[[<0;5;3m"), "{text}");
    }

    /// A PTY write failure between validation and write must fail send-mouse on attached,
    /// headless, and waiting paths instead of returning a false acknowledgement.
    #[test]
    fn send_mouse_propagates_pty_write_failure_without_acknowledging() {
        let (mut server, _received, attached_id, _attached_stream) =
            mouse_server("send-mouse-pty-fail");
        // Kill the PTY without marking `pane.exited`, simulating a write failure before exit
        // processing completes.
        server.panes[&3].pty.as_ref().unwrap().kill().unwrap();
        let epoch_before = server.panes[&3].output_coalesce_epoch;

        let attached = attached_send_mouse(&mut server, attached_id, 10);
        assert!(!attached.ok, "{attached:?}");
        assert_eq!(
            attached.code,
            Some(crate::control::ControlErrorCode::PaneNotRunning)
        );
        assert_eq!(server.panes[&3].output_coalesce_epoch, epoch_before);

        let headless = headless_send_mouse(&mut server, None, None);
        assert!(!headless.ok, "{headless:?}");
        assert_eq!(
            headless.code,
            Some(crate::control::ControlErrorCode::PaneNotRunning)
        );

        let waiting = headless_send_mouse(
            &mut server,
            Some(crate::control::PaneWait {
                text: Some("clicked".to_string()),
                settle_ms: None,
                timeout_ms: 5_000,
            }),
            Some(crate::control::CaptureRender::Text),
        );
        assert!(!waiting.ok, "{waiting:?}");
        assert_eq!(
            waiting.code,
            Some(crate::control::ControlErrorCode::PaneNotRunning)
        );
        assert!(waiting.data.is_none(), "{waiting:?}");
        assert!(server.capture_waits.is_empty());
    }
}
