use std::sync::Arc;

use tui_lipan::prelude::*;

pub struct TerminalPane {
    pub screen: TerminalScreen,
    pub snapshot: TerminalRenderSnapshot,
    pub pty: Option<TerminalPty>,
    pub cols: u16,
    pub rows: u16,
    pub status: ManagedTerminalStatus,
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
}

impl TerminalPane {
    pub fn new(scrollback: usize) -> Self {
        let cols = 120;
        let rows = 32;
        Self {
            screen: TerminalScreen::new(rows, cols, scrollback),
            snapshot: TerminalRenderSnapshot::default(),
            pty: None,
            cols,
            rows,
            status: ManagedTerminalStatus::Starting,
        }
    }

    pub fn set_palette(&mut self, palette: TerminalColorPalette) -> bool {
        if self.screen.palette() == palette {
            return false;
        }
        self.screen.set_palette(palette);
        self.snapshot = self.screen.render_snapshot();
        true
    }

    pub fn set_pty(&mut self, pty: TerminalPty) -> std::result::Result<(), String> {
        pty.resize(self.cols, self.rows)
            .map_err(|err| format!("pty resize failed: {err}"))?;
        self.pty = Some(pty);
        self.status = ManagedTerminalStatus::Ready;
        Ok(())
    }

    pub fn handle_pty_event(&mut self, event: TerminalPtyEvent) -> PaneEventOutcome {
        match event {
            TerminalPtyEvent::Output(bytes) => {
                self.screen.process_bytes(&bytes);
                if let Some(pty) = &self.pty {
                    for response in self.screen.drain_responses() {
                        if let Err(err) = pty.write(&response) {
                            self.status = ManagedTerminalStatus::Error(Arc::from(format!(
                                "pty response write failed: {err}"
                            )));
                            self.snapshot = self.screen.render_snapshot();
                            return PaneEventOutcome::StatusChanged;
                        }
                    }
                }
                self.snapshot = self.screen.render_snapshot();
                PaneEventOutcome::Repaint
            }
            TerminalPtyEvent::Exited(code) => {
                self.status = ManagedTerminalStatus::Exited(code);
                self.pty = None;
                PaneEventOutcome::Exited(code)
            }
            TerminalPtyEvent::Error(message) => {
                self.status = ManagedTerminalStatus::Error(message);
                PaneEventOutcome::StatusChanged
            }
        }
    }

    pub fn send_key(&mut self, key: KeyEvent) -> std::result::Result<PaneWriteResult, String> {
        let Some(pty) = &self.pty else {
            return Ok(PaneWriteResult::default());
        };
        let forwarded = pty
            .send_key(key)
            .map_err(|err| format!("stdin write failed: {err}"))?;
        let repaint = self.snap_to_live_scrollback();
        Ok(PaneWriteResult { forwarded, repaint })
    }

    pub fn send_bytes(&mut self, bytes: &[u8]) -> std::result::Result<(), String> {
        let Some(pty) = &self.pty else {
            return Ok(());
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
        if let Some(pty) = &self.pty {
            pty.resize(cols, rows)
                .map_err(|err| format!("pty resize failed: {err}"))?;
        }
        self.screen.resize(rows, cols);
        self.snapshot = self.screen.render_snapshot();
        Ok(true)
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

        let needle = query.to_ascii_lowercase();
        let original_offset = self.screen.scrollback_offset();
        let max_offset = self.screen.total_scrollback_rows();
        let mut matches = Vec::new();
        let mut seen_lines = std::collections::HashMap::new();
        let step = usize::from(self.rows.max(1));

        let mut offset = max_offset;
        loop {
            self.screen.set_scrollback(offset);
            let snapshot = self.screen.render_snapshot();
            for (line, text) in snapshot.text.lines().enumerate() {
                if text.to_ascii_lowercase().contains(&needle) {
                    let logical_line = line as isize - offset as isize;
                    let matched = TerminalSearchMatch { offset, line };
                    if let Some(index) = seen_lines.get(&logical_line).copied() {
                        // Prefer the lowest scrollback offset for overlapping scan windows, so
                        // matches already visible in the live viewport do not jump upward.
                        matches[index] = matched;
                    } else {
                        seen_lines.insert(logical_line, matches.len());
                        matches.push(matched);
                    }
                }
            }
            if offset == 0 {
                break;
            }
            offset = offset.saturating_sub(step);
        }

        self.screen.set_scrollback(original_offset);
        self.snapshot = self.screen.render_snapshot();
        matches
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
        if let Some(pty) = self.pty.take() {
            let _ = pty.kill();
        }
    }

    /// The live working directory of the pane's shell, read on demand from `/proc/<pid>/cwd`
    /// (Linux only). Returns `None` when there is no PTY, no pid, or off Linux. This reads the
    /// shell leader's cwd, which matches most terminals' "open here" behavior.
    pub fn working_directory(&self) -> Option<String> {
        #[cfg(target_os = "linux")]
        {
            let pid = self.pty.as_ref()?.pid()?;
            let path = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
            Some(path.to_string_lossy().to_string())
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }

    /// The title the running program set via OSC 0/2 (shell `$PWD`, `vim`, etc.),
    /// trimmed and ignored when blank. `None` falls back to the pane's own label.
    pub fn title(&self) -> Option<String> {
        self.screen
            .title()
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
        if self.screen.scrollback_offset() == 0 {
            return false;
        }
        self.screen.set_scrollback(0);
        self.snapshot = self.screen.render_snapshot();
        true
    }
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
}
