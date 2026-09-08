//! What the session picker says about a remote host it is not connected to.
//!
//! The picker never probes: every remote row it draws for a host this client holds no attachment on
//! is replayed from the host-session cache, which is a memory of the last successful probe. Those
//! rows used to be built as `Running` and rendered exactly like live ones — a pane count and
//! nothing else — so a session on a machine that had been off for a week read as ready to use, and
//! the footer offered to restart and kill it.

use rozi::session::discovery::{DiscoveredSession, DiscoveredSessionStatus};
use rozi::session::remote::RemoteTarget;
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

/// Rendering the app recurses deeply enough to overflow a default test stack.
fn on_a_big_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(body)
        .expect("spawn render thread")
        .join()
        .expect("render thread completes");
}

fn last_seen(name: &str, panes: usize, target: &RemoteTarget) -> DiscoveredSession {
    DiscoveredSession {
        name: name.to_string(),
        ephemeral: false,
        host: Some(target.display_label()),
        remote_target: Some(target.clone()),
        status: DiscoveredSessionStatus::LastSeen { panes },
    }
}

fn live(name: &str, target: &RemoteTarget) -> DiscoveredSession {
    DiscoveredSession {
        name: name.to_string(),
        ephemeral: false,
        host: Some(target.display_label()),
        remote_target: Some(target.clone()),
        status: DiscoveredSessionStatus::Running {
            panes: 1,
            has_layout: true,
            clients: 1,
            created_from_profile: None,
        },
    }
}

fn picker_showing(rows: Vec<DiscoveredSession>) -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(VIEWPORT);
    {
        let state = backend.state_mut();
        *state.current_mut() = rozi::state::Attachment::new();
        state.show_session_picker = true;
        state.session_picker = Some(SessionPickerState::new(rows));
    }
    backend
}

fn screen(backend: &mut TestBackend<AppRoot>) -> String {
    backend.render();
    backend.capture_frame().plain_text()
}

/// The row itself has to carry the news. A bare pane count is the same thing a live session says,
/// and the user cannot tell from it that nothing has answered on that host this sweep.
#[test]
fn a_remembered_session_says_so_instead_of_reading_as_live() {
    on_a_big_stack(|| {
        let target = RemoteTarget::Alias("winvm".to_string());
        let mut backend = picker_showing(vec![last_seen("test", 1, &target)]);

        let rendered = screen(&mut backend);
        assert!(
            rendered.contains("test@winvm"),
            "the session is listed:\n{rendered}"
        );
        assert!(
            rendered.contains("last seen"),
            "a remembered row is dated, not presented as live:\n{rendered}"
        );
    });
}

/// The host's own state belongs on its group header, the way the sidebar has always badged it.
/// `REMOTE · winvm` alone described a machine that might be on or might be gone.
#[test]
fn the_remote_group_header_names_the_hosts_state() {
    on_a_big_stack(|| {
        let target = RemoteTarget::Alias("winvm".to_string());
        let mut backend = picker_showing(vec![last_seen("test", 1, &target)]);

        let rendered = screen(&mut backend);
        assert!(
            rendered.contains("REMOTE") && rendered.contains("winvm"),
            "the group still names its host:\n{rendered}"
        );
        assert!(
            rendered.contains("disconnected"),
            "an unreached host says so on its header:\n{rendered}"
        );
    });
}

/// Restart and kill act on a live server. Against a row nothing has confirmed exists they were an
/// invitation to ssh into an offline machine, so the footer withholds them — as the sidebar has,
/// which gives its cached rows no ✕ for the same reason.
#[test]
fn a_remembered_row_offers_neither_restart_nor_kill() {
    on_a_big_stack(|| {
        let target = RemoteTarget::Alias("winvm".to_string());
        let mut backend = picker_showing(vec![last_seen("test", 1, &target)]);

        let rendered = screen(&mut backend);
        assert!(
            !rendered.contains("restart"),
            "there is no live server to recreate:\n{rendered}"
        );
        assert!(
            !rendered.contains("kill"),
            "there is nothing there to kill:\n{rendered}"
        );

        // The keys themselves, not just their pills: a chord the footer withholds must not still
        // arm a confirmation behind it.
        backend
            .send_key(KeyEvent {
                code: KeyCode::Char('k'),
                mods: KeyMods::CTRL,
            })
            .expect("press ctrl+k on the remembered row");
        let picker = backend
            .state()
            .session_picker
            .as_ref()
            .expect("the picker is still open");
        assert!(
            picker.pending_kill.is_none(),
            "ctrl+k does not arm a kill against a session nothing has confirmed"
        );
    });
}

/// The gate is the row's status, not the fact that it is remote: a live session on a host this
/// client has reached keeps every action it had.
#[test]
fn a_live_remote_row_keeps_its_actions() {
    on_a_big_stack(|| {
        let target = RemoteTarget::Alias("winvm".to_string());
        let mut backend = picker_showing(vec![live("test", &target)]);

        let rendered = screen(&mut backend);
        assert!(
            rendered.contains("restart") && rendered.contains("kill"),
            "a live session is still restartable and killable:\n{rendered}"
        );
        assert!(
            !rendered.contains("last seen"),
            "and it is not dated:\n{rendered}"
        );
    });
}

/// Enter stays on a remembered row on purpose: a host's known workplaces are places the user
/// returns to, and selecting one connects the host and attaches. Only the actions that need a live
/// server are withheld.
#[test]
fn a_remembered_row_can_still_be_activated() {
    on_a_big_stack(|| {
        let target = RemoteTarget::Alias("winvm".to_string());
        let mut backend = picker_showing(vec![last_seen("test", 1, &target)]);

        backend
            .dispatch(Msg::SessionPickerActivate(0))
            .expect("activate the remembered row");
        assert_eq!(
            backend
                .state()
                .current()
                .pending_session_attach
                .as_ref()
                .map(|pending| pending.name.as_str()),
            Some("test"),
            "the row is a way back to the session, not just a label"
        );
    });
}
