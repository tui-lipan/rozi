//! TestBackend coverage for the full-size Portal and Scan pane reveals.

use std::time::Duration;

use rozi::AppRoot;
use rozi::layout::anim::{
    GeometryAnimation, PaneAnimationStyle, pane_opacity_target, retained_pane_timeout,
};
use rozi::layout::tiling::build_dwindle_tree;
use rozi::state::{POPUP_PANE_ID, Pane, PaneBorderMode, SplitAxis};
use tui_lipan::TestBackend;
use tui_lipan::prelude::{FloatRect, Key, Rect};

const VIEWPORT: Rect = Rect {
    x: 0,
    y: 0,
    w: 40,
    h: 10,
};
const REVEAL_TITLE: &str = "REVEAL-TITLE-ABCDEFGHIJKLMNOPQR";
const REVEAL_TITLE_PREFIX: &str = "REVEAL-TITLE-ABCDEFGHIJKLMN";

fn pane_key(id: u32) -> Key {
    format!("rozi-pane-{id}-0").into()
}

fn backend(style: PaneAnimationStyle) -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(VIEWPORT);
    {
        let state = backend.state_mut();
        configure_reveal(state, style);
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.start_axis = SplitAxis::Horizontal;
        workspace.panes.clear();
        for id in [10, 11] {
            let mut pane = Pane::new(id, 5_000, Default::default());
            pane.opening = id == 11;
            pane.terminal_active = true;
            let _ = pane
                .terminal
                .process_server_output(b"abcdefghijklmnopqrstuvwxyz\r\n");
            workspace.panes.push(pane);
        }
        workspace.tile_tree = build_dwindle_tree(&[10, 11], workspace.start_axis, &[]);
        workspace.focused_pane = Some(10);
        state.current_mut().focused_pane = Some(10);
    }
    backend
}

fn configure_reveal(state: &mut rozi::state::State, style: PaneAnimationStyle) {
    state.animation = GeometryAnimation::Spawn;
    state.config.animations.enabled = true;
    state.config.animations.pane_style = style;
    state.config.animations.geometry_duration = Duration::from_millis(200);
    state.config.animations.open_delay = Duration::ZERO;
    state.config.pane.show_workbar = false;
    state.config.pane.show_titles = false;
    state.config.pane.border_mode = PaneBorderMode::Separate;
}

fn popup_backend(style: PaneAnimationStyle) -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(VIEWPORT);
    {
        let state = backend.state_mut();
        configure_reveal(state, style);
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.panes.clear();
        workspace.tile_tree = None;
        workspace.focused_pane = None;
        state.current_mut().focused_pane = None;

        let mut popup = Pane::new(
            POPUP_PANE_ID,
            5_000,
            FloatRect {
                x: 8.0,
                y: 2.0,
                w: 24.0,
                h: 6.0,
            },
        );
        popup.opening = true;
        popup.terminal_active = true;
        let _ = popup
            .terminal
            .process_server_output(b"popup-original-content\r\n");
        state.popup = Some(popup);
    }
    backend
}

fn titled_reveal_backend(
    style: PaneAnimationStyle,
    border_mode: PaneBorderMode,
) -> TestBackend<AppRoot> {
    let mut backend = backend(style);
    backend.set_viewport(Rect { w: 80, ..VIEWPORT });
    let state = backend.state_mut();
    state.config.pane.show_titles = true;
    state.config.pane.titlebar = rozi::state::PaneTitlebarMode::Border;
    state.config.pane.border_mode = border_mode;
    let panes = &mut state.current_mut().workspaces[0].panes;
    panes
        .iter_mut()
        .find(|pane| pane.id == 10)
        .expect("stable pane")
        .set_custom_title("STABLE-PANE");
    panes
        .iter_mut()
        .find(|pane| pane.id == 11)
        .expect("revealing pane")
        .set_custom_title(REVEAL_TITLE);
    backend
}

fn single_pane_backend(style: PaneAnimationStyle) -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(VIEWPORT);
    {
        let state = backend.state_mut();
        configure_reveal(state, style);
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.panes.clear();
        workspace.start_axis = SplitAxis::Horizontal;
        let mut pane = Pane::new(
            11,
            5_000,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: f32::from(VIEWPORT.w),
                h: f32::from(VIEWPORT.h),
            },
        );
        pane.opening = false;
        pane.terminal_active = true;
        let _ = pane
            .terminal
            .process_server_output(b"closing-pane-content\r\n");
        workspace.panes.push(pane);
        workspace.tile_tree = build_dwindle_tree(&[11], workspace.start_axis, &[]);
        workspace.focused_pane = Some(11);
        state.current_mut().focused_pane = Some(11);
    }
    backend
}

