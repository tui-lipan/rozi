//! The modal that stands in for the terminal prompt `ssh` would otherwise write over the UI.
//!
//! Two properties matter enough to pin: the question reaches the user verbatim (a host-key
//! fingerprint is unusable summarized), and a secret never lands on screen — including in the
//! frame a capture or a screen share would pick up.

use rozi::AppRoot;
use rozi::session::remote::AskpassKind;
use tui_lipan::TestBackend;
use tui_lipan::prelude::{KeyCode, KeyEvent, KeyMods, Rect};

fn askpass_backend(kind: AskpassKind, prompt: &str) -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 100,
        h: 30,
    });
    backend
        .dispatch(rozi::Msg::RemoteAskpassPrompt {
            id: 7,
            session: "ssh-1".to_string(),
            kind,
            prompt: prompt.to_string(),
        })
        .expect("dispatch prompt");
    backend
}

fn rendered_lines(backend: &mut TestBackend<AppRoot>) -> String {
    backend.render();
    backend.capture_frame().to_fixed_grid_lines().join("\n")
}

fn type_text(backend: &mut TestBackend<AppRoot>, text: &str) {
    backend.render();
    for character in text.chars() {
        backend
            .send_key(KeyEvent {
                code: KeyCode::Char(character),
                mods: KeyMods::NONE,
            })
            .expect("type into the prompt");
    }
}

fn prompt_msg(id: u64, session: &str, prompt: &str) -> rozi::Msg {
    rozi::Msg::RemoteAskpassPrompt {
        id,
        session: session.to_string(),
        kind: AskpassKind::Secret,
        prompt: prompt.to_string(),
    }
}

/// A backend with the remote host picker open, which is where an ssh prompt is raised from.
fn picker_backend() -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 100,
        h: 30,
    });
    backend
        .dispatch(rozi::Msg::SessionPickerRemoteHosts)
        .expect("open the remote picker");
    backend
}

fn on_large_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(body)
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
}

#[test]
fn a_password_prompt_names_the_account_and_masks_what_is_typed() {
    on_large_stack(|| {
        let mut backend = askpass_backend(AskpassKind::Secret, "dev@workbox's password: ");
        type_text(&mut backend, "hunter2");
        let frame = rendered_lines(&mut backend);

        assert!(
            frame.contains("dev@workbox"),
            "the prompt says which account:\n{frame}"
        );
        assert!(
            !frame.contains("hunter2"),
            "the secret must never reach the screen:\n{frame}"
        );
        assert!(frame.contains("•••••••"), "masked instead:\n{frame}");
    });
}

/// A fingerprint the user cannot read is a fingerprint they cannot check, so the confirmation
/// carries the whole question and echoes the answer back.
#[test]
fn a_host_key_prompt_shows_the_fingerprint_and_does_not_mask_the_answer() {
    on_large_stack(|| {
        let mut backend = askpass_backend(
            AskpassKind::Confirm,
            "The authenticity of host 'workbox (192.0.2.7)' can't be established.\n\
             ED25519 key fingerprint is SHA256:qJv1zHtest.\n\
             Are you sure you want to continue connecting (yes/no/[fingerprint])? ",
        );
        type_text(&mut backend, "yes");
        let frame = rendered_lines(&mut backend);

        assert!(
            frame.contains("SHA256:qJv1zHtest"),
            "the fingerprint is the thing being confirmed:\n{frame}"
        );
        assert!(frame.contains("yes"), "the answer is echoed:\n{frame}");
        // The trailing question is the last thing a wrapped prompt renders, so it is the first
        // thing lost if the modal ever measures the text unwrapped again.
        assert!(
            frame.contains("Are you sure you want to continue connecting"),
            "the question itself survives wrapping:\n{frame}"
        );
    });
}

