//! Signed release manifest schema and canonical validation.

use super::target::Target;
use super::{MAX_ARCHIVE_SIZE, MAX_MEMBER_SIZE, ReleaseError, Result};
use chrono::DateTime;
use semver::Version;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::ops::Deref;

/// The only manifest schema accepted by this build.
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Metadata for the exact executable member selected from an archive.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PayloadInfo {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

impl PayloadInfo {
    pub fn new(path: impl Into<String>, size: u64, sha256: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            sha256: sha256.into(),
            size,
        }
    }
}

/// Metadata for the Windows-only launcher protocol member.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LauncherInfo {
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub protocol: u32,
}

impl LauncherInfo {
    pub fn new(
        path: impl Into<String>,
        protocol: u32,
        size: u64,
        sha256: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            sha256: sha256.into(),
            size,
            protocol,
        }
    }
}

/// One target's signed release asset record.
///
/// The target is intentionally not serialized here. It is the authenticated map key in
/// [`ReleaseManifest::targets`], so a caller cannot trust a redundant target field inside a signed
/// value after selecting an entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseAsset {
    pub archive: String,
    pub archive_sha256: String,
    pub archive_size: u64,
    pub payload: PayloadInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<LauncherInfo>,
}

pub type Asset = ReleaseAsset;
pub type FileDigest = PayloadInfo;

impl ReleaseAsset {
    /// Construct an asset with canonical names and no launcher. Windows callers should use
    /// [`Self::with_launcher`] so the protocol field is explicit.
    pub fn new(
        version: &Version,
        target: Target,
        archive_size: u64,
        archive_sha256: impl Into<String>,
        payload_size: u64,
        payload_sha256: impl Into<String>,
    ) -> Self {
        Self {
            archive: target.archive_name(version),
            archive_sha256: archive_sha256.into(),
            archive_size,
            payload: PayloadInfo::new(target.payload_path(version), payload_size, payload_sha256),
            launcher: None,
        }
    }

    pub fn with_launcher(
        mut self,
        version: &Version,
        target: Target,
        protocol: u32,
        launcher_size: u64,
        launcher_sha256: impl Into<String>,
    ) -> Self {
        if let Some(path) = target.launcher_path(version) {
            self.launcher = Some(LauncherInfo::new(
                path,
                protocol,
                launcher_size,
                launcher_sha256,
            ));
        }
        self
    }

    pub fn validate(&self, version: &Version, target: Target) -> Result<()> {
        let expected_archive = target.archive_name(version);
        if self.archive != expected_archive {
            return Err(ReleaseError::invalid(format!(
                "noncanonical archive name for {}: expected {expected_archive}, got {}",
                target, self.archive
            )));
        }
        validate_basename(&self.archive, "archive name")?;
        validate_size(self.archive_size, MAX_ARCHIVE_SIZE, "archive")?;
        validate_sha256(&self.archive_sha256, "archive")?;

        let expected_payload = target.payload_path(version);
        if self.payload.path != expected_payload {
            return Err(ReleaseError::invalid(format!(
                "noncanonical payload path for {}: expected {expected_payload}, got {}",
                target, self.payload.path
            )));
        }
        validate_member_path(&self.payload.path, "payload path")?;
        validate_size(self.payload.size, MAX_MEMBER_SIZE, "payload")?;
        validate_sha256(&self.payload.sha256, "payload")?;

        match (target.is_windows(), &self.launcher) {
            (true, Some(launcher)) => {
                let expected = target
                    .launcher_path(version)
                    .expect("Windows targets have a launcher");
                if launcher.path != expected {
                    return Err(ReleaseError::invalid(format!(
                        "noncanonical launcher path for {}: expected {expected}, got {}",
                        target, launcher.path
                    )));
                }
                if launcher.protocol != target.launcher_protocol().unwrap_or(0) {
                    return Err(ReleaseError::invalid(format!(
                        "unsupported launcher protocol {} for {}",
                        launcher.protocol, target
                    )));
                }
                validate_member_path(&launcher.path, "launcher path")?;
                validate_size(launcher.size, MAX_MEMBER_SIZE, "launcher")?;
                validate_sha256(&launcher.sha256, "launcher")?;
                if launcher.path == self.payload.path {
                    return Err(ReleaseError::invalid(
                        "payload and launcher paths must be distinct",
                    ));
                }
            }
            (true, None) => {
                return Err(ReleaseError::invalid(
                    "Windows release asset is missing its protocol-1 launcher",
                ));
            }
            (false, Some(_)) => {
                return Err(ReleaseError::invalid(format!(
                    "non-Windows release asset {} must not contain a launcher",
                    target
                )));
            }
            (false, None) => {}
        }
        Ok(())
    }

