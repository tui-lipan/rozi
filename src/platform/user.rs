//! Cross-platform current-user identity (cross-platform plan Phase 10, `USER` vs `USERNAME`).
//!
//! Two distinct notions, deliberately not conflated:
//!
//! - [`current_user_tag`] - a *machine* identifier, safe to embed in a filesystem path or an IPC
//!   endpoint name. Stable, collision-free, and never influenced by a spoofable env var on Unix.
//! - [`current_user_label`] - a *human* identifier, shown to other clients attached to the same
//!   session. Best-effort and cosmetic; a wrong or missing value degrades to `"client"`.

/// A stable per-user identifier suitable for embedding in a fallback filesystem path or an IPC
/// endpoint name.
///
/// Unix/macOS: the numeric uid (stringified) via [`super::fs_security::current_uid`] - stable
/// even when `$USER` is unset or does not match the real account. Windows: the current user's SID
/// string, which is the same identity the named-pipe DACL is written against (see
/// [`super::ipc`]); falls back to `%USERNAME%` and then a fixed placeholder if the SID cannot be
/// read, since a *wrong* tag only costs discoverability, never privacy - privacy comes from the
/// DACL, not from the name.
pub fn current_user_tag() -> String {
    #[cfg(unix)]
    {
        super::fs_security::current_uid().to_string()
    }
    #[cfg(windows)]
    {
        super::fs_security::current_user_sid()
            .ok()
            .or_else(|| non_empty_env("USERNAME"))
            .unwrap_or_else(|| "unknown".to_string())
    }
}

/// A human-readable name for the current user, shown as a client's label to everyone else attached
/// to a shared session.
///
/// Unix/macOS reads `$USER` then `$LOGNAME`; Windows reads `%USERNAME%` then `%USER%` (MSYS/Git
/// Bash shells set the latter instead). Purely cosmetic - falls back to `"client"` rather than
/// failing, and is never used for an authorization or path decision.
pub fn current_user_label() -> String {
    let (primary, secondary) = if cfg!(windows) {
        ("USERNAME", "USER")
    } else {
        ("USER", "LOGNAME")
    };
    non_empty_env(primary)
        .or_else(|| non_empty_env(secondary))
        .unwrap_or_else(|| "client".to_string())
}

/// The local machine's hostname, used to tell a *local* OSC 7 working directory (which a pane may
/// be spawned into) from a *remote* one over SSH (which is displayable but never spawnable).
///
/// `None` when the platform cannot report one - callers must then treat any OSC 7 report carrying a
/// host component as remote, which is the safe direction to fail.
pub fn hostname() -> Option<String> {
    static HOSTNAME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    HOSTNAME.get_or_init(resolve_hostname).clone()
}

fn resolve_hostname() -> Option<String> {
    #[cfg(unix)]
    {
        let mut buffer = [0 as libc::c_char; 256];
        let ok = unsafe { libc::gethostname(buffer.as_mut_ptr(), buffer.len() - 1) } == 0;
        if !ok {
            return None;
        }
        // gethostname is not guaranteed to NUL-terminate on truncation; the reserved final byte
        // above is already zero, so the C string is bounded either way.
        let bytes = buffer
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| *byte as u8)
            .collect::<Vec<u8>>();
        String::from_utf8(bytes)
            .ok()
            .filter(|name| !name.is_empty())
    }
    #[cfg(windows)]
    {
        non_empty_env("COMPUTERNAME")
    }
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn current_user_tag_is_numeric_uid_on_unix() {
        let tag = current_user_tag();
        assert!(
            tag.chars().all(|c| c.is_ascii_digit()),
            "expected a numeric uid, got {tag:?}"
        );
    }

    #[test]
    fn current_user_label_always_yields_something_printable() {
        let label = current_user_label();
        assert!(!label.is_empty());
        assert!(!label.contains('\0'));
    }

    #[test]
    fn hostname_is_non_empty_when_reported() {
        // A CI container can have an odd hostname but never an empty or NUL-padded one; the point
        // is that the C-string decode above does not leak the buffer's trailing zeros.
        if let Some(host) = hostname() {
            assert!(!host.is_empty());
            assert!(!host.contains('\0'));
        }
    }
}
