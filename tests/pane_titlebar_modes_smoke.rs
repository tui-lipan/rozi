//! Pins the visible pane titlebar layouts and their independent visibility switch against a real
//! `AppRoot` render. Only `Bar` may add a row before the frame.

use rozi::AppRoot;
use rozi::state::{Pane, PaneId, PaneTitlebarMode};
use rozi::tiling::build_dwindle_tree;
use tui_lipan::TestBackend;
use tui_lipan::prelude::{CapStyle, FloatRect, Rect};

const TITLE: &str = "nvim src/pane.rs";

fn render(
    mode: PaneTitlebarMode,
    show_titles: bool,
    title_style: CapStyle,
    floating: bool,
) -> Vec<String> {
    render_capture(mode, show_titles, title_style, floating, true).to_fixed_grid_lines()
}

fn render_capture(
    mode: PaneTitlebarMode,
    show_titles: bool,
    title_style: CapStyle,
    floating: bool,
    highlight_focused_titlebar: bool,
) -> tui_lipan::CapturedFrame {
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 48,
        h: 12,
    });
    {
        let state = backend.state_mut();
        state.config.pane.show_workbar = false;
        state.config.pane.show_titles = show_titles;
        state.config.pane.titlebar = mode;
        state.config.pane.title_style = title_style;
        state.config.pane.highlight_focused_titlebar = highlight_focused_titlebar;
        state.config.pane.border_style = rozi::state::PaneBorderStyle::Rounded;
        state.current_mut().workspaces[0].panes.clear();
        state.current_mut().workspaces[0].tile_tree = None;
        let mut pane = Pane::new(
            10,
            5_000,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 48.0,
                h: 12.0,
            },
        );
        pane.opening = false;
        pane.terminal_active = true;
        pane.floating = floating;
        pane.set_custom_title(TITLE);
        pane.terminal.process_server_output(b"body");
        state.current_mut().workspaces[0].panes.push(pane);
        let id = 10 as PaneId;
        let start_axis = state.current().workspaces[0].start_axis;
        let ratios = state.current().workspaces[0].split_ratios.clone();
        state.current_mut().workspaces[0].tile_tree =
            build_dwindle_tree(&[id], start_axis, &ratios);
        state.current_mut().focused_pane = Some(id);
        state.current_mut().workspaces[0].focused_pane = Some(id);
    }
    backend.render();
    backend.capture_frame()
}

#[test]
fn pane_titlebar_layouts_and_visibility_use_the_expected_frame_rows() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let bar = render(PaneTitlebarMode::Bar, true, CapStyle::Padded, false);
            let border = render(PaneTitlebarMode::Border, true, CapStyle::Padded, false);
            let floating_border = render(PaneTitlebarMode::Border, true, CapStyle::Padded, true);
            let integrated = render(PaneTitlebarMode::Integrated, true, CapStyle::Padded, false);
            let integrated_half = render(PaneTitlebarMode::Integrated, true, CapStyle::Half, false);
            let integrated_round =
                render(PaneTitlebarMode::Integrated, true, CapStyle::Round, false);
            let disabled = render(PaneTitlebarMode::Border, false, CapStyle::Padded, false);

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
                floating_border[0].contains("· floating═╗"),
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
                integrated_round[0].contains('\u{e0b6}')
                    && integrated_round[0].contains('\u{e0b4}'),
                "integrated round caps: {}",
                integrated_round[0]
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

            for mode in [
                PaneTitlebarMode::Bar,
                PaneTitlebarMode::Border,
                PaneTitlebarMode::Integrated,
            ] {
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
        })
        .expect("spawn pane-titlebar smoke test")
        .join()
        .expect("pane-titlebar smoke test completes");
}
