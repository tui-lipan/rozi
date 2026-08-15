//! The which-key strip is chrome driven entirely by framework chord state, so the things worth
//! pinning are the edges where it appears, disappears, and changes shape: it must not paint when no
//! chord is pending, it must collapse the directional families, it must drop the relational
//! commands in an unsplit workspace, and `[input] which_key = "off"` must remove it completely.

use std::time::Duration;

use rozi::AppRoot;
use rozi::config::WhichKey;
use tui_lipan::TestBackend;
use tui_lipan::prelude::*;

const VIEWPORT: Rect = Rect {
    x: 0,
    y: 0,
    w: 120,
    h: 30,
};

fn prefix() -> KeyEvent {
    KeyEvent {
        code: KeyCode::Char('a'),
        mods: KeyMods::CTRL,
    }
}

fn live_pane(id: u32) -> rozi::state::Pane {
    let mut pane = rozi::state::Pane::new(
        id,
        100,
        FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 10.0,
        },
    );
    pane.opening = false;
    pane.terminal_active = true;
    pane
}

/// `AppCommandsFirst` mirrors `main.rs`. It is load-bearing: under `WidgetFirst` the framework
/// resets a chord as soon as it goes pending, so nothing would ever be pending to draw.
fn backend(panes: usize, which_key: WhichKey) -> TestBackend<AppRoot> {
    backend_with_delay(panes, which_key, Duration::ZERO)
}

fn backend_with_delay(
    panes: usize,
    which_key: WhichKey,
    reveal_delay: Duration,
) -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let app = App::new()
        .key_dispatch_policy(KeyDispatchPolicy::AppCommandsFirst)
        .terminal_key_policy(TerminalKeyPolicy::AppCommandsThenTerminal)
        .command_chord_reveal_delay(reveal_delay);
    let mut backend = TestBackend::new_with_app(app, AppRoot::default(), ());
    backend.set_viewport(VIEWPORT);
    {
        let state = backend.state_mut();
        state.config.input.which_key = which_key;
        let pane = &mut state.current_mut().workspaces[0].panes[0];
        pane.opening = false;
        pane.terminal_active = true;
        for index in 1..panes {
            let id = 200 + index as u32;
            state.current_mut().workspaces[0].panes.push(live_pane(id));
        }
    }
    backend.render();
    backend.focus_next();
    backend
}

fn rendered(backend: &mut TestBackend<AppRoot>) -> String {
    backend.render();
    backend.capture_ui_snapshot().to_markdown()
}

fn on_deep_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(body)
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
}

#[test]
fn strip_appears_only_while_a_chord_is_pending() {
    on_deep_stack(|| {
        let mut backend = backend(3, WhichKey::Instant);
        assert!(
            !rendered(&mut backend).contains("New pane"),
            "no chord pending, so nothing to advertise"
        );

        backend.send_key(prefix()).expect("prefix goes pending");
        assert!(
            rendered(&mut backend).contains("New pane"),
            "a pending prefix should show what it can do next"
        );

        backend
            .send_key(KeyEvent {
                code: KeyCode::Esc,
                mods: KeyMods::NONE,
            })
            .expect("esc cancels the chord");
        assert!(
            !rendered(&mut backend).contains("New pane"),
            "cancelling the chord should take the strip with it"
        );
    });
}

#[test]
fn directional_families_render_as_one_row_each() {
    on_deep_stack(|| {
        let mut backend = backend(3, WhichKey::Instant);
        backend.send_key(prefix()).expect("prefix goes pending");
        let view = rendered(&mut backend);
        for collapsed in ["hjkl Focus pane", "HJKL Swap pane", "Ctrl+hjkl Move pane"] {
            assert!(
                view.contains(collapsed),
                "expected `{collapsed}` in:\n{view}"
            );
        }
        assert!(
            !view.contains("Focus left"),
            "a collapsed family should not also spell out its members"
        );
        assert!(
            view.contains("1-9 Workspace"),
            "the nine digit switches collapse the same way"
        );
    });
}

#[test]
fn an_unsplit_workspace_drops_the_relational_commands() {
    on_deep_stack(|| {
        let mut backend = backend(1, WhichKey::Instant);
        backend.send_key(prefix()).expect("prefix goes pending");
        let view = rendered(&mut backend);
        assert!(
            view.contains("New pane"),
            "the strip is still shown for a single pane"
        );
        for inert in ["Focus pane", "Swap pane", "Move pane", "Resize split"] {
            assert!(
                !view.contains(inert),
                "`{inert}` cannot do anything with one pane, so it should not be offered:\n{view}"
            );
        }
    });
}

#[test]
fn the_reveal_delay_holds_the_strip_back_without_hiding_the_prefix_badge() {
    on_deep_stack(|| {
        let delay = Duration::from_millis(60);
        let mut backend = backend_with_delay(3, WhichKey::Short, delay);
        backend.send_key(prefix()).expect("prefix goes pending");

        let held = rendered(&mut backend);
        assert!(
            !held.contains("New pane"),
            "the strip must wait out the delay:\n{held}"
        );
        assert!(
            held.contains("PREFIX"),
            "instant chrome does not wait - the badge is how you know the key landed"
        );

        std::thread::sleep(delay + Duration::from_millis(40));
        assert!(
            rendered(&mut backend).contains("New pane"),
            "the strip appears once the chord has been held past the delay"
        );
    });
}

/// The four named steps are the whole which-key UI, so their order, wrap, and durations are the
/// contract: `Off` must draw nothing, `Instant` must actually mean no wait, and `Long` must outlast
/// `Short`.
#[test]
fn it_steps_through_its_four_named_states() {
    assert_eq!(WhichKey::default(), WhichKey::Short);
    assert!(!WhichKey::Off.enabled());
    assert!(WhichKey::Instant.enabled());
    assert_eq!(WhichKey::Instant.reveal_delay(), Duration::ZERO);
    assert!(WhichKey::Long.reveal_delay() > WhichKey::Short.reveal_delay());

    let mut which_key = WhichKey::Off;
    for expected in [
        WhichKey::Instant,
        WhichKey::Short,
        WhichKey::Long,
        WhichKey::Off,
    ] {
        which_key = which_key.step(false);
        assert_eq!(which_key, expected, "forward cycle wraps through all four");
    }
    for expected in [
        WhichKey::Long,
        WhichKey::Short,
        WhichKey::Instant,
        WhichKey::Off,
    ] {
        which_key = which_key.step(true);
        assert_eq!(which_key, expected, "reverse cycle mirrors it");
    }

    for which_key in WhichKey::all() {
        assert_eq!(
            WhichKey::parse(which_key.id()),
            Some(*which_key),
            "every step round-trips through its config spelling"
        );
    }
    assert_eq!(WhichKey::parse("300ms"), None);
    assert_eq!(WhichKey::parse("false"), None);
}

#[test]
fn the_config_key_removes_it_entirely() {
    on_deep_stack(|| {
        let mut backend = backend(3, WhichKey::Off);
        backend.send_key(prefix()).expect("prefix goes pending");
        let view = rendered(&mut backend);
        assert!(
            !view.contains("New pane"),
            "`which_key = \"off\"` should draw nothing at all"
        );
        assert!(
            view.contains("PREFIX"),
            "the workbar badge is a separate affordance and stays"
        );
    });
}
