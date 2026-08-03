//! Managed installation, activation, and crash recovery.
//!
//! The release modules own the signed wire formats and archive trust boundary.  This module owns
//! only the local lifecycle: private state, immutable version directories, an authoritative
//! platform selector, and the small activation journal needed to recover a crash at any point in
//! the selector switch.

use super::executable;
use super::fs_security;
use super::paths::{self, PlatformEnv};
use crate::release::{self, Downloader, ReleaseMetadata, ReleaseTarget, UreqDownloader};
use semver::Version;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use url::Url;

const STATE_SCHEMA_VERSION: u32 = 1;
pub const REPOSITORY_URL: &str = "https://github.com/Razuer/hyprmux/";
const VERSIONS_DIR: &str = "versions";
const STAGING_DIR: &str = ".staging";
const LOCK_FILE: &str = ".lock";
const INSTALL_FILE: &str = "install.json";
const PENDING_FILE: &str = "pending-activation.json";
#[cfg(windows)]
const ACTIVE_FILE: &str = "active";
#[cfg(windows)]
const BIN_DIR: &str = "bin";
const PAYLOAD_UNIX: &str = "hyprmux";
const PAYLOAD_WINDOWS: &str = "hyprmux.exe";
const LAUNCHER_WINDOWS: &str = "hyprmux-launcher.exe";
const MANIFEST_FILE: &str = "release.json";
const SIGNATURE_FILE: &str = "release.signatures.json";
const VERSION_FILE: &str = "version.json";
#[cfg(windows)]
const LAUNCHER_CREATED_MARKER: &str = ".launcher-created";

/// A point after which a durable activation boundary has completed.
///
/// The names are intentionally about observable filesystem boundaries rather than implementation
/// helper calls.  A fault injector can therefore model a process dying immediately after any
/// journal step without knowing how a particular platform performs that step.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FaultPoint {
    LockAcquired,
    StagingCreated,
    PayloadWritten,
    Verified,
    StagingSynced,
    VersionRenamed,
    PendingWritten,
    PointerSwitched,
    InstallWritten,
    PendingRemoved,
    ParentsSynced,
}

/// Descriptive alias for [`FaultPoint`].
pub type ActivationBoundary = FaultPoint;

/// Injectable failure boundary used by deterministic activation and recovery tests.
pub trait FaultInjector: Send + Sync {
    /// Called after the named boundary has completed.  Returning an error simulates a process
    /// failure observed by the caller; the filesystem is deliberately left at that boundary.
    fn after(&self, point: FaultPoint) -> io::Result<()> {
        self.inject(point)
    }

    /// Alternate spelling useful for small test injectors.  Implement either this method or
    /// [`Self::after`]; the default implementation is a no-op.
    fn inject(&self, _point: FaultPoint) -> io::Result<()> {
        Ok(())
    }
}

impl<T: FaultInjector + ?Sized> FaultInjector for Arc<T> {
    fn after(&self, point: FaultPoint) -> io::Result<()> {
        (**self).after(point)
    }
}

impl<F> FaultInjector for F
where
    F: Fn(FaultPoint) -> io::Result<()> + Send + Sync,
{
    fn after(&self, point: FaultPoint) -> io::Result<()> {
        self(point)
    }
}

/// The production fault injector.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoFaultInjector;

impl FaultInjector for NoFaultInjector {}

/// Errors raised by local managed-installation policy or by the signed release layer.
#[derive(Debug)]
pub enum InstallError {
    Io(io::Error),
    Release(release::ReleaseError),
    Json(serde_json::Error),
    Invalid(String),
    Unmanaged,
    Downgrade {
        current: Version,
        requested: Version,
    },
    Fault {
        point: FaultPoint,
        source: io::Error,
    },
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "managed installation I/O error: {error}"),
            Self::Release(error) => write!(f, "release verification error: {error}"),
            Self::Json(error) => write!(f, "managed installation state JSON error: {error}"),
            Self::Invalid(message) => f.write_str(message),
            Self::Unmanaged => f.write_str("managed installation is not present"),
            Self::Downgrade { current, requested } => {
                write!(
                    f,
                    "refusing to downgrade managed hyprmux from {current} to {requested}"
                )
            }
            Self::Fault { point, source } => write!(f, "fault injector at {point:?}: {source}"),
        }
    }
}

impl std::error::Error for InstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Release(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Fault { source, .. } => Some(source),
            Self::Invalid(_) | Self::Unmanaged | Self::Downgrade { .. } => None,
        }
    }
}

impl From<io::Error> for InstallError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<release::ReleaseError> for InstallError {
    fn from(error: release::ReleaseError) -> Self {
        Self::Release(error)
    }
}

impl From<serde_json::Error> for InstallError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub type Result<T> = std::result::Result<T, InstallError>;

/// The state recorded beside one immutable payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VersionState {
    pub schema_version: u32,
    #[serde(deserialize_with = "deserialize_canonical_version")]
    pub version: Version,
    pub target: ReleaseTarget,
    pub binary_sha256: String,
    pub size: u64,
    pub installation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<LauncherMetadata>,
}

/// Signed launcher metadata copied into `version.json` on Windows.  The stable launcher itself is
/// recorded separately in [`LauncherOwnership`] because updates never replace it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LauncherMetadata {
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub protocol: u32,
}

/// Descriptive launcher ownership state stored in `install.json`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LauncherOwnership {
    pub owned: bool,
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub protocol: u32,
}

/// Descriptive installation state.  The platform selector, not this document, is authoritative.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallState {
    pub schema_version: u32,
    #[serde(deserialize_with = "deserialize_optional_canonical_version")]
    pub active: Option<Version>,
    #[serde(deserialize_with = "deserialize_optional_canonical_version")]
    pub previous: Option<Version>,
    pub installation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<LauncherOwnership>,
}

/// The activation journal.  `from: null` is the first-install transaction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PendingActivation {
    pub schema_version: u32,
    #[serde(deserialize_with = "deserialize_optional_canonical_version")]
    pub from: Option<Version>,
    #[serde(deserialize_with = "deserialize_canonical_version")]
    pub to: Version,
    pub transaction_id: String,
}

/// Result of checking signed latest metadata.  No archive is fetched by this operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckResult {
    pub current: Option<Version>,
    pub latest: Version,
    pub managed: bool,
}

/// Result of an install/update/rollback activation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationResult {
    pub version: Version,
    pub changed: bool,
}

/// High-level managed installation manager.
pub struct Installation<D = UreqDownloader> {
    root: PathBuf,
    command_path: PathBuf,
    downloader: D,
    fault: Arc<dyn FaultInjector>,
    /// `None` means production verification through the compiled trust anchor.  `Some` exists
    /// only as an explicit test/tooling seam and is never read from process environment.
    trusted_keys: Option<Vec<release::TrustedKey>>,
}

/// Short name suitable for callers that think in terms of a manager rather than an installation.
pub type Manager<D = UreqDownloader> = Installation<D>;

impl<D: Downloader> Installation<D> {
    /// Construct an installation with explicit paths, downloader, and fault injector.
    pub fn new<F>(
        root: impl Into<PathBuf>,
        command_path: impl Into<PathBuf>,
        downloader: D,
        fault: F,
    ) -> Self
    where
        F: FaultInjector + 'static,
    {
        Self {
            root: absolute_path(root.into()),
            command_path: absolute_path(command_path.into()),
            downloader,
            fault: Arc::new(fault),
            trusted_keys: None,
        }
    }

    /// Construct with the production no-fault behavior.
    pub fn without_faults(
        root: impl Into<PathBuf>,
        command_path: impl Into<PathBuf>,
        downloader: D,
    ) -> Self {
        Self::new(root, command_path, downloader, NoFaultInjector)
    }

