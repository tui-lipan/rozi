//! The fixed release target matrix and canonical names derived from it.

use super::ReleaseError;
use semver::Version;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// Targets for which signed hyprmux assets are published.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Target {
    X86_64UnknownLinuxGnu,
    Aarch64UnknownLinuxGnu,
    X86_64AppleDarwin,
    Aarch64AppleDarwin,
    X86_64PcWindowsMsvc,
}

impl Ord for Target {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl PartialOrd for Target {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Descriptive alias used by release tooling.
pub type ReleaseTarget = Target;

impl Target {
    /// The complete, ordered release matrix.
    pub const ALL: [Self; 5] = [
        Self::Aarch64AppleDarwin,
        Self::Aarch64UnknownLinuxGnu,
        Self::X86_64AppleDarwin,
        Self::X86_64PcWindowsMsvc,
        Self::X86_64UnknownLinuxGnu,
    ];

    pub const X86_64_UNKNOWN_LINUX_GNU: Self = Self::X86_64UnknownLinuxGnu;
    pub const AARCH64_UNKNOWN_LINUX_GNU: Self = Self::Aarch64UnknownLinuxGnu;
    pub const X86_64_APPLE_DARWIN: Self = Self::X86_64AppleDarwin;
    pub const AARCH64_APPLE_DARWIN: Self = Self::Aarch64AppleDarwin;
    pub const X86_64_PC_WINDOWS_MSVC: Self = Self::X86_64PcWindowsMsvc;

    /// Return the Rust target triple used in release names.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::X86_64UnknownLinuxGnu => "x86_64-unknown-linux-gnu",
            Self::Aarch64UnknownLinuxGnu => "aarch64-unknown-linux-gnu",
            Self::X86_64AppleDarwin => "x86_64-apple-darwin",
            Self::Aarch64AppleDarwin => "aarch64-apple-darwin",
            Self::X86_64PcWindowsMsvc => "x86_64-pc-windows-msvc",
        }
    }

    pub const fn is_windows(self) -> bool {
        matches!(self, Self::X86_64PcWindowsMsvc)
    }

    pub const fn archive_suffix(self) -> &'static str {
        if self.is_windows() { ".zip" } else { ".tar.gz" }
    }

    pub const fn archive_format(self) -> &'static str {
        if self.is_windows() { "zip" } else { "tar.gz" }
    }

    pub const fn launcher_protocol(self) -> Option<u32> {
        if self.is_windows() { Some(1) } else { None }
    }

    pub const fn payload_name(self) -> &'static str {
        if self.is_windows() {
            "hyprmux.exe"
        } else {
            "hyprmux"
        }
    }

    pub const fn launcher_name(self) -> Option<&'static str> {
        if self.is_windows() {
            Some("hyprmux-launcher.exe")
        } else {
            None
        }
    }

    /// The canonical top-level directory in an archive.
    pub fn root_name(self, version: &Version) -> String {
        format!("hyprmux-{version}-{}", self.as_str())
    }

    /// The canonical published archive filename.
    pub fn archive_name(self, version: &Version) -> String {
        format!("{}{}", self.root_name(version), self.archive_suffix())
    }

    /// The canonical payload member path inside an archive.
    pub fn payload_path(self, version: &Version) -> String {
        format!("{}/{}", self.root_name(version), self.payload_name())
    }

    /// The canonical optional launcher member path inside a Windows archive.
    pub fn launcher_path(self, version: &Version) -> Option<String> {
        self.launcher_name()
            .map(|name| format!("{}/{}", self.root_name(version), name))
    }

    /// Parse a version string before deriving names. This is useful to release tooling that starts
    /// with a tag or CLI argument rather than an already parsed [`Version`].
    pub fn parse_version(version: &str) -> Result<Version, ReleaseError> {
        let parsed = Version::parse(version)
            .map_err(|error| ReleaseError::invalid(format!("invalid release version: {error}")))?;
        if parsed.to_string() != version {
            return Err(ReleaseError::invalid(format!(
                "release version is not canonical: {version}"
            )));
        }
        Ok(parsed)
    }
}

#[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"))]
impl Target {
    /// Map the compile-time host triple to the supported release matrix.
    pub const fn current() -> Option<Self> {
        Some(Self::X86_64UnknownLinuxGnu)
    }
}

#[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
impl Target {
    pub const fn current() -> Option<Self> {
        Some(Self::Aarch64UnknownLinuxGnu)
    }
}

#[cfg(all(target_arch = "x86_64", target_os = "macos"))]
impl Target {
    pub const fn current() -> Option<Self> {
        Some(Self::X86_64AppleDarwin)
    }
}

#[cfg(all(target_arch = "aarch64", target_os = "macos"))]
impl Target {
    pub const fn current() -> Option<Self> {
        Some(Self::Aarch64AppleDarwin)
    }
}

#[cfg(all(target_arch = "x86_64", target_os = "windows", target_env = "msvc"))]
impl Target {
    pub const fn current() -> Option<Self> {
        Some(Self::X86_64PcWindowsMsvc)
    }
}

#[cfg(not(any(
    all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"),
    all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"),
    all(target_arch = "x86_64", target_os = "macos"),
    all(target_arch = "aarch64", target_os = "macos"),
    all(target_arch = "x86_64", target_os = "windows", target_env = "msvc")
)))]
impl Target {
    pub const fn current() -> Option<Self> {
        None
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Target {
    type Err = ReleaseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "x86_64-unknown-linux-gnu" => Ok(Self::X86_64UnknownLinuxGnu),
            "aarch64-unknown-linux-gnu" => Ok(Self::Aarch64UnknownLinuxGnu),
            "x86_64-apple-darwin" => Ok(Self::X86_64AppleDarwin),
            "aarch64-apple-darwin" => Ok(Self::Aarch64AppleDarwin),
            "x86_64-pc-windows-msvc" => Ok(Self::X86_64PcWindowsMsvc),
            other => Err(ReleaseError::invalid(format!(
                "unsupported release target: {other}"
            ))),
        }
    }
}

impl Serialize for Target {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Target {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_target_has_canonical_names() {
        let version = Version::parse("1.2.3").unwrap();
        assert_eq!(
            Target::X86_64UnknownLinuxGnu.archive_name(&version),
            "hyprmux-1.2.3-x86_64-unknown-linux-gnu.tar.gz"
        );
        assert_eq!(
            Target::Aarch64AppleDarwin.payload_path(&version),
            "hyprmux-1.2.3-aarch64-apple-darwin/hyprmux"
        );
        assert_eq!(
            Target::X86_64PcWindowsMsvc.launcher_path(&version),
            Some("hyprmux-1.2.3-x86_64-pc-windows-msvc/hyprmux-launcher.exe".to_string())
        );
    }

    #[test]
    fn target_parser_rejects_near_misses() {
        for target in [
            "x86_64-unknown-linux-musl",
            "aarch64-pc-windows-msvc",
            "X86_64-apple-darwin",
            "x86_64-pc-windows-gnu",
        ] {
            assert!(Target::from_str(target).is_err(), "accepted {target}");
        }
    }

    #[test]
    fn version_parser_rejects_noncanonical_spelling() {
        assert!(Target::parse_version("v1.2.3").is_err());
        assert!(Target::parse_version("01.2.3").is_err());
        assert!(Target::parse_version("1.2.3").is_ok());
    }
}
