use hyprmux::config::{SidebarTab, SidebarTabId};
use hyprmux::state::{SidebarCommandOutput, SidebarCommandRow};
use hyprmux::{HyprmuxApp, Msg as HyprmuxMsg};
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

fn scroll(backend: &mut TestBackend<SidebarTabs>, kind: MouseKind) {
    for _ in 0..12 {
        backend
            .send_mouse(mouse(1, kind, KeyMods::SHIFT))
            .expect("horizontal scroll");
    }
}

fn click_label(backend: &mut TestBackend<SidebarTabs>, label: &str) {
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
            let mut backend = TestBackend::new(HyprmuxApp::default());
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
                state.sidebar.active_tab = Some(SidebarTab::Panes.id());
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
                .dispatch(HyprmuxMsg::SidebarTabSelected(files))
                .expect("select command tab");

            assert!(backend.capture_frame().to_fixed_grid().contains("file-row"));
        })
        .expect("spawn sidebar command tab test")
        .join()
        .expect("sidebar command tab test completes");
}
