//! The session picker's route to this client's scratch (ephemeral) session. The key always works;
//! the footer only spends a pill on it when the list cannot point the way itself.

use rozi::session::discovery::{DiscoveredSession, DiscoveredSessionStatus};
use rozi::state::SessionPickerState;
use rozi::{AppRoot, Msg};
use tui_lipan::TestBackend;
use tui_lipan::prelude::*;

const VIEWPORT: Rect = Rect {
    x: 0,
    y: 0,
    w: 100,
    h: 30,
};

fn session_row(name: &str) -> DiscoveredSession {
    DiscoveredSession {
        name: name.to_string(),
        ephemeral: false,
        host: None,
        remote_target: None,
        status: DiscoveredSessionStatus::Running {
            panes: 1,
            has_layout: true,
            clients: 1,
            created_from_profile: None,
        },
    }
}

/// Rendering the app recurses deeply enough to overflow a default test stack.
fn on_a_big_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(body)
        .expect("spawn render thread")
        .join()
        .expect("render thread completes");
}

fn screen(backend: &mut TestBackend<AppRoot>) -> String {
    backend.render();
    backend.capture_frame().plain_text()
}

#[test]
fn nothing_to_pick_puts_the_scratch_session_on_enter() {
    on_a_big_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(VIEWPORT);
        {
            let state = backend.state_mut();
            *state.current_mut() = rozi::state::Attachment::new();
            state.show_session_picker = true;
            state.session_picker = Some(SessionPickerState::new(Vec::new()));
        }

        let rendered = screen(&mut backend);
        assert!(
            rendered.contains("ephemeral shell Enter"),
            "with no row to activate, Enter carries the scratch session:\n{rendered}"
        );
        assert!(
            !rendered.contains("ephemeral shell Ctrl+t"),
            "the chord goes unsaid while Enter already offers it:\n{rendered}"
        );

        // The key itself, not just the message it sends: the palette must let a bare Enter
        // through once it has no row of its own to activate.
        backend
            .send_key(KeyEvent {
                code: KeyCode::Enter,
                mods: KeyMods::NONE,
            })
            .expect("press enter on the empty picker");
        let state = backend.state();
        assert!(!state.show_session_picker);
        assert_eq!(
            state
                .current()
                .pending_session_attach
                .as_ref()
                .map(|pending| pending.name.as_str()),
            Some(rozi::state::ephemeral_session_name().as_str())
        );
    });
}

#[test]
fn a_query_that_matches_nothing_frees_enter_the_same_way() {
    on_a_big_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(VIEWPORT);
        {
            let state = backend.state_mut();
            *state.current_mut() = rozi::state::Attachment::new();
            state.show_session_picker = true;
            let mut picker = SessionPickerState::new(vec![session_row("dev")]);
            picker.input.set_text("zzz".to_string());
            state.session_picker = Some(picker);
        }

        let rendered = screen(&mut backend);
        assert!(
            rendered.contains("ephemeral shell Enter"),
            "a filter that hides every row leaves the list as empty as an empty one:\n{rendered}"
        );
    });
}

#[test]
fn a_populated_list_advertises_the_chord_until_the_scratch_session_exists() {
    on_a_big_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(VIEWPORT);
        {
            let state = backend.state_mut();
            *state.current_mut() = rozi::state::Attachment::new();
            state.show_session_picker = true;
            state.session_picker = Some(SessionPickerState::new(vec![session_row("dev")]));
        }

        let rendered = screen(&mut backend);
        assert!(
            rendered.contains("ephemeral shell Ctrl+t"),
            "with rows on the list, Enter belongs to them and the chord is spelled out:\n{rendered}"
        );
        assert!(
            !rendered.contains("ephemeral shell Enter"),
            "Enter stays the list's own key:\n{rendered}"
        );
    });
}

#[test]
fn holding_the_scratch_session_drops_the_hint_but_not_the_key() {
    on_a_big_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(VIEWPORT);
        {
            let state = backend.state_mut();
            state.current_mut().session_name = Some(rozi::state::ephemeral_session_name());
            state.current_mut().session_attached = true;
            // Startup queued its own attach; this client is meant to be settled on the session.
            state.current_mut().pending_session_attach = None;
            state.show_session_picker = true;
            state.session_picker = Some(SessionPickerState::new(vec![session_row("dev")]));
        }

        let rendered = screen(&mut backend);
        assert!(
            !rendered.contains("ephemeral shell Ctrl+t")
                && !rendered.contains("ephemeral shell Enter"),
            "the scratch session is on the list itself, so the pill would be noise:\n{rendered}"
        );

        backend
            .dispatch(Msg::SessionPickerEphemeral)
            .expect("the chord still answers");
        let state = backend.state();
        assert!(
            !state.show_session_picker,
            "asking for the session you are already on closes the picker"
        );
        assert!(
            state.current().pending_session_attach.is_none(),
            "and does not re-attach what is already attached"
        );
    });
}

#[test]
fn a_parked_scratch_session_also_drops_the_hint() {
    on_a_big_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(VIEWPORT);
        {
            let state = backend.state_mut();
            state.current_mut().session_name = Some("dev".into());
            state.current_mut().session_attached = true;
            state.current_mut().pending_session_attach = None;
            let mut parked = rozi::state::Attachment::new();
            parked.session_name = Some(rozi::state::ephemeral_session_name());
            parked.session_attached = true;
            state.background.insert(7, parked);
            state.show_session_picker = true;
            state.session_picker = Some(SessionPickerState::new(vec![session_row("dev")]));
        }

        let rendered = screen(&mut backend);
        assert!(
            !rendered.contains("ephemeral shell Ctrl+t"),
            "a scratch session parked in the background is one the client already has:\n{rendered}"
        );
    });
}
