//! Keyboard navigation of the sidebar's row list.
//!
//! The sidebar is deliberately outside the Tab ring and outside click-to-focus, so `focus-sidebar`
//! is the only way the keyboard gets in and Escape is the way out. These cover that round trip plus
//! the two things the list has to get right once it has focus: stepping over non-selectable section
//! headers, and Enter resolving to the same action a click would have run.

use rozi::config::{
    SidebarLauncherEntry, SidebarTab, SidebarTabId, SidebarTreeConfig, SidebarTreeView,
    UserCommandAction,
};
use rozi::input::Action;
use rozi::state::Pane;
use rozi::{AppRoot, Msg};
use tui_lipan::TestBackend;
use tui_lipan::core::event::{MouseButton, MouseEvent, MouseKind};
use tui_lipan::prelude::{Color, KeyCode, KeyEvent, KeyMods, Rect};
use tui_lipan::style::geometry::FloatRect;

/// A pane in the state a live one is in: its terminal is up and the open animation has finished.
/// Both matter — `request_pane_focus` silently no-ops on an `opening` or inactive pane, so a pane
/// left in `Pane::new`'s defaults cannot take focus back and would hide any bug where something
/// else re-grabs it.
fn pane(id: u32) -> Pane {
    let mut pane = Pane::new(
        id,
        100,
        FloatRect {
            x: 0.0,
            y: 0.0,
            w: 20.0,
            h: 10.0,
        },
    );
    pane.opening = false;
    pane.terminal_active = true;
    pane
}

/// Focus notifications are queued after reconciliation, so a focus change needs another pass
/// before `on_focus` has turned into state.
fn settle(backend: &mut TestBackend<AppRoot>) {
    for _ in 0..4 {
        backend.render();
        let _ = backend.pump();
    }
    backend.render();
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        mods: KeyMods::NONE,
    }
}

fn modified_key(code: KeyCode, mods: KeyMods) -> KeyEvent {
    KeyEvent { code, mods }
}

/// Every backend in this binary is built here: reordering tabs and toggling the split persist
/// `[sidebar]`, so the config has to resolve out of a scratch root rather than the developer's own
/// (`rozi::test_support`).
fn sidebar_backend() -> TestBackend<AppRoot> {
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 100,
        h: 30,
    });
    backend
}

/// Two workspaces, so the list carries section headers between the selectable rows.
fn backend_with_panes() -> TestBackend<AppRoot> {
    let mut backend = sidebar_backend();
    {
        let state = backend.state_mut();
        state.sidebar_visible = true;
        state.config.sidebar.tabs = vec![SidebarTab::Panes];
        state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("panes"));
        state.current_mut().workspaces[0].panes = vec![pane(1), pane(2)];
        state.current_mut().workspaces[1].panes = vec![pane(3)];
        state.current_mut().focused_pane = Some(1);
    }
    backend.render();
    backend
}

#[test]
fn focus_sidebar_then_escape_is_a_round_trip() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_panes();
            assert!(
                !backend.state().sidebar.focused,
                "the sidebar starts as a passive readout"
            );

            backend
                .dispatch(Msg::RunAction(Action::FocusSidebar))
                .expect("focus sidebar");
            settle(&mut backend);
            assert!(backend.state().sidebar.focused);

            let _ = backend.send_key(key(KeyCode::Esc));
            settle(&mut backend);
            assert!(
                !backend.state().sidebar.focused,
                "Escape hands the keyboard back to the pane"
            );
        })
        .expect("spawn round trip thread")
        .join()
        .expect("round trip completes");
}

fn backend_with_explorer() -> TestBackend<AppRoot> {
    let mut backend = sidebar_backend();
    {
        let state = backend.state_mut();
        let mut config = SidebarTreeConfig::for_view(SidebarTreeView::Files);
        config.explorer = true;
        let tab = SidebarTab::Tree {
            view: SidebarTreeView::Files,
            config,
        };
        state.sidebar_visible = true;
        state.sidebar.panels[0].tabs = vec![tab.id()];
        state.sidebar.panels[0].active_tab = Some(tab.id());
        state.config.sidebar.tabs = vec![tab];
        let mut pane = pane(1);
        pane.terminal.cwd = Some(env!("CARGO_MANIFEST_DIR").to_string());
        state.current_mut().workspaces[0].panes = vec![pane];
        state.current_mut().focused_pane = Some(1);
        state.sidebar.tree_cwd = Some(env!("CARGO_MANIFEST_DIR").to_string());
    }
    backend.render();
    backend
}

