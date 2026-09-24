//! The `spans` capture: a [`CapturedFrame`] as styled runs, compact enough to read whole.

use base64::Engine as _;
use tui_lipan::prelude::*;
use tui_lipan::{CapturedFrame, CapturedImage, CellRun, CursorShape, UnderlineStyle};

use crate::control::{
    AnsiColorName, ControlResponse, SPAN_FRAME_FORMAT, SPAN_FRAME_VERSION, SpanColor, SpanCursor,
    SpanCursorShape, SpanFrame, SpanImage, SpanPalette, SpanRun, SpanUnderline,
};

const ANSI_NAMES: [AnsiColorName; 16] = [
    AnsiColorName::Black,
    AnsiColorName::Red,
    AnsiColorName::Green,
    AnsiColorName::Yellow,
    AnsiColorName::Blue,
    AnsiColorName::Magenta,
    AnsiColorName::Cyan,
    AnsiColorName::White,
    AnsiColorName::BrightBlack,
    AnsiColorName::BrightRed,
    AnsiColorName::BrightGreen,
    AnsiColorName::BrightYellow,
    AnsiColorName::BrightBlue,
    AnsiColorName::BrightMagenta,
    AnsiColorName::BrightCyan,
    AnsiColorName::BrightWhite,
];

/// `frame` as a [`SpanFrame`], its palette resolved the way a PNG of it is drawn.
///
/// Colors stay as the frame holds them, so a pane's `red` is still `red`; the palette says what
/// that looked like. `image_pixels` must already be checked against the render.
pub(crate) fn span_frame(
    frame: &CapturedFrame,
    palette: TerminalColorPalette,
    image_pixels: bool,
) -> std::result::Result<SpanFrame, ControlResponse> {
    let images = frame
        .images
        .iter()
        .map(|image| span_image(image, image_pixels))
        .collect::<std::result::Result<_, _>>()?;
    Ok(SpanFrame {
        format: SPAN_FRAME_FORMAT.to_string(),
        version: SPAN_FRAME_VERSION,
        width: frame.width,
        height: frame.height,
        palette: span_palette(&palette),
        cursor: frame.cursor.as_ref().map(|cursor| SpanCursor {
            x: cursor.x,
            y: cursor.y,
            visible: cursor.visible,
            shape: match cursor.shape {
                CursorShape::Block => SpanCursorShape::Block,
                CursorShape::HollowBlock => SpanCursorShape::HollowBlock,
                CursorShape::Underline => SpanCursorShape::Underline,
                CursorShape::Bar => SpanCursorShape::Bar,
            },
            blinking: cursor.blinking,
            color: cursor.color.and_then(span_color),
        }),
        rows: frame.runs().iter().map(|runs| span_row(runs)).collect(),
        images,
    })
}

/// One row's runs, with runs that differ only in ways the frame format does not show merged, and
/// the row's trailing default blanks left out.
fn span_row(runs: &[CellRun]) -> Vec<SpanRun> {
    let mut row: Vec<SpanRun> = Vec::with_capacity(runs.len());
    for run in runs {
        let run = span_run(run);
        match row.last_mut() {
            Some(last) if same_style(last, &run) => {
                last.text.push_str(&run.text);
                last.width += run.width;
            }
            _ => row.push(run),
        }
    }
    if let Some(last) = row.last_mut()
        && same_style(last, &SpanRun::default())
    {
        let kept = last.text.trim_end_matches(' ').len();
        let trimmed = (last.text.len() - kept) as u16;
        last.text.truncate(kept);
        last.width -= trimmed;
        if last.width == 0 {
            row.pop();
        }
    }
    row
}

fn same_style(a: &SpanRun, b: &SpanRun) -> bool {
    let style = |run: &SpanRun| SpanRun {
        x: 0,
        width: 0,
        text: String::new(),
        ..run.clone()
    };
    style(a) == style(b)
}

