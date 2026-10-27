//! Pane border and title composition through `AppRoot`: divider and merged modes, titlebar
//! layouts, seam strips, special frames, rounded junctions, floating occlusion, and the
//! scratchpad's own merged layer.

use rozi::AppRoot;
use rozi::layout::tiling::build_dwindle_tree;
use rozi::state::{
    MoveSession, Pane, PaneBorderMode, PaneBorderStyle, PaneId, PaneTitlebarMode, SplitAxis,
    Workspace,
};
use tui_lipan::prelude::{CapStyle, FloatRect, Rect};
use tui_lipan::{CapturedFrame, TestBackend};

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
        let ids: Vec<_> = (0..pane_count).map(|index| index as u32 + 10).collect();
        fill_workspace(
            &mut state.current_mut().workspaces[0],
            &ids,
            SplitAxis::Horizontal,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 30.0,
                h: 10.0,
            },
        );
        state.current_mut().focused_pane = ids.first().copied();
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

        let mut moving = backend(PaneBorderMode::Dividers, 4);
        moving.render();
        let frame = moving.capture_frame();
        let seam_x = (0..frame.width)
            .find(|&x| frame.cell(x, 0).symbol == "│")
            .expect("the unmoving upper tiles share a divider");
        moving.state_mut().moving_pane = Some(MoveSession {
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
        moving.render();
        assert_eq!(
            moving.capture_frame().cell(seam_x, 0).symbol,
            "│",
            "moving a lower tile preserves the unaffected upper divider"
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
        for (mode, titlebar, title_style) in [
            (
                PaneBorderMode::None,
                PaneTitlebarMode::Integrated,
                CapStyle::Padded,
            ),
            (
                PaneBorderMode::Dividers,
                PaneTitlebarMode::Inset,
                CapStyle::Half,
            ),
        ] {
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
    });
}

/// Inset keeps the lower title on its own interior row, which the focused upper pane used to
/// paint over. `Border` and `Integrated` seam strips are checked below.
#[test]
fn merged_inset_title_survives_the_focused_pane_drawing_last() {
    rozi::test_support::on_deeper_stack(|| {
        merged_stack_keeps_both_titles(PaneTitlebarMode::Inset);
    });
}

fn merged_stack_keeps_both_titles(titlebar: PaneTitlebarMode) {
    let lines = render_titled(
        titlebar,
        CapStyle::Padded,
        TOP,
        &["TOP-PANE", "BOTTOM-PANE"],
    )
    .to_fixed_grid_lines();
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
        // None has no workspace frame, so a double rule can only be the floating pane.
        // Dividers uses the same special-frame path.
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
                .any(|cell| matches!(cell.symbol.as_str(), "═" | "║" | "╔" | "╗" | "╚" | "╝")),
            "a floating pane keeps its double frame when tiles have none"
        );

        backend.state_mut().config.pane.keep_special_borders = false;
        backend.render();
        assert!(
            backend
                .capture_frame()
                .cells
                .iter()
                .all(|cell| !matches!(cell.symbol.as_str(), "═" | "║" | "╔" | "╗" | "╚" | "╝")),
            "the opt-out drops the floating frame"
        );
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

const TITLE: &str = "nvim src/pane.rs";

fn render_title(
    mode: PaneTitlebarMode,
    show_titles: bool,
    title_style: CapStyle,
    floating: bool,
) -> Vec<String> {
    render_capture(mode, show_titles, title_style, floating, true).to_fixed_grid_lines()
}

fn render_padded(mode: PaneTitlebarMode, padding: (u16, u16, u16, u16)) -> Vec<String> {
    render_with(mode, true, CapStyle::Padded, false, true, padding).to_fixed_grid_lines()
}

fn render_capture(
    mode: PaneTitlebarMode,
    show_titles: bool,
    title_style: CapStyle,
    floating: bool,
    highlight_focused_titlebar: bool,
) -> tui_lipan::CapturedFrame {
    render_with(
        mode,
        show_titles,
        title_style,
        floating,
        highlight_focused_titlebar,
        (0, 0, 0, 0),
    )
}

fn render_with(
    mode: PaneTitlebarMode,
    show_titles: bool,
    title_style: CapStyle,
    floating: bool,
    highlight_focused_titlebar: bool,
    padding: (u16, u16, u16, u16),
) -> tui_lipan::CapturedFrame {
    let mut backend = title_backend(mode, title_style);
    {
        let state = backend.state_mut();
        state.config.pane.show_titles = show_titles;
        state.config.pane.highlight_focused_titlebar = highlight_focused_titlebar;
        state.config.pane.padding = padding;
        state.current_mut().workspaces[0].panes[0].floating = floating;
    }
    backend.render();
    backend.capture_frame()
}

fn title_backend(mode: PaneTitlebarMode, title_style: CapStyle) -> TestBackend<AppRoot> {
    let mut backend = backend(PaneBorderMode::Separate, 1);
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 48,
        h: 12,
    });
    let state = backend.state_mut();
    state.config.pane.show_titles = true;
    state.config.pane.titlebar = mode;
    state.config.pane.title_style = title_style;
    state.config.pane.border_style = PaneBorderStyle::Rounded;
    let pane = &mut state.current_mut().workspaces[0].panes[0];
    pane.floating_rect = FloatRect {
        x: 0.0,
        y: 0.0,
        w: 48.0,
        h: 12.0,
    };
    pane.set_custom_title(TITLE);
    pane.terminal.process_server_output(b"body");
    backend
}