fn explorer_input_position(backend: &TestBackend<AppRoot>) -> (u16, u16) {
    backend
        .capture_frame()
        .to_fixed_grid_lines()
        .iter()
        .enumerate()
        .find_map(|(y, line)| {
            line.chars()
                .position(|ch| ch == 'F')
                .filter(|_| line.contains("Find files"))
                .map(|x| (x as u16, y as u16))
        })
        .expect("explorer input is visible")
}

fn click(backend: &mut TestBackend<AppRoot>, x: u16, y: u16) {
    for kind in [
        MouseKind::Down(MouseButton::Left),
        MouseKind::Up(MouseButton::Left),
    ] {
        backend
            .send_mouse(MouseEvent {
                x,
                y,
                kind,
                mods: KeyMods::NONE,
            })
            .expect("click explorer input");
    }
}

#[test]
fn pointer_focused_explorer_escape_returns_to_the_pane() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_explorer();
            backend
                .dispatch(Msg::RunAction(Action::FocusSidebar))
                .expect("focus sidebar");
            settle(&mut backend);
            backend.send_key(key(KeyCode::Esc)).expect("leave sidebar");
            settle(&mut backend);
            let pane_focus = backend.focused_key().cloned().expect("pane has focus");

            let (x, y) = explorer_input_position(&backend);
            click(&mut backend, x, y);
            settle(&mut backend);
            assert!(
                backend
                    .focused_key()
                    .is_some_and(|key| key.as_ref().ends_with("-explorer"))
            );
            assert!(!backend.state().sidebar.focused);

            backend.send_key(key(KeyCode::Esc)).expect("leave explorer");
            settle(&mut backend);
            assert_eq!(backend.focused_key(), Some(&pane_focus));
            assert!(!backend.state().sidebar.focused);
        })
        .expect("spawn pointer explorer escape thread")
        .join()
        .expect("pointer explorer escape completes");
}

#[test]
fn slash_focused_explorer_escape_returns_to_sidebar_tree() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_explorer();
            backend
                .dispatch(Msg::RunAction(Action::FocusSidebar))
                .expect("focus sidebar");
            settle(&mut backend);

            backend
                .send_key(key(KeyCode::Char('/')))
                .expect("open explorer from tree");
            settle(&mut backend);
            assert!(
                backend
                    .focused_key()
                    .is_some_and(|key| key.as_ref().ends_with("-explorer"))
            );
            assert!(backend.state().sidebar.focused);
            assert!(backend.state().sidebar.explorer_entered_from_tree);

            backend.send_key(key(KeyCode::Esc)).expect("return to tree");
            settle(&mut backend);
            assert!(backend.focused().is_some());
            assert!(
                backend
                    .focused_key()
                    .is_none_or(|key| !key.as_ref().ends_with("-explorer"))
            );
            assert!(backend.state().sidebar.focused);
        })
        .expect("spawn keyboard explorer escape thread")
        .join()
        .expect("keyboard explorer escape completes");
}

#[test]
fn enter_in_explorer_enables_sidebar_tree_mode() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_explorer();
            backend
                .dispatch(Msg::RunAction(Action::FocusSidebar))
                .expect("focus sidebar");
            settle(&mut backend);
            backend
                .send_key(key(KeyCode::Char('/')))
                .expect("open explorer");
            settle(&mut backend);

            backend
                .send_key(key(KeyCode::Enter))
                .expect("commit explorer");
            settle(&mut backend);
            assert!(
                backend
                    .focused_key()
                    .is_some_and(|key| key.as_ref().ends_with("-tree"))
            );
            assert!(backend.state().sidebar.focused);
            assert!(!backend.state().sidebar.explorer_entered_from_tree);
        })
        .expect("spawn explorer Enter thread")
        .join()
        .expect("explorer Enter completes");
}

#[test]
fn clicking_tree_outside_pointer_focused_explorer_returns_to_pane() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_explorer();
            let input_y = explorer_input_position(&backend).1;
            click(&mut backend, 2, input_y);
            settle(&mut backend);
            assert!(!backend.state().sidebar.focused);

            click(&mut backend, 2, input_y.saturating_add(2));
            settle(&mut backend);
            assert!(!backend.state().sidebar.focused);
            assert!(
                backend
                    .focused_key()
                    .is_some_and(|key| key.as_ref().starts_with("hyprmux-terminal-"))
            );
        })
        .expect("spawn outside-click explorer thread")
        .join()
        .expect("outside-click explorer completes");
}

