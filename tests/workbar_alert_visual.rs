//! Visual reference for workspace-tab alert markers.
//!
//! Alert markers are a colour decision, and colour is the one thing an ASCII grid cannot show. This
//! renders the workbar with a blocked and a finished background workspace at both ends of the
//! breathe and writes PNGs to `target/ui-sketches/`, so the peak and trough can be compared side by
//! side instead of judged from a description.
//!
//! ```bash
//! cargo test --features ui-snapshot --test workbar_alert_visual -- --nocapture
//! ```
//!
//! Feature-gated: `ui-snapshot` pulls in the PNG encoder and fonts, so an ordinary `cargo test`
//! never builds any of it.
#![cfg(feature = "ui-snapshot")]

use hyprmux::HyprmuxApp;
use hyprmux::state::{AlertMode, Pane};
use tui_lipan::TestBackend;
use tui_lipan::core::event::{MouseEvent, MouseKind};
use tui_lipan::prelude::{FloatRect, KeyMods, Rect};

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

/// Workspace 1 active and quiet, 2 blocked, 3 finished-unseen: one tab per marker plus an unmarked
/// neighbour, which is what makes "is this subtle enough" answerable at a glance.
fn workbar_backend(phase: bool, calm_phase: bool) -> TestBackend<HyprmuxApp> {
    hyprmux::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(HyprmuxApp::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 80,
        h: 6,
    });
    let state = backend.state_mut();
    state.config.pane.show_workbar = true;
    state.config.workbar.alert.mode = AlertMode::Pulse;
    state.alert_pulse_armed = true;
    state.alert_pulse_phase = phase;
    state.alert_pulse_calm_phase = calm_phase;

    state.current_mut().workspaces[0].panes.push(live_pane(10));
    state.current_mut().workspaces[0].focused_pane = Some(10);
    state.current_mut().focused_pane = Some(10);

    let mut blocked = live_pane(11);
    blocked.terminal.reported_status = Some(hyprmux::session::protocol::PaneStatus {
        value: "blocked".into(),
        reason: None,
        set_at: 0,
    });
    state.current_mut().workspaces[1].panes.push(blocked);

    let mut finished = live_pane(12);
    finished.terminal.finished_unseen = true;
    state.current_mut().workspaces[2].panes.push(finished);

    backend
}

fn capture(name: &str, phase: bool, calm_phase: bool) {
    capture_at(name, phase, calm_phase, None);
}

/// `hover_x` puts the mouse over a tab. Hover is the case worth capturing because the hover style
/// layers over whatever the tab already resolved to: an absolute colour there silently discards the
/// alert, and only a rendered frame shows whether it survived.
fn capture_at(name: &str, phase: bool, calm_phase: bool, hover_x: Option<u16>) {
    let mut backend = workbar_backend(phase, calm_phase);
    if let Some(x) = hover_x {
        backend.render();
        backend
            .send_mouse(MouseEvent {
                x,
                y: 0,
                kind: MouseKind::Moved,
                mods: KeyMods::NONE,
            })
            .expect("hover the workbar");
    }
    backend.render();
    let png = backend
        .capture_ui_snapshot()
        .to_png_default()
        .expect("encode workbar png");
    let dir = std::path::Path::new("target/ui-sketches");
    std::fs::create_dir_all(dir).expect("create sketch dir");
    let path = dir.join(format!("{name}.png"));
    std::fs::write(&path, png).expect("write workbar png");
    println!("wrote {}", path.display());
}

#[test]
fn workbar_alert_markers_at_both_breathe_ends() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            // Blocked runs at the urgent rate and finished at the calm one, so the interesting
            // frames are the three the two rates actually produce together.
            capture("workbar-alert-both-peak", false, false);
            capture("workbar-alert-urgent-trough", true, false);
            capture("workbar-alert-both-trough", true, true);
            // Hovering the blocked tab must lift its colour, not replace it.
            capture_at("workbar-alert-hovered", false, false, Some(21));
        })
        .expect("spawn workbar visual thread")
        .join()
        .expect("workbar visual completes");
}
