//! TestBackend coverage for the full-size Portal and Scan pane reveals.

use std::time::Duration;

use rozi::AppRoot;
use rozi::layout::anim::{
    GeometryAnimation, PaneAnimationStyle, pane_opacity_target, retained_pane_timeout,
};
use rozi::layout::tiling::build_dwindle_tree;
use rozi::state::{POPUP_PANE_ID, Pane, PaneBorderMode, SplitAxis};
use tui_lipan::TestBackend;
use tui_lipan::core::event::{MouseButton, MouseKind};
use tui_lipan::prelude::{FloatRect, Key, MouseEvent, Rect};

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

fn client_backend(frame_rate: u16) -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let app = tui_lipan::App::new().frame_rate(frame_rate);
    let mut backend = TestBackend::new_with_app(app, AppRoot::default(), ());
    let theme = backend.state().theme.clone();
    let mut config = rozi::config::Config::default();
    config.frame_rate = frame_rate;
    *backend.state_mut() = rozi::state::State::new(config, theme);
    backend.set_viewport(VIEWPORT);
    backend
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
    popup_backend_at_frame_rate(style, tui_lipan::prelude::DEFAULT_FRAME_RATE)
}

fn popup_backend_at_frame_rate(style: PaneAnimationStyle, frame_rate: u16) -> TestBackend<AppRoot> {
    let mut backend = client_backend(frame_rate);
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
    single_pane_backend_at_frame_rate(style, tui_lipan::prelude::DEFAULT_FRAME_RATE)
}

fn single_pane_backend_at_frame_rate(
    style: PaneAnimationStyle,
    frame_rate: u16,
) -> TestBackend<AppRoot> {
    let mut backend = client_backend(frame_rate);
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
                    // A top-left Scan erases the bottom-right first on close. Sample late enough
                    // for its frontier to reach the title, before the retained pane is pruned.
                    backend.advance(Duration::from_millis(175));
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
fn revealed_content_stays_opaque_and_portal_remains_visible_late_in_close() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            for style in [PaneAnimationStyle::Portal, PaneAnimationStyle::Scan] {
                let mut backend = single_pane_backend(style);
                backend.state_mut().current_mut().workspaces[0].panes[0]
                    .terminal
                    .process_server_output(b"\x1b[48;2;200;40;50m\x1b[2J");
                backend.render();
                let (x, y) = if style == PaneAnimationStyle::Portal {
                    (20, 5)
                } else {
                    (2, 3)
                };
                let before = backend.capture_frame().cell(x, y).bg;
                backend
                    .dispatch(rozi::Msg::RunAction(rozi::input::Action::Close))
                    .expect("close colored pane");
                backend.advance(Duration::from_millis(100));
                assert_eq!(
                    backend.capture_frame().cell(x, y).bg,
                    before,
                    "{style:?} content behind the frontier must cover the underlying layer"
                );
                if style == PaneAnimationStyle::Portal {
                    backend.advance(Duration::from_millis(80));
                    assert_eq!(
                        backend.capture_frame().cell(x, y).bg,
                        before,
                        "Portal must keep its center visible at 90% of the close duration"
                    );
                    let rect = backend.rect_of_key(&pane_key(11)).expect("retained pane");
                    assert!(
                        contains_frontier(&mut backend, rect),
                        "the late portal ring must still be visible"
                    );
                }
            }
        })
        .expect("spawn opaque reveal test")
        .join()
        .expect("opaque reveal test completes");
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
                    1.0,
                    "{style:?} the reveal mask owns visibility, without a whole-pane fade"
                );
                assert_eq!(
                    retained_pane_timeout(
                        backend.state().config.animations,
                        backend.state().runtime_frame_rate()
                    ),
                    Duration::from_millis(229),
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

#[test]
fn closing_reveals_pass_mouse_presses_to_the_surviving_pane() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            for style in [PaneAnimationStyle::Portal, PaneAnimationStyle::Scan] {
                for titlebar in [
                    rozi::state::PaneTitlebarMode::Border,
                    rozi::state::PaneTitlebarMode::Integrated,
                    rozi::state::PaneTitlebarMode::Bar,
                    rozi::state::PaneTitlebarMode::Inset,
                ] {
                    let mut backend = backend(style);
                    {
                        let state = backend.state_mut();
                        state.config.pane.show_titles = true;
                        state.config.pane.titlebar = titlebar;
                        state.config.pane.focus_on_hover = false;
                        for pane in &mut state.current_mut().workspaces[0].panes {
                            pane.opening = false;
                            let color = if pane.id == 10 { 44 } else { 42 };
                            pane.terminal
                                .process_server_output(format!("\x1b[{color}m\x1b[2J").as_bytes());
                        }
                    }
                    backend.render();
                    backend.advance(Duration::from_millis(300));
                    let original = backend.rect_of_key(&pane_key(10)).unwrap();
                    backend
                        .dispatch(rozi::Msg::RunAction(rozi::input::Action::Close))
                        .expect("close left pane");
                    backend.advance(Duration::from_millis(175));
                    let retained = backend
                        .rect_of_key(&pane_key(10))
                        .expect("closing pane paints");
                    assert!(contains_frontier(&mut backend, retained));
                    // Near the old pane's bottom-right: both masks reveal the expanded survivor.
                    let x = (original.x + original.w as i16 - 3) as u16;
                    let y = (original.y + original.h as i16 - 3) as u16;
                    assert!(retained.contains(x as i16, y as i16));
                    let frame = backend.capture_frame();
                    assert_eq!(
                        frame.cell(x, y).bg,
                        frame.cell(VIEWPORT.w - 4, y).bg,
                        "{style:?}/{titlebar:?}: click must land on a revealed survivor cell"
                    );
                    // Closing automatically focuses the survivor. Clear that result before each
                    // press so routing through the actual widget tree must establish focus again.
                    // Cover the terminal body, border header, and interior titlebar rows.
                    for click_y in [y, original.y as u16, original.y as u16 + 1] {
                        backend.state_mut().current_mut().focused_pane = None;
                        backend.state_mut().current_mut().workspaces[0].focused_pane = None;
                        backend.render();
                        for kind in [MouseKind::Down(MouseButton::Left), MouseKind::Up(MouseButton::Left)] {
                            backend.send_mouse(MouseEvent {
                                x, y: click_y, kind, mods: Default::default(),
                            }).expect("press through closing reveal");
                        }
                        assert_eq!(backend.state().current().focused_pane, Some(11),
                            "{style:?}/{titlebar:?} at row {click_y}: retained pane swallowed the press");
                    }
                    backend.state_mut().config.pane.focus_on_hover = true;
                    backend.state_mut().current_mut().focused_pane = None;
                    backend.state_mut().current_mut().workspaces[0].focused_pane = None;
                    backend.render();
                    backend.send_mouse(MouseEvent {
                        x, y, kind: MouseKind::Moved, mods: Default::default(),
                    }).expect("hover through closing reveal");
                    assert_eq!(backend.state().current().focused_pane, Some(11),
                        "{style:?}/{titlebar:?}: retained pane swallowed hover focus");
                }
            }
        })
        .expect("spawn reveal mouse test")
        .join()
        .expect("reveal mouse test completes");
}

