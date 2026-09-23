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
        origin: Default::default(),
        ephemeral: false,
        host: Some(target.display_label()),
        remote_target: Some(target.clone()),
        status: DiscoveredSessionStatus::LastSeen { panes },
    }
}

fn live(name: &str, target: &RemoteTarget) -> DiscoveredSession {
    DiscoveredSession {
        name: name.to_string(),
        origin: Default::default(),
        ephemeral: false,
        host: Some(target.display_label()),
        remote_target: Some(target.clone()),
        status: DiscoveredSessionStatus::Running {
            panes: 1,
            has_layout: true,
            clients: 1,
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
            rendered.contains("test") && !rendered.contains("test@winvm"),
            "the session is listed by name; its group header already names the host:\n{rendered}"
        );
        assert!(
            rendered.contains("last seen"),
            "a remembered row is dated, not presented as live:\n{rendered}"
        );
    });
}

/// Rows no longer spell out their host, so a query for the host has to match it through the hidden
/// alias and keep the header that names it above the surviving rows.
#[test]
fn a_host_query_keeps_its_rows_under_their_header() {
    on_a_big_stack(|| {
        let target = RemoteTarget::Alias("winvm".to_string());
        let mut backend = picker_showing(vec![last_seen("test", 1, &target)]);
        screen(&mut backend);
        for ch in "winvm".chars() {
            backend
                .send_key(KeyEvent {
                    code: KeyCode::Char(ch),
                    mods: KeyMods::NONE,
                })
                .expect("type the host into the query");
        }

        let rendered = screen(&mut backend);
        assert_eq!(
            backend
                .state()
                .session_picker
                .as_ref()
                .map(|picker| picker.input.text().to_string())
                .as_deref(),
            Some("winvm"),
            "the query reached the picker"
        );
        assert!(
            rendered.contains("REMOTE") && rendered.contains("last seen"),
            "the host's header and its row survive the query:\n{rendered}"
        );
    });
}

/// Once one row carries a connection marker, the rows this client holds nothing on get a muted dot
/// in that column instead of a blank.
#[test]
fn unheld_rows_get_a_dot_beside_a_marked_row() {
    on_a_big_stack(|| {
        let target = RemoteTarget::Alias("winvm".to_string());
        let local = DiscoveredSession {
            name: "dev".to_string(),
            origin: Default::default(),
            ephemeral: false,
            host: None,
            remote_target: None,
            status: DiscoveredSessionStatus::Running {
                panes: 1,
                has_layout: true,
                clients: 1,
            },
        };
        let mut backend = picker_showing(vec![local, last_seen("test", 1, &target)]);
        backend.state_mut().current_mut().session_name = Some("dev".to_string());

        let rendered = screen(&mut backend);
        assert!(
            rendered.contains("● dev"),
            "the session on screen is marked:\n{rendered}"
        );
        assert!(
            rendered.contains("· test"),
            "the unheld row gets a dot, not a blank:\n{rendered}"
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

/// Restart is withheld: there is no live server to recreate. Kill is withheld for the same reason.
/// Forget is offered instead — the row is local cached knowledge, and Ctrl+K drops that memory
/// using the same arm-then-confirm the picker already uses for snapshots.
#[test]
fn a_remembered_row_offers_forget_not_kill() {
    on_a_big_stack(|| {
        let target = RemoteTarget::Alias("winvm".to_string());
        let mut backend = picker_showing(vec![last_seen("test", 1, &target)]);

        let rendered = screen(&mut backend);
        assert!(
            rendered.contains("forget Ctrl+K"),
            "a remembered row can be forgotten:\n{rendered}"
        );
        assert!(
            !rendered.contains("restart"),
            "there is no live server to recreate:\n{rendered}"
        );
        assert!(
            !rendered.contains("kill"),
            "forgetting a cache entry is not a live kill:\n{rendered}"
        );

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
        assert_eq!(
            picker.pending_kill,
            Some(0),
            "ctrl+k arms forget with the same confirmation the rest of the picker uses"
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
