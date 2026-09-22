//! Smoke tests for the modal pick overlay rendering and interactions.

use rozi::AppRoot;
use rozi::state::PickRow;
use std::sync::mpsc;
use tui_lipan::TestBackend;
use tui_lipan::prelude::{KeyCode, KeyEvent, KeyMods, Rect};

fn pick_backend(w: u16, h: u16) -> (TestBackend<AppRoot>, rozi::state::PickReplyReceiver) {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect { x: 0, y: 0, w, h });

    let (tx, rx) = rozi::state::PickReply::channel();
    let (ack_tx, _ack_rx) = mpsc::channel();
    backend
        .dispatch(rozi::Msg::PickStreamOpen {
            width: None,
            actions: Vec::new(),
            id: 1,
            title: Some("Select Branch".into()),
            placeholder: Some("Search branches…".into()),
            empty: None,
            extension: None,
            tabs: Vec::new(),
            tab: None,
            sender: tx,
            ack: ack_tx,
        })
        .expect("dispatch open");

    backend
        .dispatch(rozi::Msg::PickRowsReported {
            id: 1,
            tab: None,
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

#[test]
fn typing_a_miss_on_a_populated_picker_shows_no_matches() {
    on_large_stack(|| {
        let (mut backend, _rx) = pick_backend(100, 30);
        type_query(&mut backend, "zzzzz-not-a-branch");
        let frame = rendered_lines(&mut backend);
        assert!(
            frame.contains("No matches"),
            "a live filter miss on a populated list:\n{frame}"
        );
        assert!(
            !frame.contains("feat/x"),
            "matching rows must leave:\n{frame}"
        );
    });
}

#[test]
fn an_empty_collection_shows_producer_copy_and_a_filter_miss_says_no_matches() {
    on_large_stack(|| {
        rozi::test_support::isolate_user_dirs();
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 80,
            h: 24,
        });
        let (tx, _rx) = rozi::state::PickReply::channel();
        let (ack_tx, _ack_rx) = mpsc::channel();
        backend
            .dispatch(rozi::Msg::PickStreamOpen {
                width: None,
                actions: Vec::new(),
                id: 1,
                title: Some("Snippets".into()),
                placeholder: Some("Filter…".into()),
                empty: Some("No snippets yet".into()),
                extension: None,
                tabs: Vec::new(),
                tab: None,
                sender: tx,
                ack: ack_tx,
            })
            .expect("dispatch open");

        let empty_list = rendered_lines(&mut backend);
        assert!(
            empty_list.contains("No snippets yet"),
            "producer empty copy:\n{empty_list}"
        );
        assert!(
            !empty_list.contains("No matches"),
            "empty collection must not borrow the filter-miss copy:\n{empty_list}"
        );

        backend
            .dispatch(rozi::Msg::PickQueryChanged("zzz".into()))
            .expect("type a filter");
        let miss = rendered_lines(&mut backend);
        assert!(miss.contains("No matches"), "filter miss copy:\n{miss}");
        assert!(
            !miss.contains("No snippets yet"),
            "filter miss must not keep the producer empty copy:\n{miss}"
        );
    });
}

#[test]
fn a_masked_prompt_hides_its_seed_value() {
    on_large_stack(|| {
        rozi::test_support::isolate_user_dirs();
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 80,
            h: 24,
        });
        let (tx, _rx) = rozi::state::PickReply::channel();
        let (ack_tx, _ack_rx) = mpsc::channel();
        backend
            .dispatch(rozi::Msg::PickStreamOpen {
                width: None,
                actions: vec![rozi::state::PickAction {
                    id: "token".into(),
                    key: "ctrl-n".into(),
                    label: "token".into(),
                    prompt: Some(rozi::state::PickPromptSpec::Fields(
                        rozi::state::PickPromptFields {
                            title: "Token".into(),
                            placeholder: None,
                            value: Some("super-secret-token".into()),
                            masked: true,
                        },
                    )),
                    close: false,
                    confirm: false,
                }],
                id: 1,
                title: Some("Secrets".into()),
                placeholder: None,
                empty: None,
                extension: None,
                tabs: Vec::new(),
                tab: None,
                sender: tx,
                ack: ack_tx,
            })
            .expect("dispatch open");
        backend
            .dispatch(rozi::Msg::PickActionKey(0))
            .expect("open prompt");
        let frame = rendered_lines(&mut backend);
        assert!(frame.contains("Token"), "prompt title:\n{frame}");
        assert!(
            !frame.contains("super-secret-token"),
            "masked seed must not appear:\n{frame}"
        );
    });
}

/// A producer can send a description far longer than the row: a build command line beside a short
/// script name. The label is what the user is choosing between, so it must survive whole.
#[test]
fn a_long_description_never_costs_a_row_its_label() {
    on_large_stack(|| {
        rozi::test_support::isolate_user_dirs();
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        });
        let (tx, _rx) = rozi::state::PickReply::channel();
        let (ack_tx, _ack_rx) = std::sync::mpsc::channel();
        backend
            .dispatch(rozi::Msg::PickStreamOpen {
                width: None,
                actions: Vec::new(),
                id: 1,
                title: Some("Tasks".into()),
                placeholder: Some("Filter tasks…".into()),
                empty: None,
                extension: None,
                tabs: Vec::new(),
                tab: None,
                sender: tx,
                ack: ack_tx,
            })
            .expect("dispatch open");
        backend
            .dispatch(rozi::Msg::PickRowsReported {
                id: 1,
                tab: None,
                rows: vec![PickRow {
                    id: Some("npm:build:wasm".into()),
                    label: "build:wasm".into(),
                    description: Some(
                        "wasm-pack build wasm/showcase --target web --out-dir pkg".into(),
                    ),
                    group: Some("package.json".into()),
                    disabled: None,
                    active: false,
                    priority: None,
                }],
            })
            .expect("dispatch rows");

        let frame = rendered_lines(&mut backend);
        assert!(
            frame.contains("build:wasm"),
            "the label survives its own description:\n{frame}"
        );
        assert!(
            !frame.contains("--out-dir pkg"),
            "the description gave up its tail rather than the label:\n{frame}"
        );
    });
}