/// `focus-sidebar` reveals a hidden sidebar rather than silently doing nothing — otherwise the
/// binding would appear dead whenever the panel happened to be closed.
#[test]
fn focus_sidebar_reveals_a_hidden_sidebar() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_panes();
            backend.state_mut().sidebar_visible = false;
            backend.render();

            backend
                .dispatch(Msg::RunAction(Action::FocusSidebar))
                .expect("focus sidebar");
            settle(&mut backend);
            assert!(backend.state().sidebar_visible);
            assert!(backend.state().sidebar.focused);
        })
        .expect("spawn reveal thread")
        .join()
        .expect("reveal completes");
}

/// Moving down and pressing Enter focuses the pane on that row. The list holds three panes split
/// across two workspaces, so reaching the third one means the cursor stepped over a section header
/// without ever landing on it.
#[test]
fn arrow_keys_skip_headers_and_enter_activates_the_row() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_panes();
            backend
                .dispatch(Msg::RunAction(Action::FocusSidebar))
                .expect("focus sidebar");
            settle(&mut backend);

            // Row order is: [Workspace 1] first, second, [Workspace 2] third. Two steps down from
            // the first selectable row lands on `third` only if both headers were skipped.
            for _ in 0..2 {
                let _ = backend.send_key(key(KeyCode::Down));
                settle(&mut backend);
            }
            let _ = backend.send_key(key(KeyCode::Enter));
            settle(&mut backend);

            assert_eq!(
                backend.state().current().focused_pane,
                Some(3),
                "two steps down from the first row reaches the third pane, so both workspace \
                 headers were stepped over rather than selected"
            );
            assert_eq!(
                backend.state().current().active_workspace,
                1,
                "activating a row in another workspace switches to it, exactly as a click does"
            );
        })
        .expect("spawn navigation thread")
        .join()
        .expect("navigation completes");
}

/// Tab cycles sidebar tabs instead of the focus ring. The sidebar is outside that ring by design,
/// which is what frees Tab up for this.
#[test]
fn tab_cycles_sidebar_tabs_while_focused() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_panes();
            backend.state_mut().config.sidebar.tabs = vec![SidebarTab::Panes, SidebarTab::Agents];
            backend.state_mut().sidebar.panels[0].tabs =
                vec![SidebarTabId::new("panes"), SidebarTabId::new("agents")];
            backend
                .dispatch(Msg::RunAction(Action::FocusSidebar))
                .expect("focus sidebar");
            settle(&mut backend);

            let _ = backend.send_key(key(KeyCode::Tab));
            settle(&mut backend);
            assert_eq!(
                backend.state().sidebar.active_tab(),
                Some(&SidebarTabId::new("agents")),
                "Tab moves to the next sidebar tab"
            );
            assert!(
                backend.state().sidebar.focused,
                "cycling keeps the keyboard in the sidebar"
            );
        })
        .expect("spawn tab cycle thread")
        .join()
        .expect("tab cycle completes");
}

#[test]
fn left_and_right_do_not_cycle_non_tree_sidebar_tabs() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_panes();
            backend.state_mut().config.sidebar.tabs = vec![SidebarTab::Panes, SidebarTab::Agents];
            backend.state_mut().sidebar.panels[0].tabs =
                vec![SidebarTabId::new("panes"), SidebarTabId::new("agents")];
            backend
                .dispatch(Msg::RunAction(Action::FocusSidebar))
                .expect("focus sidebar");
            settle(&mut backend);

            let _ = backend.send_key(key(KeyCode::Right));
            settle(&mut backend);
            assert_eq!(
                backend.state().sidebar.active_tab(),
                Some(&SidebarTabId::new("panes")),
                "horizontal arrows belong to the active row widget, not tab cycling"
            );

            let _ = backend.send_key(key(KeyCode::Left));
            settle(&mut backend);
            assert_eq!(
                backend.state().sidebar.active_tab(),
                Some(&SidebarTabId::new("panes"))
            );
        })
        .expect("spawn arrow tab cycle thread")
        .join()
        .expect("arrow navigation completes");
}

