//! Mirrors the capped-workbar structure from `view::workbar::workbar_with_caps` (a full-width
//! panel-colored row with a borderless `Frame` + `ZStack` overlay that paints an end cap over each
//! outer cell) to pin the tui-lipan behavior rozi depends on: the caps must land on the bar's
//! two edge cells, without shifting the bar content, and each cap must take the color of the
//! badge/segment sitting on that edge (drawn over the backdrop).

use tui_lipan::TestBackend;
use tui_lipan::prelude::*;

const PANEL: Color = Color::Rgb(40, 44, 60);
const BACKDROP: Color = Color::Rgb(12, 12, 18);
// Accent used by the leading `rozi`-style badge (mirrors `theme.border_active`).
const ACCENT: Color = Color::Rgb(120, 200, 255);
// Trailing powerline chip colors (mirror `status.warning` and `border_active`).
const WARNING: Color = Color::Rgb(230, 180, 80);
const SESSION: Color = Color::Rgb(120, 200, 255);
// Round (pill) caps, matching `CapStyle::Round`.
const LEFT_CAP: &str = "\u{e0b6}";
const RIGHT_CAP: &str = "\u{e0b4}";

struct BarApp {
    caps: bool,
}

#[derive(Clone)]
enum Msg {}

impl Component for BarApp {
    type State = ();
    type Message = Msg;
    type Properties = ();

    fn create_state(&self, _props: &Self::Properties) -> Self::State {}

    fn update(&mut self, _msg: Msg, _ctx: &mut Context<Self>) -> Update {
        Update::none()
    }

    fn view(&self, _ctx: &Context<Self>) -> Element {
        // A leading accent badge (`rozi`) then a plain panel-colored segment, so the left edge
        // is a colored badge and the right edge is a plain bar cell.
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
            .child(
                Text::new(" ok ")
                    .style(Style::new().fg(Color::White).bg(PANEL))
                    .height(Length::Px(1)),
            );
        let bar: Element = if self.caps {
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
            Frame::new()
                .border(false)
                .padding(0)
                .width(Length::Flex(1))
                .height(Length::Px(1))
                .child(ZStack::new().child(row).child(overlay))
                .into()
        } else {
            row.into()
        };
        VStack::new()
            .align(Align::Stretch)
            .style(Style::new().bg(BACKDROP))
            .child(bar)
            .into()
    }
}

fn backend(caps: bool) -> TestBackend<BarApp> {
    let mut backend = TestBackend::new(BarApp { caps });
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 20,
        h: 1,
    });
    backend.render();
    backend
}

#[test]
fn padded_workbar_is_flush() {
    let backend = backend(false);
    let lines = backend.capture_frame().to_fixed_grid_lines();
    eprintln!("{}", lines[0]);
    // Flush bar: the badge's leading space padding sits in the first cell, text right after.
    assert_eq!(lines[0].chars().next().unwrap_or(' '), ' ', "leading pad");
    assert_eq!(lines[0].chars().nth(1).unwrap_or(' '), 'r', "text at col 1");
}

#[test]
fn capped_workbar_paints_edge_caps_without_shifting_content() {
    let backend = backend(true);
    let frame = backend.capture_frame();
    eprintln!("{}", frame.to_fixed_grid_lines()[0]);
    // The caps overwrite the bar's outer padding cells; content stays put (text still at col 1).
    assert_eq!(
        frame.cell(0, 0).symbol,
        LEFT_CAP,
        "left cap glyph on first cell"
    );
    assert_eq!(
        frame.cell(1, 0).symbol,
        "r",
        "content unshifted under the left cap"
    );
    assert_eq!(
        frame.cell(19, 0).symbol,
        RIGHT_CAP,
        "right cap glyph on last cell"
    );
}

/// Mirrors `view::workbar::trailing_cluster` when badge caps are on: mode chips and the session
/// badge chain with no gap, and each chip's left cap is drawn over its left neighbor's color so the
/// run reads as a powerline (`panel -> WARNING -> SESSION`).
struct ChainApp;

impl Component for ChainApp {
    type State = ();
    type Message = Msg;
    type Properties = ();

    fn create_state(&self, _props: &Self::Properties) -> Self::State {}

    fn update(&mut self, _msg: Msg, _ctx: &mut Context<Self>) -> Update {
        Update::none()
    }

    fn view(&self, _ctx: &Context<Self>) -> Element {
        // A capped left-cap badge: `[cap over neighbor][body]`, mirroring `workbar_badge`.
        let badge = |label: &'static str, bg: Color, neighbor: Color| -> Element {
            HStack::new()
                .width(Length::Auto)
                .height(Length::Px(1))
                .child(
                    Text::new(LEFT_CAP)
                        .style(Style::new().fg(bg).bg(neighbor))
                        .width(Length::Px(1))
                        .height(Length::Px(1)),
                )
                .child(
                    Text::new(label)
                        .style(Style::new().fg(BACKDROP).bg(bg).bold())
                        .height(Length::Px(1)),
                )
                .into()
        };
        // gap(0): the chips interlock instead of being separated by a blank cell.
        let cluster = HStack::new()
            .width(Length::Auto)
            .height(Length::Px(1))
            .gap(0)
            .child(badge("PREFIX ", WARNING, PANEL))
            .child(badge("session ", SESSION, WARNING));
        VStack::new()
            .align(Align::Stretch)
            .style(Style::new().bg(BACKDROP))
            .child(
                HStack::new()
                    .width(Length::Flex(1))
                    .height(Length::Px(1))
                    .style(Style::new().bg(PANEL))
                    .child(Spacer::new())
                    .child(cluster),
            )
            .into()
    }
}