/// Two ssh processes can prompt at once. The second waits its turn instead of replacing the first,
/// whose helper is still holding a connection open for an answer.
#[test]
fn a_second_prompt_queues_behind_the_one_on_screen() {
    on_large_stack(|| {
        let mut backend = askpass_backend(AskpassKind::Secret, "dev@first's password: ");
        backend
            .dispatch(prompt_msg(8, "ssh-2", "dev@second's password: "))
            .expect("dispatch second prompt");

        let frame = rendered_lines(&mut backend);
        assert!(frame.contains("dev@first"), "first still shown:\n{frame}");
        assert!(!frame.contains("dev@second"), "second waits:\n{frame}");

        backend
            .dispatch(rozi::Msg::SubmitRemoteAskpass)
            .expect("answer the first");
        let frame = rendered_lines(&mut backend);
        assert!(frame.contains("dev@second"), "second follows:\n{frame}");
    });
}

/// Answering the last prompt takes the modal down; nothing is left covering the workspace once
/// ssh has what it asked for.
#[test]
fn answering_the_last_prompt_closes_the_modal() {
    on_large_stack(|| {
        let mut backend = askpass_backend(AskpassKind::Secret, "dev@workbox's password: ");
        backend
            .dispatch(rozi::Msg::SubmitRemoteAskpass)
            .expect("answer");
        let frame = rendered_lines(&mut backend);
        assert!(!frame.contains("dev@workbox"), "modal dismissed:\n{frame}");
    });
}

/// A helper that gave up waiting takes its own prompt with it, and only its own: a queued prompt
/// belongs to a different ssh that is still waiting.
#[test]
fn an_expired_prompt_is_dropped_without_disturbing_the_queue() {
    on_large_stack(|| {
        let mut backend = askpass_backend(AskpassKind::Secret, "dev@first's password: ");
        backend
            .dispatch(prompt_msg(8, "ssh-2", "dev@second's password: "))
            .expect("dispatch second prompt");
        backend
            .dispatch(rozi::Msg::RemoteAskpassExpired { id: 7 })
            .expect("first helper gives up");

        let frame = rendered_lines(&mut backend);
        assert!(
            frame.contains("dev@second"),
            "the waiting prompt takes over:\n{frame}"
        );
        assert!(
            !frame.contains("dev@first"),
            "expired one is gone:\n{frame}"
        );
    });
}

/// The prompt stacks on the picker that provoked it and fades it back, rather than replacing it:
/// the picker keeps its state and the user returns to exactly what they left.
#[test]
fn the_prompt_stacks_on_the_remote_picker_and_leaves_it_mounted() {
    on_large_stack(|| {
        let mut backend = picker_backend();
        let frame = rendered_lines(&mut backend);
        assert!(frame.contains("Remote hosts"), "picker is up:\n{frame}");

        backend
            .dispatch(prompt_msg(7, "ssh-1", "dev@workbox's password: "))
            .expect("dispatch prompt");
        let frame = rendered_lines(&mut backend);
        assert!(frame.contains("dev@workbox"), "prompt is up:\n{frame}");
        // Both frames on one row: the picker is still drawn (and faded) behind the prompt rather
        // than unmounted, which is what keeps its query and highlight.
        assert!(
            frame.contains("\u{2502} \u{2502}"),
            "the picker's frame is still drawn behind it:\n{frame}"
        );
        assert!(
            backend.state().remote_picker.is_some(),
            "and its state is untouched"
        );

        backend
            .dispatch(rozi::Msg::CancelRemoteAskpass)
            .expect("refuse the prompt");
        let frame = rendered_lines(&mut backend);
        assert!(frame.contains("Remote hosts"), "picker returns:\n{frame}");
        assert!(!frame.contains("dev@workbox"), "prompt is gone:\n{frame}");
    });
}