    pub fn archive_name(&self) -> &str {
        &self.archive
    }

    pub fn archive_size(&self) -> u64 {
        self.archive_size
    }

    pub fn archive_sha256(&self) -> &str {
        &self.archive_sha256
    }
}

/// A selected asset whose target comes from the authenticated map key, never from signed fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectedAsset<'a> {
    pub target: Target,
    pub asset: &'a ReleaseAsset,
}

impl<'a> SelectedAsset<'a> {
    pub fn new(target: Target, asset: &'a ReleaseAsset) -> Self {
        Self { target, asset }
    }

    pub fn archive(&self) -> &str {
        &self.asset.archive
    }

    pub fn archive_sha256(&self) -> &str {
        &self.asset.archive_sha256
    }

    pub fn archive_size(&self) -> u64 {
        self.asset.archive_size
    }

    pub fn payload(&self) -> &PayloadInfo {
        &self.asset.payload
    }

    pub fn launcher(&self) -> Option<&LauncherInfo> {
        self.asset.launcher.as_ref()
    }
}

impl Deref for SelectedAsset<'_> {
    type Target = ReleaseAsset;

    fn deref(&self) -> &Self::Target {
        self.asset
    }
}

/// Schema-versioned release metadata.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifest {
    pub schema_version: u32,
    #[serde(deserialize_with = "deserialize_canonical_version")]
    pub version: Version,
    pub published_at: String,
    pub targets: BTreeMap<Target, ReleaseAsset>,
}

pub type Manifest = ReleaseManifest;

