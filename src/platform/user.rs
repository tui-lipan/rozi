//! Cross-platform current-user identity.
//!
//! Only [`current_user_tag`] is implemented so far, pulled forward from the plan's Phase 10
//! ("`USER` vs `USERNAME`") because [`super::paths`] needed a stable per-user fallback path
//! component now. Broader Phase 10 concerns (hostname resolution, editor/notification identity)
//! are not covered here yet.

/// A stable per-user identifier suitable for embedding in a fallback filesystem path.
///
/// Unix/macOS: the numeric uid (stringified) via [`super::fs_security::current_uid`] - stable
/// even when `$USER` is unset or does not match the real account. Windows: `$USERNAME`, falling
/// back to `$USER` (e.g. MSYS/Git Bash shells set this instead), then a fixed placeholder if
/// neither is set.
pub fn current_user_tag() -> String {
    #[cfg(unix)]
    {
        super::fs_security::current_uid().to_string()
    }
    #[cfg(not(unix))]
    {
        std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "unknown".to_string())
    }
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
}
