//! Bounded, non-following release archive inspection and extraction.

use super::manifest::ReleaseAsset;
use super::target::Target;
use super::{
    MAX_ARCHIVE_SIZE, MAX_MEMBER_SIZE, MAX_UNCOMPRESSED_SIZE, ReleaseError, Result,
    path_is_safe_directory, read_limited, sha256_bytes, verify_bytes,
};
use flate2::read::GzDecoder;
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use zip::CompressionMethod;

/// A member that matched one of the manifest's exact expected paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedMember {
    pub path: String,
    pub data: Vec<u8>,
}

/// All expected members after an archive has been completely inspected and hashed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractedRelease {
    pub payload: VerifiedMember,
    pub launcher: Option<VerifiedMember>,
}

pub type VerifiedArchive = ExtractedRelease;

/// Verify the published archive bytes before handing them to tar or ZIP parsing.
pub fn verify_archive_bytes(bytes: &[u8], asset: &ReleaseAsset) -> Result<()> {
    let target = target_from_asset(asset)?;
    asset.validate(&canonical_version_from_asset(asset, target), target)?;
    if bytes.len() as u64 > MAX_ARCHIVE_SIZE {
        return Err(ReleaseError::archive(format!(
            "archive exceeds maximum size {MAX_ARCHIVE_SIZE}"
        )));
    }
    verify_bytes(
        bytes,
        asset.archive_size,
        &asset.archive_sha256,
        "release archive",
    )
}

/// Read and verify an archive file before parsing it.
pub fn verify_archive_file(path: &Path, asset: &ReleaseAsset) -> Result<()> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(ReleaseError::archive(format!(
            "release archive is not a regular file: {}",
            path.display()
        )));
    }
    if metadata.len() > MAX_ARCHIVE_SIZE {
        return Err(ReleaseError::archive(format!(
            "archive exceeds maximum size {MAX_ARCHIVE_SIZE}"
        )));
    }
    let mut file = fs::File::open(path)?;
    let bytes = read_limited(&mut file, MAX_ARCHIVE_SIZE)?;
    verify_archive_bytes(&bytes, asset)
}

/// Inspect every archive member, verify the selected payload and optional launcher, and return
/// their bytes without writing any unrelated member to disk.
pub fn inspect_archive(bytes: &[u8], asset: &ReleaseAsset) -> Result<ExtractedRelease> {
    validate_asset_against_manifest_shape(asset)?;
    verify_archive_bytes(bytes, asset)?;
    if target_from_asset(asset)?.is_windows() {
        inspect_zip(bytes, asset)
    } else {
        inspect_tar_gz(bytes, asset)
    }
}

/// Inspect an archive and write only its exact expected executable members into `destination`.
/// The destination is the install directory; the canonical archive root is not recreated.
pub fn extract_archive(
    bytes: &[u8],
    asset: &ReleaseAsset,
    destination: &Path,
) -> Result<ExtractedPaths> {
    let release = inspect_archive(bytes, asset)?;
    path_is_safe_directory(destination)?;
    let payload_path = write_member(
        destination,
        &release.payload,
        target_from_asset(asset)?.payload_name(),
    )?;
    let launcher_path = match &release.launcher {
        Some(launcher) => Some(write_member(
            destination,
            launcher,
            target_from_asset(asset)?
                .launcher_name()
                .expect("validated Windows asset has a launcher"),
        )?),
        None => None,
    };
    Ok(ExtractedPaths {
        payload: payload_path,
        launcher: launcher_path,
    })
}

/// Verify and extract an archive file.
pub fn extract_archive_file(
    archive_path: &Path,
    asset: &ReleaseAsset,
    destination: &Path,
) -> Result<ExtractedPaths> {
    let metadata = fs::metadata(archive_path)?;
    if !metadata.is_file() || metadata.len() > MAX_ARCHIVE_SIZE {
        return Err(ReleaseError::archive(format!(
            "invalid or oversized release archive: {}",
            archive_path.display()
        )));
    }
    let mut file = fs::File::open(archive_path)?;
    let bytes = read_limited(&mut file, MAX_ARCHIVE_SIZE)?;
    extract_archive(&bytes, asset, destination)
}

/// Paths written by [`extract_archive`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractedPaths {
    pub payload: PathBuf,
    pub launcher: Option<PathBuf>,
}

pub type ExtractedFiles = ExtractedPaths;

