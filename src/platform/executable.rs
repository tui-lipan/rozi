//! Durable executable and selector operations used by managed installations.
//!
//! This module is deliberately small.  It owns the operations whose correctness depends on the
//! host filesystem (temporary files, durable replacement, executable permissions, and Unix
//! symlinks); the installation module owns the policy about which paths may be changed.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Return the executable that started the current process, after resolving a launcher symlink when
/// the platform can do so.  A regular file is required: an installer must never copy a directory,
/// FIFO, or another indirection as its own payload.
pub fn current_exe() -> io::Result<PathBuf> {
    let path = std::env::current_exe()?;
    let path = fs::canonicalize(path)?;
    ensure_regular_file(&path)?;
    Ok(path)
}

/// Descriptive alias for [`current_exe`].
pub fn current_executable() -> io::Result<PathBuf> {
    current_exe()
}

/// Resolve the launcher-v1 payload from the launcher's own location and an `active` file value.
///
/// This parser is platform-neutral so the path and semantic-version contract is unit tested on
/// every host. The launcher accepts no path from state: the only input is a canonical semantic
/// version, and the payload path is always derived as `versions/<version>/hyprmux.exe`.
pub fn resolve_launcher_v1_payload(launcher: &Path, active: &[u8]) -> io::Result<PathBuf> {
    let bin = launcher.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "launcher has no parent directory",
        )
    })?;
    if bin.file_name().and_then(|name| name.to_str()) != Some("bin") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "launcher must be installed directly under the managed bin directory",
        ));
    }
    let root = bin.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "launcher bin directory has no install root",
        )
    })?;
    let raw = std::str::from_utf8(active)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "active is not UTF-8"))?;
    if raw.is_empty() || raw.len() > 128 || raw.trim() != raw {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "active must contain only a canonical semantic version",
        ));
    }
    let version = semver::Version::parse(raw).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("active does not contain a semantic version: {error}"),
        )
    })?;
    if version.to_string() != raw {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "active version is not canonical",
        ));
    }
    Ok(root
        .join("versions")
        .join(version.to_string())
        .join("hyprmux.exe"))
}

/// Execute the immutable Windows launcher-v1 protocol and return the payload exit code.
#[cfg(windows)]
pub fn run_windows_launcher() -> io::Result<i32> {
    use std::io::Read;

    let launcher = std::env::current_exe()?;
    let root = launcher
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid launcher location"))?;
    let active_path = root.join("active");
    ensure_regular_file(&active_path)?;
    let mut active = Vec::new();
    File::open(&active_path)?
        .take(129)
        .read_to_end(&mut active)?;
    if active.len() > 128 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "active selector is too large",
        ));
    }
    let payload = resolve_launcher_v1_payload(&launcher, &active)?;
    ensure_regular_file(&payload)?;

    // Command inherits the exact environment, working directory, standard handles, and attached
    // console. Only argv[0] changes from the stable launcher to the selected payload.
    let status = std::process::Command::new(payload)
        .args(std::env::args_os().skip(1))
        .status()?;
    status.code().ok_or_else(|| {
        io::Error::other("managed payload exited without a Windows process exit code")
    })
}

/// The launcher artifact is Windows-only; keeping a stub lets all-target checks build the named
/// binary without pretending it can launch on another platform.
#[cfg(not(windows))]
pub fn run_windows_launcher() -> io::Result<i32> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "hyprmux-launcher is only supported on Windows",
    ))
}

/// Reject a path which is not a regular, non-link file.
pub fn ensure_regular_file(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || is_reparse_point(path)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is not a regular file", path.display()),
        ));
    }
    Ok(())
}

/// Whether `path` is a Windows reparse point.  Unix has no equivalent indirection bit; symlink
/// checks use `symlink_metadata` at every policy boundary instead.
pub fn is_reparse_point(path: &Path) -> io::Result<bool> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        Ok(fs::symlink_metadata(path)?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(false)
    }
}

