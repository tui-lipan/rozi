use std::str::FromStr;

use rozi::AppRoot;
use tui_lipan::TestBackend;
use tui_lipan::prelude::{KeyBinding, KeyCode, KeyEvent, KeyMods, Rect};

fn backend() -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 110,
        h: 50,
    });
    backend
        .dispatch(rozi::Msg::RunAction(rozi::input::Action::ToggleHelp))
        .expect("open keybindings");
    backend
}

fn selected(backend: &TestBackend<AppRoot>) -> Option<String> {
    backend
        .state()
        .keybindings
        .as_ref()
        .and_then(|keybindings| keybindings.selected.clone())
}

fn frame_y(backend: &TestBackend<AppRoot>, title: &str) -> i16 {
    backend
        .capture_ui_snapshot()
        .widgets
        .iter()
        .find(|widget| {
            widget.kind == tui_lipan::UiWidgetKind::Frame && widget.title.as_deref() == Some(title)
        })
        .unwrap_or_else(|| panic!("missing {title} frame"))
        .rect
        .y
}

fn send(backend: &mut TestBackend<AppRoot>, code: KeyCode) {
    backend
        .send_key(KeyEvent {
            code,
            mods: KeyMods::NONE,
        })
        .expect("send key");
    backend.render();
}

fn frame(backend: &mut TestBackend<AppRoot>) -> String {
    backend.render();
    backend.capture_frame().to_fixed_grid_lines().join("\n")
}

#[test]
fn keybindings_overlay_edits_from_search_and_stops_on_conflicts() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(body)
        .expect("spawn keybinding edit smoke thread")
        .join()
        .expect("keybinding edit smoke completes");
}