fn span_run(run: &CellRun) -> SpanRun {
    let modifiers = &run.modifiers;
    SpanRun {
        x: run.x,
        width: run.width,
        text: run.text.clone(),
        fg: span_color(run.fg),
        bg: span_color(run.bg),
        underline_color: span_color(run.underline_color),
        bold: modifiers.bold,
        dim: modifiers.dim,
        italic: modifiers.italic,
        underline: modifiers.underline.map(|style| match style {
            UnderlineStyle::Single => SpanUnderline::Single,
            UnderlineStyle::Double => SpanUnderline::Double,
            UnderlineStyle::Curly => SpanUnderline::Curly,
            UnderlineStyle::Dotted => SpanUnderline::Dotted,
            UnderlineStyle::Dashed => SpanUnderline::Dashed,
        }),
        reverse: modifiers.reverse,
        strikethrough: modifiers.strikethrough,
    }
}

/// `color` as the frame format writes it, or `None` for a default: [`Color::Reset`], and the
/// sentinels a finished frame should not hold but would draw as the default if it did.
fn span_color(color: Color) -> Option<SpanColor> {
    if let Some(slot) = ansi_slot(color) {
        return Some(SpanColor::Named(ANSI_NAMES[slot]));
    }
    match color {
        Color::Indexed(index) => Some(SpanColor::Indexed(index)),
        Color::Rgb(r, g, b) => Some(SpanColor::Rgb(hex((r, g, b)))),
        _ => None,
    }
}

/// The ANSI palette slot `color` names, as a named color or as an index below 16.
fn ansi_slot(color: Color) -> Option<usize> {
    Some(match color {
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
        Color::Indexed(index) if index < 16 => usize::from(index),
        _ => return None,
    })
}

fn span_palette(palette: &TerminalColorPalette) -> SpanPalette {
    let resolve = |color: Color| {
        ansi_slot(color)
            .map_or(color, |slot| palette.ansi[slot])
            .to_rgb()
    };
    let (foreground, background) = super::png_default_colors(palette);
    SpanPalette {
        foreground: hex(resolve(foreground).unwrap_or((255, 255, 255))),
        background: hex(resolve(background).unwrap_or((0, 0, 0))),
        ansi: palette
            .ansi
            .iter()
            .map(|&color| hex(color.to_rgb().unwrap_or((0, 0, 0))))
            .collect(),
    }
}

