//! The sessionless client: what it draws, and that it stays sessionless until asked otherwise.

use hyprmux::HyprmuxApp;
use hyprmux::state::Attachment;
use tui_lipan::TestBackend;
use tui_lipan::prelude::Rect;

fn launcher_backend() -> TestBackend<HyprmuxApp> {
    let mut backend = TestBackend::new(HyprmuxApp::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 100,
        h: 30,
    });
    // What dismissing the startup picker (or killing the last session) leaves behind: no session
    // client, no pending attach, no panes.
    *backend.state_mut().current_mut() = Attachment::new();
    backend
}

/// A client with no session is a normal state, so it must say so — and say how to leave it. The
/// empty-workspace hint would be wrong here: there is no session for a pane to be spawned into.
#[test]
fn a_sessionless_client_renders_the_launcher_panel() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = launcher_backend();
            assert!(backend.state().is_launcher());
            backend.render();
            let lines = backend.capture_frame().to_fixed_grid_lines();

            assert!(
                lines.iter().any(|line| line.contains("No session")),
                "the launcher must name its own state, got {lines:#?}"
            );
            // The hint reflows, so each way out is checked on its own rather than as one line.
            assert!(
                lines.iter().any(|line| line.contains("pick a session")),
                "the launcher must say how to reach the picker, got {lines:#?}"
            );
            assert!(
                lines.iter().any(|line| line.contains("start a shell")),
                "the launcher must say how to start a session, got {lines:#?}"
            );
            assert!(
                lines
                    .iter()
                    .any(|line| line.contains("Ctrl+A d") && line.contains("detach")),
                "the launcher must say how to leave, got {lines:#?}"
            );
            // The launcher claims a bare Enter (`key_routing::launcher_start_key`); advertising
            // only the spawn binding here is what made the state look like a dead end.
            assert!(
                lines
                    .iter()
                    .any(|line| line.contains("Enter / Ctrl+A Enter")
                        && line.contains("start a shell")),
                "the launcher must offer the bare Enter it accepts, got {lines:#?}"
            );
            // Prefix and held-modifier spellings resolve to the same table entry
            // (`config::schema::scheme_shortcuts`), so listing both would be one binding said twice.
            assert!(
                !lines.iter().any(|line| line.contains("Alt+Enter")),
                "the modifier spelling is the same binding as the prefix one, got {lines:#?}"
            );
            assert!(
                !lines.iter().any(|line| line.contains("Empty workspace")),
                "the empty-workspace hint is about panes, not sessions"
            );
        })
        .expect("spawn launcher smoke thread")
        .join()
        .expect("launcher smoke completes");
}