fn body() {
    let mut backend = backend();
    let editor = frame(&mut backend);
    assert!(editor.contains("╭Keybindings─"), "{editor}");
    assert!(!editor.contains("current ← default"), "{editor}");
    assert!(editor.contains("Global"), "{editor}");
    assert!(editor.contains("Search keybindings…"), "{editor}");
    assert!(editor.contains("Prefix · then key"), "{editor}");
    assert!(editor.contains("APP"), "{editor}");
    assert!(!editor.contains("← w"), "{editor}");
    assert_eq!(
        backend.focused_key().map(|key| key.as_ref()),
        Some("rozi-help-filter")
    );

    // The scheme rows lead the list and are selectable, and each arrow press paints at once.
    assert_eq!(selected(&backend), None, "the list opens on its first row");
    send(&mut backend, KeyCode::Down);
    let second = selected(&backend).expect("Down selects the next row");
    assert!(second.contains("Mod"), "{second}");
    send(&mut backend, KeyCode::Up);
    let first = selected(&backend).expect("Up selects the first row");
    assert!(first.contains("Prefix"), "{first}");
    // The Prefix row changes the scheme itself: it can be changed, but not unbound or reset.
    let footer = frame(&mut backend);
    assert!(footer.contains("change Enter"), "{footer}");
    for hint in ["unbind Ctrl+U", "reset Ctrl+D", "switch tabs ←/→"] {
        assert!(!footer.contains(hint), "footer shows `{hint}`:\n{footer}");
    }
    send(&mut backend, KeyCode::Enter);
    let prefix_card = frame(&mut backend);
    assert!(prefix_card.contains("Change prefix"), "{prefix_card}");
    assert!(prefix_card.contains("Press new prefix…"), "{prefix_card}");
    send(&mut backend, KeyCode::Esc);
    assert!(!frame(&mut backend).contains("Change prefix"));

    // A reference-only row advertises no row actions.
    backend
        .dispatch(rozi::Msg::HelpTabSelected(1))
        .expect("show mode keys");
    let modes = frame(&mut backend);
    assert!(!modes.contains("change Enter"), "{modes}");
    backend
        .dispatch(rozi::Msg::HelpTabSelected(0))
        .expect("back to global keys");

    // Changing tab keeps focus in the search field and highlights the new tab's first row.
    for (key, expected) in [
        (KeyCode::Right, rozi::state::HelpTab::Modes),
        (KeyCode::Left, rozi::state::HelpTab::Global),
        (KeyCode::Tab, rozi::state::HelpTab::Modes),
        (KeyCode::BackTab, rozi::state::HelpTab::Global),
    ] {
        send(&mut backend, key);
        let keybindings = backend.state().keybindings.as_ref().expect("overlay open");
        assert_eq!(keybindings.tab, expected, "after {key:?}");
        assert_eq!(keybindings.selected, None, "after {key:?}");
        assert_eq!(
            backend.focused_key().map(|key| key.as_ref()),
            Some("rozi-help-filter"),
            "after {key:?}"
        );
    }

    // Typing filters and resets the highlight to the first match, which Enter then edits.
    for ch in "close pane".chars() {
        send(&mut backend, KeyCode::Char(ch));
    }
    assert_eq!(selected(&backend), None);
    send(&mut backend, KeyCode::Down);
    send(&mut backend, KeyCode::Up);
    assert_eq!(selected(&backend).as_deref(), Some("close"));
    let editable = frame(&mut backend);
    for hint in ["change Enter", "unbind Ctrl+U"] {
        assert!(
            editable.contains(hint),
            "footer misses `{hint}`:\n{editable}"
        );
    }
    assert!(
        !editable.contains("reset Ctrl+D"),
        "no override to reset:\n{editable}"
    );
    backend
        .send_key(KeyEvent {
            code: KeyCode::Enter,
            mods: KeyMods::NONE,
        })
        .expect("start capture");
    let capture = frame(&mut backend);
    assert!(capture.contains("╭Keybindings─"), "{capture}");
    assert!(capture.contains("Change keybinding"), "{capture}");
    assert_eq!(
        frame_y(&backend, "Change keybinding"),
        frame_y(&backend, "Keybindings") + 1,
        "the capture card sits one row below Keybindings"
    );
    assert!(capture.contains("Press new keybinding…"), "{capture}");
    assert!(capture.contains("● REC"), "{capture}");
    let rec_line = capture
        .lines()
        .find(|line| line.contains("● REC"))
        .unwrap_or_else(|| panic!("recording field:\n{capture}"));
    let rec_at = rec_line.find("● REC").expect("REC");
    let idle_at = rec_line
        .find('…')
        .unwrap_or_else(|| panic!("idle chord:\n{rec_line}"));
    let rec_end = rec_at + "● REC".len();
    assert!(
        idle_at > rec_end + 4,
        "REC overlays the left, chord stays centered:\n{rec_line}"
    );
    assert!(capture.contains("Close pane"), "{capture}");
    assert_eq!(
        backend.focused_key().map(|key| key.as_ref()),
        Some("rozi-keybinding-capture")
    );
    assert!(backend.modifier_key_reporting_enabled());
    backend
        .set_held_modifiers(KeyMods::CTRL)
        .expect("show held Ctrl");
    let held_ctrl = frame(&mut backend);
    assert!(held_ctrl.contains("Ctrl+"), "{held_ctrl}");
    backend
        .set_held_modifiers(KeyMods {
            ctrl: true,
            shift: true,
            ..KeyMods::NONE
        })
        .expect("show held Ctrl+Shift");
    let held_ctrl_shift = frame(&mut backend);
    assert!(held_ctrl_shift.contains("Ctrl+Shift+"), "{held_ctrl_shift}");
    backend
        .set_held_modifiers(KeyMods::NONE)
        .expect("clear released modifiers");

    // Bare Enter expands through the input scheme, where New pane already owns it.
    backend
        .send_key(KeyEvent {
            code: KeyCode::Enter,
            mods: KeyMods::NONE,
        })
        .expect("capture conflicting binding");
    let conflict = frame(&mut backend);
    assert!(!backend.modifier_key_reporting_enabled());
    assert!(conflict.contains("╭Keybindings─"), "{conflict}");
    assert!(conflict.contains("Change keybinding"), "{conflict}");
    assert!(conflict.contains("Already bound to"), "{conflict}");
    assert!(
        conflict.contains("Replace existing keybinding?"),
        "{conflict}"
    );
    assert!(conflict.contains("New pane"), "{conflict}");
    assert!(conflict.contains("replace Enter"), "{conflict}");
    assert_eq!(
        backend.focused_key().map(|key| key.as_ref()),
        Some("rozi-keybinding-capture")
    );

    backend
        .send_key(KeyEvent {
            code: KeyCode::Esc,
            mods: KeyMods::NONE,
        })
        .expect("return from warning to capture");
    let retry = frame(&mut backend);
    assert!(backend.modifier_key_reporting_enabled());
    assert!(retry.contains("Change keybinding"), "{retry}");
    assert!(retry.contains("● REC"), "{retry}");
    assert!(!retry.contains("Already bound to"), "{retry}");
    backend
        .send_key(KeyEvent {
            code: KeyCode::Esc,
            mods: KeyMods::NONE,
        })
        .expect("cancel capture");
    let editor = frame(&mut backend);
    assert!(!backend.modifier_key_reporting_enabled());
    assert!(editor.contains("╭Keybindings─"), "{editor}");
    assert!(!editor.contains("● REC"), "{editor}");

    // Enter confirms the captured conflict without moving focus out of the recorder.
    backend
        .dispatch(rozi::Msg::KeybindingCapture("close".to_string()))
        .expect("capture conflicting binding again");
    frame(&mut backend);
    for _ in 0..2 {
        backend
            .send_key(KeyEvent {
                code: KeyCode::Enter,
                mods: KeyMods::NONE,
            })
            .expect("propose and confirm conflicting binding");
        backend.render();
    }
    let expanded_enter = ["ctrl+a enter", "alt+enter"]
        .map(|binding| KeyBinding::from_str(binding).unwrap())
        .to_vec();
    assert_eq!(
        backend.state().config.key_overrides["close"],
        expanded_enter
    );
    assert!(backend.state().config.key_overrides["spawn"].is_empty());
    backend
        .dispatch(rozi::Msg::KeybindingReset("close".to_string()))
        .expect("restore close after conflict test");
    backend
        .dispatch(rozi::Msg::KeybindingReset("spawn".to_string()))
        .expect("restore new pane after conflict test");

    backend
        .dispatch(rozi::Msg::KeybindingCapture("close".to_string()))
        .expect("capture a free binding");
    frame(&mut backend);
    backend
        .send_key(KeyEvent {
            code: KeyCode::F(11),
            mods: KeyMods::CTRL,
        })
        .expect("capture free binding");
    assert!(!backend.modifier_key_reporting_enabled());
    let review = frame(&mut backend);
    assert!(review.contains("Ctrl+F11"), "{review}");
    assert!(review.contains("save Enter"), "{review}");
    assert!(review.contains("record again Esc"), "{review}");
    // With a candidate on screen, a stray key does not replace it.
    send(&mut backend, KeyCode::Char('x'));
    let still = frame(&mut backend);
    assert!(still.contains("Ctrl+F11"), "{still}");
    assert!(!still.contains("● REC"), "{still}");
    backend
        .send_key(KeyEvent {
            code: KeyCode::Enter,
            mods: KeyMods::NONE,
        })
        .expect("save free binding");
    let overridden = frame(&mut backend);
    assert!(overridden.contains("Ctrl+F11 ← w"), "{overridden}");
    let saved = KeyBinding::from_str("ctrl+f11").unwrap();
    assert_eq!(backend.state().config.key_overrides["close"], vec![saved]);

    // Saving returns to the search field with the edited row still highlighted, so the row
    // actions work straight from the keyboard.
    assert_eq!(
        backend.focused_key().map(|key| key.as_ref()),
        Some("rozi-help-filter")
    );
    assert_eq!(selected(&backend).as_deref(), Some("close"));
    send_ctrl(&mut backend, 'u');
    assert!(backend.state().config.key_overrides["close"].is_empty());
    // An unbound action lives on the Unbound tab, where the query still finds it first.
    send(&mut backend, KeyCode::Right);
    send(&mut backend, KeyCode::Right);
    let unbound_override = frame(&mut backend);
    assert!(unbound_override.contains("— ← w"), "{unbound_override}");
    send_ctrl(&mut backend, 'd');
    assert!(!backend.state().config.key_overrides.contains_key("close"));

    send(&mut backend, KeyCode::Esc);
    assert!(backend.state().keybindings.is_none());
}

