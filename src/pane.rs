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

    pub fn kill(&mut self) {
        if let Some(pty) = self.pty.take() {
            let _ = pty.kill();
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
