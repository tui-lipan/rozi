//! Production workbar caps and workspace tabs: edge glyphs, powerline chaining, pointer hit-testing
//! through the decorative overlay, and dragging one tab onto another. The cases render `AppRoot`,
//! not a hand-built stand-in for `view::workbar`.

use rozi::AppRoot;
use rozi::config::{WorkbarItem, WorkbarSegment};
use tui_lipan::core::event::{MouseButton, MouseEvent, MouseKind};
use tui_lipan::prelude::{CapStyle, KeyMods, Rect};
use tui_lipan::{CapturedCell, PointerShape, TestBackend};

const LEFT_CAP: &str = "\u{e0b6}";
const RIGHT_CAP: &str = "\u{e0b4}";

fn backend() -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 100,
        h: 8,
    });
    {
        let state = backend.state_mut();
        state.config.animations.enabled = false;
        state.config.sidebar.startup = false;
        state.sidebar.hide_all();
        state.config.nerd_icons = true;
        state.config.workbar.right.clear();
    }
    backend
}

fn row(backend: &mut TestBackend<AppRoot>) -> Vec<CapturedCell> {
    backend.render();
    let frame = backend.capture_frame();
    frame.cells[..frame.width as usize].to_vec()
}

fn click(backend: &mut TestBackend<AppRoot>, x: u16) {
    for kind in [
        MouseKind::Down(MouseButton::Left),
        MouseKind::Up(MouseButton::Left),
    ] {
        backend
            .send_mouse(MouseEvent {
                x,
                y: 0,
                kind,
                mods: KeyMods::NONE,
            })
            .expect("workbar click");
    }
}

fn find_run(row: &[CapturedCell], needle: &str) -> Option<usize> {
    let chars: Vec<char> = needle.chars().collect();
    row.windows(chars.len()).position(|window| {
        window
            .iter()
            .zip(&chars)
            .all(|(cell, ch)| cell.symbol == ch.to_string())
    })
}

#[test]
fn edge_caps_take_the_end_segment_colors_and_leave_the_title_in_place() {
    rozi::test_support::on_deeper_stack(|| {
        let mut capped = backend();
        capped.state_mut().config.pane.workbar_style = CapStyle::Round;
        let theme = capped.state().theme.clone();
        let cells = row(&mut capped);
        let left = &cells[0];
        let right = cells.last().expect("workbar row");
        assert_eq!(
            left.symbol, LEFT_CAP,
            "left cap occupies the title's padding cell"
        );
        assert_eq!(
            left.fg, theme.border_active,
            "left cap uses the title badge"
        );
        assert_eq!(
            left.bg, theme.surface.backdrop,
            "left cap sits on the backdrop"
        );
        assert_eq!(cells[1].symbol, "r", "the title stays in the next cell");
        assert_eq!(
            right.symbol, RIGHT_CAP,
            "right cap occupies the trailing cell"
        );
        assert_eq!(
            right.fg, theme.surface.panel,
            "a bar with no trailing badge keeps a panel-colored right cap"
        );
        assert_eq!(right.bg, theme.surface.backdrop);

        let mut flush = backend();
        flush.state_mut().config.pane.workbar_style = CapStyle::Padded;
        let cells = row(&mut flush);
        assert_eq!(cells[0].symbol, " ", "a padded bar keeps its leading space");
        assert_eq!(cells[1].symbol, "r");
        assert_ne!(cells[0].symbol, LEFT_CAP);
        assert_ne!(cells.last().expect("row").symbol, RIGHT_CAP);
    });
}

