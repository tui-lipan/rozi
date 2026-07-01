use std::sync::Arc;

use tui_lipan::prelude::*;

pub struct TerminalPane {
    pub pane_id: crate::state::PaneId,
    pub generation: u64,
    pub snapshot: TerminalRenderSnapshot,
    pub cols: u16,
    pub rows: u16,
    pub status: ManagedTerminalStatus,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub last_palette: Option<TerminalColorPalette>,
    backend: TerminalBackend,
}

enum TerminalBackend {
    /// Compatibility runtime used by the app until Phase 3.4 wires session attach/lifecycle.
    Local {
        screen: Box<TerminalScreen>,
        pty: Option<TerminalPty>,
    },
    /// Server-backed cache façade. It intentionally owns no PTY/screen.
    Server,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PaneWriteResult {
    pub forwarded: bool,
    pub repaint: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneEventOutcome {
    Repaint,
    StatusChanged,
    Exited(i32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalSearchMatch {
    pub offset: usize,
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSearchHighlight {
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
}

impl TerminalPane {
    pub fn new(scrollback: usize) -> Self {
        let cols = 120;
        let rows = 32;
        let mut screen = TerminalScreen::new(rows, cols, scrollback);
        let snapshot = screen.render_snapshot();
        Self {
            pane_id: 0,
            generation: 0,
            snapshot,
            cols,
            rows,
            status: ManagedTerminalStatus::Starting,
            title: None,
            cwd: None,
            last_palette: None,
            backend: TerminalBackend::Local {
                screen: Box::new(screen),
                pty: None,
            },
        }
    }

    pub fn bind_session(&mut self, pane_id: crate::state::PaneId, generation: u64) {
        self.pane_id = pane_id;
        self.generation = generation;
    }

    pub fn bind_server_backend(&mut self, pane_id: crate::state::PaneId, generation: u64) {
        self.bind_session(pane_id, generation);
        self.backend = TerminalBackend::Server;
    }

    pub fn apply_snapshot(
        &mut self,
        snapshot: TerminalRenderSnapshot,
        title: Option<String>,
        cwd: Option<String>,
    ) {
        self.snapshot = snapshot;
        self.title = title.filter(|title| !title.trim().is_empty());
        self.cwd = cwd;
        self.status = ManagedTerminalStatus::Ready;
        self.backend = TerminalBackend::Server;
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.status, ManagedTerminalStatus::Ready)
    }

    pub fn accepts_input(&self) -> bool {
        match &self.backend {
            TerminalBackend::Local { pty, .. } => pty.is_some() && self.is_ready(),
            TerminalBackend::Server => self.is_ready(),
        }
    }

    pub fn is_server_backed(&self) -> bool {
        matches!(self.backend, TerminalBackend::Server)
    }

    pub fn set_palette(&mut self, palette: TerminalColorPalette) -> bool {
        if self.last_palette == Some(palette) {
            return false;
        }
        match &mut self.backend {
            TerminalBackend::Local { screen, .. } => {
                screen.set_palette(palette);
                self.snapshot = screen.render_snapshot();
                self.last_palette = Some(palette);
                true
            }
            TerminalBackend::Server => false,
        }
    }

    pub fn set_pty(&mut self, pty: TerminalPty) -> std::result::Result<(), String> {
        pty.resize(self.cols, self.rows)
            .map_err(|err| format!("pty resize failed: {err}"))?;
        match &mut self.backend {
            TerminalBackend::Local { pty: slot, .. } => *slot = Some(pty),
            TerminalBackend::Server => {
                let mut screen = TerminalScreen::new(self.rows, self.cols, 0);
                if let Some(palette) = self.last_palette {
                    screen.set_palette(palette);
                }
                self.snapshot = screen.render_snapshot();
                self.backend = TerminalBackend::Local {
                    screen: Box::new(screen),
                    pty: Some(pty),
                };
            }
        }
        self.status = ManagedTerminalStatus::Ready;
        Ok(())
    }

    pub fn handle_pty_event(&mut self, event: TerminalPtyEvent) -> PaneEventOutcome {
        match event {
            TerminalPtyEvent::Output(bytes) => {
                let TerminalBackend::Local { screen, pty } = &mut self.backend else {
                    return PaneEventOutcome::Repaint;
                };
                screen.process_bytes(&bytes);
                if let Some(pty) = pty {
                    for response in screen.drain_responses() {
                        if let Err(err) = pty.write(&response) {
                            self.status = ManagedTerminalStatus::Error(Arc::from(format!(
                                "pty response write failed: {err}"
                            )));
                            self.snapshot = screen.render_snapshot();
                            return PaneEventOutcome::StatusChanged;
                        }
                    }
                }
                self.title = screen
                    .title()
                    .map(|title| title.trim().to_string())
                    .filter(|title| !title.is_empty());
                self.snapshot = screen.render_snapshot();
                PaneEventOutcome::Repaint
            }
            TerminalPtyEvent::Exited(code) => {
                self.status = ManagedTerminalStatus::Exited(code);
                if let TerminalBackend::Local { pty, .. } = &mut self.backend {
                    *pty = None;
                }
                PaneEventOutcome::Exited(code)
            }
            TerminalPtyEvent::Error(message) => {
                self.status = ManagedTerminalStatus::Error(message);
                PaneEventOutcome::StatusChanged
            }
        }
    }

    pub fn send_key(&mut self, key: KeyEvent) -> std::result::Result<PaneWriteResult, String> {
        if !self.accepts_input() {
            return Ok(PaneWriteResult::default());
        }
        let TerminalBackend::Local { pty: Some(pty), .. } = &mut self.backend else {
            return Err("server-backed key forwarding is not wired yet".to_string());
        };
        let forwarded = pty
            .send_key(key)
            .map_err(|err| format!("stdin write failed: {err}"))?;
        let repaint = self.snap_to_live_scrollback();
        Ok(PaneWriteResult { forwarded, repaint })
    }

    pub fn send_bytes(&mut self, bytes: &[u8]) -> std::result::Result<(), String> {
        if !self.accepts_input() {
            return Ok(());
        }
        let TerminalBackend::Local { pty: Some(pty), .. } = &mut self.backend else {
            return Err("server-backed byte forwarding is not wired yet".to_string());
        };
        pty.write(bytes)
            .map_err(|err| format!("pty write failed: {err}"))
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> std::result::Result<bool, String> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows {
            return Ok(false);
        }

        self.cols = cols;
        self.rows = rows;
        if let TerminalBackend::Local { screen, pty } = &mut self.backend {
            if let Some(pty) = pty {
                pty.resize(cols, rows)
                    .map_err(|err| format!("pty resize failed: {err}"))?;
            }
            screen.resize(rows, cols);
            self.snapshot = screen.render_snapshot();
        }
        Ok(true)
    }

    pub fn set_scrollback(&mut self, offset: usize) -> bool {
        if let TerminalBackend::Local { screen, .. } = &mut self.backend {
            if screen.scrollback_offset() == offset {
                return false;
            }
            screen.set_scrollback(offset);
            self.snapshot = screen.render_snapshot();
            return true;
        }
        if self.snapshot.scrollback_offset == offset {
            return false;
        }
        self.snapshot.scrollback_offset = offset;
        true
    }

    pub fn search_scrollback(&mut self, query: &str) -> Vec<TerminalSearchMatch> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }

        let TerminalBackend::Local { screen, .. } = &mut self.backend else {
            return Vec::new();
        };
        let original_offset = screen.scrollback_offset();
        let max_offset = screen.total_scrollback_rows();
        let mut matches = Vec::new();
        let mut seen_matches = std::collections::HashMap::new();
        let step = usize::from(self.rows.max(1));

        let mut offset = max_offset;
        loop {
            screen.set_scrollback(offset);
            let snapshot = screen.render_snapshot();
            for (line, text) in snapshot.text.lines().enumerate() {
                let logical_line = line as isize - offset as isize;
                for (start_col, end_col) in search_match_ranges(text, query) {
                    let matched = TerminalSearchMatch {
                        offset,
                        line,
                        start_col,
                        end_col,
                        text: text.to_string(),
                    };
                    let key = (logical_line, start_col, end_col);
                    if let Some(index) = seen_matches.get(&key).copied() {
                        matches[index] = matched;
                    } else {
                        seen_matches.insert(key, matches.len());
                        matches.push(matched);
                    }
                }
            }
            if offset == 0 {
                break;
            }
            offset = offset.saturating_sub(step);
        }

        screen.set_scrollback(original_offset);
        self.snapshot = screen.render_snapshot();
        matches
    }

    pub fn search_highlighted_snapshot(
        &self,
        query: &str,
        highlight_style: Style,
        active_highlight_style: Style,
        active_highlight: Option<TerminalSearchHighlight>,
    ) -> TerminalRenderSnapshot {
        let query = query.trim();
        if query.is_empty() {
            return self.snapshot.clone();
        }

        let mut snapshot = self.snapshot.clone();
        let plain_lines: Vec<&str> = snapshot.text.lines().collect();
        let mut color_lines: Vec<Vec<Span>> = snapshot.color_lines.iter().cloned().collect();
        let mut changed = false;

        for (row, spans) in color_lines.iter_mut().enumerate() {
            let Some(line) = plain_lines.get(row) else {
                continue;
            };
            let ranges = search_match_ranges(line, query);
            if ranges.is_empty() {
                continue;
            }
            *spans = highlight_span_ranges(
                row,
                spans,
                &ranges,
                highlight_style,
                active_highlight_style,
                active_highlight,
            );
            changed = true;
        }

        if changed {
            snapshot.color_lines = color_lines.into();
        }
        snapshot
    }

    /// Current cursor position in the visible snapshot grid as `(row, col)`.
    pub fn cursor_position(&self) -> (usize, usize) {
        (
            usize::from(self.snapshot.cursor_row),
            usize::from(self.snapshot.cursor_col),
        )
    }

    pub fn scrollback_offset(&self) -> usize {
        self.snapshot.scrollback_offset
    }

    pub fn total_scrollback_rows(&self) -> usize {
        self.snapshot.total_scrollback_rows
    }

    /// Extract the text covered by a selection from the current snapshot grid. `anchor` and
    /// `cursor` are `(row, col)` in visible-viewport coordinates; ordering is normalized.
    /// Trailing whitespace is trimmed per line and lines are joined with `\n`.
    pub fn extract_text(&self, anchor: (usize, usize), cursor: (usize, usize)) -> String {
        let (start, end) = if anchor <= cursor {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        };
        let lines: Vec<&str> = self.snapshot.text.lines().collect();
        let mut out = String::new();
        for row in start.0..=end.0 {
            let Some(line) = lines.get(row) else {
                continue;
            };
            let chars: Vec<char> = line.chars().collect();
            let col_start = if row == start.0 { start.1 } else { 0 };
            let col_end = if row == end.0 {
                (end.1 + 1).min(chars.len())
            } else {
                chars.len()
            };
            let segment: String = chars
                .get(col_start..col_end.max(col_start))
                .map(|slice| slice.iter().collect())
                .unwrap_or_default();
            out.push_str(segment.trim_end());
            if row < end.0 {
                out.push('\n');
            }
        }
        out
    }

    pub fn kill(&mut self) {
        if let TerminalBackend::Local { pty, .. } = &mut self.backend
            && let Some(pty) = pty.take()
        {
            let _ = pty.kill();
        }
        self.status = ManagedTerminalStatus::Exited(0);
    }

    /// The working directory last reported by the session server for the pane's shell.
    pub fn working_directory(&self) -> Option<String> {
        if let TerminalBackend::Local { pty, .. } = &self.backend
            && let Some(pid) = pty.as_ref().and_then(TerminalPty::pid)
        {
            return cwd_for_pid(pid);
        }
        self.cwd.clone()
    }

    /// The title the running program set via OSC 0/2 (shell `$PWD`, `vim`, etc.),
    /// trimmed and ignored when blank. `None` falls back to the pane's own label.
    pub fn title(&self) -> Option<String> {
        self.title
            .clone()
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty())
    }

    pub fn status_text(&self) -> String {
        match &self.status {
            ManagedTerminalStatus::Starting => "starting".to_string(),
            ManagedTerminalStatus::Ready => format!("{}×{}", self.cols, self.rows),
            ManagedTerminalStatus::Exited(code) => format!("exited {code}"),
            ManagedTerminalStatus::Error(message) => format!("error: {message}"),
        }
    }

    fn snap_to_live_scrollback(&mut self) -> bool {
        let TerminalBackend::Local { screen, .. } = &mut self.backend else {
            return false;
        };
        if screen.scrollback_offset() == 0 {
            return false;
        }
        screen.set_scrollback(0);
        self.snapshot = screen.render_snapshot();
        true
    }
}

#[cfg(target_os = "linux")]
fn cwd_for_pid(pid: u32) -> Option<String> {
    let path = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    Some(path.to_string_lossy().to_string())
}

#[cfg(not(target_os = "linux"))]
fn cwd_for_pid(_pid: u32) -> Option<String> {
    None
}

fn search_match_ranges(line: &str, query: &str) -> Vec<(usize, usize)> {
    let needle = query.to_ascii_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }

