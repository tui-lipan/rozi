use std::cell::RefCell;

use tui_lipan::prelude::*;

/// A terminal pane. Its screen is a client-side `TerminalScreen` parser fed by raw PTY bytes
/// broadcast from the session server; the server owns the actual PTY.
pub struct TerminalPane {
    pub pane_id: crate::state::PaneId,
    pub generation: u64,
    pub cols: u16,
    pub rows: u16,
    pub status: ManagedTerminalStatus,
    pub title: Option<String>,
    /// Account that launched the pane's original shell. Supplied by the session server on attach;
    /// fresh panes also learn it from their first conventional `user@host:cwd` shell title.
    pub original_user: Option<String>,
    pub cwd: Option<String>,
    pub cwd_host: Option<String>,
    /// Compact, server-computed cwd used as titlebar context.
    pub display_path: Option<String>,
    /// Server-computed Git project containing `cwd`, and the branch it has checked out. The
    /// sidebar's Agents tab groups by the root and heads each group with the branch.
    pub project_root: Option<String>,
    pub git_branch: Option<String>,
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
    /// Set when this agent pane's effective status transitions from `working` to a quiescent state
    /// (finished) while the pane is not focused, and cleared once the pane is focused. Drives the
    /// sidebar's "unseen finish" pulse so a completed run does not blend into panes that were idle
    /// all along. Never set for `blocked`, which already has its own attention glyph.
    pub finished_unseen: bool,
    /// Server timestamp for the current active agent run. It stays unchanged while an agent is
    /// blocked and resumes working, so the sidebar can show one continuous run age.
    pub work_started_at: Option<u64>,
    /// Fallback for peers that do not send [`Self::work_started_at`].
    pub status_since: Option<std::time::Instant>,
    /// How long the agent's last `working` stretch lasted, captured as it ended. A finished run
    /// reports what it cost, not how long ago it stopped — the attention pulse already says the
    /// finish is recent, and a number that climbs after the work is over says nothing.
    pub last_run: Option<std::time::Duration>,
    pub command_phase: crate::session::protocol::PaneCommandPhase,
    pub last_exit_status: Option<i32>,
    pub runtime_sequence: u64,
    pub last_palette: Option<TerminalColorPalette>,
    seen_bell_count: u64,
    /// Behind a `RefCell` so [`TerminalPane::snapshot`] can rebuild through a shared reference:
    /// the render snapshot is pulled by the view (which only ever holds `&State`), and rebuilding
    /// it at read time rather than at write time is what collapses a burst of output messages into
    /// one rebuild. See [`TerminalPane::process_server_output`].
    screen: RefCell<Box<TerminalScreen>>,
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

fn status_is_quiescent(value: &str) -> bool {
    let value = value.trim();
    value.eq_ignore_ascii_case(crate::session::protocol::pane_status::IDLE)
        || value.eq_ignore_ascii_case(crate::session::protocol::pane_status::DONE)
}

impl TerminalPane {
    pub fn new(scrollback: usize) -> Self {
        let cols = 120;
        let rows = 32;
        let screen = TerminalScreen::new(rows, cols, scrollback);
        Self {
            pane_id: 0,
            generation: 0,
            cols,
            rows,
            status: ManagedTerminalStatus::Starting,
            title: None,
            original_user: None,
            cwd: None,
            cwd_host: None,
            display_path: None,
            project_root: None,
            git_branch: None,
            child_pid: None,
            foreground_program: None,
            reported_status: None,
            detected_agent: None,
            finished_unseen: false,
            work_started_at: None,
            status_since: None,
            last_run: None,
            command_phase: crate::session::protocol::PaneCommandPhase::Unknown,
            last_exit_status: None,
            runtime_sequence: 0,
            last_palette: None,
            seen_bell_count: 0,
            screen: RefCell::new(Box::new(screen)),
        }
    }

    /// The current render snapshot, rebuilt on demand.
    ///
    /// `TerminalScreen` already caches its snapshot behind an internal dirty flag, so this is a
    /// cheap `Arc` clone whenever no bytes have arrived since the last call. Pulling it here rather
    /// than pushing it on every write is what makes a burst of output cost one rebuild instead of
    /// one per message.
    pub fn snapshot(&self) -> TerminalRenderSnapshot {
        self.screen.borrow_mut().render_snapshot()
    }

