//! Filesystem operations for installed extensions.

use std::fs;
use std::path::Path;

/// Resolve a diagnostic path back to the direct filesystem entry that produced it.
///
/// Extension diagnostics are a public UTF-8 contract and therefore store paths lossily. Re-read
/// the parent directory when that representation cannot be addressed directly, preserving native
/// path bytes for manager operations.
pub(crate) fn resolve_installation_path(
    root: &Path,
    displayed: &str,
) -> Result<std::path::PathBuf, String> {
    let direct = std::path::PathBuf::from(displayed);
    if fs::symlink_metadata(&direct).is_ok() {
        return Ok(direct);
    }
    let mut matches = fs::read_dir(root)
        .map_err(|error| format!("Could not read extensions directory: {error}"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.to_string_lossy() == displayed);
    let Some(path) = matches.next() else {
        return Err(format!("Could not find extension installation {displayed}"));
    };
    if matches.next().is_some() {
        return Err(format!(
            "Refusing ambiguous extension path representation {displayed}"
        ));
    }
    Ok(path)
}

/// Remove one direct child of the extensions directory without following an installation symlink.
pub(crate) fn remove_installation(root: &Path, path: &Path) -> Result<(), String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("Could not resolve extensions directory: {error}"))?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect extension {}: {error}", path.display()))?;

    if metadata.file_type().is_symlink() {
        let parent = path
            .parent()
            .ok_or_else(|| "Extension path has no parent".to_string())?
            .canonicalize()
            .map_err(|error| format!("Could not resolve extension parent: {error}"))?;
        if parent != root {
            return Err("Refusing to remove an extension outside the extensions directory".into());
        }
        return remove_symlink(path);
    }

    let canonical = path
        .canonicalize()
        .map_err(|error| format!("Could not resolve extension {}: {error}", path.display()))?;
    if canonical == root || !canonical.starts_with(&root) {
        return Err("Refusing to remove an extension outside the extensions directory".into());
    }
    if !metadata.is_dir() {
        return Err(format!(
            "Extension installation is not a directory: {}",
            path.display()
        ));
    }
    fs::remove_dir_all(&canonical)
        .map_err(|error| format!("Could not remove extension {}: {error}", path.display()))
}

fn remove_symlink(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(file_error) => fs::remove_dir(path).map_err(|dir_error| {
            format!(
                "Could not remove extension link {}: {file_error}; {dir_error}",
                path.display()
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removal_rejects_paths_outside_the_extensions_root() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        assert!(remove_installation(root.path(), outside.path()).is_err());
        assert!(outside.path().exists());
    }

    #[test]
    fn removal_deletes_an_installation_inside_the_extensions_root() {
        let root = tempfile::tempdir().unwrap();
        let extension = root.path().join("tasks");
        fs::create_dir(&extension).unwrap();
        fs::write(extension.join("extension.toml"), "[extension]\n").unwrap();
        remove_installation(root.path(), &extension).unwrap();
        assert!(!extension.exists());
    }

    #[cfg(unix)]
    #[test]
    fn removal_unlinks_a_development_checkout_without_following_it() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        fs::write(checkout.path().join("keep"), "source").unwrap();
        let link = root.path().join("tasks");
        symlink(checkout.path(), &link).unwrap();

        remove_installation(root.path(), &link).unwrap();
        assert!(fs::symlink_metadata(&link).is_err());
        assert!(checkout.path().join("keep").exists());
    }

    #[cfg(unix)]
    #[test]
    fn lossy_diagnostic_path_resolves_to_the_native_installation_path() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let root = tempfile::tempdir().unwrap();
        let extension = root.path().join(OsString::from_vec(b"tasks-\xff".to_vec()));
        fs::create_dir(&extension).unwrap();
        let displayed = extension.to_string_lossy();
        assert_eq!(
            resolve_installation_path(root.path(), &displayed).unwrap(),
            extension
        );
    }
}
