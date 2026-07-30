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

/// The state of a leave prompt: the client is on its way out and the temporary sessions it would
/// take with it need an answer first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeaveIntent {
    /// How many temporary sessions leaving would close, counting the current one. Only ever the
    /// sessions worth asking about — untouched ones are discarded without a prompt.
    pub temporary: usize,
    /// Whether an empty submit has already been made once and is waiting for the second press that
    /// closes those sessions. Rendered in the prompt rather than a toast, so what the next press
    /// destroys is on screen while the finger is still over the key.
    pub armed: bool,
}

/// Prompt state for unified naming/renaming overlays.
pub struct SessionRenameState {
    pub input: TextInput,
    pub mode: NamingMode,
    /// Set when this prompt is the last step before the client exits: naming the session keeps it
    /// running, submitting nothing closes it. `None` for an ordinary in-place rename.
    pub leave: Option<LeaveIntent>,
    pub profile_seed: Option<(String, std::path::PathBuf)>,
    /// When set, a [`NamingMode::CreateSession`] prompt creates the session on this remote host
    /// (parking the current session in the background) instead of locally. Only the target string
    /// is involved — SSH handles authentication out of band.
    pub host_target: Option<crate::session::remote::RemoteTarget>,
    /// Why the last submit was rejected. Rendered inside the prompt rather than as a toast: the
    /// prompt is still open and holding the field being corrected, which a toast would cover.
    /// Cleared on the next edit, so it never outlives the text that caused it.
    pub error: Option<String>,
}

impl SessionRenameState {
    pub fn new(initial: impl AsRef<str>, mode: NamingMode) -> Self {
        Self {
            input: TextInput::new(initial.as_ref()),
            mode,
            leave: None,
            profile_seed: None,
            host_target: None,
            error: None,
        }
    }

    /// The prompt raised on the way out of the client when leaving would close `temporary`
    /// temporary sessions: name this one to keep it running, or submit nothing to close them.
    pub fn for_leave(temporary: usize) -> Self {
        Self {
            input: TextInput::new(""),
            mode: NamingMode::NameEphemeralSession,
            leave: Some(LeaveIntent {
                temporary,
                armed: false,
            }),
            profile_seed: None,
            host_target: None,
            error: None,
        }
    }

    /// A create-session prompt prefilled with `initial` (the session picker's query, when the
    /// prompt was raised from it). Empty is the ordinary case.
    pub fn new_create_named(initial: impl AsRef<str>) -> Self {
        Self::new(initial, NamingMode::CreateSession)
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