    pub fn bind_session(&mut self, pane_id: crate::state::PaneId, generation: u64) {
        if self.pane_id != pane_id || self.generation != generation {
            self.original_user = None;
            self.display_path = None;
            self.project_root = None;
            self.git_branch = None;
            self.reported_status = None;
            self.detected_agent = None;
            self.finished_unseen = false;
            self.work_started_at = None;
            self.status_since = None;
            self.last_run = None;
        }
        self.pane_id = pane_id;
        self.generation = generation;
    }

    /// Prepare the pane to (re)receive a server pane's output: reset the parser to a fresh screen
    /// of the current size, ready to be seeded by the replay bytes that follow an attach or spawn.
    pub fn bind_server_backend(&mut self, pane_id: crate::state::PaneId, generation: u64) {
        self.bind_session(pane_id, generation);
        self.runtime_sequence = 0;
        let mut screen = self.screen.borrow_mut();
        **screen = TerminalScreen::new(self.rows, self.cols, 5000);
        self.seen_bell_count = screen.bell_count();
        if let Some(palette) = self.last_palette {
            screen.set_palette(palette);
        }
        drop(screen);
    }

    pub fn take_bell(&mut self) -> bool {
        let count = self.screen.borrow().bell_count();
        let rang = count > self.seen_bell_count;
        self.seen_bell_count = count;
        rang
    }

    /// Feed raw PTY bytes broadcast by the server into the client-side parser. Query responses
    /// (DA/DSR/OSC) are discarded here: the server's own screen already answered them.
    pub fn process_server_output(&mut self, bytes: &[u8]) -> PaneEventOutcome {
        self.screen.borrow_mut().process_bytes(bytes);
        let _ = self.screen.borrow_mut().drain_responses();
        let title = self
            .screen
            .borrow()
            .title()
            .and_then(sanitize_terminal_title);
        if self.original_user.is_none() {
            self.original_user = title
                .as_deref()
                .and_then(shell_title_parts)
                .and_then(|(user, _)| user.map(str::to_string));
        }
        self.title = title;
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
        self.screen.borrow_mut().set_palette(palette);
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
        self.screen.borrow_mut().resize(rows, cols);
        true
    }

    pub fn set_scrollback(&mut self, offset: usize) -> bool {
        if self.screen.borrow().scrollback_offset() == offset {
            return false;
        }
        self.screen.borrow_mut().set_scrollback(offset);
        true
    }

    pub fn search_scrollback(&mut self, query: &str) -> Vec<TerminalSearchMatch> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }

        let total = self.screen.borrow().total_text_lines();
        // `text_lines(start, count)` returns up to `count` lines beginning at absolute
        // `start`. Indices below are `start + i` so a future clamp/shift in the exporter
        // cannot silently renumber matches.
        let start = 0;
        let lines = self.screen.borrow().text_lines(start, total);
        let mut matches = Vec::new();
        for (i, text) in lines.into_iter().enumerate() {
            let absolute = start + i;
            let Some((offset, line)) = self.screen.borrow().absolute_line_to_viewport(absolute)
            else {
                continue;
            };
            for (start_col, end_col) in search_match_ranges(&text, query) {
                matches.push(TerminalSearchMatch {
                    offset,
                    line,
                    start_col,
                    end_col,
                    text: text.clone(),
                });
            }
        }
        matches
    }

    pub fn search_highlighted_snapshot(
        &self,
        query: &str,
        highlight_style: Style,
        active_highlight_style: Style,
        active_highlight: Option<TerminalSearchHighlight>,
    ) -> TerminalRenderSnapshot {
        search_highlighted_snapshot(
            self.snapshot(),
            query,
            highlight_style,
            active_highlight_style,
            active_highlight,
        )
    }

    pub fn hint_snapshot(
        &self,
        matches: &[crate::hints::HintMatch],
        labels: &[String],
        input: &str,
        match_style: Style,
        label_style: Style,
    ) -> TerminalRenderSnapshot {
        hint_snapshot(
            self.snapshot(),
            matches,
            labels,
            input,
            match_style,
            label_style,
        )
    }
}

