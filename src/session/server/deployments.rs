//! Extension deployments owned by a session server.
//!
//! A deployment is a verified, content-addressed copy of an extension's files kept beside the
//! session that will run them. It outlives the client that uploaded it: a detached or resurrected
//! session still knows which extension, digest, and generation it was asked to run. Only the
//! session server writes here, and an explicit session deletion removes everything it owns.
//!
//! The store is deliberately separate from the client runtime's bundle cache. That cache belongs
//! to whichever client is attached and changes with it; this one must not, or a restart would
//! forget what the session was told to run. The bytes are staged through the same verified,
//! read-only representation, so a bundle that changed on disk is refused before it is used.

use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::control::{
    ExtensionDeploymentInfo, ExtensionDeploymentStatus, ExtensionDeploymentUpload,
};
use crate::extension_runtime::bundle::{self, Bundle, MAX_BUNDLE_BYTES};
use crate::extension_runtime::store::{BundleStore, StoreError};
use crate::platform::fs_security;

/// How much of a bundle one control request carries. Base64 makes this about 683 KiB on the wire,
/// under the control line limit and well inside a session frame.
pub const DEPLOY_CHUNK_BYTES: usize = 512 * 1024;

const METADATA_FILE: &str = "deployments.json";
const METADATA_VERSION: u32 = 1;
const UPLOADS: &str = "uploads";
const BUNDLES: &str = "bundles";

#[derive(Debug)]
pub enum DeployError {
    /// The request names something that is not a valid extension, digest, size, or offset.
    Invalid(String),
    /// The session has no state directory, or the store cannot be written.
    Unavailable(String),
    /// The bundle does not match the digest it was sent under, or its manifest disagrees.
    Mismatch(String),
}

impl DeployError {
    pub fn message(&self) -> &str {
        match self {
            Self::Invalid(message) | Self::Unavailable(message) | Self::Mismatch(message) => {
                message
            }
        }
    }
}

impl std::fmt::Display for DeployError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for DeployError {}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Metadata {
    version: u32,
    deployments: BTreeMap<String, DeploymentRecord>,
    /// The highest generation this session has ever issued. It has to be remembered rather than
    /// derived from the current records: undeploying the newest extension would otherwise let the
    /// next deployment reuse its generation, and a client could mistake new files for old ones.
    #[serde(default)]
    last_generation: u64,
}

