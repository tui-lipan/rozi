use std::time::Instant;

use tui_lipan::prelude::FloatRect;

use crate::pane::TerminalPane;

use super::{PaneId, PaneIdentity};

pub struct Pane {
    pub id: PaneId,
    pub pty_generation: u64,
    pub title: String,
    pub identity: PaneIdentity,
    pub floating: bool,
    pub fullscreen: bool,
    pub floating_rect: FloatRect,
    pub opening: bool,
    pub terminal_active: bool,
    pub closing: bool,
    pub logging: bool,
    pub activity: PaneActivity,
    pub terminal: TerminalPane,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PaneActivity {
    pub last_activity: Option<Instant>,
    pub has_unseen_output: bool,
    pub bell: bool,
}

impl Pane {
    pub fn new(id: PaneId, scrollback: usize, floating_rect: FloatRect) -> Self {
        Self {
            id,
            pty_generation: 0,
            title: "shell".to_string(),
            identity: PaneIdentity::default(),
            floating: false,
            fullscreen: false,
            floating_rect,
            opening: true,
            terminal_active: false,
            closing: false,
            logging: false,
            activity: PaneActivity::default(),
            terminal: {
                let mut terminal = TerminalPane::new(scrollback);
                terminal.bind_session(id, 0);
                terminal
            },
        }
    }

    pub fn display_title(&self, terminal_title: Option<String>) -> String {
        self.identity
            .custom_title
            .clone()
            .or(terminal_title)
            .unwrap_or_else(|| self.title.clone())
    }

    pub fn set_custom_title(&mut self, title: impl AsRef<str>) {
        self.identity.set_custom_title(title);
    }

    pub fn clear_custom_title(&mut self) {
        self.identity.custom_title = None;
    }

    pub fn subtitle(&self) -> Option<&str> {
        self.identity
            .command
            .as_deref()
            .or(self.identity.cwd.as_deref())
    }

    pub fn subtitle_for_title(&self, title: &str) -> Option<String> {
        // A replay pane (profile-restored command typed into its interactive shell) is
        // behaviorally a shell pane: its live title/cwd describe it better than the launch
        // command, which the shell may have long finished.
        if !self.identity.replay
            && let Some(command) = self.identity.command.as_deref()
        {
            return Some(command.to_string());
        }

        // Prefer the shell's real live cwd (which the initial pane never captures into its launch
        // identity) and fall back to the configured launch cwd.
        let cwd = self.live_cwd().or_else(|| self.identity.cwd.clone())?;
        if title_contains_cwd(title, &cwd) {
            None
        } else {
            Some(cwd)
        }
    }

    /// The shell's current working directory if it can be discovered live, else `None`.
    pub fn live_cwd(&self) -> Option<String> {
        self.terminal.working_directory()
    }

    /// A local working directory safe to reuse for spawning, falling back to launch identity.
    pub fn local_cwd(&self) -> Option<String> {
        self.local_cwd_ref().map(str::to_string)
    }

    /// [`local_cwd`](Self::local_cwd) without the allocation, for callers that only compare it.
    /// The sidebar checks this on every message, including output from off-screen panes that
    /// otherwise cost nothing to handle, so that check must not allocate a string to throw away.
    pub fn local_cwd_ref(&self) -> Option<&str> {
        self.terminal
            .cwd
            .as_deref()
            .filter(|_| self.terminal.cwd_host.is_none())
            .or(self.identity.cwd.as_deref())
    }
}

fn title_contains_cwd(title: &str, cwd: &str) -> bool {
    if cwd.is_empty() || title.contains(cwd) {
        return !cwd.is_empty();
    }

    let Ok(home) = std::env::var("HOME") else {
        return false;
    };
    let home = home.trim_end_matches('/');
    if home.is_empty() || !cwd.starts_with(home) {
        return false;
    }

    let rest = cwd[home.len()..].trim_start_matches('/');
    let tilde_cwd = if rest.is_empty() {
        "~".to_string()
    } else {
        format!("~/{rest}")
    };
    title.contains(&tilde_cwd)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane() -> Pane {
        Pane::new(
            1,
            100,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        )
    }

    #[test]
    fn pane_display_title_prefers_custom_title() {
        let mut pane = pane();
        pane.title = "terminal title".to_string();
        pane.set_custom_title("custom title");

        assert_eq!(
            pane.display_title(Some("terminal title".to_string())),
            "custom title"
        );
    }

    #[test]
    fn pane_display_title_uses_terminal_title_before_fallback() {
        let mut pane = pane();
        pane.title = "fallback title".to_string();

        assert_eq!(
            pane.display_title(Some("terminal title".to_string())),
            "terminal title"
        );
    }

    #[test]
    fn empty_custom_title_is_cleared() {
        let mut pane = pane();
        pane.set_custom_title("custom title");
        pane.set_custom_title("   ");

        assert_eq!(pane.identity.custom_title, None);
        assert_eq!(pane.display_title(None), "shell");
    }

    #[test]
    fn pane_subtitle_prefers_command_before_cwd() {
        let mut pane = pane();
        pane.identity.cwd = Some("/tmp/project".to_string());
        pane.identity.command = Some("vim src/main.rs".to_string());

        assert_eq!(pane.subtitle(), Some("vim src/main.rs"));
    }

    #[test]
    fn replay_pane_subtitle_shows_cwd_not_the_replayed_command() {
        let mut pane = pane();
        pane.identity.cwd = Some("/tmp/project".to_string());
        pane.identity.command = Some("nvim".to_string());
        pane.identity.replay = true;

        // The replay command runs inside a live interactive shell; the pane's cwd (and live
        // title) describe it, not the launch command the shell may have long finished.
        assert_eq!(
            pane.subtitle_for_title("shell"),
            Some("/tmp/project".to_string())
        );
    }

    #[test]
    fn pane_subtitle_hides_cwd_already_in_terminal_title() {
        let mut pane = pane();
        pane.identity.cwd = Some("/tmp/project".to_string());

        assert_eq!(pane.subtitle_for_title("razuer@host:/tmp/project"), None);
    }

    #[test]
    fn pane_subtitle_hides_home_relative_cwd_in_terminal_title() {
        let Ok(home) = std::env::var("HOME") else {
            return;
        };
        let home = home.trim_end_matches('/');
        if home.is_empty() {
            return;
        }

        let mut pane = pane();
        pane.identity.cwd = Some(format!("{home}/Work/Projects/opencode-tui"));

        assert_eq!(
            pane.subtitle_for_title("razuer@host:~/Work/Projects/opencode-tui"),
            None
        );
    }

    #[test]
    fn pane_subtitle_keeps_command_even_when_title_contains_cwd() {
        let mut pane = pane();
        pane.identity.cwd = Some("/tmp/project".to_string());
        pane.identity.command = Some("cargo run".to_string());

        assert_eq!(
            pane.subtitle_for_title("razuer@host:/tmp/project"),
            Some("cargo run".to_string())
        );
    }
}
