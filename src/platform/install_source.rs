//! Which distribution channel produced the running `rozi`.
//!
//! `relswap` updates installs it created and rejects every other one with
//! [`relswap::InstallError::Unmanaged`]. Refusing is correct - the managed layout owns version
//! retention, activation, and rollback, and none of that exists for a binary some other tool
//! placed on `PATH` - but "managed installation is not present" is a dead end for the user who
//! installed through cargo, mise, Homebrew, or a distribution package. Naming the channel turns
//! that into an instruction they can act on.
//!
//! Detection is a pure function of the executable path and a snapshot of the few environment
//! variables these channels relocate themselves with. Nothing here touches the filesystem or the
//! process environment, so the table below is exercised directly by unit tests rather than
//! inferred from whatever produced the test runner.
//!
//! This is a hint, never an authority. An unrecognised layout is [`InstallSource::Unknown`], which
//! points at the documentation rather than guessing a command that might do something else.

use std::path::{Path, PathBuf};

use crate::platform::paths::{self, PlatformEnv};

/// Environment inputs that relocate a distribution channel away from its default path.
///
/// Separate from [`PlatformEnv`], which describes where *rozi* keeps user data. These variables
/// describe where *other tools* keep theirs, and only this module has any use for them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceEnv {
    /// `$HOME`, for the default locations the variables below override.
    pub home: Option<PathBuf>,
    /// `$CARGO_HOME`, else `~/.cargo`.
    pub cargo_home: Option<PathBuf>,
    /// `$MISE_DATA_DIR`, else `~/.local/share/mise`.
    pub mise_data_dir: Option<PathBuf>,
    /// `$HOMEBREW_PREFIX`, else one of the well-known prefixes.
    pub homebrew_prefix: Option<PathBuf>,
    /// Rozi's own managed data root, so a managed install is recognised as such.
    pub managed_root: Option<PathBuf>,
}

impl SourceEnv {
    /// Snapshot the process environment, taking rozi's managed root from `env`.
    pub fn from_platform_env(env: &PlatformEnv) -> Self {
        Self {
            home: env.home.clone(),
            cargo_home: absolute_env("CARGO_HOME"),
            mise_data_dir: absolute_env("MISE_DATA_DIR"),
            homebrew_prefix: absolute_env("HOMEBREW_PREFIX"),
            managed_root: Some(paths::data_dir(env)),
        }
    }
}

fn absolute_env(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

/// The channel a `rozi` binary most likely came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallSource {
    /// Installed by rozi itself, into the `relswap` managed layout.
    Managed,
    /// `cargo install rozi`, or `cargo binstall`.
    Cargo,
    Mise,
    Homebrew,
    Scoop,
    WinGet,
    /// A distribution package: the binary sits in a prefix a system package manager owns.
    SystemPackage,
    /// An unrecognised layout - a manual copy, a build directory, or a channel not listed here.
    Unknown,
}

impl InstallSource {
    /// Classify `executable` against `env`.
    ///
    /// First match wins, most specific first. The managed layout is checked ahead of everything
    /// else because a managed root can legitimately sit inside a directory another channel also
    /// uses.
    pub fn detect(executable: &Path, env: &SourceEnv) -> Self {
        if let Some(root) = &env.managed_root
            && executable.starts_with(root)
        {
            return Self::Managed;
        }
        if within_any(executable, [cargo_bin(env)]) {
            return Self::Cargo;
        }
        if within_any(executable, [mise_installs(env)]) || adjacent(executable, "mise", "installs")
        {
            return Self::Mise;
        }
        if within_any(executable, homebrew_prefixes(env)) {
            return Self::Homebrew;
        }
        if within_any(executable, [scoop_apps(env)]) || adjacent(executable, "scoop", "apps") {
            return Self::Scoop;
        }
        if adjacent(executable, "WinGet", "Packages") {
            return Self::WinGet;
        }
        // Deliberately not `/usr/local/bin`: a distribution owns `/usr/bin`, but `/usr/local/bin`
        // is equally the place people copy a binary by hand, and telling those users that a
        // package manager owns it would be wrong. They fall through to `Unknown`.
        if within_any(executable, [Some(PathBuf::from("/usr/bin"))]) {
            return Self::SystemPackage;
        }
        Self::Unknown
    }

    /// The command that updates an install from this channel, when one command covers it.
    pub fn upgrade_command(self) -> Option<&'static str> {
        match self {
            Self::Cargo => Some("cargo install rozi --locked"),
            Self::Mise => Some("mise upgrade rozi"),
            Self::Homebrew => Some("brew upgrade rozi"),
            Self::Scoop => Some("scoop update rozi"),
            Self::WinGet => Some("winget upgrade rozi"),
            // A managed install updates itself, and no single command covers every distribution.
            Self::Managed | Self::SystemPackage | Self::Unknown => None,
        }
    }

    /// Short name for this channel, for `rozi update --check`.
    pub fn label(self) -> &'static str {
        match self {
            Self::Managed => "managed",
            Self::Cargo => "cargo",
            Self::Mise => "mise",
            Self::Homebrew => "homebrew",
            Self::Scoop => "scoop",
            Self::WinGet => "winget",
            Self::SystemPackage => "system package",
            Self::Unknown => "unmanaged",
        }
    }
}

fn cargo_bin(env: &SourceEnv) -> Option<PathBuf> {
    env.cargo_home
        .clone()
        .or_else(|| Some(env.home.as_ref()?.join(".cargo")))
        .map(|home| home.join("bin"))
}

