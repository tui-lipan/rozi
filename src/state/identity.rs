use tui_lipan::prelude::TextInput;

use super::PaneId;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneIdentity {
    pub custom_title: Option<String>,
    pub profile_name: Option<String>,
    pub cwd: Option<String>,
    pub command: Option<String>,
    pub keep_open: bool,
    /// Run `command` by typing it into the pane's interactive shell instead of handing it to the
    /// deterministic command-runner shell. Set for profile-restored panes: a captured command is
    /// something the user once typed at their prompt, so replaying it must resolve aliases, shell
    /// functions, and rc-file `PATH` additions, and the shell prompt (with its title/OSC
    /// integration) must come up first. Never persisted; restore derives it.
    pub replay: bool,
    /// Extra environment for this spawn only, merged over the standard pane environment.
    ///
    /// This is how a command line receives untrusted values such as a filename: the value never
    /// enters the command string, so the shell expands `"$VAR"` as one word instead of re-parsing
    /// it for command syntax. Never persisted — a restored pane re-derives whatever it needs.
    pub env: Vec<(String, String)>,
}

impl PaneIdentity {
    pub fn set_custom_title(&mut self, title: impl AsRef<str>) {
        let title = title.as_ref().trim();
        if title.is_empty() {
            self.custom_title = None;
            self.profile_name = None;
        } else {
            self.custom_title = Some(title.to_string());
        }
    }
}

pub struct PaneRenameState {
    pub target: PaneId,
    pub input: TextInput,
}

impl PaneRenameState {
    pub fn new(target: PaneId, initial: impl AsRef<str>) -> Self {
        Self {
            target,
            input: TextInput::new(initial.as_ref()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamingMode {
    CreateSession,
    OpenProfileAs,
    NameEphemeralSession,
    RenameSession,
    RenameWorkspace {
        index: usize,
    },
    /// Enter an SSH target and attach to a fresh session on that remote host.
    ConnectRemoteHost,
}

/// Prompt state for unified naming/renaming overlays.
pub struct SessionRenameState {
    pub input: TextInput,
    pub mode: NamingMode,
    pub detach_after: bool,
    /// Set once the first Enter has warned that creating this session will discard the current
    /// disposable ephemeral one; the modal shows the armed state (red border + inline note) and a
    /// second Enter commits. Cleared when the name is edited so the guard re-arms. Only meaningful
    /// for [`NamingMode::CreateSession`] while attached to an ephemeral session.
    pub pending_confirm: bool,
    pub profile_seed: Option<(String, std::path::PathBuf)>,
    /// When set, a [`NamingMode::CreateSession`] prompt creates the session on this remote host
    /// (parking the current session in the background) instead of locally. Only the target string
    /// is involved — SSH handles authentication out of band.
    pub host_target: Option<crate::session::remote::RemoteTarget>,
}

impl SessionRenameState {
    pub fn new(initial: impl AsRef<str>, mode: NamingMode) -> Self {
        Self {
            input: TextInput::new(initial.as_ref()),
            mode,
            detach_after: false,
            pending_confirm: false,
            profile_seed: None,
            host_target: None,
        }
    }

    /// A rename prompt raised by `prefix d` on an ephemeral session: name it, then detach.
    pub fn for_detach() -> Self {
        Self {
            input: TextInput::new(""),
            mode: NamingMode::NameEphemeralSession,
            detach_after: true,
            pending_confirm: false,
            profile_seed: None,
            host_target: None,
        }
    }

    pub fn new_create() -> Self {
        Self::new("", NamingMode::CreateSession)
    }

    /// A "New session on `<host>`" prompt: names a session to create on `target`.
    pub fn new_create_on_host(target: crate::session::remote::RemoteTarget) -> Self {
        let mut state = Self::new("", NamingMode::CreateSession);
        state.host_target = Some(target);
        state
    }

    /// A "Connect remote host…" prompt, prefilled with the most recently used ad-hoc target so a
    /// quick reconnect is one keypress away.
    pub fn new_connect_host() -> Self {
        let recent = crate::session::read_recent_remotes();
        Self::new(
            recent.first().map(String::as_str).unwrap_or_default(),
            NamingMode::ConnectRemoteHost,
        )
    }

    pub fn new_open_profile_as(profile: String, path: std::path::PathBuf) -> Self {
        let mut state = Self::new("", NamingMode::OpenProfileAs);
        state.profile_seed = Some((profile, path));
        state
    }

    pub fn new_name_ephemeral() -> Self {
        Self::new("", NamingMode::NameEphemeralSession)
    }

    pub fn new_rename_workspace(index: usize, initial: impl AsRef<str>) -> Self {
        Self::new(initial, NamingMode::RenameWorkspace { index })
    }

    pub fn new_rename_session(initial: impl AsRef<str>) -> Self {
        Self::new(initial, NamingMode::RenameSession)
    }
}
