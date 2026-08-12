//! Pins borderless headers, internal Divider rendering, and the config-only special-pane frame.

use rozi::AppRoot;
use rozi::state::{MoveSession, Pane, PaneBorderMode, PaneTitlebarMode, SplitAxis};
use rozi::tiling::build_dwindle_tree;
use tui_lipan::TestBackend;
use tui_lipan::prelude::{CapStyle, FloatRect, Rect};

fn backend(mode: PaneBorderMode, pane_count: usize) -> TestBackend<AppRoot> {
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 30,
        h: 10,
    });
    {
        let state = backend.state_mut();
        state.config.animations.enabled = false;
        state.config.pane.show_workbar = false;
        state.config.pane.show_titles = false;
        state.config.pane.border_mode = mode;
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.start_axis = SplitAxis::Horizontal;
        workspace.panes.clear();
        workspace.tile_tree = None;
        let ids: Vec<_> = (0..pane_count).map(|index| index as u32 + 10).collect();
        for id in &ids {
            let mut pane = Pane::new(
                *id,
                5_000,
                FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 30.0,
                    h: 10.0,
                },
            );
            pane.opening = false;
            pane.terminal_active = true;
            workspace.panes.push(pane);
        }
        workspace.tile_tree = build_dwindle_tree(&ids, workspace.start_axis, &[]);
        workspace.focused_pane = ids.first().copied();
    }
    backend
}

fn on_large_stack(test: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(test)
        .expect("spawn border-mode smoke test")
        .join()
        .expect("border-mode smoke test completes");
}

#[test]
fn none_removes_frames_while_dividers_draw_only_internal_splits() {
    on_large_stack(|| {
        let mut none = backend(PaneBorderMode::None, 2);
        none.render();
        assert!(none.capture_frame().cells.iter().all(|cell| {
            !matches!(
                cell.symbol.as_str(),
                "─" | "│" | "┌" | "┐" | "└" | "┘" | "╭" | "╮" | "╰" | "╯"
            )
        }));

        let mut dividers = backend(PaneBorderMode::Dividers, 2);
        dividers.render();
        let frame = dividers.capture_frame();
        assert!(
            frame.cells.iter().any(|cell| cell.symbol == "│"),
            "side-by-side panes should have an internal vertical Divider"
        );
        let lines = frame.to_fixed_grid_lines();
        assert!(
            [&lines[0], &lines[9]].iter().all(|line| !line
                .chars()
                .any(|ch| matches!(ch, '─' | '┌' | '┐' | '└' | '┘'))),
            "divider mode must not draw an outer frame"
        );
    });
}

#[test]
fn nested_dividers_compose_an_automatic_junction() {
    on_large_stack(|| {
        let mut backend = backend(PaneBorderMode::Dividers, 3);
        backend.render();
        let frame = backend.capture_frame();
        let lines = frame.to_fixed_grid_lines();
        assert!(
            frame
                .cells
                .iter()
                .any(|cell| matches!(cell.symbol.as_str(), "├" | "┤" | "┬" | "┴" | "┼")),
            "nested Divider widgets should compose a junction:\n{}",
            lines.join("\n")
        );
    });
}

#[test]
fn titled_divider_segments_tee_instead_of_cornering() {
    on_large_stack(|| {
        let mut backend = backend(PaneBorderMode::Dividers, 4);
        {
            let state = backend.state_mut();
            state.config.pane.show_titles = true;
            state.config.pane.titlebar = PaneTitlebarMode::Border;
            let workspace = &mut state.current_mut().workspaces[0];
            workspace.start_axis = SplitAxis::Vertical;
            let ids: Vec<_> = workspace.panes.iter().map(|pane| pane.id).collect();
            workspace.tile_tree = build_dwindle_tree(&ids, SplitAxis::Vertical, &[]);
            for (index, pane) in workspace.panes.iter_mut().enumerate() {
                pane.set_custom_title(format!("PANE{index}"));
            }
        }
        backend.render();
        let lines = backend.capture_frame().to_fixed_grid_lines();
        assert!(
            lines
                .iter()
                .all(|line| !line.contains('┌') && !line.contains('┐')),
            "titled horizontal segments meeting a split vertical must tee (┬/┼), not corner:\n{}",
            lines.join("\n")
        );
        assert!(
            lines.iter().any(|line| {
                line.contains('┬') || line.contains('┼') || line.contains('├') || line.contains('┤')
            }),
            "expected a composed tee/cross among titled dividers:\n{}",
            lines.join("\n")
        );
    });
}

#[test]
fn moving_one_pane_preserves_unaffected_dividers() {
    on_large_stack(|| {
        let mut backend = backend(PaneBorderMode::Dividers, 4);
        backend.state_mut().config.animations.enabled = true;
        backend.render();
        backend.state_mut().moving_pane = Some(MoveSession {
            id: 13,
            was_floating: false,
            drag_rect: FloatRect {
                x: 4.0,
                y: 2.0,
                w: 12.0,
                h: 6.0,
            },
            pointer_x: 5,
            pointer_y: 3,
        });

        backend.render();
        assert!(
            backend.capture_frame().cells.iter().any(|cell| matches!(
                cell.symbol.as_str(),
                "─" | "│" | "┌" | "┐" | "└" | "┘" | "├" | "┤" | "┬" | "┴" | "┼"
            )),
            "stable split dividers should remain visible during a tiled move"
        );
    });
}

