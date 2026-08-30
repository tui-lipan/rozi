//! How the remote host's session list reports who else is on a session.

use rozi::AppRoot;
use rozi::session::discovery::{DiscoveredSession, DiscoveredSessionStatus};
use rozi::session::remote::RemoteTarget;
use tui_lipan::TestBackend;
use tui_lipan::prelude::Rect;

fn on_large_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(body)
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
}

fn running(name: &str, clients: u32, target: &RemoteTarget) -> DiscoveredSession {
    DiscoveredSession {
        name: name.to_string(),
        status: DiscoveredSessionStatus::Running {
            panes: 1,
            clients,
            has_layout: false,
            created_from_profile: None,
        },
        ephemeral: false,
        host: Some(target.display_label()),
        remote_target: Some(target.clone()),
    }
}

/// The picker showing one host's sessions, with `rows` already discovered. Driven through the
/// picker's own state rather than a probe, which would need a reachable host.
fn host_sessions_backend(
    target: &RemoteTarget,
    rows: Vec<DiscoveredSession>,
) -> TestBackend<AppRoot> {
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
    let picker = backend
        .state_mut()
        .remote_picker
        .as_mut()
        .expect("the picker is open");
    picker.enter_host_sessions(target.clone());
    picker.replace_sessions(rows);
    backend
}

fn rendered_lines(backend: &mut TestBackend<AppRoot>) -> String {
    backend.render();
    backend.capture_frame().to_fixed_grid_lines().join("\n")
}

/// A session this client opened is itself one of the clients the remote server counts, and rozi
/// keeps it attached in the background after switching away. Counting that as company tells the
/// user somebody else is on every session they have ever opened on the host.
#[test]
fn a_remote_session_this_client_holds_is_not_reported_as_shared() {
    on_large_stack(|| {
        let target = RemoteTarget::Alias("localhost".to_string());
        let mut backend = host_sessions_backend(&target, vec![running("dev", 1, &target)]);

        let frame = rendered_lines(&mut backend);
        assert!(frame.contains("dev"), "the session is listed:\n{frame}");
        assert!(
            frame.contains("shared with 1 other"),
            "a client that is not us is company:\n{frame}"
        );

        // Now that one client is this rozi, holding the session.
        backend.state_mut().current_mut().session_name = Some("dev".to_string());
        backend.state_mut().current_mut().remote_target = Some(target);
        let frame = rendered_lines(&mut backend);
        assert!(frame.contains("dev"), "still listed:\n{frame}");
        assert!(
            !frame.contains("shared with"),
            "our own connection is not somebody else:\n{frame}"
        );
    });
}

/// Discounting our own connection must not hide a real one: a session we hold *and* someone else
/// is on still reports the someone else.
#[test]
fn a_session_we_hold_still_reports_the_other_client_on_it() {
    on_large_stack(|| {
        let target = RemoteTarget::Alias("localhost".to_string());
        let mut backend = host_sessions_backend(&target, vec![running("dev", 2, &target)]);
        backend.state_mut().current_mut().session_name = Some("dev".to_string());
        backend.state_mut().current_mut().remote_target = Some(target);

        let frame = rendered_lines(&mut backend);
        assert!(
            frame.contains("shared with 1 other"),
            "two clients minus ourselves is one other:\n{frame}"
        );
    });
}