fn inspect_tar_gz(bytes: &[u8], asset: &ReleaseAsset) -> Result<ExtractedRelease> {
    let target = target_from_asset(asset)?;
    let version = canonical_version_from_asset(asset, target);
    let root = target.root_name(&version);
    let mut archive = tar::Archive::new(GzDecoder::new(Cursor::new(bytes)));
    let mut names = HashSet::new();
    let mut total_uncompressed = 0u64;
    let mut payload = None;
    let mut launcher = None;
    let mut root_seen = false;
    let expected_launcher = target.launcher_path(&version);

    let entries = archive
        .entries()
        .map_err(|error| ReleaseError::archive(format!("invalid tar archive: {error}")))?;
    for (index, entry) in entries.enumerate() {
        let mut entry = entry.map_err(|error| {
            ReleaseError::archive(format!("invalid tar entry {index}: {error}"))
        })?;
        let raw_path = entry.path_bytes().into_owned();
        let path = validate_member_name(&raw_path, &root, index)?;
        if !names.insert(path.clone()) {
            return Err(ReleaseError::archive(format!(
                "duplicate archive member: {path}"
            )));
        }

        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink()
            || entry_type.is_hard_link()
            || entry_type.is_character_special()
            || entry_type.is_block_special()
            || entry_type.is_fifo()
            || entry_type.is_contiguous()
            || !entry_type.is_file() && !entry_type.is_dir()
        {
            return Err(ReleaseError::archive(format!(
                "unsupported or unsafe tar entry type for {path}"
            )));
        }

        if raw_path.last() == Some(&b'/') && entry_type.is_file() {
            return Err(ReleaseError::archive(format!(
                "regular tar member has a directory name: {path}"
            )));
        }

        let declared_size = entry.size();
        if declared_size > MAX_MEMBER_SIZE {
            return Err(ReleaseError::archive(format!(
                "tar member {path} exceeds maximum size {MAX_MEMBER_SIZE}"
            )));
        }
        if entry_type.is_dir() {
            if declared_size != 0 {
                return Err(ReleaseError::archive(format!(
                    "directory member {path} has nonzero size"
                )));
            }
            if path == root {
                root_seen = true;
            }
            continue;
        }
        if path == root {
            return Err(ReleaseError::archive(
                "canonical archive root must be a directory".to_string(),
            ));
        }

        total_uncompressed = total_uncompressed
            .checked_add(declared_size)
            .ok_or_else(|| ReleaseError::archive("tar uncompressed size overflow"))?;
        if total_uncompressed > MAX_UNCOMPRESSED_SIZE {
            return Err(ReleaseError::archive(format!(
                "tar contents exceed maximum uncompressed size {MAX_UNCOMPRESSED_SIZE}"
            )));
        }
        let expected = if path == asset.payload.path {
            Some((&asset.payload.size, &asset.payload.sha256, true))
        } else if expected_launcher.as_deref() == Some(path.as_str()) {
            let launcher = asset
                .launcher
                .as_ref()
                .expect("validated Windows launcher path cannot be selected for tar");
            Some((&launcher.size, &launcher.sha256, false))
        } else {
            None
        };
        let data = read_member_data(&mut entry, declared_size, expected.is_some(), &path)?;
        if let Some((expected_size, expected_hash, is_payload)) = expected {
            if entry.header().mode().unwrap_or_default() & 0o111 == 0 {
                return Err(ReleaseError::archive(format!(
                    "expected executable member is not executable: {path}"
                )));
            }
            verify_bytes(&data, *expected_size, expected_hash, &path)?;
            let member = VerifiedMember { path, data };
            if is_payload {
                payload = Some(member);
            } else {
                launcher = Some(member);
            }
        }
    }
    if !root_seen {
        return Err(ReleaseError::archive(format!(
            "archive has no canonical root directory {root}"
        )));
    }
    finish_members(payload, launcher, asset)
}

