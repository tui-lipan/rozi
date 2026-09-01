//! Panes: the client-side terminal widget and the machinery that spawns, feeds, and retires it.
//!
//! [`TerminalPane`] here is the screen a client draws; [`state::pane`](crate::state::pane) holds the
//! per-pane app state and `view/pane.rs` renders it.

pub mod launch;
pub mod lifecycle;
pub mod pty_events;
pub mod rules;

use std::cell::RefCell;
use std::ops::ControlFlow;
use std::rc::Rc;
use std::sync::Arc;

use tui_lipan::prelude::*;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Decoded Kitty graphics retained by one pane in one client.
///
/// `tui-lipan` defaults to 96 MiB per screen, which is appropriate for a standalone terminal but
/// multiplies by both pane count and attached-client count in Rozi. Thirty-two MiB keeps several
/// full-screen plots while bounding an eight-pane attachment to 256 MiB of decoded pixels.
const CLIENT_IMAGE_BUDGET_BYTES: usize = 32 * 1024 * 1024;

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
    /// Absolute path of that same program, sent only when its name would not resolve on the
    /// session server's `PATH` (a shell alias, a build-tree binary). Profile capture replays this
    /// instead of the name, which is all a restored pane could otherwise fail to find.
    pub foreground_executable: Option<String>,
    /// Arguments that program was launched with, `argv[0]` excluded, as read by the session
    /// server. Empty on platforms that cannot read another process's arguments.
    pub foreground_arguments: Vec<String>,
    /// Free-form status reported by the pane through the session server. This is distinct from
    /// `status`, which tracks whether the client-side terminal parser is ready or exited.
    pub reported_status: Option<crate::session::protocol::PaneStatus>,
    pub detected_agent: Option<crate::session::protocol::DetectedAgent>,
    /// Set when this agent pane's effective status transitions from `working` to a quiescent state
    /// (finished), and cleared once it is attended (its host window and the pane are both focused).
    /// Drives the sidebar's "unseen finish" pulse so a completed run does not blend into panes that
    /// were idle all along. Never set for `blocked`, which already has its own attention glyph.
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
    /// Logical agents or activities this pane's program published for itself; empty for every other pane.
    pub published_rows: Vec<crate::session::protocol::PublishedRow>,
    /// Per-row presentation state the server does not own, keyed by row id. Entries for ids the
    /// publisher has dropped are pruned whenever rows are applied.
    pub published_row_ui: std::collections::HashMap<String, PublishedRowUiState>,
    pub command_phase: crate::session::protocol::PaneCommandPhase,
    pub last_exit_status: Option<i32>,
    pub runtime_sequence: u64,
    pub last_palette: Option<TerminalColorPalette>,
    /// Which out-of-band graphics media this pane's screen will read. It follows the attachment
    /// feeding the pane rather than the pane itself: pixels named as a path only exist for a
    /// client on the machine that wrote them, so a `--remote` attachment allows none of it and a
    /// path arriving anyway must not be opened here.
    media_policy: GraphicsMediaPolicy,
    image_budget_bytes: usize,
    seen_bell_count: u64,
    scrollback_limit: usize,
    /// Holds forwarded pointer motion to one position in flight at a time. See
    /// [`crate::pane::pty_events::pointer_flow`].
    pub(crate) pointer_flow: crate::pane::pty_events::pointer_flow::PointerFlow,
    /// Behind a `RefCell` so [`TerminalPane::snapshot`] can rebuild through a shared reference:
    /// the render snapshot is pulled by the view (which only ever holds `&State`), and rebuilding
    /// it at read time rather than at write time is what collapses a burst of output messages into
    /// one rebuild. See [`TerminalPane::process_server_output`].
    /// Shared rather than owned so the view can hand the widget a [`TerminalScreenHandle`] instead
    /// of a snapshot: with the screen out of the element tree, output repaints instead of rebuilding.
    screen: Rc<RefCell<TerminalScreen>>,
}

