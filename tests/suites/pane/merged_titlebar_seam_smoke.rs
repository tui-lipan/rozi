//! Merged borders overlap stacked tiles by one row, so a lower pane's `Border`/`Integrated`
//! titlebar shares that row with the upper pane's bottom border. Whichever pane draws last owns
//! the row, and `ordered_panes` draws the focused pane last - so the title under the focused pane
//! used to be painted over by its neighbor's border. The title is lifted into its own strip layer
//! above every tile; these tests pin that it survives and still looks like an in-frame titlebar.

use rozi::AppRoot;
use rozi::state::{Pane, PaneBorderMode, PaneBorderStyle, PaneId, PaneTitlebarMode, SplitAxis};
use rozi::tiling::build_dwindle_tree;
use tui_lipan::prelude::{CapStyle, FloatRect, Rect};
use tui_lipan::{CapturedFrame, TestBackend};

/// Equal-length titles so the seam row and the top pane's row differ only in the title itself.
const TOP_TITLE: &str = "pane-aa";
const BOTTOM_TITLE: &str = "pane-bb";

const TOP: PaneId = 10;
const BOTTOM: PaneId = 11;

/// Every titlebar layout puts the icon in column 2: `  󰖲`, `▐ 󰖲`, `╭─󰖲`, and `╭<cap>󰖲` alike.
const ICON_X: u16 = 2;

const MODES: [PaneTitlebarMode; 2] = [PaneTitlebarMode::Border, PaneTitlebarMode::Integrated];
const CAPS: [CapStyle; 4] = [
    CapStyle::Padded,
    CapStyle::Half,
    CapStyle::Round,
    CapStyle::Arrow,
];

/// Two tiles stacked top/bottom in merged-border mode, with `focused` holding the focus.
fn render(mode: PaneTitlebarMode, title_style: CapStyle, focused: PaneId) -> CapturedFrame {
    render_titled(mode, title_style, focused, &[TOP_TITLE, BOTTOM_TITLE])
}

/// A dwindle layout of `titles.len()` merged tiles, the first split stacking them top/bottom.
fn render_titled(
    mode: PaneTitlebarMode,
    title_style: CapStyle,
    focused: PaneId,
    titles: &[&str],
) -> CapturedFrame {
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 48,
        h: 16,
    });
    {
        let state = backend.state_mut();
        state.config.pane.show_workbar = false;
        state.config.pane.show_titles = true;
        state.config.pane.titlebar = mode;
        state.config.pane.title_style = title_style;
        state.config.pane.border_mode = PaneBorderMode::Merged;
        state.config.pane.border_style = PaneBorderStyle::Rounded;
        state.current_mut().workspaces[0].panes.clear();
        state.current_mut().workspaces[0].tile_tree = None;
        state.current_mut().workspaces[0].start_axis = SplitAxis::Vertical;
        let ids: Vec<PaneId> = (0..titles.len())
            .map(|index| TOP + index as PaneId)
            .collect();
        for (id, title) in ids.iter().zip(titles) {
            let mut pane = Pane::new(
                *id,
                5_000,
                FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 48.0,
                    h: 8.0,
                },
            );
            pane.opening = false;
            pane.terminal_active = true;
            pane.set_custom_title(title);
            pane.terminal.process_server_output(b"body");
            state.current_mut().workspaces[0].panes.push(pane);
        }
        let start_axis = state.current().workspaces[0].start_axis;
        let ratios = state.current().workspaces[0].split_ratios.clone();
        state.current_mut().workspaces[0].tile_tree = build_dwindle_tree(&ids, start_axis, &ratios);
        state.current_mut().focused_pane = Some(focused);
        state.current_mut().workspaces[0].focused_pane = Some(focused);
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

fn run<F: FnOnce() + Send + 'static>(body: F) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(body)
        .expect("spawn merged titlebar seam test")
        .join()
        .expect("merged titlebar seam test completes");
}

#[test]
fn merged_stacked_panes_keep_both_titlebars() {
    run(|| {
        for mode in MODES {
            for title_style in CAPS {
                // Both focus positions: the focused pane draws last, so focusing the *upper* pane
                // is what used to bury the lower pane's titlebar.
                for focused in [TOP, BOTTOM] {
                    let frame = render(mode, title_style, focused);
                    let lines = frame.to_fixed_grid_lines();
                    let joined = lines.join("\n");
                    for title in [TOP_TITLE, BOTTOM_TITLE] {
                        assert!(
                            joined.contains(title),
                            "{title} missing for {mode:?}/{title_style:?} focus={focused}:\n{joined}"
                        );
                    }
                }
            }
        }
    });
}

#[test]
fn seam_titlebar_matches_the_in_frame_titlebar_layout() {
    run(|| {
        for mode in MODES {
            for title_style in CAPS {
                let frame = render(mode, title_style, TOP);
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
    run(|| {
        for mode in MODES {
            for title_style in CAPS {
                // The top pane's row is never on a seam, so it is the reference rendering. Compare
                // it against the seam strip in the matching focus state: focused vs focused,
                // unfocused vs unfocused.
                for (focus_for_top, focus_for_seam) in [(TOP, BOTTOM), (BOTTOM, TOP)] {
                    let reference = render(mode, title_style, focus_for_top);
                    let seamed = render(mode, title_style, focus_for_seam);
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
    run(|| {
        for mode in MODES {
            for title_style in CAPS {
                // Focus each tile in turn: the focused pane draws last, so this is what used to
                // decide which of the lower titlebars survived.
                for focused in [TOP, TOP + 1, TOP + 2] {
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