#[test]
fn ctrl_shift_left_and_right_reorder_sidebar_tabs() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_panes();
            backend.state_mut().config.sidebar.tabs = vec![SidebarTab::Panes, SidebarTab::Agents];
            backend.state_mut().sidebar.panels[0].tabs =
                vec![SidebarTabId::new("panes"), SidebarTabId::new("agents")];
            backend.state_mut().sidebar.panels[0].active_tab = Some(SidebarTabId::new("agents"));
            backend
                .dispatch(Msg::RunAction(Action::FocusSidebar))
                .expect("focus sidebar");
            settle(&mut backend);

            let _ = backend.send_key(modified_key(
                KeyCode::Left,
                KeyMods {
                    ctrl: true,
                    shift: true,
                    ..KeyMods::NONE
                },
            ));
            settle(&mut backend);
            assert_eq!(
                backend.state().sidebar.panels[0].tabs,
                vec![SidebarTabId::new("agents"), SidebarTabId::new("panes")]
            );

            let _ = backend.send_key(modified_key(
                KeyCode::Right,
                KeyMods {
                    ctrl: true,
                    shift: true,
                    ..KeyMods::NONE
                },
            ));
            settle(&mut backend);
            assert_eq!(
                backend.state().sidebar.panels[0].tabs,
                vec![SidebarTabId::new("panes"), SidebarTabId::new("agents")]
            );
        })
        .expect("spawn tab reorder thread")
        .join()
        .expect("tab reorder completes");
}

#[test]
fn s_toggles_sidebar_split_while_focused() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_panes();
            backend
                .dispatch(Msg::RunAction(Action::FocusSidebar))
                .expect("focus sidebar");
            settle(&mut backend);

            // The default sidebar is already split, so the first press collapses it.
            assert!(backend.state().config.sidebar.split);
            assert_eq!(backend.state().sidebar.panels.len(), 2);

            let _ = backend.send_key(key(KeyCode::Char('s')));
            settle(&mut backend);
            assert!(!backend.state().config.sidebar.split);
            assert_eq!(backend.state().sidebar.panels.len(), 1);

            let _ = backend.send_key(key(KeyCode::Char('s')));
            settle(&mut backend);
            assert!(backend.state().config.sidebar.split);
            assert_eq!(backend.state().sidebar.panels.len(), 2);
        })
        .expect("spawn focused split thread")
        .join()
        .expect("focused split completes");
}

#[test]
fn ctrl_vertical_navigation_moves_keyboard_focus_between_sidebar_panels() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_panes();
            {
                let state = backend.state_mut();
                state.sidebar.panels = vec![
                    rozi::state::SidebarPanelState {
                        tabs: vec![SidebarTabId::new("panes")],
                        active_tab: Some(SidebarTabId::new("panes")),
                        ..Default::default()
                    },
                    rozi::state::SidebarPanelState {
                        tabs: vec![SidebarTabId::new("agents")],
                        active_tab: Some(SidebarTabId::new("agents")),
                        ..Default::default()
                    },
                ];
                state.config.sidebar.tabs = vec![SidebarTab::Panes, SidebarTab::Agents];
            }
            backend
                .dispatch(Msg::RunAction(Action::FocusSidebar))
                .expect("focus sidebar");
            settle(&mut backend);

            let _ = backend.send_key(modified_key(
                KeyCode::Down,
                KeyMods {
                    ctrl: true,
                    ..KeyMods::NONE
                },
            ));
            settle(&mut backend);
            assert_eq!(backend.state().sidebar.active_panel, 1);
            assert!(backend.state().sidebar.focused);

            let _ = backend.send_key(modified_key(
                KeyCode::Up,
                KeyMods {
                    ctrl: true,
                    ..KeyMods::NONE
                },
            ));
            settle(&mut backend);
            assert_eq!(backend.state().sidebar.active_panel, 0);
        })
        .expect("spawn panel navigation thread")
        .join()
        .expect("panel navigation completes");
}