/// What a chunk of pane output needs from the next frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFrame {
    /// Only the screen moved. The view hands the widget the screen itself, so the new contents reach
    /// the buffer without `view()` running again.
    Repaint,
    /// Metadata the chrome around the screen renders moved too - an OSC title, or the parser
    /// reporting itself ready for the first time - so the view has to run.
    Rebuild,
}

pub struct ProcessedOutput {
    pub frame: OutputFrame,
    pub clipboard_events: Vec<TerminalClipboardEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalSearchMatch {
    pub offset: usize,
    pub line: usize,
    /// Display-column range in the visible terminal grid.
    pub start_col: usize,
    pub end_col: usize,
    pub text: Arc<str>,
}

pub struct TerminalSearchResults {
    pub matches: Vec<TerminalSearchMatch>,
    pub truncated: bool,
}

fn display_col_at(text: &str, byte_index: usize) -> usize {
    text[..byte_index]
        .graphemes(true)
        .map(|grapheme| {
            if grapheme.chars().all(char::is_control) {
                0
            } else {
                UnicodeWidthStr::width(grapheme)
            }
        })
        .sum()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSearchHighlight {
    pub line: usize,
    /// Display-column range in the visible terminal grid.
    pub start_col: usize,
    pub end_col: usize,
}

fn status_is_quiescent(value: &str) -> bool {
    let value = value.trim();
    value.eq_ignore_ascii_case(crate::session::protocol::pane_status::IDLE)
        || value.eq_ignore_ascii_case(crate::session::protocol::pane_status::DONE)
}

fn status_is_blocked(value: &str) -> bool {
    value
        .trim()
        .eq_ignore_ascii_case(crate::session::protocol::pane_status::BLOCKED)
}

/// Elapsed wall-clock time since a server-stamped run start, saturating at zero so clock skew
/// against another host can only shorten the answer, never invent one.
fn row_run_age(started_at: Option<u64>) -> Option<std::time::Duration> {
    let started_at = started_at?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Some(std::time::Duration::from_secs(
        now.saturating_sub(started_at),
    ))
}

fn previous_started(rows: &[crate::session::protocol::PublishedRow], id: &str) -> Option<u64> {
    rows.iter()
        .find(|row| row.id == id)
        .and_then(|row| row.work_started_at)
}

/// Presentation state for one published row: the parts of a row the server has no opinion about.
///
/// Kept beside the rows rather than inside them because the server owns what a row *is* and the
/// client owns what this viewer has seen of it. Two clients watching one session disagree about
/// which finishes they have looked at, and that disagreement is correct.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PublishedRowUiState {
    /// This row finished a run that has not been looked at. Cleared by attending the row, which
    /// means the pane is focused *and* the publisher has this row on screen.
    pub finished_unseen: bool,
    /// How long this row's last run lasted, captured as it ended.
    pub last_run: Option<std::time::Duration>,
}

impl TerminalPane {
    pub fn new(scrollback: usize) -> Self {
        let cols = 120;
        let rows = 32;
        let mut screen = TerminalScreen::new(rows, cols, scrollback);
        // Images the child draws are sized in cells against this. The same value rides to the
        // server with every resize, so the PTY reports it to the child and both ends agree on how
        // many rows a picture takes.
        screen.set_cell_size(tui_lipan::host_cell_size());
        screen.set_image_media_policy(GraphicsMediaPolicy::SHARED);
        screen.set_image_budget(CLIENT_IMAGE_BUDGET_BYTES);
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
            foreground_executable: None,
            foreground_arguments: Vec::new(),
            reported_status: None,
            detected_agent: None,
            finished_unseen: false,
            work_started_at: None,
            status_since: None,
            last_run: None,
            published_rows: Vec::new(),
            published_row_ui: std::collections::HashMap::new(),
            command_phase: crate::session::protocol::PaneCommandPhase::Unknown,
            last_exit_status: None,
            runtime_sequence: 0,
            last_palette: None,
            media_policy: GraphicsMediaPolicy::SHARED,
            image_budget_bytes: CLIENT_IMAGE_BUDGET_BYTES,
            seen_bell_count: 0,
            scrollback_limit: scrollback,
            pointer_flow: crate::pane::pty_events::pointer_flow::PointerFlow::default(),
            screen: Rc::new(RefCell::new(screen)),
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

    /// The screen itself, for the `Terminal` widget to read at paint time.
    ///
    /// Preferred over [`snapshot`](Self::snapshot) in the view: a snapshot in the element makes the
    /// element change on every chunk of output, which forces a full `view()` + layout pass for the
    /// whole window. A handle holds still, so output is answered with [`Update::paint`].
    ///
    /// [`Update::paint`]: tui_lipan::prelude::Update::paint
    pub fn screen_handle(&self) -> TerminalScreenHandle {
        TerminalScreenHandle::new(Rc::clone(&self.screen))
    }

    /// Whether this pane currently retains Kitty graphics.
    ///
    /// Process name is the wrong signal: any child that speaks the protocol looks the same
    /// here. Close animation shrinks and fades the widget; Ghostty's image layer does not,
    /// so these panes are the ones that can look wrong on the way out.
    pub fn has_images(&self) -> bool {
        self.screen.borrow().has_images()
    }

    #[cfg(test)]
    fn set_image_budget(&mut self, bytes: usize) {
        self.image_budget_bytes = bytes;
        self.screen.borrow_mut().set_image_budget(bytes);
    }

    pub fn bind_session(&mut self, pane_id: crate::state::PaneId, generation: u64) {
        if self.pane_id != pane_id || self.generation != generation {
            // A position held for the process that just went away belongs to nothing: the pointer
            // was over the old pane's content, and the new one has never been pointed at.
            self.pointer_flow.reset();
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
            self.published_rows.clear();
            self.published_row_ui.clear();
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
        *screen = TerminalScreen::new(self.rows, self.cols, self.scrollback_limit);
        screen.set_cell_size(tui_lipan::host_cell_size());
        screen.set_image_media_policy(self.media_policy);
        screen.set_image_budget(self.image_budget_bytes);
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
    pub fn process_server_output(&mut self, bytes: &[u8]) -> ProcessedOutput {
        let mut screen = self.screen.borrow_mut();
        screen.process_bytes(bytes);
        let _ = screen.drain_responses();
        let clipboard_events = screen.drain_clipboard_events();
        // Runtime metadata comes from the server. Keep the screen's bounded semantic marks, but do
        // not retain a second, unbounded client-side copy of every OSC 7/133 event.
        let _ = screen.drain_semantic_events();
        let title = screen.title().and_then(sanitize_terminal_title);
        drop(screen);
        if self.original_user.is_none() {
            self.original_user = title
                .as_deref()
                .and_then(shell_title_parts)
                .and_then(|(user, _)| user.map(str::to_string));
        }
        // The titlebar renders both of these, so a chunk that moves either has to be answered with a
        // frame that runs the view - unlike the screen itself, which the widget reads for itself.
        let chrome_changed =
            self.title != title || !matches!(self.status, ManagedTerminalStatus::Ready);
        self.title = title;
        self.status = ManagedTerminalStatus::Ready;
        let frame = if chrome_changed {
            OutputFrame::Rebuild
        } else {
            OutputFrame::Repaint
        };
        ProcessedOutput {
            frame,
            clipboard_events,
        }
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

    pub fn set_media_policy(&mut self, policy: GraphicsMediaPolicy) {
        if self.media_policy == policy {
            return;
        }
        self.media_policy = policy;
        self.screen.borrow_mut().set_image_media_policy(policy);
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

    pub fn search_scrollback(&self, query: &str) -> Vec<TerminalSearchMatch> {
        self.search_scrollback_range(query, 0, usize::MAX, usize::MAX)
            .matches
    }

    /// Number of retained text lines available to a range-bounded search.
    pub fn search_line_count(&self) -> usize {
        self.screen.borrow().total_text_lines()
    }

    /// Search the retained half-open line range `[start, end)`.
    ///
    /// `max_matches` bounds retained results while still probing for one additional valid match,
    /// so a zero cap can distinguish "no later match" from truncation without retaining anything.
    pub fn search_scrollback_range(
        &self,
        query: &str,
        start: usize,
        end: usize,
        max_matches: usize,
    ) -> TerminalSearchResults {
        let query = query.trim();
        if query.is_empty() {
            return TerminalSearchResults {
                matches: Vec::new(),
                truncated: false,
            };
        }

        let needle = query.to_ascii_lowercase();
        let screen = self.screen.borrow();
        let total = screen.total_text_lines();
        let mut matches = Vec::new();
        let mut truncated = false;
        let _ = screen.try_for_each_text_line(start, end.min(total), |absolute, text| {
            let haystack = text.to_ascii_lowercase();
            let mut search_from = 0usize;
            let mut viewport = None;
            let mut shared_text = None;
            while search_from < haystack.len() {
                let Some(relative_start) = haystack[search_from..].find(&needle) else {
                    break;
                };
                let start = search_from + relative_start;
                let end = start + needle.len();
                let Some((offset, line)) =
                    *viewport.get_or_insert_with(|| screen.absolute_line_to_viewport(absolute))
                else {
                    break;
                };
                let start_col = display_col_at(text, start);
                let end_col = display_col_at(text, end);
                if start_col < end_col {
                    if matches.len() == max_matches {
                        truncated = true;
                        return ControlFlow::Break(());
                    }
                    let text = Arc::clone(shared_text.get_or_insert_with(|| Arc::from(text)));
                    matches.push(TerminalSearchMatch {
                        offset,
                        line,
                        start_col,
                        end_col,
                        text,
                    });
                }
                search_from = end;
            }
            ControlFlow::Continue(())
        });
        TerminalSearchResults { matches, truncated }
    }
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

    /// Extract selected text across retained scrollback using display columns.
    pub fn selection_display_text(
        &self,
        sel: &TerminalSelection,
        endpoint: SelectionEnd,
        trim_row_end: bool,
    ) -> String {
        self.screen
            .borrow()
            .selection_display_text(sel, endpoint, trim_row_end)
    }
}

impl TerminalPane {
    /// Mark the pane as exited locally. The server owns the PTY and is asked to kill it via a
    /// separate `Kill` RPC (see `close_pane`).
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

    /// The effective agent status for a detected-agent pane. An explicit active status wins, while
    /// a detected blocked prompt elevates over a stale quiescent `idle`/`done` report.
    /// `None` when the pane is not a detected agent. Single source of truth shared by the sidebar
    /// and the "unseen finish" edge detector so they never disagree on what "working" means. See
    /// [`Self::is_blocked`] and [`Self::is_working`] for reported-only status predicates.
    pub fn agent_status(&self) -> Option<String> {
        let detected = self.detected_agent.as_ref()?;
        let value = crate::session::protocol::effective_agent_status(
            self.reported_status.as_ref(),
            Some(detected),
        )?;
        Some(value.to_string())
    }

    /// Whether this pane's agent is waiting on the user.
    ///
    /// Uses the shared reported/detected authority rule from [`Self::agent_status`].
    /// Unlike `agent_status`, a reported-only pane can be blocked.
    pub fn is_blocked(&self) -> bool {
        crate::session::protocol::effective_agent_status(
            self.reported_status.as_ref(),
            self.detected_agent.as_ref(),
        )
        .is_some_and(|status| {
            status
                .trim()
                .eq_ignore_ascii_case(crate::session::protocol::pane_status::BLOCKED)
        })
    }

    /// Whether this pane's agent is actively working, under the same shared authority rule as
    /// [`Self::is_blocked`].
    pub fn is_working(&self) -> bool {
        crate::session::protocol::effective_agent_status(
            self.reported_status.as_ref(),
            self.detected_agent.as_ref(),
        )
        .is_some_and(|status| {
            status
                .trim()
                .eq_ignore_ascii_case(crate::session::protocol::pane_status::WORKING)
        })
    }

    /// How long this pane's active agent run has lasted, for the sidebar's duration column.
    ///
    /// Replace the pane's published rows, recording each row's finish edge.
    ///
    /// Returns the ids of rows that just finished a run. Their alerts are raised per row rather
    /// than per pane: a background tab finishing is news even while the pane is focused, because
    /// focusing the pane only ever showed the row the publisher had on screen.
    pub fn apply_rows(&mut self, rows: Vec<crate::session::protocol::PublishedRow>) -> Vec<String> {
        let mut finished = Vec::new();
        for row in &rows {
            let previous = self
                .published_rows
                .iter()
                .find(|candidate| candidate.id == row.id)
                .map(|candidate| candidate.status.as_str());
            let was_working = previous.is_some_and(|status| {
                status
                    .trim()
                    .eq_ignore_ascii_case(crate::session::protocol::pane_status::WORKING)
            });
            let now_working = row
                .status
                .trim()
                .eq_ignore_ascii_case(crate::session::protocol::pane_status::WORKING);
            let entry = self.published_row_ui.entry(row.id.clone()).or_default();
            if now_working {
                entry.finished_unseen = false;
            } else if was_working && !status_is_blocked(&row.status) {
                entry.last_run = row_run_age(previous_started(&self.published_rows, &row.id));
                entry.finished_unseen = true;
                finished.push(row.id.clone());
            }
        }
        // A publisher that closed a tab should not leave its row's pulse behind.
        self.published_row_ui
            .retain(|id, _| rows.iter().any(|s| &s.id == id));
        self.published_rows = rows;
        finished
    }

    /// How long a row's current run has lasted, by the same rule as [`Self::status_age`].
    pub fn row_age(
        &self,
        row: &crate::session::protocol::PublishedRow,
    ) -> Option<std::time::Duration> {
        (!status_is_quiescent(&row.status))
            .then(|| row_run_age(row.work_started_at))
            .flatten()
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

#[cfg(test)]
mod tests {
    use super::*;
    use tui_lipan::utils::{GridPos, GridSelection};

    #[test]
    fn shell_title_parser_is_narrow_and_preserves_the_user_and_cwd() {
        assert_eq!(
            shell_title_parts("razuer@host:~/Work/Projects/rozi"),
            Some((Some("razuer"), "~/Work/Projects/rozi"))
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

    #[test]
    fn has_images_follows_kitty_graphics_not_the_foreground_program() {
        let mut pane = TerminalPane::new(100);
        assert!(!pane.has_images());

        // 1x1 RGB pixel; "gICA" is `[0x80, 0x80, 0x80]` in standard base64.
        pane.process_server_output(b"\x1b_Ga=T,f=24,s=1,v=1,t=d,i=1,C=1;gICA\x1b\\");
        assert!(pane.has_images());

        pane.process_server_output(b"\x1b_Ga=d,d=A\x1b\\");
        assert!(!pane.has_images());
    }

    #[test]
    fn client_image_budget_survives_server_backend_rebinds() {
        assert_eq!(CLIENT_IMAGE_BUDGET_BYTES, 32 * 1024 * 1024);
        let mut pane = TerminalPane::new(100);
        pane.set_image_budget(4);

        let overflow_budget = |pane: &mut TerminalPane| {
            pane.process_server_output(b"\x1b_Ga=T,f=24,s=1,v=1,t=d,i=1,C=1;gICA\x1b\\");
            pane.process_server_output(b"\x1b_Ga=T,f=24,s=1,v=1,t=d,i=2,C=1;gICA\x1b\\");
            pane.process_server_output(b"\x1b_Ga=d,d=I,i=2;\x1b\\");
            assert!(
                !pane.has_images(),
                "the first image must have been evicted before the second was deleted"
            );
        };
        overflow_budget(&mut pane);

        pane.bind_server_backend(1, 2);
        overflow_budget(&mut pane);
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
    fn client_discards_semantic_events_after_recording_marks() {
        let mut pane = TerminalPane::new(100);
        pane.process_server_output(b"\x1b]133;A\x07");

        assert!(pane.screen.borrow_mut().drain_semantic_events().is_empty());
        assert!(!pane.semantic_marks().is_empty());
    }

    #[test]
    fn client_returns_child_osc52_store_requests() {
        let mut pane = TerminalPane::new(100);

        let output = pane.process_server_output(b"\x1b]52;c;aGVsbG8=\x07");

        assert_eq!(
            output.clipboard_events,
            vec![TerminalClipboardEvent {
                target: TerminalClipboardTarget::Clipboard,
                text: "hello".to_string(),
            }]
        );
    }

    #[test]
    fn binding_server_backend_preserves_configured_scrollback_limit() {
        let mut pane = TerminalPane::new(3);
        pane.bind_server_backend(1, 1);
        pane.apply_server_resize(20, 5);
        for i in 0..20 {
            pane.process_server_output(format!("line-{i}\r\n").as_bytes());
        }

        assert!(pane.screen.borrow_mut().total_text_lines() <= 8);
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
            agent: crate::session::protocol::AgentIdentity::new("opencode", "OpenCode").into(),
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
    fn agent_alert_predicates_prefer_reported_status_and_accept_each_source() {
        let mut pane = TerminalPane::new(100);
        pane.detected_agent = Some(crate::session::protocol::DetectedAgent {
            agent: crate::session::protocol::AgentIdentity::new("opencode", "OpenCode").into(),
            state: crate::session::protocol::DetectedAgentState::Blocked,
        });
        assert!(pane.is_blocked());
        assert!(!pane.is_working());

        pane.detected_agent.as_mut().unwrap().state =
            crate::session::protocol::DetectedAgentState::Working;
        assert!(!pane.is_blocked());
        assert!(pane.is_working());

        pane.detected_agent.as_mut().unwrap().state =
            crate::session::protocol::DetectedAgentState::Blocked;
        pane.reported_status = Some(crate::session::protocol::PaneStatus {
            value: " Working ".into(),
            reason: None,
            set_at: 1,
        });
        assert!(!pane.is_blocked());
        assert!(pane.is_working());

        for value in ["idle", "done"] {
            pane.reported_status.as_mut().unwrap().value = value.into();
            assert!(pane.is_blocked(), "detected prompt elevates stale {value}");
            assert!(!pane.is_working());
            assert_eq!(pane.agent_status().as_deref(), Some("blocked"));
        }

        pane.detected_agent = None;
        pane.reported_status.as_mut().unwrap().value = "BLOCKED".into();
        assert!(pane.is_blocked());
        assert!(!pane.is_working());
    }

    #[test]
    fn selection_text_uses_display_columns_and_trims_rows() {
        let snapshot = TerminalRenderSnapshot {
            color_lines: std::sync::Arc::from([
                vec![Span::new("hello world   ")],
                vec![Span::new("foo bar")],
                vec![Span::new("baz")],
            ]),
            ..TerminalRenderSnapshot::default()
        };
        let selection = GridSelection {
            anchor: GridPos { row: 0, col: 0 },
            cursor: GridPos { row: 2, col: 2 },
        };

        // Single-line span is inclusive of the cursor cell and trims trailing space.
        assert_eq!(
            snapshot.selection_text(
                &GridSelection {
                    anchor: GridPos { row: 0, col: 0 },
                    cursor: GridPos { row: 0, col: 4 },
                },
                SelectionEnd::Inclusive,
                true,
            ),
            "hello"
        );
        // Multi-line span joins rows with newlines, trimming each line's trailing space.
        assert_eq!(
            snapshot.selection_text(&selection, SelectionEnd::Inclusive, true),
            "hello world\nfoo bar\nbaz"
        );
        // Anchor/cursor order is normalized.
        assert_eq!(
            snapshot.selection_text(
                &GridSelection {
                    anchor: GridPos { row: 0, col: 4 },
                    cursor: GridPos { row: 0, col: 0 },
                },
                SelectionEnd::Inclusive,
                true,
            ),
            "hello"
        );
    }

    #[test]
    fn streaming_search_matches_owned_export_without_mutating_offset() {
        let mut pane = TerminalPane::new(50);
        pane.apply_server_resize(10, 4);
        // Drive enough lines to push content into history.
        for i in 0..12 {
            pane.process_server_output(format!("line-{i}\r\n").as_bytes());
        }
        let offset_before = pane.scrollback_offset();
        let matches = pane.search_scrollback("line-1");
        let expected = {
            let screen = pane.screen.borrow();
            let total = screen.total_text_lines();
            let needle = "line-1".to_ascii_lowercase();
            let mut expected = Vec::new();
            for (absolute, text) in screen.text_lines(0, total).into_iter().enumerate() {
                let Some((offset, line)) = screen.absolute_line_to_viewport(absolute) else {
                    continue;
                };
                let haystack = text.to_ascii_lowercase();
                let mut search_from = 0;
                while let Some(relative) = haystack[search_from..].find(&needle) {
                    let start = search_from + relative;
                    let end = start + needle.len();
                    let spans = [Span::new(text.as_str())];
                    let start_col = tui_lipan::utils::spans::char_col_to_display_col(
                        &spans,
                        text[..start].chars().count(),
                    );
                    let end_col = tui_lipan::utils::spans::char_col_to_display_col(
                        &spans,
                        text[..end].chars().count(),
                    );
                    if start_col < end_col {
                        expected.push((offset, line, start_col, end_col, text.clone()));
                    }
                    search_from = end;
                }
            }
            expected
        };
        assert!(!matches.is_empty());
        assert!(matches.iter().any(|m| m.text.contains("line-1")));
        assert_eq!(
            matches
                .iter()
                .map(|matched| (
                    matched.offset,
                    matched.line,
                    matched.start_col,
                    matched.end_col,
                    matched.text.to_string(),
                ))
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(pane.scrollback_offset(), offset_before);

        let full = pane.capture_scrollback_text(None);
        assert!(full.contains("line-0"));
        assert!(full.contains("line-11"));
        let last_two = pane.capture_scrollback_text(Some(2));
        assert!(last_two.lines().count() <= 2);

        let mut wide = TerminalPane::new(20);
        wide.apply_server_resize(20, 2);
        wide.process_server_output("你 alpha\r\n".as_bytes());
        assert!(
            wide.search_scrollback("alpha")
                .iter()
                .any(|matched| matched.start_col == 3 && matched.end_col == 8)
        );
    }

    #[test]
    fn repeated_hits_share_one_line_allocation() {
        let mut pane = TerminalPane::new(10);
        pane.apply_server_resize(40, 2);
        pane.process_server_output(b"hit HIT hit\r\n");

        let matches = pane.search_scrollback("hit");

        assert_eq!(matches.len(), 3);
        assert!(Arc::ptr_eq(&matches[0].text, &matches[1].text));
        assert!(Arc::ptr_eq(&matches[1].text, &matches[2].text));
        assert_eq!(
            matches
                .iter()
                .map(|matched| (matched.start_col, matched.end_col))
                .collect::<Vec<_>>(),
            [(0, 3), (4, 7), (8, 11)]
        );
    }

    #[test]
    fn range_partitioning_matches_full_search_and_zero_cap_probes_extra_hits() {
        let mut pane = TerminalPane::new(50);
        pane.apply_server_resize(24, 4);
        pane.process_server_output(
            b"zero needle needle\r\none\r\ntwo needle\r\nthree\r\nfour needle\r\n",
        );
        let total = pane.search_line_count();
        let full = pane.search_scrollback("needle");
        let mut partitioned = Vec::new();
        for (start, end) in [(0, 1), (1, 3), (3, total)] {
            let result = pane.search_scrollback_range("needle", start, end, usize::MAX);
            assert!(!result.truncated);
            partitioned.extend(result.matches);
        }
        assert_eq!(partitioned, full);

        let with_hit = pane.search_scrollback_range("needle", 0, 1, 0);
        assert!(with_hit.matches.is_empty());
        assert!(with_hit.truncated);
        let without_hit = pane.search_scrollback_range("needle", 1, 2, 0);
        assert!(without_hit.matches.is_empty());
        assert!(!without_hit.truncated);
    }

    #[test]
    fn search_folds_ascii_only_and_keeps_non_ascii_case_sensitive() {
        let mut pane = TerminalPane::new(10);
        pane.apply_server_resize(40, 2);
        pane.process_server_output("Äbc äBC ABC abc\r\n".as_bytes());

        let ascii = pane.search_scrollback("aBc");
        assert_eq!(ascii.len(), 2);
        assert_eq!(
            ascii
                .iter()
                .map(|matched| matched.start_col)
                .collect::<Vec<_>>(),
            [8, 12]
        );

        let upper_non_ascii = pane.search_scrollback("ÄBC");
        assert_eq!(upper_non_ascii.len(), 1);
        assert_eq!(upper_non_ascii[0].start_col, 0);
        let lower_non_ascii = pane.search_scrollback("äbc");
        assert_eq!(lower_non_ascii.len(), 1);
        assert_eq!(lower_non_ascii[0].start_col, 4);
    }

    #[test]
    fn search_columns_preserve_combining_wide_and_control_widths_without_allocating() {
        let raw = "a\u{301}你\u{7}z";
        let wide_end = raw.find('你').expect("wide character") + '你'.len_utf8();
        let control_end = raw.find('\u{7}').expect("control") + 1;
        assert_eq!(display_col_at(raw, "a\u{301}".len()), 1);
        assert_eq!(display_col_at(raw, wide_end), 3);
        assert_eq!(display_col_at(raw, control_end), 3);
        assert_eq!(display_col_at(raw, raw.len()), 4);

        let mut pane = TerminalPane::new(10);
        pane.apply_server_resize(40, 2);
        pane.process_server_output("a\u{301}你\u{7}needle\r\n".as_bytes());
        let matches = pane.search_scrollback("needle");
        assert_eq!(matches.len(), 1);
        assert_eq!((matches[0].start_col, matches[0].end_col), (3, 9));
    }

    #[test]
    fn selection_display_text_spans_history_and_uses_display_columns() {
        let mut pane = TerminalPane::new(20);
        pane.apply_server_resize(20, 2);
        pane.process_server_output("a你b\r\n".as_bytes());
        pane.process_server_output("line-2\r\n".as_bytes());
        pane.process_server_output("line-3\r\n".as_bytes());

        let wide = TerminalSelection {
            anchor: TerminalPos { line: 0, col: 0 },
            cursor: TerminalPos { line: 0, col: 2 },
        };
        assert_eq!(
            pane.selection_display_text(&wide, SelectionEnd::Inclusive, true),
            "a你"
        );

        let spanning = TerminalSelection {
            anchor: TerminalPos { line: 1, col: 0 },
            cursor: TerminalPos { line: 2, col: 5 },
        };
        assert_eq!(
            pane.selection_display_text(&spanning, SelectionEnd::Inclusive, true),
            "line-2\nline-3"
        );
    }

    /// A pane decodes the kitty graphics the child writes, and sizes the result against the host
    /// cell rather than a guess - the wiring the server's PTY reports to the child in pixels.
    #[test]
    fn kitty_graphics_from_the_child_become_a_placement() {
        let mut pane = TerminalPane::new(100);
        pane.apply_server_resize(40, 20);
        let cell = tui_lipan::host_cell_size();
        assert_eq!(pane.screen.borrow().cell_size(), cell);

        // Two cells wide by three tall, in whatever the host's cell happens to be. All-zero RGB
        // keeps the base64 trivial (`A` per 6 bits) so the test needs no encoder.
        let (width, height) = (u32::from(cell.width) * 2, u32::from(cell.height) * 3);
        let payload = "A".repeat((width * height * 4) as usize);
        pane.process_server_output(
            format!("\x1b_Ga=T,f=24,s={width},v={height},t=d,i=1;{payload}\x1b\\").as_bytes(),
        );

        let snapshot = pane.screen.borrow_mut().render_snapshot();
        assert_eq!(snapshot.images.len(), 1);
        assert_eq!((snapshot.images[0].rows, snapshot.images[0].cols), (3, 2));
        // The escape is consumed, not painted.
        assert!(!snapshot.text.contains("_Ga="));
    }
}
