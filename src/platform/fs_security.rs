//! Cross-platform "private directory" security policy (cross-platform plan Phase 3/5).
//!
//! On Unix (Linux/macOS) this enforces a directory that is a real directory (never a symlink,
//! checked via `symlink_metadata` rather than `metadata`), owned by the current uid, with no
//! group/other access bits set (`mode & 0o077 == 0`) - the same policy `control::runtime_dir`
//! enforced inline before this module existed.
//!
//! Windows equivalents (an explicit current-user SID DACL and rejecting reparse-point traversal)
//! are Phase 5/5b work and are **not implemented yet**: [`ensure_private_dir`] on Windows only
//! creates the directory today, with no privacy enforcement. Do not treat a Windows runtime/state
//! directory as access-controlled until that lands.

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

/// Create (if missing) a private-to-the-user directory.
///
/// **Not yet implemented for Windows**: this only creates the directory with default ACLs
/// inherited from its parent. The cross-platform plan calls for an explicit current-user SID DACL
/// plus reparse-point-traversal rejection (Phase 5/5b); until that lands, do not rely on this
/// enforcing any privacy guarantee on Windows the way it does on Unix.
#[cfg(not(unix))]
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
}

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