#[test]
fn keyboard_tab_cycling_scrolls_the_active_top_tab_into_view() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_panes();
            let launcher = |name: &str, label: &str| SidebarTab::Launcher {
                name: SidebarTabId::new(name),
                label: label.into(),
                entries: vec![SidebarLauncherEntry {
                    label: "Action".into(),
                    action: UserCommandAction::Send("true".into()),
                }],
            };
            backend.state_mut().config.sidebar.tabs = vec![
                SidebarTab::Panes,
                launcher("build", "Build"),
                launcher("sessions-long", "Sessions"),
                launcher("deployment", "Deployment"),
            ];
            backend.state_mut().sidebar.panels[0].tabs = vec![
                SidebarTabId::new("panes"),
                SidebarTabId::new("build"),
                SidebarTabId::new("sessions-long"),
                SidebarTabId::new("deployment"),
            ];
            backend
                .dispatch(Msg::RunAction(Action::FocusSidebar))
                .expect("focus sidebar");
            settle(&mut backend);

            for _ in 0..3 {
                let _ = backend.send_key(key(KeyCode::Tab));
                settle(&mut backend);
            }

            assert_eq!(
                backend.state().sidebar.active_tab(),
                Some(&SidebarTabId::new("deployment"))
            );
            let top = &backend.capture_frame().to_fixed_grid_lines()[0];
            assert!(
                top.chars()
                    .take(32)
                    .collect::<String>()
                    .contains("Deployment"),
                "horizontal tab navigation keeps the active tab visible: {top:?}"
            );
        })
        .expect("spawn tab visibility thread")
        .join()
        .expect("tab visibility completes");
}

#[test]
fn keyboard_row_navigation_scrolls_the_cursor_into_view() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_panes();
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 80,
                h: 10,
            });
            let panes = (1..=12)
                .map(|id| {
                    let mut pane = pane(id);
                    pane.title = format!("pane-{id}");
                    pane
                })
                .collect();
            backend.state_mut().current_mut().workspaces[0].panes = panes;
            backend.state_mut().current_mut().workspaces[1]
                .panes
                .clear();
            backend
                .dispatch(Msg::RunAction(Action::FocusSidebar))
                .expect("focus sidebar");
            settle(&mut backend);

            let _ = backend.send_key(key(KeyCode::End));
            settle(&mut backend);

            assert!(
                backend.capture_frame().to_fixed_grid().contains("pane-12"),
                "vertical row navigation keeps the cursor row visible"
            );
        })
        .expect("spawn row visibility thread")
        .join()
        .expect("row visibility completes");
}

#[test]
fn page_navigation_uses_the_visible_row_count() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_panes();
            backend.state_mut().current_mut().workspaces[0].panes =
                (1..=12).map(pane).collect();
            backend.state_mut().current_mut().workspaces[1].panes.clear();

            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 10,
            });
            backend.render();
            settle(&mut backend);
            let short_page = backend.state().sidebar.panels[0].page_rows;

            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            });
            backend.render();
            settle(&mut backend);
            let tall_page = backend.state().sidebar.panels[0].page_rows;

            assert!(short_page > 0);
            assert!(
                tall_page > short_page,
                "a taller sidebar should page across more selectable rows: {short_page} -> {tall_page}"
            );
        })
        .expect("spawn page navigation thread")
        .join()
        .expect("page navigation completes");
}

/// The active row's accent bar runs the full height of the row. A two-line entry gets `▍` on both
/// its title and its detail line, not a tick beside the first one.
#[test]
fn the_active_row_marker_spans_every_line_of_the_row() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_panes();
            settle(&mut backend);

            let lines = backend.capture_frame().to_fixed_grid_lines();
            let marked: Vec<usize> = lines
                .iter()
                .enumerate()
                .filter(|(_, line)| line.starts_with('▍'))
                .map(|(index, _)| index)
                .collect();
            assert_eq!(
                marked.len(),
                2,
                "the focused pane's two-line row carries the marker on both lines: {marked:?}"
            );
            assert_eq!(
                marked[1],
                marked[0] + 1,
                "the two marked lines are the same row, not two different ones"
            );
        })
        .expect("spawn marker thread")
        .join()
        .expect("marker completes");
}

/// The keyboard cursor highlights the whole row, both lines of it, in the same color the workbar's
/// active workspace tab uses — so the three selection surfaces read as one language.
#[test]
fn the_cursor_highlight_covers_both_lines_of_a_row() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_panes();
            // The cursor uses the same lift as pointer hover, so it reads as "about to act on this"
            // rather than as a second, louder kind of selection.
            let selection = backend.state().theme.surface.element.elevate_by(0.08);

            backend
                .dispatch(Msg::RunAction(Action::FocusSidebar))
                .expect("focus sidebar");
            settle(&mut backend);

            let frame = backend.capture_frame();
            let height = frame.to_fixed_grid_lines().len() as u16;
            // Column 2 is inside the row body, past the marker gutter and its separating cell.
            let highlighted: Vec<u16> = (1..height)
                .filter(|row| frame.cell(2, *row).bg == selection)
                .collect();
            assert_eq!(
                highlighted.len(),
                2,
                "the cursor covers both lines of one row: {highlighted:?}"
            );
            assert_eq!(
                highlighted[1],
                highlighted[0] + 1,
                "the highlighted lines are one row, not two"
            );
        })
        .expect("spawn cursor thread")
        .join()
        .expect("cursor completes");
}

