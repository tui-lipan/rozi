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

    /// Keep `bytes` for pane `pane_id` behind the Enter held for it, if there is one. An Enter held
    /// for a process the pane no longer runs is dropped, with what was queued behind it.
    pub(super) fn queue_behind_held_submit(
        &mut self,
        pane_id: PaneId,
        generation: u64,
        bytes: &[u8],
    ) -> bool {
        let Some(held) = self.held_submits.get_mut(&pane_id) else {
            return false;
        };
        if held.generation != generation {
            self.held_submits.remove(&pane_id);
            return false;
        }
        held.queued.push_back(Queued::Input(bytes.to_vec()));
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
}
