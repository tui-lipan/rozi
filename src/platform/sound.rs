//! Cross-platform best-effort alert-cue playback.
//!
//! | Platform | Mechanism |
//! |---|---|
//! | Linux/BSD | first available of `pw-play`, `paplay`, `aplay`, `canberra-gtk-play` |
//! | macOS | `afplay` |
//! | Windows | PowerShell `System.Media.SoundPlayer.PlaySync()` |

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Cue {
    Bell,
    Blocked,
    Done,
    Error,
}

impl Cue {
    fn file_name(self) -> &'static str {
        match self {
            Self::Bell => "bell.wav",
            Self::Blocked => "blocked.wav",
            Self::Done => "done.wav",
            Self::Error => "error.wav",
        }
    }
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Bell => include_bytes!("../../assets/sounds/bell.wav"),
            Self::Blocked => include_bytes!("../../assets/sounds/blocked.wav"),
            Self::Done => include_bytes!("../../assets/sounds/done.wav"),
            Self::Error => include_bytes!("../../assets/sounds/error.wav"),
        }
    }
}

/// Play a cue off-thread. Failures and hosts without a player are deliberately silent.
pub fn play(cue: Cue, override_file: Option<&Path>, player: Option<&str>) {
    let override_file = override_file.map(Path::to_path_buf);
    let player = player.map(str::to_string);
    std::thread::spawn(move || {
        let path = override_file.unwrap_or_else(|| extract(cue));
        let _ = show(&path, player.as_deref());
    });
}

fn extract(cue: Cue) -> PathBuf {
    let path = super::paths::cache_dir(&super::paths::PlatformEnv::from_process())
        .join("sounds")
        .join(cue.file_name());
    let bytes = cue.bytes();
    if std::fs::metadata(&path)
        .map(|meta| meta.len() as usize != bytes.len())
        .unwrap_or(true)
    {
        let _ = std::fs::create_dir_all(path.parent().expect("sound cache has parent"));
        let _ = std::fs::write(&path, bytes);
    }
    path
}

#[cfg(target_os = "macos")]
fn show(path: &Path, player: Option<&str>) -> std::io::Result<()> {
    let mut command = std::process::Command::new(player.unwrap_or("afplay"));
    command.arg(path).status().map(|_| ())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn show(path: &Path, player: Option<&str>) -> std::io::Result<()> {
    if let Some(player) = player {
        return std::process::Command::new(player)
            .arg(path)
            .status()
            .map(|_| ());
    }
    let player = ["pw-play", "paplay", "aplay", "canberra-gtk-play"]
        .into_iter()
        .find(|player| super::command::program_exists(player))
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no sound player"))?;
    let mut command = std::process::Command::new(player);
    if player == "canberra-gtk-play" {
        command.arg("--file");
    }
    command.arg(path).status().map(|_| ())
}

#[cfg(windows)]
fn show(path: &Path, player: Option<&str>) -> std::io::Result<()> {
    if let Some(player) = player {
        let mut command = hidden_command(player);
        return command.arg(path).status().map(|_| ());
    }
    const SCRIPT: &str = "(New-Object System.Media.SoundPlayer $env:ROZI_SOUND_FILE).PlaySync()";
    let player = super::command::lookup_program("powershell.exe")
        .or_else(|| super::command::lookup_program("pwsh.exe"))
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no PowerShell available for sound",
            )
        })?;
    let mut command = hidden_command(player);
    command
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg(SCRIPT)
        .env("ROZI_SOUND_FILE", path);
    command.status().map(|_| ())
}

#[cfg(windows)]
fn hidden_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extraction_is_nonempty_and_idempotent() {
        for cue in [Cue::Bell, Cue::Blocked, Cue::Done, Cue::Error] {
            let path = extract(cue);
            let first = std::fs::metadata(&path).unwrap().len();
            assert!(first > 0);
            assert_eq!(std::fs::metadata(extract(cue)).unwrap().len(), first);
        }
    }
}
