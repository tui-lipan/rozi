//! The hover-revealed ✕ on Panes and Sessions rows.

use hyprmux::HyprmuxApp;
use hyprmux::config::{SidebarTab, SidebarTabId};
use hyprmux::state::SidebarClose;
use tui_lipan::TestBackend;
use tui_lipan::core::event::{MouseEvent, MouseKind};
use tui_lipan::prelude::{KeyMods, Rect};

/// Park the pointer at `(x, y)` and let the frames that crossing asks for run.
fn move_to(backend: &mut TestBackend<HyprmuxApp>, x: u16, y: u16) {
    backend
        .send_mouse(MouseEvent {
            x,
            y,
            kind: MouseKind::Moved,
            mods: KeyMods::NONE,
        })
        .expect("move the pointer");
    settle(backend);
}

fn settle(backend: &mut TestBackend<HyprmuxApp>) {
    for _ in 0..4 {
        backend.render();
        let _ = backend.pump();
    }
    backend.render();
}

/// Render the Panes tab after `setup` has adjusted state, returning the sidebar's columns.
fn panes_sidebar_lines(
    setup: impl FnOnce(&mut hyprmux::state::State) + Send + 'static,
) -> Vec<String> {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            });
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("panes"));
                state.config.sidebar.tabs = vec![SidebarTab::Panes];
                setup(state);
            }
            backend.render();
            backend
                .capture_frame()
                .to_fixed_grid_lines()
                .iter()
                .map(|line| line.chars().take(32).collect())
                .collect()
        })
        .expect("spawn sidebar close smoke thread")
        .join()
        .expect("sidebar close smoke completes")
}

/// The row index of the only pane on the Panes tab: row 0 is the workspace header.
const PANE_ROW: usize = 1;

/// A resting list shows no ✕ at all — nothing advertises a destructive action it is not being aimed
/// at. Hovering the row reveals one, and it takes the badge's slot rather than fighting it for the
/// narrow column.
#[test]
fn the_close_affordance_is_revealed_by_hover_and_hidden_at_rest() {
    let resting = panes_sidebar_lines(|_| {});
    assert!(
        !resting.iter().any(|line| line.contains('✕')),
        "a resting Panes tab shows no ✕: {resting:#?}"
    );

    let hovered = panes_sidebar_lines(|state| {
        state.sidebar.panels[0].hovered_row = Some(PANE_ROW);
    });
    assert!(
        hovered.iter().any(|line| line.contains('✕')),
        "hovering a pane row reveals its ✕: {hovered:#?}"
    );
}

/// Keyboard navigation parks a stale pointer over whatever row it was last on. The ✕ is gated on the
/// same `suppress_row_hover` flag as the row's hover lift, so it does not linger there.
#[test]
fn keyboard_navigation_suppresses_a_stale_hovered_close_affordance() {
    let lines = panes_sidebar_lines(|state| {
        state.sidebar.panels[0].hovered_row = Some(PANE_ROW);
        state.sidebar.panels[0].suppress_row_hover = true;
    });
    assert!(
        !lines.iter().any(|line| line.contains('✕')),
        "a suppressed hover shows no ✕: {lines:#?}"
    );
}

/// The ✕ is a nested MouseRegion so it can own clicks and its red foreground hover. Moving onto it
/// must not drop the parent row's background lift: both effects compose while the pointer is there.
///
/// Every cell of the ✕ is the same target — the glyph and the padding cell beside it that holds it
/// off the panel edge — so the row must read identically on both. The pointer drives the whole
/// thing, hover state included: seeding `hovered_row` by hand would bypass the crossing this is
/// about and leave the baseline reading an unhovered row.
#[test]
fn hovering_the_x_keeps_the_row_hover_and_adds_the_x_hover() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            });
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("panes"));
                state.config.sidebar.tabs = vec![SidebarTab::Panes];
            }
            settle(&mut backend);

            let y = backend
                .capture_frame()
                .to_fixed_grid_lines()
                .iter()
                .position(|line| line.starts_with('▍'))
                .expect("the active pane's row, marked in the gutter") as u16;

            // Park the pointer on the row body: that is what reveals the ✕ and what the row's own
            // hover lift has to survive.
            move_to(&mut backend, 4, y);
            let lines = backend.capture_frame().to_fixed_grid_lines();
            let x = lines[y as usize]
                .chars()
                .position(|ch| ch == '✕')
                .expect("hovering the row reveals its ✕") as u16;
            let row_hover_bg = backend.capture_frame().cell(4, y).bg;
            let resting_x_fg = backend.capture_frame().cell(x, y).fg;
            let error = backend.state().theme.status.error;
            assert_ne!(resting_x_fg, error, "an unaimed-at ✕ is not red yet");

            for probe in [x, x + 1] {
                move_to(&mut backend, probe, y);
                let frame = backend.capture_frame();
                assert_eq!(
                    frame.cell(4, y).bg,
                    row_hover_bg,
                    "the row keeps its background hover while the nested region owns hover (x={probe})"
                );
                assert_eq!(
                    frame.cell(probe, y).bg,
                    row_hover_bg,
                    "the row hover also covers the ✕ cell (x={probe})"
                );
                assert_eq!(
                    frame.cell(x, y).fg,
                    error,
                    "the ✕ adds its own foreground hover, reaching the configured error color (x={probe})"
                );
            }
        })
        .expect("spawn nested hover smoke thread")
        .join()
        .expect("nested hover smoke completes");
}

/// An armed row spells the confirmation out on its detail line — the same language the host
/// disconnect row and the session picker use — and keeps the ✕ visible without hover, since a live
/// confirmation that vanished when the pointer drifted would leave the next click killing a pane
/// with no warning on screen.
#[test]
fn an_armed_row_asks_on_its_detail_line_and_keeps_its_x_visible_unhovered() {
    let lines = panes_sidebar_lines(|state| {
        let id = state.current().focused_pane.expect("a focused pane");
        state.sidebar.pending_row_close = Some(SidebarClose::Pane(id));
    });
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Click again to confirm")),
        "an armed row asks for the confirming click: {lines:#?}"
    );
    assert!(
        lines.iter().any(|line| line.contains('✕')),
        "the ✕ stays put so the confirming click has a target: {lines:#?}"
    );
    // The detail line it replaced is gone: the row says one thing, not two.
    assert!(
        !lines.iter().any(|line| line.contains("shell")),
        "the armed row gives its detail line over to the confirmation: {lines:#?}"
    );
}
