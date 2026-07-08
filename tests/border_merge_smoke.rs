//! Mirrors the merged-border pane structure from `view::pane_element` (an unstyled VStack
//! wrapping a title row + bordered Frame, wrapped in Animated, placed overlapping on a
//! Canvas) to pin the tui-lipan behavior hyprmux depends on: adjacent pane borders sharing
//! a seam cell must fuse into junction glyphs (`┬ ├ ┤ ┴`) regardless of draw order.

use tui_lipan::TestBackend;
use tui_lipan::prelude::*;

struct MergeApp {
    titles: bool,
    focused: usize,
    floating: bool,
    border_style: BorderStyle,
}

#[derive(Clone)]
enum Msg {}

impl Component for MergeApp {
    type State = ();
    type Message = Msg;
    type Properties = ();

    fn create_state(&self, _props: &Self::Properties) -> Self::State {}

    fn update(&mut self, _msg: Msg, _ctx: &mut Context<Self>) -> Update {
        Update::none()
    }

    fn view(&self, _ctx: &Context<Self>) -> Element {
        let unfocused = Style::new()
            .fg(Color::rgb(120, 120, 120))
            .bg(Color::rgb(20, 20, 30));
        let focused = Style::new()
            .fg(Color::rgb(0, 200, 255))
            .bg(Color::rgb(20, 20, 30));

        // Mirrors `pane_element`: the wrapper stack stays unstyled so it never fills over a
        // neighbor's border on the shared seam; a left-seam pane insets its title row by one
        // cell with an empty-Text spacer that leaves the seam cell untouched.
        let pane = |style: Style, titles: bool, left_seam: bool| -> Element {
            let mut stack = VStack::new().align(Align::Stretch);
            if titles {
                let row: Element = HStack::new()
                    .style(Style::new().bg(Color::rgb(60, 60, 90)))
                    .width(Length::Flex(1))
                    .height(Length::Px(1))
                    .child(Text::new(" title ").height(Length::Px(1)))
                    .into();
                let row: Element = if left_seam {
                    HStack::new()
                        .height(Length::Px(1))
                        .child(Text::new("").width(Length::Px(1)).height(Length::Px(1)))
                        .child(row)
                        .into()
                } else {
                    row
                };
                stack = stack.child(row);
            }
            let body: Element = Frame::new()
                .border(true)
                .border_style(self.border_style)
                .border_merge_mode(BorderMergeMode::Fuzzy)
                .style(style)
                .focus_style(Style::default())
                .child(
                    Text::new("~ >")
                        .width(Length::Flex(1))
                        .height(Length::Flex(1)),
                )
                .into();
            stack = stack.child(body);
            Animated::new(Element::from(stack)).opacity(1.0).into()
        };

        // Dwindle-ish layout on a 40x14 canvas: pane 1 tall left, panes 2/3 stacked right.
        // Horizontal gap -1 (always overlaps when merging); vertical gap 0 with titles
        // (title sits between the borders), -1 without.
        let (p1, p2, p3) = if self.titles {
            (
                Rect {
                    x: 0,
                    y: 0,
                    w: 20,
                    h: 13,
                },
                Rect {
                    x: 19,
                    y: 0,
                    w: 20,
                    h: 7,
                },
                Rect {
                    x: 19,
                    y: 7,
                    w: 20,
                    h: 6,
                },
            )
        } else {
            (
                Rect {
                    x: 0,
                    y: 0,
                    w: 20,
                    h: 13,
                },
                Rect {
                    x: 19,
                    y: 0,
                    w: 20,
                    h: 7,
                },
                Rect {
                    x: 19,
                    y: 6,
                    w: 20,
                    h: 7,
                },
            )
        };

        let style_for = |index: usize| {
            if self.focused == index {
                focused
            } else {
                unfocused
            }
        };
        let mut children: Vec<(usize, Rect, bool)> =
            vec![(1, p1, false), (2, p2, true), (3, p3, true)];
        // The focused pane draws last so its border color wins on shared seams.
        children.sort_by_key(|(index, _, _)| *index == self.focused);

        let mut canvas = Canvas::new();
        for (index, rect, left_seam) in children {
            canvas = canvas.child_at(rect, pane(style_for(index), self.titles, left_seam));
        }
        if self.floating {
            // Mirrors a floating pane: Double border, Replace merge mode, drawn above the
            // tiled layer. Its border must occlude tiled borders beneath, never fuse.
            let float_body: Element = Frame::new()
                .border(true)
                .border_style(BorderStyle::Double)
                .border_merge_mode(BorderMergeMode::Replace)
                .style(focused)
                .focus_style(Style::default())
                .child(
                    Text::new("~ >")
                        .width(Length::Flex(1))
                        .height(Length::Flex(1)),
                )
                .into();
            let float_stack: Element = VStack::new().align(Align::Stretch).child(float_body).into();
            canvas = canvas.child_at(
                Rect {
                    x: 10,
                    y: 3,
                    w: 18,
                    h: 8,
                },
                Animated::new(float_stack).opacity(1.0),
            );
        }
        canvas.into()
    }
}

