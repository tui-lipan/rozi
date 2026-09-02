//! Installation lifecycle for extension directories.
//!
//! Installations use a stable source model and keep Rozi-owned metadata outside the extension
//! payload. Linked checkouts therefore remain user-owned, while Git installs retain the remote and
//! exact checked-out revision needed by a future explicit update command.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

const INSTALLATION_SCHEMA_VERSION: u32 = 1;
const CONTROL_DIRECTORY: &str = ".rozi";
static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InstallRequest {
    Source(String),
    Link(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InstalledExtension {
    pub(crate) id: String,
    pub(crate) destination: PathBuf,
    pub(crate) kind: InstallKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InstallKind {
    Local,
    Git,
    Link,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UpdatedExtension {
    pub(crate) id: String,
    pub(crate) changed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemovedExtension {
    pub(crate) id: String,
    pub(crate) linked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct InstallationRecord {
    schema_version: u32,
    id: String,
    source: InstallationSource,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum InstallationSource {
    Local { path: String },
    Git { remote: String, revision: String },
    Link { path: String },
}

enum ResolvedSource {
    Local(PathBuf),
    Git(String),
    Link(PathBuf),
}

impl InstallationSource {
    fn kind(&self) -> InstallKind {
        match self {
            Self::Local { .. } => InstallKind::Local,
            Self::Git { .. } => InstallKind::Git,
            Self::Link { .. } => InstallKind::Link,
        }
    }
}

pub(crate) fn install(request: InstallRequest) -> Result<InstalledExtension, String> {
    let source = resolve_source(request)?;
    let source_path = match &source {
        ResolvedSource::Local(path) | ResolvedSource::Link(path) => Some(path.as_path()),
        ResolvedSource::Git(_) => None,
    };
    let source_id = source_path.map(validate_candidate).transpose()?;
    let user = crate::config::read_user_extension_config()?;

    let root = crate::config::extensions_dir_path();
    ensure_storage_root(&root)?;
    reject_scan_errors_and_conflicts(&root, source_id.as_deref())?;

    let control = root.join(CONTROL_DIRECTORY);
    let records = control.join("installations");
    crate::platform::fs_security::ensure_private_dir(&records).map_err(|error| {
        format!(
            "Could not prepare extension metadata storage {}: {error}",
            records.display()
        )
    })?;

    match source {
        ResolvedSource::Local(path) => {
            install_local(&root, &control, &records, &path, &user.disabled)
        }
        ResolvedSource::Git(remote) => {
            install_git(&root, &control, &records, &remote, &user.disabled)
        }
        ResolvedSource::Link(path) => {
            let id = source_id.expect("linked source was validated");
            install_link(&root, &records, &path, &id, &user.disabled)
        }
    }
}

pub(crate) fn installation_kind(id: &str) -> Option<InstallKind> {
    read_record(&crate::config::extensions_dir_path(), id)
        .ok()
        .flatten()
        .map(|record| record.source.kind())
}

pub(crate) fn forget_installation_record(id: &str) -> Result<(), String> {
    crate::config::validate_extension_installation_id(id)?;
    remove_record(&crate::config::extensions_dir_path(), id)
}

pub(crate) fn update_available(id: &str) -> Result<bool, String> {
    let root = crate::config::extensions_dir_path();
    let record = required_git_record(&root, id)?;
    let InstallationSource::Git { remote, revision } = record.source else {
        unreachable!("required_git_record returned a non-Git source");
    };
    let installed = root.join(id);
    if git_revision(&installed)? != revision {
        return Err(format!(
            "Extension `{id}` no longer matches its recorded installed revision"
        ));
    }
    Ok(remote_head(&remote)? != revision)
}

pub(crate) fn update(id: &str) -> Result<UpdatedExtension, String> {
    crate::config::validate_extension_installation_id(id)?;
    let root = crate::config::extensions_dir_path();
    ensure_storage_root(&root)?;
    let record = required_git_record(&root, id)?;
    let InstallationSource::Git {
        remote,
        revision: installed_revision,
    } = record.source
    else {
        unreachable!("required_git_record returned a non-Git source");
    };
    let destination = root.join(id);
    if git_revision(&destination)? != installed_revision {
        return Err(format!(
            "Extension `{id}` no longer matches its recorded installed revision"
        ));
    }
    reject_dirty_checkout(&destination, id)?;

    let control = root.join(CONTROL_DIRECTORY);
    let staging = staging_path(&control)?;
    if let Err(error) = clone_git(&remote, &staging) {
        cleanup_staging(&staging);
        return Err(error);
    }
    let new_revision = match git_revision(&staging) {
        Ok(revision) => revision,
        Err(error) => {
            cleanup_staging(&staging);
            return Err(error);
        }
    };
    if let Err(error) = validate_installed_candidate(&staging, id) {
        cleanup_staging(&staging);
        return Err(error);
    }
    if new_revision == installed_revision {
        cleanup_staging(&staging);
        return Ok(UpdatedExtension {
            id: id.to_string(),
            changed: false,
        });
    }

    replace_git_installation(
        &root,
        &control,
        &destination,
        staging,
        InstallationRecord {
            schema_version: INSTALLATION_SCHEMA_VERSION,
            id: id.to_string(),
            source: InstallationSource::Git {
                remote,
                revision: new_revision,
            },
        },
    )?;
    Ok(UpdatedExtension {
        id: id.to_string(),
        changed: true,
    })
}

pub(crate) fn remove(id: &str) -> Result<RemovedExtension, String> {
    crate::config::validate_extension_installation_id(id)?;
    let user = crate::config::read_user_extension_config()?;
    let root = crate::config::extensions_dir_path();
    ensure_storage_root(&root)?;
    let scan = crate::config::scan_extensions();
    if let Some(error) = scan.root_errors.first() {
        return Err(error.clone());
    }
    let matches = scan
        .entries()
        .into_iter()
        .filter(|entry| entry.id.as_deref() == Some(id))
        .collect::<Vec<_>>();
    let entry = match matches.as_slice() {
        [] => return Err(format!("Extension `{id}` is not installed")),
        [entry] => entry,
        _ => {
            return Err(format!(
                "Extension id `{id}` is declared by multiple installations; remove the conflict in {} first",
                root.display()
            ));
        }
    };
    let path = crate::platform::extensions::resolve_installation_path(&root, &entry.path)?;
    let linked = fs::symlink_metadata(&path)
        .map_err(|error| format!("Could not inspect extension {}: {error}", path.display()))?
        .file_type()
        .is_symlink();

    crate::platform::extensions::remove_installation(&root, &path)?;
    remove_record(&root, id)?;

    if user.disabled.iter().any(|candidate| candidate.trim() == id) {
        let disabled = user
            .disabled
            .into_iter()
            .filter(|candidate| candidate.trim() != id)
            .collect::<Vec<_>>();
        crate::config::persist_extensions_disabled(&disabled).map_err(|error| {
            format!("Extension `{id}` was removed, but its disabled config entry remains: {error}")
        })?;
    }

    Ok(RemovedExtension {
        id: id.to_string(),
        linked,
    })
}

fn ensure_storage_root(root: &Path) -> Result<(), String> {
    let data_root = root
        .parent()
        .ok_or_else(|| format!("Extension storage has no parent: {}", root.display()))?;
    for directory in [data_root, root] {
        crate::platform::fs_security::ensure_private_dir(directory).map_err(|error| {
            format!(
                "Could not prepare extension storage {}: {error}",
                directory.display()
            )
        })?;
    }
    Ok(())
}

fn install_local(
    root: &Path,
    control: &Path,
    records: &Path,
    source: &Path,
    disabled: &[String],
) -> Result<InstalledExtension, String> {
    let staging = staging_path(control)?;
    if let Err(error) = crate::platform::extensions::copy_directory(source, &staging) {
        cleanup_staging(&staging);
        return Err(error);
    }
    let path = utf8_path(source, "Local extension source")?;
    finish_managed_install(
        root,
        records,
        staging,
        InstallationSource::Local { path },
        InstallKind::Local,
        disabled,
    )
}

fn install_git(
    root: &Path,
    control: &Path,
    records: &Path,
    remote: &str,
    disabled: &[String],
) -> Result<InstalledExtension, String> {
    let staging = staging_path(control)?;
    if let Err(error) = clone_git(remote, &staging) {
        cleanup_staging(&staging);
        return Err(error);
    }
    let revision = match git_revision(&staging) {
        Ok(revision) => revision,
        Err(error) => {
            cleanup_staging(&staging);
            return Err(error);
        }
    };
    finish_managed_install(
        root,
        records,
        staging,
        InstallationSource::Git {
            remote: remote.to_string(),
            revision,
        },
        InstallKind::Git,
        disabled,
    )
}

fn install_link(
    root: &Path,
    records: &Path,
    source: &Path,
    id: &str,
    disabled: &[String],
) -> Result<InstalledExtension, String> {
    reject_destination_conflict(root, records, id)?;
    let destination = root.join(id);
    crate::platform::extensions::create_directory_link(source, &destination)?;
    if let Err(error) = validate_installed_candidate(&destination, id) {
        let _ = crate::platform::extensions::remove_installation(root, &destination);
        return Err(error);
    }
    let record = InstallationRecord {
        schema_version: INSTALLATION_SCHEMA_VERSION,
        id: id.to_string(),
        source: InstallationSource::Link {
            path: utf8_path(source, "Linked extension source")?,
        },
    };
    complete_install(
        root,
        records,
        destination,
        record,
        InstallKind::Link,
        disabled,
    )
}

fn finish_managed_install(
    root: &Path,
    records: &Path,
    staging: PathBuf,
    source: InstallationSource,
    kind: InstallKind,
    disabled: &[String],
) -> Result<InstalledExtension, String> {
    let id = match validate_candidate(&staging) {
        Ok(id) => id,
        Err(error) => {
            cleanup_staging(&staging);
            return Err(error);
        }
    };
    reject_scan_errors_and_conflicts(root, Some(&id)).inspect_err(|_| cleanup_staging(&staging))?;
    reject_destination_conflict(root, records, &id).inspect_err(|_| cleanup_staging(&staging))?;
    let destination = root.join(&id);
    fs::rename(&staging, &destination).map_err(|error| {
        cleanup_staging(&staging);
        format!(
            "Could not move extension into {}: {error}",
            destination.display()
        )
    })?;
    if let Err(error) = validate_installed_candidate(&destination, &id) {
        let _ = crate::platform::extensions::remove_installation(root, &destination);
        return Err(error);
    }
    let record = InstallationRecord {
        schema_version: INSTALLATION_SCHEMA_VERSION,
        id,
        source,
    };
    complete_install(root, records, destination, record, kind, disabled)
}

fn complete_install(
    root: &Path,
    records: &Path,
    destination: PathBuf,
    record: InstallationRecord,
    kind: InstallKind,
    disabled: &[String],
) -> Result<InstalledExtension, String> {
    let id = record.id.clone();
    if let Err(error) = write_record(records, &record) {
        let _ = crate::platform::extensions::remove_installation(root, &destination);
        return Err(error);
    }
    if disabled.iter().any(|candidate| candidate.trim() == id) {
        let enabled = disabled
            .iter()
            .filter(|candidate| candidate.trim() != id)
            .cloned()
            .collect::<Vec<_>>();
        if let Err(error) = crate::config::persist_extensions_disabled(&enabled) {
            let _ = remove_record(root, &id);
            let _ = crate::platform::extensions::remove_installation(root, &destination);
            return Err(format!("Could not enable extension `{id}`: {error}"));
        }
    }
    Ok(InstalledExtension {
        id,
        destination,
        kind,
    })
}

fn resolve_source(request: InstallRequest) -> Result<ResolvedSource, String> {
    match request {
        InstallRequest::Link(path) => {
            canonical_directory(&path, "Linked extension source").map(ResolvedSource::Link)
        }
        InstallRequest::Source(source) => {
            let path = Path::new(&source);
            match fs::symlink_metadata(path) {
                Ok(_) => {
                    canonical_directory(path, "Local extension source").map(ResolvedSource::Local)
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if is_git_url(&source) {
                        Ok(ResolvedSource::Git(source))
                    } else {
                        Err(format!(
                            "Invalid extension source `{source}`: expected an existing local directory, an HTTPS Git URL, or an SSH Git URL"
                        ))
                    }
                }
                Err(error) => Err(format!(
                    "Could not inspect extension source {}: {error}",
                    path.display()
                )),
            }
        }
    }
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, String> {
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("Could not resolve {label} {}: {error}", path.display()))?;
    if !canonical.is_dir() {
        return Err(format!("{label} is not a directory: {}", path.display()));
    }
    utf8_path(&canonical, label)?;
    Ok(canonical)
}

fn is_git_url(source: &str) -> bool {
    if let Ok(url) = url::Url::parse(source)
        && matches!(url.scheme(), "https" | "ssh")
    {
        return url.host_str().is_some()
            && url.path() != "/"
            && !url.path().is_empty()
            && url.fragment().is_none();
    }
    let Some((user, host_and_path)) = source.split_once('@') else {
        return false;
    };
    let Some((host, path)) = host_and_path.split_once(':') else {
        return false;
    };
    !user.is_empty()
        && !host.is_empty()
        && !path.is_empty()
        && !source.chars().any(char::is_whitespace)
}

fn validate_candidate(path: &Path) -> Result<String, String> {
    let candidate = crate::config::check_extension(path);
    let info = candidate.info;
    if info.status != crate::config::ExtensionStatus::Loaded {
        let reason = if info.errors.is_empty() {
            info.status.as_str().to_string()
        } else {
            info.errors.join("; ")
        };
        return Err(format!(
            "Extension source {} is not installable: {reason}",
            path.display()
        ));
    }
    info.id
        .ok_or_else(|| format!("Extension source {} has no extension id", path.display()))
}

fn validate_installed_candidate(path: &Path, expected_id: &str) -> Result<(), String> {
    let actual_id = validate_candidate(path)?;
    if actual_id != expected_id {
        return Err(format!(
            "Installed extension id changed from `{expected_id}` to `{actual_id}`"
        ));
    }
    Ok(())
}

fn reject_scan_errors_and_conflicts(root: &Path, id: Option<&str>) -> Result<(), String> {
    let scan = crate::config::scan_extensions();
    if let Some(error) = scan.root_errors.first() {
        return Err(error.clone());
    }
    if let Some(id) = id
        && let Some(existing) = scan
            .entries()
            .into_iter()
            .find(|entry| entry.id.as_deref() == Some(id))
    {
        return Err(format!(
            "Extension id `{id}` is already installed at {}",
            existing.path
        ));
    }
    let metadata = root.join(CONTROL_DIRECTORY);
    if fs::symlink_metadata(&metadata).is_ok() && !metadata.is_dir() {
        return Err(format!(
            "Unsafe extension metadata destination: {} is not a directory",
            metadata.display()
        ));
    }
    Ok(())
}

fn reject_destination_conflict(root: &Path, records: &Path, id: &str) -> Result<(), String> {
    crate::config::validate_extension_installation_id(id)?;
    for path in [root.join(id), records.join(format!("{id}.toml"))] {
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                return Err(format!(
                    "Extension installation destination already exists: {}",
                    path.display()
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Could not inspect installation destination {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

fn staging_path(control: &Path) -> Result<PathBuf, String> {
    let staging_root = control.join("staging");
    crate::platform::fs_security::ensure_private_dir(&staging_root).map_err(|error| {
        format!(
            "Could not prepare extension staging directory {}: {error}",
            staging_root.display()
        )
    })?;
    for _ in 0..100 {
        let sequence = NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed);
        let path = staging_root.join(format!("{}-{sequence}", std::process::id()));
        if !path.exists() {
            return Ok(path);
        }
    }
    Err("Could not allocate a unique extension staging directory".to_string())
}

fn cleanup_staging(path: &Path) {
    if fs::symlink_metadata(path).is_ok() {
        let _ = fs::remove_dir_all(path);
    }
}

fn clone_git(remote: &str, destination: &Path) -> Result<(), String> {
    let mut command = Command::new("git");
    command.arg("clone").arg("--").arg(remote).arg(destination);
    let output = non_interactive_git(&mut command)
        .output()
        .map_err(|error| format!("Could not run `git clone`: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Could not clone Git extension from `{remote}`: {}",
            command_failure(&output)
        ))
    }
}

fn remote_head(remote: &str) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .args(["ls-remote", "--exit-code", "--"])
        .arg(remote)
        .arg("HEAD");
    let output = non_interactive_git(&mut command)
        .output()
        .map_err(|error| format!("Could not run `git ls-remote`: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Could not check Git extension remote `{remote}`: {}",
            command_failure(&output)
        ));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| "Git returned a non-UTF-8 remote revision".to_string())?;
    let revision = stdout
        .split_whitespace()
        .next()
        .ok_or_else(|| format!("Git remote `{remote}` did not report HEAD"))?;
    validate_git_revision(revision, "remote")
}

fn non_interactive_git(command: &mut Command) -> &mut Command {
    command.env("GIT_TERMINAL_PROMPT", "0")
}

fn reject_dirty_checkout(repository: &Path, id: &str) -> Result<(), String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["status", "--porcelain", "--untracked-files=all"])
        .output()
        .map_err(|error| format!("Could not inspect Git extension `{id}`: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Could not inspect Git extension `{id}`: {}",
            command_failure(&output)
        ));
    }
    if output.stdout.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Git extension `{id}` has local changes; refusing to replace its managed checkout"
        ))
    }
}

fn git_revision(repository: &Path) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .map_err(|error| format!("Could not inspect cloned Git revision: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Could not inspect cloned Git revision: {}",
            command_failure(&output)
        ));
    }
    let revision = String::from_utf8(output.stdout)
        .map_err(|_| "Git returned a non-UTF-8 revision".to_string())?
        .trim()
        .to_string();
    validate_git_revision(&revision, "installed")
}