#[test]
fn keyboard_navigation_suppresses_stale_row_hover_until_the_pointer_moves() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_panes();
            backend.state_mut().current_mut().workspaces[1].panes[0].title =
                "pointer target".into();
            settle(&mut backend);
            let highlight = backend.state().theme.surface.element.elevate_by(0.08);
            let target_row = backend
                .capture_frame()
                .to_fixed_grid_lines()
                .iter()
                .position(|line| line.contains("pointer target"))
                .expect("pointer target row") as u16;

            let _ = backend.send_mouse(MouseEvent {
                x: 4,
                y: target_row,
                kind: MouseKind::Moved,
                mods: KeyMods::NONE,
            });
            settle(&mut backend);
            assert_eq!(
                backend.capture_frame().cell(4, target_row).bg,
                highlight,
                "pointer movement highlights the row"
            );

            backend
                .dispatch(Msg::RunAction(Action::FocusSidebar))
                .expect("focus sidebar");
            let _ = backend.send_key(key(KeyCode::Down));
            settle(&mut backend);
            assert_ne!(
                backend.capture_frame().cell(4, target_row).bg,
                highlight,
                "keyboard navigation clears hover left at the old pointer position"
            );

            let _ = backend.send_mouse(MouseEvent {
                x: 5,
                y: target_row,
                kind: MouseKind::Moved,
                mods: KeyMods::NONE,
            });
            settle(&mut backend);
            assert_eq!(
                backend.capture_frame().cell(5, target_row).bg,
                highlight,
                "the next real pointer movement restores row hover"
            );
        })
        .expect("spawn hover modality thread")
        .join()
        .expect("hover modality completes");
}

/// `j`/`k` move the cursor alongside the arrows, matching resize and copy mode — and matching the
/// file tree, whose widget keymap has always included them.
#[test]
fn vim_keys_move_the_cursor() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_panes();
            backend
                .dispatch(Msg::RunAction(Action::FocusSidebar))
                .expect("focus sidebar");
            settle(&mut backend);

            // Down to the third pane, then back up one. `k` has to step over the workspace header
            // that sits between the second and third panes, exactly as the arrows do.
            for _ in 0..2 {
                let _ = backend.send_key(key(KeyCode::Char('j')));
                settle(&mut backend);
            }
            let _ = backend.send_key(key(KeyCode::Char('k')));
            settle(&mut backend);
            let _ = backend.send_key(key(KeyCode::Enter));
            settle(&mut backend);

            assert_eq!(
                backend.state().current().focused_pane,
                Some(2),
                "two `j` then one `k` lands on the second pane, so both keys move the cursor and \
                 both step over headers"
            );
            assert!(
                !backend.state().sidebar.focused,
                "activating a pane row hands the keyboard to that pane — the point of pressing it"
            );
        })
        .expect("spawn vim keys thread")
        .join()
        .expect("vim keys completes");
}

/// Hover-focus must not yank the keyboard out of the sidebar.
///
/// It is ambient — the pointer moves and focus follows, with no intent behind it — and reaching the
/// sidebar with the mouse means crossing panes to get there. Without this guard that transit hands
/// the keyboard back before the click on the tab strip ever arrives.
#[test]
fn hovering_a_pane_does_not_steal_the_keyboard_from_the_sidebar() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_panes();
            assert!(
                backend.state().config.pane.focus_on_hover,
                "hover focus is on by default, which is what makes this reachable"
            );

            backend
                .dispatch(Msg::RunAction(Action::FocusSidebar))
                .expect("focus sidebar");
            settle(&mut backend);

            // The pointer crosses a different pane on its way to the sidebar.
            backend
                .dispatch(Msg::HoverPane(2))
                .expect("hover another pane");
            settle(&mut backend);

            assert_eq!(
                backend.state().current().focused_pane,
                Some(1),
                "the hovered pane does not become focused while the sidebar owns the keyboard"
            );
            assert!(
                backend.state().sidebar.focused,
                "the sidebar keeps the keyboard across the pointer's transit"
            );

            // Leaving the sidebar restores the normal ambient behaviour.
            let _ = backend.send_key(key(KeyCode::Esc));
            settle(&mut backend);
            backend.dispatch(Msg::HoverPane(2)).expect("hover again");
            settle(&mut backend);
            assert_eq!(
                backend.state().current().focused_pane,
                Some(2),
                "with the keyboard back on the panes, hover focus works as configured"
            );
        })
        .expect("spawn hover thread")
        .join()
        .expect("hover completes");
}