#[test]
fn pane_titlebar_layouts_and_visibility_use_the_expected_frame_rows() {
    rozi::test_support::on_deeper_stack(|| {
        let bar = render_title(PaneTitlebarMode::Bar, true, CapStyle::Padded, false);
        let border = render_title(PaneTitlebarMode::Border, true, CapStyle::Padded, false);
        let floating_border = render_title(PaneTitlebarMode::Border, true, CapStyle::Padded, true);
        let integrated = render_title(PaneTitlebarMode::Integrated, true, CapStyle::Padded, false);
        let integrated_half =
            render_title(PaneTitlebarMode::Integrated, true, CapStyle::Half, false);
        let integrated_round =
            render_title(PaneTitlebarMode::Integrated, true, CapStyle::Round, false);
        let disabled = render_title(PaneTitlebarMode::Border, false, CapStyle::Padded, false);

        assert!(bar[0].contains(TITLE), "bar title row: {}", bar[0]);
        assert!(bar[1].starts_with('╭'), "bar frame: {}", bar[1]);

        assert!(border[0].contains(TITLE), "border title: {}", border[0]);
        assert!(
            border[0].starts_with("╭─󰖲  "),
            "border title spacing: {}",
            border[0]
        );
        assert!(
            border[0].contains("─󰖲  nvim src/pane.rs─"),
            "border title label: {}",
            border[0]
        );
        assert!(border[1].starts_with('│'), "border body: {}", border[1]);
        assert!(
            floating_border[0].starts_with("╔═󰹙  nvim src/pane.rs"),
            "floating border title: {}",
            floating_border[0]
        );
        assert!(
            floating_border[0].contains("═floating═╗"),
            "floating border status: {}",
            floating_border[0]
        );

        assert!(
            integrated[0].contains(TITLE),
            "integrated title: {}",
            integrated[0]
        );
        assert!(
            !integrated[0].starts_with('┌'),
            "integrated header replaces the top border: {}",
            integrated[0]
        );
        assert!(
            integrated[1].starts_with('│'),
            "integrated body: {}",
            integrated[1]
        );
        assert!(
            integrated_half[0].starts_with("▐ 󰖲  nvim src/pane.rs"),
            "integrated half-block corners: {}",
            integrated_half[0]
        );
        assert!(
            integrated_half[0].ends_with('▌'),
            "integrated half-block right corner: {}",
            integrated_half[0]
        );
        assert!(
            integrated_round[0].contains('\u{e0b6}') && integrated_round[0].contains('\u{e0b4}'),
            "integrated round caps: {}",
            integrated_round[0]
        );

        let inset = render_title(PaneTitlebarMode::Inset, true, CapStyle::Padded, false);
        assert!(
            inset[0].starts_with('╭') && inset[0].ends_with('╮'),
            "inset keeps an unbroken top border: {}",
            inset[0]
        );
        assert!(
            inset[1].starts_with("│ 󰖲  nvim src/pane.rs"),
            "inset title on the first interior row: {}",
            inset[1]
        );
        assert!(
            inset[1].ends_with('│'),
            "inset title stays inside the side borders: {}",
            inset[1]
        );
        assert!(
            inset[2].starts_with("│body"),
            "inset body follows the title row: {}",
            inset[2]
        );
        // The quiet layout: foreground only. The title row must carry the pane's own
        // background, not a titlebar fill, and no cap style may put one back.
        for title_style in [CapStyle::Padded, CapStyle::Round, CapStyle::Half] {
            let frame = render_capture(PaneTitlebarMode::Inset, true, title_style, false, true);
            let title_row_bgs: Vec<_> = (1..frame.width).map(|x| frame.cell(x, 1).bg).collect();
            let body_bg = frame.cell(1, 3).bg;
            assert!(
                title_row_bgs.iter().all(|bg| *bg == body_bg),
                "{title_style:?} inset title row must not be filled: {title_row_bgs:?} vs body {body_bg:?}"
            );
        }

        // The title row gives up the frame's padding so it stays flush under the border and
        // aligned with where `border` puts its label; the configured padding applies to the
        // terminal below it instead.
        let inset_padded = render_padded(PaneTitlebarMode::Inset, (1, 1, 1, 1));
        assert!(
            inset_padded[1].starts_with("│ 󰖲  nvim src/pane.rs"),
            "padding never shifts the title row: {}",
            inset_padded[1]
        );
        assert!(
            inset_padded[3].starts_with("│ body"),
            "padding still applies below the title row: {}",
            inset_padded[3]
        );

        assert!(
            disabled[0].starts_with('╭'),
            "disabled frame: {}",
            disabled[0]
        );
        assert!(
            !disabled[0].contains(TITLE),
            "disabled title: {}",
            disabled[0]
        );

        for mode in PaneTitlebarMode::all().iter().copied() {
            let focused = render_capture(mode, true, CapStyle::Padded, false, true);
            let unfocused_titlebar = render_capture(mode, true, CapStyle::Padded, false, false);
            let focused_title = focused
                .cells
                .iter()
                .find(|cell| cell.symbol == "n")
                .expect("focused title cell");
            let unfocused_title = unfocused_titlebar
                .cells
                .iter()
                .find(|cell| cell.symbol == "n")
                .expect("unfocused title cell");
            assert!(
                focused_title.fg != unfocused_title.fg
                    || focused_title.bg != unfocused_title.bg
                    || focused_title.modifiers.bold != unfocused_title.modifiers.bold,
                "focused titlebar toggle has no visible effect for {mode:?}"
            );
        }
    });
}

