//! Pane border composition through `AppRoot`: divider and merged modes, title survival, special
//! frames, rounded junctions, floating occlusion, and the scratchpad's own merged layer.

use rozi::AppRoot;
use rozi::layout::tiling::build_dwindle_tree;
use rozi::state::{
    MoveSession, Pane, PaneBorderMode, PaneBorderStyle, PaneTitlebarMode, SplitAxis, Workspace,
};
use tui_lipan::TestBackend;
use tui_lipan::prelude::{CapStyle, FloatRect, Rect};

fn backend(mode: PaneBorderMode, pane_count: usize) -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
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

#[test]
fn none_removes_frames_while_dividers_draw_only_internal_splits() {
    rozi::test_support::on_deeper_stack(|| {
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
fn moving_one_pane_preserves_unaffected_dividers() {
    rozi::test_support::on_deeper_stack(|| {
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
    rozi::test_support::on_deeper_stack(|| {
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
    rozi::test_support::on_deeper_stack(|| {
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

/// `Border` and `Integrated` seam survival is pinned by `merged_titlebar_seam_smoke`. Inset keeps
/// the lower title on its own interior row, which the focused upper pane used to paint over.
#[test]
fn merged_inset_title_survives_the_focused_pane_drawing_last() {
    rozi::test_support::on_deeper_stack(|| {
        merged_stack_keeps_both_titles(PaneTitlebarMode::Inset);
    });
}

fn merged_stack_keeps_both_titles(titlebar: PaneTitlebarMode) {
    rozi::test_support::isolate_user_dirs();
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

/// A floating pane keeps its double frame in the frameless modes by default - nothing else marks
/// where the layer above the tiles ends - and only an explicit opt-out drops it.
#[test]
fn special_pane_frames_are_on_by_default_and_config_only() {
    rozi::test_support::on_deeper_stack(|| {
        for mode in [PaneBorderMode::None, PaneBorderMode::Dividers] {
            let mut backend = backend(mode, 1);
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
                    .any(|cell| matches!(cell.symbol.as_str(), "═" | "║" | "╔" | "╗" | "╚" | "╝")),
                "{mode:?} should frame a floating pane by default"
            );

            backend.state_mut().config.pane.keep_special_borders = false;
            backend.render();
            assert!(
                backend
                    .capture_frame()
                    .cells
                    .iter()
                    .all(|cell| !matches!(cell.symbol.as_str(), "═" | "║" | "╔" | "╗" | "╚" | "╝")),
                "{mode:?} opt-out should drop the floating frame"
            );
        }
    });
}

#[test]
fn pane_border_style_follows_tiled_float_and_fullscreen() {
    rozi::test_support::on_deeper_stack(|| {
        let mut backend = backend(PaneBorderMode::Separate, 1);
        {
            let state = backend.state_mut();
            state.config.pane.border_style = PaneBorderStyle::Rounded;
            state.config.pane.float_border_style = PaneBorderStyle::Thick;
            state.config.pane.fullscreen_border_style = PaneBorderStyle::Double;
        }

        backend.render();
        assert!(
            backend
                .capture_frame()
                .cells
                .iter()
                .any(|cell| matches!(cell.symbol.as_str(), "╭" | "╮" | "╰" | "╯")),
            "tiled panes use border_style"
        );

        {
            let pane = &mut backend.state_mut().current_mut().workspaces[0].panes[0];
            pane.floating = true;
            pane.floating_rect = FloatRect {
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
                .any(|cell| matches!(cell.symbol.as_str(), "┏" | "┓" | "┗" | "┛")),
            "floating panes use float_border_style"
        );

        {
            let pane = &mut backend.state_mut().current_mut().workspaces[0].panes[0];
            pane.floating = false;
            pane.fullscreen = true;
        }
        backend.render();
        assert!(
            backend
                .capture_frame()
                .cells
                .iter()
                .any(|cell| matches!(cell.symbol.as_str(), "╔" | "╗" | "╚" | "╝")),
            "fullscreen panes use fullscreen_border_style"
        );
    });
}

/// Rounded corners have no arc junction glyphs. Fuzzy merge, which merged pane frames select,
/// fuses the shared corners into plain tees and leaves the outer corners rounded.
#[test]
fn rounded_merged_tiles_fuse_shared_corners_and_keep_outer_arcs() {
    rozi::test_support::on_deeper_stack(|| {
        let mut backend = backend(PaneBorderMode::Merged, 3);
        backend.state_mut().config.pane.border_style = PaneBorderStyle::Rounded;
        backend.render();
        let frame = backend.capture_frame();
        let lines = frame.to_fixed_grid_lines();
        let arcs = frame
            .cells
            .iter()
            .filter(|cell| matches!(cell.symbol.as_str(), "╭" | "╮" | "╰" | "╯"))
            .count();
        assert_eq!(
            arcs,
            4,
            "only the outer corners stay rounded:\n{}",
            lines.join("\n")
        );
        assert!(
            frame
                .cells
                .iter()
                .any(|cell| matches!(cell.symbol.as_str(), "┬" | "├" | "┤" | "┴" | "┼")),
            "shared rounded corners should fall back to plain junctions:\n{}",
            lines.join("\n")
        );
    });
}

/// A floating pane draws in Replace mode, so its double frame crosses a tiled seam without
/// growing a junction into it.
#[test]
fn a_floating_double_border_occludes_the_tiled_seam() {
    rozi::test_support::on_deeper_stack(|| {
        let mut backend = backend(PaneBorderMode::Merged, 2);
        {
            let state = backend.state_mut();
            state.config.pane.border_style = PaneBorderStyle::Rounded;
            state.config.pane.float_border_style = PaneBorderStyle::Double;
            let mut pane = Pane::new(
                40,
                5_000,
                FloatRect {
                    x: 4.0,
                    y: 1.0,
                    w: 22.0,
                    h: 7.0,
                },
            );
            pane.opening = false;
            pane.terminal_active = true;
            pane.floating = true;
            pane.floating_rect = FloatRect {
                x: 4.0,
                y: 1.0,
                w: 22.0,
                h: 7.0,
            };
            state.current_mut().workspaces[0].panes.push(pane);
        }
        backend.render();
        let lines = backend.capture_frame().to_fixed_grid_lines();
        let top = lines
            .iter()
            .find(|line| line.contains('╔'))
            .unwrap_or_else(|| panic!("floating double frame missing:\n{}", lines.join("\n")));
        let start = top.find('╔').expect("float corner");
        let end = top.rfind('╗').expect("float corner") + '╗'.len_utf8();
        let edge = &top[start..end];
        assert!(
            edge.contains('═'),
            "float top edge should stay a double rule:\n{top}"
        );
        assert!(
            !edge.chars().any(fused_with_workspace),
            "float top edge fused with the tiled seam:\n{}",
            lines.join("\n")
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains('│') || line.contains('┬')),
            "the tiled seam should remain visible outside the float:\n{}",
            lines.join("\n")
        );
    });
}

fn fill_workspace(workspace: &mut Workspace, ids: &[u32], axis: SplitAxis, size: FloatRect) {
    workspace.start_axis = axis;
    workspace.panes.clear();
    workspace.tile_tree = None;
    for id in ids {
        let mut pane = Pane::new(*id, 5_000, size);
        pane.opening = false;
        pane.terminal_active = true;
        workspace.panes.push(pane);
    }
    workspace.tile_tree = build_dwindle_tree(ids, axis, &[]);
    workspace.focused_pane = ids.first().copied();
}

fn scratch_backend(workspace_panes: usize, scratch_panes: usize) -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 40,
        h: 16,
    });
    {
        let state = backend.state_mut();
        state.config.animations.enabled = false;
        state.config.pane.show_workbar = false;
        state.config.pane.show_titles = false;
        state.config.pane.border_mode = PaneBorderMode::Merged;
        state.config.pane.border_style = PaneBorderStyle::Rounded;
        state.config.pane.scratch_border_style = PaneBorderStyle::Double;
        let size = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 40.0,
            h: 16.0,
        };
        let workspace_ids: Vec<u32> = (1..=workspace_panes as u32).collect();
        fill_workspace(
            &mut state.current_mut().workspaces[0],
            &workspace_ids,
            SplitAxis::Horizontal,
            size,
        );
        state.current_mut().focused_pane = workspace_ids.first().copied();
        let scratch_ids: Vec<u32> = (100..100 + scratch_panes as u32).collect();
        fill_workspace(
            &mut state.scratch,
            &scratch_ids,
            SplitAxis::Horizontal,
            size,
        );
        state.scratch_visible = true;
    }
    backend.render();
    backend
}

fn fused_with_workspace(ch: char) -> bool {
    matches!(
        ch,
        '┬' | '┴'
            | '├'
            | '┤'
            | '┼'
            | '╤'
            | '╥'
            | '╦'
            | '╧'
            | '╨'
            | '╩'
            | '╪'
            | '╫'
            | '╟'
            | '╢'
            | '╞'
            | '╡'
            | '╠'
            | '╣'
            | '│'
            | '╭'
            | '╮'
    )
}

#[test]
fn scratchpad_does_not_junction_with_workspace_seams() {
    rozi::test_support::on_deeper_stack(|| {
        let backend = scratch_backend(2, 1);
        let lines = backend.capture_frame().to_fixed_grid_lines();
        let Some((top, row)) = lines
            .iter()
            .enumerate()
            .find(|(_, line)| line.contains('╔'))
        else {
            panic!("scratchpad double frame missing:\n{}", lines.join("\n"));
        };
        assert!(
            top > 0,
            "dropdown should sit over workspace tiles:\n{}",
            lines.join("\n")
        );
        assert!(
            !row.chars().any(fused_with_workspace),
            "scratchpad top edge fused with the workspace split:\n{}",
            lines.join("\n")
        );
        assert!(
            row.contains('═'),
            "scratchpad top edge should stay a double rule:\n{}",
            lines.join("\n")
        );
        assert!(
            lines[..top].iter().any(|line| line.contains('│')),
            "workspace split should remain visible above the dropdown:\n{}",
            lines.join("\n")
        );
        assert!(
            lines.iter().any(|line| line.contains('╚')),
            "scratchpad floor should stay a double corner, not a workspace tee:\n{}",
            lines.join("\n")
        );
    });
}

#[test]
fn scratchpad_tiles_still_merge_with_each_other() {
    rozi::test_support::on_deeper_stack(|| {
        let backend = scratch_backend(1, 2);
        let lines = backend.capture_frame().to_fixed_grid_lines();
        let Some((_, row)) = lines
            .iter()
            .enumerate()
            .find(|(_, line)| line.contains('╔'))
        else {
            panic!("scratchpad double frame missing:\n{}", lines.join("\n"));
        };
        assert!(
            row.contains('╦'),
            "split scratch tiles should still share a merged top junction:\n{}",
            lines.join("\n")
        );
        assert!(
            !row.contains('│') && !row.contains('╭') && !row.contains('╮'),
            "scratch row should not pick up rounded workspace glyphs:\n{}",
            lines.join("\n")
        );
    });
}