fn inspect_zip(bytes: &[u8], asset: &ReleaseAsset) -> Result<ExtractedRelease> {
    let target = target_from_asset(asset)?;
    let version = canonical_version_from_asset(asset, target);
    let root = target.root_name(&version);
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| ReleaseError::archive(format!("invalid ZIP archive: {error}")))?;
    let mut names = HashSet::new();
    let mut total_uncompressed = 0u64;
    let mut payload = None;
    let mut launcher = None;
    let mut root_seen = false;
    let expected_launcher = target.launcher_path(&version);

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            ReleaseError::archive(format!("invalid ZIP entry {index}: {error}"))
        })?;
        let raw_path = entry.name_raw().to_vec();
        let path = validate_member_name(&raw_path, &root, index)?;
        if !names.insert(path.clone()) {
            return Err(ReleaseError::archive(format!(
                "duplicate archive member: {path}"
            )));
        }
        if entry.encrypted() {
            return Err(ReleaseError::archive(format!(
                "encrypted ZIP member is not supported: {path}"
            )));
        }
        if !matches!(
            entry.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            return Err(ReleaseError::archive(format!(
                "unsupported ZIP compression for {path}"
            )));
        }
        if entry.is_symlink() {
            return Err(ReleaseError::archive(format!(
                "symbolic-link ZIP member is not supported: {path}"
            )));
        }
        if let Some(mode) = entry.unix_mode() {
            let file_type = mode & 0o170000;
            if file_type != 0 && file_type != 0o100000 && file_type != 0o040000 {
                return Err(ReleaseError::archive(format!(
                    "special ZIP member is not supported: {path}"
                )));
            }
            if file_type == 0o040000 && !entry.is_dir() {
                return Err(ReleaseError::archive(format!(
                    "ZIP directory mode disagrees with name: {path}"
                )));
            }
        }

        let declared_size = entry.size();
        if declared_size > MAX_MEMBER_SIZE {
            return Err(ReleaseError::archive(format!(
                "ZIP member {path} exceeds maximum size {MAX_MEMBER_SIZE}"
            )));
        }
        if entry.is_dir() {
            if declared_size != 0 {
                return Err(ReleaseError::archive(format!(
                    "directory member {path} has nonzero size"
                )));
            }
            if path == root {
                root_seen = true;
            }
            continue;
        }
        if path == root {
            return Err(ReleaseError::archive(
                "canonical archive root must be a directory".to_string(),
            ));
        }
        total_uncompressed = total_uncompressed
            .checked_add(declared_size)
            .ok_or_else(|| ReleaseError::archive("ZIP uncompressed size overflow"))?;
        if total_uncompressed > MAX_UNCOMPRESSED_SIZE {
            return Err(ReleaseError::archive(format!(
                "ZIP contents exceed maximum uncompressed size {MAX_UNCOMPRESSED_SIZE}"
            )));
        }

        let expected = if path == asset.payload.path {
            Some((&asset.payload.size, &asset.payload.sha256, true))
        } else if expected_launcher.as_deref() == Some(path.as_str()) {
            let launcher = asset
                .launcher
                .as_ref()
                .expect("validated Windows launcher path always has metadata");
            Some((&launcher.size, &launcher.sha256, false))
        } else {
            None
        };
        let data = read_member_data(&mut entry, declared_size, expected.is_some(), &path)?;
        if let Some((expected_size, expected_hash, is_payload)) = expected {
            verify_bytes(&data, *expected_size, expected_hash, &path)?;
            let member = VerifiedMember { path, data };
            if is_payload {
                payload = Some(member);
            } else {
                launcher = Some(member);
            }
        }
    }
    if !root_seen {
        return Err(ReleaseError::archive(format!(
            "archive has no canonical root directory {root}"
        )));
    }
    finish_members(payload, launcher, asset)
}

fn read_member_data<R: Read>(
    reader: &mut R,
    declared_size: u64,
    collect: bool,
    path: &str,
) -> Result<Vec<u8>> {
    let mut data = if collect {
        Vec::with_capacity(declared_size as usize)
    } else {
        Vec::new()
    };
    let mut buffer = [0u8; 64 * 1024];
    let mut actual = 0u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        actual = actual
            .checked_add(read as u64)
            .ok_or_else(|| ReleaseError::archive(format!("member {path} size overflow")))?;
        if actual > MAX_MEMBER_SIZE || actual > declared_size {
            return Err(ReleaseError::archive(format!(
                "member {path} actual size exceeds its declared size"
            )));
        }
        if collect {
            data.extend_from_slice(&buffer[..read]);
        }
    }
    if actual != declared_size {
        return Err(ReleaseError::archive(format!(
            "member {path} actual size {actual} differs from declared size {declared_size}"
        )));
    }
    Ok(data)
}

