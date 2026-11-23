//! Typing a prompt into an agent: the text as a paste, then Enter once the agent has taken it.
//!
//! Some agents apply a paste asynchronously and handle a key that arrives in the same read before
//! the pasted text lands. Devin CLI does: an Enter written with the paste submitted whatever the
//! input held before it, so every prompt went in one prompt late. The Enter is therefore held back
//! until the program draws something after the paste, and sent on its own.

use super::*;
use crate::control::ControlResponse;

/// The shortest gap between a paste and its Enter, so the two never share one read.
const SUBMIT_MIN_DELAY: Duration = Duration::from_millis(15);
/// How long the Enter waits for the program to draw the paste before it is sent anyway.
const SUBMIT_MAX_DELAY: Duration = Duration::from_millis(500);

/// A prompt encoded for one pane: its text as the pane takes a paste, and its Enter.
pub(super) struct PromptInput {
    pub(super) paste: Vec<u8>,
    pub(super) enter: Vec<u8>,
}

/// An Enter held back from a prompt typed into a session pane.
pub(super) struct HeldSubmit {
    generation: u64,
    /// The pane's output sequence when the paste was written.
    output: u64,
    pasted: Instant,
    enter: Vec<u8>,
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
        let paste = tui_lipan::prelude::encode_paste(prompt, modes);
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
        if attached {
            self.write_running_pane(id, prompt.paste)?;
        } else {
            self.session_write_input(id, prompt.paste)?;
        }
        let pane = &self.panes[&id];
        self.held_submits.insert(
            id,
            HeldSubmit {
                generation: pane.generation,
                output: pane.output_sequence,
                pasted: Instant::now(),
                enter: prompt.enter,
            },
        );
        Ok(())
    }

    /// Send every held Enter whose paste the program has drawn, or has had long enough to.
    pub(super) fn send_due_submits(&mut self) {
        let due: Vec<PaneId> = self
            .held_submits
            .iter()
            .filter(|(id, held)| {
                let elapsed = held.pasted.elapsed();
                let drawn = self
                    .panes
                    .get(id)
                    .is_none_or(|pane| pane.output_sequence != held.output);
                elapsed >= SUBMIT_MAX_DELAY || (drawn && elapsed >= SUBMIT_MIN_DELAY)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            self.send_held_submit(id);
        }
    }

    /// Send pane `id`'s held Enter now, ahead of any other input to it, so nothing typed later
    /// lands between a prompt and its Enter. One for a pane that was replaced is dropped.
    pub(super) fn send_held_submit(&mut self, id: PaneId) {
        if let Some(held) = self.held_submits.remove(&id) {
            self.write_pane_input(None, id, held.generation, &held.enter);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// Everything `pane` has echoed so far, waiting up to a second for `want` to appear.
    fn echoed(received: &mpsc::Receiver<TerminalPtyEvent>, seen: &mut Vec<u8>, want: &[u8]) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while !seen.windows(want.len()).any(|window| window == want) {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return;
            };
            if let Ok(TerminalPtyEvent::Output(bytes)) = received.recv_timeout(left) {
                seen.extend_from_slice(&bytes);
            }
        }
    }

    /// A paste and its Enter in one read were what Devin took one prompt late, and input typed
    /// while the Enter is held must not land between the two.
    #[test]
    fn a_prompt_reaches_the_pane_before_its_enter_and_ahead_of_later_input() {
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
        echoed(&received, &mut seen, b"ready");

        let prompt = server.session_prompt_input(3, "fix it").unwrap();
        server.write_prompt(3, prompt, false).unwrap();
        server.send_due_submits();
        assert!(
            server.held_submits.contains_key(&3),
            "Enter waits for the paste"
        );

        // Someone types before the agent drew anything: the Enter goes first.
        server.session_write_input(3, b"x".to_vec()).unwrap();
        assert!(server.held_submits.is_empty());

        // A second prompt is sent once its pane has had the longest wait.
        let prompt = server.session_prompt_input(3, "again").unwrap();
        server.write_prompt(3, prompt, false).unwrap();
        server.held_submits.get_mut(&3).unwrap().pasted -= SUBMIT_MAX_DELAY;
        server.send_due_submits();
        assert!(server.held_submits.is_empty());

        // `cat -v` shows a carriage return as `^M`.
        echoed(&received, &mut seen, b"again^M");
        assert_eq!(String::from_utf8_lossy(&seen), "readyfix it^Mxagain^M");
    }
}
