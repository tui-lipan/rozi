//! Filesystem operations for installed extensions.

use std::fs;
use std::path::Path;

/// Copy an extension tree without following symbolic links found inside it.
pub(crate) fn copy_directory(source: &Path, destination: &Path) -> Result<(), String> {
    let metadata = fs::metadata(source)
        .map_err(|error| format!("Could not inspect source {}: {error}", source.display()))?;
    if !metadata.is_dir() {
        return Err(format!(
            "Extension source is not a directory: {}",
            source.display()
        ));
    }
    fs::create_dir(destination).map_err(|error| {
        format!(
            "Could not create installation staging directory {}: {error}",
            destination.display()
        )
    })?;
    copy_directory_contents(source, destination)?;
    fs::set_permissions(destination, metadata.permissions()).map_err(|error| {
        format!(
            "Could not preserve permissions on {}: {error}",
            destination.display()
        )
    })
}

fn copy_directory_contents(source: &Path, destination: &Path) -> Result<(), String> {
    let entries = fs::read_dir(source)
        .map_err(|error| format!("Could not read source {}: {error}", source.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("Could not read an entry in {}: {error}", source.display()))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type().map_err(|error| {
            format!(
                "Could not inspect source {}: {error}",
                source_path.display()
            )
        })?;
        if file_type.is_symlink() {
            copy_symlink(&source_path, &destination_path)?;
        } else if file_type.is_dir() {
            copy_directory(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &destination_path).map_err(|error| {
                format!(
                    "Could not copy {} to {}: {error}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        } else {
            return Err(format!(
                "Unsupported filesystem entry in extension source: {}",
                source_path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn copy_symlink(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::unix::fs::symlink;

    let target = fs::read_link(source)
        .map_err(|error| format!("Could not read link {}: {error}", source.display()))?;
    symlink(&target, destination).map_err(|error| {
        format!(
            "Could not copy link {} to {}: {error}",
            source.display(),
            destination.display()
        )
    })
}

#[cfg(windows)]
fn copy_symlink(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::fs::{symlink_dir, symlink_file};

    let target = fs::read_link(source)
        .map_err(|error| format!("Could not read link {}: {error}", source.display()))?;
    let resolved = if target.is_absolute() {
        target.clone()
    } else {
        source
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(&target)
    };
    let target_metadata = fs::metadata(&resolved).map_err(|error| {
        format!(
            "Could not inspect link target for {}: {error}",
            source.display()
        )
    })?;
    let result = if target_metadata.is_dir() {
        symlink_dir(&target, destination)
    } else {
        symlink_file(&target, destination)
    };
    result.map_err(|error| {
        format!(
            "Could not copy link {} to {}: {error}",
            source.display(),
            destination.display()
        )
    })
}

/// Create the direct installation link used by `extensions install --link`.
pub(crate) fn create_directory_link(target: &Path, link: &Path) -> Result<(), String> {
    #[cfg(unix)]
    let result = std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_dir(target, link);
    result.map_err(|error| {
        format!(
            "Could not link extension {} to {}: {error}",
            target.display(),
            link.display()
        )
    })
}

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