#[test]
fn trailing_badges_chain_only_while_powerline_is_on() {
    rozi::test_support::on_deeper_stack(|| {
        let prepare = |powerline: bool| {
            let mut backend = backend();
            {
                let state = backend.state_mut();
                state.config.pane.workbar_style = CapStyle::Round;
                state.config.pane.workbar_badge_style = CapStyle::Round;
                state.config.pane.workbar_powerline = powerline;
                state.do_not_disturb = true;
                state.config.workbar.right = vec![WorkbarItem {
                    segment: WorkbarSegment::Session,
                    color: None,
                }];
                state.current_mut().session_name = Some("dev".into());
                state.current_mut().session_attached = true;
            }
            let theme = backend.state().theme.clone();
            let cells = row(&mut backend);
            (theme, cells)
        };

        let (theme, chained) = prepare(true);
        let dnd = find_run(&chained, "DND").expect("DND badge");
        let session = find_run(&chained, "dev").expect("session badge");
        let between = &chained[dnd + 3..session];
        let seam = between
            .iter()
            .find(|cell| cell.symbol == LEFT_CAP)
            .expect("powerline seam cap");
        assert_eq!(
            seam.fg, theme.border_active,
            "seam cap is the session color"
        );
        assert_eq!(
            seam.bg, theme.status.warning,
            "seam cap blends over the DND neighbor"
        );
        assert!(
            between.iter().all(|cell| cell.symbol != RIGHT_CAP),
            "chained badges do not close into separate pills"
        );

        let (theme, apart) = prepare(false);
        let dnd = find_run(&apart, "DND").expect("DND badge");
        let session = find_run(&apart, "dev").expect("session badge");
        let between = &apart[dnd + 3..session];
        let closing = between
            .iter()
            .find(|cell| cell.symbol == RIGHT_CAP)
            .expect("standalone DND chip rounds off");
        assert_eq!(closing.fg, theme.status.warning);
        assert_eq!(closing.bg, theme.surface.panel, "pill cap sits on the bar");
        assert!(
            between
                .iter()
                .any(|cell| cell.symbol == " " && cell.bg == theme.surface.panel),
            "powerline off leaves a gap between badges"
        );
    });
}

#[test]
fn cap_overlay_lets_workspace_tabs_and_the_title_receive_pointer_events() {
    rozi::test_support::on_deeper_stack(|| {
        let mut backend = backend();
        backend.state_mut().config.pane.workbar_style = CapStyle::Round;
        let cells = row(&mut backend);
        let tab = find_run(&cells, "2").expect("second workspace tab") as u16;

        backend
            .send_mouse(MouseEvent {
                x: tab,
                y: 0,
                kind: MouseKind::Moved,
                mods: KeyMods::NONE,
            })
            .expect("hover tab");
        assert_eq!(
            backend.pointer_shape_at(tab, 0),
            Some(PointerShape::Pointer),
            "the cap overlay must not hide the tab's hit target"
        );
        click(&mut backend, tab);
        assert_eq!(backend.state().current().active_workspace, 1);

        click(&mut backend, 0);
        assert!(
            backend.state().show_palette,
            "the left cap must pass the click through to the title badge"
        );
    });
}

#[test]
fn dropping_a_workspace_tab_on_another_swaps_the_two_and_the_view_follows() {
    rozi::test_support::on_deeper_stack(|| {
        let mut backend = backend();
        {
            let workspaces = &mut backend.state_mut().current_mut().workspaces;
            workspaces[0].name = Some("code".to_string());
            workspaces[2].name = Some("docs".to_string());
        }
        let cells = row(&mut backend);
        let source = find_run(&cells, "1:code").expect("first workspace tab") as u16;
        let target = find_run(&cells, "3:docs").expect("third workspace tab") as u16;

        for (x, kind) in [
            (source, MouseKind::Down(MouseButton::Left)),
            (source + 2, MouseKind::Drag(MouseButton::Left)),
            (target, MouseKind::Drag(MouseButton::Left)),
            (target, MouseKind::Up(MouseButton::Left)),
        ] {
            backend
                .send_mouse(MouseEvent {
                    x,
                    y: 0,
                    kind,
                    mods: KeyMods::NONE,
                })
                .expect("workbar drag");
        }

        let workspaces = &backend.state().current().workspaces;
        assert_eq!(workspaces[2].name.as_deref(), Some("code"));
        assert_eq!(workspaces[0].name.as_deref(), Some("docs"));
        assert_eq!(
            backend.state().current().active_workspace,
            2,
            "the view follows the dragged workspace to its new number"
        );
        let cells = row(&mut backend);
        let docs = find_run(&cells, "1:docs").expect("docs now holds the first slot");
        let code = find_run(&cells, "3:code").expect("code now holds the third slot");
        assert!(docs < code, "tabs keep their numeric order");
    });
}
