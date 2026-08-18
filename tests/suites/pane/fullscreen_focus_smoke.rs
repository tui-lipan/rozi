//! Fullscreen is a lock, not just a size.
//!
//! A fullscreen pane covers every tile behind it, so anything that moves the focus off it — or
//! spawns a pane beside it — would otherwise leave the keyboard on a pane nobody can see. Moving,
//! resizing, and split dragging already refuse while fullscreen; these cover the two paths that
//! did not.

use rozi::input::Action;
use rozi::state::{Direction, Pane};
use rozi::{AppRoot, Msg};
use tui_lipan::TestBackend;
use tui_lipan::prelude::Rect;
use tui_lipan::style::geometry::FloatRect;

/// A pane in the state a live one is in: terminal up, open animation finished. `request_pane_focus`
/// no-ops on an `opening` or inactive pane, so defaults straight out of `Pane::new` cannot hold
/// focus and would mask a regression here.
fn pane(id: u32) -> Pane {
    let mut pane = Pane::new(
        id,
        100,
        FloatRect {
            x: 0.0,
            y: 0.0,
            w: 20.0,
            h: 10.0,
        },
    );
    pane.opening = false;
    pane.terminal_active = true;
    pane
}

fn settle(backend: &mut TestBackend<AppRoot>) {
    for _ in 0..4 {
        backend.render();
        let _ = backend.pump();
    }
    backend.render();
}

/// Two tiled panes with the focused one covering the workspace.
fn backend_with_fullscreen() -> TestBackend<AppRoot> {
    // `Action::Spawn` writes the shell-integration scripts, which belong in a scratch cache rather
    // than the one a developer's running rozi injects from (`rozi::test_support`).
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 100,
        h: 30,
    });
    {
        let state = backend.state_mut();
        state.current_mut().workspaces[0].panes = vec![pane(1), pane(2)];
        state.current_mut().workspaces[0].panes[0].fullscreen = true;
        state.current_mut().workspaces[0].focused_pane = Some(1);
        state.current_mut().focused_pane = Some(1);
        state.current_mut().next_pane_id = 3;
    }
    backend.render();
    backend
}

fn fullscreen_ids(backend: &TestBackend<AppRoot>) -> Vec<u32> {
    backend.state().current().workspaces[0]
        .panes
        .iter()
        .filter(|pane| pane.fullscreen && !pane.closing)
        .map(|pane| pane.id)
        .collect()
}

/// Spawning used to drop the new pane into the tiling behind the fullscreen pane and focus it, so
/// the next keystroke went to a pane the user could not see. The new pane takes the fullscreen over.
#[test]
fn spawning_while_fullscreen_hands_the_screen_to_the_new_pane() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_fullscreen();

            backend
                .dispatch(Msg::RunAction(Action::Spawn))
                .expect("spawn a pane");
            settle(&mut backend);

            let focused = backend
                .state()
                .current()
                .focused_pane
                .expect("a spawn takes focus");
            assert_ne!(focused, 1, "the spawned pane takes focus");
            assert_eq!(
                fullscreen_ids(&backend),
                vec![focused],
                "the focused pane must be the one covering the workspace"
            );
        })
        .expect("spawn fullscreen thread")
        .join()
        .expect("fullscreen spawn completes");
}

/// Directional focus walks the tiled placements, which are all hidden while a pane is fullscreen.
#[test]
fn directional_focus_is_locked_while_fullscreen() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_fullscreen();

            for direction in [
                Direction::Left,
                Direction::Right,
                Direction::Up,
                Direction::Down,
            ] {
                backend
                    .dispatch(Msg::RunAction(Action::Focus(direction)))
                    .expect("focus action");
                settle(&mut backend);
                assert_eq!(
                    backend.state().current().focused_pane,
                    Some(1),
                    "focus must stay on the fullscreen pane going {direction:?}"
                );
            }

            // Toggling fullscreen off releases the lock.
            backend
                .dispatch(Msg::RunAction(Action::ToggleFullscreen))
                .expect("leave fullscreen");
            settle(&mut backend);
            backend
                .dispatch(Msg::RunAction(Action::Focus(Direction::Right)))
                .expect("focus action");
            settle(&mut backend);
            assert_eq!(
                backend.state().current().focused_pane,
                Some(2),
                "leaving fullscreen restores normal navigation"
            );
        })
        .expect("spawn focus lock thread")
        .join()
        .expect("focus lock completes");
}