/// Read a Unix selector symlink without following it.  On Windows this returns `None`; managed
/// installations use the UTF-8 `active` file instead.
#[cfg(unix)]
pub fn read_symlink(path: &Path) -> io::Result<Option<PathBuf>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => fs::read_link(path).map(Some),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not a symbolic link", path.display()),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// The non-Unix selector shape is intentionally represented by an absent symlink.
#[cfg(not(unix))]
pub fn read_symlink(path: &Path) -> io::Result<Option<PathBuf>> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not a symbolic link on this platform", path.display()),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Replace a regular file atomically and durably.  The temporary file is created beside the
/// destination, written and synced before the final replace, so a crash exposes either the old
/// complete file or the new complete file.
pub fn atomic_replace_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    atomic_replace_file_with_mode(path, bytes, None)
}

/// [`atomic_replace_file`] with an optional Unix mode for the newly-created file.
pub fn atomic_replace_file_with_mode(
    path: &Path,
    bytes: &[u8],
    mode: Option<u32>,
) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no parent directory", path.display()),
        )
    })?;
    ensure_directory(parent)?;
    reject_reparse(path)?;

    let temporary = create_temporary_file(path, mode)?;
    let result = (|| {
        let mut file = temporary.file;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        replace_existing(&temporary.path, path)?;
        sync_dir(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary.path);
    }
    result
}

/// Create a regular file without replacing an existing path, write it fully, and sync it.  This
/// is used for the Windows launcher, whose ownership contract forbids self-updating an existing
/// launcher.
pub fn create_new_file(path: &Path, bytes: &[u8], mode: Option<u32>) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no parent directory", path.display()),
        )
    })?;
    ensure_directory(parent)?;
    match fs::symlink_metadata(path) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("destination already exists: {}", path.display()),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let temporary = create_temporary_file(path, mode)?;
    let result = (|| {
        let mut file = temporary.file;
        file.write_all(bytes)?;
        file.flush()?;
        #[cfg(unix)]
        if let Some(mode) = mode {
            set_mode(&file, mode)?;
        }
        file.sync_all()?;
        drop(file);
        rename_new(&temporary.path, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary.path);
    }
    result
}

/// Change a payload into an executable without following a link.
pub fn set_executable(path: &Path) -> io::Result<()> {
    ensure_regular_file(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
        let mut options = OpenOptions::new();
        options.read(true).custom_flags(libc::O_NOFOLLOW);
        let file = options.open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a regular file", path.display()),
            ));
        }
        if metadata.nlink() > 1 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "refusing to change a hard-linked executable {}",
                    path.display()
                ),
            ));
        }
        let mode = metadata.mode() | 0o755;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

/// Atomically switch a Unix selector symlink to an absolute target.  This is not available on
/// Windows because the managed selector there is a regular UTF-8 file.
#[cfg(unix)]
pub fn atomic_switch_symlink(path: &Path, target: &Path) -> io::Result<()> {
    if !target.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "managed selector targets must be absolute",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no parent directory", path.display()),
        )
    })?;
    ensure_directory(parent)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not an existing selector symlink", path.display()),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let temporary = temporary_path(path, "symlink");
    std::os::unix::fs::symlink(target, &temporary)?;
    let result = (|| {
        replace_existing(&temporary, path)?;
        sync_dir(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Sync a directory after a rename or replacement.  Directory fsync is available on Unix; Windows
/// does not provide a portable directory handle contract, while `MOVEFILE_WRITE_THROUGH` and the
/// synced file cover the supported atomic-replacement path.
pub fn sync_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(windows)]
    {
        // Windows does not expose a portable directory-fsync operation.  The file itself is
        // flushed before replacement and MoveFileExW uses MOVEFILE_WRITE_THROUGH for the rename;
        // attempting FlushFileBuffers on a directory handle would fail on supported filesystems.
        let _ = path;
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Ok(())
    }
}

/// Rename a newly-created directory without intentionally replacing a prior version.  Callers
/// still check for existence first; the platform-specific no-replace operation prevents ordinary
/// races from silently merging or overwriting a retained version.
pub fn rename_new(source: &Path, destination: &Path) -> io::Result<()> {
    let parent = destination.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no parent directory", destination.display()),
        )
    })?;
    ensure_directory(parent)?;
    reject_reparse(source)?;
    reject_reparse(destination)?;
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
        let source_wide = wide_path(source);
        let destination_wide = wide_path(destination);
        let ok = unsafe {
            MoveFileExW(
                source_wide.as_ptr(),
                destination_wide.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        sync_dir(parent)
    }
    #[cfg(not(windows))]
    {
        #[cfg(target_os = "linux")]
        {
            use std::ffi::CString;
            use std::os::unix::ffi::OsStrExt;
            let source = CString::new(source.as_os_str().as_bytes()).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "source path contains NUL")
            })?;
            let destination = CString::new(destination.as_os_str().as_bytes()).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "destination path contains NUL")
            })?;
            let result = unsafe {
                libc::renameat2(
                    libc::AT_FDCWD,
                    source.as_ptr(),
                    libc::AT_FDCWD,
                    destination.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            if result != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        #[cfg(not(target_os = "linux"))]
        fs::rename(source, destination)?;
        sync_dir(parent)
    }
}