    let haystack = line.to_ascii_lowercase();
    let mut ranges = Vec::new();
    let mut search_from = 0usize;
    while search_from < haystack.len() {
        let Some(relative_start) = haystack[search_from..].find(&needle) else {
            break;
        };
        let start = search_from + relative_start;
        let end = start + needle.len();
        let start_col = haystack[..start].chars().count();
        let end_col = haystack[..end].chars().count();
        if start_col < end_col {
            ranges.push((start_col, end_col));
        }
        search_from = end;
    }
    ranges
}

fn highlight_span_ranges(
    row: usize,
    spans: &[Span],
    ranges: &[(usize, usize)],
    highlight_style: Style,
    active_highlight_style: Style,
    active_highlight: Option<TerminalSearchHighlight>,
) -> Vec<Span> {
    let mut out = Vec::new();
    let mut col = 0usize;

    for span in spans {
        let chars: Vec<char> = span.content.chars().collect();
        let span_start = col;
        let span_end = span_start + chars.len();
        let mut local_start = 0usize;

        for &(range_start, range_end) in ranges {
            if range_end <= span_start {
                continue;
            }
            if range_start >= span_end {
                break;
            }

            let highlight_start = range_start.max(span_start) - span_start;
            let highlight_end = range_end.min(span_end) - span_start;
            let style = if active_highlight.is_some_and(|active| {
                active.line == row && active.start_col == range_start && active.end_col == range_end
            }) {
                active_highlight_style
            } else {
                highlight_style
            };
            push_span_segment(&mut out, span, &chars, local_start, highlight_start, None);
            push_span_segment(
                &mut out,
                span,
                &chars,
                highlight_start,
                highlight_end,
                Some(style),
            );
            local_start = highlight_end;
        }

        push_span_segment(&mut out, span, &chars, local_start, chars.len(), None);
        col = span_end;
    }

    out
}

