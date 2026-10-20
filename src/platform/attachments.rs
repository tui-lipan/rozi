//! Private server-side attachment files and safe local file reads.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct StagedFile {
    pub path: PathBuf,
    pub size: u64,
    _directory: tempfile::TempDir,
}

impl StagedFile {
    pub fn create(name: &str, size: u64) -> io::Result<(Self, File)> {
        let directory = tempfile::Builder::new()
            .prefix("rozi-attachment-")
            .tempdir()?;
        let private = directory.path().join("files");
        super::fs_security::ensure_private_dir(&private)?;
        let name: String = name
            .chars()
            .take(120)
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let path = private.join(format!("attachment-{name}"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        Ok((
            Self {
                path,
                size,
                _directory: directory,
            },
            file,
        ))
    }
}

/// Reject symlinks, directories, devices and FIFOs rather than blocking or reading streams.
pub(crate) fn open_source(path: &Path) -> io::Result<File> {
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Err(io::Error::other(
            "attachment must be a regular file, not a symlink or directory",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("attachment must be a regular file"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if file.metadata()?.file_attributes() & 0x400 != 0 {
            return Err(io::Error::other("attachment must not be a reparse point"));
        }
    }
    Ok(file)
}

/// Parse one path emitted by a terminal drop, without evaluating shell syntax.
pub(crate) fn drop_path(text: &str) -> Option<PathBuf> {
    let text = if text.len() >= 2
        && (text.starts_with('\'') && text.ends_with('\'')
            || text.starts_with('"') && text.ends_with('"'))
    {
        &text[1..text.len() - 1]
    } else {
        text
    };
    #[cfg(unix)]
    let text = {
        let mut path = String::new();
        let mut chars = text.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                path.push(chars.next()?);
            } else {
                path.push(ch);
            }
        }
        path
    };
    Some(PathBuf::from(text))
}

/// Keep successive pasted paths separate and quote whitespace for terminal composers.
pub(crate) fn paste_path(path: &str) -> Result<String, String> {
    if path
        .chars()
        .any(|c| c.is_control() || matches!(c, '\"' | '\'' | '`' | '$'))
    {
        return Err("attachment temporary directory contains unsupported path characters".into());
    }
    if path.chars().any(char::is_whitespace) {
        Ok(format!("\"{path}\" "))
    } else {
        Ok(format!("{path} "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn staged_files_are_private_regular_and_removed_with_their_owner() {
        let (staged, mut file) = StagedFile::create("../../notes $x.txt", 3).unwrap();
        file.write_all(b"abc").unwrap();
        let path = staged.path.clone();
        assert_eq!(path.parent().unwrap().read_dir().unwrap().count(), 1);
        assert_eq!(std::fs::read(&path).unwrap(), b"abc");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(
                path.parent()
                    .unwrap()
                    .metadata()
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        drop(file);
        drop(staged);
        assert!(!path.exists());
    }
    #[test]
    fn source_rejects_directories_and_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        assert!(open_source(dir.path()).is_err());
        #[cfg(unix)]
        {
            let target = dir.path().join("real.txt");
            std::fs::write(&target, b"abc").unwrap();
            let link = dir.path().join("link.txt");
            std::os::unix::fs::symlink(target, &link).unwrap();
            assert!(open_source(&link).is_err());
        }
    }
    #[test]
    fn paths_are_separated_and_quoted_without_control_characters() {
        assert_eq!(paste_path("/tmp/photo.png").unwrap(), "/tmp/photo.png ");
        assert_eq!(
            paste_path("C:\\Some User\\photo.png").unwrap(),
            "\"C:\\Some User\\photo.png\" "
        );
        assert!(paste_path("/tmp/x\n.png").is_err());
    }
    #[test]
    fn drop_paths_unescape_only_where_backslash_is_not_a_separator() {
        assert_eq!(
            drop_path("\"/tmp/paste test.png\""),
            Some("/tmp/paste test.png".into())
        );
        #[cfg(unix)]
        {
            assert_eq!(
                drop_path("/tmp/paste\\ test.png"),
                Some("/tmp/paste test.png".into())
            );
            assert_eq!(drop_path("/tmp/trailing\\"), None);
        }
        #[cfg(windows)]
        assert_eq!(
            drop_path("C:\\Some User\\photo.png"),
            Some("C:\\Some User\\photo.png".into())
        );
    }
}