/// Mirrors `view::workbar::trailing_cluster` when badge caps are on and powerline is off: each
/// chip is a standalone pill with caps on both sides, separated by a 1-cell gap, and every cap
/// sits over the panel bar.
struct StandaloneApp;

impl Component for StandaloneApp {
    type State = ();
    type Message = Msg;
    type Properties = ();

    fn create_state(&self, _props: &Self::Properties) -> Self::State {}

    fn update(&mut self, _msg: Msg, _ctx: &mut Context<Self>) -> Update {
        Update::none()
    }

    fn view(&self, _ctx: &Context<Self>) -> Element {
        let badge = |label: &'static str, bg: Color| -> Element {
            HStack::new()
                .width(Length::Auto)
                .height(Length::Px(1))
                .child(
                    Text::new(LEFT_CAP)
                        .style(Style::new().fg(bg).bg(PANEL))
                        .width(Length::Px(1))
                        .height(Length::Px(1)),
                )
                .child(
                    Text::new(label)
                        .style(Style::new().fg(BACKDROP).bg(bg).bold())
                        .height(Length::Px(1)),
                )
                .child(
                    Text::new(RIGHT_CAP)
                        .style(Style::new().fg(bg).bg(PANEL))
                        .width(Length::Px(1))
                        .height(Length::Px(1)),
                )
                .into()
        };
        let cluster = HStack::new()
            .width(Length::Auto)
            .height(Length::Px(1))
            .gap(1)
            .child(badge("PREFIX", WARNING))
            .child(badge("session", SESSION));
        VStack::new()
            .align(Align::Stretch)
            .style(Style::new().bg(BACKDROP))
            .child(
                HStack::new()
                    .width(Length::Flex(1))
                    .height(Length::Px(1))
                    .style(Style::new().bg(PANEL))
                    .child(Spacer::new())
                    .child(cluster),
            )
            .into()
    }
}

#[test]
fn trailing_badges_are_standalone_pills_when_powerline_is_off() {
    let mut backend = TestBackend::new(StandaloneApp);
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 24,
        h: 1,
    });
    backend.render();
    let frame = backend.capture_frame();
    let line = frame.to_fixed_grid_lines()[0].clone();
    eprintln!("{line}");
    let first_left = line.find(LEFT_CAP).expect("prefix chip has a left cap");
    let first_right = line[first_left + LEFT_CAP.len()..]
        .find(RIGHT_CAP)
        .expect("prefix chip has a right cap")
        + first_left
        + LEFT_CAP.len();
    let first_right_col = line[..first_right].chars().count();
    let right_cap = frame.cell(first_right_col as u16, 0);
    assert_eq!(
        right_cap.symbol, RIGHT_CAP,
        "prefix chip rounds off on the right"
    );
    assert_eq!(right_cap.fg, WARNING, "right cap uses the badge color");
    assert_eq!(right_cap.bg, PANEL, "right cap sits over the panel bar");
}

#[test]
fn trailing_badges_chain_into_a_powerline_when_capped() {
    let mut backend = TestBackend::new(ChainApp);
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 20,
        h: 1,
    });
    backend.render();
    let frame = backend.capture_frame();
    let line = frame.to_fixed_grid_lines()[0].clone();
    eprintln!("{line}");
    // The two chips abut with no blank gap: the session cap sits immediately after "PREFIX ".
    let sep = line.find(LEFT_CAP).unwrap();
    let sep2 = line[sep + LEFT_CAP.len()..].find(LEFT_CAP).unwrap() + sep + LEFT_CAP.len();
    let sep2_col = line[..sep2].chars().count();
    let cap = frame.cell(sep2_col as u16, 0);
    // The session chip's cap blends from its left neighbor (the PREFIX warning), not the panel.
    assert_eq!(cap.symbol, LEFT_CAP, "session chip keeps its powerline cap");
    assert_eq!(cap.fg, SESSION, "cap is drawn in the session color");
    assert_eq!(cap.bg, WARNING, "cap blends over the PREFIX neighbor color");
}

#[test]
fn edge_caps_take_the_edge_badge_color_over_the_backdrop() {
    let backend = backend(true);
    let frame = backend.capture_frame();
    // The leading badge is accent-colored, so its cap rounds off in the accent over the backdrop.
    let left = frame.cell(0, 0);
    assert_eq!(left.fg, ACCENT, "left cap uses the leading badge color");
    assert_eq!(left.bg, BACKDROP, "left cap sits over the backdrop");
    // The trailing edge is a plain bar cell, so its cap stays panel-colored.
    let right = frame.cell(19, 0);
    assert_eq!(right.fg, PANEL, "right cap stays the panel color");
    assert_eq!(right.bg, BACKDROP, "right cap sits over the backdrop");
}
