use tui_lipan::prelude::TextInput;

use super::PaneId;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneIdentity {
    pub custom_title: Option<String>,
    pub profile_name: Option<String>,
    pub cwd: Option<String>,
    pub command: Option<String>,
    pub keep_open: bool,
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
    RenameWorkspace { index: usize },
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
}

impl SessionRenameState {
    pub fn new(initial: impl AsRef<str>, mode: NamingMode) -> Self {
        Self {
            input: TextInput::new(initial.as_ref()),
            mode,
            detach_after: false,
            pending_confirm: false,
            profile_seed: None,
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
        }
    }

    pub fn new_create() -> Self {
        Self::new("", NamingMode::CreateSession)
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
