use tui_lipan::prelude::*;

/// A terminal pane. Its screen is a client-side `TerminalScreen` parser fed by raw PTY bytes
/// broadcast from the session server; the server owns the actual PTY.
pub struct TerminalPane {
    pub pane_id: crate::state::PaneId,
    pub generation: u64,
    pub snapshot: TerminalRenderSnapshot,
    pub cols: u16,
    pub rows: u16,
    pub status: ManagedTerminalStatus,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub cwd_host: Option<String>,
    pub child_pid: Option<u32>,
    /// Normalized foreground-executable basename, server-authoritative (cross-platform plan
    /// Phase 6/7): pushed down via [`crate::session::protocol::PaneMeta::runtime`] and
    /// [`crate::session::protocol::ServerMessage::PaneRuntimeChanged`] rather than inspected
    /// locally, since the server (not necessarily this client's host) owns the PTY.
    pub foreground_program: Option<String>,
    /// Free-form status reported by the pane through the session server. This is distinct from
    /// `status`, which tracks whether the client-side terminal parser is ready or exited.
    pub reported_status: Option<crate::session::protocol::PaneStatus>,
    pub detected_agent: Option<crate::session::protocol::DetectedAgent>,
    pub command_phase: crate::session::protocol::PaneCommandPhase,
    pub last_exit_status: Option<i32>,
    pub runtime_sequence: u64,
    pub last_palette: Option<TerminalColorPalette>,
    seen_bell_count: u64,
    screen: Box<TerminalScreen>,
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
            cwd_host: None,
            child_pid: None,
            foreground_program: None,
            reported_status: None,
            detected_agent: None,
            command_phase: crate::session::protocol::PaneCommandPhase::Unknown,
            last_exit_status: None,
            runtime_sequence: 0,
            last_palette: None,
            seen_bell_count: 0,
            screen: Box::new(screen),
        }
    }

    pub fn bind_session(&mut self, pane_id: crate::state::PaneId, generation: u64) {
        if self.pane_id != pane_id || self.generation != generation {
            self.reported_status = None;
            self.detected_agent = None;
        }
        self.pane_id = pane_id;
        self.generation = generation;
    }

    /// Prepare the pane to (re)receive a server pane's output: reset the parser to a fresh screen
    /// of the current size, ready to be seeded by the replay bytes that follow an attach or spawn.
    pub fn bind_server_backend(&mut self, pane_id: crate::state::PaneId, generation: u64) {
        self.bind_session(pane_id, generation);
        self.runtime_sequence = 0;
        *self.screen = TerminalScreen::new(self.rows, self.cols, 5000);
        self.seen_bell_count = self.screen.bell_count();
        if let Some(palette) = self.last_palette {
            self.screen.set_palette(palette);
        }
        self.snapshot = self.screen.render_snapshot();
    }

    pub fn take_bell(&mut self) -> bool {
        let count = self.screen.bell_count();
        let rang = count > self.seen_bell_count;
        self.seen_bell_count = count;
        rang
    }

    /// Feed raw PTY bytes broadcast by the server into the client-side parser. Query responses
    /// (DA/DSR/OSC) are discarded here: the server's own screen already answered them.
    pub fn process_server_output(&mut self, bytes: &[u8]) -> PaneEventOutcome {
        self.screen.process_bytes(bytes);
        let _ = self.screen.drain_responses();
        self.title = self.screen.title().and_then(sanitize_terminal_title);
        self.snapshot = self.screen.render_snapshot();
        self.status = ManagedTerminalStatus::Ready;
        PaneEventOutcome::Repaint
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.status, ManagedTerminalStatus::Ready)
    }

    pub fn is_running(&self) -> bool {
        matches!(
            self.status,
            ManagedTerminalStatus::Starting | ManagedTerminalStatus::Ready
        )
    }

    pub fn accepts_input(&self) -> bool {
        self.is_ready()
    }

    pub fn set_palette(&mut self, palette: TerminalColorPalette) -> bool {
        if self.last_palette == Some(palette) {
            return false;
        }
        self.screen.set_palette(palette);
        self.snapshot = self.screen.render_snapshot();
        self.last_palette = Some(palette);
        true
    }

    /// Resize the client-side parser. The server owns the PTY, so this only reshapes the local
    /// screen; it is driven by the server's ordered `Resized` broadcast so both parsers reshape
    /// at the same byte position.
    pub fn apply_server_resize(&mut self, cols: u16, rows: u16) -> bool {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows {
            return false;
        }
        self.cols = cols;
        self.rows = rows;
        self.screen.resize(rows, cols);
        self.snapshot = self.screen.render_snapshot();
        true
    }

    pub fn set_scrollback(&mut self, offset: usize) -> bool {
        if self.screen.scrollback_offset() == offset {
            return false;
        }
        self.screen.set_scrollback(offset);
        self.snapshot = self.screen.render_snapshot();
        true
    }

    pub fn search_scrollback(&mut self, query: &str) -> Vec<TerminalSearchMatch> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }

        let screen = &mut self.screen;
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

    pub fn hint_snapshot(
        &self,
        matches: &[crate::hints::HintMatch],
        labels: &[String],
        input: &str,
        match_style: Style,
        label_style: Style,
    ) -> TerminalRenderSnapshot {
        let mut snapshot = self.snapshot.clone();
        let mut lines: Vec<Vec<Span>> = snapshot.color_lines.iter().cloned().collect();
        for (index, matched) in matches.iter().enumerate().rev() {
            let Some(label) = labels.get(index) else {
                continue;
            };
            if !label.starts_with(input) {
                continue;
            }
            let Some(spans) = lines.get_mut(matched.row) else {
                continue;
            };
            let range = [(matched.start_col, matched.end_col)];
            *spans =
                highlight_span_ranges(matched.row, spans, &range, match_style, match_style, None);
            insert_styled_span(spans, matched.end_col, label, label_style);
        }
        snapshot.color_lines = lines.into();
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

    /// Plain text of the current visible snapshot grid (reflecting whatever scrollback offset
    /// is currently applied), one row per line, joined with `\n`.
    pub fn capture_text(&self) -> String {
        self.snapshot.text.to_string()
    }

    /// Plain, right-trimmed text of a single row in the current snapshot grid, or an empty
    /// string when `row` is out of range.
    pub fn row_text(&self, row: usize) -> String {
        self.snapshot
            .text
            .lines()
            .nth(row)
            .unwrap_or("")
            .trim_end()
            .to_string()
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

    /// Mark the pane as exited locally. The server owns the PTY and is asked to kill it via a
    /// separate `Kill` RPC (see `close_pane_state`).
    pub fn kill(&mut self) {
        self.status = ManagedTerminalStatus::Exited(0);
    }

    /// The live working directory of the pane's shell.
    ///
    /// Server-authoritative (cross-platform plan Phase 6): the session server tracks this from
    /// shell-integration OSC reports and native process-inspection fallbacks and pushes it down via
    /// `PaneMeta`/`PaneRuntimeChanged`, rather than this client inspecting `/proc` itself - the
    /// server, not necessarily this client's host, owns the PTY and process identity.
    pub fn working_directory(&self) -> Option<String> {
        self.cwd.clone()
    }

    /// Return the working directory only when it belongs to this client's host.
    pub fn local_working_directory(&self) -> Option<String> {
        self.cwd.clone().filter(|_| self.cwd_host.is_none())
    }

    /// The command name of the process currently in the foreground of the pane's terminal
    /// (e.g. `bash` at a prompt, `nvim` while editing).
    ///
    /// Server-authoritative (cross-platform plan Phase 6/7), same rationale as
    /// [`working_directory`](Self::working_directory). This is the terminal's foreground process
    /// group leader, so it reflects the actually-running program regardless of shell/process-tree
    /// depth - the signal a vim-tmux-navigator-style binding uses to decide whether
    /// `Ctrl-h/j/k/l` should move focus or be forwarded to the program. Returns `None` when it
    /// cannot be determined.
    pub fn foreground_command(&self) -> Option<String> {
        self.foreground_program.clone()
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
}

pub(crate) fn sanitize_terminal_title(title: String) -> Option<String> {
    let title = title.trim();
    if title.is_empty() {
        return None;
    }
    let title = title
        .strip_prefix("Administrator: ")
        .or_else(|| title.strip_prefix("Administrator:  "))
        .unwrap_or(title);
    let lower = title.to_ascii_lowercase();
    if lower.contains("\\windowspowershell\\") && lower.contains("powershell.exe") {
        return Some("PowerShell".to_string());
    }
    if lower.ends_with("\\pwsh.exe") || lower == "pwsh.exe" {
        return Some("PowerShell".to_string());
    }
    Some(title.chars().filter(|ch| !ch.is_control()).collect())
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

fn insert_styled_span(spans: &mut Vec<Span>, col: usize, content: &str, style: Style) {
    let inserted = Span::new(content).style(style);
    let mut span_start = 0usize;

    for index in 0..spans.len() {
        let chars: Vec<char> = spans[index].content.chars().collect();
        let span_end = span_start + chars.len();
        if col > span_end {
            span_start = span_end;
            continue;
        }
        if col == span_start {
            spans.insert(index, inserted);
            return;
        }
        if col == span_end {
            spans.insert(index + 1, inserted);
            return;
        }

        let split = col - span_start;
        let mut right = spans[index].clone();
        spans[index].content = chars[..split].iter().collect::<String>().into();
        right.content = chars[split..].iter().collect::<String>().into();
        spans.insert(index + 1, inserted);
        spans.insert(index + 2, right);
        return;
    }

    spans.push(inserted);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_verbose_elevated_windows_powershell_title() {
        assert_eq!(
            sanitize_terminal_title(
                r"Administrator: C:\WINDOWS\System32\WindowsPowerShell\v1.0\powershell.exe - C:\Users\Razuer\Downloads".to_string()
            ),
            Some("PowerShell".to_string())
        );
    }

    #[test]
    fn preserves_normal_terminal_titles() {
        assert_eq!(
            sanitize_terminal_title(" nvim - src/main.rs ".to_string()),
            Some("nvim - src/main.rs".to_string())
        );
    }

    #[test]
    fn take_bell_uses_a_watermark() {
        let mut pane = TerminalPane::new(100);
        assert!(!pane.take_bell());
        pane.process_server_output(b"\x07");
        assert!(pane.take_bell());
        assert!(!pane.take_bell());
    }

    #[test]
    fn binding_a_different_backend_generation_clears_reported_status() {
        let mut pane = TerminalPane::new(100);
        pane.bind_session(1, 2);
        pane.reported_status = Some(crate::session::protocol::PaneStatus {
            value: "working".into(),
            reason: None,
            set_at: 1,
        });

        pane.bind_session(1, 2);
        assert!(pane.reported_status.is_some());
        pane.bind_session(1, 3);
        assert!(pane.reported_status.is_none());
    }

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

    #[test]
    fn hint_snapshot_appends_distinct_labels_without_replacing_match_text() {
        let mut pane = TerminalPane::new(100);
        let base_style = Style::new().fg(Color::Green);
        let match_style = Style::new().fg(Color::White).bg(Color::rgb(92, 64, 8));
        let label_style = Style::new().fg(Color::Black).bg(Color::Yellow).bold();
        pane.snapshot = TerminalRenderSnapshot {
            text: std::sync::Arc::from("go https://x.test then ./src/main.rs"),
            color_lines: std::sync::Arc::from([vec![
                Span::new("go https://x.test then ./src/main.rs").style(base_style),
            ]]),
            ..TerminalRenderSnapshot::default()
        };
        let matches = [
            crate::hints::HintMatch {
                row: 0,
                start_col: 3,
                end_col: 17,
                text: "https://x.test".to_string(),
                kind: crate::hints::HintKind::Url,
            },
            crate::hints::HintMatch {
                row: 0,
                start_col: 23,
                end_col: 36,
                text: "./src/main.rs".to_string(),
                kind: crate::hints::HintKind::Path,
            },
        ];

        let snapshot = pane.hint_snapshot(
            &matches,
            &["a".to_string(), "s".to_string()],
            "",
            match_style,
            label_style,
        );
        let line = &snapshot.color_lines[0];
        assert_eq!(
            line.iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "go https://x.testa then ./src/main.rss"
        );
        assert_eq!(line[1].content.as_ref(), "https://x.test");
        assert_eq!(line[1].style, base_style.patch(match_style));
        assert_eq!(line[2].content.as_ref(), "a");
        assert_eq!(line[2].style, label_style);
        assert_eq!(line[4].content.as_ref(), "./src/main.rs");
        assert_eq!(line[4].style, base_style.patch(match_style));
        assert_eq!(line[5].content.as_ref(), "s");
        assert_eq!(line[5].style, label_style);
    }
}
