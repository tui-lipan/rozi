//! Session naming rules shared by the server and the clients.

use std::sync::OnceLock;

/// The prefix that marks an auto-named ephemeral session. Ephemeral servers shut down on a clean
/// quit but survive a UI crash for reattach; user-typed names may not use this prefix.
pub const EPHEMERAL_SESSION_PREFIX: &str = "eph-";

/// Hex digits in a client's ephemeral identity: 96 bits, so two clients on any number of machines
/// sharing one host's sessions do not pick the same name.
const CLIENT_ID_HEX_LEN: usize = 24;

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