impl Default for Metadata {
    fn default() -> Self {
        Self {
            version: METADATA_VERSION,
            deployments: BTreeMap::new(),
            last_generation: 0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DeploymentRecord {
    pub digest: String,
    pub generation: u64,
    pub deployed_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub files: usize,
    pub size: u64,
}

/// A session's deployed extensions, stored under its own directory in state.
#[derive(Clone)]
pub struct SessionDeployments {
    root: Option<PathBuf>,
}

/// A verified generation prepared off the session loop for activation by its server.
pub(super) struct PreparedDeployment {
    pub id: String,
    pub generation: u64,
    pub directory: PathBuf,
    pub platforms: Vec<String>,
    /// The settings its services start with, or why the host's config could not supply them.
    pub settings: Result<String, String>,
    pub services: Vec<crate::config::PlacedService>,
}

impl SessionDeployments {
    /// Open the store for `session`. `root` is the deployment root itself, not a parent, and is
    /// only named by tests; production resolves the session's directory in state.
    pub fn new(session: impl Into<String>, root: Option<PathBuf>) -> Self {
        let session = session.into();
        let root = root.or_else(|| {
            crate::platform::paths::state_dir_if_available(
                &crate::platform::paths::PlatformEnv::from_process(),
            )
            .map(|dir| default_root(&dir, &session))
        });
        Self { root }
    }

    /// Fence every previous server's processes before starting the restored deployments.
    pub(super) fn restart_generations(&self) -> Result<(), DeployError> {
        let Some(root) = self.existing_root()? else {
            return Ok(());
        };
        // An upload does not outlive the server it was sent to: its sender has to start again.
        let _ = std::fs::remove_dir_all(root.join(UPLOADS));
        let mut metadata = self.metadata(&root)?;
        self.sweep(&root, &metadata);
        if metadata.deployments.is_empty() {
            return Ok(());
        }
        for record in metadata.deployments.values_mut() {
            metadata.last_generation =
                metadata.last_generation.checked_add(1).ok_or_else(|| {
                    DeployError::Unavailable("deployment generation exhausted".into())
                })?;
            record.generation = metadata.last_generation;
        }
        self.write_metadata(&root, &metadata)
    }

    /// Read definitions only from verified bundles. Corrupt records remain listed as unavailable.
    pub(super) fn prepare(&self) -> Result<Vec<PreparedDeployment>, DeployError> {
        let Some(root) = self.existing_root()? else {
            return Ok(Vec::new());
        };
        let metadata = self.metadata(&root)?;
        if metadata.deployments.is_empty() {
            return Ok(Vec::new());
        }
        let bundles = BundleStore::open(root.join(BUNDLES))
            .map_err(|error| DeployError::Unavailable(error.to_string()))?;
        // A config that cannot be read must not fail the whole pass: this result is also what stops
        // the services of an extension that was just undeployed.
        let user = crate::config::read_user_extension_config();
        let mut prepared = Vec::new();
        for (id, record) in metadata.deployments {
            let Ok(directory) = bundles.verified(&record.digest) else {
                continue;
            };
            let checked = crate::config::check_extension(&directory);
            if !checked.info.errors.is_empty() {
                continue;
            }
            let settings = match &user {
                Ok(user) => {
                    let mut warnings = Vec::new();
                    let settings = crate::config::merge_extension_settings(
                        &checked.info.setting_specs,
                        &id,
                        user.settings.get(&id),
                        &mut warnings,
                    );
                    for warning in warnings {
                        eprintln!("rozi: {warning}");
                    }
                    serde_json::to_string(&settings).map_err(|error| error.to_string())
                }
                Err(error) => Err(error.clone()),
            };
            prepared.push(PreparedDeployment {
                services: checked.session_services(),
                id,
                generation: record.generation,
                directory,
                platforms: checked.info.platforms,
                settings,
            });
        }
        Ok(prepared)
    }

    /// Move the session-owned store with a session rename. The worker must be stopped first.
    pub fn rename(&mut self, name: &str) -> std::io::Result<()> {
        let Some(root) = self.root.as_ref() else {
            return Ok(());
        };
        let destination = root.with_file_name(name);
        if destination.exists() {
            // Only a record of something deployed is worth refusing over. An earlier session of
            // that name may have left a store with nothing in it.
            let stale = Self {
                root: Some(destination.clone()),
            };
            match stale.metadata(&destination) {
                Ok(metadata) if metadata.deployments.is_empty() => stale.remove_all()?,
                _ => {
                    return Err(std::io::Error::other(
                        "the new name already has deployment state",
                    ));
                }
            }
        }
        if root.exists() {
            std::fs::rename(root, &destination)?;
        }
        self.root = Some(destination);
        Ok(())
    }

    /// Remove every deployed bundle and the metadata that names them. Used when the session is
    /// explicitly deleted, never on an ordinary restart.
    pub fn remove_all(&self) -> std::io::Result<()> {
        let Some(root) = &self.root else {
            return Ok(());
        };
        let bundles = root.join(BUNDLES);
        if bundles.is_dir()
            && let Ok(store) = BundleStore::open(bundles)
        {
            store.remove_all();
        }
        match std::fs::remove_dir_all(root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }

    /// Append one chunk of a bundle. Returns how much of the announced size has arrived and
    /// whether this call completed the upload.
    pub fn append(
        &self,
        extension: &str,
        digest: &str,
        size: u64,
        offset: u64,
        data: &[u8],
        last: bool,
    ) -> Result<ExtensionDeploymentUpload, DeployError> {
        validate_request(extension, digest, size)?;
        let root = self.root()?;
        let uploads = root.join(UPLOADS);
        fs_security::ensure_private_dir(&uploads)
            .map_err(|error| DeployError::Unavailable(format!("cannot create uploads: {error}")))?;
        let part = uploads.join(format!("{extension}-{digest}.part"));
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&part)
            .map_err(|error| DeployError::Unavailable(format!("cannot open upload: {error}")))?;
        let current = file
            .metadata()
            .map_err(|error| DeployError::Unavailable(format!("cannot inspect upload: {error}")))?
            .len();
        if offset == 0 && current != 0 {
            file.set_len(0).map_err(|error| {
                DeployError::Unavailable(format!("cannot restart upload: {error}"))
            })?;
        }
        let current = file
            .metadata()
            .map_err(|error| DeployError::Unavailable(format!("cannot inspect upload: {error}")))?
            .len();
        if offset != current {
            return Err(DeployError::Invalid(format!(
                "upload is at {current} bytes, not {offset}; start again from the first chunk"
            )));
        }
        let received = current
            .checked_add(data.len() as u64)
            .ok_or_else(|| DeployError::Invalid("upload size overflowed".to_string()))?;
        if received > size {
            return Err(DeployError::Invalid(format!(
                "chunk would bring the upload to {received} bytes, past its announced {size}"
            )));
        }
        file.seek(SeekFrom::Start(offset))
            .and_then(|_| file.write_all(data))
            .and_then(|()| file.sync_data())
            .map_err(|error| DeployError::Unavailable(format!("cannot write upload: {error}")))?;
        if last && received != size {
            return Err(DeployError::Invalid(
                "last chunk does not complete the announced size".into(),
            ));
        }
        if received != size {
            return Ok(ExtensionDeploymentUpload {
                extension: extension.to_string(),
                digest: digest.to_string(),
                received,
                size,
                complete: false,
                generation: None,
            });
        }
        if !last {
            return Err(DeployError::Invalid(
                "the final byte arrived without the request saying it was the last chunk"
                    .to_string(),
            ));
        }
        let finished = self.finish_upload(&mut file, extension, digest, size);
        drop(file);
        // Finished either way: a refused upload starts again from its first chunk, so nothing it
        // or an abandoned upload of this extension wrote is worth keeping.
        clear_uploads(&root, extension);
        let record = finished?;
        Ok(ExtensionDeploymentUpload {
            extension: extension.to_string(),
            digest: digest.to_string(),
            received,
            size,
            complete: true,
            generation: Some(record.generation),
        })
    }

    /// Check a completed upload and commit it.
    fn finish_upload(
        &self,
        file: &mut std::fs::File,
        extension: &str,
        digest: &str,
        size: u64,
    ) -> Result<DeploymentRecord, DeployError> {
        file.seek(SeekFrom::Start(0))
            .map_err(|error| DeployError::Unavailable(format!("cannot read upload: {error}")))?;
        let mut archive = Vec::with_capacity(size as usize);
        file.read_to_end(&mut archive)
            .map_err(|error| DeployError::Unavailable(format!("cannot read upload: {error}")))?;
        let bundle = Bundle::from_archive(&archive, digest)
            .map_err(|error| DeployError::Mismatch(error.to_string()))?;
        let identity = manifest_identity(&bundle)?;
        if identity.id.as_deref() != Some(extension) {
            return Err(DeployError::Mismatch(format!(
                "the bundle declares extension `{}`, not `{extension}`",
                identity.id.as_deref().unwrap_or("")
            )));
        }
        self.commit(extension, &bundle, identity.title, identity.version)
    }

    /// Remove one deployment. `Ok(false)` means there was nothing by that name.
    pub fn undeploy(&self, extension: &str) -> Result<bool, DeployError> {
        if !crate::platform::paths::valid_extension_id(extension) {
            return Err(DeployError::Invalid(format!(
                "`{extension}` is not an extension id"
            )));
        }
        let Some(root) = self.existing_root()? else {
            return Ok(false);
        };
        let mut metadata = self.metadata(&root)?;
        if metadata.deployments.remove(extension).is_none() {
            return Ok(false);
        }
        self.write_metadata(&root, &metadata)?;
        clear_uploads(&root, extension);
        self.sweep(&root, &metadata);
        Ok(true)
    }

    /// Every deployment, verified now. A record whose files changed or disappeared is reported
    /// unavailable rather than deleted, so the reason survives to the next `status`.
    pub fn status(&self) -> Result<Vec<ExtensionDeploymentInfo>, DeployError> {
        if self.root.is_none() {
            self.root()?;
        }
        let Some(root) = self.existing_root()? else {
            return Ok(Vec::new());
        };
        let metadata = self.metadata(&root)?;
        let bundles = BundleStore::open(root.join(BUNDLES)).map_err(|error| {
            DeployError::Unavailable(format!("cannot open bundle store: {error}"))
        })?;
        let mut report = Vec::with_capacity(metadata.deployments.len());
        for (extension, record) in metadata.deployments {
            let (status, reason) = match bundles.verified(&record.digest) {
                Ok(directory) => {
                    // What `prepare` would refuse to run must not read as ready to run. A newer
                    // `rozi` on this host can reject a manifest an older one accepted.
                    let checked = crate::config::check_extension(&directory);
                    if checked.info.errors.is_empty() {
                        (ExtensionDeploymentStatus::Deployed, None)
                    } else {
                        (
                            ExtensionDeploymentStatus::Unavailable,
                            Some(format!(
                                "its manifest is not valid on this host: {}",
                                checked.info.errors.join("; ")
                            )),
                        )
                    }
                }
                Err(StoreError::Missing) => (
                    ExtensionDeploymentStatus::Unavailable,
                    Some("its files are no longer stored on this host".to_string()),
                ),
                Err(StoreError::Corrupt(reason)) => (
                    ExtensionDeploymentStatus::Unavailable,
                    Some(format!("its files were modified and discarded: {reason}")),
                ),
                Err(StoreError::Io(reason)) => (
                    ExtensionDeploymentStatus::Unavailable,
                    Some(format!("its files cannot be read: {reason}")),
                ),
            };
            report.push(ExtensionDeploymentInfo {
                extension,
                digest: record.digest,
                generation: record.generation,
                deployed_at_unix_ms: record.deployed_at_unix_ms,
                title: record.title,
                version: record.version,
                files: record.files,
                size: record.size,
                status,
                local_copy: None,
                services: Vec::new(),
                reason,
            });
        }
        Ok(report)
    }

    fn commit(
        &self,
        extension: &str,
        bundle: &Bundle,
        title: Option<String>,
        version: Option<String>,
    ) -> Result<DeploymentRecord, DeployError> {
        let root = self.root()?;
        let bundles = BundleStore::open(root.join(BUNDLES)).map_err(|error| {
            DeployError::Unavailable(format!("cannot open bundle store: {error}"))
        })?;
        let directory = bundles.stage(bundle).map_err(|error| {
            DeployError::Unavailable(format!("cannot store the bundle: {error}"))
        })?;
        let checked = crate::config::check_extension(&directory);
        if !checked.info.errors.is_empty() {
            if let Ok(metadata) = self.metadata(&root) {
                self.sweep(&root, &metadata);
            }
            return Err(DeployError::Mismatch(format!(
                "invalid deployed manifest: {}",
                checked.info.errors.join("; ")
            )));
        }
        let mut metadata = self.metadata(&root)?;
        let generation = metadata.last_generation.checked_add(1).ok_or_else(|| {
            DeployError::Unavailable("deployment generation exhausted".to_string())
        })?;
        let record = DeploymentRecord {
            digest: bundle.digest().to_string(),
            generation,
            deployed_at_unix_ms: crate::runtime_metrics::unix_time_millis(),
            title,
            version,
            files: bundle.files().len(),
            size: bundle.size(),
        };
        metadata
            .deployments
            .insert(extension.to_string(), record.clone());
        metadata.last_generation = generation;
        self.write_metadata(&root, &metadata)?;
        // The generation this one replaced may have been the last to name its files.
        self.sweep(&root, &metadata);
        Ok(record)
    }

    /// Delete every stored bundle no record names. This store has one writer, the session's worker,
    /// so nothing unnamed here is in use by another deployment. A file a running process still holds
    /// may refuse on Windows, and the next sweep gets it.
    fn sweep(&self, root: &Path, metadata: &Metadata) {
        let Ok(bundles) = BundleStore::open(root.join(BUNDLES)) else {
            return;
        };
        let keep: HashSet<&str> = metadata
            .deployments
            .values()
            .map(|record| record.digest.as_str())
            .collect();
        bundles.retain_only(&keep);
    }

    /// The store's directory if this session has ever written one. Reading must not create it, or
    /// every named session would leave a store behind whether or not anything was deployed.
    fn existing_root(&self) -> Result<Option<PathBuf>, DeployError> {
        match &self.root {
            Some(root) if root.is_dir() => self.root().map(Some),
            _ => Ok(None),
        }
    }

    fn root(&self) -> Result<PathBuf, DeployError> {
        let root = self.root.as_ref().ok_or_else(|| {
            DeployError::Unavailable(
                "the state directory is unavailable, so this session cannot hold deployments"
                    .to_string(),
            )
        })?;
        fs_security::ensure_private_dir(root)
            .map_err(|error| DeployError::Unavailable(format!("cannot create store: {error}")))?;
        Ok(root.clone())
    }

    fn metadata(&self, root: &Path) -> Result<Metadata, DeployError> {
        let path = root.join(METADATA_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Metadata::default());
            }
            Err(error) => {
                return Err(DeployError::Unavailable(format!(
                    "cannot read deployment metadata: {error}"
                )));
            }
        };
        let metadata: Metadata = serde_json::from_str(&text).map_err(|error| {
            DeployError::Unavailable(format!("deployment metadata is invalid: {error}"))
        })?;
        if metadata.version != METADATA_VERSION {
            return Err(DeployError::Unavailable(format!(
                "deployment metadata is version {}, not {METADATA_VERSION}",
                metadata.version
            )));
        }
        // Metadata written before the counter existed carries records but no `last_generation`;
        // trust the highest record so the next deployment still moves forward.
        let mut metadata = metadata;
        metadata.last_generation = metadata.last_generation.max(
            metadata
                .deployments
                .values()
                .map(|record| record.generation)
                .max()
                .unwrap_or(0),
        );
        Ok(metadata)
    }

    /// Write the metadata atomically: a reader sees the old complete file or the new one, never a
    /// half-written list that would forget deployments after a crash.
    fn write_metadata(&self, root: &Path, metadata: &Metadata) -> Result<(), DeployError> {
        let text = serde_json::to_vec_pretty(metadata).map_err(|error| {
            DeployError::Unavailable(format!("cannot encode metadata: {error}"))
        })?;
        crate::platform::persist::replace_file(&root.join(METADATA_FILE), text)
            .map_err(|error| DeployError::Unavailable(format!("cannot write metadata: {error}")))
    }
}

/// Forget every partial upload of `extension`. Another extension's are named differently even when
/// one ID is a prefix of the other, because the whole remainder has to be a digest.
fn clear_uploads(root: &Path, extension: &str) {
    let Ok(entries) = std::fs::read_dir(root.join(UPLOADS)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let is_upload = name
            .to_str()
            .and_then(|name| name.strip_prefix(extension))
            .and_then(|name| name.strip_prefix('-'))
            .and_then(|name| name.strip_suffix(".part"))
            .is_some_and(bundle::is_digest);
        if is_upload {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn default_root(state: &Path, session: &str) -> PathBuf {
    state.join("session-deployments").join(session)
}

fn validate_request(extension: &str, digest: &str, size: u64) -> Result<(), DeployError> {
    if !crate::platform::paths::valid_extension_id(extension) {
        return Err(DeployError::Invalid(format!(
            "`{extension}` is not an extension id"
        )));
    }
    if !bundle::is_digest(digest) {
        return Err(DeployError::Invalid(format!(
            "`{digest}` is not a bundle digest"
        )));
    }
    if size == 0 {
        return Err(DeployError::Invalid(
            "an empty bundle cannot be deployed".to_string(),
        ));
    }
    if size > MAX_BUNDLE_BYTES {
        return Err(DeployError::Invalid(format!(
            "bundle is {size} bytes; the limit is {MAX_BUNDLE_BYTES}"
        )));
    }
    Ok(())
}

/// `[extension]` identity from the bundle's own manifest. The name a deployment is filed under has
/// to be the extension inside it, or `extensions deploy a` could install the files of `b`.
struct ManifestIdentity {
    id: Option<String>,
    title: Option<String>,
    version: Option<String>,
}

fn manifest_identity(bundle: &Bundle) -> Result<ManifestIdentity, DeployError> {
    let manifest = bundle
        .files()
        .iter()
        .find(|file| file.path == "extension.toml")
        .ok_or_else(|| DeployError::Mismatch("the bundle has no extension.toml".to_string()))?;
    let text = std::str::from_utf8(&manifest.contents)
        .map_err(|_| DeployError::Mismatch("extension.toml is not UTF-8".to_string()))?;
    let value: toml::Value = toml::from_str(text)
        .map_err(|error| DeployError::Mismatch(format!("extension.toml is invalid: {error}")))?;
    let extension = value
        .get("extension")
        .and_then(|table| table.as_table())
        .ok_or_else(|| {
            DeployError::Mismatch("the manifest has no [extension] table".to_string())
        })?;
    let string = |field: &str| {
        extension
            .get(field)
            .and_then(toml::Value::as_str)
            .map(str::to_string)
    };
    Ok(ManifestIdentity {
        id: string("id"),
        title: string("title"),
        version: string("version"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension_runtime::bundle::BundleFile;

    fn bundle(id: &str, body: &str) -> Bundle {
        Bundle::from_files(vec![
            BundleFile {
                path: "extension.toml".to_string(),
                executable: false,
                contents: format!(
                    "[extension]\nid = \"{id}\"\ntitle = \"{id}\"\nversion = \"0.1.0\"\napi = 1\n"
                )
                .into_bytes(),
            },
            BundleFile {
                path: "bin/run".to_string(),
                executable: true,
                contents: body.as_bytes().to_vec(),
            },
        ])
        .expect("valid bundle")
    }

    /// A scratch store. The store root is created here rather than handed the tempdir itself:
    /// `tempfile` makes that directory group-readable, and the private-directory rule refuses one.
    fn store(temp: &tempfile::TempDir) -> SessionDeployments {
        SessionDeployments::new("dev", Some(temp.path().join("store")))
    }

    fn bundle_dir(temp: &tempfile::TempDir) -> PathBuf {
        temp.path().join("store").join(BUNDLES)
    }

    fn send(
        store: &SessionDeployments,
        bundle: &Bundle,
        chunk: usize,
    ) -> ExtensionDeploymentUpload {
        let archive = bundle.archive();
        let mut offset = 0;
        let mut last = None;
        while offset < archive.len() {
            let end = (offset + chunk).min(archive.len());
            last = Some(
                store
                    .append(
                        "demo",
                        bundle.digest(),
                        archive.len() as u64,
                        offset as u64,
                        &archive[offset..end],
                        end == archive.len(),
                    )
                    .expect("chunk accepted"),
            );
            offset = end;
        }
        last.expect("at least one chunk")
    }

    #[test]
    fn a_bundle_sent_in_chunks_is_stored_verified_and_listed() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let bundle = bundle("demo", "echo hello\n");
        let done = send(&store, &bundle, 7);
        assert!(done.complete);
        let record = done.generation.expect("generation");

        let status = store.status().unwrap();
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].extension, "demo");
        assert_eq!(status[0].generation, record);
        assert_eq!(status[0].digest, bundle.digest());
        assert_eq!(status[0].status, ExtensionDeploymentStatus::Deployed);
        assert!(bundle_dir(&temp).join(bundle.digest()).is_dir());
    }

    #[test]
    fn chunks_must_arrive_in_order_and_stop_at_the_announced_size() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let bundle = bundle("demo", "hello");
        let archive = bundle.archive();
        let error = store
            .append(
                "demo",
                bundle.digest(),
                archive.len() as u64,
                4,
                &archive[..4],
                false,
            )
            .unwrap_err();
        assert!(matches!(error, DeployError::Invalid(_)), "{error}");
        let error = store
            .append("demo", bundle.digest(), 1, 0, &archive[..4], false)
            .unwrap_err();
        assert!(matches!(error, DeployError::Invalid(_)), "{error}");
    }

    #[test]
    fn content_that_does_not_match_its_digest_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let original = bundle("demo", "hello");
        let incoming = bundle("demo", "goodbye");
        let error = store
            .append(
                "demo",
                original.digest(),
                incoming.archive().len() as u64,
                0,
                &incoming.archive(),
                true,
            )
            .unwrap_err();
        assert!(matches!(error, DeployError::Mismatch(_)), "{error}");
        assert!(store.status().unwrap().is_empty());
    }

    #[test]
    fn a_bundle_saying_it_is_another_extension_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let bundle = bundle("other", "hello");
        let error = store
            .append(
                "demo",
                bundle.digest(),
                bundle.archive().len() as u64,
                0,
                &bundle.archive(),
                true,
            )
            .unwrap_err();
        assert!(matches!(error, DeployError::Mismatch(_)), "{error}");
    }

    #[test]
    fn undeploy_forgets_the_record_and_removes_an_unshared_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let bundle = bundle("demo", "hello");
        send(&store, &bundle, 1024);
        assert!(store.undeploy("demo").unwrap());
        assert!(store.status().unwrap().is_empty());
        assert!(!temp.path().join(BUNDLES).join(bundle.digest()).exists());
        assert!(!store.undeploy("demo").unwrap());
    }

