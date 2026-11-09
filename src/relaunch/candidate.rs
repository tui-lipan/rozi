//! Finding the rozi binary a relaunch should run, and proving what it is before running it.
//!
//! The binary is found where its install channel puts it (see
//! [`InstallSource::relaunch_entries`]), and then asked: `rozi api describe` names its release and
//! the session wire generations it speaks. Nothing is assumed from the path. A channel can move a
//! link, leave the old version in place, or put something unexpected on `PATH`; only the answer of
//! the binary itself says whether it is the build an update installed and which sessions it can
//! carry on.

use std::io::Read;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use semver::Version;

use crate::platform::install_source::{InstallSource, SourceEnv};
use crate::platform::paths::PlatformEnv;
use crate::platform::relaunch::{FileIdentity, file_identity};

/// How long a candidate may take to describe itself. Starting a binary is quick; this only bounds a
/// binary that hangs or a network filesystem that does.
const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Bytes read from a candidate's answer. `api describe` prints well under this.
const MAX_DESCRIBE_BYTES: u64 = 256 * 1024;

/// A rozi binary that described itself.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub path: PathBuf,
    pub release: Version,
    /// Every session wire generation it speaks, oldest first.
    pub protocol: RangeInclusive<u32>,
    identity: FileIdentity,
}

impl Candidate {
    /// Whether the file at [`Self::path`] is still the one that described itself. Checked right
    /// before running it: anything that replaced it since is unverified.
    pub fn unchanged(&self) -> bool {
        file_identity(&self.path).is_ok_and(|identity| identity == self.identity)
    }

    /// Whether this build can attach to a session server speaking `generation`.
    pub fn speaks(&self, generation: u32) -> bool {
        self.protocol.contains(&generation)
    }

    /// A candidate at the running test binary, which really is a file that stays unchanged, as
    /// though it had described itself as `release` speaking `protocol`.
    #[cfg(test)]
    pub(crate) fn for_test(release: &str, protocol: RangeInclusive<u32>) -> Self {
        let path = std::env::current_exe().expect("the test binary has a path");
        Self {
            identity: file_identity(&path).expect("the test binary is a file"),
            path,
            release: Version::parse(release).expect("a test release is semver"),
            protocol,
        }
    }
}

/// Why no binary qualified, naming each path tried.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoCandidate {
    pub expected: Option<Version>,
    pub tried: Vec<(PathBuf, String)>,
}

impl std::fmt::Display for NoCandidate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.expected {
            Some(version) => write!(f, "rozi {version} was not found where it was installed")?,
            None => f.write_str("no runnable rozi was found")?,
        }
        for (path, reason) in &self.tried {
            write!(f, "\n{}: {reason}", path.display())?;
        }
        Ok(())
    }
}

/// Find the binary to relaunch into: the one that reports `expected`, or, without one, the first
/// runnable rozi where the install keeps it.
pub fn find(expected: Option<&Version>) -> Result<Candidate, NoCandidate> {
    let running = Version::parse(env!("CARGO_PKG_VERSION")).expect("the package version is semver");
    let platform_env = PlatformEnv::from_process();
    let source_env = SourceEnv::from_platform_env(&platform_env);
    let launched = crate::platform::relaunch::launched_from();
    let source = launched
        .as_deref()
        .map(|path| InstallSource::detect(path, &source_env))
        .unwrap_or(InstallSource::Unknown);
    let on_path = crate::platform::relaunch::on_path();
    let paths = candidate_paths(
        launched.as_deref(),
        source,
        &source_env,
        &running,
        expected,
        on_path,
    );
    first_qualifying(paths, expected, describe)
}

fn first_qualifying(
    paths: Vec<PathBuf>,
    expected: Option<&Version>,
    describe: impl Fn(&Path) -> Result<Candidate, String>,
) -> Result<Candidate, NoCandidate> {
    let mut tried = Vec::new();
    for path in paths {
        match describe(&path) {
            Ok(candidate) if expected.is_none_or(|expected| candidate.release == *expected) => {
                return Ok(candidate);
            }
            Ok(candidate) => tried.push((path, format!("is rozi {}", candidate.release))),
            Err(reason) => tried.push((path, reason)),
        }
    }
    Err(NoCandidate {
        expected: expected.cloned(),
        tried,
    })
}