/// Hover feedback has to survive on rows that already have a background — the active pane's row and
/// the row under the keyboard cursor. Hover is a transform of what the row painted rather than a
/// style of its own, precisely so those rows still respond to the pointer.
#[test]
fn hover_still_reads_on_active_and_selected_rows() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = backend_with_panes();
            settle(&mut backend);

            // The focused pane's row carries the active tint; find its first line by the marker.
            let lines = backend.capture_frame().to_fixed_grid_lines();
            let active_row = lines
                .iter()
                .position(|line| line.starts_with('▍'))
                .expect("the focused pane's row is on screen") as u16;
            let resting = backend.capture_frame().cell(4, active_row).bg;

            let _ = backend.send_mouse(MouseEvent {
                x: 4,
                y: active_row,
                kind: MouseKind::Moved,
                mods: Default::default(),
            });
            settle(&mut backend);
            let hovered = backend.capture_frame().cell(4, active_row).bg;

            assert_ne!(
                hovered, resting,
                "hovering the active row changes its background rather than being swallowed by it"
            );
        })
        .expect("spawn hover style thread")
        .join()
        .expect("hover style completes");
}

/// The hover lift has to add contrast on light themes too.
///
/// `Color::elevate` lightens a dark surface and dims a light one, but `ColorTransform` offers only
/// the two directions, so the pointer-hover transform picks the direction the same way. Lightening
/// unconditionally would wash a light theme's rows toward white and make hover nearly invisible.
#[test]
fn hover_lifts_away_from_the_surface_on_light_and_dark_themes() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            for dark in [true, false] {
                let mut backend = backend_with_panes();
                {
                    let state = backend.state_mut();
                    let surface = if dark {
                        Color::Rgb(20, 22, 28)
                    } else {
                        Color::Rgb(240, 240, 245)
                    };
                    state.theme.surface.element = surface;
                    state.theme.surface.backdrop = surface;
                }
                settle(&mut backend);

                let lines = backend.capture_frame().to_fixed_grid_lines();
                let row = lines
                    .iter()
                    .position(|line| line.starts_with('▍'))
                    .expect("the focused pane's row is on screen") as u16;
                let resting = backend.capture_frame().cell(4, row).bg;

                let _ = backend.send_mouse(MouseEvent {
                    x: 4,
                    y: row,
                    kind: MouseKind::Moved,
                    mods: Default::default(),
                });
                settle(&mut backend);
                let hovered = backend.capture_frame().cell(4, row).bg;

                let luminance = |color: Color| match color {
                    Color::Rgb(r, g, b) => {
                        0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)
                    }
                    _ => panic!("expected an rgb cell background, got {color:?}"),
                };
                let (resting, hovered) = (luminance(resting), luminance(hovered));
                if dark {
                    assert!(
                        hovered > resting,
                        "a dark theme lifts toward light: {resting} -> {hovered}"
                    );
                } else {
                    assert!(
                        hovered < resting,
                        "a light theme lifts toward dark: {resting} -> {hovered}"
                    );
                }
            }
        })
        .expect("spawn theme hover thread")
        .join()
        .expect("theme hover completes");
}

/// The invariant `sidebar_backend` exists to hold: a sidebar action persists `[sidebar]`, and a
/// test process must never be able to write that into the developer's live config, which a running
/// hyprmux would live-reload.
#[test]
fn sidebar_preferences_persist_inside_the_test_scratch_root() {
    let root = rozi::test_support::isolate_user_dirs();
    let path = rozi::config::config_path();
    assert!(
        path.starts_with(root),
        "config writes escaped the scratch root: {}",
        path.display()
    );
}