fn begin_arrival(backend: &mut TestBackend<AppRoot>) {
    backend.state_mut().current_mut().workspaces[0]
        .panes
        .iter_mut()
        .find(|pane| pane.id == 11)
        .expect("arriving pane")
        .opening = false;
    backend.render();
}

fn rect_text(backend: &mut TestBackend<AppRoot>, rect: Rect) -> String {
    let captured = backend.capture_frame();
    let frame = &captured;
    let x = rect.x.max(0) as u16;
    let y = rect.y.max(0) as u16;
    (y..y.saturating_add(rect.h))
        .flat_map(|y| (x..x.saturating_add(rect.w)).map(move |x| frame.cell(x, y).symbol.clone()))
        .collect()
}

fn contains_frontier(backend: &mut TestBackend<AppRoot>, rect: Rect) -> bool {
    ".:+*#%/="
        .chars()
        .any(|glyph| rect_text(backend, rect).contains(glyph))
}

#[test]
fn reveal_styles_keep_the_pane_rectangle_and_terminal_grid_fixed() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            for style in [PaneAnimationStyle::Portal, PaneAnimationStyle::Scan] {
                let mut backend = backend(style);
                backend.render();
                begin_arrival(&mut backend);
                let initial = backend
                    .rect_of_key(&pane_key(11))
                    .expect("arriving pane rectangle");

                backend.advance(Duration::from_millis(100));
                let midpoint = backend
                    .rect_of_key(&pane_key(11))
                    .expect("midpoint pane rectangle");
                assert_eq!(
                    midpoint, initial,
                    "{style:?} must not resize the terminal while revealing"
                );
                assert!(
                    contains_frontier(&mut backend, midpoint),
                    "{style:?} should paint its punctuation frontier"
                );

                backend.advance(Duration::from_millis(300));
                let settled = backend
                    .rect_of_key(&pane_key(11))
                    .expect("settled pane rectangle");
                assert_eq!(settled, initial, "{style:?} changed pane geometry");
                assert!(
                    rect_text(&mut backend, settled).contains("abcdefgh"),
                    "{style:?} did not restore terminal output"
                );
            }
        })
        .expect("spawn pane-reveal smoke test")
        .join()
        .expect("pane-reveal smoke test completes");
}

#[test]
fn reveal_titles_stay_inside_the_effect_scope_until_merged_or_divider_settlement() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            for style in [PaneAnimationStyle::Portal, PaneAnimationStyle::Scan] {
                for border_mode in [PaneBorderMode::Merged, PaneBorderMode::Dividers] {
                    let mut backend = titled_reveal_backend(style, border_mode);
                    backend.render();
                    begin_arrival(&mut backend);

                    // EaseOut reveals the scan's top row early; sample while the effect still masks
                    // the title so an external seam/divider copy cannot hide behind a full frame.
                    backend.advance(Duration::from_millis(50));
                    let midpoint = backend.capture_frame().to_fixed_grid_lines();
                    assert!(
                        midpoint
                            .iter()
                            .all(|line| !line.contains(REVEAL_TITLE_PREFIX)),
                        "{style:?}/{border_mode:?} leaked a full external title mid-reveal:\n{}",
                        midpoint.join("\n")
                    );

                    backend.advance(Duration::from_millis(300));
                    let settled = backend.capture_frame().to_fixed_grid_lines();
                    assert!(
                        settled
                            .iter()
                            .any(|line| line.contains(REVEAL_TITLE_PREFIX)),
                        "{style:?}/{border_mode:?} did not lift its title after settlement:\n{}",
                        settled.join("\n")
                    );

                    backend.state_mut().current_mut().focused_pane = Some(11);
                    backend.state_mut().current_mut().workspaces[0].focused_pane = Some(11);
                    backend
                        .dispatch(rozi::Msg::RunAction(rozi::input::Action::Close))
                        .expect("close titled pane through lifecycle");
                    backend.render();
                    backend.advance(Duration::from_millis(100));
                    let closing_midpoint = backend.capture_frame().to_fixed_grid_lines();
                    assert!(
                        closing_midpoint
                            .iter()
                            .all(|line| !line.contains(REVEAL_TITLE_PREFIX)),
                        "{style:?}/{border_mode:?} leaked a full external title mid-close:\n{}",
                        closing_midpoint.join("\n")
                    );
                }
            }
        })
        .expect("spawn titled pane-reveal smoke test")
        .join()
        .expect("titled pane-reveal smoke test completes");
}