fn push_span_segment(
    out: &mut Vec<Span>,
    source: &Span,
    chars: &[char],
    start: usize,
    end: usize,
    style_patch: Option<Style>,
) {
    if start >= end {
        return;
    }
    let mut span = source.clone();
    span.content = chars[start..end].iter().collect::<String>().into();
    if let Some(style_patch) = style_patch {
        span.style = span.style.patch(style_patch);
    }
    out.push(span);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_text_trims_and_joins_selected_rows() {
        let mut pane = TerminalPane::new(100);
        pane.snapshot = TerminalRenderSnapshot {
            text: std::sync::Arc::from("hello world   \nfoo bar\nbaz"),
            ..TerminalRenderSnapshot::default()
        };

        // Single-line span is inclusive of the cursor cell and trims trailing space.
        assert_eq!(pane.extract_text((0, 0), (0, 4)), "hello");
        // Multi-line span joins rows with newlines, trimming each line's trailing space.
        assert_eq!(
            pane.extract_text((0, 0), (2, 2)),
            "hello world\nfoo bar\nbaz"
        );
        // Anchor/cursor order is normalized.
        assert_eq!(pane.extract_text((0, 4), (0, 0)), "hello");
    }

    #[test]
    fn search_match_ranges_returns_each_case_insensitive_occurrence() {
        assert_eq!(
            search_match_ranges("Alpha beta alpha", "alpha"),
            vec![(0, 5), (11, 16)]
        );
    }

    #[test]
    fn search_highlighted_snapshot_marks_all_visible_matches() {
        let mut pane = TerminalPane::new(100);
        let base_style = Style::new().fg(Color::Green);
        let highlight_style = Style::new().fg(Color::White).bg(Color::rgb(92, 64, 8));
        let active_highlight_style = Style::new().fg(Color::Black).bg(Color::Yellow).bold();
        pane.snapshot = TerminalRenderSnapshot {
            text: std::sync::Arc::from("Alpha beta alpha"),
            color_lines: std::sync::Arc::from([vec![
                Span::new("Alpha beta alpha").style(base_style),
            ]]),
            ..TerminalRenderSnapshot::default()
        };

        let snapshot = pane.search_highlighted_snapshot(
            "alpha",
            highlight_style,
            active_highlight_style,
            None,
        );
        let line = &snapshot.color_lines[0];
        assert_eq!(line.len(), 3);
        assert_eq!(line[0].content.as_ref(), "Alpha");
        assert_eq!(line[1].content.as_ref(), " beta ");
        assert_eq!(line[2].content.as_ref(), "alpha");
        assert_eq!(line[0].style, base_style.patch(highlight_style));
        assert_eq!(line[1].style, base_style);
        assert_eq!(line[2].style, base_style.patch(highlight_style));
    }

    #[test]
    fn search_highlighted_snapshot_marks_active_match_differently() {
        let mut pane = TerminalPane::new(100);
        let base_style = Style::new().fg(Color::Green);
        let highlight_style = Style::new().fg(Color::White).bg(Color::rgb(92, 64, 8));
        let active_highlight_style = Style::new().fg(Color::Black).bg(Color::Yellow).bold();
        pane.snapshot = TerminalRenderSnapshot {
            text: std::sync::Arc::from("Alpha beta alpha"),
            color_lines: std::sync::Arc::from([vec![
                Span::new("Alpha beta alpha").style(base_style),
            ]]),
            ..TerminalRenderSnapshot::default()
        };

        let snapshot = pane.search_highlighted_snapshot(
            "alpha",
            highlight_style,
            active_highlight_style,
            Some(TerminalSearchHighlight {
                line: 0,
                start_col: 11,
                end_col: 16,
            }),
        );
        let line = &snapshot.color_lines[0];
        assert_eq!(line[0].style, base_style.patch(highlight_style));
        assert_eq!(line[2].style, base_style.patch(active_highlight_style));
    }
}