#[test]
fn pane_titlebar_omits_nerd_glyphs_when_nerd_icons_are_off() {
    rozi::test_support::on_deeper_stack(|| {
        let mut backend = title_backend(PaneTitlebarMode::Border, CapStyle::Round);
        backend.state_mut().config.nerd_icons = false;
        backend.render();
        let line = backend.capture_frame().to_fixed_grid_lines()[0].clone();
        assert!(
            line.contains(TITLE),
            "title still renders without nerd icons: {line}"
        );
        assert!(
            !line.contains('󰖲') && !line.contains('\u{e0b6}') && !line.contains('\u{e0b4}'),
            "nerd glyphs stay off when nerd_icons is false: {line}"
        );
    });
}

#[test]
fn floating_title_caps_clear_underlying_terminal_attributes() {
    rozi::test_support::isolate_user_dirs();
    rozi::test_support::on_deeper_stack(|| {
        // Bar/Round and Integrated/Half take the two title widgets and the two cap glyph families.
        for (mode, caps) in [
            (PaneTitlebarMode::Bar, CapStyle::Round),
            (PaneTitlebarMode::Integrated, CapStyle::Half),
        ] {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 48,
                h: 12,
            });
            let state = backend.state_mut();
            state.config.animations.enabled = false;
            state.config.pane.show_workbar = false;
            state.config.pane.titlebar = mode;
            state.config.pane.title_style = caps;
            state.config.pane.padding = (0, 0, 0, 0);
            let mut tiled = Pane::new(
                10,
                5_000,
                FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 48.0,
                    h: 12.0,
                },
            );
            tiled.opening = false;
            tiled.terminal_active = true;
            let workspace = &mut state.current_mut().workspaces[0];
            workspace.panes = vec![tiled];
            workspace.tile_tree =
                build_dwindle_tree(&[10], workspace.start_axis, &workspace.split_ratios);
            backend.render();
            let output = (1..=10)
                .map(|row| format!("\x1b[{row};1H\x1b[1;2;3;4;7;9m{}", "x".repeat(40)))
                .collect::<String>();
            backend.state_mut().current_mut().workspaces[0].panes[0]
                .terminal
                .process_server_output(output.as_bytes());
            backend.render();
            let underneath = backend.capture_frame();
            let mut floating = Pane::new(
                11,
                5_000,
                FloatRect {
                    x: 8.0,
                    y: 4.0,
                    w: 24.0,
                    h: 5.0,
                },
            );
            floating.floating = true;
            floating.opening = false;
            floating.terminal_active = true;
            floating.set_custom_title("floating cap");
            backend.state_mut().current_mut().workspaces[0]
                .panes
                .push(floating);
            backend.state_mut().current_mut().focused_pane = Some(11);
            backend.state_mut().current_mut().workspaces[0].focused_pane = Some(11);
            backend.render();
            let frame = backend.capture_frame();
            let (left, right) = caps.glyphs().expect("capped style");
            let mut found = 0;
            for y in 3..9 {
                for x in 7..33 {
                    let cell = frame.cell(x, y);
                    if cell.symbol == left || cell.symbol == right {
                        assert!(
                            underneath.cell(x, y).modifiers.dim,
                            "cap must overlap dim terminal text"
                        );
                        assert_eq!(
                            cell.modifiers,
                            tui_lipan::capture::CellModifiers::default(),
                            "{mode:?}/{caps:?} cap inherited terminal attributes at {x},{y}"
                        );
                        let inner_x = if cell.symbol == left { x + 1 } else { x - 1 };
                        assert_eq!(
                            cell.fg,
                            frame.cell(inner_x, y).bg,
                            "cap fill must match the titlebar background"
                        );
                        found += 1;
                    }
                }
            }
            assert_eq!(found, 2, "{mode:?}/{caps:?} must render both caps");
        }
    });
}

