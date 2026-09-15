//! The scratchpad is a layer above the workspace. Merged-border Fuzzy fusing is buffer-level, so
//! a dropdown that paints on top of tiled seams would otherwise grow junctions into them.

use rozi::AppRoot;
use rozi::layout::tiling::build_dwindle_tree;
use rozi::state::{Pane, PaneBorderMode, PaneBorderStyle, SplitAxis, Workspace};
use tui_lipan::TestBackend;
use tui_lipan::prelude::{FloatRect, Rect};

const WIDTH: u16 = 40;
const HEIGHT: u16 = 16;

fn on_large_stack(test: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(test)
        .expect("spawn scratchpad border-merge smoke test")
        .join()
        .expect("scratchpad border-merge smoke test completes");
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

fn backend(workspace_panes: usize, scratch_panes: usize) -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: WIDTH,
        h: HEIGHT,
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
            w: f32::from(WIDTH),
            h: f32::from(HEIGHT),
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

fn lines(backend: &mut TestBackend<AppRoot>) -> Vec<String> {
    backend.capture_frame().to_fixed_grid_lines()
}

fn scratch_top_row(lines: &[String]) -> Option<(usize, &str)> {
    lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.contains('╔'))
        .map(|(index, line)| (index, line.as_str()))
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
    on_large_stack(|| {
        let mut backend = backend(2, 1);
        let lines = lines(&mut backend);
        let Some((top, row)) = scratch_top_row(&lines) else {
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
    on_large_stack(|| {
        let mut backend = backend(1, 2);
        let lines = lines(&mut backend);
        let Some((_, row)) = scratch_top_row(&lines) else {
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