/// Overlay search highlights onto a snapshot. Split out from [`TerminalPane`] so it can be tested
/// against a synthetic snapshot; see [`extract_snapshot_text`].
fn search_highlighted_snapshot(
    mut snapshot: TerminalRenderSnapshot,
    query: &str,
    highlight_style: Style,
    active_highlight_style: Style,
    active_highlight: Option<TerminalSearchHighlight>,
) -> TerminalRenderSnapshot {
    let query = query.trim();
    if query.is_empty() {
        return snapshot;
    }

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

/// Overlay hint labels onto a snapshot. Split out from [`TerminalPane`] for testability; see
/// [`extract_snapshot_text`].
fn hint_snapshot(
    mut snapshot: TerminalRenderSnapshot,
    matches: &[crate::hints::HintMatch],
    labels: &[String],
    input: &str,
    match_style: Style,
    label_style: Style,
) -> TerminalRenderSnapshot {
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
        *spans = highlight_span_ranges(matched.row, spans, &range, match_style, match_style, None);
        insert_styled_span(spans, matched.end_col, label, label_style);
    }
    snapshot.color_lines = lines.into();
    snapshot
}

impl TerminalPane {
    /// Current cursor position in the visible snapshot grid as `(row, col)`.
    pub fn cursor_position(&self) -> (usize, usize) {
        (
            usize::from(self.snapshot().cursor_row),
            usize::from(self.snapshot().cursor_col),
        )
    }

    /// Read straight from the screen rather than through [`TerminalPane::snapshot`]:
    /// `process_bytes` keeps this field current on its own, so it needs no snapshot rebuild.
    pub fn scrollback_offset(&self) -> usize {
        self.screen.borrow().scrollback_offset()
    }

    pub fn total_scrollback_rows(&self) -> usize {
        self.screen.borrow_mut().total_scrollback_rows()
    }

    /// Plain text of the current visible snapshot grid (reflecting whatever scrollback offset
    /// is currently applied), one row per line, joined with `\n`.
    pub fn capture_text(&self) -> String {
        self.snapshot().text.to_string()
    }

    /// Plain text from retained scrollback history.
    ///
    /// `lines = None` exports the full retained grid (history + live). `Some(n)` exports the
    /// trailing `n` lines. Does not mutate the pane's scrollback offset.
    pub fn capture_scrollback_text(&self, lines: Option<usize>) -> String {
        let total = self.screen.borrow().total_text_lines();
        let start = match lines {
            None => 0,
            Some(n) => total.saturating_sub(n),
        };
        self.screen.borrow().export_text(start, total)
    }

    /// Plain text of the last shell-integration command's output, when marks are available.
    pub fn capture_last_command_output(&self) -> Option<String> {
        self.screen.borrow().export_last_command_output()
    }

    /// Absolute-line semantic marks from OSC 133 (Prompt / OutputStart / OutputEnd).
    pub fn semantic_marks(&self) -> Vec<tui_lipan::prelude::SemanticMark> {
        self.screen.borrow().semantic_marks()
    }

    /// Map an absolute text line to `(scrollback_offset, viewport_row)`.
    pub fn absolute_line_to_viewport(&self, absolute: usize) -> Option<(usize, usize)> {
        self.screen.borrow().absolute_line_to_viewport(absolute)
    }

    /// Plain, right-trimmed text of a single row in the current snapshot grid, or an empty
    /// string when `row` is out of range.
    pub fn row_text(&self, row: usize) -> String {
        self.snapshot()
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
        extract_snapshot_text(&self.snapshot(), anchor, cursor)
    }
}

