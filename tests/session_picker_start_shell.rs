//! The session picker's pinned *start a shell* row: offered in the launcher, where starting a
//! session is the client's own single offer, and nowhere else.

use hyprmux::session::discovery::{DiscoveredSession, DiscoveredSessionStatus};
use hyprmux::state::SessionPickerState;
use hyprmux::{HyprmuxApp, Msg};
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

fn screen(backend: &mut TestBackend<HyprmuxApp>) -> String {
    backend.render();
    backend.capture_frame().plain_text()
}

#[test]
fn the_launcher_picker_offers_a_shell_without_being_dismissed_first() {
    on_a_big_stack(|| {
        let mut backend = TestBackend::new(HyprmuxApp::default());
        backend.set_viewport(VIEWPORT);
        {
            let state = backend.state_mut();
            *state.current_mut() = hyprmux::state::Attachment::new();
            state.show_session_picker = true;
            state.session_picker = Some(SessionPickerState::new(vec![session_row("dev")]));
        }
        assert!(backend.state().is_launcher());

        let rendered = screen(&mut backend);
        assert!(
            rendered.contains("start a shell"),
            "the launcher's offer is on the picker itself:\n{rendered}"
        );
        // The highlight still lands on a session; the pinned row is an alternative, not the default.
        assert!(
            rendered.contains("kill"),
            "session-row hints stay while a session is highlighted:\n{rendered}"
        );

        backend
            .dispatch(Msg::SessionPickerSelectStartShell)
            .expect("highlight the pinned row");
        let rendered = screen(&mut backend);
        assert!(
            !rendered.contains("kill") && !rendered.contains("restart"),
            "nothing on the pinned row can be killed or restarted, so nothing says so:\n{rendered}"
        );
    });
}

#[test]
fn an_empty_launcher_picker_is_a_single_actionable_row() {
    on_a_big_stack(|| {
        let mut backend = TestBackend::new(HyprmuxApp::default());
        backend.set_viewport(VIEWPORT);
        {
            let state = backend.state_mut();
            *state.current_mut() = hyprmux::state::Attachment::new();
            state.show_session_picker = true;
            state.session_picker = Some(SessionPickerState::new(Vec::new()));
        }

        let rendered = screen(&mut backend);
        assert!(
            rendered.contains("start a shell"),
            "an empty picker offers the one thing left to do:\n{rendered}"
        );
        assert!(
            !rendered.contains("No sessions"),
            "an actionable row replaces the dead end:\n{rendered}"
        );
    });
}

#[test]
fn an_attached_picker_lists_only_sessions() {
    on_a_big_stack(|| {
        let mut backend = TestBackend::new(HyprmuxApp::default());
        backend.set_viewport(VIEWPORT);
        {
            let state = backend.state_mut();
            state.current_mut().session_name = Some("dev".into());
            state.current_mut().session_attached = true;
            state.show_session_picker = true;
            state.session_picker = Some(SessionPickerState::new(vec![session_row("notes")]));
        }
        assert!(!backend.state().is_launcher());

        let rendered = screen(&mut backend);
        assert!(
            !rendered.contains("start a shell"),
            "an attached client already has a session to spawn into:\n{rendered}"
        );
    });
}
