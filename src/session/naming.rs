//! Session naming rules shared by the server and the clients.

/// The prefix that marks an auto-named ephemeral session. Ephemeral servers shut down on a clean
/// quit but survive a UI crash for reattach; user-typed names may not use this prefix.
pub const EPHEMERAL_SESSION_PREFIX: &str = "eph-";

/// Whether `name` denotes an auto-managed ephemeral session.
pub fn is_ephemeral_session_name(name: &str) -> bool {
    name.starts_with(EPHEMERAL_SESSION_PREFIX)
}

/// The ephemeral session name for this UI process (`eph-<pid>`).
pub fn ephemeral_session_name() -> String {
    format!("{EPHEMERAL_SESSION_PREFIX}{}", std::process::id())
}

/// A fresh ephemeral name that will not collide with a still-running ephemeral server left behind
/// by a prior detach (`eph-<pid>-<salt>`).
pub fn fresh_ephemeral_session_name(salt: u64) -> String {
    format!("{EPHEMERAL_SESSION_PREFIX}{}-{salt}", std::process::id())
}

/// Ephemeral name qualified by a stable per-client identifier (`eph-<host>-<pid>`), for `--remote`.
///
/// A bare `eph-<pid>` names a session that lives on the *remote* host, where two clients on
/// different machines can plausibly share a pid and would silently land on the same session. The
/// hostname disambiguates them; it stays constant for the process lifetime, so a dropped link
/// reconnects to the same ephemeral name.
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
    format!("{EPHEMERAL_SESSION_PREFIX}{host}-{}", std::process::id())
}
