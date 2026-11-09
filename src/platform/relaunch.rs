//! Replacing the running client process with another rozi binary.
//!
//! Unix replaces the process image: the new build keeps this process's id, its parent, and its
//! controlling terminal, so the shell or terminal emulator that started rozi never sees it exit.
//! Windows has no such call. There the new build runs as a child that inherits the console, and
//! this process waits for it and leaves with its exit code, which is what the shell that started
//! rozi then sees.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Enough of a file's identity to notice that the binary at a path was replaced: the same path
/// naming a different file, or the same file rewritten.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileIdentity {
    len: u64,
    modified: Option<SystemTime>,
    /// Device and inode on Unix, where they identify the file itself rather than its contents.
    node: Option<(u64, u64)>,
}

/// The identity of the file `path` resolves to, following symlinks the way running it would.
pub fn file_identity(path: &Path) -> io::Result<FileIdentity> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a file", path.display()),
        ));
    }
    Ok(FileIdentity {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        node: node(&metadata),
    })
}

#[cfg(unix)]
fn node(metadata: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn node(_metadata: &std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

/// The file name a rozi binary has on this platform.
pub fn executable_name() -> &'static str {
    if cfg!(windows) { "rozi.exe" } else { "rozi" }
}

/// The `rozi` a shell would run, found on `PATH`.
#[cfg(windows)]
pub fn on_path() -> Option<PathBuf> {
    crate::platform::command::lookup_program("rozi")
}

/// The `rozi` a shell would run, found on `PATH`.
#[cfg(not(windows))]
pub fn on_path() -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(executable_name()))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

/// The path the running binary was started from, even after an update unlinked it.
///
/// Linux reports an unlinked executable as `<path> (deleted)`. The path without the suffix is where
/// the install keeps putting rozi, which is exactly where the build that replaced it is.
pub fn launched_from() -> Option<PathBuf> {
    let path = std::env::current_exe().ok()?;
    let text = path.to_str()?;
    Some(
        text.strip_suffix(" (deleted)")
            .map(PathBuf::from)
            .unwrap_or(path),
    )
}

/// Run `executable` with `args` in place of this process.
///
/// On Unix this returns only when the replacement could not start; on success the process image is
/// already the new build's. Elsewhere it returns the replacement's exit code once it ends, for this
/// process to leave with.
pub fn replace_process(executable: &Path, args: &[OsString]) -> io::Result<i32> {
    let mut command = std::process::Command::new(executable);
    command.args(args);
    run(command)
}

#[cfg(unix)]
fn run(mut command: std::process::Command) -> io::Result<i32> {
    use std::os::unix::process::CommandExt;
    Err(command.exec())
}

#[cfg(not(unix))]
fn run(mut command: std::process::Command) -> io::Result<i32> {
    let status = command.status()?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewriting_a_file_changes_its_identity() {
        let dir = crate::test_support::private_temp_dir("relaunch-identity");
        let path = dir.join(executable_name());
        std::fs::write(&path, b"first build").unwrap();
        let before = file_identity(&path).unwrap();
        assert_eq!(file_identity(&path).unwrap(), before);

        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"the second, longer build").unwrap();
        assert_ne!(file_identity(&path).unwrap(), before);
        assert!(file_identity(&dir).is_err(), "a directory is not a binary");
    }
}