fn mise_installs(env: &SourceEnv) -> Option<PathBuf> {
    env.mise_data_dir
        .clone()
        .or_else(|| Some(env.home.as_ref()?.join(".local").join("share").join("mise")))
        .map(|data| data.join("installs"))
}

fn scoop_apps(env: &SourceEnv) -> Option<PathBuf> {
    Some(env.home.as_ref()?.join("scoop").join("apps"))
}

fn homebrew_prefixes(env: &SourceEnv) -> [Option<PathBuf>; 4] {
    [
        env.homebrew_prefix.clone(),
        Some(PathBuf::from("/opt/homebrew")),
        Some(PathBuf::from("/usr/local/Cellar")),
        Some(PathBuf::from("/home/linuxbrew/.linuxbrew")),
    ]
}

fn within_any<I>(path: &Path, bases: I) -> bool
where
    I: IntoIterator<Item = Option<PathBuf>>,
{
    bases
        .into_iter()
        .flatten()
        .any(|base| path.starts_with(&base))
}

/// Whether `path` contains `first` immediately followed by `second`.
///
/// Compared case-insensitively on every platform: Windows paths are case-insensitive, and a
/// lowercase `winget` in a hand-written path should still be recognised on Unix.
fn adjacent(path: &Path, first: &str, second: &str) -> bool {
    let components: Vec<String> = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    components
        .windows(2)
        .any(|pair| pair[0].eq_ignore_ascii_case(first) && pair[1].eq_ignore_ascii_case(second))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> SourceEnv {
        SourceEnv {
            home: Some(PathBuf::from("/home/dev")),
            managed_root: Some(PathBuf::from("/home/dev/.local/share/rozi")),
            ..SourceEnv::default()
        }
    }

    #[test]
    fn managed_layout_is_recognised_before_any_other_channel() {
        let exe = Path::new("/home/dev/.local/share/rozi/versions/0.1.0/rozi");
        assert_eq!(InstallSource::detect(exe, &env()), InstallSource::Managed);
    }

    #[test]
    fn default_cargo_bin_is_recognised() {
        let exe = Path::new("/home/dev/.cargo/bin/rozi");
        assert_eq!(InstallSource::detect(exe, &env()), InstallSource::Cargo);
    }

    #[test]
    fn relocated_cargo_home_is_recognised() {
        let source = SourceEnv {
            cargo_home: Some(PathBuf::from("/opt/cargo")),
            ..env()
        };
        let exe = Path::new("/opt/cargo/bin/rozi");
        assert_eq!(InstallSource::detect(exe, &source), InstallSource::Cargo);
    }

    #[test]
    fn default_mise_install_is_recognised() {
        let exe = Path::new("/home/dev/.local/share/mise/installs/rozi/0.1.0/bin/rozi");
        assert_eq!(InstallSource::detect(exe, &env()), InstallSource::Mise);
    }

    #[test]
    fn relocated_mise_data_dir_is_recognised() {
        let source = SourceEnv {
            mise_data_dir: Some(PathBuf::from("/var/mise-data")),
            ..env()
        };
        let exe = Path::new("/var/mise-data/installs/rozi/0.1.0/bin/rozi");
        assert_eq!(InstallSource::detect(exe, &source), InstallSource::Mise);
    }

    #[test]
    fn homebrew_prefixes_are_recognised() {
        for exe in [
            "/opt/homebrew/bin/rozi",
            "/home/linuxbrew/.linuxbrew/bin/rozi",
        ] {
            assert_eq!(
                InstallSource::detect(Path::new(exe), &env()),
                InstallSource::Homebrew,
                "{exe}"
            );
        }
    }

    #[test]
    fn windows_channels_are_recognised_case_insensitively() {
        let exe = Path::new("C:/Users/dev/AppData/Local/microsoft/winget/packages/rozi/rozi.exe");
        assert_eq!(InstallSource::detect(exe, &env()), InstallSource::WinGet);
        let scoop = Path::new("C:/Users/dev/Scoop/Apps/rozi/current/rozi.exe");
        assert_eq!(InstallSource::detect(scoop, &env()), InstallSource::Scoop);
    }

    #[test]
    fn distribution_prefix_is_a_system_package() {
        let exe = Path::new("/usr/bin/rozi");
        assert_eq!(
            InstallSource::detect(exe, &env()),
            InstallSource::SystemPackage
        );
    }

    /// `/usr/local/bin` is as often a hand-placed binary as a packaged one, so it must not claim a
    /// package manager owns it.
    #[test]
    fn usr_local_bin_is_not_claimed_by_a_package_manager() {
        let exe = Path::new("/usr/local/bin/rozi");
        assert_eq!(InstallSource::detect(exe, &env()), InstallSource::Unknown);
    }

    #[test]
    fn an_unrecognised_layout_suggests_nothing() {
        let exe = Path::new("/home/dev/Projects/rozi/target/release/rozi");
        let source = InstallSource::detect(exe, &env());
        assert_eq!(source, InstallSource::Unknown);
        assert_eq!(source.upgrade_command(), None);
    }

    #[test]
    fn every_package_manager_channel_names_a_command() {
        for source in [
            InstallSource::Cargo,
            InstallSource::Mise,
            InstallSource::Homebrew,
            InstallSource::Scoop,
            InstallSource::WinGet,
        ] {
            assert!(
                source.upgrade_command().is_some(),
                "{source:?} has no upgrade command"
            );
        }
    }
}
