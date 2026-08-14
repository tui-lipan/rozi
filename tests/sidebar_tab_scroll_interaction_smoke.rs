use rozi::config::{SidebarTab, SidebarTabId};
use rozi::state::{SidebarCommandOutput, SidebarCommandRow};
use rozi::{AppRoot, Msg};
use tui_lipan::core::event::{KeyMods, MouseButton, MouseEvent, MouseKind};
use tui_lipan::prelude::*;
use tui_lipan::{Rect, TestBackend};

struct SidebarTabs;

#[derive(Clone)]
enum SidebarMsg {
    Select(usize),
}

impl Component for SidebarTabs {
    type State = usize;
    type Message = SidebarMsg;
    type Properties = ();

    fn create_state(&self, _props: &Self::Properties) -> Self::State {
        0
    }

    fn update(&mut self, msg: SidebarMsg, ctx: &mut Context<Self>) -> Update {
        match msg {
            SidebarMsg::Select(index) => ctx.state = index,
        }
        Update::none()
    }

    fn view(&self, ctx: &Context<Self>) -> Element {
        let tabs = Tabs::new()
            .tabs(
                ["Agents", "Panes", "Sessions", "Deploy", "Files"]
                    .into_iter()
                    .map(Tab::new)
                    .collect::<Vec<_>>(),
            )
            .active(ctx.state)
            .focusable(false)
            .width(Length::Auto)
            .height(Length::Px(1))
            .divider(' ')
            .caps(Some(('', '')))
            .on_change(
                ctx.link()
                    .callback(|event: TabsEvent| SidebarMsg::Select(event.index)),
            );

        ScrollView::new()
            .axis(ScrollAxis::Horizontal)
            .h_scrollbar(false)
            .width(Length::Px(32))
            .height(Length::Px(1))
            .child(tabs)
            .into()
    }
}

fn mouse(x: u16, kind: MouseKind, mods: KeyMods) -> MouseEvent {
    MouseEvent {
        x,
        y: 0,
        kind,
        mods,
    }
}

fn scroll<C: Component>(backend: &mut TestBackend<C>, kind: MouseKind) {
    for _ in 0..12 {
        backend
            .send_mouse(mouse(1, kind, KeyMods::SHIFT))
            .expect("horizontal scroll");
    }
}

fn click_label<C: Component>(backend: &mut TestBackend<C>, label: &str) {
    let line = backend.capture_frame().to_fixed_grid_lines().remove(0);
    let x = line.find(label).expect("label should be visible") as u16;
    backend
        .send_mouse(mouse(x, MouseKind::Down(MouseButton::Left), KeyMods::NONE))
        .expect("mouse down");
    backend
        .send_mouse(mouse(x, MouseKind::Up(MouseButton::Left), KeyMods::NONE))
        .expect("mouse up");
}

#[test]
fn overflowed_sidebar_tab_can_be_selected_twice() {
    let mut backend = TestBackend::new(SidebarTabs);
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 32,
        h: 3,
    });
    backend.render();

    scroll(&mut backend, MouseKind::ScrollDown);
    click_label(&mut backend, "Files");
    assert_eq!(*backend.state(), 4);

    scroll(&mut backend, MouseKind::ScrollUp);
    click_label(&mut backend, "Panes");
    assert_eq!(*backend.state(), 1);

    scroll(&mut backend, MouseKind::ScrollDown);
    click_label(&mut backend, "Files");
    assert_eq!(*backend.state(), 4);
}