/// Equal-length titles so the seam row and the top pane's row differ only in the title itself.
const TOP_TITLE: &str = "pane-aa";
const BOTTOM_TITLE: &str = "pane-bb";

const TOP: PaneId = 10;
const BOTTOM: PaneId = 11;

/// Every titlebar layout puts the icon in column 2: `  󰖲`, `▐ 󰖲`, `╭─󰖲`, and `╭<cap>󰖲` alike.
const ICON_X: u16 = 2;

const MODES: [PaneTitlebarMode; 2] = [PaneTitlebarMode::Border, PaneTitlebarMode::Integrated];
/// Padded and Half own the seam row through separate composition paths.
/// Round covers the generic capped path shared with Arrow, with inset corner junctions.
const CAPS: [CapStyle; 3] = [CapStyle::Padded, CapStyle::Half, CapStyle::Round];

/// Two tiles stacked top/bottom in merged-border mode, with `focused` holding the focus.
fn render_seam(mode: PaneTitlebarMode, title_style: CapStyle, focused: PaneId) -> CapturedFrame {
    render_titled(mode, title_style, focused, &[TOP_TITLE, BOTTOM_TITLE])
}

/// A dwindle layout of `titles.len()` merged tiles, the first split stacking them top/bottom.
fn render_titled(
    mode: PaneTitlebarMode,
    title_style: CapStyle,
    focused: PaneId,
    titles: &[&str],
) -> CapturedFrame {
    let mut backend = backend(PaneBorderMode::Merged, titles.len());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 48,
        h: 16,
    });
    {
        let state = backend.state_mut();
        state.config.pane.show_titles = true;
        state.config.pane.titlebar = mode;
        state.config.pane.title_style = title_style;
        state.config.pane.border_style = PaneBorderStyle::Rounded;
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.start_axis = SplitAxis::Vertical;
        let ids: Vec<_> = workspace.panes.iter().map(|pane| pane.id).collect();
        workspace.tile_tree = build_dwindle_tree(&ids, workspace.start_axis, &[]);
        for (pane, title) in workspace.panes.iter_mut().zip(titles) {
            pane.set_custom_title(title);
            pane.terminal.process_server_output(b"body");
        }
        workspace.focused_pane = Some(focused);
        state.current_mut().focused_pane = Some(focused);
    }
    backend.render();
    backend.capture_frame()
}

/// The row carrying the lower pane's titlebar - the seam it shares with the tile above it.
fn seam_row(frame: &CapturedFrame) -> u16 {
    let lines = frame.to_fixed_grid_lines();
    let row = lines
        .iter()
        .position(|line| line.contains(BOTTOM_TITLE))
        .unwrap_or_else(|| panic!("lower pane title missing entirely:\n{}", lines.join("\n")));
    assert!(row > 0, "the lower pane cannot be the top row");
    row as u16
}