/// Extract the text covered by a selection from a snapshot grid.
///
/// Split out from [`TerminalPane`] so it can be exercised against a synthetic snapshot: the pane's
/// own snapshot is now derived from its live screen and cannot be injected.
fn extract_snapshot_text(
    snapshot: &TerminalRenderSnapshot,
    anchor: (usize, usize),
    cursor: (usize, usize),
) -> String {
    let (start, end) = if anchor <= cursor {
        (anchor, cursor)
    } else {
        (cursor, anchor)
    };
    let lines: Vec<&str> = snapshot.text.lines().collect();
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

impl TerminalPane {
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

    /// The effective agent status for a detected-agent pane: the explicitly reported status if the
    /// integration published one, otherwise the server's inferred `working`/`idle`/`blocked` state.
    /// `None` when the pane is not a detected agent. Single source of truth shared by the sidebar
    /// and the "unseen finish" edge detector so they never disagree on what "working" means.
    pub fn agent_status(&self) -> Option<String> {
        let detected = self.detected_agent.as_ref()?;
        let value = self
            .reported_status
            .as_ref()
            .map(|status| status.value.as_str())
            .unwrap_or_else(|| match detected.state {
                crate::session::protocol::DetectedAgentState::Idle => {
                    crate::session::protocol::pane_status::IDLE
                }
                crate::session::protocol::DetectedAgentState::Working => {
                    crate::session::protocol::pane_status::WORKING
                }
                crate::session::protocol::DetectedAgentState::Blocked => {
                    crate::session::protocol::pane_status::BLOCKED
                }
            });
        Some(value.to_string())
    }

    /// How long this pane's active agent run has lasted, for the sidebar's duration column.
    ///
    /// The server-owned run timestamp survives detach/reattach and is deliberately preferred over
    /// the current status's `set_at`, because blocking and resuming are still one run. Wall-clock
    /// skew against a server on another host can only ever shorten the answer, never invent one,
    /// because the subtraction saturates at zero.
    pub fn status_age(&self) -> Option<std::time::Duration> {
        if self
            .agent_status()
            .as_deref()
            .is_some_and(|status| !status_is_quiescent(status))
            && let Some(started_at) = self.work_started_at
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            return Some(std::time::Duration::from_secs(
                now.saturating_sub(started_at),
            ));
        }
        if let Some(status) = self.reported_status.as_ref() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            return Some(std::time::Duration::from_secs(
                now.saturating_sub(status.set_at),
            ));
        }
        self.status_since.map(|since| since.elapsed())
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

/// Split a conventional shell title into its optional user and working-directory parts.
///
/// Accepted forms are `user@host:/path`, `user@host:~/path`, and a bare absolute/tilde/Windows
/// path. Matching is deliberately narrow so application titles that merely mention a path remain
/// application titles.
pub(crate) fn shell_title_parts(title: &str) -> Option<(Option<&str>, &str)> {
    let title = title.trim();
    let (user, rest) = match title.split_once(':') {
        Some((head, rest))
            if head
                .split_once('@')
                .is_some_and(|(user, host)| !user.is_empty() && !host.is_empty())
                && !head.contains(['/', '\\', ' ']) =>
        {
            (
                head.split_once('@').map(|(user, _)| user),
                rest.trim_start(),
            )
        }
        _ => (None, title),
    };
    let bare_path = !rest.contains(char::is_whitespace)
        && (rest.starts_with('/')
            || rest.starts_with('~')
            || crate::platform::paths::is_windows_path_shape(rest));
    bare_path.then_some((user, rest))
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
    fn shell_title_parser_is_narrow_and_preserves_the_user_and_cwd() {
        assert_eq!(
            shell_title_parts("razuer@host:~/Work/Projects/hyprmux"),
            Some((Some("razuer"), "~/Work/Projects/hyprmux"))
        );
        assert_eq!(shell_title_parts("/etc/nginx"), Some((None, "/etc/nginx")));
        assert_eq!(shell_title_parts("nvim ~/src/main.rs"), None);
        assert_eq!(shell_title_parts("make: *** [all] Error 1"), None);
    }

    #[test]
    fn fresh_terminal_learns_original_user_from_its_first_shell_title() {
        let mut pane = TerminalPane::new(100);
        pane.process_server_output(b"\x1b]2;razuer@host:~\x07");
        assert_eq!(pane.original_user.as_deref(), Some("razuer"));

        pane.process_server_output(b"\x1b]2;root@host:/etc\x07");
        assert_eq!(pane.original_user.as_deref(), Some("razuer"));
    }

    /// The snapshot is rebuilt on read, so every path that changes what the pane should display
    /// must leave the screen dirty. A write path that forgets would show stale content until
    /// something else happened to dirty the screen — the failure mode this whole design risks.
    #[test]
    fn every_write_path_is_reflected_without_an_explicit_rebuild() {
        let mut pane = TerminalPane::new(100);

        pane.process_server_output(b"first-line\r\n");
        assert!(
            pane.snapshot().text.contains("first-line"),
            "output must reach the snapshot"
        );

        // Reading twice with no intervening write is the cached path, and must agree.
        let sequence = pane.snapshot().sequence;
        assert_eq!(
            pane.snapshot().sequence,
            sequence,
            "cached read must be stable"
        );

        pane.process_server_output(b"second-line\r\n");
        let after = pane.snapshot();
        assert!(after.text.contains("second-line"));
        assert!(
            after.sequence > sequence,
            "new output must advance the snapshot sequence"
        );

        assert!(pane.apply_server_resize(40, 10));
        assert_eq!(pane.snapshot().color_lines.len(), 10, "resize must reshape");

        // Drive enough lines to build history, then scroll into it.
        for i in 0..40 {
            pane.process_server_output(format!("hist-{i}\r\n").as_bytes());
        }
        assert!(pane.set_scrollback(5));
        assert_eq!(
            pane.snapshot().scrollback_offset,
            5,
            "scrollback moves must reach the snapshot"
        );
    }

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
    fn status_age_uses_the_server_run_start_across_active_statuses() {
        let mut pane = TerminalPane::new(100);
        pane.detected_agent = Some(crate::session::protocol::DetectedAgent {
            kind: crate::session::protocol::AgentKind::OpenCode,
            state: crate::session::protocol::DetectedAgentState::Working,
        });
        pane.work_started_at = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                .saturating_sub(120),
        );
        pane.reported_status = Some(crate::session::protocol::PaneStatus {
            value: "blocked".into(),
            reason: None,
            set_at: 1,
        });

        assert!(pane.status_age().unwrap() >= std::time::Duration::from_secs(120));
        pane.reported_status.as_mut().unwrap().value = "working".into();
        assert!(pane.status_age().unwrap() >= std::time::Duration::from_secs(120));
    }

    #[test]
    fn extract_text_trims_and_joins_selected_rows() {
        let snapshot = TerminalRenderSnapshot {
            text: std::sync::Arc::from("hello world   \nfoo bar\nbaz"),
            ..TerminalRenderSnapshot::default()
        };

        // Single-line span is inclusive of the cursor cell and trims trailing space.
        assert_eq!(extract_snapshot_text(&snapshot, (0, 0), (0, 4)), "hello");
        // Multi-line span joins rows with newlines, trimming each line's trailing space.
        assert_eq!(
            extract_snapshot_text(&snapshot, (0, 0), (2, 2)),
            "hello world\nfoo bar\nbaz"
        );
        // Anchor/cursor order is normalized.
        assert_eq!(extract_snapshot_text(&snapshot, (0, 4), (0, 0)), "hello");
    }

    #[test]
    fn search_match_ranges_returns_each_case_insensitive_occurrence() {
        assert_eq!(
            search_match_ranges("Alpha beta alpha", "alpha"),
            vec![(0, 5), (11, 16)]
        );
    }

    #[test]
    fn search_scrollback_uses_text_export_without_mutating_offset() {
        let mut pane = TerminalPane::new(50);
        pane.apply_server_resize(10, 4);
        // Drive enough lines to push content into history.
        for i in 0..12 {
            pane.process_server_output(format!("line-{i}\r\n").as_bytes());
        }
        let offset_before = pane.scrollback_offset();
        let matches = pane.search_scrollback("line-1");
        assert!(!matches.is_empty());
        assert!(matches.iter().any(|m| m.text.contains("line-1")));
        assert_eq!(pane.scrollback_offset(), offset_before);

        let full = pane.capture_scrollback_text(None);
        assert!(full.contains("line-0"));
        assert!(full.contains("line-11"));
        let last_two = pane.capture_scrollback_text(Some(2));
        assert!(last_two.lines().count() <= 2);
    }

    #[test]
    fn search_highlighted_snapshot_marks_all_visible_matches() {
        let base_style = Style::new().fg(Color::Green);
        let highlight_style = Style::new().fg(Color::White).bg(Color::rgb(92, 64, 8));
        let active_highlight_style = Style::new().fg(Color::Black).bg(Color::Yellow).bold();
        let base = TerminalRenderSnapshot {
            text: std::sync::Arc::from("Alpha beta alpha"),
            color_lines: std::sync::Arc::from([vec![
                Span::new("Alpha beta alpha").style(base_style),
            ]]),
            ..TerminalRenderSnapshot::default()
        };

        let snapshot = search_highlighted_snapshot(
            base.clone(),
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
        let base_style = Style::new().fg(Color::Green);
        let highlight_style = Style::new().fg(Color::White).bg(Color::rgb(92, 64, 8));
        let active_highlight_style = Style::new().fg(Color::Black).bg(Color::Yellow).bold();
        let base = TerminalRenderSnapshot {
            text: std::sync::Arc::from("Alpha beta alpha"),
            color_lines: std::sync::Arc::from([vec![
                Span::new("Alpha beta alpha").style(base_style),
            ]]),
            ..TerminalRenderSnapshot::default()
        };

        let snapshot = search_highlighted_snapshot(
            base.clone(),
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
        let base_style = Style::new().fg(Color::Green);
        let match_style = Style::new().fg(Color::White).bg(Color::rgb(92, 64, 8));
        let label_style = Style::new().fg(Color::Black).bg(Color::Yellow).bold();
        let base = TerminalRenderSnapshot {
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

        let snapshot = hint_snapshot(
            base.clone(),
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