fn render(titles: bool, focused: usize) -> Vec<String> {
    render_with(titles, focused, false, BorderStyle::Plain)
}

fn render_with(
    titles: bool,
    focused: usize,
    floating: bool,
    border_style: BorderStyle,
) -> Vec<String> {
    let mut backend = TestBackend::new(MergeApp {
        titles,
        focused,
        floating,
        border_style,
    });
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 40,
        h: 14,
    });
    backend.render();
    let frame = backend.capture_frame();
    frame.to_fixed_grid_lines()
}

fn char_at(lines: &[String], x: usize, y: usize) -> char {
    lines[y].chars().nth(x).unwrap_or(' ')
}

#[test]
fn untitled_junctions_merge() {
    let lines = render(false, 1);
    for line in &lines {
        eprintln!("{line}");
    }
    assert_eq!(char_at(&lines, 19, 0), '┬', "top seam junction");
    assert_eq!(char_at(&lines, 19, 6), '├', "middle-left junction");
    assert_eq!(char_at(&lines, 38, 6), '┤', "middle-right junction");
    assert_eq!(char_at(&lines, 19, 12), '┴', "bottom seam junction");
}

#[test]
fn titled_junctions_merge() {
    let lines = render(true, 1);
    for line in &lines {
        eprintln!("{line}");
    }
    assert_eq!(char_at(&lines, 19, 1), '┬', "top seam junction");
    assert_eq!(
        char_at(&lines, 19, 6),
        '├',
        "pane2 bottom joins pane1 border"
    );
    assert_eq!(char_at(&lines, 19, 8), '├', "pane3 top joins pane1 border");
    assert_eq!(char_at(&lines, 19, 12), '┴', "bottom seam junction");
    assert_eq!(
        char_at(&lines, 19, 7),
        '│',
        "pane1 border shows through pane3's inset title row"
    );
}

#[test]
fn titled_focused_right_pane_keeps_neighbor_border() {
    let lines = render(true, 3);
    for line in &lines {
        eprintln!("{line}");
    }
    // Pane 3 draws last; its title row must not punch a hole in pane 1's right border.
    assert_eq!(char_at(&lines, 19, 7), '│', "seam cell in pane3 title row");
    assert_eq!(char_at(&lines, 19, 8), '├', "pane3 top joins pane1 border");
    assert_eq!(char_at(&lines, 19, 12), '┴', "bottom seam junction");
}

#[test]
fn rounded_junctions_merge_as_plain() {
    let lines = render_with(false, 1, false, BorderStyle::Rounded);
    for line in &lines {
        eprintln!("{line}");
    }
    // Arc corners have no junction glyphs; Fuzzy merges them into plain tees while the
    // unshared outer corners keep their rounding.
    assert_eq!(char_at(&lines, 19, 0), '┬', "top seam junction");
    assert_eq!(char_at(&lines, 19, 6), '├', "middle-left junction");
    assert_eq!(char_at(&lines, 38, 6), '┤', "middle-right junction");
    assert_eq!(char_at(&lines, 19, 12), '┴', "bottom seam junction");
    assert_eq!(char_at(&lines, 0, 0), '╭', "outer top-left stays rounded");
    assert_eq!(char_at(&lines, 38, 0), '╮', "outer top-right stays rounded");
    assert_eq!(
        char_at(&lines, 0, 12),
        '╰',
        "outer bottom-left stays rounded"
    );
    assert_eq!(
        char_at(&lines, 38, 12),
        '╯',
        "outer bottom-right stays rounded"
    );
}

#[test]
fn floating_pane_occludes_instead_of_merging() {
    let lines = render_with(false, 1, true, BorderStyle::Plain);
    for line in &lines {
        eprintln!("{line}");
    }
    // The floating border crosses the tiled seams; the cells must stay pure Double glyphs
    // instead of fusing into junctions like `╫`/`╪`.
    assert_eq!(
        char_at(&lines, 19, 3),
        '═',
        "float top crosses tiled seam column"
    );
    assert_eq!(
        char_at(&lines, 19, 10),
        '═',
        "float bottom crosses tiled seam column"
    );
    assert_eq!(
        char_at(&lines, 27, 6),
        '║',
        "float right crosses tiled seam row"
    );
}
