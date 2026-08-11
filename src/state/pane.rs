use std::time::Instant;

use tui_lipan::prelude::{FloatRect, ManagedTerminalStatus};

use crate::pane::{TerminalPane, shell_title_parts};

use super::{DEFAULT_SCROLLABLE_WIDTH, PaneId, PaneIdentity};

pub struct Pane {
    pub id: PaneId,
    pub pty_generation: u64,
    pub title: String,
    pub identity: PaneIdentity,
    pub floating: bool,
    pub fullscreen: bool,
    pub floating_rect: FloatRect,
    /// Scrollable layout column width as a fraction of the tile viewport.
    pub scrollable_width: f32,
    pub opening: bool,
    pub terminal_active: bool,
    /// Removed from the layout but still described, so its close animation can run.
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
            scrollable_width: DEFAULT_SCROLLABLE_WIDTH,
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

    /// Titlebar text in user name → application title → contextual cwd → generic fallback order.
    /// A custom/application primary title is qualified with the project-relative cwd when known.
    /// Conventional shell `user@host:cwd` titles count as cwd metadata rather than application
    /// titles; their hostname is omitted, while a user changed after shell launch remains visible.
    pub fn titlebar_title(&self, remote_attached: bool) -> String {
        let terminal_title = self.terminal.title();
        let shell_title = terminal_title.as_deref().and_then(shell_title_parts);
        let primary = self.identity.custom_title.clone().or_else(|| {
            shell_title
                .is_none()
                .then(|| terminal_title.clone())
                .flatten()
        });

        let path = self
            .terminal
            .display_path
            .clone()
            .or_else(|| shell_title.map(|(_, cwd)| cwd.to_string()))
            .or_else(|| self.live_cwd())
            .or_else(|| self.identity.cwd.clone())
            .map(|cwd| {
                if remote_attached || self.terminal.display_path.is_some() {
                    cwd
                } else {
                    crate::platform::paths::compress_home(&cwd)
                }
            });
        let switched_user = shell_title.and_then(|(user, _)| user).filter(|user| {
            self.terminal
                .original_user
                .as_deref()
                .is_some_and(|original| original != *user)
        });
        let location = path.map(|path| match switched_user {
            Some(user) => format!("{user} · {path}"),
            None => path,
        });

        match (primary, location) {
            (Some(primary), Some(location)) if primary != location => {
                format!("{primary} · {location}")
            }
            (Some(primary), _) => primary,
            (None, Some(location)) => location,
            (None, None) => self.title.clone(),
        }
    }

    pub fn set_custom_title(&mut self, title: impl AsRef<str>) {
        self.identity.set_custom_title(title);
    }

    pub fn clear_custom_title(&mut self) {
        self.identity.custom_title = None;
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

    /// The working directory as the *session server* sees it, without the `cwd_host` filter.
    ///
    /// Under `--remote` every reported path is server-relative, so the filter that keeps
    /// [`local_cwd_ref`](Self::local_cwd_ref) honest would discard exactly the path the remote file
    /// tree needs. Callers must already know the server is remote.
    pub fn server_cwd_ref(&self) -> Option<&str> {
        self.terminal
            .cwd
            .as_deref()
            .or(self.identity.cwd.as_deref())
    }

    /// Whether this pane is blocked and still worth acting on.
    pub fn awaits_input(&self) -> bool {
        !self.closing
            && !matches!(self.terminal.status, ManagedTerminalStatus::Exited(_))
            && self.terminal.is_blocked()
    }
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
    fn titlebar_uses_cwd_without_the_original_user_or_host() {
        let mut pane = pane();
        pane.terminal.title = Some("razuer@workstation:~/Work/Projects/hyprmux".to_string());
        pane.terminal.original_user = Some("razuer".to_string());
        pane.terminal.cwd = Some("/home/razuer/Work/Projects/hyprmux".to_string());

        assert_eq!(pane.titlebar_title(false), "~/Work/Projects/hyprmux");
    }

    #[test]
    fn titlebar_qualifies_cwd_after_switching_users() {
        let mut pane = pane();
        pane.terminal.title = Some("root@workstation:/etc/nginx".to_string());
        pane.terminal.original_user = Some("razuer".to_string());
        pane.terminal.cwd = Some("/etc/nginx".to_string());

        assert_eq!(pane.titlebar_title(false), "root · /etc/nginx");
    }

    #[test]
    fn titlebar_qualifies_a_custom_name_with_the_project_path() {
        let mut pane = pane();
        pane.set_custom_title("logs");
        pane.terminal.title = Some("razuer@workstation:/work/hyprmux/services/api".to_string());
        pane.terminal.original_user = Some("razuer".to_string());
        pane.terminal.cwd = Some("/work/hyprmux/services/api".to_string());
        pane.terminal.display_path = Some("hyprmux/services/api".to_string());

        assert_eq!(pane.titlebar_title(false), "logs · hyprmux/services/api");
    }

    #[test]
    fn titlebar_qualifies_an_application_title_with_the_project_path() {
        let mut pane = pane();
        pane.terminal.title = Some("nvim src/main.rs".to_string());
        pane.terminal.cwd = Some("/work/hyprmux/src".to_string());
        pane.terminal.display_path = Some("hyprmux/src".to_string());

        assert_eq!(pane.titlebar_title(false), "nvim src/main.rs · hyprmux/src");
    }

    #[test]
    fn titlebar_uses_structured_cwd_without_a_terminal_title() {
        let mut pane = pane();
        pane.terminal.cwd = Some("/work/hyprmux".to_string());

        assert_eq!(pane.titlebar_title(false), "/work/hyprmux");
    }

    #[test]
    fn titlebar_uses_the_generic_fallback_when_no_identity_is_available() {
        let mut pane = pane();
        pane.title = "Shell".to_string();

        assert_eq!(pane.titlebar_title(false), "Shell");
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
    fn awaits_input_ignores_closing_and_exited_panes() {
        let mut pane = pane();
        pane.terminal.reported_status = Some(crate::session::protocol::PaneStatus {
            value: "blocked".into(),
            reason: None,
            set_at: 1,
        });
        assert!(pane.awaits_input());
        pane.closing = true;
        assert!(!pane.awaits_input());
        pane.closing = false;
        pane.terminal.status = ManagedTerminalStatus::Exited(0);
        assert!(!pane.awaits_input());
    }
}