#[test]
fn floating_panes_occlude_tiled_terminal_scrollbars() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend(PaneAnimationStyle::Scan);
            for pane in &mut backend.state_mut().current_mut().workspaces[0].panes {
                pane.opening = false;
            }
            backend.render();
            backend.advance(Duration::from_millis(300));
            let tiled = backend.rect_of_key(&pane_key(10)).expect("left tile");
            backend.state_mut().current_mut().workspaces[0].panes[0]
                .terminal
                .process_server_output("scrollback row\r\n".repeat(100).as_bytes());
            let mut float = Pane::new(
                12,
                5_000,
                FloatRect {
                    x: f32::from(tiled.x) + f32::from(tiled.w) - 6.0,
                    y: 1.0,
                    w: 16.0,
                    h: 8.0,
                },
            );
            float.floating = true;
            float.opening = false;
            float.terminal_active = true;
            backend.state_mut().current_mut().workspaces[0]
                .panes
                .push(float);
            backend.render();
            backend.advance(Duration::from_millis(300));
            let x = (tiled.x + tiled.w as i16 - 1) as u16;
            let y = (tiled.y + 3) as u16;
            let float_rect = backend.rect_of_key(&pane_key(12)).expect("floating pane");
            assert!(float_rect.contains(x as i16, y as i16));
            assert!(
                x < (float_rect.x + float_rect.w as i16 - 2) as u16,
                "the float's own scrollbar must be elsewhere"
            );
            for (kind, mouse_y) in [
                (MouseKind::Down(MouseButton::Left), y),
                (MouseKind::Drag(MouseButton::Left), y + 1),
                (MouseKind::Up(MouseButton::Left), y + 1),
            ] {
                backend
                    .send_mouse(MouseEvent {
                        x,
                        y: mouse_y,
                        kind,
                        mods: Default::default(),
                    })
                    .expect("click/drag over covered tiled scrollbar");
            }
            assert_eq!(
                backend.state().current().workspaces[0].panes[0]
                    .terminal
                    .scrollback_offset(),
                0,
                "a floating pane must occlude the tiled scrollbar underneath it"
            );
            // Prove the coordinate and scrollback are usable: removing the covering float should
            // let the same press move the tiled terminal away from its bottom position.
            backend.state_mut().current_mut().workspaces[0]
                .panes
                .retain(|pane| pane.id != 12);
            backend.render();
            for kind in [
                MouseKind::Down(MouseButton::Left),
                MouseKind::Up(MouseButton::Left),
            ] {
                backend
                    .send_mouse(MouseEvent {
                        x,
                        y,
                        kind,
                        mods: Default::default(),
                    })
                    .expect("click uncovered tiled scrollbar");
            }
            assert!(
                backend.state().current().workspaces[0].panes[0]
                    .terminal
                    .scrollback_offset()
                    > 0,
                "control press must actually operate the tiled scrollbar"
            );
        })
        .expect("spawn scrollbar occlusion test")
        .join()
        .expect("scrollbar occlusion test completes");
}