fn finish_members(
    payload: Option<VerifiedMember>,
    launcher: Option<VerifiedMember>,
    asset: &ReleaseAsset,
) -> Result<ExtractedRelease> {
    let payload = payload.ok_or_else(|| {
        ReleaseError::archive(format!(
            "archive did not contain exact payload {}",
            asset.payload.path
        ))
    })?;
    let target = target_from_asset(asset)?;
    if target.is_windows() && launcher.is_none() {
        return Err(ReleaseError::archive(
            "archive did not contain the exact Windows launcher".to_string(),
        ));
    }
    if !target.is_windows() && launcher.is_some() {
        return Err(ReleaseError::archive(
            "non-Windows archive contained an unexpected launcher".to_string(),
        ));
    }
    Ok(ExtractedRelease { payload, launcher })
}

fn validate_member_name(raw: &[u8], root: &str, index: usize) -> Result<String> {
    if raw.is_empty() || raw.contains(&0) || raw.contains(&b'\\') {
        return Err(ReleaseError::archive(format!(
            "malformed archive member name at index {index}"
        )));
    }
    let raw = std::str::from_utf8(raw).map_err(|_| {
        ReleaseError::archive(format!("non-UTF-8 archive member name at index {index}"))
    })?;
    if raw.starts_with('/') || raw.starts_with("//") {
        return Err(ReleaseError::archive(format!(
            "absolute archive member name: {raw:?}"
        )));
    }
    let without_trailing_slash = raw.strip_suffix('/').unwrap_or(raw);
    if without_trailing_slash.is_empty()
        || without_trailing_slash.contains("//")
        || without_trailing_slash
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(ReleaseError::archive(format!(
            "malformed archive member name: {raw:?}"
        )));
    }
    let mut components = without_trailing_slash.split('/');
    let first = components.next().unwrap_or_default();
    if first.contains(':')
        || first != root
        || without_trailing_slash
            .split('/')
            .any(|component| component.contains(':'))
    {
        return Err(ReleaseError::archive(format!(
            "archive member is outside canonical root {root:?}: {raw:?}"
        )));
    }
    Ok(without_trailing_slash.to_string())
}