fn validate_git_revision(revision: &str, label: &str) -> Result<String, String> {
    if revision.len() >= 40
        && revision
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        Ok(revision.to_string())
    } else {
        Err(format!(
            "Git returned an invalid {label} revision `{revision}`"
        ))
    }
}

fn command_failure(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        output.status.code().map_or_else(
            || "process terminated by a signal".to_string(),
            |code| format!("exit status {code}"),
        )
    } else {
        stderr.to_string()
    }
}

fn utf8_path(path: &Path, label: &str) -> Result<String, String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| format!("{label} path is not valid UTF-8: {}", path.display()))
}

fn required_git_record(root: &Path, id: &str) -> Result<InstallationRecord, String> {
    crate::config::validate_extension_installation_id(id)?;
    let record = read_record(root, id)?
        .ok_or_else(|| format!("Extension `{id}` is not a Rozi-managed Git installation"))?;
    if matches!(record.source, InstallationSource::Git { .. }) {
        Ok(record)
    } else {
        Err(format!(
            "Extension `{id}` is not a Rozi-managed Git installation"
        ))
    }
}

fn read_record(root: &Path, id: &str) -> Result<Option<InstallationRecord>, String> {
    let path = record_path(root, id);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "Could not inspect extension source metadata {}: {error}",
                path.display()
            ));
        }
    };
    if !metadata.file_type().is_file() {
        return Err(format!(
            "Extension source metadata is not a regular file: {}",
            path.display()
        ));
    }
    let text = fs::read_to_string(&path).map_err(|error| {
        format!(
            "Could not read extension source metadata {}: {error}",
            path.display()
        )
    })?;
    let record: InstallationRecord = toml::from_str(&text).map_err(|error| {
        format!(
            "Could not parse extension source metadata {}: {error}",
            path.display()
        )
    })?;
    if record.schema_version != INSTALLATION_SCHEMA_VERSION {
        return Err(format!(
            "Unsupported extension source metadata schema {} in {}",
            record.schema_version,
            path.display()
        ));
    }
    if record.id != id {
        return Err(format!(
            "Extension source metadata {} belongs to `{}` instead of `{id}`",
            path.display(),
            record.id
        ));
    }
    Ok(Some(record))
}