#[test]
fn border_header_survives_a_borderless_frame() {
    on_large_stack(|| {
        let mut backend = backend(PaneBorderMode::None, 1);
        {
            let state = backend.state_mut();
            state.config.pane.show_titles = true;
            state.config.pane.titlebar = PaneTitlebarMode::Border;
            state.current_mut().workspaces[0].panes[0].set_custom_title("borderless title");
        }
        backend.render();
        let lines = backend.capture_frame().to_fixed_grid_lines();
        assert!(lines[0].contains("borderless title"), "{}", lines[0]);
        assert!(!lines[0].starts_with('╭'), "{}", lines[0]);
    });
}

/// With no frame edge to sit on or beneath, both compact layouts collapse to the same shape: one
/// row of title, then the terminal.
#[test]
fn compact_headers_use_one_row_in_borderless_modes() {
    on_large_stack(|| {
        for mode in [PaneBorderMode::None, PaneBorderMode::Dividers] {
            for titlebar in [PaneTitlebarMode::Integrated, PaneTitlebarMode::Inset] {
                for title_style in [CapStyle::Padded, CapStyle::Half] {
                    let mut backend = backend(mode, 1);
                    {
                        let state = backend.state_mut();
                        state.config.pane.show_titles = true;
                        state.config.pane.titlebar = titlebar;
                        state.config.pane.title_style = title_style;
                        let pane = &mut state.current_mut().workspaces[0].panes[0];
                        pane.set_custom_title("integrated title");
                        pane.terminal.process_server_output(b"body");
                    }

                    backend.render();
                    let lines = backend.capture_frame().to_fixed_grid_lines();
                    assert!(
                        lines[0].contains("integrated title"),
                        "{mode:?}/{titlebar:?}/{title_style:?} title should occupy the first row: {}",
                        lines[0]
                    );
                    assert!(
                        lines[1].starts_with("body"),
                        "{mode:?}/{titlebar:?}/{title_style:?} terminal should start immediately below: {}",
                        lines[1]
                    );
                }
            }
        }
    });
}

#[test]
fn merged_border_titlebar_keeps_lower_title_when_upper_focused() {
    on_large_stack(|| {
        // Border lifts the lower title onto the shared seam; Inset keeps it on its own interior
        // row, which nothing else can claim. Both must survive the focused upper pane drawing last.
        for titlebar in [PaneTitlebarMode::Border, PaneTitlebarMode::Inset] {
            merged_stack_keeps_both_titles(titlebar);
        }
    });
}

fn merged_stack_keeps_both_titles(titlebar: PaneTitlebarMode) {
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 40,
        h: 12,
    });
    {
        let state = backend.state_mut();
        state.config.animations.enabled = false;
        state.config.pane.show_workbar = false;
        state.config.pane.show_titles = true;
        state.config.pane.titlebar = titlebar;
        state.config.pane.border_mode = PaneBorderMode::Merged;
        state.config.pane.highlight_focused_border = true;
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.start_axis = SplitAxis::Vertical;
        workspace.panes.clear();
        workspace.tile_tree = None;
        let ids = [10u32, 11u32];
        for (i, id) in ids.iter().enumerate() {
            let mut pane = Pane::new(
                *id,
                5_000,
                FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 40.0,
                    h: 12.0,
                },
            );
            pane.opening = false;
            pane.terminal_active = true;
            pane.set_custom_title(if i == 0 { "TOP-PANE" } else { "BOTTOM-PANE" });
            workspace.panes.push(pane);
        }
        workspace.tile_tree = build_dwindle_tree(&ids, workspace.start_axis, &[]);
        workspace.focused_pane = Some(10);
        state.current_mut().focused_pane = Some(10);
    }
    backend.render();
    let lines = backend.capture_frame().to_fixed_grid_lines();
    assert!(
        lines.iter().any(|line| line.contains("TOP-PANE")),
        "{titlebar:?} top title missing: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("󰖲  BOTTOM-PANE")),
        "{titlebar:?} bottom title wiped or its icon gap collapsed by focused upper border: {lines:?}"
    );
}

#[test]
fn special_pane_frames_are_config_only_and_off_by_default() {
    on_large_stack(|| {
        let mut backend = backend(PaneBorderMode::None, 1);
        {
            let state = backend.state_mut();
            state.current_mut().workspaces[0].panes[0].floating = true;
            state.current_mut().workspaces[0].panes[0].floating_rect = FloatRect {
                x: 2.0,
                y: 1.0,
                w: 20.0,
                h: 7.0,
            };
        }
        backend.render();
        assert!(
            backend
                .capture_frame()
                .cells
                .iter()
                .all(|cell| !matches!(cell.symbol.as_str(), "═" | "║" | "╔" | "╗" | "╚" | "╝"))
        );

        backend.state_mut().config.pane.keep_special_borders = true;
        backend.render();
        assert!(
            backend
                .capture_frame()
                .cells
                .iter()
                .any(|cell| matches!(cell.symbol.as_str(), "═" | "║" | "╔" | "╗" | "╚" | "╝"))
        );
    });
}
