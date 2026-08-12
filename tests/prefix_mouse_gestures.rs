use hyprmux::AppRoot;
use tui_lipan::TestBackend;
use tui_lipan::core::event::{MouseButton, MouseKind};
use tui_lipan::prelude::*;

const VIEWPORT: Rect = Rect {
    x: 0,
    y: 0,
    w: 100,
    h: 30,
};

fn prefix() -> KeyEvent {
    KeyEvent {
        code: KeyCode::Char('a'),
        mods: KeyMods::CTRL,
    }
}

fn mouse(x: u16, y: u16, kind: MouseKind) -> MouseEvent {
    MouseEvent {
        x,
        y,
        kind,
        mods: KeyMods::NONE,
    }
}

fn backend() -> TestBackend<AppRoot> {
    let app = App::new()
        .key_dispatch_policy(KeyDispatchPolicy::AppCommandsFirst)
        .terminal_key_policy(TerminalKeyPolicy::AppCommandsThenTerminal);
    let mut backend = TestBackend::new_with_app(app, AppRoot::default(), ());
    backend.set_viewport(VIEWPORT);
    {
        let pane = &mut backend.state_mut().current_mut().workspaces[0].panes[0];
        pane.opening = false;
        pane.terminal_active = true;
    }
    backend.render();
    backend.focus_next();
    backend
}

fn on_deep_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(body)
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
}

#[test]
fn prefix_controls_pane_drag_and_resize_until_release() {
    on_deep_stack(|| {
        let mut backend = backend();

        backend
            .send_key(prefix())
            .expect("prefix enters command mode");
        backend
            .send_mouse(mouse(20, 12, MouseKind::Down(MouseButton::Left)))
            .expect("left mouse down");
        backend
            .send_mouse(mouse(24, 12, MouseKind::Drag(MouseButton::Left)))
            .expect("left mouse drag");
        assert!(
            backend.state().moving_pane.is_some(),
            "prefix should enable pane movement"
        );
        backend
            .send_mouse(mouse(24, 12, MouseKind::Up(MouseButton::Left)))
            .expect("left mouse release");
        assert!(
            backend.state().moving_pane.is_none(),
            "left release should finish the prefix gesture"
        );

        backend
            .send_key(prefix())
            .expect("prefix re-enters command mode");
        backend
            .send_mouse(mouse(20, 12, MouseKind::Down(MouseButton::Right)))
            .expect("right mouse down");
        backend
            .send_mouse(mouse(24, 12, MouseKind::Drag(MouseButton::Right)))
            .expect("right mouse drag");
        assert!(
            backend.state().resizing_pane.is_some(),
            "prefix should enable pane resizing"
        );
        backend
            .send_mouse(mouse(24, 12, MouseKind::Up(MouseButton::Right)))
            .expect("right mouse release");
        assert!(
            backend.state().resizing_pane.is_none(),
            "right release should finish the prefix gesture"
        );
    });
}