#[test]
fn closing_raised_panes_pass_mouse_input_to_tiles() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            for style in [PaneAnimationStyle::Portal, PaneAnimationStyle::Scan] {
                for fullscreen in [false, true] {
                    let mut backend = backend(style);
                    let state = backend.state_mut();
                    for pane in &mut state.current_mut().workspaces[0].panes {
                        pane.opening = false;
                    }
                    let mut raised = Pane::new(
                        12,
                        5_000,
                        FloatRect {
                            x: 12.0,
                            y: 1.0,
                            w: 16.0,
                            h: 8.0,
                        },
                    );
                    raised.floating = true;
                    raised.fullscreen = fullscreen;
                    raised.opening = false;
                    raised.terminal_active = true;
                    state.current_mut().workspaces[0].panes.push(raised);
                    state.current_mut().workspaces[0].focused_pane = Some(12);
                    state.current_mut().focused_pane = Some(12);
                    backend.render();
                    backend.advance(Duration::from_millis(300));
                    backend
                        .dispatch(rozi::Msg::RunAction(rozi::input::Action::Close))
                        .expect("close raised pane");
                    backend.advance(Duration::from_millis(175));
                    let retained = backend
                        .rect_of_key(&pane_key(12))
                        .expect("retained raised pane");
                    assert!(retained.contains(17, 5));
                    assert!(contains_frontier(&mut backend, retained));
                    backend.state_mut().current_mut().focused_pane = None;
                    backend.state_mut().current_mut().workspaces[0].focused_pane = None;
                    backend.render();
                    backend
                        .send_mouse(MouseEvent {
                            x: 17,
                            y: 5,
                            kind: MouseKind::Down(MouseButton::Left),
                            mods: Default::default(),
                        })
                        .expect("press through raised close");
                    assert_eq!(
                        backend.state().current().focused_pane,
                        Some(10),
                        "{style:?}/fullscreen={fullscreen}: raised closing layer swallowed input"
                    );
                }
            }
        })
        .expect("spawn raised close test")
        .join()
        .expect("raised close test completes");
}

fn retained_pane(backend: &TestBackend<AppRoot>, id: u32) -> Option<&Pane> {
    let state = backend.state();
    state
        .popup
        .as_ref()
        .filter(|pane| pane.id == id)
        .or_else(|| {
            state.current().workspaces[0]
                .panes
                .iter()
                .find(|pane| pane.id == id)
        })
}

