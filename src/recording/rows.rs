//! Row changes smaller than a row: the column spans that differ.
//!
//! A dashboard such as `btop` changes most rows on every update, but only a few cells in each, so
//! resending whole rows writes several times what changed. A partial [`RowChange`] replaces just
//! the columns its runs cover. Every partial change is checked by applying it before it is written,
//! and a row it would not reproduce exactly is written whole instead.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::format::RowChange;
use crate::control::SpanRun;

/// Unchanged columns between two changed spans that are still sent, rather than starting another
/// span: a span's own overhead costs about this much.
const SPAN_GAP: usize = 6;
/// A row whose changed columns cover more than this share of it is sent whole.
const WHOLE_ROW_SHARE: f64 = 0.6;

/// The style of a blank cell.
static DEFAULT_STYLE: SpanRun = SpanRun {
    x: 0,
    width: 0,
    text: String::new(),
    fg: None,
    bg: None,
    underline_color: None,
    bold: false,
    dim: false,
    italic: false,
    underline: None,
    reverse: false,
    strikethrough: false,
};

/// Whether two runs look alike, wherever they are and whatever they say.
fn same_style(a: &SpanRun, b: &SpanRun) -> bool {
    a.fg == b.fg
        && a.bg == b.bg
        && a.underline_color == b.underline_color
        && a.bold == b.bold
        && a.dim == b.dim
        && a.italic == b.italic
        && a.underline == b.underline
        && a.reverse == b.reverse
        && a.strikethrough == b.strikethrough
}

/// One column of a row, borrowed from the runs it came from: the glyph that starts there, or the
/// second half of a wide one.
#[derive(Clone, Copy, Debug)]
struct Column<'a> {
    /// Empty for a wide glyph's second column.
    text: &'a str,
    width: u16,
    style: &'a SpanRun,
}

impl PartialEq for Column<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text && self.width == other.width && same_style(self.style, other.style)
    }
}

const BLANK: Column<'static> = Column {
    text: " ",
    width: 1,
    style: &DEFAULT_STYLE,
};

/// Lay `runs` over `columns`, each at its own `x`.
fn lay<'a>(columns: &mut [Column<'a>], runs: &'a [SpanRun]) {
    for run in runs {
        let end = usize::from(run.x.saturating_add(run.width)).min(columns.len());
        let mut x = usize::from(run.x);
        for grapheme in run.text.graphemes(true) {
            if x >= end {
                break;
            }
            let width = UnicodeWidthStr::width(grapheme).max(1);
            columns[x] = Column {
                text: grapheme,
                width: width as u16,
                style: run,
            };
            for covered in columns.iter_mut().take(end.min(x + width)).skip(x + 1) {
                *covered = Column {
                    text: "",
                    width: 0,
                    style: run,
                };
            }
            x += width;
        }
        for column in columns.iter_mut().take(end).skip(x) {
            *column = Column { style: run, ..BLANK };
        }
    }
}

fn columns(runs: &[SpanRun], width: u16) -> Vec<Column<'_>> {
    let mut columns = vec![BLANK; usize::from(width)];
    lay(&mut columns, runs);
    columns
}

/// `columns[start..end]` as runs, each as long as its style lasts.
fn runs(columns: &[Column<'_>], start: usize, end: usize) -> Vec<SpanRun> {
    let mut out: Vec<SpanRun> = Vec::new();
    for (x, column) in columns.iter().enumerate().take(end).skip(start) {
        match out.last_mut() {
            Some(last) if same_style(last, column.style) => {
                last.text.push_str(column.text);
                last.width += 1;
            }
            _ => out.push(SpanRun {
                x: x as u16,
                width: 1,
                text: column.text.to_string(),
                ..column.style.clone()
            }),
        }
    }
    out
}

/// A row as a `rozi-spans` frame writes it: runs as long as their style, and the default blanks
/// at the end left out.
fn canonical(columns: &[Column<'_>]) -> Vec<SpanRun> {
    let mut row = runs(columns, 0, columns.len());
    if let Some(last) = row.last_mut()
        && same_style(last, &DEFAULT_STYLE)
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

/// Apply one change to a row of a frame `width` columns wide.
pub fn apply(row: &mut Vec<SpanRun>, change: &RowChange, width: u16) {
    if !change.partial {
        row.clone_from(&change.runs);
        return;
    }
    let mut laid = columns(row, width);
    lay(&mut laid, &change.runs);
    *row = canonical(&laid);
}

/// The changes that turn `old` into `new`: the spans that differ, or the whole row when that is as
/// cheap or a span would not reproduce it exactly.
pub fn diff(y: u16, old: &[SpanRun], new: &[SpanRun], width: u16) -> Vec<RowChange> {
    let whole = || {
        vec![RowChange {
            y,
            runs: new.to_vec(),
            partial: false,
        }]
    };
    let before = columns(old, width);
    let after = columns(new, width);
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut changed = 0;
    for x in (0..after.len()).filter(|&x| before[x] != after[x]) {
        changed += 1;
        match spans.last_mut() {
            Some((_, end)) if x <= *end + SPAN_GAP => *end = x + 1,
            _ => spans.push((x, x + 1)),
        }
    }
    if spans.is_empty() {
        return Vec::new();
    }
    if f64::from(changed) > f64::from(width) * WHOLE_ROW_SHARE {
        return whole();
    }
    let mut changes = Vec::with_capacity(spans.len());
    for (mut start, mut end) in spans {
        // Never start on, or stop inside, a wide glyph.
        while start > 0 && after[start].width == 0 {
            start -= 1;
        }
        while end < after.len() && after[end].width == 0 {
            end += 1;
        }
        changes.push(RowChange {
            y,
            runs: runs(&after, start, end),
            partial: true,
        });
    }
    let mut laid = before;
    for change in &changes {
        lay(&mut laid, &change.runs);
    }
    if canonical(&laid) != new {
        return whole();
    }
    changes
}
