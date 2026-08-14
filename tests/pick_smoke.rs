//! Smoke tests for the modal pick overlay rendering and interactions.

use rozi::AppRoot;
use rozi::state::PickRow;
use std::sync::mpsc;
use tui_lipan::TestBackend;
use tui_lipan::prelude::{KeyCode, KeyEvent, KeyMods, Rect};

fn pick_backend(w: u16, h: u16) -> (TestBackend<AppRoot>, mpsc::Receiver<String>) {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect { x: 0, y: 0, w, h });

    let (tx, rx) = mpsc::sync_channel(1);
    let (ack_tx, _ack_rx) = mpsc::channel();
    backend
        .dispatch(rozi::Msg::PickStreamOpen {
            id: 1,
            title: Some("Select Branch".into()),
            placeholder: Some("Search branches…".into()),
            sender: tx,
            ack: ack_tx,
        })
        .expect("dispatch open");

    backend
        .dispatch(rozi::Msg::PickRowsReported {
            id: 1,
            rows: vec![
                PickRow {
                    id: Some("main".into()),
                    label: "main".into(),
                    description: Some("2 hours ago".into()),
                    group: Some("Local".into()),
                    disabled: None,
                    active: true,
                    priority: None,
                },
                PickRow {
                    id: Some("feat/x".into()),
                    label: "feat/x".into(),
                    description: Some("yesterday".into()),
                    group: Some("Local".into()),
                    disabled: None,
                    active: false,
                    priority: None,
                },
                PickRow {
                    id: Some("origin/pr-12".into()),
                    label: "origin/pr-12".into(),
                    description: Some("3 days ago".into()),
                    group: Some("Remote".into()),
                    disabled: Some("Locked by CI".into()),
                    active: false,
                    priority: None,
                },
            ],
        })
        .expect("dispatch rows");

    (backend, rx)
}

fn rendered_lines(backend: &mut TestBackend<AppRoot>) -> String {
    backend.render();
    backend.capture_frame().to_fixed_grid_lines().join("\n")
}

fn type_query(backend: &mut TestBackend<AppRoot>, query: &str) {
    backend.render();
    for character in query.chars() {
        backend
            .send_key(KeyEvent {
                code: KeyCode::Char(character),
                mods: KeyMods::NONE,
            })
            .expect("type pick query");
    }
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
fn pick_overlay_renders_groups_descriptions_and_disabled_reason() {
    on_large_stack(|| {
        let (mut backend, _rx) = pick_backend(100, 30);
        let frame = rendered_lines(&mut backend);

        assert!(frame.contains("Select Branch"), "title rendered:\n{frame}");
        assert!(frame.contains("Local"), "Local group rendered:\n{frame}");
        assert!(frame.contains("Remote"), "Remote group rendered:\n{frame}");
        assert!(frame.contains("main"), "main branch rendered:\n{frame}");
        assert!(
            frame.contains("2 hours ago"),
            "main description rendered:\n{frame}"
        );
        assert!(frame.contains("feat/x"), "feat/x rendered:\n{frame}");
        assert!(
            frame.contains("yesterday"),
            "feat/x description rendered:\n{frame}"
        );
        assert!(
            frame.contains("origin/pr-12"),
            "remote row rendered:\n{frame}"
        );
        assert!(
            frame.contains("Locked by CI"),
            "disabled reason rendered:\n{frame}"
        );
    });
}

#[test]
fn pick_overlay_filters_rows() {
    on_large_stack(|| {
        let (mut backend, _rx) = pick_backend(100, 30);
        type_query(&mut backend, "feat");
        let frame = rendered_lines(&mut backend);

        assert!(frame.contains("feat/x"), "matching row rendered:\n{frame}");
        assert!(
            !frame.contains("origin/pr-12"),
            "non-matching row filtered out:\n{frame}"
        );
    });
}