#[test]
fn seam_titlebar_matches_the_in_frame_titlebar_layout() {
    rozi::test_support::on_deeper_stack(|| {
        for mode in MODES {
            for title_style in CAPS {
                let frame = render_seam(mode, title_style, TOP);
                let lines = frame.to_fixed_grid_lines();
                let seam = seam_row(&frame);
                let top_row = &lines[0];
                let seam_row_text = &lines[seam as usize];
                let expected = top_row.replace(TOP_TITLE, BOTTOM_TITLE);

                let owns_full_row = mode == PaneTitlebarMode::Integrated
                    && matches!(title_style, CapStyle::Padded | CapStyle::Half);
                if owns_full_row {
                    // The strip reproduces the frame's top-edge decoration, corner cells included.
                    assert_eq!(
                        &expected, seam_row_text,
                        "{mode:?}/{title_style:?} seam row differs from the in-frame titlebar"
                    );
                } else {
                    // The strip is inset past the frame's corner glyphs, which stay on the seam and
                    // fuse with the neighbor's border into junctions.
                    let inner = |line: &str| {
                        let chars: Vec<char> = line.chars().collect();
                        chars[1..chars.len() - 1].iter().collect::<String>()
                    };
                    assert_eq!(
                        inner(&expected),
                        inner(seam_row_text),
                        "{mode:?}/{title_style:?} seam row differs from the in-frame titlebar"
                    );
                    let corners: Vec<char> = seam_row_text.chars().collect();
                    assert_eq!(
                        (corners[0], corners[corners.len() - 1]),
                        ('├', '┤'),
                        "{mode:?}/{title_style:?} seam corners should fuse with the tile above"
                    );
                }
            }
        }
    });
}

#[test]
fn seam_titlebar_carries_the_same_focus_styling_as_an_in_frame_titlebar() {
    rozi::test_support::on_deeper_stack(|| {
        for mode in MODES {
            for title_style in CAPS {
                // The top pane's row is never on a seam, so it is the reference rendering. Compare
                // it against the seam strip in the matching focus state: focused vs focused,
                // unfocused vs unfocused.
                for (focus_for_top, focus_for_seam) in [(TOP, BOTTOM), (BOTTOM, TOP)] {
                    let reference = render_seam(mode, title_style, focus_for_top);
                    let seamed = render_seam(mode, title_style, focus_for_seam);
                    let seam = seam_row(&seamed);

                    let expected = reference.cell(ICON_X, 0);
                    let actual = seamed.cell(ICON_X, seam);
                    let focus_state = if focus_for_top == TOP {
                        "focused"
                    } else {
                        "unfocused"
                    };
                    assert_eq!(
                        (expected.fg, expected.bg, expected.modifiers.bold),
                        (actual.fg, actual.bg, actual.modifiers.bold),
                        "{mode:?}/{title_style:?} {focus_state} seam titlebar styling \
                         differs from the in-frame titlebar"
                    );
                }
            }
        }
    });
}

/// A wide tile over two side-by-side tiles: both lower titlebars share the same seam row, and
/// their strips must not swallow the vertical junction where they meet.
#[test]
fn side_by_side_tiles_below_a_seam_keep_their_titles_and_junction() {
    const TITLES: [&str; 3] = ["pane-aa", "pane-bb", "pane-cc"];
    rozi::test_support::on_deeper_stack(|| {
        for mode in MODES {
            for title_style in CAPS {
                // Focus each tile in turn: the focused pane draws last, so this is what used to
                // decide which of the lower titlebars survived.
                // Upper focus is the draw-last case that used to bury the lower titles. One
                // lower pane focused covers the other direction.
                for focused in [TOP, TOP + 1] {
                    let frame = render_titled(mode, title_style, focused, &TITLES);
                    let lines = frame.to_fixed_grid_lines();
                    let joined = lines.join("\n");
                    for title in TITLES {
                        assert!(
                            joined.contains(title),
                            "{title} missing for {mode:?}/{title_style:?} focus={focused}:\n{joined}"
                        );
                    }

                    let seam = lines
                        .iter()
                        .find(|line| line.contains(TITLES[1]))
                        .expect("seam row");
                    assert!(
                        seam.contains(TITLES[2]),
                        "{mode:?}/{title_style:?} focus={focused}: both lower titles share the \
                         seam row:\n{joined}"
                    );
                    // Inset strips leave the frame corners on the seam, so the column the two lower
                    // tiles share still fuses into a `┬` with the divider below. Padded and Half
                    // integrated titles own their corner cells instead, on a seam as off one.
                    let owns_full_row = mode == PaneTitlebarMode::Integrated
                        && matches!(title_style, CapStyle::Padded | CapStyle::Half);
                    if !owns_full_row {
                        assert!(
                            seam.contains('┬'),
                            "{mode:?}/{title_style:?} focus={focused}: lost the junction between \
                             the two lower tiles:\n{seam}"
                        );
                    }
                }
            }
        }
    });
}
