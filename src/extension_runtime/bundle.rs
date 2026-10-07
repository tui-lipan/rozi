//! Immutable, content-addressed snapshots of a loaded extension.
//!
//! A contribution that runs on a session host does not run the remote host's copy of the extension
//! (there may be none, or a different version). It runs the files this client loaded, carried there
//! as a bundle: every file under the extension directory, read once when the extension loads, in a
//! canonical encoding whose SHA-256 is the bundle's identity.
//!
//! The digest names *code*. It is deliberately separate from `ROZI_EXTENSION_GENERATION`, which
//! fences a *lifecycle*: two loads of identical files share a digest while getting different
//! generations, and a linked extension edited on disk gets a new digest (and so a new generation)
//! without its manifest changing.
//!
//! The canonical archive is also the wire format. A receiver writes the files out, then encodes
//! what it actually wrote and compares digests, so a bundle is only ever trusted for what is on
//! disk, never for what was said about it.

use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};

/// Version tag at the head of every archive, so a future encoding can never collide with this one.
const ARCHIVE_MAGIC: &[u8] = b"rozi-extension-bundle-v1\0";

/// Largest bundle Rozi will snapshot and stage. An extension is a few scripts; a directory past
/// this is carrying something (a virtualenv, `node_modules`, build output) that should be installed
/// on the host rather than copied there every time the extension changes.
pub const MAX_BUNDLE_BYTES: u64 = 32 * 1024 * 1024;

/// Most files a bundle may hold, for the same reason as [`MAX_BUNDLE_BYTES`].
pub const MAX_BUNDLE_FILES: usize = 4096;

/// Directory names never carried: version-control metadata and interpreter caches. They are not
/// part of what the extension *is*, and a cache an interpreter writes beside the code when the
/// local copy runs would otherwise change the digest - and restart the remote copy - on its own.
const EXCLUDED_DIRECTORIES: &[&str] = &[".git", ".hg", ".svn", "__pycache__"];

/// File suffixes never carried, for the same reason as [`EXCLUDED_DIRECTORIES`].
const EXCLUDED_SUFFIXES: &[&str] = &[".pyc", ".pyo"];

/// One regular file in a bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleFile {
    /// Relative path, `/`-separated, every component a plain name.
    pub path: String,
    pub executable: bool,
    pub contents: Vec<u8>,
}

/// The files of one loaded extension and their digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bundle {
    files: Vec<BundleFile>,
    digest: String,
    size: u64,
}

impl Bundle {
    /// Build a bundle from files already in memory. Sorts them into canonical order and refuses a
    /// path no host could write safely.
    pub fn from_files(mut files: Vec<BundleFile>) -> Result<Self, String> {
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let mut size = 0u64;
        for pair in files.windows(2) {
            if pair[0].path == pair[1].path {
                return Err(format!("bundle lists `{}` twice", pair[0].path));
            }
        }
        for file in &files {
            validate_relative_path(&file.path)?;
            size = size.saturating_add(file.contents.len() as u64);
        }
        // A file and a directory of the same name cannot both exist on disk.
        for pair in files.windows(2) {
            if pair[1]
                .path
                .strip_prefix(&pair[0].path)
                .is_some_and(|rest| rest.starts_with('/'))
            {
                return Err(format!(
                    "bundle has `{}` as both a file and a directory",
                    pair[0].path
                ));
            }
        }
        if files.len() > MAX_BUNDLE_FILES {
            return Err(format!(
                "extension has {} files; a bundle holds at most {MAX_BUNDLE_FILES}",
                files.len()
            ));
        }
        if size > MAX_BUNDLE_BYTES {
            return Err(format!(
                "extension is {size} bytes; a bundle holds at most {MAX_BUNDLE_BYTES}"
            ));
        }
        let digest = hex_digest(&encode(&files));
        Ok(Self {
            files,
            digest,
            size,
        })
    }