#[test]
fn low_frame_rate_scale_close_finishes_fading_before_prune() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            for popup in [false, true] {
                for close_ms in [120, 20] {
                    let mut backend = if popup {
                        popup_backend_at_frame_rate(PaneAnimationStyle::Scale, 15)
                    } else {
                        single_pane_backend_at_frame_rate(PaneAnimationStyle::Scale, 15)
                    };
                    let id = if popup { POPUP_PANE_ID } else { 11 };
                    {
                        let state = backend.state_mut();
                        state.config.confirm.close_pane = false;
                        state.config.animations.close_duration = Duration::from_millis(close_ms);
                        let pane = if popup {
                            state.popup.as_mut().unwrap()
                        } else {
                            &mut state.current_mut().workspaces[0].panes[0]
                        };
                        pane.opening = false;
                        pane.terminal
                            .process_server_output(b"\x1b[48;2;200;40;50m\x1b[2J");
                    }
                    backend.render();
                    backend.advance(Duration::from_millis(300));
                    let original_bg = backend.capture_frame().cell(20, 5).bg;
                    backend
                        .dispatch(if popup {
                            rozi::Msg::ClosePopup
                        } else {
                            rozi::Msg::RunAction(rozi::input::Action::Close)
                        })
                        .expect("start Scale close");
                    let timeout = rozi::layout::anim::retained_pane_timeout_for_pane(
                        backend.state().config.animations,
                        retained_pane(&backend, id).expect("retained pane"),
                        backend.state().runtime_frame_rate(),
                    );
                    // TestBackend caps individual animation ticks at 50 ms. It still exercises
                    // the delayed handoff and the real rendered opacity at the retention boundary.
                    backend.advance(timeout - Duration::from_millis(1));
                    let before_prune = backend.capture_frame().cell(20, 5).bg;
                    let pane = retained_pane(&backend, id).unwrap();
                    let generation = pane.pty_generation;
                    assert!(pane.closing);
                    backend
                        .dispatch(rozi::Msg::PruneClosed(
                            backend.state().runtime_epoch,
                            id,
                            generation,
                        ))
                        .expect("prune after retained timeout");
                    let after_prune = backend.capture_frame().cell(20, 5).bg;
                    assert_ne!(
                        original_bg, after_prune,
                        "the pane must cover the sampled cell before closing"
                    );
                    assert_eq!(
                        before_prune, after_prune,
                        "popup={popup}/close_ms={close_ms}: pruning removed a still-visible fade"
                    );
                    // Budget both the 67 ms handoff and a finishing paint at 15 fps.
                    assert_eq!(timeout, Duration::from_millis(close_ms + 67 + 67));
                    assert!(retained_pane(&backend, id).is_none());
                }
            }
        })
        .expect("spawn low frame rate fade test")
        .join()
        .expect("low frame rate fade test completes");
}

#[test]
fn frame_rate_reload_keeps_scale_close_retention_at_client_cadence() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            rozi::test_support::isolate_user_dirs();
            let _config = rozi::test_support::lock_config_file();
            let path = rozi::config::config_path();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let original = std::fs::read(&path).ok();
            for popup in [false, true] {
                let mut backend = if popup {
                    popup_backend_at_frame_rate(PaneAnimationStyle::Scale, 15)
                } else {
                    single_pane_backend_at_frame_rate(PaneAnimationStyle::Scale, 15)
                };
                // Establish the watcher baseline, then change the startup-only setting on disk.
                std::fs::write(&path, "frame_rate = 15\n").unwrap();
                rozi::config::load_config();
                std::fs::write(
                    &path,
                    "frame_rate = 480\n[animations]\npane_style = \"scale\"\nclose_ms = 120\n",
                )
                .unwrap();
                backend.dispatch(rozi::Msg::ConfigFileChanged).unwrap();
                assert_eq!(backend.state().config.frame_rate, 480);
                assert_eq!(backend.state().runtime_frame_rate(), 15);
                backend.state_mut().config.confirm.close_pane = false;
                backend.advance(Duration::from_millis(300));
                backend
                    .dispatch(if popup {
                        rozi::Msg::ClosePopup
                    } else {
                        rozi::Msg::RunAction(rozi::input::Action::Close)
                    })
                    .unwrap();
                let id = if popup { POPUP_PANE_ID } else { 11 };
                let pane = retained_pane(&backend, id).expect("closing pane retained");
                assert!(pane.closing);
                assert_eq!(
                    rozi::layout::anim::retained_pane_timeout_for_pane(
                        backend.state().config.animations,
                        pane,
                        backend.state().runtime_frame_rate(),
                    ),
                    Duration::from_millis(254),
                    "reload must retain the 15 fps handoff and finishing paint budget",
                );
            }
            if let Some(original) = original {
                std::fs::write(path, original).unwrap();
            } else {
                std::fs::remove_file(path).unwrap();
            }
        })
        .unwrap()
        .join()
        .unwrap();
}
