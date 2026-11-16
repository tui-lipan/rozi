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

/// An operating-system account, as the kernel identifies it: the numeric uid on Unix.
///
/// This is the identity session sharing authorizes against. It is read from the kernel for a
/// connected peer ([`super::ipc::IpcConnection::peer_user`]) and resolved from the account
/// database for a name the session owner typed ([`lookup_user`]). It never comes from an
/// environment variable or from anything a peer sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UserId(u32);

impl UserId {
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    pub const fn as_raw(self) -> u32 {
        self.0
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// A [`UserId`] with the account name the system's account database gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserAccount {
    pub id: UserId,
    pub name: String,
}

/// Why session sharing between accounts is unavailable on this platform, or `None` where it works.
pub const fn session_sharing_unavailable() -> Option<&'static str> {
    if cfg!(unix) {
        None
    } else {
        Some("sharing a session with another user is not supported on this platform")
    }
}

/// The account this process runs as.
pub fn current_user_id() -> UserId {
    #[cfg(unix)]
    {
        UserId(super::fs_security::current_uid())
    }
    #[cfg(not(unix))]
    {
        UserId(0)
    }
}

/// Resolve an account from what a person typed: an account name, or a numeric uid.
///
/// `Ok(None)` means the account database has no such account.
pub fn lookup_user(spec: &str) -> std::io::Result<Option<UserAccount>> {
    #[cfg(unix)]
    {
        unix_impl::lookup_user(spec)
    }
    #[cfg(not(unix))]
    {
        let _ = spec;
        Err(sharing_unsupported())
    }
}

/// The account a [`UserId`] belongs to, when the account database knows it.
pub fn account_of(id: UserId) -> Option<UserAccount> {
    #[cfg(unix)]
    {
        unix_impl::account_of(id)
    }
    #[cfg(not(unix))]
    {
        let _ = id;
        None
    }
}

#[cfg(not(unix))]
pub(crate) fn sharing_unsupported() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        session_sharing_unavailable().unwrap_or("session sharing is not implemented"),
    )
}

#[cfg(unix)]
mod unix_impl {
    use std::ffi::{CStr, CString};
    use std::io;

    use super::{UserAccount, UserId};

    const MAX_PW_BUF_SIZE: usize = 1024 * 1024;

    pub(super) fn lookup_user(spec: &str) -> io::Result<Option<UserAccount>> {
        if spec.is_empty() {
            return Ok(None);
        }
        if spec.bytes().all(|byte| byte.is_ascii_digit()) {
            let Ok(uid) = spec.parse::<u32>() else {
                return Ok(None);
            };
            return lookup_by_uid_with_buf_len(uid, initial_pw_buf_len());
        }
        let Ok(c_name) = CString::new(spec) else {
            return Ok(None);
        };
        lookup_by_name_with_buf_len(&c_name, initial_pw_buf_len())
    }

    pub(super) fn account_of(id: UserId) -> Option<UserAccount> {
        lookup_by_uid_with_buf_len(id.as_raw(), initial_pw_buf_len())
            .ok()
            .flatten()
    }

    fn initial_pw_buf_len() -> usize {
        let max = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
        if max > 0 {
            (max as usize).clamp(1024, 16 * 1024)
        } else {
            1024
        }
    }

    pub(super) fn lookup_by_uid_with_buf_len(
        uid: u32,
        initial_len: usize,
    ) -> io::Result<Option<UserAccount>> {
        #[allow(clippy::unnecessary_cast)]
        let raw_uid = uid as libc::uid_t;
        if raw_uid as u32 != uid {
            return Ok(None);
        }
        lookup_passwd(initial_len, |pwd, buf, result| unsafe {
            libc::getpwuid_r(
                raw_uid,
                pwd,
                buf.as_mut_ptr().cast::<libc::c_char>(),
                buf.len(),
                result,
            )
        })
    }

    pub(super) fn lookup_by_name_with_buf_len(
        name: &CStr,
        initial_len: usize,
    ) -> io::Result<Option<UserAccount>> {
        lookup_passwd(initial_len, |pwd, buf, result| unsafe {
            libc::getpwnam_r(
                name.as_ptr(),
                pwd,
                buf.as_mut_ptr().cast::<libc::c_char>(),
                buf.len(),
                result,
            )
        })
    }

    fn lookup_passwd(
        initial_len: usize,
        mut call: impl FnMut(*mut libc::passwd, &mut [u8], *mut *mut libc::passwd) -> libc::c_int,
    ) -> io::Result<Option<UserAccount>> {
        let mut buf = vec![0u8; initial_len.max(1)];
        loop {
            let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
            let mut result: *mut libc::passwd = std::ptr::null_mut();
            let rc = call(&mut pwd, &mut buf, &mut result);
            match rc {
                0 => {
                    if result.is_null() || pwd.pw_name.is_null() {
                        return Ok(None);
                    }
                    #[allow(clippy::unnecessary_cast)]
                    let uid = pwd.pw_uid as u32;
                    let name = unsafe { CStr::from_ptr(pwd.pw_name) }
                        .to_string_lossy()
                        .into_owned();
                    return Ok(Some(UserAccount {
                        id: UserId::from_raw(uid),
                        name,
                    }));
                }
                libc::EINTR => continue,
                libc::ERANGE => {
                    let next_len = buf.len().saturating_mul(2);
                    if next_len > MAX_PW_BUF_SIZE || next_len <= buf.len() {
                        return Err(io::Error::from_raw_os_error(rc));
                    }
                    buf.resize(next_len, 0);
                }
                // Some NSS modules report an unknown account as ENOENT or ESRCH instead of 0 + NULL.
                libc::ENOENT | libc::ESRCH => return Ok(None),
                other => return Err(io::Error::from_raw_os_error(other)),
            }
        }
    }
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
    fn lookup_user_round_trips_current_account_and_rejects_unknown() {
        let uid = current_user_id();
        let account = account_of(uid).expect("current uid exists in passwd");
        assert_eq!(account.id, uid);
        assert!(!account.name.is_empty());

        assert_eq!(
            lookup_user(&uid.to_string()).expect("lookup by numeric uid"),
            Some(account.clone())
        );
        assert_eq!(
            lookup_user(&account.name).expect("lookup by name"),
            Some(account.clone())
        );
        assert_eq!(
            lookup_user("rozi-nonexistent-account-99999").expect("unknown name"),
            None
        );
        assert_eq!(lookup_user("").expect("empty spec"), None);
        assert_eq!(lookup_user("alice\0bob").expect("nul in spec"), None);
        assert_eq!(
            lookup_user("999999999999999999999999").expect("overflowing uid"),
            None
        );

        // Force ERANGE growth from an undersized initial buffer.
        let c_name = std::ffi::CString::new(account.name.as_str()).unwrap();
        assert_eq!(
            unix_impl::lookup_by_name_with_buf_len(&c_name, 8).expect("erange growth by name"),
            Some(account.clone())
        );
        assert_eq!(
            unix_impl::lookup_by_uid_with_buf_len(uid.as_raw(), 8).expect("erange growth by uid"),
            Some(account)
        );
    }
}