    /// Hex SHA-256 of the canonical archive.
    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn files(&self) -> &[BundleFile] {
        &self.files
    }

    /// Total bytes of file contents.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// The canonical archive: what is hashed and what crosses the wire.
    pub fn archive(&self) -> Vec<u8> {
        encode(&self.files)
    }

    /// Decode an archive received from a peer, verifying it against the digest it was sent under.
    pub fn from_archive(archive: &[u8], expected_digest: &str) -> Result<Self, String> {
        let files = decode(archive)?;
        let bundle = Self::from_files(files)?;
        if bundle.digest != expected_digest {
            return Err(format!(
                "bundle content does not match its digest (expected {expected_digest}, got {})",
                bundle.digest
            ));
        }
        Ok(bundle)
    }
}

/// Snapshot the extension installed at `root`.
///
/// Symbolic links are followed only to a regular file inside `root`, and carried as a copy of it:
/// a host has no business resolving a link the client wrote, and a link pointing out of the
/// extension would smuggle an unrelated file onto another machine. Any other link refuses the
/// whole bundle rather than leaving a hole in it.
pub fn snapshot(root: &Path) -> Result<Bundle, String> {
    let canonical_root = std::fs::canonicalize(root).map_err(|error| {
        format!(
            "cannot read extension directory {}: {error}",
            root.display()
        )
    })?;
    let mut files = Vec::new();
    let mut size = 0u64;
    walk(
        &canonical_root,
        &canonical_root,
        String::new(),
        &mut files,
        &mut size,
    )?;
    Bundle::from_files(files)
}

fn walk(
    root: &Path,
    directory: &Path,
    prefix: String,
    files: &mut Vec<BundleFile>,
    size: &mut u64,
) -> Result<(), String> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("cannot list {}: {error}", directory.display()))?;
    let mut entries: Vec<_> = entries
        .collect::<io::Result<_>>()
        .map_err(|error| format!("cannot list {}: {error}", directory.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(format!(
                "{} has a file name that is not UTF-8",
                directory.display()
            ));
        };
        let relative = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot read {relative}: {error}"))?;
        if metadata.is_dir() {
            if EXCLUDED_DIRECTORIES.contains(&name) {
                continue;
            }
            walk(root, &path, relative, files, size)?;
            continue;
        }
        if EXCLUDED_SUFFIXES
            .iter()
            .any(|suffix| name.ends_with(suffix))
        {
            continue;
        }
        let (source, metadata) = if metadata.file_type().is_symlink() {
            let target = std::fs::canonicalize(&path)
                .map_err(|error| format!("symbolic link {relative} is broken: {error}"))?;
            if !target.starts_with(root) {
                return Err(format!(
                    "symbolic link {relative} points outside the extension, to {}",
                    target.display()
                ));
            }
            let target_metadata = std::fs::metadata(&target)
                .map_err(|error| format!("cannot read {relative}: {error}"))?;
            if !target_metadata.is_file() {
                return Err(format!(
                    "symbolic link {relative} does not point to a regular file"
                ));
            }
            (target, target_metadata)
        } else if metadata.is_file() {
            (path, metadata)
        } else {
            return Err(format!("{relative} is not a regular file"));
        };
        *size = size.saturating_add(metadata.len());
        if *size > MAX_BUNDLE_BYTES {
            return Err(format!(
                "extension is larger than the {MAX_BUNDLE_BYTES}-byte bundle limit"
            ));
        }
        if files.len() >= MAX_BUNDLE_FILES {
            return Err(format!(
                "extension has more than the {MAX_BUNDLE_FILES}-file bundle limit"
            ));
        }
        let contents =
            std::fs::read(&source).map_err(|error| format!("cannot read {relative}: {error}"))?;
        let executable = is_executable(&metadata, &contents);
        files.push(BundleFile {
            path: relative,
            executable,
            contents,
        });
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata, _contents: &[u8]) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