fn write_member(destination: &Path, member: &VerifiedMember, filename: &str) -> Result<PathBuf> {
    if filename.is_empty() || filename.contains('/') || filename.contains('\\') {
        return Err(ReleaseError::archive("invalid output filename"));
    }
    let output = destination.join(filename);
    match fs::symlink_metadata(&output) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(ReleaseError::archive(format!(
                "refusing to overwrite symlink {}",
                output.display()
            )));
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err(ReleaseError::archive(format!(
                "refusing to overwrite non-file {}",
                output.display()
            )));
        }
        Ok(metadata) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.nlink() > 1 {
                    return Err(ReleaseError::archive(format!(
                        "refusing to overwrite hard-linked file {}",
                        output.display()
                    )));
                }
            }
            #[cfg(not(unix))]
            let _ = metadata;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&output)?;
    file.write_all(&member.data)?;
    file.flush()?;
    drop(file);
    let metadata = fs::metadata(&output)?;
    if metadata.len() != member.data.len() as u64 {
        return Err(ReleaseError::archive(format!(
            "written member size mismatch: {}",
            output.display()
        )));
    }
    if sha256_bytes(&member.data) != sha256_bytes(&fs::read(&output)?) {
        return Err(ReleaseError::archive(format!(
            "written member hash mismatch: {}",
            output.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&output, fs::Permissions::from_mode(0o755))?;
    }
    Ok(output)
}

fn validate_asset_against_manifest_shape(asset: &ReleaseAsset) -> Result<()> {
    // The asset API intentionally does not accept an independent version. The version embedded in
    // its canonical paths is therefore checked by the caller's manifest before this function is
    // reached. This catches malformed manually constructed assets without trusting archive names.
    if asset.archive_size == 0
        || asset.archive_size > MAX_ARCHIVE_SIZE
        || asset.payload.size == 0
        || asset.payload.size > MAX_MEMBER_SIZE
    {
        return Err(ReleaseError::archive("invalid release asset size"));
    }
    if asset.archive_sha256.len() != 64 || asset.payload.sha256.len() != 64 {
        return Err(ReleaseError::archive("invalid release asset hash"));
    }
    if target_from_asset(asset)?.is_windows() {
        let launcher = asset
            .launcher
            .as_ref()
            .ok_or_else(|| ReleaseError::archive("Windows asset has no launcher metadata"))?;
        if launcher.protocol != 1 || launcher.size == 0 || launcher.size > MAX_MEMBER_SIZE {
            return Err(ReleaseError::archive("invalid Windows launcher metadata"));
        }
    } else if asset.launcher.is_some() {
        return Err(ReleaseError::archive("unexpected launcher metadata"));
    }
    Ok(())
}

fn canonical_version_from_asset(asset: &ReleaseAsset, target: Target) -> semver::Version {
    // Archive entry names are compared with the manifest's paths. This fallback is only used by
    // public byte-level helpers that receive an asset rather than its parent manifest; parse the
    // version component from the canonical root instead of accepting arbitrary caller input.
    let suffix = format!("-{}", target.as_str());
    let root = asset
        .payload
        .path
        .split('/')
        .next()
        .and_then(|root| root.strip_prefix("hyprmux-"))
        .and_then(|rest| rest.strip_suffix(&suffix))
        .unwrap_or("0.0.0");
    semver::Version::parse(root).unwrap_or_else(|_| semver::Version::new(0, 0, 0))
}

fn target_from_asset(asset: &ReleaseAsset) -> Result<Target> {
    Target::ALL
        .into_iter()
        .find(|target| {
            asset.archive
                == target.archive_name(&canonical_version_from_archive_name(
                    &asset.archive,
                    *target,
                ))
        })
        .ok_or_else(|| ReleaseError::archive("release asset has no supported target"))
}

fn canonical_version_from_archive_name(name: &str, target: Target) -> semver::Version {
    let suffix = target.archive_suffix();
    let stem = name.strip_suffix(suffix).unwrap_or_default();
    let prefix = "hyprmux-";
    let version = stem
        .strip_prefix(prefix)
        .and_then(|value| value.strip_suffix(&format!("-{}", target.as_str())))
        .unwrap_or("0.0.0");
    semver::Version::parse(version).unwrap_or_else(|_| semver::Version::new(0, 0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release::Target;
    use flate2::{Compression, write::GzEncoder};
    use std::io::Cursor;
    use tar::Builder;
    use zip::write::{SimpleFileOptions, ZipWriter};

    const ARCHIVE_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const PAYLOAD_HASH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn tar_archive(_version: &semver::Version, path: &str, data: &[u8]) -> Vec<u8> {
        let root = path.split('/').next().unwrap_or(path);
        let builder_path = if path.contains("..") {
            format!("{root}/placeholder")
        } else {
            path.to_string()
        };
        let mut compressed = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut builder = Builder::new(&mut compressed);
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
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, builder_path, Cursor::new(data))
                .unwrap();
            builder.finish().unwrap();
        }
        let compressed = compressed.finish().unwrap();
        if path.contains("..") {
            rewrite_tar_member_path(&compressed, path)
        } else {
            compressed
        }
    }

    fn rewrite_tar_member_path(compressed: &[u8], path: &str) -> Vec<u8> {
        let mut decoded = Vec::new();
        GzDecoder::new(Cursor::new(compressed))
            .read_to_end(&mut decoded)
            .unwrap();
        assert!(path.len() < 100);
        let header = &mut decoded[512..1024];
        header[..100].fill(0);
        header[..path.len()].copy_from_slice(path.as_bytes());
        header[148..156].fill(b' ');
        let checksum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
        let checksum_field = format!("{checksum:06o}\0 ");
        header[148..156].copy_from_slice(checksum_field.as_bytes());

        let mut output = GzEncoder::new(Vec::new(), Compression::default());
        output.write_all(&decoded).unwrap();
        output.finish().unwrap()
    }

    fn zip_archive(path: &str, data: &[u8]) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut output);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            if let Some((root, _)) = path.split_once('/') {
                writer.add_directory(format!("{root}/"), options).unwrap();
            }
            writer.start_file(path, options).unwrap();
            writer.write_all(data).unwrap();
            writer.finish().unwrap();
        }
        output.into_inner()
    }

    fn zip_windows_archive(root: &str) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut output);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            writer.add_directory(format!("{root}/"), options).unwrap();
            writer
                .start_file(format!("{root}/hyprmux.exe"), options)
                .unwrap();
            writer.write_all(b"bin").unwrap();
            writer
                .start_file(format!("{root}/hyprmux-launcher.exe"), options)
                .unwrap();
            writer.write_all(b"run").unwrap();
            writer.finish().unwrap();
        }
        output.into_inner()
    }

    fn unix_asset(version: &semver::Version, archive: &[u8]) -> ReleaseAsset {
        let mut asset = ReleaseAsset::new(
            version,
            Target::X86_64UnknownLinuxGnu,
            archive.len() as u64,
            sha256_bytes(archive),
            3,
            sha256_bytes(b"bin"),
        );
        asset.payload.path = Target::X86_64UnknownLinuxGnu.payload_path(version);
        asset
    }

    #[test]
    fn tar_payload_hash_and_archive_hash_are_checked() {
        let version = semver::Version::parse("1.2.3").unwrap();
        let root = Target::X86_64UnknownLinuxGnu.root_name(&version);
        let archive = tar_archive(&version, &format!("{root}/hyprmux"), b"bin");
        let asset = unix_asset(&version, &archive);
        assert_eq!(
            inspect_archive(&archive, &asset).unwrap().payload.data,
            b"bin"
        );

        let mut bad_archive = asset.clone();
        bad_archive.archive_sha256 = ARCHIVE_HASH.to_string();
        assert!(inspect_archive(&archive, &bad_archive).is_err());
        let mut bad_payload = asset;
        bad_payload.payload.sha256 = PAYLOAD_HASH.to_string();
        assert!(inspect_archive(&archive, &bad_payload).is_err());
    }

    #[test]
    fn tar_traversal_symlink_and_duplicate_members_are_rejected() {
        let version = semver::Version::parse("1.2.3").unwrap();
        let root = Target::X86_64UnknownLinuxGnu.root_name(&version);
        let archive = tar_archive(&version, &format!("{root}/../hyprmux"), b"bin");
        let asset = unix_asset(&version, &archive);
        assert!(inspect_archive(&archive, &asset).is_err());

        let mut compressed = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut builder = Builder::new(&mut compressed);
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
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_link_name("somewhere").unwrap();
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    format!("{root}/hyprmux"),
                    Cursor::new(Vec::<u8>::new()),
                )
                .unwrap();
            builder.finish().unwrap();
        }
        let symlink = compressed.finish().unwrap();
        let symlink_asset = unix_asset(&version, &symlink);
        assert!(inspect_archive(&symlink, &symlink_asset).is_err());

        // The single-member helper cannot create a duplicate, but an archive with a second
        // identical header must be rejected before either payload is selected.
        let mut compressed = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut builder = Builder::new(&mut compressed);
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
            for _ in 0..2 {
                let mut header = tar::Header::new_gnu();
                header.set_size(3);
                header.set_cksum();
                builder
                    .append_data(&mut header, format!("{root}/hyprmux"), Cursor::new(b"bin"))
                    .unwrap();
            }
            builder.finish().unwrap();
        }
        let duplicate = compressed.finish().unwrap();
        let duplicate_asset = unix_asset(&version, &duplicate);
        assert!(inspect_archive(&duplicate, &duplicate_asset).is_err());
    }

    #[test]
    fn zip_payload_is_checked_and_traversal_is_rejected() {
        let version = semver::Version::parse("1.2.3").unwrap();
        let target = Target::X86_64PcWindowsMsvc;
        let root = target.root_name(&version);
        let complete = zip_windows_archive(&root);
        let complete_asset = ReleaseAsset::new(
            &version,
            target,
            complete.len() as u64,
            sha256_bytes(&complete),
            3,
            sha256_bytes(b"bin"),
        )
        .with_launcher(&version, target, 1, 3, sha256_bytes(b"run"));
        let complete_release = inspect_archive(&complete, &complete_asset).unwrap();
        assert_eq!(complete_release.payload.data, b"bin");
        assert_eq!(complete_release.launcher.unwrap().data, b"run");

        let payload = zip_archive(&format!("{root}/hyprmux.exe"), b"bin");
        let mut asset = ReleaseAsset::new(
            &version,
            target,
            payload.len() as u64,
            sha256_bytes(&payload),
            3,
            sha256_bytes(b"bin"),
        )
        .with_launcher(&version, target, 1, 3, sha256_bytes(b"run"));
        // The launcher is required for Windows, so a payload-only archive must fail.
        assert!(inspect_archive(&payload, &asset).is_err());
        asset.launcher = None;
        assert!(inspect_archive(&payload, &asset).is_err());

        let traversal = zip_archive("../hyprmux.exe", b"bin");
        let mut traversal_asset = ReleaseAsset::new(
            &version,
            target,
            traversal.len() as u64,
            sha256_bytes(&traversal),
            3,
            sha256_bytes(b"bin"),
        )
        .with_launcher(&version, target, 1, 3, sha256_bytes(b"run"));
        traversal_asset.archive_size = traversal.len() as u64;
        assert!(inspect_archive(&traversal, &traversal_asset).is_err());
    }
}
