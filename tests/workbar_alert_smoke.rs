//! Workspace-tab alerts are carried entirely by the tab background, with no glyph in the label, so
//! anything that overwrites that background silently deletes the whole signal. Hover is the case
//! that actually did: a `tab_hover_style` with an absolute background replaces the alert instead of
//! layering over it, and nothing but a rendered frame catches it.

use hyprmux::AppRoot;
use hyprmux::state::{AlertMode, Pane};
use tui_lipan::TestBackend;
use tui_lipan::core::event::{MouseEvent, MouseKind};
use tui_lipan::prelude::{Color, FloatRect, KeyMods, Rect};

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

/// Workspace 1 active and quiet, workspace 2 blocked. `Static` rather than `Pulse` so the colour
/// rests at its peak and the assertions do not depend on which half of a breathe was captured.
fn backend() -> TestBackend<AppRoot> {
    hyprmux::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 80,
        h: 6,
    });
    let state = backend.state_mut();
    state.config.pane.show_workbar = true;
    state.config.workbar.alert.mode = AlertMode::Static;

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
    backend
}

/// Backgrounds along the workbar row, left to right.
fn row_backgrounds(backend: &mut TestBackend<AppRoot>) -> Vec<Color> {
    backend.render();
    let frame = backend.capture_frame();
    let width = frame.width as usize;
    frame.cells[..width].iter().map(|cell| cell.bg).collect()
}

fn on_large_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(body)
        .expect("spawn workbar alert smoke thread")
        .join()
        .expect("workbar alert smoke completes");
}

/// `background` paint fills the tab and shapes it like the active one; `text` colors the label and
/// leaves the fill alone. Asserted on the rendered row so a paint wired to the wrong channel — the
/// easy mistake, since both end up in the same `Style` — cannot pass.
#[test]
fn the_two_paints_color_different_channels_on_screen() {
    on_large_stack(|| {
        let quiet_bg = row_backgrounds(&mut quiet_backend());

        let mut text = backend();
        text.state_mut().config.workbar.alert.paint = hyprmux::state::AlertPaint::Text;
        assert_eq!(
            row_backgrounds(&mut text),
            quiet_bg,
            "text paint must leave every background untouched"
        );

        let filled = row_backgrounds(&mut backend());
        assert_ne!(
            filled, quiet_bg,
            "background paint must change the tab's fill"
        );
    });
}

/// The filled variant carries its own emphasis, so it takes the active tab's end caps rather than
/// sitting flat beside it. Needs `Tab::capped`; without it the glyphs never appear.
#[test]
fn a_filled_marked_tab_is_end_capped_like_the_active_one() {
    on_large_stack(|| {
        let mut backend = backend();
        backend.state_mut().config.pane.workbar_tab_style = tui_lipan::prelude::CapStyle::Round;
        backend.render();
        let frame = backend.capture_frame();
        let width = frame.width as usize;
        let row: String = frame.cells[..width]
            .iter()
            .map(|cell| cell.symbol.clone())
            .collect();

        // The round caps the active tab uses; a marked tab must reach for the same pair.
        let caps = ['\u{e0b6}', '\u{e0b4}'];
        let count = row.chars().filter(|c| caps.contains(c)).count();
        assert!(
            count >= 4,
            "expected caps on both the active and the marked tab, found {count} in {row:?}"
        );
    });
}

#[test]
fn a_marked_tab_paints_a_background_no_quiet_tab_uses() {
    on_large_stack(|| {
        let alert_row = row_backgrounds(&mut backend());
        let quiet_row = row_backgrounds(&mut quiet_backend());

        assert_ne!(
            alert_row, quiet_row,
            "a blocked workspace left the workbar row identical to a quiet one"
        );
    });
}

/// The same workbar with the blocked trigger switched off. Silencing the trigger rather than
/// removing the pane is what makes a column-by-column diff meaningful: the label is identity only
/// (`2 ·1`), so both renders lay out identically and every differing column is a colour difference
/// rather than a width shift.
fn quiet_backend() -> TestBackend<AppRoot> {
    let mut quiet = backend();
    quiet.state_mut().config.workbar.alert.blocked = false;
    quiet
}

/// The columns the blocked workspace actually paints. Located by diffing rather than by colour
/// arithmetic, which would duplicate the tint maths the view does and pass even if both were wrong
/// in the same way.
fn alert_columns() -> (Vec<Color>, Vec<usize>) {
    let alert_row = row_backgrounds(&mut backend());
    let quiet_row = row_backgrounds(&mut quiet_backend());
    let columns = alert_row
        .iter()
        .zip(quiet_row.iter())
        .enumerate()
        .filter_map(|(x, (alert, quiet))| (alert != quiet).then_some(x))
        .collect();
    (alert_row, columns)
}

#[test]
fn hovering_a_marked_tab_keeps_its_alert_background() {
    on_large_stack(|| {
        let (resting, columns) = alert_columns();
        assert!(!columns.is_empty(), "the blocked tab painted nothing");
        let x = columns[columns.len() / 2];

        let mut backend = backend();
        row_backgrounds(&mut backend);
        backend
            .send_mouse(MouseEvent {
                x: x as u16,
                y: 0,
                kind: MouseKind::Moved,
                mods: KeyMods::NONE,
            })
            .expect("hover the workbar");
        let hovered = row_backgrounds(&mut backend);

        assert_ne!(
            hovered[x], resting[x],
            "hover left the tab untouched - the hover never registered, so this proves nothing"
        );

        // The hover style layers over the tab's own background rather than replacing it, so a
        // hovered marked tab is a *lifted* alert colour. An absolute `bg(...)` there collapses it
        // to the same colour an unmarked tab hovers to, which is the regression this pins.
        let mut quiet = quiet_backend();
        row_backgrounds(&mut quiet);
        quiet
            .send_mouse(MouseEvent {
                x: x as u16,
                y: 0,
                kind: MouseKind::Moved,
                mods: KeyMods::NONE,
            })
            .expect("hover the quiet workbar");
        let quiet_hovered = row_backgrounds(&mut quiet);

        assert_ne!(
            hovered[x], quiet_hovered[x],
            "hovering a marked tab gave it the same colour as hovering a quiet one - \
             the alert was replaced rather than lifted"
        );
    });
}