/// Where to look, most trustworthy first and without repeats: the channel's own entry points, the
/// launched path with its version directory moved to the expected release, the launched path
/// itself, and finally whatever `rozi` the `PATH` finds.
pub fn candidate_paths(
    launched: Option<&Path>,
    source: InstallSource,
    source_env: &SourceEnv,
    running: &Version,
    expected: Option<&Version>,
    on_path: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(launched) = launched {
        paths.extend(source.relaunch_entries(launched, source_env));
        if let Some(expected) = expected {
            paths.extend(with_version_moved(launched, running, expected));
        }
        paths.push(launched.to_path_buf());
    }
    paths.extend(on_path);
    let mut unique = Vec::with_capacity(paths.len());
    for path in paths {
        if !unique.contains(&path) {
            unique.push(path);
        }
    }
    unique
}

/// `path` with every component naming `from` (as `1.2.3` or `v1.2.3`) renamed to `to`, the way
/// channels that keep each version in a directory of its own (mise, Homebrew's Cellar, Scoop) lay
/// out the next one. `None` when no component names the version.
pub fn with_version_moved(path: &Path, from: &Version, to: &Version) -> Option<PathBuf> {
    let plain = from.to_string();
    let prefixed = format!("v{plain}");
    let mut moved = false;
    let renamed = path
        .components()
        .map(|component| {
            let text = component.as_os_str().to_string_lossy();
            if text == plain {
                moved = true;
                std::ffi::OsString::from(to.to_string())
            } else if text == prefixed {
                moved = true;
                std::ffi::OsString::from(format!("v{to}"))
            } else {
                component.as_os_str().to_os_string()
            }
        })
        .collect::<PathBuf>();
    moved.then_some(renamed)
}

/// Ask the binary at `path` what it is.
pub fn describe(path: &Path) -> Result<Candidate, String> {
    let identity = file_identity(path).map_err(|error| error.to_string())?;
    let output = run_bounded(path, &["api", "describe"])?;
    let mut described = parse_description(&output)?;
    if described.release.is_none() {
        // A build from before `api describe` named its release still prints it here.
        described.release = parse_version_banner(&run_bounded(path, &["--version"])?);
    }
    let release = described
        .release
        .ok_or_else(|| "did not say which release it is".to_string())?;
    // Described from the file that is there now, not one that replaced it while it ran.
    if file_identity(path).ok().as_ref() != Some(&identity) {
        return Err("changed while it was being checked".to_string());
    }
    Ok(Candidate {
        path: path.to_path_buf(),
        release,
        protocol: described.protocol,
        identity,
    })
}

struct Described {
    release: Option<Version>,
    protocol: RangeInclusive<u32>,
}

fn parse_description(output: &[u8]) -> Result<Described, String> {
    let value: serde_json::Value =
        serde_json::from_slice(output).map_err(|_| "is not a rozi binary".to_string())?;
    let generation = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
    };
    let (Some(max), Some(min)) = (
        generation("session_protocol"),
        generation("session_protocol_min"),
    ) else {
        return Err("does not say which sessions it can attach to".to_string());
    };
    if min > max {
        return Err("describes an empty session protocol range".to_string());
    }
    let release = value
        .get("release")
        .and_then(serde_json::Value::as_str)
        .and_then(|release| Version::parse(release).ok());
    Ok(Described {
        release,
        protocol: min..=max,
    })
}

/// The version in `rozi --version`'s first line, `rozi <version>`.
fn parse_version_banner(output: &[u8]) -> Option<Version> {
    let text = std::str::from_utf8(output).ok()?;
    let version = text.lines().next()?.strip_prefix("rozi ")?;
    Version::parse(version.trim()).ok()
}

