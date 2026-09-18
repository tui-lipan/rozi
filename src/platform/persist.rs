//! Atomic file replacement for user-owned state.
//!
//! Callers write a complete temporary sibling and then replace the destination, so an interruption
//! leaves either the previous complete file or the new complete file, never a truncated mix.
//! A symlink at the destination is followed: the target is replaced and the link itself stays.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_SYMLINK_FOLLOW: usize = 8;

/// Write `contents` so `path` is either its previous complete value or this complete value.
///
/// If `path` is a symlink, the target file is replaced and the symlink remains. Parent directories
/// must already exist.
pub fn replace_file(path: &Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let dest = follow_leaf_symlinks(path)?;
    let parent = dest
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let temporary = write_temporary_sibling(parent, &dest, contents.as_ref())?;
    match replace_existing(&temporary, &dest) {
        Ok(()) => {
            sync_parent_dir(parent);
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

fn follow_leaf_symlinks(path: &Path) -> io::Result<PathBuf> {
    let mut current = path.to_path_buf();
    for _ in 0..MAX_SYMLINK_FOLLOW {
        match fs::symlink_metadata(&current) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(current),
            Err(error) => return Err(error),
            Ok(metadata) if !metadata.file_type().is_symlink() => return Ok(current),
            Ok(_) => {
                let target = fs::read_link(&current)?;
                current = if target.is_absolute() {
                    target
                } else if let Some(parent) = current.parent() {
                    parent.join(target)
                } else {
                    target
                };
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{} has too many symlink hops", path.display()),
    ))
}

fn write_temporary_sibling(parent: &Path, dest: &Path, contents: &[u8]) -> io::Result<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let stem = dest
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("file"));
    loop {
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(
            ".{}.{}.{}.tmp",
            stem.to_string_lossy(),
            std::process::id(),
            unique
        ));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        copy_existing_permissions(dest, &file);
        file.write_all(contents)?;
        file.sync_all()?;
        return Ok(temporary);
    }
}

fn copy_existing_permissions(dest: &Path, file: &fs::File) {
    if let Ok(metadata) = fs::metadata(dest) {
        let _ = file.set_permissions(metadata.permissions());
    }
}

fn sync_parent_dir(parent: &Path) {
    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
}

#[cfg(unix)]
fn replace_existing(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

#[cfg(windows)]
fn replace_existing(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    let from_w = wide(from);
    let to_w = wide(to);
    let ok = unsafe {
        MoveFileExW(
            from_w.as_ptr(),
            to_w.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(1);

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rozi-persist-{label}-{}-{}",
            std::process::id(),
            NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn replace_file_creates_and_overwrites_a_regular_file() {
        let dir = scratch("regular");
        let path = dir.join("state.txt");
        replace_file(&path, b"first\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first\n");
        replace_file(&path, b"second\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second\n");
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn replace_file_writes_through_a_symlink() {
        let dir = scratch("symlink");
        let target = dir.join("real.toml");
        let link = dir.join("config.toml");
        fs::write(&target, "old\n").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        replace_file(&link, b"new\n").unwrap();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the config path must stay a symlink"
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "new\n");
        assert_eq!(fs::read_to_string(&link).unwrap(), "new\n");
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn replace_file_follows_a_relative_symlink() {
        let dir = scratch("relative-link");
        fs::write(dir.join("real.toml"), "old\n").unwrap();
        std::os::unix::fs::symlink("real.toml", dir.join("config.toml")).unwrap();

        replace_file(&dir.join("config.toml"), b"new\n").unwrap();

        assert!(
            fs::symlink_metadata(dir.join("config.toml"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(dir.join("real.toml")).unwrap(), "new\n");
        let _ = fs::remove_dir_all(dir);
    }
}