/// Windows has no execute bit to read. A script that names its interpreter is meant to be run, so
/// that is what marks it executable for a host that does have the bit.
#[cfg(not(unix))]
fn is_executable(_metadata: &std::fs::Metadata, contents: &[u8]) -> bool {
    contents.starts_with(b"#!")
}

/// Refuse a path that is absolute, climbs, is empty, or would mean something different on another
/// operating system.
pub fn validate_relative_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("bundle has an empty path".to_string());
    }
    for component in path.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(format!("bundle path `{path}` is not a plain relative path"));
        }
        if component.contains(['\\', ':', '\0']) || component.chars().any(char::is_control) {
            return Err(format!(
                "bundle path `{path}` has a character another host cannot use in a file name"
            ));
        }
    }
    Ok(())
}

/// Join a validated bundle path onto a directory.
pub fn join_relative(root: &Path, path: &str) -> PathBuf {
    let mut joined = root.to_path_buf();
    for component in path.split('/') {
        joined.push(component);
    }
    joined
}

/// Whether `path` stays inside the bundle once `.` and `..` are resolved lexically.
pub fn is_contained(path: &str) -> bool {
    let mut depth = 0usize;
    for component in Path::new(path).components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => {
                if depth == 0 {
                    return false;
                }
                depth -= 1;
            }
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

fn encode(files: &[BundleFile]) -> Vec<u8> {
    let total: usize = files
        .iter()
        .map(|file| file.path.len() + file.contents.len() + 10)
        .sum();
    let mut out = Vec::with_capacity(ARCHIVE_MAGIC.len() + 8 + total);
    out.extend_from_slice(ARCHIVE_MAGIC);
    out.extend_from_slice(&(files.len() as u64).to_be_bytes());
    for file in files {
        out.extend_from_slice(file.path.as_bytes());
        out.push(0);
        out.push(if file.executable { b'x' } else { b'-' });
        out.extend_from_slice(&(file.contents.len() as u64).to_be_bytes());
        out.extend_from_slice(&file.contents);
    }
    out
}

fn decode(mut archive: &[u8]) -> Result<Vec<BundleFile>, String> {
    let malformed = || "bundle archive is malformed".to_string();
    archive = archive.strip_prefix(ARCHIVE_MAGIC).ok_or_else(malformed)?;
    let count = take_u64(&mut archive).ok_or_else(malformed)?;
    if count > MAX_BUNDLE_FILES as u64 {
        return Err(malformed());
    }
    let mut files = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let end = archive
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(malformed)?;
        let path = std::str::from_utf8(&archive[..end])
            .map_err(|_| malformed())?
            .to_string();
        archive = &archive[end + 1..];
        let (&flag, rest) = archive.split_first().ok_or_else(malformed)?;
        archive = rest;
        let executable = match flag {
            b'x' => true,
            b'-' => false,
            _ => return Err(malformed()),
        };
        let length = take_u64(&mut archive).ok_or_else(malformed)?;
        let length = usize::try_from(length).map_err(|_| malformed())?;
        if length > archive.len() {
            return Err(malformed());
        }
        let (contents, rest) = archive.split_at(length);
        archive = rest;
        files.push(BundleFile {
            path,
            executable,
            contents: contents.to_vec(),
        });
    }
    if !archive.is_empty() {
        return Err(malformed());
    }
    Ok(files)
}

fn take_u64(bytes: &mut &[u8]) -> Option<u64> {
    let (head, rest) = bytes.split_first_chunk::<8>()?;
    *bytes = rest;
    Some(u64::from_be_bytes(*head))
}