/// One Esc has to mean "stop asking". `ssh` re-raises the same question three times whatever the
/// helper answers, and a probe runs several ssh invocations back to back, so a refusal that only
/// dismissed one dialog would leave the user pressing Esc at nine of them.
#[test]
fn refusing_one_prompt_silences_the_retries_behind_it() {
    on_large_stack(|| {
        let mut backend = picker_backend();
        backend
            .dispatch(prompt_msg(7, "ssh-1", "dev@workbox's password: "))
            .expect("dispatch prompt");
        backend
            .dispatch(rozi::Msg::CancelRemoteAskpass)
            .expect("refuse");

        for id in 8..=9 {
            backend
                .dispatch(prompt_msg(id, "ssh-1", "dev@workbox's password: "))
                .expect("ssh asks again");
            let frame = rendered_lines(&mut backend);
            assert!(
                !frame.contains("dev@workbox"),
                "retry {id} must not reopen the dialog:\n{frame}"
            );
        }
    });
}

/// Refusing gives up on the probe that raised the prompt, so the picker stops spinning on a
/// connection the user has just called off.
#[test]
fn refusing_a_prompt_gives_up_on_the_host_probe() {
    on_large_stack(|| {
        let mut backend = picker_backend();
        backend
            .dispatch(rozi::Msg::RemotePickerHostActivate(
                rozi::session::remote::RemoteTarget::Alias("workbox".to_string()),
            ))
            .expect("activate the host");
        assert!(
            backend
                .state()
                .remote_picker
                .as_ref()
                .is_some_and(|picker| {
                    matches!(picker.host_probe, rozi::state::HostProbe::InFlight)
                }),
            "the probe is in flight"
        );

        backend
            .dispatch(prompt_msg(7, "ssh-1", "dev@workbox's password: "))
            .expect("dispatch prompt");
        backend
            .dispatch(rozi::Msg::CancelRemoteAskpass)
            .expect("refuse");

        assert!(
            backend
                .state()
                .remote_picker
                .as_ref()
                .is_some_and(|picker| {
                    !matches!(picker.host_probe, rozi::state::HostProbe::InFlight)
                }),
            "the picker is no longer connecting"
        );
    });
}

/// `ssh` re-asks in silence after a wrong password, so the modal has to say why it came back —
/// otherwise it reads as a submit that did nothing.
#[test]
fn a_rejected_password_says_so_when_ssh_asks_again() {
    on_large_stack(|| {
        let mut backend = picker_backend();
        backend
            .dispatch(prompt_msg(7, "ssh-1", "dev@workbox's password: "))
            .expect("dispatch prompt");
        backend
            .dispatch(rozi::Msg::SubmitRemoteAskpass)
            .expect("answer it");
        backend
            .dispatch(prompt_msg(8, "ssh-1", "dev@workbox's password: "))
            .expect("ssh asks again");

        let frame = rendered_lines(&mut backend);
        assert!(
            frame.contains("dev@workbox"),
            "the dialog is back:\n{frame}"
        );
        assert!(
            frame.contains("Rejected"),
            "and it says the answer was refused:\n{frame}"
        );
    });
}

/// The picker behind the prompt has to recede, not just sit there competing with it. Measured on
/// the picker's own frame rather than the text grid, because fading is a colour change and a
/// colourless capture cannot see it.
#[test]
fn the_prompt_fades_the_picker_behind_it() {
    on_large_stack(|| {
        let mut backend = picker_backend();
        backend.render();
        // A cell on the picker's left border, outside where the narrower prompt is drawn.
        let (x, y) = (18, 7);
        let lit = backend.capture_frame().cell(x, y).fg;

        backend
            .dispatch(prompt_msg(7, "ssh-1", "dev@workbox's password: "))
            .expect("dispatch prompt");
        backend.render();
        let faded = backend.capture_frame().cell(x, y).fg;

        assert_ne!(
            lit, faded,
            "the picker's frame is repainted, not left as it was"
        );
        assert!(
            channel_sum(faded) < channel_sum(lit),
            "and repainted darker: {lit:?} -> {faded:?}"
        );
    });
}

fn channel_sum(color: tui_lipan::prelude::Color) -> u32 {
    let (r, g, b) = color
        .to_rgb()
        .expect("a captured cell reports a concrete color");
    u32::from(r) + u32::from(g) + u32::from(b)
}
