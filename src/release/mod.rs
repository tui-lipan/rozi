//! Signed release metadata, verification, downloading, and extraction.
//!
//! The release code deliberately keeps the trust boundary small: metadata is verified over the
//! bytes as received, archives are checked before an archive reader sees them, and extraction only
//! writes the two files selected by the validated manifest.

pub mod archive;
pub mod download;
pub mod manifest;
pub mod signature;
pub mod target;

pub use archive::{
    ExtractedFiles, ExtractedPaths, ExtractedRelease, VerifiedArchive, VerifiedMember,
    extract_archive, extract_archive_file, inspect_archive, verify_archive_bytes,
    verify_archive_file,
};
pub use download::{
    DownloadResponse, DownloadedArchive, Downloader, ReleaseMetadata, UreqDownloader,
    download_archive, fetch_exact_metadata, fetch_latest_metadata, fetch_latest_metadata_with_keys,
    fetch_version_metadata_with_keys,
};
pub use manifest::{
    Asset, FileDigest, LauncherInfo, Manifest, PayloadInfo, ReleaseAsset, ReleaseManifest,
    SelectedAsset,
};
pub use signature::{
    SignatureEntry, SignatureEnvelope, TrustedKey, TrustedKeySet, VerifiedSignature,
    compiled_trusted_keys, sign_manifest, sign_manifest_bytes, verify_manifest,
    verify_manifest_with_keys,
};
pub use target::{ReleaseTarget, Target};

use sha2::{Digest, Sha256};
use std::fmt;
use std::io::{self, Read};
use std::path::Path;

/// Maximum bytes accepted for a published archive.
pub const MAX_ARCHIVE_SIZE: u64 = 256 * 1024 * 1024;
/// Maximum bytes accepted for one extracted member.
pub const MAX_MEMBER_SIZE: u64 = 256 * 1024 * 1024;
/// Maximum uncompressed bytes inspected in one archive.
pub const MAX_UNCOMPRESSED_SIZE: u64 = 256 * 1024 * 1024;
/// Maximum metadata or detached-signature response body.
pub const MAX_METADATA_SIZE: usize = 1024 * 1024;

/// Errors raised while handling a signed release.
#[derive(Debug)]
pub enum ReleaseError {
    /// The supplied release data is structurally or cryptographically invalid.
    Invalid(String),
    /// No compiled or injected release signing key was available.
    TrustAnchorNotConfigured,
    /// A local file or stream could not be read or written.
    Io(io::Error),
    /// JSON could not be decoded or encoded.
    Json(serde_json::Error),
    /// The HTTP client could not fetch a release response.
    Download(String),
    /// The archive could not be safely inspected or extracted.
    Archive(String),
}

/// Result type shared by the release modules.
pub type Result<T> = std::result::Result<T, ReleaseError>;
pub type Error = ReleaseError;

impl ReleaseError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    pub(crate) fn archive(message: impl Into<String>) -> Self {
        Self::Archive(message.into())
    }

    pub(crate) fn download(message: impl Into<String>) -> Self {
        Self::Download(message.into())
    }
}

impl fmt::Display for ReleaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::TrustAnchorNotConfigured => {
                f.write_str("trust anchor not configured: no trusted release signing key")
            }
            Self::Io(error) => write!(f, "release I/O error: {error}"),
            Self::Json(error) => write!(f, "release JSON error: {error}"),
            Self::Download(message) => write!(f, "release download error: {message}"),
            Self::Archive(message) => write!(f, "release archive error: {message}"),
        }
    }
}

impl std::error::Error for ReleaseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for ReleaseError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for ReleaseError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Hash bytes with SHA-256 and return lowercase hexadecimal.
pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Hash a reader without loading it into memory as a whole.
pub fn sha256_reader<R: Read>(reader: &mut R) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Hash a file without loading it into memory as a whole.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    sha256_reader(&mut file)
}

pub(crate) fn read_limited<R: Read>(
    reader: &mut R,
    limit: u64,
) -> std::result::Result<Vec<u8>, ReleaseError> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| ReleaseError::invalid("release body size overflow"))?;
        if total > limit {
            return Err(ReleaseError::invalid(format!(
                "release body exceeds {} bytes",
                limit
            )));
        }
        output.extend_from_slice(&buffer[..read]);
    }
    Ok(output)
}

pub(crate) fn verify_bytes(
    bytes: &[u8],
    expected_size: u64,
    expected_sha256: &str,
    label: &str,
) -> std::result::Result<(), ReleaseError> {
    if bytes.len() as u64 != expected_size {
        return Err(ReleaseError::invalid(format!(
            "{label} size mismatch: expected {expected_size}, got {}",
            bytes.len()
        )));
    }
    let actual = sha256_bytes(bytes);
    if actual != expected_sha256 {
        return Err(ReleaseError::invalid(format!(
            "{label} SHA-256 mismatch: expected {expected_sha256}, got {actual}"
        )));
    }
    Ok(())
}

pub(crate) fn path_is_safe_directory(path: &Path) -> std::result::Result<(), ReleaseError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(ReleaseError::archive(format!(
            "refusing to extract through symlink {}",
            path.display()
        ))),
        Ok(metadata) if !metadata.is_dir() => Err(ReleaseError::archive(format!(
            "extraction destination is not a directory: {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(path)?;
            let metadata = std::fs::symlink_metadata(path)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(ReleaseError::archive(format!(
                    "invalid extraction destination: {}",
                    path.display()
                )));
            }
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_published_vector() {
        assert_eq!(
            sha256_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn reader_hash_is_streaming_equivalent() {
        let bytes = (0..=255u8).cycle().take(100_003).collect::<Vec<_>>();
        let expected = sha256_bytes(&bytes);
        let mut reader = std::io::Cursor::new(bytes);
        assert_eq!(sha256_reader(&mut reader).unwrap(), expected);
    }
}