fn replace_git_installation(
    root: &Path,
    control: &Path,
    destination: &Path,
    staging: PathBuf,
    record: InstallationRecord,
) -> Result<(), String> {
    let backup = staging_path(control).inspect_err(|_| cleanup_staging(&staging))?;
    fs::rename(destination, &backup).map_err(|error| {
        cleanup_staging(&staging);
        format!(
            "Could not preserve the current extension checkout {}: {error}",
            destination.display()
        )
    })?;
    if let Err(error) = fs::rename(&staging, destination) {
        let rollback = fs::rename(&backup, destination);
        cleanup_staging(&staging);
        return match rollback {
            Ok(()) => Err(format!(
                "Could not replace extension checkout {}: {error}",
                destination.display()
            )),
            Err(rollback_error) => Err(format!(
                "Could not replace extension checkout {}: {error}; restoring the previous checkout also failed: {rollback_error}",
                destination.display()
            )),
        };
    }
    if let Err(error) = replace_record(root, &record) {
        let _ = crate::platform::extensions::remove_installation(root, destination);
        return match fs::rename(&backup, destination) {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(format!(
                "{error}; restoring the previous checkout also failed: {rollback_error}"
            )),
        };
    }
    cleanup_staging(&backup);
    Ok(())
}

fn replace_record(root: &Path, record: &InstallationRecord) -> Result<(), String> {
    let path = record_path(root, &record.id);
    let records = path
        .parent()
        .ok_or_else(|| format!("Extension metadata path has no parent: {}", path.display()))?;
    let temporary = unique_path(records, &format!(".{}.new", record.id))?;
    let backup = unique_path(records, &format!(".{}.old", record.id))?;
    let text = toml::to_string(record)
        .map_err(|error| format!("Could not encode extension source metadata: {error}"))?;
    crate::platform::fs_security::write_private_file(&temporary, text.as_bytes()).map_err(
        |error| {
            format!(
                "Could not write extension source metadata {}: {error}",
                temporary.display()
            )
        },
    )?;
    if let Err(error) = fs::rename(&path, &backup) {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "Could not preserve extension source metadata {}: {error}",
            path.display()
        ));
    }
    if let Err(error) = fs::rename(&temporary, &path) {
        let rollback = fs::rename(&backup, &path);
        let _ = fs::remove_file(&temporary);
        return match rollback {
            Ok(()) => Err(format!(
                "Could not replace extension source metadata {}: {error}",
                path.display()
            )),
            Err(rollback_error) => Err(format!(
                "Could not replace extension source metadata {}: {error}; restoring the previous record also failed: {rollback_error}",
                path.display()
            )),
        };
    }
    let _ = fs::remove_file(backup);
    Ok(())
}