#[test]
fn reveal_styles_reverse_a_real_close_at_a_fixed_rectangle_and_prune_hidden() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            for style in [PaneAnimationStyle::Portal, PaneAnimationStyle::Scan] {
                let mut backend = single_pane_backend(style);
                backend.render();

                backend
                    .dispatch(rozi::Msg::RunAction(rozi::input::Action::Close))
                    .expect("close pane through lifecycle");
                assert!(
                    backend.state().current().workspaces[0]
                        .panes
                        .iter()
                        .any(|pane| pane.id == 11 && pane.closing),
                    "{style:?} close should retain the pane until prune"
                );
                backend.render();
                let closing = backend
                    .rect_of_key(&pane_key(11))
                    .expect("closing pane rectangle");
                // The real close lifecycle freezes the departing pane's stored floating rect once
                // it leaves the tiling tree. That handoff can differ by a cell from the pre-close
                // target; the reveal must remain fixed from the first closing frame onward.
                assert_ne!(
                    closing,
                    Rect::default(),
                    "{style:?} close lost its rectangle"
                );

                backend.advance(Duration::from_millis(100));
                assert_eq!(
                    backend.rect_of_key(&pane_key(11)),
                    Some(closing),
                    "{style:?} close resized the pane"
                );
                let mid_text = rect_text(&mut backend, closing);
                assert!(
                    contains_frontier(&mut backend, closing),
                    "{style:?} close should show its reverse frontier: {mid_text}"
                );

                backend.advance(Duration::from_millis(200));
                let generation = backend.state().current().workspaces[0]
                    .panes
                    .iter()
                    .find(|pane| pane.id == 11)
                    .expect("closing pane retained through its reveal")
                    .pty_generation;
                let closing_pane = backend.state().current().workspaces[0]
                    .panes
                    .iter()
                    .find(|pane| pane.id == 11)
                    .expect("closing pane retained until prune");
                assert_eq!(
                    pane_opacity_target(backend.state().config.animations, closing_pane),
                    0.0,
                    "{style:?} settled close must stay hidden before prune"
                );
                assert_eq!(
                    retained_pane_timeout(backend.state().config.animations),
                    Duration::from_millis(220),
                    "{style:?} retention must cover the reveal duration"
                );
                backend
                    .dispatch(rozi::Msg::PruneClosed(
                        backend.state().runtime_epoch,
                        11,
                        generation,
                    ))
                    .expect("prune closed pane after reveal");
                assert!(
                    backend.state().current().workspaces[0]
                        .panes
                        .iter()
                        .all(|pane| pane.id != 11),
                    "{style:?} closing pane should be pruned, not reappear"
                );
                assert!(
                    backend.rect_of_key(&pane_key(11)).is_none(),
                    "{style:?} pruned pane must not remain rendered"
                );
            }
        })
        .expect("spawn pane-reveal close smoke test")
        .join()
        .expect("pane-reveal close smoke test completes");
}

#[test]
fn reveal_styles_keep_popup_rect_fixed_through_opening_and_settle_content() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            for style in [PaneAnimationStyle::Portal, PaneAnimationStyle::Scan] {
                let mut backend = popup_backend(style);
                backend.render();
                let initial = backend
                    .rect_of_key(&pane_key(POPUP_PANE_ID))
                    .expect("opening popup rectangle");

                backend.state_mut().popup.as_mut().unwrap().opening = false;
                backend.render();
                assert_eq!(
                    backend.rect_of_key(&pane_key(POPUP_PANE_ID)),
                    Some(initial),
                    "{style:?} opening popup changed its rectangle"
                );

                backend.advance(Duration::from_millis(100));
                assert_eq!(
                    backend.rect_of_key(&pane_key(POPUP_PANE_ID)),
                    Some(initial),
                    "{style:?} popup reveal resized its rectangle"
                );
                assert!(
                    contains_frontier(&mut backend, initial),
                    "{style:?} popup reveal should show its frontier"
                );

                backend.advance(Duration::from_millis(200));
                assert_eq!(
                    backend.rect_of_key(&pane_key(POPUP_PANE_ID)),
                    Some(initial),
                    "{style:?} settled popup changed its rectangle"
                );
                assert!(
                    rect_text(&mut backend, initial).contains("popup-original-content"),
                    "{style:?} popup did not restore original content"
                );
            }
        })
        .expect("spawn popup-reveal smoke test")
        .join()
        .expect("popup-reveal smoke test completes");
}