    /// Replace compiled verification with an explicit in-memory key set for deterministic tests.
    /// Production constructors never call this method.
    pub fn with_trusted_keys(mut self, keys: Vec<release::TrustedKey>) -> Self {
        self.trusted_keys = Some(keys);
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn command_path(&self) -> &Path {
        &self.command_path
    }

    /// Recover an interrupted activation if the installation root already exists.  An absent root
    /// is the normal unmanaged state and returns `false` without creating any files.
    pub fn recover_if_managed(&self) -> Result<bool> {
        if !lexists(&self.root)? {
            return Ok(false);
        }
        fs_security::ensure_private_dir(&self.root)?;
        let has_install = lexists(&self.install_state_path())?;
        let has_pending = lexists(&self.pending_path())?;
        let has_pointer = self.pointer_path_exists()?;
        #[cfg(windows)]
        let has_staging_recovery = self.staging_has_launcher_marker()?;
        #[cfg(not(windows))]
        let has_staging_recovery = false;
        if !has_install && !has_pending && !has_pointer && !has_staging_recovery {
            return Ok(false);
        }
        #[cfg(unix)]
        if has_pointer && !has_install && !has_pending {
            // A command symlink is also a perfectly ordinary unmanaged user command.  Only a
            // canonical pointer into this private root is installation evidence; an unrelated
            // symlink must remain untouched and must not make an unmanaged check fail.
            if self.read_pointer_unlocked().is_err() {
                return Ok(false);
            }
        }
        let _lock = self.lock_existing()?;
        self.recover_locked()
    }

    /// Recover an interrupted activation, returning whether managed state was found.
    pub fn recover(&self) -> Result<bool> {
        self.recover_if_managed()
    }

    /// Install the exact package version compiled into this binary.
    pub fn install(&self) -> Result<ActivationResult> {
        let version = Version::parse(env!("CARGO_PKG_VERSION"))
            .map_err(|error| InstallError::Invalid(format!("invalid package version: {error}")))?;
        self.install_version(version)
    }

    /// Descriptive alias for [`Self::install`].
    pub fn install_current(&self) -> Result<ActivationResult> {
        self.install()
    }

    /// Fetch and install one exact signed release version.
    pub fn install_version(&self, version: Version) -> Result<ActivationResult> {
        self.recover_if_managed()?;
        let metadata = self.fetch_exact_metadata(&version)?;
        let target = current_target()?;
        let archive = release::download_archive(&self.downloader, &metadata, target)?;
        self.activate_download(metadata, archive, version)
    }

    /// Explicit-version spelling for callers that expose an `install --version` command.
    pub fn install_exact(&self, version: Version) -> Result<ActivationResult> {
        self.install_version(version)
    }

    /// Fetch only signed latest metadata and report the authoritative current pointer.
    pub fn check_latest(&self) -> Result<CheckResult> {
        let _ = self.recover_if_managed()?;
        let latest = self.fetch_latest_metadata()?.version;
        let current = if lexists(&self.root)?
            && (self.read_install_state()?.is_some() || self.pointer_path_exists()?)
        {
            self.read_pointer_unlocked()?
        } else {
            None
        };
        let managed = current.is_some() && self.install_state_path().exists();
        Ok(CheckResult {
            current,
            latest,
            managed,
        })
    }

    /// Descriptive alias for [`Self::check_latest`].
    pub fn check(&self) -> Result<CheckResult> {
        self.check_latest()
    }

    /// Update a managed installation to the signed latest release.  Unmanaged installations are
    /// rejected before any download or filesystem mutation.
    pub fn update(&self) -> Result<ActivationResult> {
        self.recover_if_managed()?;
        let current = self.require_managed_current()?;
        let metadata = self.fetch_latest_metadata()?;
        if metadata.version < current {
            return Err(InstallError::Downgrade {
                current,
                requested: metadata.version,
            });
        }
        if metadata.version == current {
            return Ok(ActivationResult {
                version: current,
                changed: false,
            });
        }
        let archive = release::download_archive(&self.downloader, &metadata, current_target()?)?;
        let version = metadata.version.clone();
        self.activate_download(metadata, archive, version)
    }

    /// Explicit latest-update spelling for CLI integrations.
    pub fn update_latest(&self) -> Result<ActivationResult> {
        self.update()
    }

    /// Roll back to `install.json.previous` using the same pending/pointer/install sequence as an
    /// update.  The retained target is fully reverified before a pending journal is written.
    pub fn rollback(&self) -> Result<ActivationResult> {
        self.recover_if_managed()?;
        let current = self.require_managed_current()?;
        let install = self.read_install_state()?.ok_or(InstallError::Unmanaged)?;
        let target = install.previous.clone().ok_or_else(|| {
            InstallError::Invalid("managed installation has no previous version".to_string())
        })?;
        if target >= current {
            return Err(InstallError::Invalid(
                "managed installation previous version is not older than active".to_string(),
            ));
        }
        self.activate_existing(target)
    }

    /// Explicit previous-version spelling for CLI integrations.
    pub fn rollback_previous(&self) -> Result<ActivationResult> {
        self.rollback()
    }

    /// Roll back to a specific retained version.  The normal CLI-facing operation is
    /// [`Self::rollback`], which targets the descriptive previous field.
    pub fn rollback_to(&self, target: Version) -> Result<ActivationResult> {
        self.recover_if_managed()?;
        let current = self.require_managed_current()?;
        if target >= current {
            return Err(InstallError::Invalid(
                "rollback target must be older than active version".to_string(),
            ));
        }
        self.activate_existing(target)
    }

    fn fetch_exact_metadata(&self, version: &Version) -> Result<ReleaseMetadata> {
        let repository = repository_url()?;
        Ok(match &self.trusted_keys {
            Some(keys) => release::fetch_version_metadata_with_keys(
                &self.downloader,
                &repository,
                version,
                keys,
            )?,
            None => release::fetch_exact_metadata(&self.downloader, &repository, version)?,
        })
    }

    fn fetch_latest_metadata(&self) -> Result<ReleaseMetadata> {
        let repository = repository_url()?;
        Ok(match &self.trusted_keys {
            Some(keys) => {
                release::fetch_latest_metadata_with_keys(&self.downloader, &repository, keys)?
            }
            None => release::fetch_latest_metadata(&self.downloader, &repository)?,
        })
    }

    fn activate_download(
        &self,
        metadata: ReleaseMetadata,
        archive: release::DownloadedArchive,
        version: Version,
    ) -> Result<ActivationResult> {
        let _ = self.recover_if_managed()?;
        if lexists(&self.command_path)? && !lexists(&self.install_state_path())? {
            #[cfg(windows)]
            let proven = self.prove_retained_launcher_ownership()?;
            #[cfg(not(windows))]
            let proven = false;
            if !proven {
                return Err(InstallError::Invalid(format!(
                    "refusing to replace existing unmanaged command {}",
                    self.command_path.display()
                )));
            }
        }
        let lock = self.lock_for_mutation()?;
        let result = self.activate_download_locked(&lock, metadata, archive, version);
        drop(lock);
        result
    }

    fn activate_download_locked(
        &self,
        _lock: &InstallLock,
        metadata: ReleaseMetadata,
        archive: release::DownloadedArchive,
        version: Version,
    ) -> Result<ActivationResult> {
        let target = current_target()?;
        if archive.target != target {
            return Err(InstallError::Invalid(format!(
                "downloaded archive target {} differs from current target {target}",
                archive.target
            )));
        }
        if metadata.version != version {
            return Err(InstallError::Invalid(
                "downloaded metadata version differs from requested version".to_string(),
            ));
        }
        self.verify_metadata(&metadata, target)?;
        let install = self.read_install_state()?;
        if let Some(install) = &install {
            validate_install_state(install)?;
        }
        let from = self.read_pointer_unlocked()?;
        #[cfg(windows)]
        let retained_launcher = if install.is_none() {
            self.retained_launcher_ownership()?
        } else {
            None
        };
        #[cfg(not(windows))]
        let retained_launcher: Option<LauncherOwnership> = None;
        let managed = install.is_some() && from.is_some();
        if managed {
            let active = from.as_ref().expect("managed pointer is present");
            if version <= *active {
                if version == *active {
                    let state = self.verify_final_version(
                        active,
                        install.as_ref().map(|s| s.installation_id.as_str()),
                    )?;
                    if fs::read(self.version_manifest_path(active))? != metadata.manifest_bytes
                        || fs::read(self.version_signature_path(active))?
                            != metadata.signature_bytes
                    {
                        return Err(InstallError::Invalid(
                            "existing final version has different signed metadata".to_string(),
                        ));
                    }
                    self.ensure_command_owned(Some(active), install.as_ref())?;
                    let _ = state;
                    return Ok(ActivationResult {
                        version: active.clone(),
                        changed: false,
                    });
                }
                return Err(InstallError::Downgrade {
                    current: active.clone(),
                    requested: version.clone(),
                });
            }
        }

        self.ensure_private_layout()?;
        let installation_id = self.choose_installation_id(&version, install.as_ref())?;
        if from.is_some() || !lexists(&self.command_path)? {
            self.cleanup_staging()?;
        }
        #[cfg(windows)]
        if from.is_none() && install.is_none() {
            self.cleanup_staging()?;
        }
        self.ensure_command_owned(from.as_ref(), install.as_ref())?;
        let already_managed = install.is_some() || retained_launcher.is_some();

        let final_dir = self.version_dir(&version);
        let existing = lexists(&final_dir)?;
        let mut transaction = None;
        if existing {
            let _state = self.verify_final_version(&version, Some(&installation_id))?;
            if fs::read(self.version_manifest_path(&version))? != metadata.manifest_bytes
                || fs::read(self.version_signature_path(&version))? != metadata.signature_bytes
            {
                return Err(InstallError::Invalid(
                    "existing final version has different signed metadata".to_string(),
                ));
            }
            if let Some(install) = &install {
                self.verify_installed_launcher(install)?;
            } else {
                #[cfg(windows)]
                {
                    let (launcher_size, launcher_sha256) =
                        if let Some(launcher) = retained_launcher.as_ref() {
                            (launcher.size, launcher.sha256.as_str())
                        } else {
                            let launcher = _state.launcher.as_ref().ok_or_else(|| {
                                InstallError::Invalid(
                                    "Windows version state lacks launcher metadata".into(),
                                )
                            })?;
                            (launcher.size, launcher.sha256.as_str())
                        };
                    executable::ensure_regular_file(&self.command_path)?;
                    verify_file_digest(
                        &self.command_path,
                        launcher_size,
                        launcher_sha256,
                        "installed launcher",
                    )?;
                }
            }
        } else {
            let tx = self.create_transaction()?;
            transaction = Some(tx.clone());
            self.write_downloaded_version(
                &tx,
                &metadata,
                &archive,
                &installation_id,
                target,
                already_managed,
            )?;
            self.fault(FaultPoint::PayloadWritten)?;
            self.verify_staged_version(&tx, &version, &installation_id)?;
            self.fault(FaultPoint::Verified)?;
            self.sync_staging_transaction(&tx)?;
            self.fault(FaultPoint::StagingSynced)?;

            if lexists(&final_dir)? {
                return Err(InstallError::Invalid(format!(
                    "retained version directory appeared during installation: {}",
                    final_dir.display()
                )));
            }
            executable::rename_new(&tx.version_dir, &final_dir)?;
            self.fault(FaultPoint::VersionRenamed)?;
        }

        self.ensure_command_parent()?;
        let prior_install = match (install, retained_launcher) {
            (Some(install), _) => Some(install),
            (None, Some(launcher)) => Some(InstallState {
                schema_version: STATE_SCHEMA_VERSION,
                active: None,
                previous: None,
                installation_id: installation_id.clone(),
                launcher: Some(launcher),
            }),
            (None, None) => None,
        };
        self.activate_pointer_and_state(
            from,
            version.clone(),
            installation_id,
            prior_install,
            transaction
                .as_ref()
                .map(|transaction| transaction.id.clone()),
            transaction.as_ref(),
        )?;
        Ok(ActivationResult {
            version,
            changed: true,
        })
    }

    fn activate_existing(&self, target: Version) -> Result<ActivationResult> {
        let lock = self.lock_for_mutation()?;
        let result = self.activate_existing_locked(&lock, target);
        drop(lock);
        result
    }

    fn activate_existing_locked(
        &self,
        _lock: &InstallLock,
        target: Version,
    ) -> Result<ActivationResult> {
        let install = self.read_install_state()?.ok_or(InstallError::Unmanaged)?;
        let from = self
            .read_pointer_unlocked()?
            .ok_or(InstallError::Unmanaged)?;
        if target >= from {
            return Err(InstallError::Invalid(
                "retained activation target must be older than active pointer".to_string(),
            ));
        }
        self.ensure_command_owned(Some(&from), Some(&install))?;
        self.verify_final_version(&target, Some(&install.installation_id))?;
        self.verify_installed_launcher(&install)?;
        self.fault(FaultPoint::Verified)?;
        self.ensure_command_parent()?;
        self.activate_pointer_and_state(
            Some(from),
            target,
            install.installation_id.clone(),
            Some(install),
            None,
            None,
        )
    }

    fn activate_pointer_and_state(
        &self,
        from: Option<Version>,
        to: Version,
        installation_id: String,
        prior_install: Option<InstallState>,
        transaction_id: Option<String>,
        transaction: Option<&Transaction>,
    ) -> Result<ActivationResult> {
        let pending = PendingActivation {
            schema_version: STATE_SCHEMA_VERSION,
            from: from.clone(),
            to: to.clone(),
            transaction_id: transaction_id.unwrap_or(random_id("transaction")?),
        };
        self.write_pending(&pending)?;
        self.fault(FaultPoint::PendingWritten)?;
        self.switch_pointer(&to)?;
        self.fault(FaultPoint::PointerSwitched)?;

        let launcher = self.launcher_ownership_for(&to, prior_install.as_ref())?;
        let install = InstallState {
            schema_version: STATE_SCHEMA_VERSION,
            active: Some(to.clone()),
            previous: from,
            installation_id,
            launcher,
        };
        self.write_install_state(&install)?;
        self.fault(FaultPoint::InstallWritten)?;
        self.remove_pending()?;
        self.fault(FaultPoint::PendingRemoved)?;
        if let Some(transaction) = transaction {
            self.remove_transaction_after_rename(transaction)?;
        }
        self.sync_affected_parents()?;
        self.fault(FaultPoint::ParentsSynced)?;
        Ok(ActivationResult {
            version: to,
            changed: true,
        })
    }

    fn recover_locked(&self) -> Result<bool> {
        self.ensure_private_layout()?;
        let pending = self.read_pending()?;
        let install = self.read_install_state()?;
        if let Some(install) = &install {
            validate_install_state(install)?;
        }
        let pointer = self.read_pointer_unlocked()?;
        if let Some(pending) = pending {
            validate_pending(&pending)?;
            match pointer {
                Some(pointer) if pointer == pending.to => {
                    let state = self.verify_final_version(
                        &pending.to,
                        install.as_ref().map(|state| state.installation_id.as_str()),
                    )?;
                    if let Some(install) = &install {
                        if install.installation_id != state.installation_id {
                            return Err(InstallError::Invalid(
                                "pending target installation id differs from install.json"
                                    .to_string(),
                            ));
                        }
                        self.verify_installed_launcher(install)?;
                    }
                    let launcher = self.launcher_ownership_for(&pending.to, install.as_ref())?;
                    let repaired = InstallState {
                        schema_version: STATE_SCHEMA_VERSION,
                        active: Some(pending.to.clone()),
                        previous: pending.from.clone(),
                        installation_id: state.installation_id,
                        launcher,
                    };
                    self.write_install_state(&repaired)?;
                    self.fault(FaultPoint::InstallWritten)?;
                    self.remove_pending()?;
                    self.fault(FaultPoint::PendingRemoved)?;
                    self.sync_affected_parents()?;
                    self.fault(FaultPoint::ParentsSynced)?;
                    self.cleanup_staging()?;
                    return Ok(true);
                }
                Some(pointer) if Some(pointer.clone()) == pending.from => {
                    self.remove_pending()?;
                    self.fault(FaultPoint::PendingRemoved)?;
                    self.sync_affected_parents()?;
                    self.fault(FaultPoint::ParentsSynced)?;
                    self.cleanup_staging()?;
                    return Ok(install.is_some());
                }
                Some(pointer) => {
                    return Err(InstallError::Invalid(format!(
                        "pending activation pointer is neither from nor to: {pointer}"
                    )));
                }
                None if pending.from.is_none() => {
                    self.remove_pending()?;
                    self.fault(FaultPoint::PendingRemoved)?;
                    self.sync_affected_parents()?;
                    self.fault(FaultPoint::ParentsSynced)?;
                    self.cleanup_staging()?;
                    return Ok(install.is_some());
                }
                None => {
                    return Err(InstallError::Invalid(
                        "pending activation lost its prior pointer".to_string(),
                    ));
                }
            }
        }

        let Some(pointer) = pointer else {
            if install.as_ref().is_some_and(|state| state.active.is_some()) {
                return Err(InstallError::Invalid(
                    "install.json claims an active version but the authoritative pointer is missing"
                    .to_string(),
                ));
            }
            #[cfg(windows)]
            if self.staging_has_launcher_marker()? {
                self.cleanup_staging()?;
            }
            return Ok(false);
        };
        // The pointer is authoritative during reconciliation.  In particular, do not reject a
        // valid pointer merely because a descriptive installation id is stale; regenerate the
        // descriptive document from the pointer below.
        let state = self.verify_final_version(&pointer, None)?;
        let metadata_matches = install.as_ref().is_some_and(|install| {
            state.schema_version == STATE_SCHEMA_VERSION
                && install.active.as_ref() == Some(&pointer)
                && install.installation_id == state.installation_id
        });
        let needs_repair = match &install {
            Some(install) => {
                install.schema_version != STATE_SCHEMA_VERSION
                    || install.active.as_ref() != Some(&pointer)
                    || install.installation_id != state.installation_id
                    || self.launcher_state_disagrees(install, state.launcher.as_ref())
            }
            None => true,
        } || !metadata_matches;
        if needs_repair {
            let previous = install
                .as_ref()
                .and_then(|state| state.previous.clone())
                .filter(|previous| previous != &pointer)
                .filter(|previous| {
                    self.verify_final_version(previous, Some(&state.installation_id))
                        .is_ok()
                });
            let launcher = self.launcher_ownership_for(&pointer, install.as_ref())?;
            let repaired = InstallState {
                schema_version: STATE_SCHEMA_VERSION,
                active: Some(pointer),
                previous,
                installation_id: state.installation_id,
                launcher,
            };
            // Reconciliation is intentionally metadata-only: never switch the authoritative
            // pointer while repairing a descriptive document.
            self.write_install_state(&repaired)?;
            self.sync_affected_parents()?;
        }
        self.cleanup_staging()?;
        Ok(true)
    }

    fn lock_existing(&self) -> Result<InstallLock> {
        self.open_lock(false)
    }

    fn lock_for_mutation(&self) -> Result<InstallLock> {
        self.ensure_root()?;
        self.open_lock(true)
    }

    fn open_lock(&self, create: bool) -> Result<InstallLock> {
        if create {
            fs_security::ensure_private_dir(&self.root)?;
        }
        let path = self.root.join(LOCK_FILE);
        match fs::symlink_metadata(&path) {
            Ok(metadata)
                if metadata.file_type().is_symlink()
                    || !metadata.is_file()
                    || executable::is_reparse_point(&path)? =>
            {
                return Err(InstallError::Invalid(
                    "managed lock path is not a regular file".into(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound && create => {}
            Err(error) => return Err(error.into()),
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Do not follow a symlink planted after the metadata preflight and before open.
            options.custom_flags(libc::O_NOFOLLOW);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            // Open a reparse point itself so the post-open metadata check cannot be redirected.
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        if create {
            options.create(true);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        fs4::FileExt::lock(&file)?;
        self.fault(FaultPoint::LockAcquired)?;
        Ok(InstallLock { file })
    }

    fn ensure_root(&self) -> Result<()> {
        if lexists(&self.root)? {
            fs_security::ensure_private_dir(&self.root)?;
            return Ok(());
        }
        fs_security::ensure_private_dir(&self.root)?;
        Ok(())
    }

    fn ensure_private_layout(&self) -> Result<()> {
        fs_security::ensure_private_dir(&self.root)?;
        fs_security::ensure_private_dir(&self.versions_dir())?;
        fs_security::ensure_private_dir(&self.staging_dir())?;
        #[cfg(windows)]
        fs_security::ensure_private_dir(&self.bin_dir())?;
        Ok(())
    }

    fn create_transaction(&self) -> Result<Transaction> {
        self.ensure_private_layout()?;
        let transaction_id = random_id("transaction")?;
        let transaction_dir = self.staging_dir().join(&transaction_id);
        fs_security::ensure_private_dir(&transaction_dir)?;
        let version_dir = transaction_dir.join("version");
        fs_security::ensure_private_dir(&version_dir)?;
        self.fault(FaultPoint::StagingCreated)?;
        Ok(Transaction {
            id: transaction_id,
            dir: transaction_dir,
            version_dir,
        })
    }

    fn write_downloaded_version(
        &self,
        transaction: &Transaction,
        metadata: &ReleaseMetadata,
        archive: &release::DownloadedArchive,
        installation_id: &str,
        target: ReleaseTarget,
        already_managed: bool,
    ) -> Result<()> {
        #[cfg(not(windows))]
        let _ = already_managed;
        let selected = self.verify_metadata(metadata, target)?;
        let archive_path = transaction.dir.join(&selected.asset.archive);
        executable::create_new_file(&archive_path, &archive.bytes, Some(0o600))?;
        release::extract_archive_file(&archive_path, selected.asset, &transaction.version_dir)?;
        executable::atomic_replace_file_with_mode(
            &transaction.version_dir.join(MANIFEST_FILE),
            &metadata.manifest_bytes,
            Some(0o600),
        )?;
        executable::atomic_replace_file_with_mode(
            &transaction.version_dir.join(SIGNATURE_FILE),
            &metadata.signature_bytes,
            Some(0o600),
        )?;
        let payload = transaction.version_dir.join(target.payload_name());
        executable::set_executable(&payload)?;
        let state = VersionState {
            schema_version: STATE_SCHEMA_VERSION,
            version: metadata.version.clone(),
            target,
            binary_sha256: selected.asset.payload.sha256.clone(),
            size: selected.asset.payload.size,
            installation_id: installation_id.to_string(),
            launcher: selected.launcher().map(launcher_metadata),
        };
        self.write_version_state(&transaction.version_dir, &state)?;

        #[cfg(windows)]
        {
            let launcher = selected.launcher().ok_or_else(|| {
                InstallError::Invalid("Windows release is missing signed launcher metadata".into())
            })?;
            if launcher.protocol != 1 {
                return Err(InstallError::Invalid(
                    "managed Windows installations require launcher protocol 1".into(),
                ));
            }
            let staged_launcher = transaction.version_dir.join(LAUNCHER_WINDOWS);
            executable::ensure_regular_file(&staged_launcher)?;
            verify_file_digest(
                &staged_launcher,
                launcher.size,
                &launcher.sha256,
                "launcher",
            )?;
            if !lexists(&self.command_path)? {
                if already_managed {
                    return Err(InstallError::Invalid(
                        "managed Windows launcher is missing and cannot be recreated by an update"
                            .into(),
                    ));
                }
                self.ensure_command_parent()?;
                executable::atomic_replace_file_with_mode(
                    &transaction.dir.join(LAUNCHER_CREATED_MARKER),
                    launcher.sha256.as_bytes(),
                    Some(0o600),
                )?;
                executable::create_new_file(
                    &self.command_path,
                    &fs::read(&staged_launcher)?,
                    None,
                )?;
            } else if !already_managed {
                return Err(InstallError::Invalid(
                    "existing Windows launcher requires a proven managed owner".into(),
                ));
            } else if let Some(install) = self.read_install_state()? {
                self.verify_installed_launcher(&install)?;
            } else if !self.prove_retained_launcher_ownership()? {
                return Err(InstallError::Invalid(
                    "existing Windows launcher ownership could not be re-established".into(),
                ));
            }
            fs::remove_file(staged_launcher)?;
        }
        #[cfg(not(windows))]
        {
            let _ = LAUNCHER_WINDOWS;
        }
        Ok(())
    }

    fn verify_metadata<'a>(
        &self,
        metadata: &'a ReleaseMetadata,
        target: ReleaseTarget,
    ) -> Result<release::SelectedAsset<'a>> {
        if metadata.version != metadata.manifest.version {
            return Err(InstallError::Invalid(
                "release metadata version does not match its manifest".to_string(),
            ));
        }
        if metadata.manifest.version.to_string() != metadata.version.to_string() {
            return Err(InstallError::Invalid(
                "release metadata version is not canonical".to_string(),
            ));
        }
        let verified = match &self.trusted_keys {
            Some(keys) => release::verify_manifest_with_keys(
                &metadata.manifest_bytes,
                &metadata.signature_bytes,
                keys,
            )?,
            None => release::verify_manifest(&metadata.manifest_bytes, &metadata.signature_bytes)?,
        };
        if verified.key_id != metadata.verified_signature.key_id {
            return Err(InstallError::Invalid(
                "release signature result differs from downloaded metadata".into(),
            ));
        }
        let parsed = release::ReleaseManifest::from_bytes(&metadata.manifest_bytes)?;
        if parsed != metadata.manifest {
            return Err(InstallError::Invalid(
                "downloaded release metadata changed after verification".into(),
            ));
        }
        Ok(metadata.manifest.asset_for(target)?)
    }

    fn verify_staged_version(
        &self,
        transaction: &Transaction,
        version: &Version,
        installation_id: &str,
    ) -> Result<VersionState> {
        let state =
            self.verify_version_dir_inner(&transaction.version_dir, Some(installation_id))?;
        if &state.version != version {
            return Err(InstallError::Invalid(
                "staged version state differs from transaction version".to_string(),
            ));
        }
        Ok(state)
    }

    fn verify_final_version(
        &self,
        version: &Version,
        installation_id: Option<&str>,
    ) -> Result<VersionState> {
        let state = self.verify_version_dir_inner(&self.version_dir(version), installation_id)?;
        if state.version != *version {
            return Err(InstallError::Invalid(
                "version.json does not match its immutable directory name".into(),
            ));
        }
        Ok(state)
    }

    fn verify_version_dir_inner(
        &self,
        dir: &Path,
        installation_id: Option<&str>,
    ) -> Result<VersionState> {
        fs_security::ensure_private_dir(dir)?;
        let state = read_json::<VersionState>(&dir.join(VERSION_FILE))?
            .ok_or_else(|| InstallError::Invalid("version.json is missing".into()))?;
        validate_version_state(&state)?;
        if let Some(expected) = installation_id
            && state.installation_id != expected
        {
            return Err(InstallError::Invalid(
                "version installation id differs from managed installation".into(),
            ));
        }
        let target = current_target()?;
        if state.target != target {
            return Err(InstallError::Invalid(
                "version target differs from current host target".into(),
            ));
        }
        let manifest_bytes =
            read_regular_limited(&dir.join(MANIFEST_FILE), release::MAX_METADATA_SIZE)?;
        let signature_bytes =
            read_regular_limited(&dir.join(SIGNATURE_FILE), release::MAX_METADATA_SIZE)?;
        let verified = match &self.trusted_keys {
            Some(keys) => {
                release::verify_manifest_with_keys(&manifest_bytes, &signature_bytes, keys)?
            }
            None => release::verify_manifest(&manifest_bytes, &signature_bytes)?,
        };
        let manifest = release::ReleaseManifest::from_bytes(&manifest_bytes)?;
        if manifest.version != state.version {
            return Err(InstallError::Invalid(
                "version state does not match signed release version".into(),
            ));
        }
        let selected = manifest.asset_for(target)?;
        if selected.asset.payload.path
            != format!(
                "{}/{}",
                target.root_name(&state.version),
                target.payload_name()
            )
        {
            return Err(InstallError::Invalid(
                "signed payload path is not canonical".into(),
            ));
        }
        if verified.key_id.is_empty() {
            return Err(InstallError::Invalid(
                "release signature key id is empty".into(),
            ));
        }
        if state.binary_sha256 != selected.asset.payload.sha256
            || state.size != selected.asset.payload.size
        {
            return Err(InstallError::Invalid(
                "version state payload digest does not match signed manifest".into(),
            ));
        }
        let payload = dir.join(target.payload_name());
        executable::ensure_regular_file(&payload)?;
        verify_file_digest(&payload, state.size, &state.binary_sha256, "payload")?;
        if target.is_windows() {
            let launcher = selected.launcher().ok_or_else(|| {
                InstallError::Invalid("Windows release is missing launcher metadata".into())
            })?;
            validate_signed_launcher(launcher)?;
            if state.launcher.as_ref() != Some(&launcher_metadata(launcher)) {
                return Err(InstallError::Invalid(
                    "version state launcher metadata does not match signed manifest".into(),
                ));
            }
        } else if state.launcher.is_some() || selected.launcher().is_some() {
            return Err(InstallError::Invalid(
                "Unix version contains Windows launcher metadata".into(),
            ));
        }
        validate_exact_version_members(dir, target)?;
        Ok(state)
    }

    fn write_version_state(&self, dir: &Path, state: &VersionState) -> Result<()> {
        validate_version_state(state)?;
        let bytes = serde_json::to_vec(state)?;
        executable::atomic_replace_file_with_mode(&dir.join(VERSION_FILE), &bytes, Some(0o600))?;
        Ok(())
    }

    fn sync_staging_transaction(&self, transaction: &Transaction) -> Result<()> {
        sync_regular_files(&transaction.version_dir)?;
        sync_regular_files(&transaction.dir)?;
        executable::sync_dir(&self.staging_dir())?;
        Ok(())
    }

    fn remove_transaction_after_rename(&self, transaction: &Transaction) -> Result<()> {
        if lexists(&transaction.dir)? {
            fs::remove_dir_all(&transaction.dir)?;
            executable::sync_dir(&self.staging_dir())?;
        }
        Ok(())
    }

    fn write_pending(&self, pending: &PendingActivation) -> Result<()> {
        validate_pending(pending)?;
        let bytes = serde_json::to_vec(pending)?;
        executable::atomic_replace_file_with_mode(&self.pending_path(), &bytes, Some(0o600))?;
        Ok(())
    }

    fn remove_pending(&self) -> Result<()> {
        match fs::symlink_metadata(self.pending_path()) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(
                InstallError::Invalid("pending activation is not a regular file".into()),
            ),
            Ok(_) => {
                fs::remove_file(self.pending_path())?;
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn write_install_state(&self, state: &InstallState) -> Result<()> {
        validate_install_state(state)?;
        let bytes = serde_json::to_vec(state)?;
        executable::atomic_replace_file_with_mode(&self.install_state_path(), &bytes, Some(0o600))?;
        Ok(())
    }

    fn switch_pointer(&self, version: &Version) -> Result<()> {
        let payload = self.payload_path(version);
        executable::ensure_regular_file(&payload)?;
        #[cfg(unix)]
        {
            executable::atomic_switch_symlink(&self.command_path, &payload)?;
        }
        #[cfg(windows)]
        {
            executable::atomic_replace_file(&self.active_path(), version.to_string().as_bytes())?;
        }
        Ok(())
    }

    fn read_pointer_unlocked(&self) -> Result<Option<Version>> {
        #[cfg(unix)]
        {
            let Some(target) = executable::read_symlink(&self.command_path)? else {
                return Ok(None);
            };
            if !target.is_absolute() {
                return Err(InstallError::Invalid(
                    "managed command symlink must contain an absolute payload path".into(),
                ));
            }
            let version = parse_pointer_version(&self.root, &target, PAYLOAD_UNIX)?;
            if !same_path(&target, &self.payload_path(&version)) {
                return Err(InstallError::Invalid(
                    "managed command symlink points outside the current installation".into(),
                ));
            }
            Ok(Some(version))
        }
        #[cfg(windows)]
        {
            let path = self.active_path();
            match fs::symlink_metadata(&path) {
                Ok(metadata)
                    if metadata.file_type().is_symlink()
                        || executable::is_reparse_point(&path)?
                        || !metadata.is_file() =>
                {
                    Err(InstallError::Invalid(
                        "active selector is not a regular file".into(),
                    ))
                }
                Ok(_) => {
                    let bytes = read_regular_limited(&path, 128)?;
                    let raw = std::str::from_utf8(&bytes).map_err(|_| {
                        InstallError::Invalid("active selector is not UTF-8".into())
                    })?;
                    let version = parse_canonical_version(raw)?;
                    Ok(Some(version))
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error.into()),
            }
        }
    }

    fn pointer_path_exists(&self) -> Result<bool> {
        #[cfg(unix)]
        {
            match fs::symlink_metadata(&self.command_path) {
                Ok(metadata) => Ok(metadata.file_type().is_symlink()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(error.into()),
            }
        }
        #[cfg(windows)]
        {
            Ok(lexists(&self.active_path())?)
        }
    }

    fn require_managed_current(&self) -> Result<Version> {
        let current = self
            .read_pointer_unlocked()?
            .ok_or(InstallError::Unmanaged)?;
        let install = self.read_install_state()?.ok_or(InstallError::Unmanaged)?;
        validate_install_state(&install)?;
        if install.active.as_ref() != Some(&current) {
            return Err(InstallError::Invalid(
                "managed install metadata does not describe the authoritative pointer".into(),
            ));
        }
        self.verify_final_version(&current, Some(&install.installation_id))?;
        self.ensure_command_owned(Some(&current), Some(&install))?;
        Ok(current)
    }

    fn choose_installation_id(
        &self,
        version: &Version,
        install: Option<&InstallState>,
    ) -> Result<String> {
        if let Some(install) = install {
            validate_install_state(install)?;
            return Ok(install.installation_id.clone());
        }
        let final_dir = self.version_dir(version);
        if lexists(&final_dir)? {
            return Ok(self.verify_final_version(version, None)?.installation_id);
        }
        if lexists(&self.versions_dir())? {
            let mut existing_id = None;
            for entry in fs::read_dir(self.versions_dir())? {
                let path = entry?.path();
                let metadata = fs::symlink_metadata(&path)?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(InstallError::Invalid(
                        "versions contains a non-directory entry".into(),
                    ));
                }
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| {
                        InstallError::Invalid("version directory is not UTF-8".into())
                    })?;
                let retained = parse_canonical_version(name)?;
                let state = self.verify_final_version(&retained, None)?;
                if let Some(existing) = &existing_id {
                    if existing != &state.installation_id {
                        return Err(InstallError::Invalid(
                            "retained versions belong to different installations".into(),
                        ));
                    }
                } else {
                    existing_id = Some(state.installation_id);
                }
            }
            if let Some(existing_id) = existing_id {
                return Ok(existing_id);
            }
        }
        random_id("installation")
    }

    fn ensure_command_owned(
        &self,
        active: Option<&Version>,
        install: Option<&InstallState>,
    ) -> Result<()> {
        let exists = lexists(&self.command_path)?;
        if !exists {
            #[cfg(windows)]
            if active.is_some() && install.is_some() {
                return Err(InstallError::Invalid(
                    "managed Windows launcher is missing and cannot be recreated by an update"
                        .into(),
                ));
            }
            return Ok(());
        }
        let Some(active) = active else {
            #[cfg(windows)]
            if install.is_none() && self.prove_retained_launcher_ownership()? {
                // A first install can create the stable launcher before its version directory is
                // renamed into place.  If the process dies at that boundary, the marker and the
                // retained version prove that this launcher is ours even though install.json and
                // active do not exist yet.
                return Ok(());
            }
            return Err(InstallError::Invalid(format!(
                "refusing to replace existing unmanaged command {}",
                self.command_path.display()
            )));
        };
        let Some(install) = install else {
            return Err(InstallError::Invalid(
                "existing command has no installation ownership record".into(),
            ));
        };
        if install.active.as_ref() != Some(active) {
            return Err(InstallError::Invalid(
                "existing command ownership does not match the active pointer".into(),
            ));
        }
        let version_state = self.verify_final_version(active, Some(&install.installation_id))?;
        #[cfg(unix)]
        {
            let Some(pointer) = executable::read_symlink(&self.command_path)? else {
                return Err(InstallError::Invalid(
                    "managed Unix command is missing its symlink".into(),
                ));
            };
            if !pointer.is_absolute() || !same_path(&pointer, &self.payload_path(active)) {
                return Err(InstallError::Invalid(
                    "existing command symlink is not owned by this installation".into(),
                ));
            }
            let _ = version_state;
        }
        #[cfg(windows)]
        {
            self.verify_installed_launcher(install)?;
            let _ = version_state;
        }
        Ok(())
    }

    fn launcher_ownership_for(
        &self,
        version: &Version,
        prior: Option<&InstallState>,
    ) -> Result<Option<LauncherOwnership>> {
        #[cfg(unix)]
        {
            let _ = (version, prior);
            Ok(None)
        }
        #[cfg(windows)]
        {
            if let Some(prior) = prior {
                self.verify_installed_launcher(prior)?;
                return Ok(prior.launcher.clone());
            }
            let state = self.verify_final_version(version, None)?;
            let launcher = state.launcher.ok_or_else(|| {
                InstallError::Invalid("Windows version has no signed launcher metadata".into())
            })?;
            let ownership = LauncherOwnership {
                owned: true,
                path: self.command_path.to_string_lossy().into_owned(),
                sha256: launcher.sha256,
                size: launcher.size,
                protocol: launcher.protocol,
            };
            self.verify_installed_launcher_record(&ownership)?;
            Ok(Some(ownership))
        }
    }

    fn verify_installed_launcher(&self, install: &InstallState) -> Result<()> {
        #[cfg(unix)]
        {
            let _ = install;
            Ok(())
        }
        #[cfg(windows)]
        {
            let launcher = install.launcher.as_ref().ok_or_else(|| {
                InstallError::Invalid(
                    "managed Windows installation lacks launcher ownership".into(),
                )
            })?;
            self.verify_installed_launcher_record(launcher)
        }
    }

    #[cfg(windows)]
    fn verify_installed_launcher_record(&self, launcher: &LauncherOwnership) -> Result<()> {
        #[cfg(unix)]
        {
            let _ = launcher;
            Ok(())
        }
        #[cfg(windows)]
        {
            if !launcher.owned
                || launcher.protocol != 1
                || launcher.path != self.command_path.to_string_lossy()
            {
                return Err(InstallError::Invalid(
                    "Windows launcher ownership/protocol metadata is invalid".into(),
                ));
            }
            executable::ensure_regular_file(&self.command_path)?;
            verify_file_digest(
                &self.command_path,
                launcher.size,
                &launcher.sha256,
                "installed launcher",
            )
        }
    }

    fn launcher_state_disagrees(
        &self,
        install: &InstallState,
        signed: Option<&LauncherMetadata>,
    ) -> bool {
        #[cfg(unix)]
        {
            let _ = signed;
            install.launcher.is_some()
        }
        #[cfg(windows)]
        {
            let Some(launcher) = install.launcher.as_ref() else {
                return true;
            };
            signed.is_none()
                || !launcher.owned
                || launcher.protocol != 1
                || launcher.path != self.command_path.to_string_lossy()
                || self.verify_installed_launcher_record(launcher).is_err()
        }
    }

    fn cleanup_staging(&self) -> Result<()> {
        let staging = self.staging_dir();
        if !lexists(&staging)? {
            return Ok(());
        }
        fs_security::ensure_private_dir(&staging)?;
        #[cfg(windows)]
        let remove_orphan_launcher =
            self.read_pointer_unlocked()?.is_none() && self.read_install_state()?.is_none();
        for entry in fs::read_dir(&staging)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || executable::is_reparse_point(&path)?
            {
                return Err(InstallError::Invalid(
                    "staging contains a non-directory entry".into(),
                ));
            }
            #[cfg(windows)]
            if remove_orphan_launcher {
                let marker = path.join(LAUNCHER_CREATED_MARKER);
                if lexists(&marker)? {
                    let expected = std::str::from_utf8(&read_regular_limited(&marker, 128)?)
                        .map_err(|_| {
                            InstallError::Invalid("staging launcher marker is not UTF-8".into())
                        })?
                        .to_string();
                    if !lower_sha256(&expected) {
                        return Err(InstallError::Invalid(
                            "staging launcher marker has an invalid digest".into(),
                        ));
                    }
                    if lexists(&self.command_path)? {
                        executable::ensure_regular_file(&self.command_path)?;
                        let actual = release::sha256_file(&self.command_path)?;
                        if actual != expected {
                            return Err(InstallError::Invalid(
                                "orphaned launcher differs from its transaction marker".into(),
                            ));
                        }
                        if !self.retained_launcher_matches(&expected)? {
                            fs::remove_file(&self.command_path)?;
                        }
                    }
                }
            }
            fs::remove_dir_all(path)?;
        }
        executable::sync_dir(&staging)?;
        Ok(())
    }

    #[cfg(windows)]
    fn retained_launcher_matches(&self, expected: &str) -> Result<bool> {
        let versions = self.versions_dir();
        if !lexists(&versions)? {
            return Ok(false);
        }
        for entry in fs::read_dir(versions)? {
            let path = entry?.path();
            if !fs::symlink_metadata(&path)?.is_dir() {
                continue;
            }
            let Some(state) = read_json::<VersionState>(&path.join(VERSION_FILE))? else {
                continue;
            };
            if state
                .launcher
                .as_ref()
                .is_some_and(|launcher| launcher.sha256 == expected)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    #[cfg(windows)]
    fn staging_has_launcher_marker(&self) -> Result<bool> {
        let staging = self.staging_dir();
        if !lexists(&staging)? {
            return Ok(false);
        }
        for entry in fs::read_dir(staging)? {
            let path = entry?.path();
            if fs::symlink_metadata(&path)?.is_dir()
                && lexists(&path.join(LAUNCHER_CREATED_MARKER))?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    #[cfg(windows)]
    fn prove_retained_launcher_ownership(&self) -> Result<bool> {
        Ok(self.retained_launcher_ownership()?.is_some())
    }

    #[cfg(windows)]
    fn retained_launcher_ownership(&self) -> Result<Option<LauncherOwnership>> {
        let versions = self.versions_dir();
        if !lexists(&versions)? {
            return Ok(None);
        }
        for entry in fs::read_dir(versions)? {
            let path = entry?.path();
            if !fs::symlink_metadata(&path)?.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let version = parse_canonical_version(name)?;
            let state = self.verify_final_version(&version, None)?;
            if let Some(launcher) = state.launcher
                && lexists(&self.command_path)?
            {
                executable::ensure_regular_file(&self.command_path)?;
                if verify_file_digest(
                    &self.command_path,
                    launcher.size,
                    &launcher.sha256,
                    "installed launcher",
                )
                .is_ok()
                {
                    return Ok(Some(LauncherOwnership {
                        owned: true,
                        path: self.command_path.to_string_lossy().into_owned(),
                        sha256: launcher.sha256,
                        size: launcher.size,
                        protocol: launcher.protocol,
                    }));
                }
            }
        }
        Ok(None)
    }

    fn sync_affected_parents(&self) -> Result<()> {
        executable::sync_dir(&self.root)?;
        if lexists(&self.versions_dir())? {
            executable::sync_dir(&self.versions_dir())?;
        }
        if let Some(parent) = self.command_path.parent()
            && lexists(parent)?
        {
            executable::sync_dir(parent)?;
        }
        Ok(())
    }

    fn ensure_command_parent(&self) -> Result<()> {
        let parent = self.command_path.parent().ok_or_else(|| {
            InstallError::Invalid(format!(
                "managed command has no parent directory: {}",
                self.command_path.display()
            ))
        })?;
        #[cfg(windows)]
        {
            fs_security::ensure_private_dir(parent)?;
        }
        #[cfg(unix)]
        {
            fs::create_dir_all(parent)?;
            let metadata = fs::symlink_metadata(parent)?;
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || executable::is_reparse_point(parent)?
            {
                return Err(InstallError::Invalid(format!(
                    "managed command parent is not a real directory: {}",
                    parent.display()
                )));
            }
        }
        Ok(())
    }

    fn fault(&self, point: FaultPoint) -> Result<()> {
        self.fault
            .after(point)
            .map_err(|source| InstallError::Fault { point, source })
    }

    fn read_install_state(&self) -> Result<Option<InstallState>> {
        read_json(&self.install_state_path())
    }

    fn read_pending(&self) -> Result<Option<PendingActivation>> {
        read_json(&self.pending_path())
    }

    fn versions_dir(&self) -> PathBuf {
        self.root.join(VERSIONS_DIR)
    }

    fn staging_dir(&self) -> PathBuf {
        self.root.join(STAGING_DIR)
    }

    fn version_dir(&self, version: &Version) -> PathBuf {
        self.versions_dir().join(version.to_string())
    }

    fn payload_path(&self, version: &Version) -> PathBuf {
        self.version_dir(version).join(current_payload_name())
    }

    fn version_manifest_path(&self, version: &Version) -> PathBuf {
        self.version_dir(version).join(MANIFEST_FILE)
    }

    fn version_signature_path(&self, version: &Version) -> PathBuf {
        self.version_dir(version).join(SIGNATURE_FILE)
    }

    fn install_state_path(&self) -> PathBuf {
        self.root.join(INSTALL_FILE)
    }

    fn pending_path(&self) -> PathBuf {
        self.root.join(PENDING_FILE)
    }

    #[cfg(windows)]
    fn active_path(&self) -> PathBuf {
        self.root.join(ACTIVE_FILE)
    }

    #[cfg(windows)]
    fn bin_dir(&self) -> PathBuf {
        self.root.join(BIN_DIR)
    }
}

impl Installation<UreqDownloader> {
    /// Production constructor using the platform's managed data and command paths.
    pub fn from_platform_env(env: &PlatformEnv) -> Self {
        Self::new(
            paths::data_dir(env),
            paths::managed_command_path(env),
            UreqDownloader::new(),
            NoFaultInjector,
        )
    }

    /// Production constructor using the process environment snapshot.
    pub fn from_process() -> Self {
        Self::from_platform_env(&PlatformEnv::from_process())
    }
}

struct InstallLock {
    file: File,
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = fs4::FileExt::unlock(&self.file);
    }
}

#[derive(Clone, Debug)]
struct Transaction {
    id: String,
    dir: PathBuf,
    version_dir: PathBuf,
}

fn repository_url() -> Result<Url> {
    Url::parse(REPOSITORY_URL)
        .map_err(|error| InstallError::Invalid(format!("invalid release repository URL: {error}")))
}

fn current_target() -> Result<ReleaseTarget> {
    ReleaseTarget::current().ok_or_else(|| {
        InstallError::Invalid("this host has no supported signed release target".to_string())
    })
}

fn current_payload_name() -> &'static str {
    if cfg!(windows) {
        PAYLOAD_WINDOWS
    } else {
        PAYLOAD_UNIX
    }
}

fn absolute_path(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&path))
            .unwrap_or(path)
    }
}

fn lexists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn read_json<T>(path: &Path) -> Result<Option<T>>
where
    T: for<'de> Deserialize<'de>,
{
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || executable::is_reparse_point(path)? =>
        {
            Err(InstallError::Invalid(format!(
                "state path is not a regular file: {}",
                path.display()
            )))
        }
        Ok(metadata) => {
            if metadata.len() > release::MAX_METADATA_SIZE as u64 {
                return Err(InstallError::Invalid(format!(
                    "state file is too large: {}",
                    path.display()
                )));
            }
            Ok(Some(serde_json::from_slice(&fs::read(path)?)?))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn read_regular_limited(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || executable::is_reparse_point(path)?
    {
        return Err(InstallError::Invalid(format!(
            "not a regular file: {}",
            path.display()
        )));
    }
    if metadata.len() > limit as u64 {
        return Err(InstallError::Invalid(format!(
            "file is larger than its limit: {}",
            path.display()
        )));
    }
    Ok(fs::read(path)?)
}

fn verify_file_digest(
    path: &Path,
    expected_size: u64,
    expected_hash: &str,
    label: &str,
) -> Result<()> {
    executable::ensure_regular_file(path)?;
    let metadata = fs::metadata(path)?;
    if metadata.len() != expected_size {
        return Err(InstallError::Invalid(format!(
            "{label} size mismatch: expected {expected_size}, got {}",
            metadata.len()
        )));
    }
    let actual = release::sha256_file(path)?;
    if actual != expected_hash {
        return Err(InstallError::Invalid(format!(
            "{label} SHA-256 mismatch: expected {expected_hash}, got {actual}"
        )));
    }
    Ok(())
}

fn sync_regular_files(dir: &Path) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || executable::is_reparse_point(&path)? {
            return Err(InstallError::Invalid(format!(
                "staging contains a symlink: {}",
                path.display()
            )));
        }
        if metadata.is_dir() {
            sync_regular_files(&path)?;
            executable::sync_dir(&path)?;
        } else if metadata.is_file() {
            File::open(&path)?.sync_all()?;
        } else {
            return Err(InstallError::Invalid(format!(
                "staging contains a special file: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn validate_exact_version_members(dir: &Path, target: ReleaseTarget) -> Result<()> {
    let expected = [
        PAYLOAD_UNIX,
        PAYLOAD_WINDOWS,
        MANIFEST_FILE,
        SIGNATURE_FILE,
        VERSION_FILE,
    ];
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| InstallError::Invalid("version member is not UTF-8".into()))?;
        let allowed = if target.is_windows() {
            name == PAYLOAD_WINDOWS || expected[2..].contains(&name)
        } else {
            name == PAYLOAD_UNIX || expected[2..].contains(&name)
        };
        if !allowed {
            return Err(InstallError::Invalid(format!(
                "unexpected immutable version member: {name}"
            )));
        }
    }
    let required = if target.is_windows() {
        [PAYLOAD_WINDOWS, MANIFEST_FILE, SIGNATURE_FILE, VERSION_FILE]
    } else {
        [PAYLOAD_UNIX, MANIFEST_FILE, SIGNATURE_FILE, VERSION_FILE]
    };
    for name in required {
        if !lexists(&dir.join(name))? {
            return Err(InstallError::Invalid(format!(
                "missing immutable version member: {name}"
            )));
        }
    }
    Ok(())
}

fn validate_version_state(state: &VersionState) -> Result<()> {
    if state.schema_version != STATE_SCHEMA_VERSION {
        return Err(InstallError::Invalid(format!(
            "unsupported version state schema {}",
            state.schema_version
        )));
    }
    if state.version.to_string().is_empty()
        || !valid_id(&state.installation_id)
        || state.size == 0
        || !lower_sha256(&state.binary_sha256)
    {
        return Err(InstallError::Invalid("invalid version state fields".into()));
    }
    if state.target.is_windows() {
        let launcher = state.launcher.as_ref().ok_or_else(|| {
            InstallError::Invalid("Windows version state lacks launcher metadata".into())
        })?;
        validate_launcher_metadata(launcher)?;
    } else if state.launcher.is_some() {
        return Err(InstallError::Invalid(
            "Unix version state contains launcher metadata".into(),
        ));
    }
    Ok(())
}

fn validate_install_state(state: &InstallState) -> Result<()> {
    if state.schema_version != STATE_SCHEMA_VERSION || !valid_id(&state.installation_id) {
        return Err(InstallError::Invalid(
            "invalid install state schema or installation id".into(),
        ));
    }
    if state.active.is_none() && state.previous.is_some() {
        return Err(InstallError::Invalid(
            "install state has previous without active".into(),
        ));
    }
    if state.active == state.previous && state.active.is_some() {
        return Err(InstallError::Invalid(
            "install state active and previous are equal".into(),
        ));
    }
    #[cfg(windows)]
    {
        let launcher = state.launcher.as_ref().ok_or_else(|| {
            InstallError::Invalid("Windows install state lacks launcher ownership".into())
        })?;
        validate_launcher_ownership(launcher)?;
    }
    #[cfg(not(windows))]
    if state.launcher.is_some() {
        return Err(InstallError::Invalid(
            "Unix install state contains launcher ownership".into(),
        ));
    }
    Ok(())
}

fn validate_pending(pending: &PendingActivation) -> Result<()> {
    if pending.schema_version != STATE_SCHEMA_VERSION || !valid_id(&pending.transaction_id) {
        return Err(InstallError::Invalid(
            "invalid pending activation schema".into(),
        ));
    }
    if pending.from.as_ref() == Some(&pending.to) {
        return Err(InstallError::Invalid(
            "pending activation from and to are equal".into(),
        ));
    }
    Ok(())
}

fn validate_launcher_metadata(launcher: &LauncherMetadata) -> Result<()> {
    if launcher.protocol != 1
        || launcher.size == 0
        || !lower_sha256(&launcher.sha256)
        || !canonical_launcher_path(&launcher.path)
    {
        return Err(InstallError::Invalid(
            "invalid signed launcher metadata".into(),
        ));
    }
    Ok(())
}

fn validate_signed_launcher(launcher: &release::LauncherInfo) -> Result<()> {
    if launcher.protocol != 1
        || launcher.size == 0
        || !lower_sha256(&launcher.sha256)
        || !canonical_launcher_path(&launcher.path)
    {
        return Err(InstallError::Invalid(
            "invalid signed launcher metadata".into(),
        ));
    }
    Ok(())
}

fn canonical_launcher_path(path: &str) -> bool {
    let mut components = path.split('/');
    let Some(root) = components.next() else {
        return false;
    };
    let Some(name) = components.next() else {
        return false;
    };
    components.next().is_none()
        && !root.is_empty()
        && root.starts_with("hyprmux-")
        && name == LAUNCHER_WINDOWS
        && !path.contains('\\')
        && !path.contains('\0')
}

#[cfg(windows)]
fn validate_launcher_ownership(launcher: &LauncherOwnership) -> Result<()> {
    if !launcher.owned
        || launcher.protocol != 1
        || launcher.path.is_empty()
        || launcher.path.contains('\0')
        || launcher.size == 0
        || !lower_sha256(&launcher.sha256)
    {
        return Err(InstallError::Invalid(
            "invalid launcher ownership state".into(),
        ));
    }
    Ok(())
}

fn launcher_metadata(launcher: &release::LauncherInfo) -> LauncherMetadata {
    LauncherMetadata {
        path: launcher.path.clone(),
        sha256: launcher.sha256.clone(),
        size: launcher.size,
        protocol: launcher.protocol,
    }
}

fn lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn random_id(label: &str) -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| {
        InstallError::Invalid(format!(
            "OS randomness failed while creating {label}: {error}"
        ))
    })?;
    Ok(hex::encode(bytes))
}

fn parse_canonical_version(value: &str) -> Result<Version> {
    let version = Version::parse(value)
        .map_err(|error| InstallError::Invalid(format!("invalid managed version: {error}")))?;
    if version.to_string() != value {
        return Err(InstallError::Invalid(
            "managed version is not canonical".into(),
        ));
    }
    Ok(version)
}

fn deserialize_canonical_version<'de, D>(deserializer: D) -> std::result::Result<Version, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    let version = Version::parse(&raw).map_err(serde::de::Error::custom)?;
    if version.to_string() != raw {
        return Err(serde::de::Error::custom("version is not canonical"));
    }
    Ok(version)
}

fn deserialize_optional_canonical_version<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<Version>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    raw.map(|raw| {
        let version = Version::parse(&raw).map_err(serde::de::Error::custom)?;
        if version.to_string() != raw {
            return Err(serde::de::Error::custom("version is not canonical"));
        }
        Ok(version)
    })
    .transpose()
}

#[allow(dead_code)]
fn parse_pointer_version(root: &Path, pointer: &Path, payload_name: &str) -> Result<Version> {
    let versions = root.join(VERSIONS_DIR);
    let relative = pointer.strip_prefix(&versions).map_err(|_| {
        InstallError::Invalid("managed command pointer is outside versions directory".into())
    })?;
    let mut components = relative.components();
    let Some(Component::Normal(version)) = components.next() else {
        return Err(InstallError::Invalid(
            "managed command pointer has no version".into(),
        ));
    };
    let Some(Component::Normal(payload)) = components.next() else {
        return Err(InstallError::Invalid(
            "managed command pointer has no payload".into(),
        ));
    };
    if components.next().is_some() || payload != payload_name {
        return Err(InstallError::Invalid(
            "managed command pointer has a noncanonical payload".into(),
        ));
    }
    let version = version.to_str().ok_or_else(|| {
        InstallError::Invalid("managed command pointer version is not UTF-8".into())
    })?;
    parse_canonical_version(version)
}

#[allow(dead_code)]
fn same_path(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release::signature::{TrustedKey, sign_manifest_bytes};
    use crate::release::{ReleaseAsset, ReleaseManifest, Target};
    use flate2::{Compression, write::GzEncoder};
    use std::collections::{BTreeMap, HashMap};
    use std::io::Cursor;
    use std::sync::Mutex;

    #[test]
    fn launcher_protocol_validation_is_strict_and_platform_neutral() {
        let good = LauncherMetadata {
            path: "hyprmux-1.2.3-x86_64-pc-windows-msvc/hyprmux-launcher.exe".into(),
            sha256: "a".repeat(64),
            size: 1,
            protocol: 1,
        };
        assert!(validate_launcher_metadata(&good).is_ok());
        let mut bad = good.clone();
        bad.protocol = 2;
        assert!(validate_launcher_metadata(&bad).is_err());
        bad = good.clone();
        bad.sha256 = "A".repeat(64);
        assert!(validate_launcher_metadata(&bad).is_err());
    }

    #[test]
    fn pointer_parser_requires_an_absolute_canonical_payload_shape() {
        let root = PathBuf::from("/tmp/hyprmux-managed");
        let pointer = root.join("versions/1.2.3/hyprmux");
        assert_eq!(
            parse_pointer_version(&root, &pointer, PAYLOAD_UNIX).unwrap(),
            Version::parse("1.2.3").unwrap()
        );
        assert!(
            parse_pointer_version(&root, &root.join("other/1.2.3/hyprmux"), PAYLOAD_UNIX).is_err()
        );
        assert!(
            parse_pointer_version(
                &root,
                &root.join("versions/1.2.3/hyprmux.exe"),
                PAYLOAD_UNIX
            )
            .is_err()
        );
    }

    // Keep the fixture helpers local to this file: the production path always uses compiled keys,
    // while these tests inject a deterministic key set through the explicit verifier seam.
    #[allow(dead_code)]
    fn signed_fixture(version: &Version) -> (ReleaseMetadata, TrustedKey, Vec<u8>) {
        let target = Target::current().unwrap();
        let payload = b"fixture payload";
        let mut compressed = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut builder = tar::Builder::new(&mut compressed);
            let root = target.root_name(version);
            let mut root_header = tar::Header::new_gnu();
            root_header.set_entry_type(tar::EntryType::Directory);
            root_header.set_size(0);
            root_header.set_mode(0o755);
            root_header.set_cksum();
            builder
                .append_data(
                    &mut root_header,
                    format!("{root}/"),
                    Cursor::new(Vec::<u8>::new()),
                )
                .unwrap();
            let mut header = tar::Header::new_gnu();
            header.set_size(payload.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    target.payload_path(version),
                    Cursor::new(payload.as_slice()),
                )
                .unwrap();
            builder.finish().unwrap();
        }
        let archive = compressed.finish().unwrap();
        let asset = ReleaseAsset::new(
            version,
            target,
            archive.len() as u64,
            release::sha256_bytes(&archive),
            payload.len() as u64,
            release::sha256_bytes(payload),
        );
        let manifest = ReleaseManifest::new(
            version.clone(),
            "2026-08-02T12:00:00Z",
            BTreeMap::from([(target, asset.clone())]),
        )
        .unwrap();
        let manifest_bytes = manifest.to_bytes().unwrap();
        let signing = ed25519_dalek::SigningKey::from_bytes(&[19; 32]);
        let trusted = TrustedKey::ed25519("test", signing.verifying_key().to_bytes());
        let signature_bytes = sign_manifest_bytes(&manifest_bytes, "test", &signing).unwrap();
        let signature = release::SignatureEnvelope::from_bytes(&signature_bytes).unwrap();
        let verified_signature = release::verify_manifest_with_keys(
            &manifest_bytes,
            &signature_bytes,
            std::slice::from_ref(&trusted),
        )
        .unwrap();
        let repository = Url::parse(&format!(
            "https://github.com/Razuer/hyprmux/releases/download/v{version}/"
        ))
        .unwrap();
        (
            ReleaseMetadata {
                version: version.clone(),
                manifest_bytes,
                manifest,
                signature_bytes,
                signature,
                verified_signature,
                release_base: repository,
            },
            trusted,
            archive,
        )
    }

    #[derive(Default)]
    struct FixtureDownloader {
        responses: Mutex<HashMap<String, release::DownloadResponse>>,
    }

    impl Downloader for FixtureDownloader {
        fn fetch(
            &self,
            url: &Url,
            _max_bytes: usize,
        ) -> release::Result<release::DownloadResponse> {
            self.responses
                .lock()
                .unwrap()
                .get(url.as_str())
                .cloned()
                .ok_or_else(|| release::ReleaseError::Download(format!("missing fixture {url}")))
        }
    }

    #[cfg(unix)]
    struct FailOnce {
        point: FaultPoint,
        fired: Mutex<bool>,
    }

    #[cfg(unix)]
    impl FaultInjector for FailOnce {
        fn after(&self, point: FaultPoint) -> io::Result<()> {
            let mut fired = self.fired.lock().unwrap();
            if point == self.point && !*fired {
                *fired = true;
                Err(io::Error::other("injected activation failure"))
            } else {
                Ok(())
            }
        }
    }

    #[cfg(unix)]
    fn fixture_manager(
        version: &Version,
        root: &Path,
        fault: impl FaultInjector + 'static,
    ) -> Installation<FixtureDownloader> {
        fixture_manager_with_command(version, root, root.join("command-dir/hyprmux"), fault)
    }

    #[cfg(unix)]
    fn fixture_manager_with_command(
        version: &Version,
        root: &Path,
        command: PathBuf,
        fault: impl FaultInjector + 'static,
    ) -> Installation<FixtureDownloader> {
        let (metadata, trusted, archive) = signed_fixture(version);
        let exact =
            release::download::exact_metadata_url(&Url::parse(REPOSITORY_URL).unwrap(), version)
                .unwrap();
        let signature = exact.join(release::download::SIGNATURE_FILENAME).unwrap();
        let target = Target::current().unwrap();
        let archive_url = metadata
            .release_base
            .join(&metadata.manifest.asset_for(target).unwrap().archive)
            .unwrap();
        let downloader = FixtureDownloader::default();
        downloader.responses.lock().unwrap().extend([
            (
                exact.to_string(),
                release::DownloadResponse::new(
                    exact.clone(),
                    exact.clone(),
                    vec![exact],
                    metadata.manifest_bytes.clone(),
                ),
            ),
            (
                signature.to_string(),
                release::DownloadResponse::new(
                    signature.clone(),
                    signature.clone(),
                    vec![signature],
                    metadata.signature_bytes.clone(),
                ),
            ),
            (
                archive_url.to_string(),
                release::DownloadResponse::new(
                    archive_url.clone(),
                    archive_url,
                    Vec::new(),
                    archive,
                ),
            ),
        ]);
        Installation::new(root, command, downloader, fault).with_trusted_keys(vec![trusted])
    }

    #[test]
    fn state_models_reject_unknown_fields() {
        let result = serde_json::from_slice::<VersionState>(
            br#"{"schema_version":1,"version":"1.2.3","target":"x86_64-unknown-linux-gnu","binary_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size":1,"installation_id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","launcher":null,"extra":true}"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn signed_install_creates_the_pointer_and_immutable_state() {
        let version = Version::parse("1.2.3").unwrap();
        let (metadata, trusted, archive) = signed_fixture(&version);
        let exact =
            release::download::exact_metadata_url(&Url::parse(REPOSITORY_URL).unwrap(), &version)
                .unwrap();
        let signature = exact.join(release::download::SIGNATURE_FILENAME).unwrap();
        let archive_url = metadata
            .release_base
            .join(
                &metadata
                    .manifest
                    .asset_for(Target::current().unwrap())
                    .unwrap()
                    .archive,
            )
            .unwrap();
        let downloader = FixtureDownloader::default();
        downloader.responses.lock().unwrap().extend([
            (
                exact.to_string(),
                release::DownloadResponse::new(
                    exact.clone(),
                    exact.clone(),
                    vec![exact.clone()],
                    metadata.manifest_bytes.clone(),
                ),
            ),
            (
                signature.to_string(),
                release::DownloadResponse::new(
                    signature.clone(),
                    signature.clone(),
                    vec![signature.clone()],
                    metadata.signature_bytes.clone(),
                ),
            ),
            (
                archive_url.to_string(),
                release::DownloadResponse::new(
                    archive_url.clone(),
                    archive_url.clone(),
                    vec![archive_url],
                    archive,
                ),
            ),
        ]);
        let root = std::env::temp_dir().join(format!(
            "hyprmux-install-test-{}-{}",
            std::process::id(),
            version
        ));
        let _ = fs::remove_dir_all(&root);
        let command = root.join("command-dir/hyprmux");
        let manager = Installation::new(&root, &command, downloader, NoFaultInjector)
            .with_trusted_keys(vec![trusted]);
        let result = manager.install_version(version.clone()).unwrap();
        assert_eq!(result.version, version);
        assert!(result.changed);
        assert_eq!(
            manager.read_pointer_unlocked().unwrap(),
            Some(version.clone())
        );
        assert!(manager.install_state_path().is_file());
        assert!(!manager.pending_path().exists());
        assert!(manager.version_dir(&version).join(MANIFEST_FILE).is_file());
        assert!(manager.version_dir(&version).join(VERSION_FILE).is_file());
        let target = fs::read_link(&command).unwrap();
        assert!(target.is_absolute());
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_version_is_reused_only_while_fully_verified() {
        let version = Version::parse("1.2.3").unwrap();
        let root = std::env::temp_dir().join(format!("hyprmux-reuse-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let manager = fixture_manager(&version, &root, NoFaultInjector);
        assert!(manager.install_version(version.clone()).unwrap().changed);
        assert!(!manager.install_version(version.clone()).unwrap().changed);
        fs::write(manager.payload_path(&version), b"corrupt").unwrap();
        assert!(manager.install_version(version).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn every_activation_boundary_recovers_to_a_valid_install() {
        let version = Version::parse("1.2.3").unwrap();
        let points = [
            FaultPoint::LockAcquired,
            FaultPoint::StagingCreated,
            FaultPoint::PayloadWritten,
            FaultPoint::Verified,
            FaultPoint::StagingSynced,
            FaultPoint::VersionRenamed,
            FaultPoint::PendingWritten,
            FaultPoint::PointerSwitched,
            FaultPoint::InstallWritten,
            FaultPoint::PendingRemoved,
            FaultPoint::ParentsSynced,
        ];
        for point in points {
            let root = std::env::temp_dir().join(format!(
                "hyprmux-fault-test-{}-{point:?}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            let manager = fixture_manager(
                &version,
                &root,
                FailOnce {
                    point,
                    fired: Mutex::new(false),
                },
            );
            assert!(
                manager.install_version(version.clone()).is_err(),
                "{point:?}"
            );
            manager.recover_if_managed().unwrap();
            if manager.read_pointer_unlocked().unwrap().is_none() {
                manager.install_version(version.clone()).unwrap();
            }
            assert_eq!(
                manager.read_pointer_unlocked().unwrap(),
                Some(version.clone())
            );
            assert!(manager.read_install_state().unwrap().is_some());
            assert!(!manager.pending_path().exists());
            let _ = fs::remove_dir_all(root);
        }
    }

    #[cfg(unix)]
    #[test]
    fn valid_pointer_repairs_disagreeing_descriptive_metadata_without_switching_it() {
        let version = Version::parse("1.2.3").unwrap();
        let root = std::env::temp_dir().join(format!("hyprmux-repair-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let manager = fixture_manager(&version, &root, NoFaultInjector);
        manager.install_version(version.clone()).unwrap();
        let pointer_before = fs::read_link(&manager.command_path).unwrap();
        let mut state = manager.read_install_state().unwrap().unwrap();
        state.active = None;
        fs::write(
            manager.install_state_path(),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        assert!(manager.recover_if_managed().unwrap());
        assert_eq!(
            fs::read_link(&manager.command_path).unwrap(),
            pointer_before
        );
        assert_eq!(
            manager.read_install_state().unwrap().unwrap().active,
            Some(version)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn initial_install_refuses_an_existing_unmanaged_command_without_creating_root_state() {
        let version = Version::parse("1.2.3").unwrap();
        let root =
            std::env::temp_dir().join(format!("hyprmux-ownership-root-{}", std::process::id()));
        let command =
            std::env::temp_dir().join(format!("hyprmux-ownership-command-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(&command);
        fs::write(&command, b"user executable").unwrap();
        let manager =
            fixture_manager_with_command(&version, &root, command.clone(), NoFaultInjector);
        assert!(manager.install_version(version).is_err());
        assert!(!root.exists());
        assert_eq!(fs::read(&command).unwrap(), b"user executable");
        let _ = fs::remove_file(command);
    }

    #[cfg(unix)]
    #[test]
    fn rollback_revalidates_the_retained_target_before_switching_pointer() {
        let first = Version::parse("1.2.3").unwrap();
        let second = Version::parse("1.3.0").unwrap();
        let root =
            std::env::temp_dir().join(format!("hyprmux-rollback-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let first_manager = fixture_manager(&first, &root, NoFaultInjector);
        first_manager.install_version(first.clone()).unwrap();
        let second_manager = fixture_manager(&second, &root, NoFaultInjector);
        second_manager.install_version(second.clone()).unwrap();
        let first_payload = second_manager.payload_path(&first);
        fs::write(&first_payload, b"tampered retained payload").unwrap();
        assert!(second_manager.rollback().is_err());
        assert_eq!(
            second_manager.read_pointer_unlocked().unwrap(),
            Some(second)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fault_points_are_ordered_and_complete() {
        let points = [
            FaultPoint::LockAcquired,
            FaultPoint::StagingCreated,
            FaultPoint::PayloadWritten,
            FaultPoint::Verified,
            FaultPoint::StagingSynced,
            FaultPoint::VersionRenamed,
            FaultPoint::PendingWritten,
            FaultPoint::PointerSwitched,
            FaultPoint::InstallWritten,
            FaultPoint::PendingRemoved,
            FaultPoint::ParentsSynced,
        ];
        assert_eq!(points.len(), 11);
        assert_ne!(points[0], points[10]);
    }
}