fn hex((r, g, b): (u8, u8, u8)) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn span_image(
    image: &CapturedImage,
    pixels: bool,
) -> std::result::Result<SpanImage, ControlResponse> {
    let area = image.area;
    let visible = (!image.visible.iter().all(|&shown| shown)).then(|| {
        (0..area.h)
            .map(|row| {
                let mut ranges: Vec<(i16, u16)> = Vec::new();
                for col in 0..area.w {
                    let x = area.x.saturating_add(col as i16);
                    let shown = image
                        .visible
                        .get(usize::from(row) * usize::from(area.w) + usize::from(col))
                        .copied()
                        .unwrap_or(false);
                    if !shown {
                        continue;
                    }
                    match ranges.last_mut() {
                        Some((start, width)) if start.saturating_add(*width as i16) == x => {
                            *width += 1;
                        }
                        _ => ranges.push((x, 1)),
                    }
                }
                ranges
            })
            .collect()
    });
    let png_base64 = if pixels {
        let png = image
            .to_png()
            .map_err(|error| ControlResponse::error(format!("image capture failed: {error}")))?;
        Some(base64::engine::general_purpose::STANDARD.encode(png))
    } else {
        None
    };
    Ok(SpanImage {
        x: area.x,
        y: area.y,
        width: area.w,
        height: area.h,
        pixel_width: image.width,
        pixel_height: image.height,
        visible,
        png_base64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{CaptureContent, CaptureRender};

    fn spans(screen: &mut TerminalScreen) -> SpanFrame {
        match super::super::capture_screen(screen, None, CaptureRender::Spans, None, false) {
            Ok(CaptureContent::Spans { frame }) => frame,
            other => panic!("expected a spans capture, got {other:?}"),
        }
    }

    fn run(x: u16, text: &str) -> SpanRun {
        SpanRun {
            x,
            width: text.chars().count() as u16,
            text: text.to_string(),
            ..SpanRun::default()
        }
    }

    #[test]
    fn a_styled_row_keeps_what_differs_from_the_default_and_drops_trailing_blanks() {
        let mut screen = TerminalScreen::new(3, 20, 100);
        screen.process_bytes(
            b"$ \x1b[1;31merror\x1b[0m: \x1b[4:3;58;5;196;38;2;1;2;3mbad\x1b[0m  \r\n\r\n\x1b[44m  \x1b[0m",
        );
        let frame = spans(&mut screen);

        assert_eq!((frame.format.as_str(), frame.version), ("rozi-spans", 1));
        assert_eq!((frame.width, frame.height), (20, 3));
        assert_eq!(
            frame.rows[0],
            vec![
                run(0, "$ "),
                SpanRun {
                    fg: Some(SpanColor::Named(AnsiColorName::Red)),
                    bold: true,
                    ..run(2, "error")
                },
                run(7, ": "),
                SpanRun {
                    fg: Some(SpanColor::Rgb("#010203".into())),
                    underline_color: Some(SpanColor::Indexed(196)),
                    underline: Some(SpanUnderline::Curly),
                    ..run(9, "bad")
                },
            ]
        );
        assert_eq!(frame.rows[1], vec![], "a blank row is empty");
        // A colored blank is not a default one, so it stays.
        assert_eq!(
            frame.rows[2],
            vec![SpanRun {
                bg: Some(SpanColor::Named(AnsiColorName::Blue)),
                ..run(0, "  ")
            }]
        );

        // Only what differs is on the wire.
        assert_eq!(
            serde_json::to_value(&frame.rows[0][1]).unwrap(),
            serde_json::json!({"x": 2, "width": 5, "text": "error", "fg": "red", "bold": true})
        );
        assert_eq!(
            serde_json::to_value(&frame.rows[0][3]).unwrap(),
            serde_json::json!({
                "x": 9, "width": 3, "text": "bad", "fg": "#010203",
                "underline_color": 196, "underline": "curly"
            })
        );
    }

    #[test]
    fn colors_that_name_the_same_slot_share_a_run() {
        let mut screen = TerminalScreen::new(1, 10, 100);
        screen.process_bytes(b"\x1b[31mab\x1b[38;5;1mcd\x1b[0m");
        assert_eq!(
            spans(&mut screen).rows[0],
            vec![SpanRun {
                fg: Some(SpanColor::Named(AnsiColorName::Red)),
                ..run(0, "abcd")
            }]
        );
    }

    #[test]
    fn a_wide_glyph_covers_two_columns_of_its_run() {
        let mut screen = TerminalScreen::new(1, 10, 100);
        screen.process_bytes("a中b\x1b[7m日\x1b[0m.".as_bytes());
        let row = &spans(&mut screen).rows[0];

        assert_eq!(
            row,
            &vec![
                SpanRun {
                    width: 4,
                    ..run(0, "a中b")
                },
                SpanRun {
                    width: 2,
                    reverse: true,
                    ..run(4, "日")
                },
                run(6, "."),
            ]
        );
        let mut column = 0;
        for run in row {
            assert_eq!(run.x, column, "runs tile the row");
            column += run.width;
        }
    }

    #[test]
    fn the_cursor_reports_what_the_program_asked_for() {
        let mut screen = TerminalScreen::new(2, 10, 100);
        screen.process_bytes(b"ab");
        let cursor = spans(&mut screen).cursor.expect("a cursor");
        assert_eq!(
            cursor,
            SpanCursor {
                x: 2,
                y: 0,
                visible: true,
                shape: SpanCursorShape::Block,
                blinking: true,
                color: None,
            }
        );

        // A steady bar in its own color, then hidden.
        screen.process_bytes(b"\x1b[6 q\x1b]12;#ff8800\x07\r\n");
        let cursor = spans(&mut screen).cursor.expect("a cursor");
        assert_eq!((cursor.x, cursor.y), (0, 1));
        assert_eq!(cursor.shape, SpanCursorShape::Bar);
        assert!(!cursor.blinking);
        assert_eq!(cursor.color, Some(SpanColor::Rgb("#ff8800".into())));
        screen.process_bytes(b"\x1b[?25l");
        assert!(!spans(&mut screen).cursor.expect("a cursor").visible);
    }

    #[test]
    fn the_palette_is_what_a_png_draws_names_and_defaults_in() {
        let mut screen = TerminalScreen::new(1, 4, 100);
        let mut ansi = [Color::Rgb(0, 0, 0); 16];
        ansi[1] = Color::Rgb(200, 10, 20);
        ansi[15] = Color::Rgb(250, 250, 250);
        screen.set_palette(TerminalColorPalette::new(
            Color::Rgb(1, 2, 3),
            Color::Rgb(4, 5, 6),
            ansi,
        ));
        let palette = spans(&mut screen).palette;
        assert_eq!(palette.foreground, "#010203");
        assert_eq!(palette.background, "#040506");
        assert_eq!(palette.ansi.len(), 16);
        assert_eq!(palette.ansi[1], "#c80a14");

        // With no default of its own, a PNG draws in the palette's bright white and black.
        let mut unthemed = TerminalColorPalette::new(Color::Reset, Color::Reset, ansi);
        unthemed.foreground = None;
        unthemed.background = None;
        let palette = span_palette(&unthemed);
        assert_eq!(
            (palette.foreground.as_str(), palette.background.as_str()),
            ("#fafafa", "#000000")
        );
    }

    #[test]
    fn an_image_reports_its_area_the_cells_still_showing_it_and_optionally_its_pixels() {
        let blank = tui_lipan::CapturedCell {
            symbol: " ".into(),
            fg: Color::Reset,
            bg: Color::Reset,
            underline_color: Color::Reset,
            modifiers: Default::default(),
        };
        let area = Rect {
            x: 1,
            y: 0,
            w: 4,
            h: 2,
        };
        let rgba: Vec<u8> = [9u8, 8, 7, 255].repeat(3 * 2);
        let mut image = CapturedImage::new(area, 3, 2, rgba.into());
        let mut frame = CapturedFrame {
            viewport: Rect {
                x: 0,
                y: 0,
                w: 6,
                h: 2,
            },
            width: 6,
            height: 2,
            cells: vec![blank; 12],
            cursor: None,
            images: vec![image.clone()],
        };

        let plain = span_frame(&frame, TerminalColorPalette::default(), false).unwrap();
        assert_eq!(
            serde_json::to_value(&plain.images).unwrap(),
            serde_json::json!([{
                "x": 1, "y": 0, "width": 4, "height": 2, "pixel_width": 3, "pixel_height": 2
            }]),
            "a fully visible image lists no cells and no pixels"
        );

        // An overlay over one cell of the top row, and the whole bottom row.
        image.visible = vec![true, true, false, true, false, false, false, false];
        frame.images = vec![image];
        let covered = span_frame(&frame, TerminalColorPalette::default(), true).unwrap();
        let image = &covered.images[0];
        assert_eq!(image.visible, Some(vec![vec![(1, 2), (4, 1)], vec![]]));

        use base64::Engine as _;
        let png = base64::engine::general_purpose::STANDARD
            .decode(image.png_base64.as_deref().expect("pixels were asked for"))
            .unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        let size = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
        assert_eq!(
            (size(16), size(20)),
            (3, 2),
            "the image's own size, not its cells'"
        );
    }

    #[test]
    fn image_pixels_belong_to_spans_only() {
        let mut screen = TerminalScreen::new(1, 4, 100);
        for render in [CaptureRender::Text, CaptureRender::Ansi, CaptureRender::Png] {
            let refused = super::super::capture_screen(&mut screen, None, render, None, true)
                .expect_err("image_pixels outside spans");
            assert_eq!(
                refused.code,
                Some(crate::control::ControlErrorCode::InvalidArgument)
            );
        }
        let refused =
            super::super::capture_screen(&mut screen, None, CaptureRender::Spans, Some(2), false)
                .expect_err("a scaled spans capture");
        assert_eq!(
            refused.code,
            Some(crate::control::ControlErrorCode::InvalidArgument)
        );
    }
}
