//! Who a client is to a session, and what that lets it do.
//!
//! A session server decides both. [`ClientOrigin`] is read from the transport: the owner's private
//! endpoint, or the shared endpoint with the peer's account as the kernel reports it. The
//! [`SessionRole`] is the lower of what the session owner granted and what the client asked for.
//! Nothing a client sends can raise either.
//!
//! Every request names the [`Permission`] it needs through an exhaustive match
//! ([`super::ClientMessage::required_permission`],
//! [`crate::control::ControlCommand::required_permission`]), so a new request does not compile
//! until its author has decided who may send it.

use serde::{Deserialize, Serialize};

/// `code` on the [`super::ServerMessage::Error`] a peer on the shared endpoint receives when the
/// session owner has not shared the session with its account. The server says nothing else to it.
pub const NOT_SHARED_ERROR_CODE: &str = "not-shared";

/// `code` on the [`super::ServerMessage::Error`] that refuses one request the client's access does
/// not cover. The connection stays open.
pub const PERMISSION_DENIED_ERROR_CODE: &str = "permission-denied";

/// What a client may do in a session. Each role includes the ones before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum SessionRole {
    /// Sees panes and layout. Cannot type, change the layout, or hold layout control.
    Viewer,
    /// Also types into panes. Typing runs commands as the session owner.
    Editor,
    /// Also may hold layout control.
    Controller,
    /// Also stops, renames, and shares the session. Only the owner's own account has this role.
    Owner,
}

impl SessionRole {
    /// The roles a session owner can grant to another account.
    pub const GRANTABLE: [SessionRole; 3] = [
        SessionRole::Viewer,
        SessionRole::Editor,
        SessionRole::Controller,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            SessionRole::Viewer => "viewer",
            SessionRole::Editor => "editor",
            SessionRole::Controller => "controller",
            SessionRole::Owner => "owner",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [
            SessionRole::Viewer,
            SessionRole::Editor,
            SessionRole::Controller,
            SessionRole::Owner,
        ]
        .into_iter()
        .find(|role| role.as_str() == name)
    }

    /// The role a client's own attach request asks for: a read-only attach is a viewer, and
    /// anything else asks for everything its grant allows.
    pub const fn requested(read_only: bool) -> Self {
        if read_only {
            SessionRole::Viewer
        } else {
            SessionRole::Owner
        }
    }
}

impl std::fmt::Display for SessionRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl super::tolerant::UnknownValue for SessionRole {
    /// The role that claims nothing: a role this build cannot name is shown as the least one.
    fn unknown(_: Option<&str>) -> Self {
        SessionRole::Viewer
    }
}

/// `#[serde(deserialize_with)]` for an optional role a newer peer may fill with a value this build
/// does not know.
pub(crate) fn deserialize_optional_role<'de, D>(
    deserializer: D,
) -> Result<Option<SessionRole>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.map(|value| {
        SessionRole::deserialize(&value)
            .unwrap_or_else(|_| super::tolerant::UnknownValue::unknown(value.as_str()))
    }))
}

/// Which account a client connected as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClientOrigin {
    /// The session owner's own account.
    Owner,
    /// Another account the owner shared the session with.
    Guest,
}

/// One class of thing a request does. A request needs exactly one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Permission {
    /// Receive panes, layout, and status, and keep client-local state. Also every request whose
    /// handler acts only for the client holding layout control, since [`Permission::Layout`]
    /// already decides who can hold it.
    View,
    /// Send input to panes, or anything else that acts as the session owner inside a pane.
    Input,
    /// Ask for or be given layout control.
    Layout,
    /// Stop, rename, or share the session, or write the owner's files outside a pane.
    Admin,
    /// Read the owner's files outside a pane: the file tree, changed files, worktrees.
    HostFiles,
    /// Read what agents were asked and what they answered. Neither has to reach a pane, so a guest
    /// who may only see panes does not read them.
    AgentContent,
}

/// A client's standing with a session, as its server holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ClientAccess {
    pub origin: ClientOrigin,
    pub role: SessionRole,
}

impl ClientAccess {
    /// A client on the owner's private endpoint that has not limited itself.
    pub const OWNER: ClientAccess = ClientAccess {
        origin: ClientOrigin::Owner,
        role: SessionRole::Owner,
    };

    /// The owner's own client with the role its attach asked for.
    pub const fn owner(read_only: bool) -> Self {
        ClientAccess {
            origin: ClientOrigin::Owner,
            role: SessionRole::requested(read_only),
        }
    }

    /// A guest granted `granted`, limited further by what its attach asked for.
    pub fn guest(granted: SessionRole, read_only: bool) -> Self {
        ClientAccess {
            origin: ClientOrigin::Guest,
            // A grant never reaches `Owner`; the clamp keeps that true whatever was stored.
            role: granted
                .min(SessionRole::Controller)
                .min(SessionRole::requested(read_only)),
        }
    }

    pub fn allows(self, permission: Permission) -> bool {
        match permission {
            Permission::View => true,
            Permission::Input => self.role >= SessionRole::Editor,
            Permission::Layout => self.role >= SessionRole::Controller,
            Permission::Admin => self.role == SessionRole::Owner,
            // Sharing a machine does not make the owner's files the guest's to read, and a
            // read-only attach by the owner still browses its own files.
            Permission::HostFiles => self.origin == ClientOrigin::Owner,
            Permission::AgentContent => {
                self.origin == ClientOrigin::Owner || self.allows(Permission::Input)
            }
        }
    }

    pub fn is_guest(self) -> bool {
        self.origin == ClientOrigin::Guest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guest_never_holds_more_than_its_grant_or_its_own_request() {
        let viewer = ClientAccess::guest(SessionRole::Viewer, false);
        assert!(viewer.allows(Permission::View));
        assert!(!viewer.allows(Permission::Input));

        let editor = ClientAccess::guest(SessionRole::Editor, false);
        assert!(editor.allows(Permission::Input));
        assert!(!editor.allows(Permission::Layout));

        let controller = ClientAccess::guest(SessionRole::Controller, false);
        assert!(controller.allows(Permission::Layout));
        assert!(!controller.allows(Permission::Admin));
        assert!(!controller.allows(Permission::HostFiles));

        // A stored grant of `Owner` must not make a guest an owner.
        let forged = ClientAccess::guest(SessionRole::Owner, false);
        assert_eq!(forged.role, SessionRole::Controller);
        assert!(!forged.allows(Permission::Admin));

        // Asking for less is honored; asking cannot add.
        assert_eq!(
            ClientAccess::guest(SessionRole::Controller, true).role,
            SessionRole::Viewer
        );

        let read_only_owner = ClientAccess::owner(true);
        assert!(!read_only_owner.allows(Permission::Input));
        assert!(read_only_owner.allows(Permission::HostFiles));
        assert!(ClientAccess::OWNER.allows(Permission::Admin));
    }
}