impl ReleaseManifest {
    pub fn new(
        version: Version,
        published_at: impl Into<String>,
        targets: BTreeMap<Target, ReleaseAsset>,
    ) -> Result<Self> {
        let manifest = Self {
            schema_version: MANIFEST_SCHEMA_VERSION,
            version,
            published_at: published_at.into(),
            targets,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > super::MAX_METADATA_SIZE {
            return Err(ReleaseError::invalid(format!(
                "release manifest exceeds {} bytes",
                super::MAX_METADATA_SIZE
            )));
        }
        let manifest: Self = serde_json::from_slice(bytes)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn from_json(text: &str) -> Result<Self> {
        Self::from_bytes(text.as_bytes())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        Ok(serde_json::to_vec(self)?)
    }

    pub fn to_json(&self) -> Result<String> {
        Ok(String::from_utf8(self.to_bytes()?)
            .expect("serde_json emits UTF-8 for a Rust string model"))
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != MANIFEST_SCHEMA_VERSION {
            return Err(ReleaseError::invalid(format!(
                "unsupported release manifest schema_version {}",
                self.schema_version
            )));
        }
        if self.version.to_string().is_empty() {
            return Err(ReleaseError::invalid("release manifest version is empty"));
        }
        DateTime::parse_from_rfc3339(&self.published_at).map_err(|error| {
            ReleaseError::invalid(format!("invalid published_at RFC3339 timestamp: {error}"))
        })?;
        if self.targets.is_empty() {
            return Err(ReleaseError::invalid(
                "release manifest must contain at least one target",
            ));
        }
        if self.targets.len() > Target::ALL.len() {
            return Err(ReleaseError::invalid(format!(
                "release manifest contains too many targets: {}",
                self.targets.len()
            )));
        }
        let mut seen = HashSet::with_capacity(self.targets.len());
        for (target, asset) in &self.targets {
            if !seen.insert(*target) {
                return Err(ReleaseError::invalid(format!(
                    "duplicate release target {}",
                    target
                )));
            }
            asset.validate(&self.version, *target)?;
        }
        Ok(())
    }

    /// Select metadata using the authenticated map key as the target authority.
    pub fn asset_for(&self, target: Target) -> Result<SelectedAsset<'_>> {
        self.validate()?;
        let asset = self
            .targets
            .get(&target)
            .ok_or_else(|| ReleaseError::invalid(format!("release has no asset for {target}")))?;
        Ok(SelectedAsset { target, asset })
    }

    pub fn target_assets(&self) -> &BTreeMap<Target, ReleaseAsset> {
        &self.targets
    }
}

fn validate_size(size: u64, maximum: u64, label: &str) -> Result<()> {
    if size == 0 {
        return Err(ReleaseError::invalid(format!(
            "{label} size must be nonzero"
        )));
    }
    if size > maximum {
        return Err(ReleaseError::invalid(format!(
            "{label} size {size} exceeds maximum {maximum}"
        )));
    }
    Ok(())
}

pub(crate) fn validate_sha256(value: &str, label: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(ReleaseError::invalid(format!(
            "invalid {label} SHA-256: expected 64 lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

fn validate_basename(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || value.contains('\0')
        || value.contains(':')
    {
        return Err(ReleaseError::invalid(format!(
            "noncanonical {label}: {value:?}"
        )));
    }
    Ok(())
}

fn validate_member_path(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.starts_with('/')
        || value.contains('\\')
        || value.contains('\0')
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(ReleaseError::invalid(format!(
            "noncanonical {label}: {value:?}"
        )));
    }
    if value
        .split('/')
        .next()
        .is_some_and(|component| component.contains(':'))
    {
        return Err(ReleaseError::invalid(format!(
            "noncanonical {label}: {value:?}"
        )));
    }
    Ok(())
}

fn deserialize_canonical_version<'de, D>(deserializer: D) -> std::result::Result<Version, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    let parsed = Version::parse(&raw).map_err(serde::de::Error::custom)?;
    if parsed.to_string() != raw {
        return Err(serde::de::Error::custom(format!(
            "release version is not canonical: {raw}"
        )));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLISHED_AT: &str = "2026-08-02T12:00:00Z";

    fn unix_asset(version: &Version, target: Target) -> ReleaseAsset {
        ReleaseAsset::new(
            version,
            target,
            10,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            3,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        )
    }

    #[test]
    fn manifest_round_trips_authoritative_target_map_shape() {
        let version = Version::parse("0.2.0").unwrap();
        let target = Target::X86_64UnknownLinuxGnu;
        let asset = unix_asset(&version, target);
        let manifest =
            ReleaseManifest::new(version, PUBLISHED_AT, BTreeMap::from([(target, asset)])).unwrap();
        let bytes = manifest.to_bytes().unwrap();
        let json = String::from_utf8(bytes.clone()).unwrap();
        assert!(json.contains("\"published_at\":\"2026-08-02T12:00:00Z\""));
        assert!(json.contains("\"targets\":{"));
        assert!(!json.contains("\"target\":"));
        assert!(!json.contains("\"format\":"));
        assert!(!json.contains("\"artifacts\":"));
        assert!(json.contains("\"archive_sha256\":\""));
        assert!(!json.contains("\"archive\":{"));
        assert_eq!(ReleaseManifest::from_bytes(&bytes).unwrap(), manifest);
    }

    #[test]
    fn selected_asset_uses_map_key_as_target_authority() {
        let version = Version::parse("0.2.0").unwrap();
        let target = Target::Aarch64AppleDarwin;
        let manifest = ReleaseManifest::new(
            version.clone(),
            PUBLISHED_AT,
            BTreeMap::from([(target, unix_asset(&version, target))]),
        )
        .unwrap();
        let selected = manifest.asset_for(target).unwrap();
        assert_eq!(selected.target, target);
        assert_eq!(selected.archive(), target.archive_name(&version));
        assert_eq!(selected.asset.payload.path, target.payload_path(&version));
    }

    #[test]
    fn manifest_rejects_unknown_fields_timestamps_hashes_and_sizes() {
        let version = Version::parse("0.2.0").unwrap();
        let target = Target::X86_64UnknownLinuxGnu;
        let asset = unix_asset(&version, target);
        let mut manifest = ReleaseManifest::new(
            version.clone(),
            PUBLISHED_AT,
            BTreeMap::from([(target, asset)]),
        )
        .unwrap();
        manifest.published_at = "not-a-timestamp".into();
        assert!(manifest.validate().is_err());

        let mut oversized = unix_asset(&version, target);
        oversized.archive_size = MAX_ARCHIVE_SIZE + 1;
        assert!(oversized.validate(&version, target).is_err());

        let unknown = br#"{
            "schema_version":1,
            "version":"0.2.0",
            "published_at":"2026-08-02T12:00:00Z",
            "targets":{},
            "extra":true
        }"#;
        assert!(ReleaseManifest::from_bytes(unknown).is_err());
    }
}
