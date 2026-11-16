//! Session naming rules shared by the server and the clients.

use std::sync::OnceLock;

/// The prefix that marks an auto-named ephemeral session. Ephemeral servers shut down on a clean
/// quit but survive a UI crash for reattach; user-typed names may not use this prefix.
pub const EPHEMERAL_SESSION_PREFIX: &str = "eph-";

/// Hex digits in a client's ephemeral identity: 96 bits, so two clients on any number of machines
/// sharing one host's sessions do not pick the same name.
const CLIENT_ID_HEX_LEN: usize = 24;

/// The longest account name accepted as a session owner. Linux caps login names at 32 bytes;
/// other systems allow more.
const MAX_OWNER_LEN: usize = 64;

/// Where a session lives among the accounts of one machine.
///
/// A bare `dev` is a session of the account running this process. `alice/dev` is the session
/// `dev` that the account `alice` shares. The qualified form is an attach target only: it names
/// someone else's session, so it is never created, renamed to, restarted, or saved from here.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SessionAddress {
    /// The owning account's name or numeric id, `None` for one's own session.
    pub owner: Option<String>,
    /// The name the session's own server knows it by.
    pub name: String,
}

impl SessionAddress {
    /// Split an attach target into owner and session name, or `None` when it is not one.
    ///
    /// Both halves are restricted to characters that cannot leave a path component, so neither
    /// can change which directory or socket a target resolves to.
    pub fn parse(target: &str) -> Option<Self> {
        let (owner, name) = match target.split_once('/') {
            Some((owner, name)) => (Some(owner), name),
            None => (None, target),
        };
        if !valid_bare_name(name) || !owner.is_none_or(valid_owner) {
            return None;
        }
        Some(Self {
            owner: owner.map(str::to_string),
            name: name.to_string(),
        })
    }

    /// Whether this is another account's session.
    pub fn is_shared(&self) -> bool {
        self.owner.is_some()
    }
}

impl std::fmt::Display for SessionAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.owner {
            Some(owner) => write!(f, "{owner}/{}", self.name),
            None => f.write_str(&self.name),
        }
    }
}

/// Whether `target` names another account's session (`owner/name`).
pub fn is_shared_target(target: &str) -> bool {
    SessionAddress::parse(target).is_some_and(|address| address.is_shared())
}

/// The owner prefix of `target` applied to `name`: what a guest calls a session after its owner
/// renamed it. A bare target leaves `name` as it is.
pub fn with_owner_of(target: &str, name: &str) -> String {
    match SessionAddress::parse(target).and_then(|address| address.owner) {
        Some(owner) => format!("{owner}/{name}"),
        None => name.to_string(),
    }
}

/// The longest session name, in bytes. See [`crate::session::discovery::MAX_SESSION_NAME_LEN`],
/// which re-exports it, for why it is bounded.
pub const MAX_SESSION_NAME_LEN: usize = 64;

pub(crate) fn valid_bare_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SESSION_NAME_LEN
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn valid_owner(owner: &str) -> bool {
    !owner.is_empty()
        && owner.len() <= MAX_OWNER_LEN
        && owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        // `.` and `..` are path components, and a leading `-` reads as an option.
        && !owner.starts_with(['.', '-'])
}

/// Whether `name` denotes an auto-managed ephemeral session.
pub fn is_ephemeral_session_name(name: &str) -> bool {
    name.starts_with(EPHEMERAL_SESSION_PREFIX)
}

/// A random identity this client process picks once and names its temporary sessions after.
///
/// Deliberately not the process id. A client can be replaced by another process that carries its
/// sessions on - a relaunch onto a newer build - and on Unix that process keeps the pid. Naming the
/// next temporary session after the pid would land the new process on a session it already
/// restored. Random bytes also cannot coincide between two machines that share a host's sessions.
pub fn client_identity() -> &'static str {
    static IDENTITY: OnceLock<String> = OnceLock::new();
    IDENTITY.get_or_init(|| {
        let mut bytes = [0u8; CLIENT_ID_HEX_LEN / 2];
        getrandom::fill(&mut bytes).expect("operating-system randomness unavailable");
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    })
}

/// The ephemeral session name for this UI process (`eph-<client identity>`).
pub fn ephemeral_session_name() -> String {
    format!("{EPHEMERAL_SESSION_PREFIX}{}", client_identity())
}

/// A fresh ephemeral name that will not collide with a still-running ephemeral server left behind
/// by a prior detach (`eph-<client identity>-<salt>`).
pub fn fresh_ephemeral_session_name(salt: u64) -> String {
    format!("{EPHEMERAL_SESSION_PREFIX}{}-{salt}", client_identity())
}

/// Ephemeral name qualified by the hostname (`eph-<host>-<client identity>`), for `--remote`.
///
/// The session lives on the *remote* host, where it sits among the sessions of every machine that
/// connects there. The client identity alone already keeps them apart; the hostname is what lets a
/// person reading `sessions list` on that host tell where each one came from. Both stay constant for
/// the process lifetime, so a dropped link reconnects to the same ephemeral name.
pub fn remote_ephemeral_session_name() -> String {
    let host = crate::platform::user::hostname()
        .map(|host| {
            // Session names permit only `[A-Za-z0-9_-]`; keep the alphanumerics and cap the length
            // so an odd or very long hostname cannot produce an invalid or unwieldy name.
            host.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(24)
                .collect::<String>()
                .to_ascii_lowercase()
        })
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "host".to_string());
    format!("{EPHEMERAL_SESSION_PREFIX}{host}-{}", client_identity())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_address_cannot_name_a_path() {
        assert_eq!(
            SessionAddress::parse("alice/dev"),
            Some(SessionAddress {
                owner: Some("alice".to_string()),
                name: "dev".to_string(),
            })
        );
        assert_eq!(SessionAddress::parse("dev").unwrap().owner, None);
        assert_eq!(
            SessionAddress::parse("1001/dev").unwrap().to_string(),
            "1001/dev"
        );
        for target in [
            "",
            "/dev",
            "alice/",
            "../dev",
            "./dev",
            "alice/../dev",
            "a/b/c",
            "alice/de v",
            "al ice/dev",
            "-rf/dev",
            "alice\\dev",
            "alice/dev.sock",
            "alice\0/dev",
        ] {
            assert_eq!(SessionAddress::parse(target), None, "{target:?}");
        }
        assert_eq!(with_owner_of("alice/dev", "api"), "alice/api");
        assert_eq!(with_owner_of("dev", "api"), "api");
    }

    #[test]
    fn temporary_names_are_valid_attach_targets_independent_of_the_pid() {
        let id = client_identity();
        assert_eq!(id.len(), CLIENT_ID_HEX_LEN);
        assert!(id.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(
            client_identity(),
            id,
            "one identity for the process lifetime"
        );
        for name in [
            ephemeral_session_name(),
            fresh_ephemeral_session_name(u64::MAX),
            remote_ephemeral_session_name(),
        ] {
            assert!(is_ephemeral_session_name(&name), "{name}");
            assert!(
                name.len() <= crate::session::discovery::MAX_SESSION_NAME_LEN,
                "{name}"
            );
            assert!(
                crate::session::discovery::valid_attach_target(&name),
                "{name}"
            );
        }
    }
}
