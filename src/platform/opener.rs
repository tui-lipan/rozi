//! Opening a local file with the platform's default application.

use std::path::Path;

/// Open a local file with the platform's default application.
pub fn open_file(path: &Path) -> std::io::Result<()> {
    open::that_detached(path)
}

/// Whether [`open_file`] could start `path` as a program rather than show it.
///
/// A pane can print any path, so a link opened with no handler must not be a way to run one. What
/// a desktop does with a file is its own configuration, so this errs towards refusing: a file is a
/// program by its name, as the platform would read it, or by being executable and holding code.
pub fn runs_when_opened(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        // A name that is not Unicode cannot be told from a program's.
        return path.file_name().is_some();
    };
    // Windows drops trailing periods and spaces when it creates or opens a file, so `tool.exe.`
    // is `tool.exe` there. A copy of a remote file is named by its host, whatever that runs.
    let name = name.trim_end_matches(['.', ' ']);
    let extension = name
        .rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .map(|(_, extension)| extension.to_ascii_lowercase());
    extension.as_deref().is_some_and(program_extension)
        || executable_code(path, extension.is_some())
}

/// Whether probing `path` could reach over the network, which the UI thread must not wait for.
pub fn reaches_network(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        matches!(
            path.components().next(),
            Some(Component::Prefix(prefix)) if !matches!(prefix.kind(), Prefix::Disk(_))
        )
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

/// Extensions some desktop starts rather than shows, on any platform: a remote file's name was
/// chosen on another machine, and Wine and Java register theirs on Unix desktops.
const PROGRAM_EXTENSIONS: &[&str] = &[
    "exe",
    "com",
    "bat",
    "cmd",
    "msi",
    "msp",
    "msc",
    "scr",
    "pif",
    "lnk",
    "url",
    "ps1",
    "vbs",
    "vbe",
    "jse",
    "wsf",
    "wsh",
    "hta",
    "cpl",
    "reg",
    "jar",
    "application",
    "appref-ms",
    "app",
    "command",
    "tool",
    "terminal",
    "workflow",
    "action",
    "scpt",
    "scptd",
    "desktop",
    "appimage",
    "run",
];

fn program_extension(extension: &str) -> bool {
    PROGRAM_EXTENSIONS.contains(&extension) || registered_program_extension(extension)
}

/// Windows runs whatever `PATHEXT` lists, which includes `.js` by default.
#[cfg(windows)]
fn registered_program_extension(extension: &str) -> bool {
    extension == "js"
        || std::env::var("PATHEXT").is_ok_and(|registered| {
            registered.split(';').any(|entry| {
                entry
                    .trim_start_matches('.')
                    .eq_ignore_ascii_case(extension)
            })
        })
}

#[cfg(not(windows))]
fn registered_program_extension(_extension: &str) -> bool {
    false
}

/// Whether `path` is an executable file that holds a binary or a script. An executable file with
/// no extension counts whatever it holds: macOS runs one in Terminal.
#[cfg(unix)]
fn executable_code(path: &Path, has_extension: bool) -> bool {
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;

    let executable = std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0);
    if !executable {
        return false;
    }
    if !has_extension && cfg!(target_os = "macos") {
        return true;
    }
    let mut magic = [0u8; 4];
    let read = std::fs::File::open(path)
        .and_then(|mut file| file.read(&mut magic))
        .unwrap_or(0);
    let magic = &magic[..read];
    magic.starts_with(b"#!")
        || matches!(
            magic,
            // ELF, then Mach-O in each byte order and width, then a universal binary.
            b"\x7fELF"
                | [0xfe, 0xed, 0xfa, 0xce | 0xcf]
                | [0xce | 0xcf, 0xfa, 0xed, 0xfe]
                | [0xca, 0xfe, 0xba, 0xbe]
        )
}

#[cfg(not(unix))]
fn executable_code(_path: &Path, _has_extension: bool) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_told_from_a_document_by_the_name_the_platform_reads() {
        for program in [
            "Setup.EXE",
            "evil.exe.",
            "evil.cmd ",
            "Evil.Bat. .",
            "tool.jar",
            "Launcher.Desktop",
            "Install.Command",
        ] {
            assert!(runs_when_opened(Path::new(program)), "{program}");
        }
        for document in ["notes.md", "plot.png", ".bashrc", "archive.tar.gz"] {
            assert!(!runs_when_opened(Path::new(document)), "{document}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_executable_file_holding_code_is_a_program_whatever_its_name() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = tempfile::tempdir().unwrap();
        let file = |name: &str, contents: &[u8], mode| {
            let path = scratch.path().join(name);
            std::fs::write(&path, contents).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            path
        };
        assert!(runs_when_opened(&file("tool", b"\x7fELF\x02", 0o755)));
        assert!(runs_when_opened(&file("setup.sh", b"#!/bin/sh\n", 0o755)));
        // Not executable, so nothing can start it; and executable in name only, as every file is
        // on a file system without permissions.
        assert!(!runs_when_opened(&file("lib.sh", b"#!/bin/sh\n", 0o644)));
        assert!(!runs_when_opened(&file("photo.png", b"\x89PNG", 0o755)));
    }
}