fn unique_path(parent: &Path, prefix: &str) -> Result<PathBuf, String> {
    for _ in 0..100 {
        let sequence = NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!("{prefix}-{}-{sequence}", std::process::id()));
        if fs::symlink_metadata(&path).is_err() {
            return Ok(path);
        }
    }
    Err(format!(
        "Could not allocate a temporary path in {}",
        parent.display()
    ))
}

fn write_record(records: &Path, record: &InstallationRecord) -> Result<(), String> {
    let text = toml::to_string(record)
        .map_err(|error| format!("Could not encode extension source metadata: {error}"))?;
    let path = records.join(format!("{}.toml", record.id));
    crate::platform::fs_security::write_private_file(&path, text.as_bytes()).map_err(|error| {
        format!(
            "Could not write extension source metadata {}: {error}",
            path.display()
        )
    })
}

fn record_path(root: &Path, id: &str) -> PathBuf {
    root.join(CONTROL_DIRECTORY)
        .join("installations")
        .join(format!("{id}.toml"))
}

fn remove_record(root: &Path, id: &str) -> Result<(), String> {
    let path = record_path(root, id);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Could not remove extension source metadata {}: {error}",
            path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_source_detection_accepts_https_and_ssh_forms_only() {
        for source in [
            "https://github.com/user/extension.git",
            "ssh://git@github.com/user/extension.git",
            "git@github.com:user/extension.git",
        ] {
            assert!(is_git_url(source), "rejected {source}");
        }
        for source in [
            "http://github.com/user/extension.git",
            "ftp://github.com/user/extension.git",
            "github.com/user/extension",
            "git@github.com:",
        ] {
            assert!(!is_git_url(source), "accepted {source}");
        }
    }

    #[test]
    fn git_metadata_round_trips_the_remote_and_revision() {
        let record = InstallationRecord {
            schema_version: INSTALLATION_SCHEMA_VERSION,
            id: "git-tools".to_string(),
            source: InstallationSource::Git {
                remote: "git@github.com:user/git-tools.git".to_string(),
                revision: "0123456789012345678901234567890123456789".to_string(),
            },
        };
        let text = toml::to_string(&record).unwrap();
        let decoded: InstallationRecord = toml::from_str(&text).unwrap();
        assert_eq!(decoded, record);
    }
}
