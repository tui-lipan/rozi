//! Opening a local file with the platform's default application.

use std::path::Path;

/// Open a local file with the platform's default application.
pub fn open_file(path: &Path) -> std::io::Result<()> {
    open::that_detached(path)
}

/// Whether [`open_file`] would start `path` as a program rather than show it.
///
/// A pane can print any path, so a link opened with no handler must never be a way to run one.
pub fn runs_when_opened(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some(extension) => program_extension(extension),
        None => extensionless_program(path),
    }
}

#[cfg(windows)]
fn program_extension(extension: &str) -> bool {
    const ALWAYS: &[&str] = &[
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
        "js",
        "jse",
        "wsf",
        "wsh",
        "hta",
        "cpl",
        "reg",
        "jar",
        "application",
        "appref-ms",
    ];
    ALWAYS.contains(&extension)
        || std::env::var("PATHEXT").is_ok_and(|registered| {
            registered.split(';').any(|entry| {
                entry
                    .trim_start_matches('.')
                    .eq_ignore_ascii_case(extension)
            })
        })
}

#[cfg(target_os = "macos")]
fn program_extension(extension: &str) -> bool {
    matches!(
        extension,
        "app" | "command" | "tool" | "terminal" | "workflow" | "action" | "scpt" | "scptd"
    )
}

#[cfg(not(any(windows, target_os = "macos")))]
fn program_extension(extension: &str) -> bool {
    matches!(extension, "desktop" | "appimage")
}

/// macOS runs an executable file with no extension in Terminal.
#[cfg(target_os = "macos")]
fn extensionless_program(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(target_os = "macos"))]
fn extensionless_program(_path: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_told_from_a_document_whatever_its_case() {
        let program = if cfg!(windows) {
            "Setup.EXE"
        } else if cfg!(target_os = "macos") {
            "Install.Command"
        } else {
            "Launcher.Desktop"
        };
        assert!(runs_when_opened(Path::new(program)));
        assert!(!runs_when_opened(Path::new("notes.md")));
    }
}