/// Run `path args...` with nothing on its input, and return what it printed if it exits
/// successfully within [`DESCRIBE_TIMEOUT`].
fn run_bounded(path: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut child = Command::new(path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not run: {error}"))?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        let _ = stdout.take(MAX_DESCRIBE_BYTES).read_to_end(&mut output);
        output
    });
    let deadline = Instant::now() + DESCRIBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "did not answer within {}s",
                    DESCRIBE_TIMEOUT.as_secs()
                ));
            }
            Err(error) => return Err(format!("could not run: {error}")),
        }
    };
    let output = reader
        .join()
        .map_err(|_| "its output could not be read".to_string())?;
    if !status.success() {
        return Err(format!("failed ({status})"));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn a_versioned_directory_moves_to_the_expected_release() {
        let moved = with_version_moved(
            Path::new("/home/dev/.local/share/mise/installs/rozi/0.0.43/bin/rozi"),
            &version("0.0.43"),
            &version("0.0.44"),
        );
        assert_eq!(
            moved,
            Some(PathBuf::from(
                "/home/dev/.local/share/mise/installs/rozi/0.0.44/bin/rozi"
            ))
        );
        let prefixed = with_version_moved(
            Path::new("/opt/rozi/v0.0.43/rozi"),
            &version("0.0.43"),
            &version("0.0.44"),
        );
        assert_eq!(prefixed, Some(PathBuf::from("/opt/rozi/v0.0.44/rozi")));
        assert_eq!(
            with_version_moved(
                Path::new("/usr/bin/rozi"),
                &version("0.0.43"),
                &version("0.0.44")
            ),
            None
        );
    }

    #[test]
    fn the_channel_entry_comes_first_and_nothing_is_tried_twice() {
        let env = SourceEnv {
            home: Some(PathBuf::from("/home/dev")),
            ..SourceEnv::default()
        };
        let name = crate::platform::relaunch::executable_name();
        let cargo = PathBuf::from("/home/dev/.cargo/bin").join(name);
        let paths = candidate_paths(
            Some(&cargo),
            InstallSource::Cargo,
            &env,
            &version("0.0.43"),
            Some(&version("0.0.44")),
            Some(cargo.clone()),
        );
        assert_eq!(paths, vec![cargo]);

        let mise = PathBuf::from("/home/dev/.local/share/mise/installs/rozi/0.0.43/rozi");
        let paths = candidate_paths(
            Some(&mise),
            InstallSource::Mise,
            &env,
            &version("0.0.43"),
            Some(&version("0.0.44")),
            Some(PathBuf::from("/usr/local/bin/rozi")),
        );
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/dev/.local/share/mise/installs/rozi/0.0.44/rozi"),
                mise,
                PathBuf::from("/usr/local/bin/rozi"),
            ]
        );
    }

    fn described(path: &Path, release: &str) -> Candidate {
        Candidate {
            path: path.to_path_buf(),
            release: version(release),
            protocol: 52..=52,
            identity: file_identity(&std::env::current_exe().unwrap()).unwrap(),
        }
    }

    #[test]
    fn only_the_release_an_update_installed_qualifies_and_every_miss_is_named() {
        let old = PathBuf::from("/old/rozi");
        let new = PathBuf::from("/new/rozi");
        let broken = PathBuf::from("/broken/rozi");
        let describe = |path: &Path| match path.to_str() {
            Some("/old/rozi") => Ok(described(path, "0.0.43")),
            Some("/new/rozi") => Ok(described(path, "0.0.44")),
            _ => Err("could not run".to_string()),
        };
        let found = first_qualifying(
            vec![broken.clone(), old.clone(), new.clone()],
            Some(&version("0.0.44")),
            describe,
        )
        .unwrap();
        assert_eq!(found.path, new);

        let missing = first_qualifying(
            vec![broken.clone(), old.clone()],
            Some(&version("0.0.44")),
            describe,
        )
        .unwrap_err();
        assert_eq!(
            missing.tried,
            vec![
                (broken, "could not run".to_string()),
                (old.clone(), "is rozi 0.0.43".to_string()),
            ]
        );
        assert!(missing.to_string().starts_with("rozi 0.0.44 was not found"));

        let any = first_qualifying(vec![old.clone()], None, describe).unwrap();
        assert_eq!(any.path, old, "a plain restart takes whatever is installed");
    }

    #[test]
    fn a_description_names_the_release_and_the_generations_spoken() {
        let described = parse_description(
            br#"{"release":"0.0.44","api":1,"schema":4,"session_protocol":53,"session_protocol_min":52,"capabilities":[]}"#,
        )
        .unwrap();
        assert_eq!(described.release, Some(version("0.0.44")));
        assert_eq!(described.protocol, 52..=53);

        let older =
            parse_description(br#"{"session_protocol":52,"session_protocol_min":52}"#).unwrap();
        assert_eq!(older.release, None);
        assert_eq!(
            parse_version_banner(b"rozi 0.0.43\nhttps://rozi.tui-lipan.dev\n"),
            Some(version("0.0.43"))
        );
        assert!(parse_description(b"not json").is_err());
        assert!(
            parse_description(br#"{"session_protocol":51,"session_protocol_min":52}"#).is_err()
        );
    }
}
