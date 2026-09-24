//! A recorded `rozi-spans` frame back as a [`CapturedFrame`], for drawing it as a PNG or replaying
//! it in a terminal.

use std::collections::HashMap;
use std::sync::Arc;

use tui_lipan::prelude::*;
use tui_lipan::{
    CapturedCell, CapturedFrame, CapturedImage, CellModifiers, CursorShape, CursorState,
    UnderlineStyle,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::control::{
    AnsiColorName, SpanColor, SpanCursorShape, SpanFrame, SpanPalette, SpanRun, SpanUnderline,
};

/// An image's pixels, decoded once from its `image` event.
#[derive(Clone, Debug)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

impl DecodedImage {
    /// Decode an 8-bit RGB or RGBA PNG.
    pub fn from_png(bytes: &[u8]) -> std::result::Result<Self, String> {
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder
            .read_info()
            .map_err(|error| format!("invalid image: {error}"))?;
        let size = reader
            .output_buffer_size()
            .ok_or_else(|| "image too large".to_string())?;
        let mut buffer = vec![0; size];
        let info = reader
            .next_frame(&mut buffer)
            .map_err(|error| format!("invalid image: {error}"))?;
        buffer.truncate(info.buffer_size());
        if info.bit_depth != png::BitDepth::Eight {
            return Err("image is not 8-bit".to_string());
        }
        let rgba = match info.color_type {
            png::ColorType::Rgba => buffer,
            png::ColorType::Rgb => buffer
                .chunks_exact(3)
                .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
                .collect(),
            other => return Err(format!("unsupported image color type {other:?}")),
        };
        Ok(Self {
            width: info.width,
            height: info.height,
            rgba: rgba.into(),
        })
    }
}

/// `frame` as the cells, cursor, and images it describes. An image whose pixels `images` does not
/// hold is left out; its half-block stand-in is still in the cells.
pub fn captured_frame(frame: &SpanFrame, images: &HashMap<String, DecodedImage>) -> CapturedFrame {
    let (width, height) = (frame.width, frame.height);
    let blank = CapturedCell {
        symbol: " ".to_string(),
        fg: Color::Reset,
        bg: Color::Reset,
        underline_color: Color::Reset,
        modifiers: CellModifiers::default(),
    };
    let mut cells = vec![blank; usize::from(width) * usize::from(height)];
    for (y, row) in frame.rows.iter().enumerate().take(usize::from(height)) {
        let line = &mut cells[y * usize::from(width)..(y + 1) * usize::from(width)];
        for run in row {
            place_run(line, run);
        }
    }
    let cursor = frame.cursor.as_ref().map(|cursor| {
        CursorState::new(cursor.x, cursor.y)
            .visible(cursor.visible)
            .shape(match cursor.shape {
                SpanCursorShape::Block => CursorShape::Block,
                SpanCursorShape::HollowBlock => CursorShape::HollowBlock,
                SpanCursorShape::Underline => CursorShape::Underline,
                SpanCursorShape::Bar => CursorShape::Bar,
            })
            .blinking(cursor.blinking)
            .color(cursor.color.as_ref().map(color))
    });
    let images = frame
        .images
        .iter()
        .filter_map(|image| {
            let pixels = images.get(image.id.as_deref()?)?;
            let area = Rect {
                x: image.x,
                y: image.y,
                w: image.width,
                h: image.height,
            };
            let mut captured =
                CapturedImage::new(area, pixels.width, pixels.height, pixels.rgba.clone());
            if let Some(visible) = &image.visible {
                captured.visible.fill(false);
                for (row, ranges) in visible.iter().enumerate().take(usize::from(image.height)) {
                    for &(x, w) in ranges {
                        for col in 0..w {
                            let col = i32::from(x) + i32::from(col) - i32::from(image.x);
                            if (0..i32::from(image.width)).contains(&col) {
                                captured.visible[row * usize::from(image.width) + col as usize] =
                                    true;
                            }
                        }
                    }
                }
            }
            Some(captured)
        })
        .collect();
    CapturedFrame {
        viewport: Rect {
            x: 0,
            y: 0,
            w: width,
            h: height,
        },
        width,
        height,
        cells,
        cursor,
        images,
    }
}

/// Lay one run's graphemes into its columns. A wide glyph takes two cells, the second left empty
/// the way a terminal capture leaves it.
fn place_run(line: &mut [CapturedCell], run: &SpanRun) {
    let style = CapturedCell {
        symbol: String::new(),
        fg: run.fg.as_ref().map_or(Color::Reset, color),
        bg: run.bg.as_ref().map_or(Color::Reset, color),
        underline_color: run.underline_color.as_ref().map_or(Color::Reset, color),
        modifiers: CellModifiers {
            bold: run.bold,
            dim: run.dim,
            italic: run.italic,
            underline: run.underline.map(|underline| match underline {
                SpanUnderline::Single => UnderlineStyle::Single,
                SpanUnderline::Double => UnderlineStyle::Double,
                SpanUnderline::Curly => UnderlineStyle::Curly,
                SpanUnderline::Dotted => UnderlineStyle::Dotted,
                SpanUnderline::Dashed => UnderlineStyle::Dashed,
            }),
            reverse: run.reverse,
            strikethrough: run.strikethrough,
        },
    };
    let end = usize::from(run.x.saturating_add(run.width)).min(line.len());
    let mut x = usize::from(run.x);
    for grapheme in run.text.graphemes(true) {
        if x >= end {
            break;
        }
        let width = UnicodeWidthStr::width(grapheme).max(1);
        line[x] = CapturedCell {
            symbol: grapheme.to_string(),
            ..style.clone()
        };
        for covered in line.iter_mut().take(end.min(x + width)).skip(x + 1) {
            *covered = style.clone();
        }
        x += width;
    }
    // Columns a run claims past its text, such as blanks in a colored background.
    for cell in line.iter_mut().take(end).skip(x) {
        *cell = CapturedCell {
            symbol: " ".to_string(),
            ..style.clone()
        };
    }
}

/// A recorded color as the framework's, keeping ANSI names symbolic so the palette resolves them.
pub fn color(color: &SpanColor) -> Color {
    match color {
        SpanColor::Indexed(index) => Color::Indexed(*index),
        SpanColor::Named(name) => match name {
            AnsiColorName::Black => Color::Black,
            AnsiColorName::Red => Color::Red,
            AnsiColorName::Green => Color::Green,
            AnsiColorName::Yellow => Color::Yellow,
            AnsiColorName::Blue => Color::Blue,
            AnsiColorName::Magenta => Color::Magenta,
            AnsiColorName::Cyan => Color::Cyan,
            AnsiColorName::White => Color::Gray,
            AnsiColorName::BrightBlack => Color::DarkGray,
            AnsiColorName::BrightRed => Color::LightRed,
            AnsiColorName::BrightGreen => Color::LightGreen,
            AnsiColorName::BrightYellow => Color::LightYellow,
            AnsiColorName::BrightBlue => Color::LightBlue,
            AnsiColorName::BrightMagenta => Color::LightMagenta,
            AnsiColorName::BrightCyan => Color::LightCyan,
            AnsiColorName::BrightWhite => Color::White,
        },
        SpanColor::Rgb(hex) => parse_hex(hex).map_or(Color::Reset, |(r, g, b)| Color::Rgb(r, g, b)),
    }
}

/// The palette a recorded frame was drawn in.
pub fn palette(palette: &SpanPalette) -> TerminalColorPalette {
    let rgb = |hex: &str| parse_hex(hex).map(|(r, g, b)| Color::Rgb(r, g, b));
    let mut resolved = TerminalColorPalette {
        foreground: rgb(&palette.foreground),
        background: rgb(&palette.background),
        ..TerminalColorPalette::default()
    };
    for (slot, hex) in resolved.ansi.iter_mut().zip(&palette.ansi) {
        if let Some(color) = rgb(hex) {
            *slot = color;
        }
    }
    resolved
}

fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
    let digits = hex.strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(digits.get(at..at + 2)?, 16).ok();
    Some((channel(0)?, channel(2)?, channel(4)?))
}
