//! Cross-platform "private directory" security policy (cross-platform plan Phase 3/5).
//!
//! On Unix (Linux/macOS) this enforces a directory that is a real directory (never a symlink,
//! checked via `symlink_metadata` rather than `metadata`), owned by the current uid, with no
//! group/other access bits set (`mode & 0o077 == 0`) - the same policy `control::runtime_dir`
//! enforced inline before this module existed.
//!
//! On Windows the equivalent is a directory created with an explicit, non-inherited DACL granting
//! full control to the current user's SID and nobody else ([`private_security_descriptor`], shared
//! with the named-pipe backend in [`super::ipc`] so an endpoint and the registry entry pointing at
//! it are protected by exactly the same ACL). Validation of an existing directory additionally
//! rejects a reparse point (junction/symlink) standing in for it, the Windows counterpart of the
//! Unix `symlink_metadata` check - an attacker-planted junction is the mechanism that would
//! otherwise redirect a private directory somewhere world-readable.
//!
//! The Windows half type-checks under `cargo check --target x86_64-pc-windows-gnu` but is
//! **unverified at runtime** - no Windows host is available in this workspace.

use std::fs;
use std::io;
use std::path::Path;

/// Current user id, used to assign ownership expectations and per-user fallback paths.
#[cfg(unix)]
pub fn current_uid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    unsafe { getuid() }
}

/// Create (if missing) or validate an existing directory as private to the current user.
#[cfg(unix)]
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    match fs::symlink_metadata(dir) {
        Ok(metadata) => validate_private_dir(dir, &metadata),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(dir)?;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
            validate_private_dir(dir, &fs::symlink_metadata(dir)?)
        }
        Err(err) => Err(err),
    }
}

/// Write `bytes` to a new file with mode `0600` (Unix) / inherited private ACL (Windows).
///
/// Creates parent directories as private when missing. Uses `create_new` so an existing path
/// is never truncated through a re-resolved path race; callers that need unique names
/// (for example scrollback dumps) should generate them before calling.
#[cfg(unix)]
pub fn write_private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::fs::PermissionsExt;

    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    // Reinforce mode on the open handle (not via path) in case the create mode was masked.
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Validate that `dir` (with pre-fetched `metadata` from `symlink_metadata`, never `metadata`, so
/// a symlink cannot substitute for the real directory) is private to the current user.
#[cfg(unix)]
pub fn validate_private_dir(dir: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    if !metadata.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is not a directory", dir.display()),
        ));
    }
    if metadata.uid() != current_uid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is not owned by the current user", dir.display()),
        ));
    }
    if metadata.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} permissions must not allow group/other access",
                dir.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(windows)]
mod windows_impl {
    use super::{Path, fs, io};