#[test]
fn reset_all_card_sits_one_row_below_keybindings() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend();
            backend.state_mut().config.key_sources.insert(
                "close".into(),
                rozi::config::KeyOverrideSpec::replace(vec![]),
            );
            backend
                .dispatch(rozi::Msg::KeybindingResetAll)
                .expect("open reset all");
            let shown = frame(&mut backend);
            assert!(shown.contains("Reset all keybindings?"), "{shown}");
            assert_eq!(
                frame_y(&backend, "Reset all keybindings?"),
                frame_y(&backend, "Keybindings") + 1,
                "reset all sits one row below Keybindings, like Change keybinding"
            );
        })
        .expect("spawn reset-all offset smoke thread")
        .join()
        .expect("reset-all offset smoke completes");
}

#[test]
fn unbind_keeps_the_highlight_on_the_next_row() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend();
            backend
                .dispatch(rozi::Msg::KeybindingSelect("close".to_string()))
                .expect("select close");
            backend
                .dispatch(rozi::Msg::KeybindingUnbind("close".to_string()))
                .expect("unbind close");
            assert!(backend.state().config.key_overrides["close"].is_empty());
            assert_eq!(
                selected(&backend).as_deref(),
                Some("toggle-float"),
                "unbind on Global must not fall back to Prefix"
            );
            send(&mut backend, KeyCode::Down);
            assert_eq!(
                selected(&backend).as_deref(),
                Some("toggle-fullscreen"),
                "Down should step from the neighbor, not from Prefix"
            );
        })
        .expect("spawn unbind-selection smoke thread")
        .join()
        .expect("unbind-selection smoke completes");
}