    #[test]
    fn metadata_survives_a_new_store_for_the_same_session() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = bundle("demo", "hello");
        send(&store(&temp), &bundle, 1024);
        let reopened = store(&temp);
        let status = reopened.status().unwrap();
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].digest, bundle.digest());
    }

    #[test]
    fn a_tampered_bundle_is_reported_unavailable() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let bundle = bundle("demo", "hello");
        send(&store, &bundle, 1024);
        let path = bundle_dir(&temp).join(bundle.digest());
        let file = path.join("bin/run");
        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        // Deliberately tamper with a scratch bundle.
        permissions.set_readonly(false);
        std::fs::set_permissions(&file, permissions).unwrap();
        let status = store.status().unwrap();
        assert_eq!(status[0].status, ExtensionDeploymentStatus::Unavailable);
        let reason = status[0].reason.as_deref().unwrap_or_default();
        assert!(
            reason.contains("writable") || reason.contains("modified"),
            "{reason}"
        );
    }

    #[test]
    fn a_redeployment_always_moves_the_generation_forward() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let first = send(&store, &bundle("demo", "hello"), 1024)
            .generation
            .expect("generation");
        let redeployed = send(&store, &bundle("demo", "hello again"), 1024)
            .generation
            .expect("generation");
        assert!(redeployed > first, "{redeployed} after {first}");
        // Undeploying the newest record must not rewind the counter for the next deployment.
        assert!(store.undeploy("demo").unwrap());
        let redeployed_after_undeploy = send(&store, &bundle("demo", "hello"), 1024)
            .generation
            .expect("generation");
        assert!(
            redeployed_after_undeploy > redeployed,
            "{redeployed_after_undeploy} after {redeployed}"
        );
    }

    fn stored_bundles(temp: &tempfile::TempDir) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(bundle_dir(temp))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_redeployment_deletes_the_files_it_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let first = bundle("demo", "first");
        let second = bundle("demo", "second");
        send(&store, &first, 1024);
        send(&store, &second, 1024);
        assert_eq!(stored_bundles(&temp), [second.digest().to_string()]);
        assert!(store.undeploy("demo").unwrap());
        assert!(stored_bundles(&temp).is_empty());
    }

    #[test]
    fn an_upload_leaves_nothing_behind_when_it_finishes_or_is_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let uploads = temp.path().join("store").join(UPLOADS);
        let parts = || std::fs::read_dir(&uploads).unwrap().count();

        // Abandoned halfway, then a different version sent to completion.
        let abandoned = bundle("demo", "abandoned");
        let archive = abandoned.archive();
        store
            .append(
                "demo",
                abandoned.digest(),
                archive.len() as u64,
                0,
                &archive[..4],
                false,
            )
            .unwrap();
        assert_eq!(parts(), 1);
        send(&store, &bundle("demo", "finished"), 1024);
        assert_eq!(parts(), 0);

        // Refused at the end for not matching its digest.
        let incoming = bundle("demo", "goodbye");
        store
            .append(
                "demo",
                abandoned.digest(),
                incoming.archive().len() as u64,
                0,
                &incoming.archive(),
                true,
            )
            .unwrap_err();
        assert_eq!(parts(), 0);
    }

    #[test]
    fn clearing_an_extensions_uploads_spares_one_whose_id_it_prefixes() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let other = bundle("demo-two", "other");
        let archive = other.archive();
        store
            .append(
                "demo-two",
                other.digest(),
                archive.len() as u64,
                0,
                &archive[..4],
                false,
            )
            .unwrap();
        send(&store, &bundle("demo", "finished"), 1024);
        let uploads = temp.path().join("store").join(UPLOADS);
        assert_eq!(std::fs::read_dir(uploads).unwrap().count(), 1);
    }

    #[test]
    fn a_session_with_nothing_deployed_keeps_no_store() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        store.restart_generations().unwrap();
        assert!(store.prepare().unwrap().is_empty());
        assert!(store.status().unwrap().is_empty());
        assert!(!store.undeploy("demo").unwrap());
        assert!(!temp.path().join("store").exists());
    }

    #[test]
    fn a_restart_abandons_uploads_in_progress_and_moves_every_generation() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let generation = send(&store, &bundle("demo", "hello"), 1024)
            .generation
            .unwrap();
        let partial = bundle("demo", "partial");
        let archive = partial.archive();
        store
            .append(
                "demo",
                partial.digest(),
                archive.len() as u64,
                0,
                &archive[..4],
                false,
            )
            .unwrap();
        store.restart_generations().unwrap();
        assert!(!temp.path().join("store").join(UPLOADS).exists());
        assert!(store.status().unwrap()[0].generation > generation);
    }

    #[test]
    fn a_stored_manifest_this_host_rejects_is_reported_unavailable() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let accepted = bundle("demo", "hello");
        send(&store, &accepted, 1024);
        // A bundle stored by a `rozi` that accepted a manifest this one does not.
        let rejected = Bundle::from_files(vec![BundleFile {
            path: "extension.toml".to_string(),
            executable: false,
            contents: b"[extension]\nid = \"demo\"\ntitle = \"Demo\"\nversion = \"1\"\napi = 999\n"
                .to_vec(),
        }])
        .unwrap();
        BundleStore::open(bundle_dir(&temp))
            .unwrap()
            .stage(&rejected)
            .unwrap();
        let root = temp.path().join("store");
        let mut metadata = store.metadata(&root).unwrap();
        metadata.deployments.get_mut("demo").unwrap().digest = rejected.digest().to_string();
        store.write_metadata(&root, &metadata).unwrap();

        let status = store.status().unwrap();
        assert_eq!(status[0].status, ExtensionDeploymentStatus::Unavailable);
        assert!(
            status[0]
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("manifest"),
            "{:?}",
            status[0].reason
        );
        assert!(store.prepare().unwrap().is_empty());
    }

    #[test]
    fn a_rename_replaces_only_a_store_with_nothing_deployed() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("deployments");
        fs_security::ensure_private_dir(&parent).unwrap();
        let open = |name: &str| SessionDeployments::new(name, Some(parent.join(name)));
        let mut renamed = open("first");
        send(&renamed, &bundle("demo", "hello"), 1024);

        // Left by an earlier session of that name: once deployed to, then undeployed.
        let emptied = open("emptied");
        send(&emptied, &bundle("demo", "gone"), 1024);
        assert!(emptied.undeploy("demo").unwrap());
        renamed.rename("emptied").unwrap();
        assert_eq!(renamed.status().unwrap().len(), 1);

        let holding = open("holding");
        send(&holding, &bundle("demo", "kept"), 1024);
        renamed.rename("holding").unwrap_err();
        assert_eq!(holding.status().unwrap().len(), 1);
        assert_eq!(renamed.status().unwrap().len(), 1);
    }

    #[test]
    fn remove_all_deletes_metadata_and_bundles() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("session");
        let store = SessionDeployments::new("dev", Some(root.clone()));
        send(&store, &bundle("demo", "hello"), 1024);
        assert!(root.exists());
        store.remove_all().unwrap();
        assert!(!root.exists());
    }
}