    use windows_sys::Win32::Foundation::{HANDLE, HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// An owned `SECURITY_DESCRIPTOR` allocated by `ConvertStringSecurityDescriptorToSecurityDescriptorW`.
    ///
    /// Wrapping it in a type with a `Drop` is the whole point: the raw pointer must be released with
    /// `LocalFree`, and every user of it (directory creation, named-pipe creation) would otherwise
    /// have to remember to do that on each of its several error paths.
    pub struct PrivateSecurityDescriptor(PSECURITY_DESCRIPTOR);

    impl PrivateSecurityDescriptor {
        /// A `SECURITY_ATTRIBUTES` pointing at this descriptor, with non-inheritable handles.
        ///
        /// Borrowed, not owned: the returned struct is only valid while `self` is alive, which the
        /// lifetime here enforces.
        pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
            SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: self.0,
                bInheritHandle: 0,
            }
        }
    }

    impl Drop for PrivateSecurityDescriptor {
        fn drop(&mut self) {
            unsafe { LocalFree(self.0 as HLOCAL) };
        }
    }

    /// The current process token's user SID, as a string (`S-1-5-21-...`).
    pub fn current_user_sid() -> io::Result<String> {
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error());
            }
            let token = OwnedHandle(token);

            // Two-call idiom: the first call fails with ERROR_INSUFFICIENT_BUFFER but reports the
            // size a TOKEN_USER plus its variable-length SID actually needs.
            let mut needed: u32 = 0;
            GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut needed);
            if needed == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut buffer = vec![0u8; needed as usize];
            if GetTokenInformation(
                token.0,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }

            let token_user = &*buffer.as_ptr().cast::<TOKEN_USER>();
            let mut sid_string: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(token_user.User.Sid, &mut sid_string) == 0 {
                return Err(io::Error::last_os_error());
            }
            let sid = wide_to_string(sid_string);
            LocalFree(sid_string as HLOCAL);
            Ok(sid)
        }
    }

    /// A security descriptor granting full control to the current user and to nobody else.
    ///
    /// `D:P` makes the DACL *protected*: inheritable ACEs from the parent container (which for a
    /// directory under `%LOCALAPPDATA%` would normally include SYSTEM and Administrators) are not
    /// merged in. `(A;OICI;GA;;;<sid>)` grants that one SID `GENERIC_ALL`, inheritable by child
    /// objects and containers so files created inside a private directory stay private.
    pub fn private_security_descriptor() -> io::Result<PrivateSecurityDescriptor> {
        let sid = current_user_sid()?;
        let sddl = format!("D:P(A;OICI;GA;;;{sid})");
        let wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(PrivateSecurityDescriptor(descriptor))
    }

    pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
        match fs::symlink_metadata(dir) {
            Ok(metadata) => validate_private_dir(dir, &metadata),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                // Every ancestor must exist before the leaf can be created with our own DACL.
                // Ancestors are `%LOCALAPPDATA%`-style directories the user already owns, so they
                // are created with inherited ACLs; only the leaf carries the protected DACL.
                if let Some(parent) = dir.parent() {
                    fs::create_dir_all(parent)?;
                }
                let descriptor = private_security_descriptor()?;
                let attributes = descriptor.attributes();
                let wide = wide_path(dir);
                if unsafe { CreateDirectoryW(wide.as_ptr(), &attributes) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                validate_private_dir(dir, &fs::symlink_metadata(dir)?)
            }
            Err(err) => Err(err),
        }
    }

    /// Write `bytes` into a private parent directory. Child files inherit the protected DACL.
    ///
    /// Uses `create_new` so an existing path is never truncated through a re-resolved path race.
    pub fn write_private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;

        if let Some(parent) = path.parent() {
            ensure_private_dir(parent)?;
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(bytes)?;
        file.sync_all()
    }

    /// Reject anything that is not a real directory we can trust: a file, or a reparse point
    /// (junction/symlink) that could silently redirect the "private" directory somewhere else.
    ///
    /// This is the Windows counterpart of the Unix symlink check, not of the Unix *permission*
    /// check: a directory we created carries the protected DACL from [`private_security_descriptor`],
    /// and one we did not create but which is a plain directory under the user's own
    /// `%LOCALAPPDATA%` is already only reachable by that user. Auditing an inherited DACL ACE by
    /// ACE would add a great deal of surface for very little: the attack this actually has to stop
    /// is the planted junction.
    fn validate_private_dir(dir: &Path, metadata: &fs::Metadata) -> io::Result<()> {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

        if !metadata.file_type().is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is not a directory", dir.display()),
            ));
        }
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is a reparse point, not a real directory", dir.display()),
            ));
        }
        Ok(())
    }

    /// A NUL-terminated wide-char path, the form every `*W` Win32 entry point wants.
    pub fn wide_path(path: &Path) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    unsafe fn wide_to_string(ptr: *const u16) -> String {
        let mut len = 0;
        while unsafe { *ptr.add(len) } != 0 {
            len += 1;
        }
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(ptr, len) })
    }

    /// Closes its `HANDLE` on drop, so the several `?` early-returns above cannot leak it.
    struct OwnedHandle(HANDLE);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
        }
    }
}

#[cfg(windows)]
pub use windows_impl::{
    current_user_sid, ensure_private_dir, private_security_descriptor, write_private_file,
};

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp_base(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "hyprmux-fs-security-test-{name}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn creates_missing_directory_with_private_mode() {
        let dir = temp_base("create");
        let _ = fs::remove_dir_all(&dir);

        ensure_private_dir(&dir).expect("create");
        let mode = fs::symlink_metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_private_file_creates_0600_file() {
        let dir = temp_base("private-file");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("dump.txt");

        write_private_file(&path, b"scrollback\n").expect("write");
        let mode = fs::symlink_metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(fs::read_to_string(&path).unwrap(), "scrollback\n");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_group_or_other_accessible_directory() {
        let dir = temp_base("perms");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();

        let err = ensure_private_dir(&dir).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_symlink_standing_in_for_the_directory() {
        let dir = temp_base("symlink");
        let target = temp_base("symlink-target");
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&target);
        fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, &dir).unwrap();

        let err = ensure_private_dir(&dir).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&target);
    }
}