fn ensure_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || is_reparse_point(path)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is not a real directory", path.display()),
        ));
    }
    Ok(())
}

fn reject_reparse(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || is_reparse_point(path)? => {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("refusing to replace indirection {}", path.display()),
            ))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

struct TemporaryFile {
    path: PathBuf,
    file: File,
}

fn create_temporary_file(path: &Path, mode: Option<u32>) -> io::Result<TemporaryFile> {
    #[cfg(not(unix))]
    let _ = mode;
    let mut last_error = None;
    for _ in 0..32 {
        let temporary = temporary_path(path, "file");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        if let Some(mode) = mode {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(mode);
        }
        match options.open(&temporary) {
            Ok(file) => {
                return Ok(TemporaryFile {
                    path: temporary,
                    file,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => last_error = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "could not allocate a temporary path beside {}",
                path.display()
            ),
        )
    }))
}

fn temporary_path(path: &Path, kind: &str) -> PathBuf {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    path.with_file_name(format!(".{name}.{kind}.{pid}.{counter}.tmp"))
}

#[cfg(unix)]
fn set_mode(file: &File, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(mode))
}

#[cfg(windows)]
fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(windows)]
fn replace_existing(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let flags = MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH;
    if unsafe { MoveFileExW(source_wide.as_ptr(), destination_wide.as_ptr(), flags) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_existing(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_replacement_is_durable_and_replaces_only_regular_files() {
        let root = std::env::temp_dir().join(format!(
            "hyprmux-executable-test-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("state.json");
        atomic_replace_file(&path, b"one").unwrap();
        atomic_replace_file(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn create_new_file_never_replaces_an_existing_path() {
        let root = std::env::temp_dir().join(format!(
            "hyprmux-executable-create-new-test-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("launcher");
        fs::write(&path, b"original").unwrap();

        let error = create_new_file(&path, b"replacement", None).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&path).unwrap(), b"original");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn launcher_v1_derives_only_a_canonical_versioned_payload() {
        let launcher = Path::new("/managed/hyprmux/bin/hyprmux.exe");
        assert_eq!(
            resolve_launcher_v1_payload(launcher, b"0.2.0").unwrap(),
            PathBuf::from("/managed/hyprmux/versions/0.2.0/hyprmux.exe")
        );
        for invalid in [
            b"v0.2.0".as_slice(),
            b"0.2.0\n".as_slice(),
            b"../payload".as_slice(),
            b"0.2".as_slice(),
            b"".as_slice(),
        ] {
            assert!(resolve_launcher_v1_payload(launcher, invalid).is_err());
        }
        assert!(resolve_launcher_v1_payload(Path::new("/managed/hyprmux.exe"), b"0.2.0").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_switch_is_absolute_and_atomic() {
        let root = std::env::temp_dir().join(format!(
            "hyprmux-executable-link-test-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let target = root.join("payload");
        fs::write(&target, b"payload").unwrap();
        let pointer = root.join("hyprmux");
        atomic_switch_symlink(&pointer, &target).unwrap();
        assert_eq!(read_symlink(&pointer).unwrap(), Some(target));
        let _ = fs::remove_dir_all(root);
    }
}
