//! Mirrors `view::workbar::workbar_with_caps`: when the whole-workbar caps are on, the row is
//! wrapped in a borderless `Frame` + `ZStack` with a decorative cap overlay on top. That overlay
//! must not swallow pointer events meant for the interactive bar beneath it (workspace tabs, mode
//! chips), so the wrapping `ZStack` has to be `passthrough`. This pins that: a passthrough overlay
//! keeps the tabs hoverable/clickable, while a non-passthrough overlay (the bug) blocks them.

use std::cell::RefCell;
use std::rc::Rc;

use tui_lipan::core::event::{MouseButton, MouseEvent, MouseKind};
use tui_lipan::prelude::*;
use tui_lipan::{Rect, TestBackend};

const PANEL: Color = Color::Rgb(40, 44, 60);
const BACKDROP: Color = Color::Rgb(12, 12, 18);
const ACCENT: Color = Color::Rgb(120, 200, 255);
const LEFT_CAP: &str = "\u{e0b6}";
const RIGHT_CAP: &str = "\u{e0b4}";

struct BarApp {
    passthrough: bool,
    changed: Rc<RefCell<Option<usize>>>,
}

#[derive(Clone)]
enum Msg {
    Switch(usize),
}

impl Component for BarApp {
    type State = ();
    type Message = Msg;
    type Properties = ();

    fn create_state(&self, _props: &Self::Properties) -> Self::State {}

    fn update(&mut self, msg: Msg, _ctx: &mut Context<Self>) -> Update {
        match msg {
            Msg::Switch(idx) => *self.changed.borrow_mut() = Some(idx),
        }
        Update::none()
    }

    fn view(&self, ctx: &Context<Self>) -> Element {
        let tabs = Tabs::new()
            .tabs(vec![Tab::new("1"), Tab::new("2"), Tab::new("3")])
            .active(0)
            .focusable(false)
            .width(Length::Flex(1))
            .height(Length::Px(1))
            .divider(' ')
            .style(Style::new().fg(Color::White).bg(PANEL))
            .active_style(Style::new().fg(BACKDROP).bg(ACCENT).bold())
            .tab_hover_style(Style::new().bg(Color::Rgb(60, 64, 80)))
            .on_change(
                ctx.link()
                    .callback(|event: TabsEvent| Msg::Switch(event.index)),
            );
        let row = HStack::new()
            .gap(1)
            .width(Length::Flex(1))
            .height(Length::Px(1))
            .style(Style::new().bg(PANEL))
            .child(
                Text::new(" rozi ")
                    .style(Style::new().fg(BACKDROP).bg(ACCENT))
                    .height(Length::Px(1)),
            )
            .child(tabs);

        let cap = |glyph: &'static str, color: Color| {
            Text::new(glyph)
                .style(Style::new().fg(color).bg(BACKDROP))
                .width(Length::Px(1))
                .height(Length::Px(1))
        };
        let overlay = HStack::new()
            .width(Length::Flex(1))
            .height(Length::Px(1))
            .child(cap(LEFT_CAP, ACCENT))
            .child(Spacer::new())
            .child(cap(RIGHT_CAP, PANEL));
        let workbar = Frame::new()
            .border(false)
            .padding(0)
            .width(Length::Flex(1))
            .height(Length::Px(1))
            .child(
                ZStack::new()
                    .passthrough(self.passthrough)
                    .child(row)
                    .child(overlay),
            );

        VStack::new()
            .style(Style::new().bg(PANEL))
            .child(workbar)
            .child(Canvas::new().height(Length::Flex(1)))
            .into()
    }
}

fn mouse(x: u16, y: u16, kind: MouseKind) -> MouseEvent {
    MouseEvent {
        x,
        y,
        kind,
        mods: Default::default(),
    }
}

fn run(passthrough: bool) -> (bool, Option<usize>) {
    let changed = Rc::new(RefCell::new(None));
    let mut backend = TestBackend::new(BarApp {
        passthrough,
        changed: changed.clone(),
    });
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 30,
        h: 5,
    });
    backend.render();

    // " rozi " is cols 0..=8, gap at col 9, tabs " 1 "/" 2 "/" 3 " from col 10, so tab "2" is
    // near col 14.
    let tab2_x = 14;
    backend
        .send_mouse(mouse(tab2_x, 0, MouseKind::Moved))
        .expect("move");
    let hovered = backend.hovered().is_some();
    backend
        .send_mouse(mouse(tab2_x, 0, MouseKind::Down(MouseButton::Left)))
        .expect("down");
    backend
        .send_mouse(mouse(tab2_x, 0, MouseKind::Up(MouseButton::Left)))
        .expect("up");
    let clicked = *changed.borrow();
    (hovered, clicked)
}

#[test]
fn passthrough_cap_overlay_keeps_tabs_hoverable_and_clickable() {
    let (hovered, clicked) = run(true);
    assert!(hovered, "tab strip should hover through the cap overlay");
    assert_eq!(
        clicked,
        Some(1),
        "clicking the second tab should fire on_change through the cap overlay"
    );
}

#[test]
fn non_passthrough_cap_overlay_blocks_tabs() {
    // Documents the bug this guards against: a plain (blocking) overlay eats the events, so the
    // tabs neither hover nor fire.
    let (hovered, clicked) = run(false);
    assert!(!hovered, "blocking overlay swallows the hover");
    assert_eq!(clicked, None, "blocking overlay swallows the click");
}