#[test]
fn recording_the_prefix_names_prefix_not_every_command() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend();
            backend
                .dispatch(rozi::Msg::KeybindingCapture("close".to_string()))
                .expect("capture close");
            frame(&mut backend);
            send_ctrl(&mut backend, 'a');
            let conflict = frame(&mut backend);
            assert!(conflict.contains("Already bound to Prefix"), "{conflict}");
            assert!(conflict.contains("Keybinding unavailable"), "{conflict}");
            assert!(!conflict.contains("replace Enter"), "{conflict}");
            assert!(!conflict.contains("config-only"), "{conflict}");
            let stage = &backend
                .state()
                .keybindings
                .as_ref()
                .expect("overlay open")
                .stage;
            match stage {
                rozi::state::KeybindingEditorStage::Conflict {
                    conflicts,
                    replaceable,
                    ..
                } => {
                    assert_eq!(conflicts.len(), 1, "{conflicts:?}");
                    assert_eq!(conflicts[0].label, "Prefix");
                    assert!(!*replaceable);
                }
                other => panic!("expected prefix conflict, got {other:?}"),
            }
        })
        .expect("spawn prefix-conflict smoke thread")
        .join()
        .expect("prefix-conflict smoke completes");
}

fn send_ctrl(backend: &mut TestBackend<AppRoot>, ch: char) {
    backend
        .send_key(KeyEvent {
            code: KeyCode::Char(ch),
            mods: KeyMods::CTRL,
        })
        .expect("send ctrl key");
    backend.render();
}