pub fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Whether `value` has the shape of a digest this module produces. A host names directories after
/// digests, so anything else is refused before it reaches a path.
pub fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// A shared snapshot. Cheap to clone into the config that carries it.
pub type SharedBundle = Arc<Bundle>;

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, relative: &str, contents: &[u8]) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn identical_trees_share_a_digest_and_any_change_moves_it() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        for root in [a.path(), b.path()] {
            write(root, "extension.toml", b"[extension]\n");
            write(root, "bin/run.py", b"print('hi')\n");
        }
        let first = snapshot(a.path()).unwrap();
        assert_eq!(first.digest(), snapshot(b.path()).unwrap().digest());
        assert!(is_digest(first.digest()));

        write(b.path(), "bin/run.py", b"print('bye')\n");
        assert_ne!(first.digest(), snapshot(b.path()).unwrap().digest());
    }

    #[test]
    fn version_control_and_interpreter_caches_are_not_part_of_the_bundle() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "extension.toml", b"x");
        let before = snapshot(root.path()).unwrap();
        write(root.path(), ".git/HEAD", b"ref: refs/heads/main\n");
        write(root.path(), "bin/__pycache__/run.cpython-314.pyc", b"\0");
        write(root.path(), "bin/stale.pyc", b"\0");
        let after = snapshot(root.path()).unwrap();
        assert_eq!(before.digest(), after.digest());
        assert_eq!(after.files().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_inside_the_extension_is_carried_as_a_copy_and_one_outside_refuses_the_bundle() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "bin/real.sh", b"#!/bin/sh\n");
        std::os::unix::fs::symlink("real.sh", root.path().join("bin/alias.sh")).unwrap();
        let bundle = snapshot(root.path()).unwrap();
        let alias = bundle
            .files()
            .iter()
            .find(|file| file.path == "bin/alias.sh")
            .unwrap();
        assert_eq!(alias.contents, b"#!/bin/sh\n");

        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "secret", b"key");
        std::os::unix::fs::symlink(outside.path().join("secret"), root.path().join("leak"))
            .unwrap();
        let error = snapshot(root.path()).unwrap_err();
        assert!(error.contains("points outside the extension"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_directory_refuses_the_bundle() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "lib/a.py", b"a");
        std::os::unix::fs::symlink("lib", root.path().join("again")).unwrap();
        let error = snapshot(root.path()).unwrap_err();
        assert!(error.contains("regular file"), "{error}");
    }

    #[test]
    fn the_archive_round_trips_and_is_checked_against_its_digest() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "a", b"one");
        write(root.path(), "dir/b", b"two");
        let bundle = snapshot(root.path()).unwrap();
        let archive = bundle.archive();
        assert_eq!(
            Bundle::from_archive(&archive, bundle.digest()).unwrap(),
            bundle
        );

        let mut tampered = archive.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        assert!(Bundle::from_archive(&tampered, bundle.digest()).is_err());
        assert!(Bundle::from_archive(&archive[..archive.len() - 1], bundle.digest()).is_err());
    }

    #[test]
    fn paths_that_climb_or_mean_something_else_elsewhere_are_refused() {
        for path in ["", "/abs", "../up", "a/../b", "a//b", "a\\b", "c:x", "./a"] {
            assert!(validate_relative_path(path).is_err(), "{path:?}");
        }
        validate_relative_path("bin/run.py").unwrap();
        let file = |path: &str| BundleFile {
            path: path.to_string(),
            executable: false,
            contents: Vec::new(),
        };
        assert!(Bundle::from_files(vec![file("a"), file("a/b")]).is_err());
        assert!(Bundle::from_files(vec![file("a"), file("a")]).is_err());
    }

    #[test]
    fn containment_is_lexical() {
        assert!(is_contained("bin/run"));
        assert!(is_contained("./bin/../run"));
        assert!(!is_contained("../run"));
        assert!(!is_contained("bin/../../run"));
        assert!(!is_contained("/etc/passwd"));
    }

    #[test]
    fn oversized_extensions_are_refused() {
        let file = BundleFile {
            path: "big".to_string(),
            executable: false,
            contents: vec![0; (MAX_BUNDLE_BYTES + 1) as usize],
        };
        assert!(Bundle::from_files(vec![file]).is_err());
    }
}
