//! The installation store's revision: one token that every change to the installed set rotates.
//!
//! Installing, updating, and removing an extension can happen in any process: the CLI, or the
//! Extensions overlay of any client. The installed set belongs to the user, not to one client, so
//! every running client must converge on it. Each client watches this single file instead of the
//! extension directories, and reloads when the token differs from the one its last extension scan
//! saw. A writer therefore never needs to know which clients exist, and a new way of changing the
//! store only has to call [`bump`].
//!
//! The token is a change marker, not authentication. Edits made directly inside a linked checkout
//! or by hand do not rotate it; those still take an explicit `reload-extensions`.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Rozi's own metadata directory inside the extensions directory. Discovery skips it.
pub const CONTROL_DIRECTORY: &str = ".rozi";

const REVISION_FILE: &str = "revision";

/// The revision this process's most recent extension scan started from.
static LAST_SEEN: Mutex<Option<String>> = Mutex::new(None);

/// Where the revision file lives for the extensions directory `root`.
pub fn path_in(root: &Path) -> PathBuf {
    root.join(CONTROL_DIRECTORY).join(REVISION_FILE)
}

/// Whether `path` names the revision file, under whatever directory spelling an event reports.
pub fn is_revision_file(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == REVISION_FILE)
}

/// The current revision of the store at `root`, or `None` when it has never changed.
pub fn read_in(root: &Path) -> Option<String> {
    std::fs::read_to_string(path_in(root))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// Create the store's control directory, privately, so it can be watched before anything has been
/// installed. Returns the directory.
pub fn ensure_control_dir_in(root: &Path) -> io::Result<PathBuf> {
    let data_root = root
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| {
            io::Error::other(format!(
                "extension storage has no parent: {}",
                root.display()
            ))
        })?;
    let control = root.join(CONTROL_DIRECTORY);
    for directory in [data_root, root, control.as_path()] {
        crate::platform::fs_security::ensure_private_dir(directory)?;
    }
    Ok(control)
}

/// Rotate the revision of the store at `root`, telling every running client to reload.
pub fn bump_in(root: &Path) -> Result<(), String> {
    let control = ensure_control_dir_in(root).map_err(|error| {
        format!(
            "Could not prepare extension metadata storage {}: {error}",
            root.join(CONTROL_DIRECTORY).display()
        )
    })?;
    let path = control.join(REVISION_FILE);
    let token = format!("{}\n", super::runtime::fresh_token());
    crate::platform::persist::replace_file(&path, token).map_err(|error| {
        format!(
            "Could not record the extension change in {}: {error}",
            path.display()
        )
    })
}

/// Rotate the revision of the user's extension store.
pub fn bump() -> Result<(), String> {
    bump_in(&super::extensions_dir_path())
}

/// The current revision of the user's extension store.
pub fn read() -> Option<String> {
    read_in(&super::extensions_dir_path())
}

/// Record the revision an extension scan is about to read. Called before the scan, so a change
/// that lands during it still counts as unseen.
pub fn note(revision: Option<String>) {
    *LAST_SEEN
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = revision;
}

/// Whether the store changed since this process last scanned it.
pub fn changed_since_last_scan() -> bool {
    let current = read();
    *LAST_SEEN
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        != current
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(temp: &tempfile::TempDir) -> PathBuf {
        temp.path().join("data").join("extensions")
    }

    #[test]
    fn a_store_that_never_changed_has_no_revision() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(read_in(&store(&temp)), None);
    }

    #[test]
    fn every_bump_rotates_the_revision() {
        let temp = tempfile::tempdir().unwrap();
        let root = store(&temp);
        bump_in(&root).unwrap();
        let first = read_in(&root).expect("bump writes a revision");
        bump_in(&root).unwrap();
        let second = read_in(&root).expect("bump writes a revision");
        assert_ne!(first, second);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn the_control_directory_exists_before_anything_is_installed() {
        let temp = tempfile::tempdir().unwrap();
        let root = store(&temp);
        let control = ensure_control_dir_in(&root).unwrap();
        assert!(control.is_dir());
        assert_eq!(control, root.join(CONTROL_DIRECTORY));
    }

    #[test]
    fn only_the_revision_file_counts_as_a_change() {
        let root = PathBuf::from("extensions");
        assert!(is_revision_file(&path_in(&root)));
        assert!(!is_revision_file(
            &root.join(CONTROL_DIRECTORY).join(".revision.1.1.tmp")
        ));
        assert!(!is_revision_file(
            &root.join(CONTROL_DIRECTORY).join("installations")
        ));
    }
}
