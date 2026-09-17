//! The Keybindings modal is sized by its list, not by the viewport. Filtering down to one row must
//! shrink it, a list too long for the viewport must stop at the cap and follow the selection, and
//! the search field must keep focus while `Tab`/`Shift+Tab` and the horizontal arrows walk the tabs.

use rozi::AppRoot;
use tui_lipan::TestBackend;
use tui_lipan::prelude::{KeyCode, KeyEvent, KeyMods, Rect};

/// Isolated per `AGENTS.md`: building an `AppRoot` otherwise resolves the developer's own config
/// and state directories.
fn help_backend(w: u16, h: u16) -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect { x: 0, y: 0, w, h });
    backend
        .dispatch(rozi::Msg::RunAction(rozi::input::Action::ToggleHelp))
        .expect("open keybindings");
    backend
}

fn frame(backend: &mut TestBackend<AppRoot>) -> String {
    backend.render();
    backend.capture_frame().to_fixed_grid_lines().join("\n")
}

fn press(backend: &mut TestBackend<AppRoot>, code: KeyCode) {
    backend
        .send_key(KeyEvent {
            code,
            mods: KeyMods::NONE,
        })
        .expect("send key");
    backend.render();
}

fn tab(backend: &TestBackend<AppRoot>) -> rozi::state::HelpTab {
    backend
        .state()
        .keybindings
        .as_ref()
        .expect("keybindings overlay is open")
        .tab
}

/// Rows the modal's own frame spans, found by the two border rows its rounded corners draw.
fn modal_rows(frame: &str) -> usize {
    let lines: Vec<&str> = frame.lines().collect();
    let top = lines
        .iter()
        .position(|line| line.contains("╭Keybindings"))
        .expect("modal top border");
    let bottom = lines
        .iter()
        .rposition(|line| line.contains('╰'))
        .expect("modal bottom border");
    bottom - top + 1
}

#[test]
fn the_keybindings_modal_is_sized_by_its_list_and_scrolls_once_capped() {
    // The full keybinding list is a deep element tree; the default test stack overflows building
    // it, the same way the extensions manager's does.
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(body)
        .expect("spawn help smoke thread")
        .join()
        .expect("help smoke completes");
}

fn body() {
    let mut backend = help_backend(110, 50);
    let full = frame(&mut backend);
    assert!(full.contains("Keybindings"), "{full}");
    // The unfiltered list outgrows the viewport, so the modal stops at its 70% cap.
    let capped = modal_rows(&full);
    assert_eq!(
        capped, 35,
        "unfiltered modal is not at the 70% cap:\n{full}"
    );

    // The arrows move the highlight, and the capped list scrolls to keep it visible.
    let selected = |backend: &TestBackend<AppRoot>| {
        backend
            .state()
            .keybindings
            .as_ref()
            .and_then(|keybindings| keybindings.selected.clone())
    };
    press(&mut backend, KeyCode::Down);
    assert!(
        selected(&backend).is_some_and(|id| id.contains("Mod")),
        "Down does not move the selection"
    );
    press(&mut backend, KeyCode::Up);
    assert!(
        selected(&backend).is_some_and(|id| id.contains("Prefix")),
        "Up does not move it back"
    );
    press(&mut backend, KeyCode::End);
    let bottom = frame(&mut backend);
    assert!(
        !bottom.contains("Prefix · then key"),
        "End does not scroll the list to its last row:\n{bottom}"
    );
    press(&mut backend, KeyCode::Home);
    assert_eq!(full, frame(&mut backend), "Home does not return to the top");
    press(&mut backend, KeyCode::Up);
    assert!(
        selected(&backend).is_some_and(|id| !id.contains("Prefix")),
        "Up from the first row wraps to the last"
    );
    press(&mut backend, KeyCode::Down);
    assert!(
        selected(&backend).is_some_and(|id| id.contains("Prefix")),
        "Down from the last row wraps to the first"
    );

    // Tab, Shift+Tab, and the horizontal arrows walk the tab strip, wrapping at both ends, and
    // focus never leaves the search field.
    for (key, expected) in [
        (KeyCode::Tab, rozi::state::HelpTab::Modes),
        (KeyCode::Right, rozi::state::HelpTab::Unbound),
        (KeyCode::Tab, rozi::state::HelpTab::All),
        (KeyCode::Tab, rozi::state::HelpTab::Global),
        (KeyCode::Left, rozi::state::HelpTab::All),
        (KeyCode::BackTab, rozi::state::HelpTab::Unbound),
    ] {
        press(&mut backend, key);
        assert_eq!(tab(&backend), expected, "after {key:?}");
        assert_eq!(
            backend.focused_key().map(|key| key.as_ref()),
            Some("rozi-help-filter"),
            "after {key:?}"
        );
    }
    let unbound = frame(&mut backend);
    assert!(
        unbound.contains("—"),
        "the Unbound tab is not shown:\n{unbound}"
    );
    backend
        .dispatch(rozi::Msg::HelpTabSelected(0))
        .expect("return to the Global tab");

    // Filtering to a handful of rows shrinks the modal instead of leaving it open at the cap, and
    // letters that used to be commands are plain query text.
    for character in "scratch".chars() {
        press(&mut backend, KeyCode::Char(character));
    }
    assert_eq!(tab(&backend), rozi::state::HelpTab::Global);
    let filtered = frame(&mut backend);
    assert!(filtered.contains("Enable scratchpad"), "{filtered}");
    assert!(
        modal_rows(&filtered) < capped,
        "a filtered list leaves the modal at its cap:\n{filtered}"
    );
}

#[test]
fn keybindings_footer_keeps_wrapped_hints_visible() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(wrapped_hints_body)
        .expect("spawn wrapped hints smoke thread")
        .join()
        .expect("wrapped hints smoke completes");
}

fn wrapped_hints_body() {
    let mut backend = help_backend(110, 50);
    backend.state_mut().config.key_sources.insert(
        "close".into(),
        rozi::config::KeyOverrideSpec::replace(vec![]),
    );
    backend
        .state_mut()
        .config
        .key_overrides
        .insert("close".into(), vec![]);
    for character in "close pane".chars() {
        press(&mut backend, KeyCode::Char(character));
    }
    press(&mut backend, KeyCode::Down);
    press(&mut backend, KeyCode::Up);
    let footer = frame(&mut backend);
    for hint in [
        "change Enter",
        "unbind Ctrl+U",
        "reset Ctrl+D",
        "reset all Ctrl+R",
    ] {
        assert!(
            footer.contains(hint),
            "wrapped footer misses `{hint}`:\n{footer}"
        );
    }
}