/// A picker owns the keyboard while it is up. The leader prefix is the one that bites: a chord
/// starting inside a picker used to put rozi into PREFIX mode behind the modal, so the next
/// keystroke ran a window command instead of filtering.
#[test]
fn a_picker_takes_the_keyboard_from_app_chords() {
    on_large_stack(|| {
        let (mut backend, _rx) = pick_backend(100, 30);
        backend.render();
        assert!(
            backend.state().has_modal_overlay(),
            "a picker is a modal overlay like any other"
        );
        // What the command registry was last built with, not what state merely says: the chords are
        // matched by the framework, so a gate nobody applied leaves them live behind the modal.
        assert!(
            !backend.state().commands_gate,
            "the registry was rebuilt for the open picker"
        );

        backend
            .send_key(KeyEvent {
                code: KeyCode::Esc,
                mods: KeyMods::NONE,
            })
            .expect("close the picker");
        backend.render();
        assert!(!backend.state().has_modal_overlay());
        assert!(
            backend.state().commands_gate,
            "closing it hands the chords back without anyone announcing it"
        );
    });
}

fn tabbed_pick_backend(w: u16, h: u16) -> (TestBackend<AppRoot>, rozi::state::PickReplyReceiver) {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect { x: 0, y: 0, w, h });

    let (tx, rx) = rozi::state::PickReply::channel();
    let (ack_tx, _ack_rx) = mpsc::channel();
    backend
        .dispatch(rozi::Msg::PickStreamOpen {
            id: 1,
            title: Some("Git".into()),
            placeholder: None,
            empty: None,
            width: None,
            actions: Vec::new(),
            tabs: ["Branches", "Worktrees"]
                .map(|label| rozi::state::PickTab {
                    id: label.to_lowercase(),
                    label: Some(label.into()),
                })
                .to_vec(),
            tab: None,
            extension: None,
            sender: tx,
            ack: ack_tx,
        })
        .expect("dispatch open");
    for (tab, labels) in [
        ("branches", ["main", "feat/tabs"]),
        ("worktrees", ["rozi-review", "rozi-docs"]),
    ] {
        backend
            .dispatch(rozi::Msg::PickRowsReported {
                id: 1,
                tab: Some(tab.into()),
                rows: labels
                    .map(|label| PickRow {
                        id: Some(label.into()),
                        label: label.into(),
                        description: None,
                        group: None,
                        disabled: None,
                        active: false,
                        priority: None,
                    })
                    .to_vec(),
            })
            .expect("dispatch rows");
    }
    (backend, rx)
}

fn press(backend: &mut TestBackend<AppRoot>, code: KeyCode) {
    backend.render();
    backend
        .send_key(KeyEvent {
            code,
            mods: KeyMods::NONE,
        })
        .expect("send key");
}

/// Tabs are the same picker with a strip on top: Tab moves between pages, each page shows only
/// its own rows, and the filter typed on one page is waiting there on the way back.
#[test]
fn a_tabbed_picker_switches_pages_and_keeps_each_filter() {
    on_large_stack(|| {
        let (mut backend, rx) = tabbed_pick_backend(100, 30);
        let frame = rendered_lines(&mut backend);
        assert!(frame.contains("Branches"), "strip rendered:\n{frame}");
        assert!(frame.contains("Worktrees"), "strip rendered:\n{frame}");
        assert!(frame.contains("feat/tabs"), "first page shown:\n{frame}");
        assert!(
            !frame.contains("rozi-review"),
            "second page hidden:\n{frame}"
        );

        type_query(&mut backend, "feat");
        let frame = rendered_lines(&mut backend);
        assert!(!frame.contains("main"), "filter applied:\n{frame}");

        press(&mut backend, KeyCode::Tab);
        let frame = rendered_lines(&mut backend);
        assert_eq!(rx.try_recv().unwrap().trim(), r#"{"tab":"worktrees"}"#);
        assert!(frame.contains("rozi-review"), "second page shown:\n{frame}");
        assert!(
            frame.contains("rozi-docs"),
            "its own empty filter:\n{frame}"
        );
        assert!(!frame.contains("feat/tabs"), "first page hidden:\n{frame}");

        press(&mut backend, KeyCode::BackTab);
        let frame = rendered_lines(&mut backend);
        assert_eq!(rx.try_recv().unwrap().trim(), r#"{"tab":"branches"}"#);
        assert!(frame.contains("feat/tabs"), "first page back:\n{frame}");
        assert!(
            !frame.contains("main"),
            "the first page's filter came back with it:\n{frame}"
        );

        // Left and Right step pages too, as on every other tabbed picker.
        press(&mut backend, KeyCode::Right);
        assert_eq!(rx.try_recv().unwrap().trim(), r#"{"tab":"worktrees"}"#);
        press(&mut backend, KeyCode::Left);
        assert_eq!(rx.try_recv().unwrap().trim(), r#"{"tab":"branches"}"#);

        press(&mut backend, KeyCode::Enter);
        assert_eq!(
            rx.try_recv().unwrap().trim(),
            r#"{"selected":"feat/tabs","tab":"branches"}"#
        );
    });
}