#[test]
fn revisiting_cached_command_tab_renders_without_an_unrelated_update() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 20,
            });
            let files = SidebarTabId::new("files");
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                // Revealing the sidebar after the first frame is a real toggle, so it runs the
                // real slide; these assertions are about the settled column, not a frame
                // part-way through it.
                state.config.animations.sidebar = false;
                state.config.sidebar.tabs = vec![
                    SidebarTab::Panes,
                    SidebarTab::Command {
                        name: files.clone(),
                        label: "Files".to_string(),
                        command: "printf file-row".to_string(),
                        interval_secs: 10,
                        on_click: None,
                    },
                ];
                // One panel holding exactly these two tabs, so nothing the default split would
                // put in a second panel can render its own rows into the assertions below.
                state.config.sidebar.split = false;
                state.sidebar.apply_configured_panels(&state.config.sidebar);
                state.sidebar.panels[0].active_tab = Some(SidebarTab::Panes.id());
                state.sidebar.command_output.insert(
                    files.clone(),
                    SidebarCommandOutput {
                        epoch: 1,
                        rows: vec![SidebarCommandRow {
                            raw: "file-row".to_string(),
                            display: "file-row".to_string(),
                            error: false,
                        }],
                    },
                );
                // Selecting the tab increments the epoch; keep polling parked so the assertion
                // covers the selection update rather than a command result racing it.
                state.sidebar.command_in_flight.insert(files.clone(), 1);
            }
            backend.render();
            assert!(!backend.capture_frame().to_fixed_grid().contains("file-row"));

            backend
                .dispatch(Msg::SidebarTabSelected { panel: 0, index: 1 })
                .expect("select command tab");

            assert!(backend.capture_frame().to_fixed_grid().contains("file-row"));
        })
        .expect("spawn sidebar command tab test")
        .join()
        .expect("sidebar command tab test completes");
}

#[test]
fn app_sidebar_tabs_keep_native_selection_hover_click_and_wheel_behavior() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let launcher = |name: &str, label: &str| SidebarTab::Launcher {
                name: SidebarTabId::new(name),
                label: label.into(),
                entries: Vec::new(),
            };
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 20,
            });
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.config.animations.sidebar = false;
                state.config.sidebar.tabs = vec![
                    SidebarTab::Panes,
                    launcher("build", "Build"),
                    launcher("sessions-long", "Sessions"),
                    launcher("deployment", "Deployment"),
                ];
                state.sidebar.panels[0].tabs = state
                    .config
                    .sidebar
                    .tabs
                    .iter()
                    .map(SidebarTab::id)
                    .collect();
                state.sidebar.panels[0].active_tab = Some(SidebarTab::Panes.id());
            }
            backend.render();

            let theme = backend.state().theme.clone();
            let initial = backend.capture_frame();
            assert_eq!(initial.cell(2, 0).bg, theme.border_active);
            assert_eq!(initial.cell(9, 0).bg, theme.surface.element);

            backend
                .send_mouse(mouse(9, MouseKind::Moved, KeyMods::NONE))
                .expect("hover Build tab");
            assert!(backend.hovered().is_some());
            assert_eq!(
                backend.capture_frame().cell(9, 0).bg,
                theme.surface.element.elevate_by(0.08)
            );

            click_label(&mut backend, "Build");
            assert_eq!(
                backend.state().sidebar.active_tab(),
                Some(&SidebarTabId::new("build"))
            );

            scroll(&mut backend, MouseKind::ScrollDown);
            click_label(&mut backend, "Deployment");
            assert_eq!(
                backend.state().sidebar.active_tab(),
                Some(&SidebarTabId::new("deployment"))
            );
        })
        .expect("spawn sidebar tab interaction thread")
        .join()
        .expect("sidebar tab interaction completes");
}

#[test]
fn destination_selection_resolves_after_transfer_into_an_empty_panel() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            {
                let state = backend.state_mut();
                state.config.sidebar.tabs = vec![SidebarTab::Agents];
                state.sidebar.panels = vec![
                    rozi::state::SidebarPanelState {
                        tabs: vec![SidebarTabId::new("agents")],
                        active_tab: Some(SidebarTabId::new("agents")),
                        ..Default::default()
                    },
                    rozi::state::SidebarPanelState::default(),
                ];
                assert!(state.sidebar.transfer_tab(0, 1, 0, 0));
            }

            backend
                .dispatch(Msg::SidebarTabSelected { panel: 1, index: 0 })
                .expect("destination change follows transfer");
            assert_eq!(
                backend.state().sidebar.active_tab(),
                Some(&SidebarTabId::new("agents"))
            );
            assert_eq!(backend.state().sidebar.active_panel, 1);
        })
        .expect("spawn transfer selection thread")
        .join()
        .expect("transfer selection completes");
}
