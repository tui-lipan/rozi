//! Visual reference for the which-key strip.
//!
//! The strip's whole design question is density: how much of the screen it eats, whether the packed
//! columns stay aligned when their labels differ in length, and whether the overflow count reads as
//! chrome rather than as a truncated row. None of that is answerable from an ASCII grid, so this
//! renders a pending prefix chord across the widths and pane counts that change the answer and
//! writes PNGs to `target/ui-sketches/`.
//!
//! ```bash
//! cargo test --features ui-snapshot --test which_key_visual -- --nocapture
//! ```
//!
//! Feature-gated: `ui-snapshot` pulls in the PNG encoder and fonts, so an ordinary `cargo test`
//! never builds any of it.
#![cfg(feature = "ui-snapshot")]

use rozi::AppRoot;
use rozi::state::Pane;
use tui_lipan::TestBackend;
use tui_lipan::prelude::*;

fn live_pane(id: u32) -> Pane {
    let mut pane = Pane::new(
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

fn prefix() -> KeyEvent {
    KeyEvent {
        code: KeyCode::Char('a'),
        mods: KeyMods::CTRL,
    }
}

/// `AppCommandsFirst` is what `main.rs` runs, and it is load-bearing here: under `WidgetFirst` the
/// framework resets a chord the moment it goes pending, so the strip would never appear.
fn backend(w: u16, h: u16, panes: usize, workbar_at_bottom: bool) -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let app = App::new()
        .key_dispatch_policy(KeyDispatchPolicy::AppCommandsFirst)
        .terminal_key_policy(TerminalKeyPolicy::AppCommandsThenTerminal);
    let mut backend = TestBackend::new_with_app(app, AppRoot::default(), ());
    backend.set_viewport(Rect { x: 0, y: 0, w, h });
    {
        let state = backend.state_mut();
        state.config.pane.workbar_at_bottom = workbar_at_bottom;
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

fn capture(name: &str, w: u16, h: u16, panes: usize, workbar_at_bottom: bool) {
    let mut backend = backend(w, h, panes, workbar_at_bottom);
    backend.send_key(prefix()).expect("prefix goes pending");
    backend.render();
    let png = backend
        .capture_ui_snapshot()
        .to_png_default()
        .expect("encode which-key png");
    let dir = std::path::Path::new("target/ui-sketches");
    std::fs::create_dir_all(dir).expect("create sketch dir");
    let path = dir.join(format!("{name}.png"));
    std::fs::write(&path, png).expect("write which-key png");
    println!("wrote {}", path.display());
}

#[test]
fn which_key_strip_across_widths_and_pane_counts() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            // One pane: the relational commands drop out, which is the smallest the strip gets.
            capture("which-key-80x24-single", 80, 24, 1, true);
            capture("which-key-120x30-single", 120, 30, 1, true);
            // Split workspace: every directional family is live, which is the widest case.
            capture("which-key-80x24-split", 80, 24, 3, true);
            capture("which-key-120x30-split", 120, 30, 3, true);
            capture("which-key-160x40-split", 160, 40, 3, true);
            // Workbar on top: the strip has to follow it rather than assume the bottom edge.
            capture("which-key-120x30-topbar", 120, 30, 3, false);
        })
        .expect("spawn which-key visual thread")
        .join()
        .expect("which-key visual thread panicked");
}
