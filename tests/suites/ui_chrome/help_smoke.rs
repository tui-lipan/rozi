//! The Keybindings modal is sized by its list, not by the viewport. Filtering down to one row must
//! shrink it, a list too long for the viewport must stop at the cap and scroll, the vertical arrows
//! must be what scrolls it, and `Tab`/`Shift+Tab` and the horizontal arrows must walk the tabs.

use rozi::AppRoot;
use tui_lipan::TestBackend;
use tui_lipan::prelude::{KeyCode, KeyEvent, KeyMods, Rect};

/// Isolated per `AGENTS.md`: building an `AppRoot` otherwise resolves the developer's own config
/// and state directories.
fn help_backend(w: u16, h: u16) -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect { x: 0, y: 0, w, h });
    backend.state_mut().show_help = true;
    backend
}

fn frame(backend: &mut TestBackend<AppRoot>) -> String {
    backend.render();
    backend.capture_frame().to_fixed_grid_lines().join("\n")
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

    // The arrows are what scrolls the capped list.
    backend
        .send_key(KeyEvent {
            code: KeyCode::Down,
            mods: KeyMods::NONE,
        })
        .expect("scroll the keybinding list down a row");
    let scrolled = frame(&mut backend);
    assert_ne!(full, scrolled, "Down does not scroll the list:\n{full}");
    backend
        .send_key(KeyEvent {
            code: KeyCode::Up,
            mods: KeyMods::NONE,
        })
        .expect("scroll the keybinding list back up");
    assert_eq!(
        full,
        frame(&mut backend),
        "Up does not scroll the list back"
    );

    // Tab, Shift+Tab, and the horizontal arrows walk the tab strip, wrapping at both ends.
    for (key, expected) in [
        (KeyCode::Tab, rozi::state::HelpTab::Modes),
        (KeyCode::Right, rozi::state::HelpTab::Unbound),
        (KeyCode::Tab, rozi::state::HelpTab::All),
        (KeyCode::Tab, rozi::state::HelpTab::Global),
        (KeyCode::Left, rozi::state::HelpTab::All),
        (KeyCode::BackTab, rozi::state::HelpTab::Unbound),
    ] {
        backend
            .send_key(KeyEvent {
                code: key,
                mods: KeyMods::NONE,
            })
            .expect("step the keybinding tab strip");
        backend.render();
        assert_eq!(backend.state().help_tab, expected, "after {key:?}");
    }
    let unbound = frame(&mut backend);
    assert!(
        unbound.contains("—"),
        "the Unbound tab is not shown:\n{unbound}"
    );
    backend
        .dispatch(rozi::Msg::HelpTabSelected(0))
        .expect("return to the Global tab");

    // Filtering to a handful of rows shrinks the modal instead of leaving it open at the cap.
    backend
        .send_key(KeyEvent {
            code: KeyCode::Char('/'),
            mods: KeyMods::NONE,
        })
        .expect("focus the keybinding filter");
    for character in "scratch".chars() {
        backend
            .send_key(KeyEvent {
                code: KeyCode::Char(character),
                mods: KeyMods::NONE,
            })
            .expect("type a keybinding filter");
    }
    let filtered = frame(&mut backend);
    assert!(filtered.contains("Enable scratchpad"), "{filtered}");
    assert!(
        modal_rows(&filtered) < capped,
        "a filtered list leaves the modal at its cap:\n{filtered}"
    );
}
